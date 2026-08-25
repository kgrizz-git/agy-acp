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

/// Remembered "always" answers, keyed by session and tool name.
///
/// In memory only, and never cleared: they are scoped to one session id and die
/// with the process, so a reloaded session in a fresh process asks again.
///
/// Tool name, not arguments: one "always allow" on `run_command` covers every
/// later command in the session. Containment and sensitive-path checks still run
/// on a remembered allow, but they read arguments as paths and a command line is
/// one opaque string, so they do not constrain it. Documented in the README and
/// tracked in TODO.md.
type AlwaysKey = (String, String);

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
    pending: HashMap<String, oneshot::Sender<Value>>,
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
        if state.active_session.is_some() {
            state.refused_during_prompt = false;
        }
    }

    /// Whether the user refused a tool call during the prompt that just ran.
    pub async fn refused_during_prompt(&self) -> bool {
        self.state.lock().await.refused_during_prompt
    }

    async fn mark_user_refusal(&self) {
        self.state.lock().await.refused_during_prompt = true;
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
        if let Some(tx) = state.pending.remove(key) {
            let _ = tx.send(result.unwrap_or_else(|| json!({})));
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

        let session_id = {
            let state = self.state.lock().await;
            match state
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
            }
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

        let always_key = (session_id.clone(), tool_name.clone());
        // Copied out before the branch: the body awaits the same mutex, and an
        // `if let` scrutinee guard would still be held inside it.
        let remembered = { self.state.lock().await.always.get(&always_key).copied() };
        if let Some(decision) = remembered {
            // A remembered deny applies immediately and unchanged.
            if decision == Decision::Deny {
                self.mark_user_refusal().await;
                return (
                    Decision::Deny,
                    format!("Always rejected `{tool_name}` in this session."),
                );
            }
            // A remembered allow is only honoured for calls the bridge itself would
            // wave through. One that leaves the workspace or names something
            // sensitive still goes to the user — the original allow never covered
            // that, so it must not become a permanent bypass.
            if !self.escapes_containment(&args).await {
                return (
                    Decision::Allow,
                    format!("Always allowed `{tool_name}` in this session."),
                );
            }
        }

        let request_id = format!("{REQUEST_ID_PREFIX}{}", Uuid::new_v4());
        let (tx, rx) = oneshot::channel();
        self.state
            .lock()
            .await
            .pending
            .insert(request_id.clone(), tx);

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
                "options": permission_options(&tool_name),
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
            Ok(Ok(value)) => value,
            _ => {
                self.state.lock().await.pending.remove(&request_id);
                self.mark_user_refusal().await;
                return (
                    Decision::Deny,
                    "agy-acp: timed out waiting for a permission decision".to_string(),
                );
            }
        };

        self.apply_outcome(&outcome, always_key, &tool_name).await
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
    ) -> (Decision, String) {
        let outcome = outcome.get("outcome").unwrap_or(outcome);
        let kind = outcome
            .get("outcome")
            .and_then(|v| v.as_str())
            .unwrap_or("cancelled");
        if kind != "selected" {
            self.mark_user_refusal().await;
            return (
                Decision::Deny,
                "Permission request was cancelled.".to_string(),
            );
        }

        let option_id = outcome
            .get("optionId")
            .and_then(|v| v.as_str())
            .unwrap_or(OPTION_REJECT_ONCE);

        let (decision, sticky, reason) = match option_id {
            OPTION_ALLOW_ONCE => (Decision::Allow, false, "Approved by user.".to_string()),
            OPTION_ALLOW_ALWAYS => (
                Decision::Allow,
                true,
                format!("Approved by user; always allowing `{tool_name}` in this session."),
            ),
            OPTION_REJECT_ALWAYS => (
                Decision::Deny,
                true,
                format!("Declined by user; always rejecting `{tool_name}` in this session."),
            ),
            _ => (Decision::Deny, false, "Declined by user.".to_string()),
        };

        if decision == Decision::Deny {
            self.mark_user_refusal().await;
        }

        if sticky {
            self.state.lock().await.always.insert(always_key, decision);
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

/// The four answers offered with every prompt.
///
/// `kind` is the ACP enum the host styles on; `name` is free display text and is
/// ours to word. The "always" labels name the tool and say "this session"
/// because that is what the answer covers -- every later call to that tool, for
/// this session only -- and the prompt is where someone decides, not the README.
fn permission_options(tool_name: &str) -> Value {
    json!([
        { "optionId": OPTION_ALLOW_ONCE, "name": "Allow once", "kind": "allow_once" },
        {
            "optionId": OPTION_ALLOW_ALWAYS,
            "name": format!("Always allow {tool_name} this session"),
            "kind": "allow_always",
        },
        { "optionId": OPTION_REJECT_ONCE, "name": "Reject", "kind": "reject_once" },
        {
            "optionId": OPTION_REJECT_ALWAYS,
            "name": format!("Always reject {tool_name} this session"),
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
        return absolute.into_iter().next().or_else(|| {
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
    async fn expect_permission_request(
        rx: &mut mpsc::UnboundedReceiver<Option<String>>,
    ) -> Value {
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

        // Second call: same tool, so the remembered reject must apply with no new
        // prompt — regardless of what the arguments are.
        let (decision, _) = bridge
            .decide(&json!({
                "conversationId": "conv-1",
                "toolCall": { "name": "run_command", "args": { "CommandLine": "rm -rf /" } },
            }))
            .await;
        assert_eq!(decision, Decision::Deny);
        assert!(
            rx.try_recv().is_err(),
            "the remembered reject must not prompt again"
        );
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
    async fn the_always_options_say_what_they_cover() {
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
            options
                .iter()
                .find(|o| o["kind"] == kind)
                .unwrap()["name"]
                .as_str()
                .unwrap()
                .to_string()
        };
        // The answer covers the tool for the session, so the label has to say so:
        // this prompt is where someone decides, and it shows only one command.
        assert_eq!(named("allow_always"), "Always allow run_command this session");
        assert_eq!(named("reject_always"), "Always reject run_command this session");
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

    /// Pins a known gap, tracked in TODO.md: sticky answers are keyed by tool
    /// name, so approving one command approves every later command. This test
    /// asserts today's behaviour deliberately -- when the gap is closed it will
    /// fail, which is the point: the fix must come with a doc change.
    #[tokio::test]
    async fn always_allow_is_remembered_per_tool_not_per_command() {
        let workspace = std::env::temp_dir().join("agy-acp-always-per-tool-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) =
            bridge_with_run_command_always_allowed(&workspace.display().to_string()).await;

        // A completely different, destructive command. "Always allow" was never
        // asked about this one, and the user is not consulted.
        let (decision, reason) = bridge
            .decide(&json!({
                "conversationId": "conv-1",
                "toolCall": { "name": "run_command", "args": { "CommandLine": "rm -rf build" } },
            }))
            .await;
        assert_eq!(decision, Decision::Allow, "{reason}");
        assert!(
            rx.try_recv().is_err(),
            "today the user is not asked again -- this is the gap, not the goal"
        );
    }

    /// Pins the other half of the same gap: a command is one opaque string, so
    /// the containment check never sees the path inside it. `absolute_paths`
    /// keeps strings that *start with* `/`, and "cat /etc/shadow" does not.
    #[tokio::test]
    async fn a_path_inside_a_command_string_is_invisible_to_the_containment_check() {
        let workspace = std::env::temp_dir().join("agy-acp-command-path-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let (bridge, mut rx) =
            bridge_with_run_command_always_allowed(&workspace.display().to_string()).await;

        let outside = json!({
            "conversationId": "conv-1",
            "toolCall": { "name": "run_command", "args": { "CommandLine": "cat /etc/shadow" } },
        });
        assert!(
            outside_workspace(
                &outside["toolCall"]["args"],
                &[PathBuf::from(&workspace)]
            )
            .is_none(),
            "the embedded path is not recognised as a path at all"
        );

        let (decision, reason) = bridge.decide(&outside).await;
        assert_eq!(decision, Decision::Allow, "{reason}");
        assert!(rx.try_recv().is_err(), "and so the user is not asked");

        // The denylist is what happens to catch the neighbouring case, by
        // substring and not by containment -- a second line, not the defence.
        let asking = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .decide(&json!({
                        "conversationId": "conv-1",
                        "toolCall": {
                            "name": "run_command",
                            "args": { "CommandLine": "cat /etc/passwd" },
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
