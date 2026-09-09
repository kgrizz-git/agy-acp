//! Synchronous client used by the hook subprocess to reach the bridge.

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
    use std::io::{Read, Write};

    let mut payload = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload);

    let decision = match std::env::var(SOCKET_ENV) {
        Ok(path) if !path.is_empty() => hook_roundtrip(&path, payload.trim()),
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

/// Sends one hook payload and returns the bridge's single-line response.
fn hook_roundtrip(socket_path: &str, payload: &str) -> std::io::Result<String> {
    use std::io::{BufRead, BufReader as StdBufReader, Write};
    use std::os::unix::net::UnixStream as StdUnixStream;

    let mut stream = StdUnixStream::connect(socket_path)?;
    stream.write_all(payload.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = StdBufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;

    let response = response.trim().to_string();
    if response.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "empty response from adapter",
        ));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
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
            let (mut stream, _) = listener.accept().unwrap();
            let mut payload = String::new();
            BufReader::new(&stream).read_line(&mut payload).unwrap();
            assert_eq!(payload, "{\"tool\":\"read\"}\n");
            stream.write_all(b"{\"decision\":\"allow\"}\n").unwrap();
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
        let server = std::thread::spawn(move || drop(listener.accept().unwrap()));

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
}
