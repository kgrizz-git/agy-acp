//! Tests for stream-json parsing into ACP notifications.

use serde_json::Value;

use crate::streaming::StreamProcessor;

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

#[test]
fn stream_processor_survives_invalid_utf8_in_a_line() {
    use crate::streaming::StreamProcessor;

    // A byte slice with an invalid UTF-8 byte (0xff) decodes to U+FFFD via
    // from_utf8_lossy, so one bad line must not stop the stream. The bad byte
    // sits outside any JSON string, so the decoded line really is unparseable --
    // inside one it would still deserialize, and the test would pass on a line
    // that never reached the error path.
    let bad_bytes = b"{\"event\":\xff\"result\",\"result\":{\"status\":\"SUCCESS\"}}";
    let bad_line = String::from_utf8_lossy(bad_bytes);
    assert!(
        bad_line.contains('\u{fffd}'),
        "precondition: line carries a replacement char"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(&bad_line).is_err(),
        "precondition: the decoded line does not parse"
    );

    let mut processor = StreamProcessor::new(false);
    let from_bad = processor.process_line(&bad_line, "sess-1");
    assert!(
        from_bad.is_empty(),
        "a malformed line yields no notifications"
    );
    assert!(
        !processor.saw_result,
        "a line that never parsed cannot have completed the turn"
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
