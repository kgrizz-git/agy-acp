//! Provides the `PreToolUse` hook that agy calls into for permission decisions.
//!
//! agy discovers hooks in the `.agents/` directory of every workspace root it is
//! given, so the adapter writes the hook into a child of its private runtime
//! directory and passes that child as an extra `--add-dir`. Nothing is installed
//! globally and no repository is touched, which keeps plain `agy` usage in a
//! terminal completely unaffected by this adapter.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::runtime::{set_read_only, RuntimeOwner};

const HOOK_NAME: &str = "agy-gated-acp-permission-bridge";

/// Hook timeout in seconds. Deliberately generous: it bounds how long a human has
/// to answer, and must exceed the bridge's own response timeout.
const HOOK_TIMEOUT_SECS: u64 = 600;

/// A private workspace root holding nothing but the permission hook.
///
/// It lives inside the adapter's private runtime directory, which owns its
/// lifecycle. This type only writes the hook definition and leaves the
/// directory's permissions read-only; removal is the runtime owner's job, so a
/// read-only hook root cannot block socket cleanup.
pub struct HookRoot {
    dir: PathBuf,
}

impl HookRoot {
    /// Writes the hook definition into the runtime owner's hook child.
    pub(crate) fn create(owner: &RuntimeOwner) -> std::io::Result<Self> {
        let exe = std::env::current_exe()?;
        let dir = owner.hook_dir();
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
        // needs to read the hook definition. Cleanup restores these before removal.
        set_read_only(&dir, true)?;

        Ok(HookRoot { dir })
    }

    /// The directory to hand to agy as an extra `--add-dir`.
    pub fn path(&self) -> &Path {
        &self.dir
    }
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
    use crate::runtime::RuntimeOwner;
    use uuid::Uuid;

    #[test]
    fn hooks_json_matches_every_tool_and_invokes_the_hook_subcommand() {
        let value = hooks_json("/usr/local/bin/agy-gated-acp");
        let group = &value[HOOK_NAME]["PreToolUse"][0];
        assert_eq!(group["matcher"], "*");
        assert_eq!(
            group["hooks"][0]["command"],
            "/usr/local/bin/agy-gated-acp permission-hook"
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
        // Short scratch name: the socket path beneath the owner must fit the
        // platform limit under a long macOS temporary directory.
        let short = &Uuid::new_v4().simple().to_string()[..8];
        let base = std::env::temp_dir().join(format!("agy-rt-{short}"));
        std::fs::create_dir_all(&base).unwrap();
        let owner = RuntimeOwner::create_in(&base).unwrap();

        let path = {
            let root = HookRoot::create(&owner).expect("hook root");
            let hooks = root.path().join(".agents").join("hooks.json");
            assert!(hooks.exists(), "hooks.json should be written");

            let parsed: Value =
                serde_json::from_str(&std::fs::read_to_string(&hooks).unwrap()).unwrap();
            assert!(parsed[HOOK_NAME]["PreToolUse"].is_array());

            root.path().to_path_buf()
        };

        owner.cleanup().unwrap();
        assert!(!path.exists(), "the runtime owner removes the hook root");

        std::fs::remove_dir_all(&base).unwrap();
    }
}
