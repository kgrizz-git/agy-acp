//! End-to-end tests driving the built binary against a real agy.
//!
//! `e2e` must stay in these test paths: CI selects this tier by substring.
//! Model-issuing tests pick a slug from `E2E_MODEL_ROSTER` via
//! `session/set_model` so consecutive runs spread the free-tier daily
//! per-model quota. Retries advance through that same roster to avoid waiting
//! on one temporarily unavailable model; an empty roster falls through to the
//! `settings.json` default. `session_load` also asserts conversation memory,
//! so there is no separate multi-turn test.

use serde_json::{json, Value};

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

/// Parse a comma-separated slug roster and pick this test's model.
///
/// `offset` rotates the starting point so consecutive CI runs spread the
/// two-turn `session_load` load across buckets: assignment is
/// `roster[(offset + test_index) % len]`. An empty roster is `None` — the
/// caller must skip `session/set_model` and let agy use the settings.json
/// default. A single-entry roster is a no-op rotation (both tests get the
/// same model).
fn pick_model_from(roster: &str, offset: usize, test_index: usize) -> Option<String> {
    let roster: Vec<&str> = roster
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if roster.is_empty() {
        return None;
    }
    Some(roster[(offset + test_index) % roster.len()].to_string())
}

/// CI sets `E2E_MODEL_ROSTER` (flash-low slugs) and `E2E_MODEL_OFFSET`
/// (`github.run_number`). Unset or empty roster → `None`.
fn pick_model(test_index: usize) -> Option<String> {
    let roster = std::env::var("E2E_MODEL_ROSTER").unwrap_or_default();
    let offset = std::env::var("E2E_MODEL_OFFSET")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    pick_model_from(&roster, offset, test_index)
}

/// Pick a different roster position for a retry of this test's model turn.
/// Retry zero is the initial assignment, so it does not switch models.
fn retry_model_from(
    roster: &str,
    offset: usize,
    test_index: usize,
    retry_number: usize,
) -> Option<String> {
    if retry_number == 0 {
        return None;
    }
    pick_model_from(roster, offset, test_index + retry_number)
}

fn retry_model(test_index: usize, retry_number: usize) -> Option<String> {
    let roster = std::env::var("E2E_MODEL_ROSTER").unwrap_or_default();
    let offset = std::env::var("E2E_MODEL_OFFSET")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    retry_model_from(&roster, offset, test_index, retry_number)
}

#[test]
fn pick_model_from_empty_or_whitespace_is_none() {
    assert_eq!(pick_model_from("", 0, 0), None);
    assert_eq!(pick_model_from("  , , ", 7, 1), None);
}

#[test]
fn pick_model_from_rotates_by_offset_and_index() {
    let roster = "a,b,c";
    // Run 0: full_round_trip -> a, session_load -> b
    assert_eq!(pick_model_from(roster, 0, 0).as_deref(), Some("a"));
    assert_eq!(pick_model_from(roster, 0, 1).as_deref(), Some("b"));
    // Run 1: b, c
    assert_eq!(pick_model_from(roster, 1, 0).as_deref(), Some("b"));
    assert_eq!(pick_model_from(roster, 1, 1).as_deref(), Some("c"));
    // Run 2: c, a — the 2-turn load wraps onto a
    assert_eq!(pick_model_from(roster, 2, 0).as_deref(), Some("c"));
    assert_eq!(pick_model_from(roster, 2, 1).as_deref(), Some("a"));
}

#[test]
fn pick_model_from_single_entry_is_a_noop() {
    assert_eq!(pick_model_from("only", 0, 0).as_deref(), Some("only"));
    assert_eq!(pick_model_from("only", 0, 1).as_deref(), Some("only"));
    assert_eq!(pick_model_from("only", 9, 1).as_deref(), Some("only"));
}

#[test]
fn pick_model_from_trims_and_drops_empty_tokens() {
    assert_eq!(pick_model_from(" a, ,b ", 0, 1).as_deref(), Some("b"));
}

#[test]
fn retry_model_advances_through_the_roster() {
    let roster = "a,b,c";
    assert_eq!(retry_model_from(roster, 0, 1, 0), None);
    assert_eq!(retry_model_from(roster, 0, 1, 1).as_deref(), Some("c"));
    assert_eq!(retry_model_from(roster, 0, 1, 2).as_deref(), Some("a"));
    assert_eq!(retry_model_from("only", 0, 0, 1).as_deref(), Some("only"));
}

#[test]
#[ignore]
fn test_e2e_agy_acp_full_round_trip() {
    use std::io::BufReader;
    use std::process::{Command, Stdio};

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
        .join("target/release/agy-gated-acp");
    if !binary.exists() {
        panic!("Run `cargo build --release` first");
    }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn agy-gated-acp");
    forward_stderr(&mut child);

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let resp = send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientName":"e2e","clientVersion":"0.1"}}"#,
    );
    let init: Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(init["result"]["protocolVersion"], 1);

    let resp = send_recv(
        &mut stdin,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}"#,
    );
    let session: Value = serde_json::from_str(&resp).unwrap();
    let session_id = session["result"]["sessionId"].as_str().unwrap();
    assert!(!session_id.is_empty());

    maybe_set_model(&mut stdin, &mut reader, session_id, 0, 3);

    let (notification_text, resp) = send_prompt_wait(
        &mut stdin,
        &mut reader,
        4,
        session_id,
        0,
        "Reply with exactly one word: PONG",
    );
    assert!(resp["error"].is_null(), "Got error: {}", resp["error"]);
    assert_eq!(resp["result"]["stopReason"], "end_turn");

    drop(stdin);
    let _ = child.wait();

    assert!(
        notification_text.is_some(),
        "Expected session/update notification"
    );
    let response_text = notification_text.unwrap_or_default();
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
        .join("target/release/agy-gated-acp");
    if !binary.exists() {
        panic!("Run `cargo build --release` first");
    }

    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn agy-gated-acp");
    forward_stderr(&mut child);
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    Some((stdin, BufReader::new(stdout), child))
}

/// Echoes the child's stderr to ours, locking the adapter's `[agy-gated-acp] agy
/// stderr: ...` lines into the test log. Without this the pipe is never read:
/// the provider's actual error (a 429, an auth failure) sits in the buffer
/// while the assertion sees only the JSON `agy failed:` wrapper, and a
/// rate-limit flake is indistinguishable from a regression in CI.
fn forward_stderr(child: &mut std::process::Child) {
    use std::io::{BufRead, BufReader};
    if let Some(err) = child.stderr.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                eprintln!("[agy-gated-acp stderr] {line}");
            }
        });
    }
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

fn set_model_rpc(rpc_id: u64, session_id: &str, slug: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": rpc_id,
        "method": "session/set_model",
        "params": { "sessionId": session_id, "modelId": slug },
    })
    .to_string()
}

#[test]
fn set_model_rpc_encodes_session_and_model_strings() {
    let request: Value =
        serde_json::from_str(&set_model_rpc(7, "session\"id", "model\nslug")).unwrap();
    assert_eq!(request["id"], 7);
    assert_eq!(request["params"]["sessionId"], "session\"id");
    assert_eq!(request["params"]["modelId"], "model\nslug");
}

/// Apply this test's roster assignment via `session/set_model`, or log that
/// we are falling through to settings.json. Panics if set_model fails so a
/// bad slug cannot silently run against the default and look like rotation.
fn maybe_set_model(
    stdin: &mut std::process::ChildStdin,
    reader: &mut std::io::BufReader<std::process::ChildStdout>,
    session_id: &str,
    test_index: usize,
    rpc_id: u64,
) {
    match pick_model(test_index) {
        Some(slug) => {
            eprintln!("[e2e] model: {slug}");
            let resp = send_recv(stdin, reader, &set_model_rpc(rpc_id, session_id, &slug));
            let val: Value = serde_json::from_str(&resp).unwrap();
            assert!(
                val["error"].is_null(),
                "session/set_model error for {slug}: {}",
                val["error"]
            );
        }
        None => {
            eprintln!("[e2e] model: (settings.json default; no E2E_MODEL_ROSTER)");
        }
    }
}

/// Switch a retried prompt to the next configured roster member. A missing or
/// rejected retry assignment is only diagnostic: the prompt still retries on
/// the current model, preserving the original failure in the test output.
fn switch_to_retry_model(
    stdin: &mut std::process::ChildStdin,
    reader: &mut std::io::BufReader<std::process::ChildStdout>,
    session_id: &str,
    test_index: usize,
    retry_number: usize,
    prompt_id: u64,
) {
    let Some(slug) = retry_model(test_index, retry_number) else {
        eprintln!("[e2e] retry model: (settings.json default; no E2E_MODEL_ROSTER)");
        return;
    };
    eprintln!("[e2e] retry model: {slug}");
    let rpc_id = 10_000 + prompt_id * TURN_ATTEMPTS as u64 + retry_number as u64;
    let response = send_recv_id(
        stdin,
        reader,
        rpc_id,
        &set_model_rpc(rpc_id, session_id, &slug),
    );
    if !response["error"].is_null() {
        eprintln!(
            "[e2e] retry model switch failed for {slug}: {}",
            response["error"]
        );
    }
}

/// The agent's answer text from a `session/update`, and nothing else.
///
/// `agent_thought_chunk` carries the identical `content.text` shape, so matching
/// on the payload alone would let a model's reasoning ("reply with exactly one
/// word: PONG") satisfy an assertion about its answer.
fn agent_text(msg: &Value) -> &str {
    let update = &msg["params"]["update"];
    if update["sessionUpdate"] != json!("agent_message_chunk") {
        return "";
    }
    update["content"]["text"].as_str().unwrap_or("")
}

/// Like [`send_recv`], but skips ahead to the response carrying `id`.
///
/// A successful `session/load` replays the stored transcript as `session/update`
/// notifications *before* its response, so a single `read_line` would return a
/// notification instead of the reply.
fn send_recv_id(
    stdin: &mut std::process::ChildStdin,
    reader: &mut std::io::BufReader<std::process::ChildStdout>,
    id: u64,
    msg: &str,
) -> Value {
    use std::io::{BufRead, Write};
    use std::time::Duration;

    writeln!(stdin, "{}", msg).unwrap();
    stdin.flush().unwrap();

    // Bounds a reply that never comes but keeps the pipe busy -- endless replay
    // notifications, say. A read blocked with nothing arriving is not covered:
    // std pipes take no read timeout, and the e2e workflow's own
    // `timeout-minutes` is what catches that.
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        if std::time::Instant::now() > deadline {
            panic!("Timed out waiting for the response to id {}", id);
        }
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 {
            panic!("agy-gated-acp closed stdout before answering id {}", id);
        }
        let msg: Value = serde_json::from_str(line.trim())
            .unwrap_or_else(|e| panic!("non-JSON line on stdout: {e}: {:?}", line));
        if msg.get("id") == Some(&json!(id)) {
            return msg;
        }
    }
}

/// Attempts for one model turn: the first try plus two bounded retries.
const TURN_ATTEMPTS: u32 = 3;
const _: () = assert!(TURN_ATTEMPTS >= 1);

/// Delays before resending a failed turn. The first catches brief 503 capacity
/// spikes; the second exceeds the observed per-minute 429 `retryDelay ~37s`.
const TURN_RETRY_DELAYS_SECS: [u64; 2] = [30, 60];
const _: () = assert!(TURN_RETRY_DELAYS_SECS.len() == (TURN_ATTEMPTS - 1) as usize);

/// Returns the delay for a valid retry count, never panicking on a bad caller.
fn retry_delay_secs(attempts_left: u32) -> Option<u64> {
    if !(1..TURN_ATTEMPTS).contains(&attempts_left) {
        return None;
    }
    let retry_index = (TURN_ATTEMPTS - attempts_left - 1) as usize;
    TURN_RETRY_DELAYS_SECS.get(retry_index).copied()
}

fn retry_number(attempts_left: u32) -> Option<usize> {
    (1..TURN_ATTEMPTS)
        .contains(&attempts_left)
        .then_some((TURN_ATTEMPTS - attempts_left) as usize)
}

#[test]
fn retry_delays_back_off_without_exceeding_the_e2e_budget() {
    assert_eq!(retry_delay_secs(2), Some(30));
    assert_eq!(retry_delay_secs(1), Some(60));
    assert_eq!(retry_delay_secs(0), None);
    assert_eq!(retry_delay_secs(TURN_ATTEMPTS), None);
    assert_eq!(retry_number(2), Some(1));
    assert_eq!(retry_number(1), Some(2));
    assert_eq!(retry_number(0), None);
    assert_eq!(retry_number(TURN_ATTEMPTS), None);
}

/// Decide whether a failed turn is worth resending. Sleeps before returning
/// true. Always logs, so a retried turn is visible in the test output; on the
/// final failure, points at the agy log, which names provider status and quota
/// details when agy exposes them.
///
/// The retry is deliberately blind to the error text: a failed turn surfaces
/// as `agy failed: <opaque agy stderr>`, which often omits the provider status
/// entirely, so matching "429"/"503" would miss transient phrasings while
/// coupling us to agy stderr wording. Refusals are not errors here
/// (`stopReason: "refusal"`), and malformed/session/auth failures cannot occur
/// past the harness gates — so the only cost of a needless retry is at most 90s
/// of backoff on an already-failed run.
fn await_turn_retry(err: &Value, attempts_left: u32) -> bool {
    use std::time::Duration;
    if attempts_left == 0 {
        eprintln!("[e2e] turn failed after retry; not retrying: {err}");
        eprintln!("[e2e] hint: provider details are in the agy-logs CI artifact");
        return false;
    }
    let Some(delay_secs) = retry_delay_secs(attempts_left) else {
        eprintln!("[e2e] invalid retry count; not retrying: {err}");
        return false;
    };
    eprintln!("[e2e] turn error, retrying ({attempts_left} remaining) after {delay_secs}s: {err}");
    std::thread::sleep(Duration::from_secs(delay_secs));
    true
}

fn send_prompt_wait(
    stdin: &mut std::process::ChildStdin,
    reader: &mut std::io::BufReader<std::process::ChildStdout>,
    id: u64,
    session_id: &str,
    test_index: usize,
    text: &str,
) -> (Option<String>, Value) {
    use std::io::{BufRead, Write};
    use std::time::Duration;

    let mut attempts_left = TURN_ATTEMPTS - 1;
    loop {
        let msg = format!(
            r#"{{"jsonrpc":"2.0","id":{},"method":"session/prompt","params":{{"sessionId":"{}","prompt":[{{"type":"text","text":"{}"}}]}}}}"#,
            id, session_id, text
        );
        // Resend under the same id on retry: the error response was already
        // consumed, so nothing is ambiguous on the wire.
        writeln!(stdin, "{}", msg).unwrap();
        stdin.flush().unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        // Accumulated, not overwritten: the answer arrives as deltas, and the
        // last one is often just a newline. Overwriting would make assertions
        // depend on how the model happened to chunk its reply.
        let mut notification_text: Option<String> = None;
        let resp = loop {
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
                notification_text
                    .get_or_insert_with(String::new)
                    .push_str(agent_text(&msg));
            }
            if msg.get("id") == Some(&json!(id)) {
                break msg;
            }
        };
        if resp["error"].is_null() {
            return (notification_text, resp);
        }
        if !await_turn_retry(&resp["error"], attempts_left) {
            return (notification_text, resp);
        }
        let Some(retry_number) = retry_number(attempts_left) else {
            return (notification_text, resp);
        };
        switch_to_retry_model(stdin, reader, session_id, test_index, retry_number, id);
        attempts_left -= 1;
    }
}

#[test]
#[ignore]
fn test_e2e_session_load() {
    // Two turns: plant a token, `session/load`, then ask for it. That is
    // replay *and* live continuity (`--conversation`), which used to be a
    // separate `multi_turn` test.
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

    maybe_set_model(&mut stdin, &mut reader, &session_id, 1, 3);

    let (_text, resp1) = send_prompt_wait(
        &mut stdin,
        &mut reader,
        4,
        &session_id,
        1,
        "Remember this word: BANANA. Reply OK.",
    );
    assert!(
        resp1["error"].is_null(),
        "First turn error: {}",
        resp1["error"]
    );

    // What the test is named for: reload the session, then keep prompting it.
    let loaded = send_recv_id(
        &mut stdin,
        &mut reader,
        5,
        &format!(
            r#"{{"jsonrpc":"2.0","id":5,"method":"session/load","params":{{"sessionId":"{}"}}}}"#,
            session_id
        ),
    );
    assert!(
        loaded["error"].is_null(),
        "session/load error: {}",
        loaded["error"]
    );

    let (text2, resp2) = send_prompt_wait(
        &mut stdin,
        &mut reader,
        6,
        &session_id,
        1,
        "What word did I ask you to remember? Reply with just that word.",
    );
    assert!(
        resp2["error"].is_null(),
        "Second turn error: {}",
        resp2["error"]
    );
    assert!(text2.is_some(), "Expected response on continued session");
    let reply = text2.unwrap_or_default().to_lowercase();
    assert!(
        reply.contains("banana"),
        "Expected 'banana' in session_load reply, got: '{}'",
        reply
    );

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
