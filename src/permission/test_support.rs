//! Bridge fixtures shared by the permission test modules.

//! Tests for the permission bridge and its policy decisions.
//!
//! Their own file rather than an inline module: permission.rs was the largest
//! file in the repo, and two thirds of it was this.

use super::*;

/// Waits for the bridge to ask the user, failing fast if it never does.
///
/// A missing prompt is the interesting failure here -- it means a check was
/// bypassed -- and without a timeout that shows up as the whole suite
/// hanging rather than as a red test.
pub(super) async fn expect_permission_request(rx: &mut mpsc::UnboundedReceiver<Option<String>>) -> Value {
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
pub(super) async fn expect_auto_decision(bridge: &PermissionBridge, payload: Value) -> (Decision, String) {
    tokio::time::timeout(std::time::Duration::from_secs(5), bridge.decide(&payload))
        .await
        .expect("the bridge must decide on its own, not ask the user")
}

/// Builds a bridge wired to a session, a workspace and an explicit policy.
pub(super) async fn test_bridge(
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
