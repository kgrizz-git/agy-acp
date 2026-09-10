//! Non-Unix fallback for the Unix-domain-socket permission bridge.
//!
//! The adapter itself can build and run on non-Unix platforms, but agy's
//! `PreToolUse` bridge requires Unix sockets and Unix file permissions. Keep the
//! type available so the normal adapter stays portable, while failing closed when
//! a caller asks to enable that optional feature.

use std::path::{Path, PathBuf};

use crate::runtime::RuntimeOwner;

pub struct HookRoot {
    dir: PathBuf,
}

impl HookRoot {
    pub fn create(_owner: &RuntimeOwner) -> std::io::Result<Self> {
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
    use std::io::ErrorKind;

    #[test]
    fn create_fails_closed_on_non_unix() {
        let owner = crate::runtime::RuntimeOwner::create();
        // The owner itself fails closed on non-Unix, so `create` can never be
        // reached with a real owner. Pinning that is the observable contract.
        assert_eq!(
            owner.err().unwrap().kind(),
            ErrorKind::Unsupported,
            "permission prompts must fail closed on non-Unix"
        );
    }
}
