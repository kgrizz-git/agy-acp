//! Private per-process runtime directory for the permission bridge.
//!
//! The bridge's socket and hook root used to live at predictable,
//! PID-derived paths directly under the system temporary directory. This
//! module replaces that with one directory whose name carries a random token
//! and whose mode is `0700`: the socket and hook root have no predictable
//! public pathname, and a name this process did not create is never reused,
//! unlinked, chmodded, or swept by prefix.
//!
//! Ownership is explicit rather than incidental. A [`RuntimeCleanup`] handle
//! removes the owned directory exactly once; ordinary exit and handled signals
//! call it, and [`RuntimeOwner`]'s `Drop` is only a no-panic fallback.
//! `SIGKILL` and machine loss cannot run cleanup, and their random,
//! owner-private remnants are inert -- a later startup never deletes a
//! prefix-matching path.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use uuid::Uuid;

/// The Unix-domain socket path limit this module targets. macOS's `sun_path`
/// is 104 bytes including the terminating NUL and is the smaller of the
/// supported platforms, so a path that fits here also fits on Linux. Checked
/// before `bind`, and a base directory that cannot hold a fitting socket fails
/// closed rather than falling back to a shared path.
pub(crate) const MAX_SOCKET_PATH_BYTES: usize = 104;

/// The socket's filename inside the owner directory. Short on purpose: the
/// directory name carries the random token, and every byte here counts against
/// the socket path limit.
const SOCKET_FILE_NAME: &str = "s.sock";

/// The hook root's directory name inside the owner directory. It is a child of
/// the owner, not a sibling, so a read-only hook root cannot make socket
/// cleanup impossible.
const HOOK_DIR_NAME: &str = "hook";

/// Prefix on the random owner directory. Short for the same reason as the
/// socket filename.
const RUNTIME_PREFIX: &str = "agy-acp-";

/// How many random names to try before treating repeated collisions as a
/// failure. Bounded so a hostile or broken temporary directory cannot make
/// startup spin.
const CREATE_ATTEMPTS: usize = 16;

/// Length of the random token in an owner directory name. 64 bits is far more
/// than enough to make the name unguessable, while keeping the path short
/// enough for the socket limit on the supported platforms.
const TOKEN_LEN: usize = 16;

/// A private, randomly named directory owned by this process, holding the
/// permission bridge's socket and hook root.
#[derive(Debug)]
pub(crate) struct RuntimeOwner {
    dir: PathBuf,
    cleanup: RuntimeCleanup,
}

impl RuntimeOwner {
    /// Creates an owner directory under the system temporary directory.
    pub(crate) fn create() -> io::Result<Self> {
        Self::create_in(&std::env::temp_dir())
    }

    /// Creates an owner directory under `base`.
    ///
    /// The base is a parameter so tests can drive collisions and overlong
    /// paths without touching the real temporary directory.
    pub(crate) fn create_in(base: &Path) -> io::Result<Self> {
        Self::create_in_with(base, random_token)
    }

    fn create_in_with<F: FnMut() -> String>(base: &Path, mut token: F) -> io::Result<Self> {
        let mut last_collision = None;
        for _ in 0..CREATE_ATTEMPTS {
            let dir = base.join(format!("{RUNTIME_PREFIX}{}", token()));
            if !socket_path_fits(&socket_path_in(&dir)) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "temporary directory is too long for a permission socket under {}",
                        base.display()
                    ),
                ));
            }
            match create_private_dir(&dir) {
                Ok(()) => {
                    return Ok(RuntimeOwner {
                        dir: dir.clone(),
                        cleanup: RuntimeCleanup {
                            dir: Arc::new(Mutex::new(Some(dir))),
                        },
                    });
                }
                // The name is taken by something this process did not create.
                // Leave it entirely alone and try another random name.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last_collision = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last_collision.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not create a private runtime directory after repeated name collisions",
            )
        }))
    }

    /// The owner directory itself. Test-only: production code addresses the
    /// socket and hook child, never the directory directly.
    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.dir
    }

    /// The socket pathname the bridge should bind.
    pub(crate) fn socket_path(&self) -> PathBuf {
        socket_path_in(&self.dir)
    }

    /// The child directory to hand to agy as an extra `--add-dir`.
    pub(crate) fn hook_dir(&self) -> PathBuf {
        self.dir.join(HOOK_DIR_NAME)
    }

    /// A cloneable handle for explicit cleanup by ordinary exit and handled
    /// signals. It shares state with this owner, so whichever runs first
    /// removes the directory and later calls are no-ops.
    pub(crate) fn cleanup_handle(&self) -> RuntimeCleanup {
        self.cleanup.clone()
    }

    /// Removes the owned directory. Test-only: production code cleans up
    /// through the shared [`RuntimeCleanup`] handle. See [`RuntimeCleanup::cleanup`].
    #[cfg(test)]
    pub(crate) fn cleanup(&self) -> io::Result<()> {
        self.cleanup.cleanup()
    }
}

impl Drop for RuntimeOwner {
    fn drop(&mut self) {
        // No-panic fallback only. Normal exit and handled signals call
        // `cleanup` explicitly, because a signalled `process::exit` skips
        // `Drop` and `SIGKILL` skips it too.
        let _ = self.cleanup.cleanup();
    }
}

/// Explicit, idempotent cleanup of one owner directory.
///
/// Cloning shares the same state, so the first call restores the read-only
/// hook child and removes the owned directory; later calls do nothing. A
/// failed removal reports the error and leaves the path recorded, so a retry
/// can still succeed.
#[derive(Clone, Debug)]
pub(crate) struct RuntimeCleanup {
    dir: Arc<Mutex<Option<PathBuf>>>,
}

impl RuntimeCleanup {
    pub(crate) fn cleanup(&self) -> io::Result<()> {
        // Cleanup must remain a no-panic fallback even if unrelated code
        // poisoned this bookkeeping mutex. Its recorded path is still valid
        // state to clean up.
        let mut guard = self
            .dir
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(dir) = guard.clone() else {
            return Ok(());
        };
        restore_hook_permissions(&dir)?;
        std::fs::remove_dir_all(&dir)?;
        *guard = None;
        Ok(())
    }
}

/// The socket path beneath an owner directory.
pub(crate) fn socket_path_in(owner_dir: &Path) -> PathBuf {
    owner_dir.join(SOCKET_FILE_NAME)
}

/// True when `path` fits a Unix-domain socket address on the supported
/// platforms.
pub(crate) fn socket_path_fits(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().len() < MAX_SOCKET_PATH_BYTES
}

/// Creates `dir` exclusively with mode `0700`.
///
/// `mkdir` is the primitive: it fails with `AlreadyExists` rather than
/// adopting an existing entry. `umask` can make the requested mode stricter
/// but cannot make it more permissive, so restoring `0700` afterwards cannot
/// open a public window, and it is done only on the directory just created.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(dir)?;
    set_dir_mode(dir, 0o700)
}

/// Toggles read-only mode on the hook root and its `.agents` subdirectory.
///
/// agy treats every `--add-dir` as a workspace root, so the model can see the
/// hook root and will otherwise try to write into it. Read-only keeps it out of
/// play; agy only ever needs to read the hook definition.
pub(crate) fn set_read_only(dir: &Path, read_only: bool) -> io::Result<()> {
    let mode = if read_only { 0o555 } else { 0o700 };
    for path in [dir.join(".agents"), dir.to_path_buf()] {
        set_dir_mode(&path, mode)?;
    }
    Ok(())
}

/// Restores write permission on the owned hook child before removal.
///
/// The hook child and its `.agents` directory may be `0555`, which would make
/// `remove_dir_all` fail. Missing paths are skipped: a partial setup has
/// nothing to restore. A failure to restore an existing path is reported, not
/// swallowed, so cleanup cannot silently leave the directory behind.
fn restore_hook_permissions(owner_dir: &Path) -> io::Result<()> {
    let hook = owner_dir.join(HOOK_DIR_NAME);
    let agents = hook.join(".agents");
    if agents.exists() {
        set_dir_mode(&agents, 0o700)?;
    }
    if hook.exists() {
        set_dir_mode(&hook, 0o700)?;
    }
    Ok(())
}

fn set_dir_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

fn random_token() -> String {
    Uuid::new_v4().simple().to_string()[..TOKEN_LEN].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A unique scratch base under the real temporary directory, removed by
    /// the test that made it. Kept deliberately short: the socket path beneath
    /// it must fit the platform limit, and the macOS temporary directory is
    /// already long.
    fn scratch_base(_label: &str) -> PathBuf {
        let short = &Uuid::new_v4().simple().to_string()[..8];
        let base = std::env::temp_dir().join(format!("agy-rt-{short}"));
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn owner_directory_is_private_and_paths_are_distinct() {
        let base = scratch_base("layout");
        let owner = RuntimeOwner::create_in(&base).unwrap();

        assert_eq!(mode(owner.path()), 0o700, "the owner must be private");
        let socket = owner.socket_path();
        let hook = owner.hook_dir();
        assert!(socket.starts_with(owner.path()));
        assert!(hook.starts_with(owner.path()));
        assert_ne!(socket, hook);
        assert_eq!(
            socket.parent(),
            Some(owner.path()),
            "the socket sits directly in the owner"
        );
        assert_eq!(
            hook.parent(),
            Some(owner.path()),
            "the hook root is an owner child, distinct from the socket's parent"
        );

        owner.cleanup().unwrap();
        assert!(!owner.path().exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn two_owners_get_distinct_random_paths() {
        let base = scratch_base("distinct");
        let first = RuntimeOwner::create_in(&base).unwrap();
        let second = RuntimeOwner::create_in(&base).unwrap();
        assert_ne!(first.path(), second.path());
        first.cleanup().unwrap();
        second.cleanup().unwrap();
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn socket_path_length_boundary_is_conservative() {
        assert!(socket_path_fits(Path::new("/tmp/s.sock")));
        let at_limit = PathBuf::from("a".repeat(MAX_SOCKET_PATH_BYTES));
        assert!(!socket_path_fits(&at_limit));
        let one_under = PathBuf::from("a".repeat(MAX_SOCKET_PATH_BYTES - 1));
        assert!(socket_path_fits(&one_under));
    }

    #[test]
    fn an_overlong_base_fails_before_creating_anything() {
        let base = std::env::temp_dir().join("a".repeat(200));
        fs::create_dir_all(&base).unwrap();

        let error = RuntimeOwner::create_in(&base).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            fs::read_dir(&base).unwrap().count(),
            0,
            "a path that cannot hold the socket must create nothing"
        );
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_name_collision_retries_without_touching_the_existing_entry() {
        let base = scratch_base("collision");
        let occupied = base.join(format!("{RUNTIME_PREFIX}fixed"));
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("sentinel"), "keep").unwrap();

        let owner = RuntimeOwner::create_in_with(&base, {
            let mut calls = 0;
            move || {
                calls += 1;
                if calls == 1 {
                    "fixed".to_string()
                } else {
                    "fresh".to_string()
                }
            }
        })
        .unwrap();

        assert_ne!(owner.path(), occupied);
        assert_eq!(
            fs::read_to_string(occupied.join("sentinel")).unwrap(),
            "keep",
            "a colliding entry must not be modified"
        );

        owner.cleanup().unwrap();
        assert!(
            occupied.join("sentinel").exists(),
            "cleanup must not remove a path this owner did not create"
        );
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn cleanup_removes_only_the_owned_directory() {
        let base = scratch_base("cleanup");
        let sibling = base.join("keep-me");
        fs::create_dir(&sibling).unwrap();
        fs::write(sibling.join("sentinel"), "keep").unwrap();

        let owner = RuntimeOwner::create_in(&base).unwrap();
        fs::write(owner.path().join("scratch"), "x").unwrap();
        let dir = owner.path().to_path_buf();

        owner.cleanup().unwrap();
        assert!(!dir.exists());
        assert_eq!(
            fs::read_to_string(sibling.join("sentinel")).unwrap(),
            "keep"
        );

        // Idempotent: a second call is a no-op, not an error.
        owner.cleanup().unwrap();
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn cleanup_succeeds_when_the_hook_child_is_read_only() {
        let base = scratch_base("readonly");
        let owner = RuntimeOwner::create_in(&base).unwrap();
        let hook = owner.hook_dir();
        fs::create_dir_all(hook.join(".agents")).unwrap();
        fs::write(hook.join(".agents").join("hooks.json"), "{}").unwrap();
        set_read_only(&hook, true).unwrap();
        assert_eq!(mode(&hook), 0o555);
        assert_eq!(mode(&hook.join(".agents")), 0o555);

        owner.cleanup().unwrap();

        assert!(!owner.path().exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn cleanup_removes_a_partially_created_owner() {
        let base = scratch_base("partial");
        let owner = RuntimeOwner::create_in(&base).unwrap();
        // A hook child with no `.agents` is the shape a failure between
        // directory creation and hook writing leaves behind.
        fs::create_dir(owner.hook_dir()).unwrap();

        owner.cleanup().unwrap();

        assert!(!owner.path().exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn the_shared_cleanup_handle_is_idempotent() {
        let base = scratch_base("handle");
        let owner = RuntimeOwner::create_in(&base).unwrap();
        let dir = owner.path().to_path_buf();
        let slot = Arc::new(Mutex::new(Some(owner.cleanup_handle())));
        let handle = slot.lock().unwrap().clone().unwrap();

        handle.cleanup().unwrap();
        assert!(!dir.exists());

        // The signal handler and ordinary exit may both run; neither may fail
        // or remove anything else.
        handle.cleanup().unwrap();
        owner.cleanup().unwrap();
        fs::remove_dir_all(&base).unwrap();
    }
}
