//! Tests for session lifecycle, persistence and eviction.
//! The module-wide `too_many_lines` deny in adapter.rs propagates here. It
//! exists to stop a turn phase growing back into what it was split from; a
//! fixture-heavy test is not that, and splitting one to satisfy it would only
//! scatter the setup a reader needs in one place.
#![allow(clippy::too_many_lines)]

use crate::test_support::*;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

use crate::adapter::{filter_narration, Adapter};
use crate::streaming::StreamProcessor;

#[test]
fn test_adapters_use_distinct_scratch_homes() {
    let first = test_adapter();
    let second = test_adapter();

    assert_ne!(first.state_file, second.state_file);
    assert_ne!(first.conversations_dir, second.conversations_dir);
}

#[test]
fn test_initialize_advertises_load_session_support() {
    let adapter = test_adapter();
    let response = adapter.handle_initialize(json!(1));
    assert_eq!(
        response
            .result
            .as_ref()
            .and_then(|r| r.get("agentCapabilities"))
            .and_then(|c| c.get("loadSession"))
            .and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[test]
fn test_initialize_advertises_resume_capability() {
    let adapter = test_adapter();
    let response = adapter.handle_initialize(json!(1));
    assert!(
        response
            .result
            .as_ref()
            .and_then(|r| r.get("agentCapabilities"))
            .and_then(|c| c.get("sessionCapabilities"))
            .and_then(|sc| sc.get("resume"))
            .is_some(),
        "sessionCapabilities.resume should be present"
    );
}

#[test]
#[ignore]
fn test_session_load_restores_persisted_session() {
    let root = std::env::temp_dir().join(format!("agy-acp-load-{}", Uuid::new_v4()));
    let _ = fs::create_dir_all(&root);

    let mut adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: root.to_string_lossy().to_string(),
        state_file: root.join("sessions.json"),
        conversations_dir: root.join("conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };
    adapter.persist_session("sess-1", Some("conv-abc"), 5, None);

    let output = adapter.handle_session_load(json!(7), &json!({"sessionId": "sess-1"}));
    let response: Value = serde_json::from_str(output.last().unwrap()).unwrap();
    assert!(response["error"].is_null());
    assert_eq!(
        adapter
            .sessions
            .get("sess-1")
            .and_then(|s| s.conversation_id.as_deref()),
        Some("conv-abc")
    );
    assert_eq!(
        adapter.sessions.get("sess-1").map(|s| s.last_step_idx),
        Some(5)
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore]
fn test_session_load_rejects_unknown_session() {
    let root = std::env::temp_dir().join(format!("agy-acp-missing-{}", Uuid::new_v4()));
    let _ = fs::create_dir_all(&root);

    let mut adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: root.to_string_lossy().to_string(),
        state_file: root.join("sessions.json"),
        conversations_dir: root.join("conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };

    let output = adapter.handle_session_load(json!(9), &json!({"sessionId": "missing"}));
    let response: Value = serde_json::from_str(output.last().unwrap()).unwrap();
    assert!(response["result"].is_null());
    assert_eq!(
        response["error"]["message"].as_str(),
        Some("unknown sessionId: missing")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore]
fn test_session_load_replays_conversation_history() {
    let root = std::env::temp_dir().join(format!("agy-acp-load-replay-{}", Uuid::new_v4()));
    let conv_dir = root.join("conversations");
    fs::create_dir_all(&conv_dir).unwrap();

    let db_path = conv_dir.join("conv-replay.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE steps (
            idx INTEGER PRIMARY KEY,
            step_type INTEGER NOT NULL DEFAULT 0,
            status INTEGER NOT NULL DEFAULT 0,
            has_subtrajectory NUMERIC NOT NULL DEFAULT 0,
            metadata BLOB,
            error_details BLOB,
            permissions BLOB,
            task_details BLOB,
            render_info BLOB,
            step_payload BLOB,
            step_format INTEGER NOT NULL DEFAULT 0
        )",
    )
    .unwrap();

    conn.execute(
        "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, 14, ?2)",
        rusqlite::params![1i64, make_user_payload("hello")],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, 15, ?2)",
        rusqlite::params![
            2i64,
            make_assistant_payload("I will inspect the workspace.")
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, 8, ?2)",
        rusqlite::params![
            3i64,
            br#"view_file
            {"AbsolutePath":"/tmp/project/README.md","toolAction":"Reading README.md","toolSummary":"View README file"}
            trailing render blob {not json}"#
                .as_slice()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, 5, ?2)",
        rusqlite::params![
            4i64,
            br#"replace_file_content
            {"AbsolutePath":"/tmp/project/README.md","toolAction":"Editing README.md","toolSummary":"Edit README file"}"#
                .as_slice()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, 21, ?2)",
        rusqlite::params![
            5i64,
            br#"run_command
            {"CommandLine":"cargo test","Cwd":"/tmp/project","toolAction":"Running tests","toolSummary":"Run cargo test"}"#
                .as_slice()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, 15, ?2)",
        rusqlite::params![6i64, make_assistant_payload("hello from agent")],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, 14, ?2)",
        rusqlite::params![7i64, make_user_payload("how are you?")],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, 15, ?2)",
        rusqlite::params![8i64, make_assistant_payload("second response")],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, 14, ?2)",
        rusqlite::params![9i64, make_user_payload("one more question")],
    )
    .unwrap();
    drop(conn);

    let mut adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: root.to_string_lossy().to_string(),
        conversations_dir: conv_dir,
        state_file: root.join("sessions.json"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };
    adapter.persist_session("sess-replay", Some("conv-replay"), 8, None);

    let output = adapter.handle_session_load(json!(1), &json!({"sessionId": "sess-replay"}));
    assert_eq!(
        adapter
            .sessions
            .get("sess-replay")
            .map(|session| session.last_step_idx),
        Some(9)
    );
    let persisted_store: crate::types::SessionStore =
        serde_json::from_str(&fs::read_to_string(root.join("sessions.json")).unwrap()).unwrap();
    assert_eq!(
        persisted_store
            .sessions
            .get("sess-replay")
            .map(|session| session.last_step_idx),
        Some(9)
    );

    assert!(
        output.len() >= 2,
        "expected replay notification + response, got {}",
        output.len()
    );

    let updates: Vec<Value> = output[..output.len() - 1]
        .iter()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(updates.iter().any(|notification| {
        notification["method"] == "session/update"
            && notification["params"]["update"]["sessionUpdate"] == "tool_call"
            && notification["params"]["update"]["title"] == "View README file"
            && notification["params"]["update"]["kind"] == "read"
    }));
    assert!(updates.iter().any(|notification| {
        notification["params"]["update"]["title"] == "Edit README file"
            && notification["params"]["update"]["kind"] == "edit"
    }));
    assert!(updates.iter().any(|notification| {
        notification["params"]["update"]["title"] == "Run cargo test"
            && notification["params"]["update"]["kind"] == "execute"
    }));
    let replay_kinds: Vec<_> = updates
        .iter()
        .map(|notification| {
            notification["params"]["update"]["sessionUpdate"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(
        replay_kinds,
        vec![
            "user_message_chunk",
            "agent_message_chunk",
            "tool_call",
            "tool_call",
            "tool_call",
            "agent_message_chunk",
            "user_message_chunk",
            "agent_message_chunk",
            "user_message_chunk"
        ]
    );
    let message_updates: Vec<_> = updates
        .iter()
        .filter(|notification| {
            matches!(
                notification["params"]["update"]["sessionUpdate"].as_str(),
                Some("user_message_chunk") | Some("agent_message_chunk")
            )
        })
        .collect();
    let update_kinds: Vec<_> = message_updates
        .iter()
        .map(|notification| {
            notification["params"]["update"]["sessionUpdate"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(
        update_kinds,
        vec![
            "user_message_chunk",
            "agent_message_chunk",
            "agent_message_chunk",
            "user_message_chunk",
            "agent_message_chunk",
            "user_message_chunk"
        ]
    );
    let message_texts: Vec<_> = message_updates
        .iter()
        .map(|notification| {
            notification["params"]["update"]["content"]["text"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(
        message_texts,
        vec![
            "hello",
            "I will inspect the workspace.",
            "hello from agent",
            "how are you?",
            "second response",
            "one more question"
        ]
    );
    assert!(
        message_texts[1].contains("I will inspect"),
        "load replay should preserve narration shown in the live session"
    );

    let response: Value = serde_json::from_str(output.last().unwrap()).unwrap();
    assert!(response["error"].is_null());
    assert_eq!(
        response["result"]["sessionId"].as_str(),
        Some("sess-replay")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore]
fn test_session_resume_restores_persisted_session() {
    let root = std::env::temp_dir().join(format!("agy-acp-resume-{}", Uuid::new_v4()));
    let _ = fs::create_dir_all(&root);

    let mut adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: root.to_string_lossy().to_string(),
        state_file: root.join("sessions.json"),
        conversations_dir: root.join("conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };
    adapter.persist_session("sess-r1", Some("conv-xyz"), 3, None);

    let response = adapter.handle_session_resume(json!(10), &json!({"sessionId": "sess-r1"}));
    assert!(response.error.is_none());
    assert_eq!(
        response
            .result
            .as_ref()
            .and_then(|r| r.get("sessionId"))
            .and_then(|s| s.as_str()),
        Some("sess-r1")
    );
    assert_eq!(
        adapter
            .sessions
            .get("sess-r1")
            .and_then(|s| s.conversation_id.as_deref()),
        Some("conv-xyz")
    );
    assert_eq!(
        adapter.sessions.get("sess-r1").map(|s| s.last_step_idx),
        Some(3)
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore]
fn test_session_resume_rejects_unknown_session() {
    let root = std::env::temp_dir().join(format!("agy-acp-resume-miss-{}", Uuid::new_v4()));
    let _ = fs::create_dir_all(&root);

    let mut adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: root.to_string_lossy().to_string(),
        state_file: root.join("sessions.json"),
        conversations_dir: root.join("conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };

    let response = adapter.handle_session_resume(json!(11), &json!({"sessionId": "nope"}));
    assert!(response.result.is_none());
    assert_eq!(
        response
            .error
            .as_ref()
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str()),
        Some("unknown sessionId: nope")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_session_resume_rejects_empty_session_id() {
    let mut adapter = test_adapter();
    let response = adapter.handle_session_resume(json!(12), &json!({}));
    assert!(response.result.is_none());
    assert_eq!(
        response
            .error
            .as_ref()
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_i64()),
        Some(-32602)
    );
}

#[test]
fn test_session_resume_accepts_in_memory_session() {
    let mut adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: "/tmp".to_string(),
        state_file: PathBuf::from("/tmp/nonexistent-agy-acp-sessions.json"),
        conversations_dir: PathBuf::from("/tmp/conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };
    adapter.sessions.insert(
        "sess-memory".to_string(),
        crate::types::Session {
            conversation_id: None,
            last_step_idx: -1,
            model_id: None,
            last_used: 0,
        },
    );

    let response = adapter.handle_session_resume(json!(12), &json!({"sessionId": "sess-memory"}));

    assert!(response.error.is_none());
    assert_eq!(
        response
            .result
            .as_ref()
            .and_then(|r| r.get("sessionId"))
            .and_then(|s| s.as_str()),
        Some("sess-memory")
    );
}

#[test]
fn test_session_load_accepts_in_memory_session_without_replay() {
    let mut adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: "/tmp".to_string(),
        state_file: PathBuf::from("/tmp/nonexistent-agy-acp-sessions.json"),
        conversations_dir: PathBuf::from("/tmp/conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };
    adapter.sessions.insert(
        "sess-memory-load".to_string(),
        crate::types::Session {
            conversation_id: None,
            last_step_idx: -1,
            model_id: None,
            last_used: 0,
        },
    );

    let output = adapter.handle_session_load(json!(13), &json!({"sessionId": "sess-memory-load"}));

    assert_eq!(output.len(), 1);
    let response: Value = serde_json::from_str(&output[0]).unwrap();
    assert!(response["error"].is_null());
    assert_eq!(response["result"]["sessionId"], "sess-memory-load");
}

#[test]
#[ignore]
fn test_session_resume_does_not_replay_history() {
    let root = std::env::temp_dir().join(format!("agy-acp-resume-noreplay-{}", Uuid::new_v4()));
    let _ = fs::create_dir_all(&root);

    let mut adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: root.to_string_lossy().to_string(),
        state_file: root.join("sessions.json"),
        conversations_dir: root.join("conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };
    adapter.persist_session("sess-nr", Some("conv-nr"), 10, None);

    let response = adapter.handle_session_resume(json!(13), &json!({"sessionId": "sess-nr"}));
    assert!(response.error.is_none());
    assert_eq!(
        response
            .result
            .as_ref()
            .and_then(|r| r.get("sessionId"))
            .and_then(|s| s.as_str()),
        Some("sess-nr")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore]
fn test_persist_and_restore_session() {
    let root = std::env::temp_dir().join(format!("agy-acp-state-{}", Uuid::new_v4()));
    let _ = fs::create_dir_all(&root);

    let adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: root.to_string_lossy().to_string(),
        state_file: root.join("sessions.json"),
        conversations_dir: root.join("conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };

    adapter.persist_session("sess-1", Some("conv-abc"), 7, None);
    let restored = adapter.restore_session("sess-1");
    assert_eq!(restored, Some(("conv-abc".to_string(), 7, None)));

    let missing = adapter.restore_session("sess-unknown");
    assert_eq!(missing, None);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_is_narration_true() {
    assert!(Adapter::is_narration("I will fetch the latest commits."));
    assert!(Adapter::is_narration("I'll fetch the latest commits."));
    assert!(Adapter::is_narration("I’ll fetch the latest commits."));
    assert!(Adapter::is_narration(
        "I will fetch the latest commits.\nI'll check the diff."
    ));
    assert!(Adapter::is_narration(
        "I will read the file.\n\nI will analyze the output."
    ));
}

#[test]
fn test_is_narration_false() {
    assert!(!Adapter::is_narration("Here is the result."));
    assert!(!Adapter::is_narration(
        "I will fetch the commits.\nHere is the result."
    ));
    assert!(!Adapter::is_narration(""));
}

#[test]
fn test_filter_narration_drops_all_narration() {
    let parts = vec![
        "I will fetch the latest commits.\nI will check the diff.".to_string(),
        "I will read the file.".to_string(),
        "The fix is confirmed! LGTM ✅".to_string(),
    ];
    let result = filter_narration(&parts);
    assert_eq!(result.as_deref(), Some("The fix is confirmed! LGTM ✅"));
}

#[test]
fn test_filter_narration_preserves_content_after_first_non_narration() {
    let parts = vec![
        "I will check things.".to_string(),
        "Here is my analysis.".to_string(),
        "I will also note this is fine.".to_string(),
    ];
    let result = filter_narration(&parts);
    assert_eq!(result.as_deref(), Some("Here is my analysis."));
}

#[test]
fn test_filter_narration_single_part_unchanged() {
    let parts = vec!["I will do something.".to_string()];
    let result = Adapter::filter_narration(&parts);
    assert_eq!(result, None);
}

#[test]
fn test_filter_narration_all_narration_drops_all() {
    let parts = vec![
        "I will fetch the file.".to_string(),
        "I'll check the output.".to_string(),
        "I will verify the fix.".to_string(),
    ];
    let result = filter_narration(&parts);
    assert_eq!(result, None);
}

/// The binding that matters survives a prune that drops the unbindable entries
/// first, so a live conversation is never lost to make room for dead ones.
#[test]
fn persist_session_prunes_unbindable_entries_first() {
    let root = std::env::temp_dir().join(format!("agy-acp-prune-{}", Uuid::new_v4()));
    let _ = fs::create_dir_all(&root);

    let adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: root.to_string_lossy().to_string(),
        state_file: root.join("sessions.json"),
        conversations_dir: root.join("conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };
    for i in 0..300 {
        // Most entries can never be resumed; they must be the first to go.
        let conv = if i == 50 {
            Some(format!("conv-bound-{i}"))
        } else {
            None
        };
        adapter.persist_session(&format!("sess-{i}"), conv.as_deref(), 0, None);
    }

    let file = fs::read_to_string(root.join("sessions.json")).unwrap();
    let store: crate::types::SessionStore = serde_json::from_str(&file).unwrap();
    assert!(
        store.sessions.len() <= 256,
        "persisted sessions must stay under the cap, got {}",
        store.sessions.len()
    );
    assert!(
        store.sessions.contains_key("sess-50"),
        "the bound entry must survive the prune"
    );
    assert_eq!(
        store
            .sessions
            .values()
            .filter(|s| s.conversation_id.is_some())
            .count(),
        1,
        "the single bound entry must be kept while null entries are dropped first"
    );
    assert!(
        store.sessions.len() < 300,
        "null-conversation entries must have been dropped to meet the cap"
    );

    let _ = fs::remove_dir_all(root);
}

/// The entry just written must not be evicted even when the store is over cap.
#[test]
fn persist_session_keeps_the_entry_it_just_wrote() {
    let root = std::env::temp_dir().join(format!("agy-acp-keep-{}", Uuid::new_v4()));
    let _ = fs::create_dir_all(&root);

    let adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: root.to_string_lossy().to_string(),
        state_file: root.join("sessions.json"),
        conversations_dir: root.join("conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };
    for i in 0..400 {
        adapter.persist_session(
            &format!("old-{i}"),
            Some(format!("conv-{i}").as_str()),
            0,
            None,
        );
    }
    adapter.persist_session("just-written", Some("conv-just"), 0, None);

    let store: crate::types::SessionStore =
        serde_json::from_str(&fs::read_to_string(root.join("sessions.json")).unwrap()).unwrap();
    assert!(
        store.sessions.contains_key("just-written"),
        "the session written last must still be present after pruning"
    );

    let _ = fs::remove_dir_all(root);
}

/// Entries written before `updated_at` existed deserialize as 0 and are thus the
/// elders when pruning — backwards compatibility without a migration step.
#[test]
fn stored_sessions_without_updated_at_load_as_oldest() {
    let root = std::env::temp_dir().join(format!("agy-acp-legacy-{}", Uuid::new_v4()));
    let _ = fs::create_dir_all(&root);

    let json = json!({
        "sessions": {
            "sess-a": { "conversation_id": "conv-a", "last_step_idx": 1 },
            "sess-b": { "conversation_id": "conv-b", "last_step_idx": 2 },
        }
    });
    fs::write(
        root.join("sessions.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();

    let adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: root.to_string_lossy().to_string(),
        state_file: root.join("sessions.json"),
        conversations_dir: root.join("conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };
    let store = adapter.load_store();
    assert_eq!(store.sessions.len(), 2, "both legacy entries should load");
    assert!(
        store.sessions.values().all(|s| s.updated_at == 0),
        "missing updated_at must default to 0"
    );

    let _ = fs::remove_dir_all(root);
}

/// Eviction drops the least-recently-used session, leaving a freshly touched one
/// alive even when it was inserted before the victim.
#[test]
fn evict_if_needed_drops_the_least_recently_used_session() {
    use crate::types::Session;

    let mut adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: "/tmp".to_string(),
        state_file: PathBuf::from("/tmp/nonexistent-agy-acp-sessions.json"),
        conversations_dir: PathBuf::from("/tmp/conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };
    for i in 0..64 {
        adapter.sessions.insert(
            format!("sess-{i}"),
            Session {
                conversation_id: None,
                last_step_idx: -1,
                model_id: None,
                last_used: i as u64,
            },
        );
    }
    // Mark one of the earliest as most-recently-used.
    adapter.sessions.get_mut("sess-1").unwrap().last_used = 1000;
    adapter.sessions.insert(
        "sess-new".to_string(),
        Session {
            conversation_id: None,
            last_step_idx: -1,
            model_id: None,
            last_used: 1001,
        },
    );
    adapter.evict_if_needed();

    assert!(
        adapter.sessions.contains_key("sess-1"),
        "a touched (recently used) session must survive eviction"
    );
    assert!(
        adapter.sessions.contains_key("sess-new"),
        "the newest session must survive eviction"
    );
    assert!(
        !adapter.sessions.contains_key("sess-0"),
        "the untouched, oldest-used session is the one evicted"
    );
}

/// Stays a plain `#[test]` with no runtime, which is the point of the queue:
/// `evict_if_needed` is sync and must not need one.
#[test]
fn evicting_a_session_queues_its_answers_for_forgetting() {
    use crate::types::Session;

    let mut adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: "/tmp".to_string(),
        state_file: PathBuf::from("/tmp/nonexistent-agy-acp-sessions.json"),
        conversations_dir: PathBuf::from("/tmp/conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };
    for i in 0..64 {
        adapter.sessions.insert(
            format!("sess-{i}"),
            Session {
                conversation_id: None,
                last_step_idx: -1,
                model_id: None,
                last_used: i as u64,
            },
        );
    }
    adapter.evict_if_needed();

    // Exactly the victim, not merely containing it: `contains` would pass an
    // implementation that queued every session id and forgot far more than it
    // should.
    let queued = adapter.pending_forget.lock().unwrap().clone();
    assert_eq!(
        queued,
        vec!["sess-0".to_string()],
        "only the evicted session may be queued"
    );
}

/// An evicted id can be readmitted before the drain runs -- `session/load`,
/// `session/resume` and prompt restoration all take a caller-supplied id out of
/// `sessions.json`, so only `session/new` mints a fresh one. Without this the
/// queue is one race away from clearing a live session's answers.
///
/// "Before the drain" is the whole of what this pins, and the whole of what
/// `keep_session_answers` can promise: the cancel is a removal from the queue,
/// so it does nothing once the drain has taken the queue and is awaiting the
/// bridge lock. That residual window is left open on purpose -- see the drain in
/// `main.rs` -- because it can only forget an answer, never grant one.
#[test]
fn readmitting_an_evicted_session_cancels_its_queued_forget() {
    use crate::types::Session;

    let dir = std::env::temp_dir().join(format!("agy-acp-readmit-{}", Uuid::new_v4()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut adapter = Adapter {
        sessions: HashMap::new(),
        working_dir: dir.to_string_lossy().to_string(),
        state_file: dir.join("sessions.json"),
        conversations_dir: dir.join("conversations"),
        available_models: vec![],
        skip_naration: false,
        permission_bridge: None,
        hook_root_dir: None,
        session_tick: 0,
        live_children: Default::default(),
        agy_bin: "agy".to_string(),
        pending_forget: Default::default(),
    };
    // Persisted, so it can be restored after eviction.
    adapter.persist_session("sess-0", Some("conv-abc"), 5, None);
    for i in 0..64 {
        adapter.sessions.insert(
            format!("sess-{i}"),
            Session {
                conversation_id: None,
                last_step_idx: -1,
                model_id: None,
                last_used: i as u64,
            },
        );
    }
    adapter.evict_if_needed();
    assert_eq!(
        adapter.pending_forget.lock().unwrap().clone(),
        vec!["sess-0".to_string()],
        "precondition: the eviction queued it"
    );

    // The host asks for it again before the dispatcher drains.
    assert!(adapter.restore_session_state("sess-0"));

    assert!(
        adapter.pending_forget.lock().unwrap().is_empty(),
        "the queued forget must be cancelled when the session comes back"
    );
    // The safety property, not just the queue state: queue state alone would pass
    // an implementation that cancels the forget and drops the answers anyway.
    assert!(
        adapter.sessions.contains_key("sess-0"),
        "and the session is live again"
    );
}

#[tokio::test]
async fn stream_notifications_go_through_the_output_channel() {
    use tokio::sync::mpsc::unbounded_channel;

    let (notify_tx, mut notify_rx) = unbounded_channel::<Option<String>>();
    let mut processor = StreamProcessor::new(false);

    let frames = [
        r#"{"event":"init","conversation_id":"conv-abc","init":{"cwd":"/tmp"}}"#,
        r#"{"event":"step_update","step_update":{"conversation_id":"conv-abc","step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"OK"}}"#,
        r#"{"event":"step_update","step_update":{"conversation_id":"conv-abc","step_index":3,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hello"}}}}"#,
        r#"{"event":"result","result":{"conversation_id":"conv-abc","status":"SUCCESS","response":"OK"}}"#,
    ];

    for frame in frames {
        crate::adapter::publish_stream_notifications(&mut processor, &notify_tx, frame, "sess-1");
    }
    drop(notify_tx);

    let mut count = 0;
    while let Some(value) = notify_rx.recv().await {
        // (a) the drain task must never send the pending-prompt sentinel.
        assert!(
            value.is_some(),
            "notification channel received None, which corrupts pending_prompts"
        );
        let notification = value.unwrap();
        // (b) every value is a complete JSON-RPC session/update notification.
        let parsed: Value = serde_json::from_str(&notification)
            .unwrap_or_else(|e| panic!("notification is not valid JSON: {e}: {notification}"));
        assert_eq!(parsed["method"], "session/update");
        assert!(parsed["params"]["update"].is_object());
        count += 1;
    }
    assert!(count >= 1, "at least one notification was emitted");
}
