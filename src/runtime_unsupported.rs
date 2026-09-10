//! Non-Unix fallback for the permission bridge's private runtime directory.
//!
//! `agy-acp` can still serve ACP requests on these platforms. Only the optional
//! `--permission-prompts` mode needs a private Unix runtime, so this keeps the
//! types available while failing closed when a caller tries to create one.

use std::io;
use std::path::{Path, PathBuf};

pub(crate) struct RuntimeOwner {
    dir: PathBuf,
}

impl RuntimeOwner {
    pub(crate) fn create() -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "permission prompts require a Unix platform",
        ))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn socket_path(&self) -> PathBuf {
        self.dir.clone()
    }

    pub(crate) fn hook_dir(&self) -> PathBuf {
        self.dir.clone()
    }

    pub(crate) fn cleanup_handle(&self) -> RuntimeCleanup {
        RuntimeCleanup
    }

    pub(crate) fn cleanup(&self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct RuntimeCleanup;

impl RuntimeCleanup {
    pub(crate) fn cleanup(&self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeOwner;
    use std::io::ErrorKind;

    #[test]
    fn create_fails_closed_on_non_unix() {
        let error = RuntimeOwner::create()
            .err()
            .expect("non-Unix platforms must not create a private runtime");

        assert_eq!(error.kind(), ErrorKind::Unsupported);
    }
}
