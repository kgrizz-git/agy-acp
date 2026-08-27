//! Provides the `PreToolUse` hook that agy calls into for permission decisions.
//!
//! agy discovers hooks in the `.agents/` directory of every workspace root it is
//! given, so the adapter writes the hook into a private directory of its own and
//! passes it as an extra `--add-dir`. Nothing is installed globally and no
//! repository is touched, which keeps plain `agy` usage in a terminal completely
//! unaffected by this adapter.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const HOOK_NAME: &str = "agy-acp-permission-bridge";
const HOOK_ROOT_PREFIX: &str = "agy-acp-hooks-";

/// Hook timeout in seconds. Deliberately generous: it bounds how long a human has
/// to answer, and must exceed the bridge's own response timeout.
const HOOK_TIMEOUT_SECS: u64 = 600;

/// A private workspace root holding nothing but the permission hook.
///
/// Removed from disk when dropped.
pub struct HookRoot {
    dir: PathBuf,
}

impl HookRoot {
    /// Writes the hook definition into a fresh private directory.
    pub fn create() -> std::io::Result<Self> {
        let exe = std::env::current_exe()?;
        // Drop cannot run when the adapter is killed outright, so sweep whatever
        // earlier runs left behind before adding another.
        sweep_stale_roots();
        let dir = std::env::temp_dir().join(format!("{HOOK_ROOT_PREFIX}{}", std::process::id()));
        let agents_dir = dir.join(".agents");
        std::fs::create_dir_all(&agents_dir)?;

        let hooks = hooks_json(&exe.display().to_string());
        std::fs::write(
            agents_dir.join("hooks.json"),
            format!("{}\n", serde_json::to_string_pretty(&hooks)?),
        )?;

        // agy treats every `--add-dir` as a workspace root, so the model can see this
        // directory and will otherwise happily write into it — files here are thrown
        // away when the adapter exits. Read-only keeps it out of play; agy only ever
        // needs to read the hook definition.
        set_read_only(&dir, true)?;

        Ok(HookRoot { dir })
    }

    /// The directory to hand to agy as an extra `--add-dir`.
    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for HookRoot {
    fn drop(&mut self) {
        // Removal needs write permission back on the directories.
        let _ = set_read_only(&self.dir, false);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Age after which an abandoned hook root is assumed to belong to a dead adapter.
/// Comfortably longer than the hook timeout, so a live run is never swept.
const STALE_ROOT_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Deletes hook roots left behind by adapters that exited without running `Drop`.
fn sweep_stale_roots() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_hook_root = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(HOOK_ROOT_PREFIX));
        if !is_hook_root {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .and_then(|t| {
                t.elapsed()
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
            })
            .map(|age| age > STALE_ROOT_AGE)
            .unwrap_or(false);
        if stale {
            let _ = set_read_only(&path, false);
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Toggles write permission on the hook root and its `.agents` subdirectory.
fn set_read_only(dir: &Path, read_only: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = if read_only { 0o555 } else { 0o700 };
    for path in [dir.join(".agents"), dir.to_path_buf()] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

fn hooks_json(exe: &str) -> Value {
    json!({
        HOOK_NAME: {
            "PreToolUse": [
                {
                    "matcher": "*",
                    "hooks": [
                        {
                            "type": "command",
                            "command": format!("{} permission-hook", shell_quote(exe)),
                            "timeout": HOOK_TIMEOUT_SECS,
                        }
                    ]
                }
            ]
        }
    })
}

/// agy runs hook commands through `sh -c`, so a path with spaces needs quoting.
fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_json_matches_every_tool_and_invokes_the_hook_subcommand() {
        let value = hooks_json("/usr/local/bin/agy-acp");
        let group = &value[HOOK_NAME]["PreToolUse"][0];
        assert_eq!(group["matcher"], "*");
        assert_eq!(
            group["hooks"][0]["command"],
            "/usr/local/bin/agy-acp permission-hook"
        );
        assert_eq!(group["hooks"][0]["timeout"], HOOK_TIMEOUT_SECS);
    }

    #[test]
    fn paths_with_spaces_are_quoted_for_sh() {
        assert_eq!(shell_quote("/plain/path"), "/plain/path");
        assert_eq!(
            shell_quote("/Applications/My App/agy-acp"),
            "'/Applications/My App/agy-acp'"
        );
    }

    #[test]
    fn the_hook_root_holds_a_hooks_file_and_cleans_up_after_itself() {
        let path = {
            let root = HookRoot::create().expect("hook root");
            let hooks = root.path().join(".agents").join("hooks.json");
            assert!(hooks.exists(), "hooks.json should be written");

            let parsed: Value =
                serde_json::from_str(&std::fs::read_to_string(&hooks).unwrap()).unwrap();
            assert!(parsed[HOOK_NAME]["PreToolUse"].is_array());

            root.path().to_path_buf()
        };
        assert!(!path.exists(), "hook root should be removed on drop");
    }
}
