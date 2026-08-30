use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

use crate::adapter::{filter_narration, Adapter};
use crate::protobuf::{
    extract_text_from_step_payload, extract_tool_name, extract_tool_update_from_step_payload,
    extract_user_text_from_step_payload, is_tool_step_type, read_varint,
};
use crate::streaming::StreamProcessor;
use crate::tools::tool_kind;
use crate::types::AgyModel;
use crate::Cli;
use clap::Parser;

fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        if value < 128 {
            out.push(value as u8);
            break;
        }
        out.push(((value as u8) & 0x7F) | 0x80);
        value >>= 7;
    }
}

fn push_len_field(out: &mut Vec<u8>, field_number: u64, bytes: &[u8]) {
    push_varint(out, (field_number << 3) | 2);
    push_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn push_varint_field(out: &mut Vec<u8>, field_number: u64, value: u64) {
    push_varint(out, field_number << 3);
    push_varint(out, value);
}

fn make_assistant_payload(text: &str) -> Vec<u8> {
    let mut inner = Vec::new();
    push_len_field(&mut inner, 1, text.as_bytes());

    let mut outer = Vec::new();
    push_len_field(&mut outer, 20, &inner);
    outer
}

fn make_user_payload(text: &str) -> Vec<u8> {
    let mut content = Vec::new();
    push_len_field(&mut content, 1, text.as_bytes());

    let mut prompt = Vec::new();
    push_len_field(&mut prompt, 2, text.as_bytes());
    push_len_field(&mut prompt, 3, &content);

    let mut outer = Vec::new();
    push_len_field(&mut outer, 19, &prompt);
    outer
}

fn make_tool_payload(
    call_id: &str,
    tool_name: &str,
    input_json: &str,
    summary: &str,
    result_field: Option<(u64, Vec<u8>)>,
) -> Vec<u8> {
    let mut call = Vec::new();
    push_len_field(&mut call, 1, call_id.as_bytes());
    push_len_field(&mut call, 2, tool_name.as_bytes());
    push_len_field(&mut call, 3, input_json.as_bytes());
    push_len_field(&mut call, 9, tool_name.as_bytes());

    let mut tool = Vec::new();
    push_len_field(&mut tool, 4, &call);
    push_len_field(&mut tool, 30, summary.as_bytes());

    let mut outer = Vec::new();
    push_varint_field(&mut outer, 1, 7);
    push_len_field(&mut outer, 5, &tool);
    if let Some((field, result)) = result_field {
        push_len_field(&mut outer, field, &result);
    }
    outer
}

fn process_lines(
    skip_naration: bool,
    session_id: &str,
    lines: &[&str],
) -> (StreamProcessor, Vec<Value>) {
    let mut processor = StreamProcessor::new(skip_naration);
    let mut updates = Vec::new();
    for line in lines {
        for notification in processor.process_line(line, session_id) {
            let parsed: Value = serde_json::from_str(&notification).unwrap();
            updates.push(parsed["params"]["update"].clone());
        }
    }
    (processor, updates)
}

fn test_adapter() -> Adapter {
    Adapter::new_for_test()
}

#[test]
fn test_parse_skip_naration_flag() {
    assert!(
        Cli::try_parse_from(["agy-acp", "--skip-naration"])
            .unwrap()
            .skip_naration
    );
    assert!(!Cli::try_parse_from(["agy-acp"]).unwrap().skip_naration);
    assert!(Cli::try_parse_from(["agy-acp", "--skip-narration"]).is_err());
}

#[test]
fn test_extract_text_from_step_payload_field20_field1() {
    let mut inner = Vec::new();
    inner.push(0x0A);
    inner.push(0x05);
    inner.extend_from_slice(b"hello");

    let mut blob = vec![0x08, 0x0F, 0xA2, 0x01, inner.len() as u8];
    blob.extend_from_slice(&inner);
    assert_eq!(
        extract_text_from_step_payload(&blob),
        Some("hello".to_string())
    );
}

#[test]
fn test_extract_text_returns_none_without_field20() {
    let blob = vec![0x08, 0x03];
    assert_eq!(extract_text_from_step_payload(&blob), None);
}

#[test]
fn test_extract_user_text_from_step_payload_field19_field2() {
    let payload = make_user_payload("how are you?");
    assert_eq!(
        extract_user_text_from_step_payload(&payload),
        Some("how are you?".to_string())
    );
}

#[test]
fn test_extract_text_multiline() {
    let text = b"Safe memory rules\nCompiler points out the flaws\nFast and fearless code";
    let mut inner = Vec::new();
    inner.push(0x0A);
    inner.push(text.len() as u8);
    inner.extend_from_slice(text);

    let mut blob = vec![0x08, 0x01, 0xA2, 0x01, inner.len() as u8];
    blob.extend_from_slice(&inner);
    assert_eq!(
        extract_text_from_step_payload(&blob),
        Some(
            "Safe memory rules\nCompiler points out the flaws\nFast and fearless code".to_string()
        )
    );
}

#[test]
fn test_extract_tool_update_from_step_payload_json() {
    let payload = br#"
        grep_search
        {"Query":"prompt","SearchPath":"/tmp/project/src/main.rs","toolAction":"Finding prompt handling","toolSummary":"Grep prompt"}
    "#;

    let update = extract_tool_update_from_step_payload(19, 7, payload).unwrap();
    assert_eq!(update["sessionUpdate"], "tool_call");
    assert_eq!(update["toolCallId"], "agy-19-7");
    assert_eq!(update["title"], "Grep prompt");
    assert_eq!(update["kind"], "search");
    assert_eq!(update["status"], "completed");
    assert_eq!(update["rawInput"]["Query"], "prompt");
    assert_eq!(update["locations"][0]["path"], "/tmp/project/src/main.rs");
}

#[test]
/// Tests that when a tool payload lacks a JSON body (and thus has no `toolSummary`
/// or `toolAction`), the extractor falls back to using the extracted tool name
/// (e.g., `view_file`) as the update title.
fn test_extract_tool_update_uses_tool_name_fallback() {
    let payload = b"view_file";
    let update = extract_tool_update_from_step_payload(3, 8, payload).unwrap();
    assert_eq!(update["title"], "view_file");
    assert_eq!(update["kind"], "read");
}

#[test]
fn test_extract_tool_update_ignores_single_letter_noise() {
    let payload = b"P";
    assert_eq!(extract_tool_update_from_step_payload(4, 17, payload), None);
}

#[test]
fn test_extract_tool_update_ignores_generic_message_fallback() {
    let payload = b"Message";
    assert_eq!(extract_tool_update_from_step_payload(5, 17, payload), None);
}

#[test]
fn test_extract_tool_update_parses_first_balanced_json_object() {
    let payload = br#"
        abc123 view_file
        {"AbsolutePath":"/tmp/project/README.md","toolAction":"Reading README.md","toolSummary":"View README file"}
        trailing render blob {not json}
    "#;

    let update = extract_tool_update_from_step_payload(6, 8, payload).unwrap();
    assert_eq!(update["sessionUpdate"], "tool_call");
    assert_eq!(update["title"], "View README file");
    assert_eq!(update["kind"], "read");
    assert_eq!(update["rawInput"]["AbsolutePath"], "/tmp/project/README.md");
    assert_eq!(update["locations"][0]["path"], "/tmp/project/README.md");
}

#[test]
fn test_extract_tool_update_kind_prefers_tool_name_over_title() {
    let payload = br#"
        view_file
        {"AbsolutePath":"/tmp/project/flow_graph_write_node.go","toolSummary":"View flow_graph_write_node.go"}
    "#;

    let update = extract_tool_update_from_step_payload(7, 8, payload).unwrap();
    assert_eq!(update["title"], "View flow_graph_write_node.go");
    assert_eq!(update["kind"], "read");
}

#[test]
fn test_extract_tool_name_from_embedded_token() {
    assert_eq!(
        extract_tool_name("abc123\tview_file\n{...}"),
        Some("view_file".to_string())
    );
}

#[test]
fn test_extract_tool_update_from_pascal_case_edit_tool() {
    let payload = br#"
        Edit
        {"file_path":"/tmp/project/src/main.rs","old_string":"old","new_string":"new"}
    "#;

    let update = extract_tool_update_from_step_payload(9, 4, payload).unwrap();
    assert_eq!(update["title"], "Edit");
    assert_eq!(update["kind"], "edit");
    assert_eq!(update["rawInput"]["file_path"], "/tmp/project/src/main.rs");
}

#[test]
fn test_extract_tool_update_from_bash_tool() {
    let payload = br#"
        run_command
        {"CommandLine":"cargo test","Cwd":"/tmp/project","toolAction":"Running tests","toolSummary":"Run cargo test"}
    "#;

    let update = extract_tool_update_from_step_payload(10, 21, payload).unwrap();
    assert_eq!(update["title"], "Run cargo test");
    assert_eq!(update["kind"], "execute");
    assert_eq!(update["rawInput"]["CommandLine"], "cargo test");
}

#[test]
fn test_extract_tool_update_from_web_search_step() {
    let payload = br#"
        search_web
        {"query":"FIFA World Cup 2026 dates","toolAction":"Searching World Cup dates","toolSummary":"Search FIFA World Cup 2026 dates"}
    "#;

    assert!(is_tool_step_type(33));
    let update = extract_tool_update_from_step_payload(3, 33, payload).unwrap();
    assert_eq!(update["sessionUpdate"], "tool_call");
    assert_eq!(update["toolCallId"], "agy-3-33");
    assert_eq!(update["title"], "Search FIFA World Cup 2026 dates");
    assert_eq!(update["kind"], "search");
    assert_eq!(update["status"], "completed");
    assert_eq!(update["rawInput"]["query"], "FIFA World Cup 2026 dates");
}

#[test]
fn test_extract_tool_update_maps_reasoning_to_think_content() {
    let payload = br#"
        thinking
        {"thought":"Need to inspect the protocol before changing serialization.","toolSummary":"Reasoning"}
    "#;

    let update = extract_tool_update_from_step_payload(21, 17, payload).unwrap();
    assert_eq!(update["sessionUpdate"], "tool_call");
    assert_eq!(update["toolCallId"], "agy-21-17");
    assert_eq!(update["title"], "Reasoning");
    assert_eq!(update["kind"], "think");
    assert_eq!(update["status"], "completed");
    assert_eq!(update["content"][0]["type"], "content");
    assert_eq!(update["content"][0]["content"]["type"], "text");
    assert_eq!(
        update["content"][0]["content"]["text"],
        "Need to inspect the protocol before changing serialization."
    );
}

#[test]
fn test_extract_tool_update_from_structured_grep_payload() {
    let mut grep = Vec::new();
    push_len_field(&mut grep, 1, b"StepPayload");
    push_len_field(&mut grep, 2, b"src/*.rs");
    push_len_field(&mut grep, 3, b"src/protobuf.rs:1:message StepPayload");
    push_len_field(&mut grep, 10, b"rg StepPayload src");
    push_len_field(&mut grep, 11, b"file:///tmp/project");
    let payload = make_tool_payload(
        "0t0p5kn3",
        "grep_search",
        r#"{"SearchPath":"/tmp/project/src","toolAction":"Searching protobuf schema"}"#,
        "Proto search",
        Some((13, grep)),
    );

    let update = extract_tool_update_from_step_payload(22, 7, &payload).unwrap();
    assert_eq!(update["toolCallId"], "0t0p5kn3");
    assert_eq!(update["title"], "Proto search");
    assert_eq!(update["kind"], "search");
    assert_eq!(update["rawInput"]["SearchPath"], "/tmp/project/src");
    assert_eq!(update["rawOutput"]["query"], "StepPayload");
    assert_eq!(
        update["rawOutput"]["textOutput"],
        "src/protobuf.rs:1:message StepPayload"
    );
    assert_eq!(update["locations"][0]["path"], "/tmp/project/src");
    assert_eq!(
        update["content"][0]["content"]["text"],
        "```\nsrc/protobuf.rs:1:message StepPayload\n```"
    );
}

#[test]
fn test_extract_tool_update_formats_structured_grep_hits_without_text_output() {
    let mut hit = Vec::new();
    push_len_field(&mut hit, 1, b"src/protobuf.rs");
    push_varint_field(&mut hit, 2, 42);
    push_len_field(
        &mut hit,
        3,
        b"fn parse_tool_result(blob: &[u8]) -> Option<Value> {",
    );

    let mut grep = Vec::new();
    push_len_field(&mut grep, 1, b"parse_tool_result");
    push_len_field(&mut grep, 4, &hit);
    let payload = make_tool_payload(
        "grep-hit-call",
        "grep_search",
        r#"{"SearchPath":"/tmp/project/src","toolAction":"Searching parser"}"#,
        "Parser search",
        Some((13, grep)),
    );

    let update = extract_tool_update_from_step_payload(26, 7, &payload).unwrap();
    assert_eq!(
        update["content"][0]["content"]["text"],
        "```\nfield1: src/protobuf.rs | field2: 42 | field3: fn parse_tool_result(blob: &[u8]) -> Option<Value> {\n```"
    );
}

#[test]
fn test_extract_tool_update_from_structured_view_payload() {
    let mut view = Vec::new();
    push_len_field(&mut view, 1, b"file:///tmp/project/src/protobuf.rs");
    push_varint_field(&mut view, 2, 10);
    push_varint_field(&mut view, 3, 12);
    push_len_field(&mut view, 4, b"pub fn read_varint() {}\n```");
    push_varint_field(&mut view, 11, 13);
    push_varint_field(&mut view, 12, 200);
    let payload = make_tool_payload(
        "view-call",
        "view_file",
        "{}",
        "Viewing file",
        Some((14, view)),
    );

    let update = extract_tool_update_from_step_payload(23, 8, &payload).unwrap();
    assert_eq!(update["title"], "Viewing file");
    assert_eq!(update["kind"], "read");
    assert_eq!(
        update["rawOutput"]["fileUri"],
        "file:///tmp/project/src/protobuf.rs"
    );
    assert_eq!(update["rawOutput"]["startLine"], 10);
    assert_eq!(
        update["locations"][0]["path"],
        "file:///tmp/project/src/protobuf.rs"
    );
    assert_eq!(update["locations"][0]["line"], 10);
    assert_eq!(
        update["content"][0]["content"]["text"],
        "````\npub fn read_varint() {}\n```\n````"
    );
}

#[test]
fn test_extract_tool_update_from_structured_list_payload() {
    let mut entry = Vec::new();
    push_len_field(&mut entry, 1, b"src");
    push_varint_field(&mut entry, 2, 1);
    push_varint_field(&mut entry, 4, 0);

    let mut list = Vec::new();
    push_len_field(&mut list, 1, b"file:///tmp/project");
    push_len_field(&mut list, 3, &entry);
    let payload = make_tool_payload(
        "list-call",
        "list_dir",
        "{}",
        "Listing directory",
        Some((15, list)),
    );

    let update = extract_tool_update_from_step_payload(24, 9, &payload).unwrap();
    assert_eq!(update["title"], "Listing directory");
    assert_eq!(update["kind"], "read");
    assert_eq!(update["rawOutput"]["dirUri"], "file:///tmp/project");
    assert_eq!(update["rawOutput"]["entries"][0]["name"], "src");
    assert_eq!(update["rawOutput"]["entries"][0]["isDirectory"], true);
    assert_eq!(update["content"][0]["content"]["text"], "```\nsrc/\n```");
}

#[test]
fn test_extract_tool_update_formats_empty_structured_list_payload() {
    let mut list = Vec::new();
    push_len_field(&mut list, 1, b"file:///tmp/project");
    let payload = make_tool_payload(
        "empty-list-call",
        "list_dir",
        "{}",
        "Listing directory",
        Some((15, list)),
    );

    let update = extract_tool_update_from_step_payload(27, 9, &payload).unwrap();
    assert_eq!(
        update["content"][0]["content"]["text"],
        "```\n(empty directory)\n```"
    );
}

#[test]
fn test_extract_tool_update_from_structured_write_payload() {
    let mut write = Vec::new();
    push_len_field(&mut write, 26, b"Wrote 42 bytes");
    let payload = make_tool_payload(
        "write-call",
        "write_to_file",
        r#"{"AbsolutePath":"/tmp/project/src/main.rs"}"#,
        "Writing file",
        Some((10, write)),
    );

    let update = extract_tool_update_from_step_payload(25, 5, &payload).unwrap();
    assert_eq!(update["title"], "Writing file");
    assert_eq!(update["kind"], "edit");
    assert_eq!(update["rawOutput"]["summary"], "Wrote 42 bytes");
    assert_eq!(update["locations"][0]["path"], "/tmp/project/src/main.rs");
    assert_eq!(
        update["content"][0]["content"]["text"],
        "```\nWrote 42 bytes\n```"
    );
}

#[test]
fn test_read_varint() {
    assert_eq!(read_varint(&[0x05]), Some((5, 1)));
    assert_eq!(read_varint(&[0xAC, 0x02]), Some((300, 2)));
    assert_eq!(read_varint(&[]), None);
}

#[test]
fn test_stream_json_binds_conversation_and_emits_text_deltas() {
    let (processor, updates) = process_lines(
        false,
        "sess-1",
        &[
            r#"{"event":"init","conversation_id":"conv-abc","init":{"cwd":"/tmp"}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"conv-abc","step_index":0,"state":"DONE","step_type":"user_input"}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"conv-abc","step_index":1,"state":"DONE","step_type":"unknown"}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"conv-abc","step_index":2,"state":"ACTIVE","step_type":"agent_response","text_delta":"OK"}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"conv-abc","step_index":2,"state":"DONE","step_type":"agent_response","text_delta":"\n"}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"conv-abc","step_index":3,"state":"DONE","step_type":"checkpoint"}}"#,
            r#"{"event":"result","result":{"conversation_id":"conv-abc","status":"SUCCESS","response":"OK\n"}}"#,
        ],
    );
    assert_eq!(processor.conversation_id.as_deref(), Some("conv-abc"));
    assert_eq!(processor.last_step_idx, 3);
    assert!(processor.had_updates);
    assert_eq!(
        updates
            .iter()
            .map(|u| u["sessionUpdate"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["agent_message_chunk", "agent_message_chunk"]
    );
    assert_eq!(updates[0]["content"]["text"], "OK");
    assert_eq!(updates[1]["content"]["text"], "\n");
}

#[test]
fn test_stream_json_skips_narration_prefix() {
    let (_processor, updates) = process_lines(
        true,
        "sess-1",
        &[
            r#"{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"agent_response","text_delta":"I will inspect the file."}}"#,
            r#"{"event":"step_update","step_update":{"step_index":2,"state":"DONE","step_type":"agent_response","text_delta":"\nHere is the result."}}"#,
        ],
    );
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["sessionUpdate"], "agent_message_chunk");
    assert_eq!(updates[0]["content"]["text"], "\nHere is the result.");
}

#[test]
fn test_stream_json_emits_tool_call_then_update() {
    let (_processor, updates) = process_lines(
        false,
        "sess-1",
        &[
            r#"{"event":"step_update","step_update":{"step_index":3,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hello"}}}}"#,
            r#"{"event":"step_update","step_update":{"step_index":3,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hello"},"output":"hello\n"}}}"#,
        ],
    );
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0]["sessionUpdate"], "tool_call");
    assert_eq!(updates[0]["status"], "in_progress");
    assert_eq!(updates[0]["toolCallId"], "agy-3");
    assert_eq!(updates[0]["kind"], "execute");
    assert_eq!(updates[0]["rawInput"]["CommandLine"], "echo hello");
    assert_eq!(updates[1]["sessionUpdate"], "tool_call_update");
    assert_eq!(updates[1]["status"], "completed");
    assert_eq!(updates[1]["rawOutput"]["output"], "hello\n");
}

#[test]
fn test_stream_json_thinking_tool_emits_thought_chunk() {
    let (_processor, updates) = process_lines(
        false,
        "sess-1",
        &[
            r#"{"event":"step_update","step_update":{"step_index":4,"state":"DONE","step_type":"tool","tool_name":"thinking","tool_info":{"name":"thinking","parameters":{"thought":"Need to inspect the protocol.","toolSummary":"Reasoning"}}}}"#,
        ],
    );
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["sessionUpdate"], "agent_thought_chunk");
    assert_eq!(
        updates[0]["content"]["text"],
        "Need to inspect the protocol."
    );
}

#[test]
fn test_stream_json_result_fallback_when_no_text_delta() {
    let (_processor, updates) = process_lines(
        false,
        "sess-1",
        &[
            r#"{"event":"init","conversation_id":"conv-x","init":{}}"#,
            r#"{"event":"result","result":{"conversation_id":"conv-x","status":"SUCCESS","response":"PONG\n"}}"#,
        ],
    );
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["sessionUpdate"], "agent_message_chunk");
    assert_eq!(updates[0]["content"]["text"], "PONG\n");
}

#[test]
fn test_tool_kind_mapping() {
    assert_eq!(tool_kind("run_command"), "execute");
    assert_eq!(tool_kind("view_file"), "read");
    assert_eq!(tool_kind("write_to_file"), "edit");
    assert_eq!(tool_kind("grep_search"), "search");
    assert_eq!(tool_kind("thinking"), "think");
}

#[test]
fn test_adapter_uses_a_scratch_home_without_model_discovery() {
    let adapter = test_adapter();

    assert!(adapter.state_file.starts_with(std::env::temp_dir()));
    assert!(adapter.conversations_dir.starts_with(std::env::temp_dir()));
    assert!(adapter.available_models.is_empty());
}

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
    };

    adapter.persist_session("sess-1", Some("conv-abc"), 7, None);
    let restored = adapter.restore_session("sess-1");
    assert_eq!(restored, Some(("conv-abc".to_string(), 7, None)));

    let missing = adapter.restore_session("sess-unknown");
    assert_eq!(missing, None);

    let _ = fs::remove_dir_all(root);
}

fn prepare_auth() -> bool {
    if std::env::var("GEMINI_API_KEY")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        eprintln!("[e2e] Using GEMINI_API_KEY");
        return true;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let settings = format!("{}/.gemini/antigravity-cli/settings.json", home);
    if std::path::Path::new(&settings).exists() {
        eprintln!("[e2e] Using local auth (keyring)");
        return true;
    }
    eprintln!("SKIP: No GEMINI_API_KEY and no local auth found");
    false
}

#[test]
#[ignore]
fn test_e2e_agy_acp_full_round_trip() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    if !prepare_auth() {
        return;
    }

    let agy_check = Command::new("agy").arg("--help").output();
    if agy_check.is_err() || !agy_check.unwrap().status.success() {
        eprintln!("SKIP: agy not found in PATH");
        return;
    }

    let binary = std::env::current_dir()
        .unwrap()
        .join("target/release/agy-acp");
    if !binary.exists() {
        panic!("Run `cargo build --release` first");
    }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn agy-acp");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let mut send_and_recv = |msg: &str| -> String {
        writeln!(stdin, "{}", msg).unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        line
    };

    let resp = send_and_recv(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientName":"e2e","clientVersion":"0.1"}}"#,
    );
    let init: Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(init["result"]["protocolVersion"], 1);

    let resp = send_and_recv(r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}"#);
    let session: Value = serde_json::from_str(&resp).unwrap();
    let session_id = session["result"]["sessionId"].as_str().unwrap();
    assert!(!session_id.is_empty());

    let prompt_msg = format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{}","prompt":[{{"type":"text","text":"Reply with exactly one word: PONG"}}]}}}}"#,
        session_id
    );
    writeln!(stdin, "{}", prompt_msg).unwrap();
    stdin.flush().unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let mut got_notification = false;
    let mut response_text = String::new();
    loop {
        if std::time::Instant::now() > deadline {
            panic!("Timed out waiting for agy-acp response");
        }
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line.is_empty() {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        let msg: Value = serde_json::from_str(line.trim()).unwrap();
        if msg.get("method") == Some(&json!("session/update")) {
            got_notification = true;
            response_text = msg["params"]["update"]["content"]["text"]
                .as_str()
                .unwrap_or("")
                .to_string();
        }
        if msg.get("id") == Some(&json!(3)) {
            assert!(msg["error"].is_null(), "Got error: {}", msg["error"]);
            assert_eq!(msg["result"]["stopReason"], "end_turn");
            break;
        }
    }

    drop(stdin);
    let _ = child.wait();

    assert!(got_notification, "Expected session/update notification");
    let lower = response_text.to_lowercase();
    assert!(
        lower.contains("pong"),
        "Expected 'PONG' in response, got: '{}'",
        response_text
    );
}

fn spawn_agy_acp() -> Option<(
    std::process::ChildStdin,
    std::io::BufReader<std::process::ChildStdout>,
    std::process::Child,
)> {
    use std::io::BufReader;
    use std::process::{Command, Stdio};

    if !prepare_auth() {
        return None;
    }
    let agy_check = Command::new("agy").arg("--help").output();
    if agy_check.is_err() || !agy_check.unwrap().status.success() {
        eprintln!("SKIP: agy not found in PATH");
        return None;
    }
    let binary = std::env::current_dir()
        .unwrap()
        .join("target/release/agy-acp");
    if !binary.exists() {
        panic!("Run `cargo build --release` first");
    }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn agy-acp");
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    Some((stdin, BufReader::new(stdout), child))
}

fn send_recv(
    stdin: &mut std::process::ChildStdin,
    reader: &mut std::io::BufReader<std::process::ChildStdout>,
    msg: &str,
) -> String {
    use std::io::{BufRead, Write};
    writeln!(stdin, "{}", msg).unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    line
}

fn send_prompt_wait(
    stdin: &mut std::process::ChildStdin,
    reader: &mut std::io::BufReader<std::process::ChildStdout>,
    id: u64,
    session_id: &str,
    text: &str,
) -> (Option<String>, Value) {
    use std::io::{BufRead, Write};
    use std::time::Duration;

    let msg = format!(
        r#"{{"jsonrpc":"2.0","id":{},"method":"session/prompt","params":{{"sessionId":"{}","prompt":[{{"type":"text","text":"{}"}}]}}}}"#,
        id, session_id, text
    );
    writeln!(stdin, "{}", msg).unwrap();
    stdin.flush().unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let mut notification_text: Option<String> = None;
    loop {
        if std::time::Instant::now() > deadline {
            panic!("Timed out");
        }
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line.is_empty() {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        let msg: Value = serde_json::from_str(line.trim()).unwrap();
        if msg.get("method") == Some(&json!("session/update")) {
            notification_text = msg["params"]["update"]["content"]["text"]
                .as_str()
                .map(String::from);
        }
        if msg.get("id") == Some(&json!(id)) {
            return (notification_text, msg);
        }
    }
}

#[test]
#[ignore]
fn test_e2e_multi_turn() {
    let Some((mut stdin, mut reader, mut child)) = spawn_agy_acp() else {
        return;
    };

    send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientName":"e2e","clientVersion":"0.1"}}"#,
    );

    let resp = send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}"#,
    );
    let session_id = serde_json::from_str::<Value>(&resp).unwrap()["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let (text1, resp1) = send_prompt_wait(
        &mut stdin,
        &mut reader,
        3,
        &session_id,
        "Remember this word: BANANA. Reply OK.",
    );
    assert!(resp1["error"].is_null(), "Turn 1 error: {}", resp1["error"]);
    assert!(text1.is_some());

    let (text2, resp2) = send_prompt_wait(
        &mut stdin,
        &mut reader,
        4,
        &session_id,
        "What word did I ask you to remember? Reply with just that word.",
    );
    assert!(resp2["error"].is_null(), "Turn 2 error: {}", resp2["error"]);
    let reply = text2.unwrap_or_default().to_lowercase();
    assert!(
        reply.contains("banana"),
        "Expected 'BANANA' in multi-turn reply, got: '{}'",
        reply
    );

    drop(stdin);
    let _ = child.wait();
}

#[test]
#[ignore]
fn test_e2e_session_load() {
    let Some((mut stdin, mut reader, mut child)) = spawn_agy_acp() else {
        return;
    };

    send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientName":"e2e","clientVersion":"0.1"}}"#,
    );
    let resp = send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}"#,
    );
    let session_id = serde_json::from_str::<Value>(&resp).unwrap()["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let (_text, resp1) = send_prompt_wait(
        &mut stdin,
        &mut reader,
        3,
        &session_id,
        "Reply with exactly: FIRST_TURN",
    );
    assert!(
        resp1["error"].is_null(),
        "First turn error: {}",
        resp1["error"]
    );

    let (text2, resp2) = send_prompt_wait(
        &mut stdin,
        &mut reader,
        4,
        &session_id,
        "Reply with exactly one word: SECOND",
    );
    assert!(
        resp2["error"].is_null(),
        "Second turn error: {}",
        resp2["error"]
    );
    assert!(text2.is_some(), "Expected response on continued session");

    drop(stdin);
    let _ = child.wait();
}

#[test]
#[ignore]
fn test_e2e_error_paths() {
    let Some((mut stdin, mut reader, mut child)) = spawn_agy_acp() else {
        return;
    };

    send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientName":"e2e","clientVersion":"0.1"}}"#,
    );

    let resp = send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"non-existent-session"}}"#,
    );
    let val: Value = serde_json::from_str(&resp).unwrap();
    assert!(
        !val["error"].is_null(),
        "Expected error for unknown session"
    );

    let resp = send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":3,"method":"bogus/method","params":{}}"#,
    );
    let val: Value = serde_json::from_str(&resp).unwrap();
    assert!(!val["error"].is_null(), "Expected error for unknown method");

    drop(stdin);
    let _ = child.wait();
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

#[test]
fn test_session_new_returns_models() {
    let mut adapter = test_adapter();
    let response = adapter.handle_session_new(json!(1));
    let result = response.result.as_ref().unwrap();
    assert!(result.get("sessionId").is_some());
    let models = result.get("models").unwrap();
    assert!(models.get("currentModelId").is_some());
    assert!(models.get("availableModels").is_some());
    let config_options = result.get("configOptions").unwrap().as_array().unwrap();
    assert_eq!(config_options.len(), 1);
    assert_eq!(config_options[0]["id"].as_str(), Some("model"));
    assert_eq!(config_options[0]["category"].as_str(), Some("model"));
    assert_eq!(config_options[0]["type"].as_str(), Some("select"));
    assert!(config_options[0].get("currentValue").is_some());
    assert!(config_options[0].get("options").is_some());
}

#[test]
fn test_session_set_model() {
    let mut adapter = test_adapter();
    adapter.available_models = test_models();
    let new_resp = adapter.handle_session_new(json!(1));
    let session_id = new_resp.result.as_ref().unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let set_resp = adapter.handle_session_set_model(
        json!(2),
        &json!({"sessionId": session_id, "modelId": "model-b"}),
    );
    assert!(set_resp.error.is_none());
    assert_eq!(
        adapter
            .sessions
            .get(&session_id)
            .unwrap()
            .model_id
            .as_deref(),
        Some("model-b")
    );
}

#[test]
fn test_session_set_model_missing_params() {
    let mut adapter = test_adapter();
    let resp = adapter.handle_session_set_model(json!(1), &json!({}));
    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap()["code"].as_i64(), Some(-32602));
}

#[test]
fn test_session_set_model_unknown_session() {
    let mut adapter = test_adapter();
    let resp = adapter.handle_session_set_model(
        json!(1),
        &json!({"sessionId": "nonexistent", "modelId": "some-model"}),
    );
    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap()["code"].as_i64(), Some(-32000));
}

#[test]
fn test_session_set_config_option_sets_model() {
    let mut adapter = test_adapter();
    adapter.available_models = test_models();
    let new_resp = adapter.handle_session_new(json!(1));
    let session_id = new_resp.result.as_ref().unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let set_resp = adapter.handle_session_set_config_option(
        json!(2),
        &json!({"sessionId": session_id, "configId": "model", "value": "model-b"}),
    );

    assert!(set_resp.error.is_none(), "error: {:?}", set_resp.error);
    assert_eq!(
        adapter
            .sessions
            .get(&session_id)
            .unwrap()
            .model_id
            .as_deref(),
        Some("model-b")
    );
    let config_options = set_resp.result.as_ref().unwrap()["configOptions"]
        .as_array()
        .unwrap();
    assert_eq!(config_options[0]["currentValue"].as_str(), Some("model-b"));
}

#[test]
fn test_session_set_config_option_rejects_unknown_config() {
    let mut adapter = test_adapter();
    let new_resp = adapter.handle_session_new(json!(1));
    let session_id = new_resp.result.as_ref().unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = adapter.handle_session_set_config_option(
        json!(2),
        &json!({"sessionId": session_id, "configId": "not-model", "value": "Model B"}),
    );

    assert!(resp.error.is_some());
    assert_eq!(resp.error.as_ref().unwrap()["code"].as_i64(), Some(-32602));
}

#[test]
#[ignore]
fn test_session_set_model_persists() {
    let root = std::env::temp_dir().join(format!("agy-acp-model-persist-{}", Uuid::new_v4()));
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
    };

    adapter.persist_session("sess-m1", Some("conv-m1"), 0, None);

    adapter.restore_session_state("sess-m1");
    adapter.handle_session_set_model(
        json!(1),
        &json!({"sessionId": "sess-m1", "modelId": "Claude Opus 4.6 (Thinking)"}),
    );

    let adapter2 = Adapter {
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
    };
    let restored = adapter2.restore_session("sess-m1");
    assert_eq!(
        restored,
        Some((
            "conv-m1".to_string(),
            0,
            Some("Claude Opus 4.6 (Thinking)".to_string())
        ))
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_session_load_returns_models() {
    let mut adapter = test_adapter();
    adapter.sessions.insert(
        "test-load".to_string(),
        crate::types::Session {
            conversation_id: None,
            last_step_idx: -1,
            model_id: Some("Gemini 3.1 Pro (High)".to_string()),
            last_used: 0,
        },
    );
    adapter.persist_session(
        "test-load",
        Some("conv-load"),
        -1,
        Some("Gemini 3.1 Pro (High)"),
    );
    adapter.sessions.clear();

    let output = adapter.handle_session_load(json!(1), &json!({"sessionId": "test-load"}));
    let response: Value = serde_json::from_str(output.last().unwrap()).unwrap();
    assert!(
        response["error"].is_null(),
        "error: {:?}",
        response["error"]
    );
    let models = response["result"]["models"].as_object().unwrap();
    assert_eq!(
        models["currentModelId"].as_str(),
        Some("Gemini 3.1 Pro (High)")
    );
    assert_eq!(
        response["result"]["configOptions"][0]["currentValue"].as_str(),
        Some("Gemini 3.1 Pro (High)")
    );
}

#[test]
fn test_session_resume_returns_models() {
    let mut adapter = test_adapter();
    adapter.persist_session(
        "test-resume",
        Some("conv-resume"),
        -1,
        Some("GPT-OSS 120B (Medium)"),
    );
    adapter.sessions.clear();

    let response = adapter.handle_session_resume(json!(1), &json!({"sessionId": "test-resume"}));
    assert!(response.error.is_none(), "error: {:?}", response.error);
    let models = response.result.as_ref().unwrap()["models"]
        .as_object()
        .unwrap();
    assert_eq!(
        models["currentModelId"].as_str(),
        Some("GPT-OSS 120B (Medium)")
    );
    assert_eq!(
        response.result.as_ref().unwrap()["configOptions"][0]["currentValue"].as_str(),
        Some("GPT-OSS 120B (Medium)")
    );
}

#[test]
fn test_session_models_json_default() {
    let mut adapter = test_adapter();
    let models = adapter.session_models_json(None);
    let current = models["currentModelId"].as_str().unwrap();
    if adapter.available_models.is_empty() {
        assert_eq!(current, "");
    } else {
        assert_eq!(current, adapter.available_models[0].id);
    }
}

/// What `agy models` actually prints on stdout: `id<TAB>label`, no header —
/// the "Fetching available models..." banner goes to stderr.
const AGY_MODELS_STDOUT: &str = "\
gemini-3.7-flash-high\tGemini 3.7 Flash (High)
gemini-3.7-flash-low\tGemini 3.7 Flash (Low)
gemini-3.1-pro-high\tGemini 3.1 Pro (High)
claude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)
";

fn test_models() -> Vec<AgyModel> {
    vec![
        AgyModel {
            id: "model-a".to_string(),
            label: "Model A".to_string(),
        },
        AgyModel {
            id: "model-b".to_string(),
            label: "Model B".to_string(),
        },
    ]
}

#[test]
fn test_parse_models_output_splits_id_from_label() {
    let models = Adapter::parse_models_output(AGY_MODELS_STDOUT);
    assert_eq!(models.len(), 4);
    assert_eq!(models[0].id, "gemini-3.7-flash-high");
    assert_eq!(models[0].label, "Gemini 3.7 Flash (High)");
    assert_eq!(models[3].id, "claude-sonnet-4-6");
    assert_eq!(models[3].label, "Claude Sonnet 4.6 (Thinking)");
    for model in &models {
        assert!(
            !model.id.contains('\t') && !model.id.contains(' '),
            "id must be the bare model name agy accepts, got {:?}",
            model.id
        );
    }
}

#[test]
fn test_parse_models_output_without_label_column() {
    let models = Adapter::parse_models_output("gemini-3.7-flash-high\n\ngemini-3.1-pro-low\n");
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gemini-3.7-flash-high");
    assert_eq!(models[0].label, "gemini-3.7-flash-high");
    assert_eq!(models[1].id, "gemini-3.1-pro-low");
}

#[test]
fn test_session_models_json_never_emits_a_label_as_an_id() {
    let mut adapter = test_adapter();
    adapter.available_models = Adapter::parse_models_output(AGY_MODELS_STDOUT);
    let models = adapter.session_models_json(None);
    let available = models["availableModels"].as_array().unwrap();
    assert_eq!(
        available[0]["modelId"].as_str(),
        Some("gemini-3.7-flash-high")
    );
    assert_eq!(
        available[0]["name"].as_str(),
        Some("Gemini 3.7 Flash (High)")
    );
    for entry in available {
        assert!(!entry["modelId"].as_str().unwrap().contains('\t'));
    }
    assert_eq!(
        models["currentModelId"].as_str(),
        Some("gemini-3.7-flash-high")
    );
}

#[test]
fn test_session_models_json_with_model() {
    let mut adapter = test_adapter();
    adapter.available_models = test_models();
    let models = adapter.session_models_json(Some("model-b"));
    assert_eq!(models["currentModelId"].as_str(), Some("model-b"));
    let available = models["availableModels"].as_array().unwrap();
    assert_eq!(available.len(), 2);
    assert_eq!(available[0]["modelId"].as_str(), Some("model-a"));
    assert_eq!(available[0]["name"].as_str(), Some("Model A"));
    assert_eq!(available[1]["modelId"].as_str(), Some("model-b"));
}

#[test]
fn test_session_config_options_json_with_model() {
    let mut adapter = test_adapter();
    adapter.available_models = test_models();
    let config_options = adapter.session_config_options_json(Some("model-b"));
    assert_eq!(config_options[0]["id"].as_str(), Some("model"));
    assert_eq!(config_options[0]["category"].as_str(), Some("model"));
    assert_eq!(config_options[0]["type"].as_str(), Some("select"));
    assert_eq!(config_options[0]["currentValue"].as_str(), Some("model-b"));
    let options = config_options[0]["options"].as_array().unwrap();
    assert_eq!(options.len(), 2);
    assert_eq!(options[0]["value"].as_str(), Some("model-a"));
    assert_eq!(options[0]["name"].as_str(), Some("Model A"));
    assert_eq!(options[1]["value"].as_str(), Some("model-b"));
}

/// A client that stored the old mangled `id<TAB>label` string and sends it back
/// must not have it passed through to `--model`, which agy would reject.
#[test]
fn test_set_model_strips_a_label_glued_to_the_id() {
    let mut adapter = test_adapter();
    adapter.available_models = Adapter::parse_models_output(AGY_MODELS_STDOUT);
    let new_resp = adapter.handle_session_new(json!(1));
    let session_id = new_resp.result.as_ref().unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = adapter.handle_session_set_model(
        json!(2),
        &json!({
            "sessionId": session_id,
            "modelId": "gemini-3.7-flash-high\tGemini 3.7 Flash (High)",
        }),
    );
    assert!(resp.error.is_none(), "error: {:?}", resp.error);
    assert_eq!(
        adapter.sessions[&session_id].model_id.as_deref(),
        Some("gemini-3.7-flash-high")
    );
}

#[test]
fn test_set_model_rejects_a_model_agy_does_not_offer() {
    let mut adapter = test_adapter();
    adapter.available_models = test_models();
    let new_resp = adapter.handle_session_new(json!(1));
    let session_id = new_resp.result.as_ref().unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = adapter.handle_session_set_model(
        json!(2),
        &json!({"sessionId": session_id, "modelId": "Model B"}),
    );
    assert_eq!(resp.error.as_ref().unwrap()["code"].as_i64(), Some(-32602));
    assert_eq!(adapter.sessions[&session_id].model_id, None);
}

#[test]
fn test_set_config_option_rejects_a_model_agy_does_not_offer() {
    let mut adapter = test_adapter();
    adapter.available_models = test_models();
    let new_resp = adapter.handle_session_new(json!(1));
    let session_id = new_resp.result.as_ref().unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = adapter.handle_session_set_config_option(
        json!(2),
        &json!({"sessionId": session_id, "configId": "model", "value": "nope"}),
    );
    assert_eq!(resp.error.as_ref().unwrap()["code"].as_i64(), Some(-32602));
}

#[cfg(test)]
/// The conversation DB is provider-controlled data this adapter only reads, so a
/// corrupted or hostile row must not be able to panic the process. Before the
/// checked-arithmetic pass, the first case here aborted with
/// "slice index starts at 12 but ends at 11".
mod harden_check {
    use crate::protobuf::{
        extract_text_from_step_payload, extract_tool_update_from_step_payload,
        extract_user_text_from_step_payload, get_proto_field, get_text_field, read_varint,
    };

    fn lcg(seed: &mut u64) -> u8 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 33) as u8
    }

    #[test]
    fn malformed_blobs_do_not_panic() {
        // Tags are varints: (20 << 3) | wire_type is 162..165, each two bytes.
        let tag_20_len_delimited = [0xa2u8, 0x01];
        let tag_20_fixed32 = [0xa5u8, 0x01];
        let tag_20_fixed64 = [0xa1u8, 0x01];
        let mut cases: Vec<Vec<u8>> = vec![
            // length = u64::MAX; `i + len` wraps and the old bounds check passed
            [&tag_20_len_delimited[..], &[0xff; 9][..], &[0x01][..]].concat(),
            // length merely points past the end
            [&tag_20_len_delimited[..], &[0x7f][..], b"short"].concat(),
            // truncated varint
            vec![0xff, 0xff, 0xff],
            // fixed32 / fixed64 running off the end
            [&tag_20_fixed32[..], &[0x01][..]].concat(),
            [&tag_20_fixed64[..], &[0x01, 0x02][..]].concat(),
        ];
        let mut seed = 7u64;
        for n in 0..64usize {
            cases.push((0..n).map(|_| lcg(&mut seed)).collect());
        }
        for blob in &cases {
            for target in [1u64, 2, 4, 19, 20, 30] {
                let _ = get_proto_field(blob, target);
                let _ = get_text_field(blob, target);
            }
            let _ = extract_text_from_step_payload(blob);
            let _ = extract_user_text_from_step_payload(blob);
            let _ = extract_tool_update_from_step_payload(0, 132, blob);
            let _ = read_varint(blob);
        }
    }
}

/// A turn ends on the stream's terminal `result` event. Without this flag the
/// adapter cannot tell a completed turn from a truncated one, since a killed agy
/// can still exit 0 after emitting partial output.
#[test]
fn test_stream_json_tracks_whether_the_result_event_arrived() {
    let mut processor = crate::streaming::StreamProcessor::new(false);
    processor.process_line(
        r#"{"event":"step_update","step_update":{"conversation_id":"c1","step_index":0,"text_delta":"partial"}}"#,
        "s1",
    );
    assert!(
        !processor.saw_result,
        "a stream carrying only step updates has not completed"
    );
    assert!(processor.had_updates, "the partial text was still emitted");

    processor.process_line(
        r#"{"event":"result","result":{"conversation_id":"c1","status":"SUCCESS","response":"done"}}"#,
        "s1",
    );
    assert!(processor.saw_result, "the result event completes the turn");
}

/// `result.response` repeats what was streamed, so it is dropped rather than
/// shown twice. Probed against agy 1.1.12: identical, byte for byte, on both a
/// plain and a tool-using turn. This pins the dedup and the empty-result case;
/// divergence is reported on stderr rather than silently swallowed.
#[test]
fn test_stream_json_drops_the_result_text_it_already_streamed() {
    let mut processor = crate::streaming::StreamProcessor::new(false);
    let streamed = processor.process_line(
        r#"{"event":"step_update","step_update":{"conversation_id":"c1","step_index":0,"text_delta":"The answer is 42.\n"}}"#,
        "s1",
    );
    assert_eq!(streamed.len(), 1, "the delta reaches the client once");

    let from_result = processor.process_line(
        r#"{"event":"result","result":{"conversation_id":"c1","status":"SUCCESS","response":"The answer is 42.\n"}}"#,
        "s1",
    );
    assert!(
        from_result.is_empty(),
        "the same text must not be sent a second time"
    );
    assert!(processor.saw_result);
}

/// A turn whose text arrives only in the result -- no deltas -- must still show
/// it, and an empty result must not produce an empty chunk.
#[test]
fn test_stream_json_emits_result_text_when_nothing_was_streamed() {
    let mut processor = crate::streaming::StreamProcessor::new(false);
    let updates = processor.process_line(
        r#"{"event":"result","result":{"conversation_id":"c1","status":"SUCCESS","response":"Answer: 42"}}"#,
        "s1",
    );
    assert_eq!(updates.len(), 1, "the only copy of the text must be sent");
    assert!(updates[0].contains("Answer: 42"));

    let mut empty = crate::streaming::StreamProcessor::new(false);
    let updates = empty.process_line(
        r#"{"event":"result","result":{"conversation_id":"c1","status":"SUCCESS","response":""}}"#,
        "s1",
    );
    assert!(updates.is_empty(), "an empty result is not a message");
    assert!(empty.saw_result, "an empty result still completes the turn");
}

/// Ten groups is the widest a u64 varint can be, and the tenth carries a single
/// bit (9 * 7 = 63). A larger tenth group overflows: before this was rejected,
/// nine continuation bytes followed by `0x02` parsed as 0 rather than failing.
#[test]
fn test_read_varint_rejects_an_overflowing_tenth_group() {
    let mut overflowing = vec![0x80u8; 9];
    overflowing.push(0x02);
    assert_eq!(read_varint(&overflowing), None);

    let mut widest = vec![0x80u8; 9];
    widest.push(0x01);
    assert_eq!(widest.len(), 10);
    assert_eq!(read_varint(&widest), Some((1u64 << 63, 10)));

    let mut too_long = vec![0x80u8; 10];
    too_long.push(0x01);
    assert_eq!(read_varint(&too_long), None);
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

#[test]
fn stream_processor_survives_invalid_utf8_in_a_line() {
    use crate::streaming::StreamProcessor;

    // A byte slice with an invalid UTF-8 byte (0xff) decodes to U+FFFD via
    // from_utf8_lossy, so one bad line must not stop the stream.
    let bad_bytes = b"{\"event\":\"result\",\"result\":{\"status\":\"\xff\"}}";
    let bad_line = String::from_utf8_lossy(bad_bytes);
    assert!(
        bad_line.contains('\u{fffd}'),
        "precondition: line carries a replacement char"
    );

    let mut processor = StreamProcessor::new(false);
    let from_bad = processor.process_line(&bad_line, "sess-1");
    assert!(
        from_bad.is_empty(),
        "a malformed line yields no notifications"
    );

    // A valid event fed right after must still be processed normally.
    let good = r#"{"event":"result","result":{"conversation_id":"conv-x","status":"SUCCESS","response":"OK"}}"#;
    let from_good = processor.process_line(good, "sess-1");
    assert_eq!(
        from_good.len(),
        1,
        "the valid event after the bad one is processed"
    );
    assert!(
        processor.saw_result,
        "result event is still tracked after a bad line"
    );
}

#[tokio::test]
async fn read_until_newline_yields_events_after_invalid_utf8_line() {
    use crate::adapter::read_until_newline;

    // A stream that starts with a line containing an invalid UTF-8 byte, then
    // two valid NDJSON events. We exercise the byte-oriented frame reader
    // directly to prove the later events are still delivered.
    let mut payload: Vec<u8> = Vec::new();
    payload.extend_from_slice(b"{\"event\":\"init\",\"conversation_id\":\"conv-x\"}\xff\n");
    payload.extend_from_slice(b"{\"event\":\"result\",\"result\":{\"conversation_id\":\"conv-x\",\"status\":\"SUCCESS\",\"response\":\"OK\"}}\n");
    payload.extend_from_slice(b"{\"event\":\"step_update\",\"step_update\":{\"step_index\":1,\"state\":\"DONE\",\"step_type\":\"checkpoint\"}}\n");

    let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(payload));
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut buf = Vec::new();
    while read_until_newline(&mut reader, &mut buf).await.unwrap() {
        frames.push(buf.clone());
    }

    assert_eq!(
        frames.len(),
        3,
        "three frames read despite the invalid byte"
    );
    // Each frame is decoded lossily and still parses as JSON after trimming.
    let mut processor = crate::streaming::StreamProcessor::new(false);
    let mut total = 0;
    for frame in &frames {
        let line = String::from_utf8_lossy(frame)
            .trim_end_matches(['\n', '\r'])
            .to_string();
        if line.trim().is_empty() {
            continue;
        }
        total += processor.process_line(&line, "sess-1").len();
    }
    assert!(
        processor.saw_result,
        "result frame survived the invalid byte"
    );
    assert!(
        total >= 1,
        "valid events after the bad line are still emitted"
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
