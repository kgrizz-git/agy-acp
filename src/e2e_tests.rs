//! End-to-end tests driving the built binary against a real agy.
//!
//! `e2e` must stay in these test paths: CI selects this tier by substring.

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
            // Accumulate: the answer arrives as deltas, and the last one is often
            // just a newline. Overwriting made the assertion below depend on how
            // the model happened to chunk its reply.
            response_text.push_str(
                msg["params"]["update"]["content"]["text"]
                    .as_str()
                    .unwrap_or(""),
            );
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
    writeln!(stdin, "{}", msg).unwrap();
    stdin.flush().unwrap();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 {
            panic!("agy-acp closed stdout before answering id {}", id);
        }
        let msg: Value = serde_json::from_str(line.trim()).unwrap();
        if msg.get("id") == Some(&json!(id)) {
            return msg;
        }
    }
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
    // Accumulated, not overwritten -- see the round-trip test above.
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
            let delta = msg["params"]["update"]["content"]["text"]
                .as_str()
                .unwrap_or_default();
            notification_text
                .get_or_insert_with(String::new)
                .push_str(delta);
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

    // What the test is named for: reload the session, then keep prompting it.
    let loaded = send_recv_id(
        &mut stdin,
        &mut reader,
        4,
        &format!(
            r#"{{"jsonrpc":"2.0","id":4,"method":"session/load","params":{{"sessionId":"{}"}}}}"#,
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
        5,
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
