# Plan: Per-Command Permission Keying

## Objective
Fix the security gap where a single "Always allow" for `run_command` approves all subsequent commands in the session. This corresponds to the `TODO.md` item "Permission decisions ignore what a command actually does".

## Evaluation of Briefs
Both `kilo-hy3-brief.md` and `kilo-longcat-brief.md` accurately identified the problem and provided excellent, pragmatic recommendations. 
- **R1 (Key sticky answer by command string)** is the correct primary fix. It avoids the complexity and unreliability of shell parsing by relying on an exact string match. It also correctly distinguishes between command tools (which need the command string in the key) and path tools (where containment checks already make tool-level keying safe).
- **R4 (Bound answers to session lifetime)** is a valuable cleanup task that can easily be bundled with this change, fixing the issue of entries growing unbounded and surviving session reloads.

## Implementation Steps

### 1. Update `AlwaysKey` logic and UI Text (`src/permission.rs`)
- Change the `AlwaysKey` type alias from `(String, String)` to `(String, String, Option<String>)`, representing `(session_id, tool_name, command_line)`.
- In the `decide` and `apply_outcome` methods, construct the key:
  - Extract the `CommandLine` argument from `args` (`args.get("CommandLine").and_then(|v| v.as_str())`).
  - If it exists, pass it as `Some(command.to_string())`.
  - For all other tools that don't have a `CommandLine` argument, pass `None`.
  - This avoids hardcoding `run_command` and handles any command-executing tool that uses `CommandLine` similarly to `tool_title`.
- Update `permission_options(tool_name: &str, command: Option<&str>)` to differentiate UI text. When a command is present, the text for `OPTION_ALLOW_ALWAYS` should be `"Allow Exact Command This Session"`, and `OPTION_REJECT_ALWAYS` should be `"Reject Exact Command This Session"`. For other tools, retain the `"Always allow {tool_name} this session"` and `"Always reject {tool_name} this session"` text.
- Similarly, update the outcome reason strings returned by `apply_outcome` **and** the cache-hit branch in `decide` to clearly state when an exact command is being remembered or applied, instead of the entire tool.
- This ensures exact-match string equality for commands, while preserving the existing tool-level behavior for path tools and management tools.

### 2. Implement Session Eviction Cleanup (`src/adapter.rs` & `src/permission.rs`)
- Add an async method to the permission bridge (e.g., `pub async fn forget_session(&self, session_id: &str)`) that removes all entries in `BridgeState.always` matching the given `session_id`. Use `state.always.retain(|(sid, _, _), _| sid != session_id);` to drop them efficiently.
- In `src/adapter.rs`, update the synchronous `Adapter::evict_if_needed` method to spawn a background task (`tokio::spawn`) that clones the optional bridge and calls `forget_session` on the evicted session.

### 3. Update Tests (`src/permission.rs`)
- Rename and rewrite `always_allow_is_remembered_per_tool_not_per_command` to `always_allow_is_remembered_per_command_for_command_tools` to assert that:
  - Allowing `run_command "ls"` does NOT auto-allow `run_command "rm -rf build"`.
  - Allowing `run_command "ls"` DOES auto-allow a subsequent `run_command "ls"`.
- Rewrite `a_path_inside_a_command_string_is_invisible_to_the_containment_check` to reflect the new command-level keying behavior (since the command is explicitly matched, the vulnerability is mitigated even though the path is still invisible).
- Add a new test verifying that `forget_session` successfully clears the remembered answers.

### 4. Update Documentation (`README.md`)
- Locate the *What "Always" remembers* section.
- Explain that for command-executing tools, "Always" applies only to the exact command string executed.
- Revise the previous warning that told users to "prefer Allow over Always allow for run_command", as the operation is now safely scoped to exact matches.
