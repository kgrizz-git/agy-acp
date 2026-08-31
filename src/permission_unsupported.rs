//! Non-Unix fallback for the Unix-domain-socket permission bridge.
//!
//! `agy-acp` can still serve ACP requests on these platforms. Only the optional
//! `--permission-prompts` mode is unavailable, and it must fail closed rather
//! than ever launching agy with its permission checks disabled.

use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

pub const SOCKET_ENV: &str = "AGY_ACP_PERMISSION_SOCKET";

#[derive(Clone)]
pub struct PermissionBridge {
    socket_path: PathBuf,
}

impl PermissionBridge {
    pub fn start(_out_tx: mpsc::UnboundedSender<Option<String>>) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "permission prompts require a Unix platform",
        ))
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn register_conversation(&self, _conversation_id: &str, _session_id: &str) {}

    pub async fn set_hook_root(&self, _hook_root: &Path) {}

    pub async fn set_workspace_root(&self, _workspace_root: &str) {}

    pub async fn set_active_session(&self, _session_id: Option<&str>) {}

    pub async fn refused_during_prompt(&self) -> bool {
        false
    }

    pub async fn abandon_pending(&self, _session_id: &str) -> usize {
        0
    }

    pub async fn forget_session(&self, _session_id: &str) {}

    pub async fn resolve_response(
        &self,
        _id: &serde_json::Value,
        _result: Option<serde_json::Value>,
    ) -> bool {
        false
    }
}

/// A hook invocation cannot function off Unix, so give agy an explicit denial.
pub fn run_hook() {
    let mut stdout = std::io::stdout();
    let _ = writeln!(
        stdout,
        "{}",
        r#"{"decision":"deny","reason":"agy-acp: permission prompts require a Unix platform"}"#
    );
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::{mpsc, PermissionBridge};
    use std::io::ErrorKind;

    #[test]
    fn start_fails_closed_on_non_unix() {
        let (out_tx, _out_rx) = mpsc::unbounded_channel();
        let error = PermissionBridge::start(out_tx)
            .err()
            .expect("non-Unix platforms must not start the permission bridge");

        assert_eq!(error.kind(), ErrorKind::Unsupported);
    }
}
