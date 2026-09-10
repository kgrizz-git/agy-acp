//! Synchronous client used by the hook subprocess to reach the bridge.

use super::frame::{
    read_bounded_line_sync, HOOK_READ_TIMEOUT, HOOK_WRITE_TIMEOUT, MAX_FRAME_BYTES,
};
use super::{Decision, SOCKET_ENV};

/// Entry point for `agy-acp permission-hook`, the command wired into agy's
/// `PreToolUse` hook. Reads the hook payload on stdin, asks the running adapter
/// over the bridge socket, and writes agy's decision JSON to stdout.
///
/// The hook only ever reaches agy through the adapter's private hook directory,
/// so a missing socket means the adapter that owns this run is gone. Every failure
/// path denies: agy runs with its own permission checks disabled whenever this
/// hook is installed, so an unanswerable request must not become an allow.
///
/// Every response carries an explicit `decision`. A decision-less response (`{}`)
/// leaves agy waiting on the tool call until the prompt times out.
pub fn run_hook() {
    use std::io::{BufReader, Write};

    // One at-most-1-MiB frame on stdin: the same compile-time limit the bridge
    // enforces, so an oversized peer cannot consume unbounded memory here.
    let stdin = std::io::stdin();
    let mut locked = BufReader::new(stdin.lock());
    let payload = match read_bounded_line_sync(&mut locked, MAX_FRAME_BYTES) {
        Ok(line) => line.trim().to_string(),
        Err(err) => {
            deny(&format!("agy-acp: permission bridge unavailable ({err})"));
            return;
        }
    };
    // An empty stdin frame carries no tool call and must not reach the bridge
    // as an empty line: deny locally, without another socket round trip.
    if payload.is_empty() {
        deny("agy-acp: malformed permission request");
        return;
    }

    let decision = match std::env::var(SOCKET_ENV) {
        Ok(path) if !path.is_empty() => hook_roundtrip(&path, &payload),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{SOCKET_ENV} is not set"),
        )),
    }
    .unwrap_or_else(|err| {
        Decision::Deny
            .as_hook_json(&format!("agy-acp: permission bridge unavailable ({err})"))
            .to_string()
    });

    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{decision}");
    let _ = stdout.flush();
}

fn deny(reason: &str) {
    use std::io::Write;

    let decision = Decision::Deny.as_hook_json(reason).to_string();
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{decision}");
    let _ = stdout.flush();
}

/// Sends one hook payload and returns the bridge's single-line response.
///
/// Both directions are bounded: the request write carries a short deadline and
/// the response read is limited to one 1-MiB frame. The response wait itself
/// is long -- it covers the bridge's host wait -- preserving the timeout
/// ordering bridge wait < hook wait < agy hook timeout. An oversized or empty
/// response denies via the caller's fail-closed mapping.
fn hook_roundtrip(socket_path: &str, payload: &str) -> std::io::Result<String> {
    use std::io::{BufReader as StdBufReader, Write};
    use std::os::unix::net::UnixStream as StdUnixStream;

    if payload.len() > MAX_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "frame exceeds size limit",
        ));
    }

    let stream = StdUnixStream::connect(socket_path)?;
    stream.set_write_timeout(Some(HOOK_WRITE_TIMEOUT))?;
    stream.set_read_timeout(Some(HOOK_READ_TIMEOUT))?;
    let mut stream = stream;
    stream.write_all(payload.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = StdBufReader::new(&stream);
    let response = read_bounded_line_sync(&mut reader, MAX_FRAME_BYTES)?;

    let response = response.trim().to_string();
    if response.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "empty response from adapter",
        ));
    }
    let has_decision = serde_json::from_str::<serde_json::Value>(&response)
        .ok()
        .map(|value| {
            value
                .get("decision")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|decision| !decision.trim().is_empty())
        })
        .unwrap_or(false);
    if !has_decision {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bridge response has no explicit decision",
        ));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixListener;

    fn socket_path(label: &str) -> std::path::PathBuf {
        // Unix socket addresses are short; `temp_dir()` can be much longer.
        std::path::PathBuf::from("/tmp").join(format!("a-{label}-{}.sock", uuid::Uuid::new_v4()))
    }

    #[test]
    fn hook_roundtrip_sends_the_payload_and_returns_the_response() {
        let path = socket_path("hook-roundtrip");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(&stream);
            let payload = read_bounded_line_sync(&mut reader, MAX_FRAME_BYTES).unwrap();
            assert_eq!(payload, "{\"tool\":\"read\"}");
            (&stream).write_all(b"{\"decision\":\"allow\"}\n").unwrap();
        });

        assert_eq!(
            hook_roundtrip(path.to_str().unwrap(), "{\"tool\":\"read\"}").unwrap(),
            "{\"decision\":\"allow\"}"
        );
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn hook_roundtrip_rejects_an_empty_bridge_response() {
        let path = socket_path("empty-hook-response");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            // Read the request before closing our write half so the client
            // cannot race the close while sending it. That makes this an
            // empty-response test rather than a write-error race.
            let mut reader = std::io::BufReader::new(&stream);
            read_bounded_line_sync(&mut reader, MAX_FRAME_BYTES).unwrap();
            stream.shutdown(std::net::Shutdown::Write).unwrap();
        });

        let error = hook_roundtrip(path.to_str().unwrap(), "{}").unwrap_err();
        // Closing a Unix stream without a reply is EOF on macOS and a reset on
        // Linux. Both mean that the bridge supplied no decision.
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
        ));
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn hook_roundtrip_reports_an_unavailable_socket() {
        let path = socket_path("missing-hook-socket");
        assert!(hook_roundtrip(path.to_str().unwrap(), "{}").is_err());
    }

    #[test]
    fn hook_roundtrip_rejects_an_oversized_response() {
        let path = socket_path("oversized-hook-response");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(&stream);
            let _ = read_bounded_line_sync(&mut reader, MAX_FRAME_BYTES).unwrap();
            // One frame just over the shared limit: the client must deny
            // rather than buffer it.
            let big = "x".repeat(MAX_FRAME_BYTES + 16);
            (&stream).write_all(big.as_bytes()).unwrap();
            let _ = (&stream).write_all(b"\n");
            let _ = (&stream).flush();
        });

        let error = hook_roundtrip(path.to_str().unwrap(), "{}").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn hook_roundtrip_rejects_an_oversized_request_without_sending() {
        let path = socket_path("oversized-hook-request");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            // The client must fail before connecting: nobody should arrive.
            drop(listener);
        });

        let big = "x".repeat(MAX_FRAME_BYTES + 1);
        let error = hook_roundtrip(path.to_str().unwrap(), &big).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn hook_roundtrip_rejects_a_response_without_a_decision() {
        let path = socket_path("decisionless-hook-response");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(&stream);
            let _ = read_bounded_line_sync(&mut reader, MAX_FRAME_BYTES).unwrap();
            (&stream)
                .write_all(b"{\"reason\":\"not enough\"}\n")
                .unwrap();
        });

        let error = hook_roundtrip(path.to_str().unwrap(), "{}").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
