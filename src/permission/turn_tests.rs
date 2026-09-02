//! Tests for the turn boundary: what happens to a request whose turn ended.

//! Tests for the permission bridge and its policy decisions.
//!
//! Their own file rather than an inline module: permission.rs was the largest
//! file in the repo, and two thirds of it was this.

use super::*;
use super::test_support::*;

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

/// Forgetting a session drops its conversation binding as well as its
/// answers, and the drain can do that to a session that has just been
/// readmitted -- the cancel in `keep_session_answers` only reaches a forget
/// still sitting in the queue. The question that matters is what the loss of
/// the binding costs, since a misrouted hook would be a real defect where an
/// extra prompt is not.
///
/// It costs nothing. A session is readmitted by its own turn, and the adapter
/// runs one turn at a time, so throughout the window where the binding is
/// missing that session is the active one and the fallback resolves to it by
/// construction. The binding is re-registered at turn teardown
/// (`adapter.rs`), so the window closes with the turn. The fallback can only
/// name a *different* session if some other session's turn is running, which
/// is the thing serialization rules out.
#[tokio::test]
async fn a_forgotten_binding_still_reaches_its_own_running_turn() {
    let workspace = std::env::temp_dir().join("agy-acp-forgotten-binding-test");
    std::fs::create_dir_all(&workspace).unwrap();
    let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;

    // The drain wins the race: the binding goes while session-1's turn runs.
    bridge.forget_session("session-1").await;
    assert!(
        !bridge
            .state
            .lock()
            .await
            .conversations
            .contains_key("conv-1"),
        "the binding must actually be gone for this to prove anything"
    );

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

    // Asked, not denied, and asked of the session whose turn is running.
    let request = expect_permission_request(&mut rx).await;
    assert_eq!(request["params"]["sessionId"], "session-1");
    bridge
        .resolve_response(
            &request["id"],
            Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_once" } })),
        )
        .await;
    assert_eq!(asking.await.unwrap().0, Decision::Allow);
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
