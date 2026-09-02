//! Tests for the auto-allow policy, sensitivity and workspace containment.

//! Tests for the permission bridge and its policy decisions.
//!
//! Their own file rather than an inline module: permission.rs was the largest
//! file in the repo, and two thirds of it was this.

use super::*;
use super::test_support::*;

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
