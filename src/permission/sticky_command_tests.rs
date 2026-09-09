//! Sticky-approval tests specific to `run_command`.

use super::test_support::*;
use super::*;

/// GNU `stat -t` is a bare switch. Its following filesystem operand must be
/// extracted before a remembered `safe:stat` allow is honoured.
#[tokio::test]
async fn always_allow_stat_does_not_cover_gnu_terse_path_outside_workspace() {
    let workspace = std::env::temp_dir().join("agy-acp-stat-terse-test");
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
                        "args": { "CommandLine": "stat", "Cwd": cwd },
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
            bridge.decide(&json!({
                "conversationId": "conv-1",
                "toolCall": { "name": "run_command", "args": { "CommandLine": "stat -t /etc/passwd", "Cwd": cwd } },
            })).await
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

/// Extracted operands need an explicit `Cwd` for containment. A leading tilde
/// is home-relative too; use a non-sensitive name to prove containment rather
/// than pattern matching.
#[tokio::test]
async fn always_allow_ls_rechecks_operands_and_requires_cwd() {
    let workspace = std::env::temp_dir().join("agy-acp-home-operand-test");
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
                        "args": { "CommandLine": "ls", "Cwd": cwd },
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
        tokio::spawn(async move {
            bridge
                .decide(&json!({
                    "conversationId": "conv-1",
                    "toolCall": {
                        "name": "run_command",
                        "args": {
                            "CommandLine": "ls ~/ordinary-not-sensitive.txt",
                            "Cwd": cwd,
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
    assert_eq!(asking.await.unwrap().0, Decision::Deny);

    let asking = {
        let bridge = bridge.clone();
        tokio::spawn(async move {
            bridge
                .decide(&json!({
                    "conversationId": "conv-1",
                    "toolCall": {
                        "name": "run_command",
                        "args": { "CommandLine": "ls ordinary.txt" },
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

/// Unclassifiable commands keep exact-string stickiness beside widened keys.
#[tokio::test]
async fn unclassifiable_commands_keep_exact_string_keys() {
    let workspace = std::env::temp_dir().join("agy-acp-fallback-key-test");
    std::fs::create_dir_all(&workspace).unwrap();
    let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;
    let cwd = workspace.display().to_string();

    let first = {
        let bridge = bridge.clone();
        let cwd = cwd.clone();
        tokio::spawn(async move {
            bridge.decide(&json!({ "conversationId": "conv-1", "toolCall": { "name": "run_command", "args": { "CommandLine": "cat file", "Cwd": cwd } } })).await
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

    for (command, option, expected) in [
        ("cat >x", "allow_always", Decision::Allow),
        ("cat >y", "reject_once", Decision::Deny),
    ] {
        let asking = {
            let bridge = bridge.clone();
            let cwd = cwd.clone();
            tokio::spawn(async move {
                bridge.decide(&json!({ "conversationId": "conv-1", "toolCall": { "name": "run_command", "args": { "CommandLine": command, "Cwd": cwd } } })).await
            })
        };
        let request = expect_permission_request(&mut rx).await;
        bridge
            .resolve_response(
                &request["id"],
                Some(json!({ "outcome": { "outcome": "selected", "optionId": option } })),
            )
            .await;
        assert_eq!(asking.await.unwrap().0, expected);
    }

    let (decision, _) = expect_auto_decision(&bridge, json!({ "conversationId": "conv-1", "toolCall": { "name": "run_command", "args": { "CommandLine": "cat >x", "Cwd": cwd } } })).await;
    assert_eq!(decision, Decision::Allow);
}

/// A classified command with an unaudited extra field remains fingerprint-keyed.
#[tokio::test]
async fn extra_args_fields_fall_back_to_fingerprint() {
    let workspace = std::env::temp_dir().join("agy-acp-extra-field-test");
    std::fs::create_dir_all(&workspace).unwrap();
    let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;
    let cwd = workspace.display().to_string();

    let first = {
        let bridge = bridge.clone();
        let cwd = cwd.clone();
        tokio::spawn(async move {
            bridge.decide(&json!({ "conversationId": "conv-1", "toolCall": { "name": "run_command", "args": { "CommandLine": "ls", "Cwd": cwd } } })).await
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
            bridge.decide(&json!({ "conversationId": "conv-1", "toolCall": { "name": "run_command", "args": { "CommandLine": "ls", "Cwd": cwd, "Shell": "zsh" } } })).await
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

    let (decision, _) = expect_auto_decision(&bridge, json!({ "conversationId": "conv-1", "toolCall": { "name": "run_command", "args": { "CommandLine": "ls", "Cwd": cwd } } })).await;
    assert_eq!(decision, Decision::Allow);
}

/// A fingerprint-keyed rejection remains narrower than a later program allow.
#[tokio::test]
async fn program_allow_is_uncontaminated_by_an_earlier_deny() {
    let workspace = std::env::temp_dir().join("agy-acp-deny-split-test");
    let other = workspace.join("other");
    std::fs::create_dir_all(&other).unwrap();
    let (bridge, mut rx) = test_bridge(&workspace.display().to_string(), &[]).await;
    let cwd = workspace.display().to_string();

    let reject = {
        let bridge = bridge.clone();
        let cwd = cwd.clone();
        tokio::spawn(async move {
            bridge.decide(&json!({ "conversationId": "conv-1", "toolCall": { "name": "run_command", "args": { "CommandLine": "ls -z", "Cwd": cwd } } })).await
        })
    };
    let request = expect_permission_request(&mut rx).await;
    let options = request["params"]["options"].as_array().unwrap();
    assert_eq!(
        options
            .iter()
            .find(|o| o["kind"] == "reject_always")
            .unwrap()["name"],
        "Always reject this exact command this session"
    );
    bridge
        .resolve_response(
            &request["id"],
            Some(json!({ "outcome": { "outcome": "selected", "optionId": "reject_always" } })),
        )
        .await;
    assert_eq!(reject.await.unwrap().0, Decision::Deny);

    let (decision, reason) = expect_auto_decision(&bridge, json!({ "conversationId": "conv-1", "toolCall": { "name": "run_command", "args": { "CommandLine": "ls -z", "Cwd": cwd } } })).await;
    assert_eq!(decision, Decision::Deny);
    assert_eq!(
        reason,
        "Always rejected this exact command in this session."
    );

    let allow = {
        let bridge = bridge.clone();
        let cwd = cwd.clone();
        tokio::spawn(async move {
            bridge.decide(&json!({ "conversationId": "conv-1", "toolCall": { "name": "run_command", "args": { "CommandLine": "ls", "Cwd": cwd } } })).await
        })
    };
    let request = expect_permission_request(&mut rx).await;
    let options = request["params"]["options"].as_array().unwrap();
    assert_eq!(
        options
            .iter()
            .find(|o| o["kind"] == "allow_always")
            .unwrap()["name"],
        "Always allow `ls` commands this session"
    );
    bridge
        .resolve_response(
            &request["id"],
            Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_always" } })),
        )
        .await;
    assert_eq!(allow.await.unwrap().0, Decision::Allow);

    let (decision, reason) = expect_auto_decision(&bridge, json!({ "conversationId": "conv-1", "toolCall": { "name": "run_command", "args": { "CommandLine": "ls other", "Cwd": cwd } } })).await;
    assert_eq!(decision, Decision::Allow);
    assert_eq!(reason, "Always allowed `ls` commands in this session.");

    let (decision, reason) = expect_auto_decision(&bridge, json!({ "conversationId": "conv-1", "toolCall": { "name": "run_command", "args": { "CommandLine": "ls -z", "Cwd": cwd } } })).await;
    assert_eq!(decision, Decision::Deny);
    assert_eq!(
        reason,
        "Always rejected this exact command in this session."
    );
}
