//! Focused tests for the bounded IPC frame path (plan work item 3).
//!
//! A valid hook request keeps its host prompt and explicit-decision behavior;
//! malformed, semantically empty, oversized, slow, or surplus peers deny
//! without a host request, without unbounded memory/tasks, and never with an
//! allow.

use super::frame::MAX_FRAME_BYTES;
use super::test_support::expect_permission_request;
use super::*;
use crate::runtime::RuntimeOwner;
use serde_json::json;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use uuid::Uuid;

/// A unique scratch base under the real temporary directory. Kept short: the
/// socket path beneath the owner must fit the platform limit under a long
/// macOS temporary directory.
fn scratch_base() -> PathBuf {
    let short = &Uuid::new_v4().simple().to_string()[..8];
    let base = std::env::temp_dir().join(format!("agy-ipc-{short}"));
    std::fs::create_dir_all(&base).unwrap();
    base
}

struct LiveBridge {
    bridge: PermissionBridge,
    owner: RuntimeOwner,
    base: PathBuf,
    rx: mpsc::UnboundedReceiver<Option<String>>,
}

async fn start_live_bridge() -> LiveBridge {
    let base = scratch_base();
    let owner = RuntimeOwner::create_in(&base).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let bridge = PermissionBridge::start(tx, &owner).unwrap();
    bridge.register_conversation("conv-1", "session-1").await;
    bridge.set_active_session(Some("session-1")).await;
    let workspace = base.join("work");
    std::fs::create_dir_all(&workspace).unwrap();
    bridge
        .set_workspace_root(&workspace.display().to_string())
        .await;
    LiveBridge {
        bridge,
        owner,
        base,
        rx,
    }
}

impl LiveBridge {
    fn socket(&self) -> PathBuf {
        self.bridge.socket_path().to_path_buf()
    }

    async fn pending_len(&self) -> usize {
        self.bridge.state.lock().await.pending.len()
    }
}

async fn write_line(stream: &mut UnixStream, line: &str) {
    stream.write_all(line.as_bytes()).await.unwrap();
    stream.write_all(b"\n").await.unwrap();
    stream.flush().await.unwrap();
}

async fn read_response_line(stream: &mut UnixStream) -> String {
    let (read_half, _) = stream.split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
        .await
        .expect("the bridge must answer with one bounded line")
        .unwrap();
    line.trim().to_string()
}

fn valid_payload(step: i64) -> String {
    json!({
        "conversationId": "conv-1",
        "stepIdx": step,
        "toolCall": { "name": "run_command", "args": { "CommandLine": "ls" } },
    })
    .to_string()
}

#[tokio::test]
async fn valid_hook_round_trip_prompts_and_honors_approval() {
    let mut live = start_live_bridge().await;
    let mut client = UnixStream::connect(live.socket()).await.unwrap();
    write_line(&mut client, &valid_payload(0)).await;

    let request = expect_permission_request(&mut live.rx).await;
    assert_eq!(request["params"]["sessionId"], "session-1");
    live.bridge
        .resolve_response(
            &request["id"],
            Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_once" } })),
        )
        .await;

    let response = read_response_line(&mut client).await;
    let value: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(value["decision"], "allow");

    live.bridge.shutdown();
    live.owner.cleanup_handle().cleanup().unwrap();
    std::fs::remove_dir_all(&live.base).unwrap();
}

#[tokio::test]
async fn malformed_frame_denies_without_host_request() {
    for raw in ["not json at all", "", "{", "{\"toolCall\":"] {
        let mut live = start_live_bridge().await;
        let mut client = UnixStream::connect(live.socket()).await.unwrap();
        let line = if raw.is_empty() {
            "   ".to_string()
        } else {
            raw.to_string()
        };
        write_line(&mut client, &line).await;

        let response = read_response_line(&mut client).await;
        let value: Value = serde_json::from_str(&response).expect("a deny is JSON");
        assert_eq!(value["decision"], "deny", "malformed input: {raw:?}");
        assert!(
            live.rx.try_recv().is_err(),
            "malformed input must not ask the host: {raw:?}"
        );
        assert_eq!(
            live.pending_len().await,
            0,
            "nothing may be left behind: {raw:?}"
        );

        live.bridge.shutdown();
        live.owner.cleanup_handle().cleanup().unwrap();
        std::fs::remove_dir_all(&live.base).unwrap();
    }
}

#[tokio::test]
async fn semantically_empty_frame_denies_without_host_request() {
    for raw in [
        "{}",
        r#"{"toolCall":{}}"#,
        r#"{"toolCall":{"name":""}}"#,
        r#"{"toolCall":{"name":"   "}}"#,
        r#"{"toolCall":null}"#,
        r#"{"toolCall":[]}"#,
        r#"{"toolCall":{"name":17}}"#,
        r#"{"conversationId":"conv-1"}"#,
    ] {
        let mut live = start_live_bridge().await;
        let mut client = UnixStream::connect(live.socket()).await.unwrap();
        write_line(&mut client, raw).await;

        let response = read_response_line(&mut client).await;
        let value: Value = serde_json::from_str(&response).expect("a deny is JSON");
        assert_eq!(value["decision"], "deny", "empty input: {raw}");
        assert!(
            value["reason"]
                .as_str()
                .unwrap_or("")
                .contains("names no tool"),
            "the deny should name the cause: {response}"
        );
        assert!(
            live.rx.try_recv().is_err(),
            "an empty tool name must not ask the host: {raw}"
        );
        assert_eq!(live.pending_len().await, 0);

        live.bridge.shutdown();
        live.owner.cleanup_handle().cleanup().unwrap();
        std::fs::remove_dir_all(&live.base).unwrap();
    }
}

#[tokio::test]
async fn oversized_frame_denies_without_host_request() {
    let mut live = start_live_bridge().await;
    let mut client = UnixStream::connect(live.socket()).await.unwrap();
    // One frame just over the shared limit. Valid JSON would be larger still;
    // raw bytes are enough: the size check fires before parsing.
    let big = "x".repeat(MAX_FRAME_BYTES + 64);
    write_line(&mut client, &big).await;

    let response = read_response_line(&mut client).await;
    let value: Value = serde_json::from_str(&response).expect("a deny is JSON");
    assert_eq!(value["decision"], "deny");
    assert!(
        value["reason"]
            .as_str()
            .unwrap_or("")
            .contains("size limit"),
        "the deny should name the cause: {response}"
    );
    assert!(
        live.rx.try_recv().is_err(),
        "an oversized frame must not ask the host"
    );
    assert_eq!(live.pending_len().await, 0);

    live.bridge.shutdown();
    live.owner.cleanup_handle().cleanup().unwrap();
    std::fs::remove_dir_all(&live.base).unwrap();
}

#[tokio::test]
async fn slow_frame_denies_without_host_request() {
    let mut live = start_live_bridge().await;
    // A peer that connects and then stalls: drive the same handler the accept
    // loop runs, with a short deadline so the suite does not wait out the
    // production bound.
    let (peer, server) = UnixStream::pair().unwrap();
    let bridge = live.bridge.clone();
    let serving = tokio::spawn(async move {
        bridge
            .serve_hook_with_timeout(server, Duration::from_millis(150))
            .await;
    });
    // Hold the peer open without sending anything.
    let mut peer = peer;
    let (read_half, _) = peer.split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
        .await
        .expect("a stalled peer must still get its deny")
        .unwrap();
    serving.await.unwrap();

    let value: Value = serde_json::from_str(line.trim()).expect("a deny is JSON");
    assert_eq!(value["decision"], "deny");
    assert!(
        value["reason"].as_str().unwrap_or("").contains("timed out"),
        "the deny should name the cause: {line}"
    );
    assert!(
        live.rx.try_recv().is_err(),
        "a slow frame must not ask the host"
    );
    assert_eq!(live.pending_len().await, 0);

    live.bridge.shutdown();
    live.owner.cleanup_handle().cleanup().unwrap();
    std::fs::remove_dir_all(&live.base).unwrap();
}

#[tokio::test]
async fn eight_permits_then_a_ninth_is_denied_without_more_host_work() {
    let mut live = start_live_bridge().await;
    let mut clients = Vec::new();
    for step in 0..MAX_CONNECTIONS as i64 {
        let mut client = UnixStream::connect(live.socket()).await.unwrap();
        write_line(&mut client, &valid_payload(step)).await;
        clients.push(client);
    }

    // All eight connections reach the host: eight prompts, eight pending.
    let mut request_ids = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        let request = expect_permission_request(&mut live.rx).await;
        request_ids.push(request["id"].clone());
    }
    // Wait for every permit to be held through its pending host answer.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if live.bridge.available_permits() == 0 && live.pending_len().await == MAX_CONNECTIONS {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "all eight permits must be occupied through pending host answers"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // The ninth connection arrives saturated: one bounded deny, no new host
    // request, no new pending entry, and no spawned long-lived task behind it.
    let mut ninth = UnixStream::connect(live.socket()).await.unwrap();
    write_line(&mut ninth, &valid_payload(99)).await;
    let response = read_response_line(&mut ninth).await;
    let value: Value = serde_json::from_str(&response).expect("a deny is JSON");
    assert_eq!(value["decision"], "deny");
    assert!(
        value["reason"].as_str().unwrap_or("").contains("busy"),
        "the deny should say the bridge is busy: {response}"
    );
    // Give a would-be task a moment to misbehave, then pin the absence.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        live.rx.try_recv().is_err(),
        "a saturated peer must not create another host request"
    );
    assert_eq!(
        live.pending_len().await,
        MAX_CONNECTIONS,
        "a saturated peer must not grow pending work"
    );
    assert_eq!(live.bridge.available_permits(), 0);

    // Answering the eight releases their permits with ordinary decisions.
    for id in request_ids {
        live.bridge
            .resolve_response(
                &id,
                Some(json!({ "outcome": { "outcome": "selected", "optionId": "allow_once" } })),
            )
            .await;
    }
    for client in clients.iter_mut() {
        let response = read_response_line(client).await;
        let value: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(value["decision"], "allow");
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if live.bridge.available_permits() == MAX_CONNECTIONS {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "permits must be released when the host answers"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    live.bridge.shutdown();
    live.owner.cleanup_handle().cleanup().unwrap();
    std::fs::remove_dir_all(&live.base).unwrap();
}
