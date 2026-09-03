//! Tests for the bridge protocol: what it asks, what it denies outright.

//! Tests for the permission bridge and its policy decisions.
//!
//! Their own file rather than an inline module: permission.rs was the largest
//! file in the repo, and two thirds of it was this.

use super::*;
use super::test_support::*;

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
    assert!(
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "cancelled" } })),
            )
            .await,
        "the id must match a pending request -- otherwise the await below hangs \
         instead of failing"
    );

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
