//! Non-Unix fallback for the Unix-domain-socket permission bridge.
//!
//! The adapter itself can build and run on non-Unix platforms, but agy's
//! `PreToolUse` bridge requires Unix sockets and Unix file permissions. Keep the
//! type available so the normal adapter stays portable, while failing closed when
//! a caller asks to enable that optional feature.

use std::path::{Path, PathBuf};

pub struct HookRoot {
    dir: PathBuf,
}

impl HookRoot {
    pub fn create() -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "permission prompts require a Unix platform",
        ))
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }
}
