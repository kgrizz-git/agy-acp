# Plan: Per-Command Permission Keying

## Objective

Close the gap recorded in `TODO.md` as "Permission decisions ignore what a
command actually does": a single **Always allow** on `run_command` approves every
later command in the session, including `rm -rf build`.

This is TODO option 1 — make the sticky key as specific as the prompt. Options 2
(extract paths from a command line) and 3 (denylist destructive commands) are
explicitly out of scope; both are defence in depth behind this, and both are
noted at the end so the next reader does not re-derive them.

## Why option 1 and not the others

Both briefs (`dev-docs/investigations/kilo-hy3-brief.md`,
`kilo-longcat-brief.md`) reach the same conclusion, and it holds up: exact-match
keying needs an equivalence test, not a shell parser. Option 2 can never be
complete — a path it fails to extract is a path that gets allowed — and option 3
is evaded by `cat .en"v"`.

The governing rule, from `TODO.md`: **the sticky key should be as specific as the
checks that still apply to a remembered allow.** Tool-level keying is defensible
for path-argument tools like `view_file`, because containment and the
sensitive-path list do constrain a remembered allow there. For command tools
those checks are inert — `escapes_containment` reads arguments as paths and a
command line is one opaque string — so the key is the *only* thing scoping the
answer, and it must carry the command.

That last sentence is the whole security argument for this change and belongs in
the code comment, not just here.

## Design decisions

### D1. What goes in the key

`AlwaysKey` becomes `(String, String, Option<String>)` —
`(session_id, tool_name, args_fingerprint)`.

The fingerprint is `Some(args.to_string())` for command tools and `None` for
everything else. **Fingerprint the whole `args` object, not just `CommandLine`.**

Rationale, and the correction to the earlier draft of this plan: keying on
`CommandLine` alone silently drops every other argument from the scope of the
answer. `Cwd` is the concrete case — it is a known agy argument key
(`src/tools.rs:183`, `PATH_FIELDS` in `src/permission.rs:759`), and `ls` run in
one directory is not `ls` run in another. Since containment does not constrain a
remembered command allow, an ignored `Cwd` is a real widening of what the user
approved. Whether agy actually sends `Cwd` on `run_command` is unverified — see
"Confirm the path-field list against real agy traffic" in `TODO.md`, which needs
the same real-agy capture — and *that uncertainty is the argument for hashing
everything*: an argument the fork has not seen is inside the key by default
instead of outside it.

The cost of over-including is a reprompt when an incidental field changes (a
timeout, a blocking flag). Per `TODO.md`: under-normalizing costs a prompt,
over-normalizing is a hole. This deliberately errs into the first.

`args.to_string()` is canonical here because `serde_json` is built without
`preserve_order`, so its `Map` is a `BTreeMap` and keys serialize sorted.
Note the trap for later: enabling `preserve_order` anywhere in the dependency
graph makes the fingerprint follow wire key order. That direction is safe (a
reordered payload reprompts rather than matches) but it would look like a
flapping bug, so state the dependency in a comment next to the fingerprint.

### D2. No normalization

Exact byte equality on the serialized args. No trimming, no whitespace
collapsing, no tokenization. `"ls"` and `"ls "` are different keys and both
prompt.

This is not laziness — each normalization step merges commands that are not
identical, and keying on the tool name is just the degenerate case of
normalizing everything away, which is how the present bug arose. Pin it with a
test so a future ergonomic tweak has to argue with a red assertion.

### D3. Which tools are command tools

Detect with the union of two cheap tests, and take per-command keying if
**either** fires:

1. `tool_kind(tool_name) == "execute"` — the existing mapping at
   `permission.rs:838` (`"run_command" | "command_status"`).
2. A `CommandLine` field appears anywhere in `args`, found with a nested walk.
   Reuse the walker shape already in this file (`path_field_args`) rather than
   `args.get("CommandLine")` — a top-level-only lookup means a nested or renamed
   command field falls back to tool-level keying, i.e. silently restores exactly
   the bug being fixed.

Detector 1 catches a command tool whose argument shape the fork has not seen;
detector 2 catches a command-executing tool whose *name* the fork has not seen.

To be unambiguous, because this is the sentence an implementer will act on:
`sticky_scope` returns `Some(args.to_string())` when **either** detector fires.
It returns `None` only when **both** miss. Reading this as an AND would leave a
known command tool with an unfamiliar argument shape on the old tool-only key,
which is the exact bypass this plan exists to close.

Both missing falls back to today's tool-level keying — unchanged behaviour, still
documented, not a regression, but also silent. Note that in the code comment.

The earlier draft chose detector 2 alone on the grounds that it follows
`tool_title`'s precedent of keying on argument presence. `tool_kind` is the
better precedent for "is this a tool that executes something", and there is no
reason to pick one detector when both are three lines.

### D4. Prompt wording

Keep the `"Always"` prefix so the option type stays identifiable next to
`allow_once`/`reject_once`; only the trailing phrasing changes.

- Command tools: `"Always allow this exact command this session"` /
  `"Always reject this exact command this session"`.
- Everything else: unchanged `"Always allow {tool_name} this session"` /
  `"Always reject {tool_name} this session"`.

The label does not repeat the command because the prompt's `title` is already
`Run \`{command}\`` (`tool_title`, `permission.rs:848`), and a full command line
inside an option label wraps badly in a host's button. "this exact command"
refers to the one shown directly above it.

**This rests on an unverified host assumption.** "this exact command" is only
meaningful if the host actually renders the tool-call `title` next to the
options. Paseo's ACP integration reports that it hands back *normalized*
permission payloads (`list_pending_permissions`), and nothing here has confirmed
that the `title` survives normalization into the UI. Check it during the capture
trip: with a prompt pending, read `list_pending_permissions` and confirm both the
`title` and the four option `name` strings are present. If the title does not
survive, the label must carry the command itself (truncated), because otherwise
"this exact command" names something the user cannot see.

## Implementation

### 1. Key and wording (`src/permission.rs`)

- Widen `AlwaysKey` (`permission.rs:75`) to the 3-tuple. Rewrite its doc comment
  (`permission.rs:65-75`): it currently says "keyed by session and tool name",
  "never cleared", and describes the gap as open. All three become wrong on
  landing. State instead what D1 and D3 decide, and why (the D-block reasoning
  above about inert containment).
- Add `fn sticky_scope(tool_name: &str, args: &Value) -> Option<String>`
  implementing D3, returning `Some(args.to_string())` or `None`.
- In `decide` (`permission.rs:289`), build
  `let scope = sticky_scope(&tool_name, &args);` and
  `let always_key = (session_id.clone(), tool_name.clone(), scope.clone());`.
- Update the two cache-hit reason strings (`permission.rs:296` deny,
  `permission.rs:306` allow) so the model-visible reason names the scope, not
  just the tool: `"Always rejected this exact command in this session."` when
  `scope.is_some()`, the existing tool wording otherwise.
- Update `permission_options` (`permission.rs:468`) to
  `permission_options(tool_name: &str, per_command: bool)` and its call site
  (`permission.rs:331`). Pass `scope.is_some()`. A bool, not the command string:
  nothing in the label needs the text, and passing it invites someone to
  interpolate it later.
- Update `apply_outcome`'s two sticky reason strings (`permission.rs:426`,
  `permission.rs:431`) the same way. It already receives `always_key`; take the
  third element rather than adding a parameter.

### 2. Forget a session's answers when the session is evicted

`BridgeState.always` and `.conversations` are never cleaned up. Per-command
keying makes this more visible: one entry per approved command instead of one
per tool.

Bound it on eviction, per `TODO.md`'s cheap fix — `evict_if_needed`
(`adapter.rs:396`) already drops the least recently used `Session`, so have it
drop that session's remembered answers too. This gives "this session" a
defensible meaning (answers live as long as the session is live in memory) and a
session restored from `sessions.json` afterwards prompts again, which is the
safe direction. It also drops remembered *denies*, which is a small downgrade —
the user is asked again rather than auto-denied — and still safe.

**The mechanism in the earlier draft of this plan does not work.** It proposed a
`forget_session_blocking` helper calling `tokio::runtime::Handle::block_on` when
a runtime handle is available. `Handle::block_on` panics *precisely* when a
handle is available and we are inside the runtime driving it:

```
Cannot start a runtime from within a runtime. This happens because a function
(like `block_on`) attempted to block the current thread while the thread is
being used to drive asynchronous tasks.
```

(Verified against this crate's pinned `tokio 1.38.2` with a throwaway
`#[tokio::test]`.) Every caller of `evict_if_needed` reaches it from the async
dispatcher in `main.rs:216`, so the helper would panic on the first eviction in
any run with `--permission-prompts` — and would not show up in the existing
`#[test] evict_if_needed_drops_the_least_recently_used_session`
(`src/tests.rs:2237`), which has `permission_bridge: None` and no runtime, so
the helper returns before ever reaching the panic. A production-only panic that
tests cannot see is the worst available outcome; do not use this.

The constraints that ruled out the obvious alternatives still hold and are
correct as recorded: `forget_session` must be `async` because `BridgeState` is
behind a `tokio::sync::Mutex`; `evict_if_needed`'s two callers
(`handle_session_new` at `adapter.rs:467`, `restore_session_state` at
`adapter.rs:432`) are sync `pub fn`, and making them async cascades through
eight sites in `src/tests.rs` and every caller of `restore_session_state`'s
bool; and an unconditional `tokio::spawn` inside `evict_if_needed` panics in the
existing plain `#[test]`.

**Use a deferred-cleanup queue instead**, and hold it *outside* the adapter
mutex. Add to `Adapter`:

```rust
/// Sessions dropped by `evict_if_needed`, waiting for their remembered
/// permission answers to be forgotten. `evict_if_needed` is sync and the
/// bridge's state is behind an async mutex, so ids are queued here and drained
/// by the dispatcher, which is already async. Deliberately its own lock and
/// not a plain field: the drain must not have to take the adapter mutex.
pending_forget: Arc<std::sync::Mutex<Vec<String>>>,
```

`evict_if_needed` pushes each victim id. `main.rs` keeps a clone of the `Arc`
and drains it at the single common point after the dispatcher's
`let output = match ... ;` and before the write loop:

```rust
let victims: Vec<String> = std::mem::take(&mut *pending_forget.lock().unwrap());
for id in victims {
    if let Some(bridge) = &permission_bridge {
        bridge.forget_session(&id).await;
    }
}
```

Why not a plain `Vec` field drained through the adapter lock, which would be
simpler: two arms of the dispatcher deliberately do not take that lock.
`session/cancel` (`main.rs:265`) handles cancellation without it, and
`session/prompt` (`main.rs:228`) holds it for the entire turn inside a
`tokio::spawn`. Draining through the adapter mutex would make every cancel block
behind a running prompt — turning a cleanup nicety into a cancellation
regression. A separate sync lock is held only for the `mem::take`, never across
an `.await`, so it cannot deadlock with the async mutex.

The `Arc` also covers the spawned-prompt path: `handle_session_prompt` calls
`restore_session_state` → `evict_if_needed` from inside the spawned task, so
those victims are queued while no dispatcher iteration is in flight and get
drained on the next one.

**A queued id can come back before it is drained**, so the queue needs one more
rule. An earlier draft of this plan justified the lag with "session ids are fresh
UUIDs and are never reused" — that is wrong. It is true of `session/new`, which
mints a UUID, but `session/load`, `session/resume` and prompt restoration all
take a *caller-supplied* id that was persisted in `sessions.json`. So the id of
an evicted session can be readmitted to the map, by the host, inside the drain
window.

The fix is not a generation counter. It is to cancel the queued forget when the
id comes back: at both session insert sites — `handle_session_new`
(`adapter.rs:467`) and `restore_session_state` (`adapter.rs:432`) — remove that
id from `pending_forget` before inserting. Sync, inside the adapter, no lock
ordering to reason about, and it states the intent directly: forget this
session's answers unless the session came back.

Note that the residual harm without this rule is bounded — a wrongly drained id
costs the user a prompt and can never grant one, because forgetting only ever
removes remembered answers. It is still worth closing: a permission map that
silently empties under a race is the kind of thing that gets debugged twice.

Add `pub async fn forget_session(&self, session_id: &str)` to the bridge:

```rust
let mut state = self.state.lock().await;
state.always.retain(|(sid, _, _), _| sid != session_id);
state.conversations.retain(|_, sid| sid != session_id);
```

Drop the `conversations` entries too — the `TODO.md` entry names both maps, and
leaving them keeps a dead conversation id resolving to an evicted session.
Do **not** touch `active_session` or `pending`: a pending request belongs to a
turn that is still running and its oneshot must resolve.

### 3. Tests

Rewrite the two tests that pin today's behaviour deliberately — both should turn
red and both should be replaced, not deleted:

- `always_allow_is_remembered_per_tool_not_per_command` (`permission.rs:1816`) →
  `always_allow_is_remembered_per_command_for_command_tools`. Assert that
  allowing `run_command "ls"` does *not* auto-allow `run_command "rm -rf build"`
  (the user is asked again), and *does* auto-allow a second `run_command "ls"`
  with no new prompt. The existing helper
  `bridge_with_run_command_always_allowed` (`permission.rs:1735`) already stamps
  the `"ls"` answer, so both halves reuse it.
- `a_path_inside_a_command_string_is_invisible_to_the_containment_check`
  (`permission.rs:1841`) → split in two, because it currently pins two separate
  facts and only one of them changes:
  - keep the containment observation as a focused unit assertion —
    `outside_workspace` still does not see `/etc/shadow` inside
    `"cat /etc/shadow"`, which remains true and is why the key has to carry the
    command;
  - add `a_path_inside_a_command_string_is_caught_by_the_command_key`, asserting
    that under a remembered allow for `"ls"`, `cat /etc/shadow` now prompts.

  Do not keep the old name on either half: it would read as the opposite of what
  the test shows.
- `the_always_options_say_what_they_cover` (`permission.rs:1762`) → split into a
  command-tool case (new wording) and a path-tool case (`view_file`, wording
  unchanged). Both keep the assertion that all four ACP kinds are present, which
  is independent of wording.

Add:

- `sticky_answers_are_not_normalized` — `"ls"` and `"ls "` produce different
  keys and the second prompts. Pins D2.
- `a_command_tool_is_detected_by_kind_and_by_argument` — `sticky_scope` returns
  `Some` for `run_command` with no `CommandLine` at all (detector 1) and for an
  unknown tool name carrying a nested `CommandLine` (detector 2), and `None` for
  `view_file` with a `TargetFile`. Pins D3, including the nesting that a
  top-level `args.get` would miss.
- `a_differing_cwd_is_a_different_command` — same `CommandLine`, different `Cwd`,
  prompts again. Pins D1's whole-args decision; without it, someone "simplifies"
  the fingerprint back to `CommandLine` and no test objects.
- `always_allow_still_applies_per_tool_for_path_tools` — `view_file` on one file
  still auto-allows a second file. Guards the ergonomics: fingerprinting *every*
  tool's args would make "Always" useless for reads, and this is the assertion
  that says so.
- `forget_session_clears_remembered_answers` — approve with `allow_always`, call
  `bridge.forget_session(&session_id).await`, assert the next identical call
  prompts.
- `readmitting_an_evicted_session_cancels_its_queued_forget` in `src/tests.rs` —
  evict an id, restore it with `restore_session_state`, and assert
  `pending_forget` no longer names it. Pins the rule above; without it the queue
  is one race away from clearing a live session's answers.
- `evicting_a_session_queues_its_answers_for_forgetting` in `src/tests.rs`,
  alongside `evict_if_needed_drops_the_least_recently_used_session` — fill past
  `MAX_SESSIONS`, call `evict_if_needed()`, assert the victim id appears in
  `pending_forget`. Stays a plain `#[test]`; no runtime needed, which is the
  point of the queue.

Existing tests that must stay green untouched:
`always_allow_does_not_bypass_the_sensitive_path_check` (`permission.rs:1591`),
`always_allow_does_not_bypass_the_workspace_check` (`permission.rs:1644`),
`always_reject_still_applies_immediately` (`permission.rs:1693`),
`always_allow_is_remembered_for_later_calls` (`permission.rs:1077`). If the last
one goes red, the fingerprint is including something that varies between two
identical calls — check `toolCallId`/step index has not leaked into `args`.

### 4. Documentation

More than the README. The repo's convention (`TODO.md` header) is that an entry
is *deleted* when the work lands and recorded in `CHANGELOG.md`.

- **`README.md`**, *What "Always" remembers* (line 145). Rewrite: the answer is
  keyed by tool for path tools and by the exact arguments for command tools.
  Remove the `[!WARNING]` at line 160 telling users to prefer **Allow** over
  **Always allow** for `run_command` — it describes the fixed behaviour. Keep
  the paragraph at line 158 explaining that containment does not see paths
  inside a command string, reframed: it is still true, and it is now the reason
  the key carries the command rather than a standing hazard. Also update line
  122, which describes the option labels as `Always allow \<tool\> this session`
  and says they name the tool deliberately.
- **`AGENTS.md` line 96** states the gap as current fact. Rewrite to describe
  the new keying. This file "describes how the code works today and carries no
  work items", so it must not gain a caveat pointing at `TODO.md`.
- **`TODO.md`**: delete the "Permission decisions ignore what a command actually
  does" entry and its *Next Up* pointer. Two things it raises are *not* fixed
  here and need to survive somewhere rather than vanish with the entry:
  - "An 'Always' answer cannot be revoked within a session" — still true. Keep
    as a small entry of its own.
  - Options 2 and 3 (path extraction from command lines, destructive-command
    denylist) — record as deliberately-not-taken depth, with the reasoning, so
    they are not re-proposed as fixes for a gap that is closed.
- **`CHANGELOG.md`**, under `## Unreleased`: this is user-visible behaviour, so
  it is a **Changed**/**Fixed** entry, not **Maintenance**.
- **`pr_compliance_checklist.yaml`**, the "Permission bridge fails closed" rule
  (line 10). Its `success_criteria` ends "Changes must not widen what is
  approved without asking" — this change narrows, so the rule passes, but the
  record of the remembered-answer granularity gap that the rule carries needs to
  come out with the fix.

## Out of scope, deliberately

- **Extracting paths from command lines** (`TODO.md` option 2) so containment
  applies to `run_command`. Real parsing: pipes, `$VAR`, `$(...)`, subcommands,
  attached flag values, and the shell re-interprets the string afterwards
  anyway. Best-effort only and must fail toward prompting. Depth behind this
  change, not a substitute for it.
- **A destructive-command denylist** (`TODO.md` option 3). Evaded by
  `cat .en"v"` or `cat $HOME/.env`. Depth, never the boundary.
- **Bounding `always` by entry count.** Per-command keying turns one entry per
  tool into one per approved command, but every entry still costs a human click
  on "Always allow", so the map cannot be inflated by the model alone. Step 2
  bounds it by live sessions, which is enough. Revisit only if a real session
  shows the map growing.
- **Revoking an "Always" answer mid-session.** Needs UI beyond
  `permission.rs`; stays in `TODO.md`.

## Verification

`cargo test`, then `cargo clippy --all-targets` (expect the one pre-existing
`type_complexity` warning in `protobuf.rs`; anything else is new). Do **not** run
a crate-wide `cargo fmt` — this repo is not rustfmt-clean on `main`. Check only
the touched files with `rustfmt --edition 2021 --check <file>` and hand-fix the
lines this change adds.

Prove each new guard non-vacuous by stubbing it out and confirming the matching
test goes red. The two rewritten pin-tests are self-proving — they fail on
`main` by construction — but the `sticky_scope` detectors and the eviction queue
are not, and a fingerprint that is quietly always `None` passes every test in
step 3 that only checks for reprompts.

End-to-end, the check that matters is the one in `TODO.md`'s Paseo item: drive a
real agent through **Always allow** on one command, then a second, different
command, and confirm the second prompts.

Two host-side preconditions for that run, both established by inspecting the
local Paseo 0.6.1 install:

- The `agy` provider (`agents.providers.agy` in `~/.paseo/config.json`) extends
  Paseo's generic `acp` provider, which exposes an **Auto Accept** toggle —
  "Automatically approves ACP permission prompts". It is currently off, and it
  must stay off for any of this to be observable: with it on, Paseo answers
  every `session/request_permission` itself and no keying behaviour is
  exercised. Confirm with `inspect_provider agy` before the run.
- The binary the provider launches is `agy-acp` from `PATH`. Rebuild and
  reinstall before testing — see the corrected note in `TODO.md`.
