//! Turn-lifecycle tests: spawn, drain, cancel and teardown, driven by stub
//! binaries through `Adapter::agy_bin` rather than a real agy.

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

use crate::adapter::Adapter;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::sync::Arc;

/// An executable `/bin/sh` stub standing in for agy, removed when the test
/// drops it. The scratch directory is per-call, so concurrent tests cannot
/// see each other's stub; the `Drop` is what keeps a test run from leaving
/// one directory per turn behind in the temp dir.
struct StubAgy {
    dir: PathBuf,
    bin: PathBuf,
}

impl StubAgy {
    fn bin(&self) -> String {
        self.bin.display().to_string()
    }
}

impl Drop for StubAgy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// The stub receives the turn's real arguments but ignores them; a test that
/// needs to assert on them has to write them out from `body` itself.
fn stub_agy(body: &str) -> StubAgy {
    let dir = std::env::temp_dir().join(format!("agy-acp-stub-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("agy-stub");
    fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    StubAgy { dir, bin }
}

/// `PermissionBridge::start` binds one socket per *process*, and it unlinks the
/// path before binding. Two bridge tests in this binary therefore race: the
/// loser's bind lands after the winner's unlink and fails with EEXIST. Held for
/// the whole test rather than just the `start` call, because the bridge owns
/// that path until it drops -- and a `tokio` mutex rather than a `std` one,
/// since it is held across the `await` on the turn.
static BRIDGE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Runs one turn against `stub`, returning the response lines and the
/// adapter, so a test can assert on live children after the turn.
async fn run_turn(adapter: &mut Adapter, cancelled: Arc<AtomicBool>) -> Vec<String> {
    run_turn_for(adapter, "sess-1", cancelled).await
}

/// For the tests that need a session the adapter actually knows about, rather
/// than the bare id most of these turns get away with.
async fn run_turn_for(
    adapter: &mut Adapter,
    session_id: &str,
    cancelled: Arc<AtomicBool>,
) -> Vec<String> {
    let (notify_tx, _notify_rx) = tokio::sync::mpsc::unbounded_channel();
    adapter
        .handle_session_prompt(
            json!(1),
            &json!({ "sessionId": session_id, "prompt": [{ "text": "hello" }] }),
            cancelled,
            notify_tx,
        )
        .await
}

fn sole_response(lines: &[String]) -> Value {
    assert_eq!(lines.len(), 1, "expected exactly one response line");
    serde_json::from_str(&lines[0]).unwrap()
}

/// A spawn that never happens still has to answer the request, and must not
/// leave a pid registered -- `handle_session_prompt` returns before the
/// `child_guard` is ever taken on this path.
#[tokio::test]
async fn spawn_failure_answers_with_an_error_and_registers_no_child() {
    let mut adapter = Adapter::new_for_test();
    adapter.agy_bin = "/nonexistent/agy-acp-not-a-real-binary".to_string();

    let lines = run_turn(&mut adapter, Arc::new(AtomicBool::new(false))).await;

    let response = sole_response(&lines);
    assert_eq!(response["error"]["code"], -32000);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("failed to run agy"),
        "unexpected message: {}",
        response["error"]["message"]
    );
    assert!(response["result"].is_null());
    assert_eq!(adapter.live_children().len(), 0);
}

/// A cancelled turn reports `cancelled`, not a provider failure, and the
/// killed child is unregistered rather than left behind. The flag is flipped
/// mid-turn rather than pre-set, so a poll loop that stopped watching after
/// the first read would fail this.
#[tokio::test]
async fn cancel_ends_the_turn_as_cancelled() {
    let mut adapter = Adapter::new_for_test();
    let stub = stub_agy("sleep 30");
    adapter.agy_bin = stub.bin();

    // Flipped after the turn is under way, so the poll loop has to observe
    // the false -> true transition. Set before the call it would also pass
    // against a loop that only ever reads the flag once, at entry.
    let cancelled = Arc::new(AtomicBool::new(false));
    let flip = Arc::clone(&cancelled);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        flip.store(true, Ordering::SeqCst);
    });
    let lines = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_turn(&mut adapter, cancelled),
    )
    .await
    .expect("a cancelled turn must not outlive its child");

    let response = sole_response(&lines);
    assert_eq!(
        response["result"]["stopReason"], "cancelled",
        "got {response}"
    );
    assert!(response["error"].is_null());
    assert_eq!(adapter.live_children().len(), 0);
}

/// Closing stdout does not make a child undrainable. The drain loop reads EOF
/// and stops
/// (the `Ok(false) => break` arm of the drain loop), which is a child that has stopped
/// talking rather than a pipe nobody can read, so the turn waits for it and
/// ends on the child's own terms. It still answers with an error here, but
/// the *missing result event* one -- a verdict only reachable after a real
/// wait. Pinned because the obvious "unreadable stdout" stub is this one, and
/// treating it as undrainable would kill turns that are still working.
///
/// The `undrainable` flag covers a different case: the read
/// itself erroring *and* the follow-up drain to a sink also failing. No shell
/// stub can produce that, so it stays uncovered rather than faked; the Phase 1
/// section of plans/split-large-files.md records why.
#[tokio::test]
async fn a_child_that_closes_stdout_is_waited_for_not_killed() {
    let mut adapter = Adapter::new_for_test();
    let stub = stub_agy("exec 1>&-; sleep 0.2");
    adapter.agy_bin = stub.bin();

    let lines = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_turn(&mut adapter, Arc::new(AtomicBool::new(false))),
    )
    .await
    .expect("turn did not finish");

    // Reached the child's own exit rather than a kill: a killed stub could
    // not have produced the "no result event" verdict, which is only
    // reachable after a successful `child.wait()`.
    let response = sole_response(&lines);
    assert_eq!(
        response["error"]["message"], "agy stream ended without a result event",
        "expected the turn to wait the child out, got {response}"
    );
    assert_eq!(adapter.live_children().len(), 0);
}

/// A cancel still wins over a child that has closed stdout and is hanging:
/// the poll loop leaves through the kill branch and `was_cancelled` is
/// re-read from the cancel flag after the poll loop, so this is a cancel and
/// not the failure the previous test pins.
#[tokio::test]
async fn cancel_wins_over_a_child_hanging_with_stdout_closed() {
    let mut adapter = Adapter::new_for_test();
    let stub = stub_agy("exec 1>&-; sleep 30");
    adapter.agy_bin = stub.bin();

    let cancelled = Arc::new(AtomicBool::new(true));
    let lines = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run_turn(&mut adapter, cancelled),
    )
    .await
    .expect("turn did not finish; the adapter left the undrainable child running");

    let response = sole_response(&lines);
    assert_eq!(
        response["result"]["stopReason"], "cancelled",
        "got {response}"
    );
    assert!(response["error"].is_null());
    assert_eq!(adapter.live_children().len(), 0);
}

/// A child that exits non-zero with text on stderr must answer with a
/// -32000 error whose message carries the stderr text -- not a
/// `stopReason: end_turn` success, which is what a clean exit looks like
/// to the client.
#[tokio::test]
async fn non_zero_exit_answers_with_an_error() {
    let mut adapter = Adapter::new_for_test();
    let stub = stub_agy("echo boom 1>&2; exit 3");
    adapter.agy_bin = stub.bin();

    let lines = run_turn(&mut adapter, Arc::new(AtomicBool::new(false))).await;

    let response = sole_response(&lines);
    assert_eq!(
        response["error"]["code"], -32000,
        "got {response}"
    );
    let message = response["error"]["message"]
        .as_str()
        .unwrap_or("");
    assert!(
        message.contains("boom"),
        "stderr text 'boom' should appear in the error message, got: {message}"
    );
    assert!(response["result"].is_null());
}

/// A turn that streamed at least one notification before failing must
/// still answer with an error, not a successful end_turn. The old gate
/// was `had_updates`, so any turn that produced a single chunk before
/// failing reported end_turn -- and the client could not tell the bad
/// turn from a good one. Pins the `!was_cancelled && !denied_by_user` gate that replaced that
/// gate.
#[tokio::test]
async fn a_turn_that_streams_then_fails_still_reports_an_error() {
    let mut adapter = Adapter::new_for_test();
    let frame = r#"{"event":"step_update","step_update":{"conversation_id":"conv-abc","step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"hello"}}"#;
    let stub = stub_agy(&format!("printf '%s\\n' '{frame}'; exit 7"));
    adapter.agy_bin = stub.bin();

    let lines = run_turn(&mut adapter, Arc::new(AtomicBool::new(false))).await;

    let response = sole_response(&lines);
    assert_eq!(
        response["error"]["code"], -32000,
        "a turn that streamed then failed must report an error, got {response}"
    );
    assert!(
        response["result"].is_null(),
        "a failed turn must not report end_turn, got {response}"
    );
    assert_eq!(adapter.live_children().len(), 0);
}

/// A `result` event carrying an error wins over every other message the
/// failure cascade can produce -- it is the only one agy actually explains
/// itself in. Nothing else pins the head of that cascade: a reorder letting
/// the "stream ended without a result event" arm shadow a present
/// `result_error` would otherwise pass every test here, and the client would
/// be told the stream was truncated when agy had said why it failed.
#[tokio::test]
async fn a_result_event_error_outranks_the_other_failure_messages() {
    let mut adapter = Adapter::new_for_test();
    let frame = r#"{"event":"result","result":{"conversation_id":"conv-abc","status":"ERROR","error":"quota exhausted"}}"#;
    // Exits 0 with stderr noise: without the result event this would be the
    // "stream ended without a result event" case, and the stderr fallback is
    // one arm further down again.
    let stub = stub_agy(&format!(
        "printf '%s\\n' '{frame}'; echo 'noise on stderr' >&2; exit 0"
    ));
    adapter.agy_bin = stub.bin();

    let lines = run_turn(&mut adapter, Arc::new(AtomicBool::new(false))).await;

    let response = sole_response(&lines);
    assert_eq!(
        response["error"]["message"], "agy failed: quota exhausted",
        "the result event's own error must win the cascade, got {response}"
    );
}

/// The success path's teardown, which nothing pinned before: every bug this
/// function has produced has been a teardown bug, and the spawn-failure one was
/// found and fixed while splitting it. This covers the other side -- what a turn
/// that actually ran leaves behind.
///
/// Four things at once, deliberately: they are one teardown, and a regression
/// that drops any of them individually is the failure mode worth catching.
#[tokio::test]
async fn a_successful_turn_binds_persists_and_releases_the_bridge() {
    let _bridge_guard = BRIDGE_LOCK.lock().await;
    let bridge = {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        crate::permission::PermissionBridge::start(tx).expect("start bridge")
    };

    let mut adapter = Adapter::new_for_test();
    let frame = r#"{"event":"result","result":{"conversation_id":"conv-xyz","status":"SUCCESS","response":"done"}}"#;
    let stub = stub_agy(&format!("printf '%s\\n' '{frame}'"));
    adapter.agy_bin = stub.bin();
    let hook_root = std::env::temp_dir().join(format!("agy-acp-hook-{}", Uuid::new_v4()));
    fs::create_dir_all(&hook_root).unwrap();
    adapter.enable_permission_bridge(&bridge, &hook_root);
    // A real session with no conversation yet -- the state a first turn starts
    // from. A bare session id would skip the in-memory half of the teardown,
    // since there would be no session to bind to.
    let session_id = adapter.handle_session_new(json!(1)).result.unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    let lines = run_turn_for(&mut adapter, &session_id, Arc::new(AtomicBool::new(false))).await;

    let response = sole_response(&lines);
    assert_eq!(response["result"]["stopReason"], "end_turn", "got {response}");

    // Bound in memory, so the next turn can pass --conversation.
    assert_eq!(
        adapter
            .sessions
            .get(&session_id)
            .and_then(|s| s.conversation_id.as_deref()),
        Some("conv-xyz")
    );
    // ...and on disk, so it survives a restart.
    assert_eq!(
        adapter
            .load_store()
            .sessions
            .get(&session_id)
            .and_then(|s| s.conversation_id.as_deref()),
        Some("conv-xyz"),
        "a bound conversation must be persisted, or resume silently starts a new one"
    );
    // The turn is over: the bridge must not still be pointing at it, or a
    // decision landing before the next turn matches a finished generation.
    assert_eq!(bridge.active_session().await, None);
    assert_eq!(bridge.abandon_pending(&session_id).await, 0);

    let _ = fs::remove_dir_all(&hook_root);
}

/// The spawn-failure arm returns after
/// `bridge.set_active_session(Some(..))` on the way in and, before the fix
/// that came with this test, returned above the teardown that clears it --
/// so the binding outlived the turn and the generation bump on the way out
/// never happened.
///
/// Asserts on the binding, not on `abandon_pending`: no tool call can have
/// happened on this path, so the pending map is empty either way and a count
/// of zero would pin nothing.
#[tokio::test]
async fn bridge_binding_is_cleared_after_spawn_failure() {
    let _bridge_guard = BRIDGE_LOCK.lock().await;
    let bridge = {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        crate::permission::PermissionBridge::start(tx).expect("start bridge")
    };

    let mut adapter = Adapter::new_for_test();
    adapter.agy_bin = "/nonexistent/agy-acp-not-a-real-binary".to_string();
    let hook_root =
        std::env::temp_dir().join(format!("agy-acp-hook-{}", Uuid::new_v4()));
    fs::create_dir_all(&hook_root).unwrap();
    adapter.enable_permission_bridge(&bridge, &hook_root);

    let lines = run_turn(&mut adapter, Arc::new(AtomicBool::new(false))).await;
    let response = sole_response(&lines);
    assert_eq!(response["error"]["code"], -32000);

    // The turn set the binding on the way in and returned
    // before the teardown that clears it, so the bridge is still pointing at
    // a session whose turn is over. Asserting on `abandon_pending` instead
    // would prove nothing: no tool call ever happened, so the pending map is
    // trivially empty either way.
    assert_eq!(
        bridge.active_session().await,
        None,
        "a turn that failed to spawn must still clear its binding, or the \
         generation bump in set_active_session never happens and a decision \
         landing in the gap can leave a sticky \"always\" behind"
    );
}
