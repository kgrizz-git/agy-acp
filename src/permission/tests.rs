//! Tests for the bridge protocol: what it asks, what it denies outright.

//! Tests for the permission bridge and its policy decisions.
//!
//! Their own file rather than an inline module: permission.rs was the largest
//! file in the repo, and two thirds of it was this.

use super::test_support::*;
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

/// The five names this fork inherited from upstream vocabulary and that agy was
/// never observed to emit. Classifying one as read/edit/search would give it the
/// weaker tool-level sticky key through `KEYED_BY_TOOL_KINDS`, so if agy ever
/// does emit them they have to arrive as unknowns and be keyed by arguments.
#[test]
fn tools_agy_was_never_seen_to_emit_are_not_pre_classified() {
    for tool in [
        "view_code_item",
        "codebase_search",
        "edit_file",
        "propose_code",
        "command_status",
    ] {
        assert_eq!(
            tool_kind(tool),
            "other",
            "{tool} must not be pre-classified"
        );
        assert!(
            !KEYED_BY_TOOL_KINDS.contains(&tool_kind(tool)),
            "{tool} must be keyed by its arguments, not by its name"
        );
        assert!(
            sticky_scope(tool, &json!({ "AbsolutePath": "/tmp/a" })).is_some(),
            "{tool} must get the argument-level sticky key"
        );
    }
}

/// agy self-reports these but no payload has carried one. They reach `"other"`
/// by falling through, which is the safe answer -- `schedule` runs its work
/// in-turn but a name-only key would still cover a schedule of a different
/// duration, and `invoke_subagent` spawns an agent whose calls reach this same
/// hook, so neither may inherit an answer remembered for a different call.
#[test]
fn self_reported_but_unobserved_tools_stay_unknown() {
    for tool in [
        "manage_task",
        "send_message",
        "schedule",
        "invoke_subagent",
        "define_subagent",
        "manage_subagents",
        "generate_image",
    ] {
        assert_eq!(tool_kind(tool), "other", "{tool} should still be unknown");
        assert!(
            sticky_scope(tool, &json!({ "Prompt": "go" })).is_some(),
            "{tool} must be keyed by its arguments"
        );
    }
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
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_once" } })),
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

/// A subagent runs under its own `conversationId`, which the bridge has never
/// registered. It must fall back to the active session and be gated there --
/// not rejected as "no session", which would break every subagent tool call.
/// The no-active-session case is covered by `unknown_conversations_are_denied`;
/// this pins the fallback arm the plan flagged as untested.
#[tokio::test]
async fn an_unknown_conversation_falls_back_to_the_active_session() {
    let workspace = std::env::temp_dir().join(format!("agy-acp-fallback-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&workspace);
    let file = workspace.join("f.txt");
    let _ = std::fs::write(&file, "f");

    // Two registered sessions, and "session-2" is the one whose turn is running.
    // This is what makes the assertion mean "the *active* session", not merely
    // "some registered session": a fallback that picked any registered session
    // could pick "session-1", and then the turn-generation check
    // (`active_session != session_id`) would deny instead of allow. Only a
    // fallback to the active session passes.
    let (bridge, _rx) = test_bridge(&workspace.display().to_string(), &["view_file"]).await;
    bridge.register_conversation("conv-2", "session-2").await;
    bridge.set_active_session(Some("session-2")).await;

    let (decision, reason) = expect_auto_decision(
        &bridge,
        json!({
            "conversationId": "subagent-unregistered-999",
            "toolCall": {
                "name": "view_file",
                "args": { "AbsolutePath": file.display().to_string() },
            },
        }),
    )
    .await;

    assert_eq!(
        decision,
        Decision::Allow,
        "an unknown conversationId must resolve to the *active* session (session-2), \
         not another registered one and not a deny: {reason}"
    );
}
