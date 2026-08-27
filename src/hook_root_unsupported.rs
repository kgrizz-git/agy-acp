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

#[cfg(test)]
mod tests {
    use super::HookRoot;
    use std::io::ErrorKind;

    #[test]
    fn create_fails_closed_on_non_unix() {
        let error = HookRoot::create()
            .err()
            .expect("non-Unix platforms must not create a permission hook root");

        assert_eq!(error.kind(), ErrorKind::Unsupported);
    }
}
