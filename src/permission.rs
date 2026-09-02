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

mod path_rules;
use path_rules::{outside_workspace, string_args};

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

    /// Test-only view of the turn binding. Pinning that a turn's teardown ran
    /// needs to see the binding cleared, not just that no request is pending:
    /// a turn that ends before any tool call still has to bump the generation
    /// on the way out, for the reason set out in `set_active_session`.
    #[cfg(test)]
    pub(crate) async fn active_session(&self) -> Option<String> {
        self.state.lock().await.active_session.clone()
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
/// costs a reprompt instead. Note the contrast with [`path_rules::path_field_args`], which
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

/// Whether the arguments reach somewhere the path checks cannot follow.
///
/// Two shapes, both of which make `escapes_containment` and the sensitive-path
/// list inert: a `CommandLine`, which is one opaque string the shell will
/// reinterpret, and a `Url`, which names a resource that is not on the filesystem
/// at all. A bare `://` anywhere in a string value counts too, so a tool that
/// takes a URL under some other field name is still caught. That last test also
/// fires on a search query that happens to contain `://`, which costs a reprompt
/// and is the direction to be wrong in.
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
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod policy_tests;
#[cfg(test)]
mod sticky_tests;
#[cfg(test)]
mod turn_tests;
