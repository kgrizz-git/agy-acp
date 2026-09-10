//! Bounded hook-serving path for the permission bridge.

use super::frame::{
    read_bounded_line_async, FrameReject, BRIDGE_READ_TIMEOUT, BRIDGE_WRITE_TIMEOUT,
    MAX_FRAME_BYTES,
};
use super::{parse_frame, Decision, PermissionBridge};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

impl PermissionBridge {
    /// Handles one hook invocation under bounded read and write deadlines.
    pub(super) async fn serve_hook(&self, stream: UnixStream) {
        self.serve_hook_with_timeout(stream, BRIDGE_READ_TIMEOUT)
            .await;
    }

    /// Same as [`PermissionBridge::serve_hook`] with an injectable read
    /// deadline so a slow-peer test need not wait out the production bound.
    pub(super) async fn serve_hook_with_timeout(&self, stream: UnixStream, read_timeout: Duration) {
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        let line = match tokio::time::timeout(
            read_timeout,
            read_bounded_line_async(&mut reader, MAX_FRAME_BYTES),
        )
        .await
        {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => {
                let reject = if error.kind() == std::io::ErrorKind::InvalidInput {
                    FrameReject::Oversized
                } else {
                    FrameReject::Malformed
                };
                self.write_hook_response(&mut write_half, reject).await;
                return;
            }
            Err(_) => {
                self.write_hook_response(&mut write_half, FrameReject::Timeout)
                    .await;
                return;
            }
        };

        let payload = match parse_frame(line.trim()) {
            Ok(payload) => payload,
            Err(reject) => {
                self.write_hook_response(&mut write_half, reject).await;
                return;
            }
        };

        let (decision, reason) = self.decide(&payload).await;
        let response = format!("{}\n", decision.as_hook_json(&reason));
        // Bounded write: the response is small, but a stuck peer must not hold
        // a permit past its deadline. A failed write denies by omission -- agy
        // treats a decision-less close as no allow, and no rejection path here
        // can become one.
        let _ = tokio::time::timeout(
            BRIDGE_WRITE_TIMEOUT,
            write_hook_line(&mut write_half, response.as_bytes()),
        )
        .await;
    }

    /// Writes a fail-closed deny for a frame rejected before `decide`.
    async fn write_hook_response(
        &self,
        write_half: &mut tokio::net::unix::OwnedWriteHalf,
        reject: FrameReject,
    ) {
        let response = format!("{}\n", Decision::Deny.as_hook_json(reject.reason()));
        let _ = tokio::time::timeout(
            BRIDGE_WRITE_TIMEOUT,
            write_hook_line(write_half, response.as_bytes()),
        )
        .await;
    }
}

async fn write_hook_line(
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
    bytes: &[u8],
) -> std::io::Result<()> {
    write_half.write_all(bytes).await?;
    write_half.flush().await
}

/// Answers a connection that arrived with no free permit.
///
/// Its deadline keeps a non-reading peer from holding the one saturated-deny
/// reserve forever.
pub(super) async fn write_saturated_deny(stream: UnixStream) {
    let (read_half, mut write_half) = stream.into_split();
    drop(read_half);
    let response = format!(
        "{}\n",
        Decision::Deny.as_hook_json(FrameReject::Saturated.reason())
    );
    // Truncate defensively: the deny is far below the frame limit, and a
    // response must never exceed what the hook will buffer.
    let bytes = response.as_bytes();
    let end = bytes.len().min(MAX_FRAME_BYTES);
    let _ = tokio::time::timeout(
        BRIDGE_WRITE_TIMEOUT,
        write_hook_line(&mut write_half, &bytes[..end]),
    )
    .await;
}
