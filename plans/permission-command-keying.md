# Plan: Per-Command Permission Keying

## Objective
Fix the security gap where a single "Always allow" for `run_command` approves all subsequent commands in the session. This corresponds to the `TODO.md` item "Permission decisions ignore what a command actually does".

## Evaluation of Briefs
Both `kilo-hy3-brief.md` and `kilo-longcat-brief.md` accurately identified the problem and provided excellent, pragmatic recommendations. 
- **R1 (Key sticky answer by command string)** is the correct primary fix. It avoids the complexity and unreliability of shell parsing by relying on an exact string match. It also correctly distinguishes between command tools (which need the command string in the key) and path tools (where containment checks already make tool-level keying safe).
- **R4 (Bound answers to session lifetime)** is a valuable cleanup task that can easily be bundled with this change, fixing the issue of entries growing unbounded and surviving session reloads.

## Implementation Steps

### 1. Update `AlwaysKey` logic and UI Text (`src/permission.rs`)
- Change the `AlwaysKey` type alias from `(String, String)` to `(String, String, Option<String>)`, representing `(session_id, tool_name, command_line)`. Update the doc-comment on `AlwaysKey` (currently `permission.rs:65-75`) to spell out that the `Option<String>` is `Some(CommandLine)` for command tools and `None` for everything else, **and** drop the "never cleared" sentence — step 2 makes that statement wrong on landing and leaving the stale comment in would re-confuse the next reader.
- In `decide` and `apply_outcome`, construct the key:
  - Extract the `CommandLine` argument from `args` (`args.get("CommandLine").and_then(|v| v.as_str())`).
  - If it exists, pass it as `Some(command.to_string())`.
  - For all other tools that don't have a `CommandLine` argument, pass `None`.
  - **Design note (deviation from briefs):** Both briefs R1 hardcode the command tool names (`run_command` | `command_status`). Keying on the `CommandLine` argument's *presence* instead follows the precedent already set by `tool_title` (`permission.rs:848`), so a new command-executing tool that ships with a `CommandLine` argument is picked up without touching the permission module. The trade-off is that any tool that smuggles a `CommandLine` field for non-execution purposes would also get per-command keying; in practice that is the conservative direction (more specific), and tool-level keying remains intact for everything else.
- Update `permission_options(tool_name: &str, command: Option<&str>)` to differentiate UI text. The **"Always" prefix must be preserved** so the option type stays identifiable at a glance alongside `allow_once`/`reject_once`; only the trailing phrasing changes:
  - When a command is present, `OPTION_ALLOW_ALWAYS` should label itself `"Always allow this exact command this session"`, and `OPTION_REJECT_ALWAYS` should be `"Always reject this exact command this session"`.
  - For all other tools, retain `"Always allow {tool_name} this session"` and `"Always reject {tool_name} this session"`.
  - Update the call site in `decide` (`permission.rs:331`) from `permission_options(&tool_name)` to `permission_options(&tool_name, command.as_deref())`, where `command` is the `Option<String>` extracted alongside `always_key`. Pass it through `apply_outcome` too if any reason strings there ever want to reference it (currently they only need `tool_name`, but keeping the wiring symmetric avoids a second pass later).
- Update the outcome reason strings returned by `apply_outcome` **and** both branches of the cache-hit path in `decide` (`permission.rs:296` deny and `permission.rs:306` allow) so each clearly states that an exact command is being remembered or applied, instead of just the tool name. For the cached deny branch, surface the command (e.g., `"Always rejected this exact command in this session."`) so the model-visible reason identifies the scope, not just the tool.
- This gives exact-match string equality for command tools, while preserving the existing tool-level behaviour for path tools and management tools.

### 2. Implement Session Eviction Cleanup (`src/adapter.rs` & `src/permission.rs`)
- Add an async method to the permission bridge: `pub async fn forget_session(&self, session_id: &str)` that removes every entry in `BridgeState.always` whose first tuple element equals `session_id`. Use `state.always.retain(|(sid, _, _), _| sid != session_id);` to drop them efficiently. `BridgeState` lives behind a `tokio::sync::Mutex`, so the method must be async — there is no sync locking primitive available without churning the rest of the bridge.
- **The two callers of `evict_if_needed` are sync `pub fn`, not async.** `handle_session_new` (`adapter.rs:467`) and `restore_session_state` (`adapter.rs:432`) are both synchronous, and `restore_session_state` is in turn called from `handle_session_load` (`adapter.rs:505,592`) and `handle_session_prompt` (`adapter.rs:635,724,791`), all sync. The dispatcher in `main.rs:216` awaits a tokio `Mutex<Adapter>` lock around them but the methods themselves never `.await` — and turning them into `async fn` cascades through eight test sites in `src/tests.rs` (lines 1566, 1585, 1630, 1660, 1695, 1907, 1931, 1949) plus every callsite that depends on the boolean return value of `restore_session_state`. So "have the two existing async callers await `bridgeless_forget`" is not a viable mechanism as written.
- Do **not** call `tokio::spawn` from inside `evict_if_needed` either. The existing test `evict_if_needed_drops_the_least_recently_used_session` (`src/tests.rs:2237`) is a plain `#[test]` (no `[tokio::test]` annotation, no runtime). An unconditional `tokio::spawn` panics outside a runtime, even when `permission_bridge: None`, so the existing test would turn red.
- Recommended mechanism: introduce a small helper `pub(crate) fn forget_session_blocking(bridge: &Option<PermissionBridge>, session_id: &str)` that, when called from inside a tokio runtime, drives `bridge.forget_session(session_id)` to completion via the current `Handle`. Sketch:
  ```rust
  pub(crate) fn forget_session_blocking(
      bridge: &Option<PermissionBridge>,
      session_id: &str,
  ) {
      let Some(bridge) = bridge else { return };
      if let Ok(handle) = tokio::runtime::Handle::try_current() {
          handle.block_on(bridge.forget_session(session_id));
      }
      // No runtime: we're in a `#[test]` for `evict_if_needed` with
      // `permission_bridge: None` (the helper returns immediately above), or
      // there is no active runtime to drive the bridge's mutex lock. In either
      // case the bridge state lives only in this process, so dropping the call
      // here is acceptable: the next prompt that needs to consult the map will
      // either find no entry (cache miss, prompt the user — safe direction) or
      // the runtime that should have driven the call will pick it up at its
      // next opportunity.
  }
  ```
  Call it directly from `evict_if_needed` after each victim is removed: `forget_session_blocking(&self.permission_bridge, &key);`. This keeps `evict_if_needed` sync, leaves the existing `#[test]` red-free (no spawn, no tokio runtime needed), and does not require the callers to change.
- Alternatively — if `Handle::block_on` inside the dispatcher concerns anyone — change `evict_if_needed` to return `Vec<String>` (the dropped ids) and have **both** callers await a thin `async fn bridgeless_forget(bridge: &Option<PermissionBridge>, id: &str)` helper, accepting the cascade through `restore_session_state` and every test that constructs an `Adapter` directly. This is a bigger refactor and the briefs explicitly call R4 "cheap, alongside R1" — prefer the blocking-helper form unless there is reason to avoid blocking the dispatcher briefly.
- Whichever mechanism is chosen, add an assertion to the existing `evict_if_needed_drops_the_least_recently_used_session` test that the victims' ids would have been returned — but only via the return-value variant. The blocking-helper form leaves that test untouched (no return value to assert on), and the new `forget_session` test in step 3 covers the bridge side.

### 3. Update Tests (`src/permission.rs` and `src/tests.rs`)
- Rewrite `always_allow_is_remembered_per_tool_not_per_command` → `always_allow_is_remembered_per_command_for_command_tools`. It must assert, against the new wiring:
  - Allowing `run_command "ls"` does **not** auto-allow `run_command "rm -rf build"` (different key → user is asked again).
  - Allowing `run_command "ls"` **does** auto-allow a subsequent `run_command "ls"` (key matches → no new prompt). Both calls in this assertion must reach a cache hit and never call into `out_tx`.
- Rewrite `a_path_inside_a_command_string_is_invisible_to_the_containment_check`. Either rename it to something like `a_path_inside_a_command_string_is_filtered_by_command_level_keying` and recast the test so the user *is* prompted on a cached `ls` followed by `cat /etc/shadow`, or split into two: one preserving the original observation that `absolute_paths`/`outside_workspace` does not see the embedded path as a separate argument, and one asserting that command-level keying prompts the user anyway. Older wording will read as the opposite of what the test now shows — pick a name that matches the new behaviour.
- Update `the_always_options_say_what_they_cover` (`permission.rs:1762`). It currently asserts:
  - `"Always allow run_command this session"` for `OPTION_ALLOW_ALWAYS`
  - `"Always reject run_command this session"` for `OPTION_REJECT_ALWAYS`
  
  Rename or split it into two: `the_always_options_say_what_they_cover_for_command_tools` (asserting the new `"Always allow this exact command this session"` / `"Always reject this exact command this session"` wording) and an existing-style assertion for a non-command tool like `view_file` whose text is unchanged. The ACP-kind-presence assertion (`allow_once`/`allow_always`/`reject_once`/`reject_always`) is independent of the wording and should be kept by both tests.
- Add `forget_session_clears_remembered_answers`. Approve a tool call with `allow_always`, call `bridge.forget_session(&session_id).await`, then assert the next call to the same tool is **prompted** again (cache miss → user goes through `session/request_permission`).
- Add a session-eviction test in `src/tests.rs` (or in `permission.rs`) covering R4 end-to-end against the adapter: stamp some `Always allow` answers into the bridge, build an adapter (with a `tokio::runtime::Runtime` for the blocking-helper form, since `Handle::try_current()` must succeed) whose session map is larger than 64 so `evict_if_needed` drops a known id, call `adapter.evict_if_needed()` and assert the dropped session's answers are gone from `bridge.state`. The adapter can hold a real `PermissionBridge` here because the test runs inside the runtime; `forget_session_blocking` will find a `Handle` and `block_on` the call. If the return-value variant of step 2 is chosen instead, the test calls the now-`async fn evict_if_needed` via `.await` and asserts on the returned victim ids rather than peeking at internal state.

### 4. Update Documentation (`README.md`)
- Locate the *What "Always" remembers* section.
- Explain that for command-executing tools, "Always" applies only to the exact command string executed.
- Revise the previous warning that told users to "prefer Allow over Always allow for run_command", as the operation is now safely scoped to exact matches.
