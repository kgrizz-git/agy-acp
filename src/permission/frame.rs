//! Shared IPC frame limits for the permission bridge and its hook client.
//!
//! Both ends run from the same binary, so no cross-version negotiation is
//! needed: one compile-time limit bounds hook stdin, socket requests, and
//! socket responses. Frames that cannot parse, exceed the limit, time out, or
//! lack a non-empty `toolCall.name` deny without asking the ACP host, and no
//! rejection path can become an allow.

use serde_json::Value;
use std::io;
use std::time::Duration;

/// At most one 1-MiB JSON frame per connection, on either side.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// At most eight live hook connections. The permit spans the full
/// read → host wait → write lifetime, so pending host requests are bounded too.
pub const MAX_CONNECTIONS: usize = 8;

/// At most one short-lived writer may be flushing a saturated denial. This
/// keeps acceptance responsive without making saturation an unbounded task or
/// connection source.
pub const MAX_SATURATION_DENIES: usize = 1;

/// How long the bridge waits for one request line before denying.
pub const BRIDGE_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the bridge waits to write one response line before giving up.
pub const BRIDGE_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the hook waits to send its request. The payload is available
/// immediately on stdin, so this only bounds a stuck local socket.
pub const HOOK_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the hook waits for the bridge's answer. This covers the bridge's
/// own host wait (default 540s) with margin under agy's hook timeout (600s),
/// preserving the timeout ordering: bridge wait < hook wait < agy timeout.
pub const HOOK_READ_TIMEOUT: Duration = Duration::from_secs(590);

/// A frame rejected before any host request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameReject {
    Malformed,
    Oversized,
    EmptyTool,
    Timeout,
    Saturated,
}

impl FrameReject {
    pub fn reason(self) -> &'static str {
        match self {
            FrameReject::Malformed => "agy-gated-acp: malformed permission request",
            FrameReject::Oversized => "agy-gated-acp: permission request exceeds size limit",
            FrameReject::EmptyTool => "agy-gated-acp: permission request names no tool",
            FrameReject::Timeout => "agy-gated-acp: timed out waiting for permission request",
            FrameReject::Saturated => "agy-gated-acp: permission bridge is busy",
        }
    }
}

/// Parses one already-bounded line and requires a non-empty `toolCall.name`.
///
/// Returns the payload on success, or the rejection that denies without
/// asking the ACP host. Malformed and semantically empty input never reaches
/// `decide`.
pub fn parse_frame(line: &str) -> Result<Value, FrameReject> {
    if line.len() > MAX_FRAME_BYTES {
        return Err(FrameReject::Oversized);
    }
    let payload: Value = serde_json::from_str(line).map_err(|_| FrameReject::Malformed)?;
    match payload
        .get("toolCall")
        .and_then(|call| call.get("name"))
        .and_then(|name| name.as_str())
    {
        Some(name) if !name.trim().is_empty() => Ok(payload),
        _ => Err(FrameReject::EmptyTool),
    }
}

/// Reads one `\n`-terminated line from a blocking reader, bounding the bytes
/// buffered to `limit + 1` so an oversized peer cannot consume unbounded memory.
///
/// The returned string keeps its trailing newline handling to the caller: the
/// newline itself is stripped. A missing newline at EOF still returns what was
/// read, so the caller can deny rather than hang.
pub fn read_bounded_line_sync<R: io::BufRead>(reader: &mut R, limit: usize) -> io::Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        // Keep one byte beyond the allowed payload so a newline immediately
        // after an exactly-limit frame is still accepted. Reading only the
        // remaining capacity also keeps the allocation hard-bounded.
        let remaining = limit.saturating_add(1).saturating_sub(buf.len());
        if remaining == 0 {
            return Err(frame_too_large());
        }
        let chunk_len = remaining.min(chunk.len());
        let read = reader.read(&mut chunk[..chunk_len])?;
        if read == 0 {
            break;
        }
        let start = buf.len();
        buf.extend_from_slice(&chunk[..read]);
        if let Some(newline) = buf[start..].iter().position(|&byte| byte == b'\n') {
            let end = start + newline;
            if end > limit {
                return Err(frame_too_large());
            }
            buf.truncate(end);
            return decode_frame(buf);
        }
        if buf.len() > limit {
            return Err(frame_too_large());
        }
    }
    decode_frame(buf)
}

/// Reads one `\n`-terminated line from a Tokio reader, bounding memory the
/// same way as [`read_bounded_line_sync`].
pub async fn read_bounded_line_async<R>(
    reader: &mut tokio::io::BufReader<R>,
    limit: usize,
) -> io::Result<String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let remaining = limit.saturating_add(1).saturating_sub(buf.len());
        if remaining == 0 {
            return Err(frame_too_large());
        }
        let chunk_len = remaining.min(chunk.len());
        let read = reader.read(&mut chunk[..chunk_len]).await?;
        if read == 0 {
            break;
        }
        let start = buf.len();
        buf.extend_from_slice(&chunk[..read]);
        if let Some(newline) = buf[start..].iter().position(|&byte| byte == b'\n') {
            let end = start + newline;
            if end > limit {
                return Err(frame_too_large());
            }
            buf.truncate(end);
            return decode_frame(buf);
        }
        if buf.len() > limit {
            return Err(frame_too_large());
        }
    }
    decode_frame(buf)
}

fn frame_too_large() -> io::Error {
    // InvalidInput distinguishes a size rejection from invalid UTF-8, which
    // the bridge reports as malformed rather than mislabelling it oversized.
    io::Error::new(io::ErrorKind::InvalidInput, "frame exceeds size limit")
}

fn decode_frame(buf: Vec<u8>) -> io::Result<String> {
    String::from_utf8(buf)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_frame_parses() {
        let payload = parse_frame(r#"{"toolCall":{"name":"run_command"}}"#).unwrap();
        assert_eq!(payload["toolCall"]["name"], "run_command");
    }

    #[test]
    fn malformed_frame_rejects() {
        assert_eq!(parse_frame("not json"), Err(FrameReject::Malformed));
        assert_eq!(parse_frame(""), Err(FrameReject::Malformed));
    }

    #[test]
    fn empty_tool_name_rejects() {
        for line in [
            "{}",
            r#"{"toolCall":{}}"#,
            r#"{"toolCall":{"name":""}}"#,
            r#"{"toolCall":{"name":"   "}}"#,
            r#"{"toolCall":{"name":17}}"#,
            r#"{"toolCall":null}"#,
            r#"{"toolCall":[]}"#,
        ] {
            assert_eq!(parse_frame(line), Err(FrameReject::EmptyTool), "{line}");
        }
    }

    #[test]
    fn oversized_line_rejects_before_parsing() {
        let line = "x".repeat(MAX_FRAME_BYTES + 1);
        assert_eq!(parse_frame(&line), Err(FrameReject::Oversized));
    }

    #[test]
    fn sync_reader_bounds_an_oversized_line() {
        let big = "a".repeat(MAX_FRAME_BYTES + 16);
        let mut cursor = io::BufReader::new(big.as_bytes());
        let error = read_bounded_line_sync(&mut cursor, MAX_FRAME_BYTES).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn async_reader_bounds_an_oversized_line() {
        let big = "a".repeat(MAX_FRAME_BYTES + 16);
        let inner = tokio::io::BufReader::new(big.as_bytes());
        let mut reader = tokio::io::BufReader::new(inner.into_inner());
        let error = read_bounded_line_async(&mut reader, MAX_FRAME_BYTES)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn sync_reader_accepts_exact_limit_and_rejects_one_more_byte() {
        let exact = "a".repeat(MAX_FRAME_BYTES);
        let mut exact_reader = io::BufReader::new(exact.as_bytes());
        assert_eq!(
            read_bounded_line_sync(&mut exact_reader, MAX_FRAME_BYTES).unwrap(),
            exact
        );

        let over = "a".repeat(MAX_FRAME_BYTES + 1);
        let mut over_reader = io::BufReader::new(over.as_bytes());
        assert_eq!(
            read_bounded_line_sync(&mut over_reader, MAX_FRAME_BYTES)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn sync_reader_rejects_invalid_utf8_as_malformed_data() {
        let mut reader = io::BufReader::new(&b"\xff\n"[..]);
        assert_eq!(
            read_bounded_line_sync(&mut reader, MAX_FRAME_BYTES)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
