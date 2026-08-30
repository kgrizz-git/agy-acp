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
- In `is_always_allowed` and `remember_allow`, construct the key:
  - If the tool is `run_command` or `command_status`, extract the `CommandLine` argument and pass it as `Some(command)`.
  - For all other tools, pass `None`.
- Update `permission_options(tool_name: &str)` to take an additional boolean parameter or the tool name directly to differentiate UI text. For command-executing tools (`run_command`, `command_status`), the text for `OPTION_ALLOW_ALWAYS` should be `"Allow Exact Command This Session"`, and `OPTION_REJECT_ALWAYS` should be `"Reject Exact Command This Session"`. For other tools, retain the `"Always allow {tool_name} this session"` and `"Always reject {tool_name} this session"` text.
- This ensures exact-match string equality for commands, while preserving the existing tool-level behavior for path tools.

### 2. Implement Session Eviction Cleanup (`src/adapter.rs` & `src/permission.rs`)
- Add a method to the permission bridge (e.g., `pub fn forget_session(&self, session_id: &str)`) that removes all entries in `BridgeState.always` matching the given `session_id`.
- In `src/adapter.rs`, update `Adapter::evict_if_needed` to call `forget_session` on the evicted session.

### 3. Update Tests (`src/permission.rs`)
- Rewrite `always_allow_is_remembered_per_tool_not_per_command` to assert that:
  - Allowing `run_command "ls"` does NOT auto-allow `run_command "rm -rf build"`.
  - Allowing `run_command "ls"` DOES auto-allow a subsequent `run_command "ls"`.
- Rewrite `a_path_inside_a_command_string_is_invisible_to_the_containment_check` to reflect the new command-level keying behavior.
- Add a new test verifying that `forget_session` successfully clears the remembered answers.

### 4. Update Documentation (`README.md`)
- Locate the *What "Always" remembers* section.
- Explain that for command-executing tools, "Always" applies only to the exact command string executed.
- Revise the previous warning that told users to "prefer Allow over Always allow for run_command", as the operation is now safely scoped to exact matches.
