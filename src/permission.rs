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
            state: Arc::new(Mutex::new(BridgeState::default())),
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

    /// Marks the session whose prompt is running, for the duration of that prompt.
    pub async fn set_active_session(&self, session_id: Option<&str>) {
        let mut state = self.state.lock().await;
        state.active_session = session_id.map(str::to_string);
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

        let always_key = (session_id.clone(), tool_name.clone());
        if let Some(decision) = self.state.lock().await.always.get(&always_key).copied() {
            let reason = match decision {
                Decision::Allow => format!("Always allowed `{tool_name}` in this session."),
                Decision::Deny => format!("Always rejected `{tool_name}` in this session."),
            };
            return (decision, reason);
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
                "options": permission_options(),
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
                return (
                    Decision::Deny,
                    "agy-acp: timed out waiting for a permission decision".to_string(),
                );
            }
        };

        self.apply_outcome(&outcome, always_key, &tool_name).await
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

fn permission_options() -> Value {
    json!([
        { "optionId": OPTION_ALLOW_ONCE, "name": "Allow", "kind": "allow_once" },
        { "optionId": OPTION_ALLOW_ALWAYS, "name": "Always allow", "kind": "allow_always" },
        { "optionId": OPTION_REJECT_ONCE, "name": "Reject", "kind": "reject_once" },
        { "optionId": OPTION_REJECT_ALWAYS, "name": "Always reject", "kind": "reject_always" },
    ])
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

        let raw = rx.recv().await.unwrap().unwrap();
        let request: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(request["method"], "session/request_permission");
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
        let raw = rx.recv().await.unwrap().unwrap();
        let request: Value = serde_json::from_str(&raw).unwrap();
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

        let raw = rx.recv().await.unwrap().unwrap();
        let request: Value = serde_json::from_str(&raw).unwrap();
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
