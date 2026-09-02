# Plan: Split Large Files

## Current State / Problem
Sizes verified with `wc -l` on this branch (the TODO.md entry's table is stale —
it predates the inline test modules and undercounts by ~1500 lines):

| Unit | Lines |
|---|---|
| `src/permission.rs` | 3724 total — 1351 code, 2372 in the inline `#[cfg(test)] mod tests` (starts at `permission.rs:1352`) |
| `src/tests.rs` | 2543, one flat module, 86 `#[test]`/`#[tokio::test]` fns, 12 of them `#[ignore]`d I/O tests, 4 of those the `test_e2e_*` tier |
| `adapter.rs::handle_session_prompt` | 317 (`adapter.rs:821`–`1138`) |

- `handle_session_prompt` carries `#[warn(clippy::cognitive_complexity, clippy::too_many_lines)]`
  with the baseline recorded in-line at `adapter.rs:819` (39/25). It is the site of every
  recent turn-lifecycle bug.
- `permission.rs` mixes policy decisions with lower-level path containment logic.
- `tests.rs` is a single flat module, making unit tests for permissions and the adapter
  hard to discover.
- **Risk / Blast Radius**: High. The permission bridge is the most security-sensitive part
  of the code, and the turn lifecycle is the core loop.

## Constraints that shape every phase
These are properties of the crate, not preferences. Violating one turns a mechanical
move into a broken build or a silently-skipped CI job.

- **No `lib.rs`.** This is a binary-only crate (`main.rs` declares every module). Tests
  cannot move to `tests/` — they must stay in-crate as `#[cfg(test)]` modules. Introducing
  `lib.rs` is the larger structural fix and is explicitly out of scope here.
- **`permission` is `cfg(unix)`-gated** (`main.rs:9-13`); Windows compiles
  `permission_unsupported.rs` instead. Any module carved out of `permission.rs` must be
  gated the same way or it is dead code on the `windows-latest` MSRV job.
- **MSRV is 1.70** and the MSRV job builds *and tests* on Windows. No newer std APIs.
- **CI selects e2e tests by substring.** `ci.yml:37` runs `--ignored --skip e2e` and
  `e2e.yml:85` runs `cargo test e2e -- --ignored`. Both match against the *full* test path,
  so any move must keep `e2e` in the path of exactly the tests that need an API key.
  Getting this wrong makes a job silently run zero tests rather than fail.
- **Coverage is reported, not gated.** `ci.yml:62-91` uploads lcov and prints a summary;
  nothing fails on a drop. `cargo llvm-cov` is also not installed locally
  (`cargo install cargo-llvm-cov` + `rustup component add llvm-tools-preview` first).
- **One PR per phase, and within a phase, move-only commits separate from fix-up commits.**
  A commit that both moves and edits is unreviewable; `git log --follow` and blame survive
  a pure move.

## Non-goals
- No behavioral changes, no new features.
- No public API or protocol changes.
- No `lib.rs`, no change to the module cfg-gating scheme.
- If extraction proves too coupled to do cleanly, the phase is aborted rather than forcing
  a messy split. Concrete abort triggers are listed per phase.

## Phase 1: Preparatory Test Coverage
Scope this to the adapter only. The path-containment cases the earlier draft asked for
already exist: symlink escape at `permission.rs:1963-1984`, `..` traversal at
`permission.rs:2010-2015`, and the pure-function battery from `permission.rs:1891`. Do not
re-write them; confirm and leave them.

What is actually uncovered is every call site inside `handle_session_prompt`. Add tests for:
- cancel arriving mid-drain (the `select!` at `adapter.rs:996`),
- child exiting while the stdout reader is still draining,
- the `undrainable` path (`adapter.rs:934`) — reader gone, child must be killed rather
  than waited on forever,
- each of the **three** early returns that build a `JsonRpcResponse` — spawn failure
  (`adapter.rs:914`), non-zero exit / failed result (`:1115`), wait failure (`:1125`) —
  since Phase 3 moves them across a function boundary,
- a cancel arriving while `undrainable` is also set. `was_cancelled` is deliberately
  `cancelled.load(...)` and not the loop's exit condition (`adapter.rs:1004`, with the
  reasoning in the comment above it), so an undrainable turn reports as a failure and a
  cancelled one as `cancelled`. Nothing currently pins that distinction.

While writing these, note a gap the current code already has: the spawn-failure return at
`adapter.rs:914` happens *after* `bridge.set_active_session(Some(...))` at `:852` and
*before* the bridge teardown at `:1047-1057`, so it leaves the bridge pointing at a
session with no `abandon_pending`. The other two returns are downstream of the teardown
block and are fine. Phase 3 must not widen this, and ideally the test written here pins
the intended behaviour before the refactor moves the code. Fixing it is a behavioural
change and belongs in its own commit, not in this plan's move.

New tests may be `#[ignore]`d if they need real I/O — note that ignored non-e2e tests
**do run in CI** (`ci.yml:37`), so an ignored test is still a real gate, and must not
require an API key or it will break that job.

- **Exit Criterion**: `cargo test` passes; `cargo llvm-cov report --summary-only` captured
  and pasted into the PR as the Phase-5 baseline.
### The seam this phase actually needs
None of the drain/cancel tests are writable today. `handle_session_prompt` hardcodes the
binary (`crate::proc::command_in_own_group("agy")`, `adapter.rs:899`), `tokio::process::Child`
cannot be mocked, and the existing I/O tests simply print `SKIP: agy not found in PATH`
(`tests.rs:1202`, `:1303`) when it is absent. So Phase 1's first task is an injection seam:
an `agy_bin` field on `Adapter`, always `"agy"` in production and pointed at a stub by the
tests, driving small shell-script stubs that emit canned stream-json, exit non-zero, hang,
or close stdout early. Not an env override: it would have to cover `fetch_available_models`
(`adapter.rs:165`, which spawns `agy` separately) or model discovery would read a different
binary than the turn runs, and that is production surface bought for a test's benefit. Without that, every
test below degrades to "skipped on CI", which is worse than not claiming coverage.

Decide the seam before writing tests; it is a (small) production change and belongs in its
own commit ahead of the rest of Phase 1.

### The specific tests, and what each pins
- `cancel_ends_the_turn_as_cancelled` — a hanging stub with the flag flipped *mid-turn*,
  so the poll loop has to see the false -> true transition. Asserts `stopReason:
  "cancelled"`. Pins the `was_cancelled` re-read and the `!was_cancelled` failure gate.
- `a_child_that_closes_stdout_is_waited_for_not_killed` — the counterweight, and a
  correction to this plan's original premise. **A stub cannot produce the `undrainable`
  case.** Closing stdout yields EOF, which the drain loop treats as a child that has
  stopped talking (`Ok(false) => break`); the flag needs the read itself to error *and*
  the follow-up drain to a sink to fail as well. That path stays uncovered rather than
  faked, and this test pins the distinction the flag depends on: EOF is waited out, not
  killed.
- `spawn_failure_leaves_no_live_child_and_no_active_session` — `AGY_BIN` pointing at a
  path that does not exist. `LiveChildren::len` already exists (`proc.rs:264`) but is
  private to `proc`; widen to `pub(crate)` to assert on it.
- one test per error response shape (spawn failure `:914`, non-zero exit `:1115`, wait
  failure `:1125`) asserting `error.code == -32000` and no `result`.
- `bridge_has_no_pending_after_spawn_failure` — the gap named above.
  `abandon_pending` returns the number it drained (`permission.rs:302`), so the assertion
  is just `== 0`. Note this is the one test expected to **fail** against current `main`;
  land it `#[ignore]`d with a comment, or fix the bug first in its own commit. Do not
  quietly write it to match today's behaviour.

Not worth writing: a test asserting `abandon_pending` runs on the `:1115`/`:1125` returns
(it already does — the bridge block at `:1047-1057` is upstream of the `match result` at
`:1087`), and anything asserting on `main.rs`'s `pending_prompts` counter, which is a local
in the dispatcher loop (`main.rs:173-243`) with no test surface.

- **Abort trigger**: if the `AGY_BIN` seam is rejected as production surface, say so and
  stop — Phase 3 without these tests is a rewrite of the turn loop with no safety net, and
  should not proceed.

## Phase 2: Extracting Path and Containment Logic
- Create `src/permission/path_rules.rs` (a submodule of `permission.rs`, declared
  `mod path_rules;` inside it). This is preferred over a top-level `src/path_rules.rs`:
  it inherits the `cfg(unix)` gate automatically instead of needing a second one, and it
  keeps the helpers at `pub(super)` rather than widening security-sensitive functions to
  `pub(crate)`.
- Move `outside_workspace`, `is_inside`, `is_inside_from`, `has_parent_component`,
  `resolve`, `lexical_normalize`, and `PATH_FIELDS` (`permission.rs:922-1066`).
  Note `has_parent_component` and `is_inside_from` are part of the same cluster and were
  missing from the earlier draft.
- The one **production** caller is `PermissionBridge::escapes_containment`
  (`permission.rs:570-577`, calling `outside_workspace` at `:574`). That call, not the
  tests, is what the `pub(super)` visibility is for — everything else in `permission.rs`
  reaches these helpers only through it and through `path_field_args`.
- `path_field_args` (`permission.rs:1068`) reads `PATH_FIELDS` but is argument-shaped, not
  containment-shaped. Decide deliberately: either move it too, or re-export `PATH_FIELDS`
  as `pub(super)`. Do not leave it importing through a chain of re-exports.
- Maintain the cross-reference between `PATH_FIELDS` and `dev-docs/agy-tool-surface.md`.
  The pointer lives in the doc, not in the source: `agy-tool-surface.md:5-6` names
  `src/permission.rs` as the home of `PATH_FIELDS` and goes stale on this move.
- Move the corresponding tests (`permission.rs:1891-2015` and the workspace assertion at
  `:3604`) into a `#[cfg(test)] mod tests` in `path_rules.rs`. Check `:3604`'s surrounding
  test first — if it is a policy test that happens to call `outside_workspace`, leave it
  where it is and import.
### Tests to carry this phase
- Put the relocated pure-function tests in `#[cfg(test)] mod tests` **inside**
  `path_rules.rs`. That placement is itself the check: if the move didn't happen, the file
  and its tests don't exist.
- Keep one test exercising `escapes_containment` (`permission.rs:570`) against a path
  outside the workspace. It is the production caller, so it fails to compile if the
  `pub(super)` visibility is wrong — which is the failure mode worth catching.
- Keep one `path_field_args` test, which pins that `PATH_FIELDS` is still reachable from
  `permission.rs` without being widened to `pub`.

- **Exit Criterion**: compiles, `cargo test` passes, and the `windows-latest` MSRV job
  is green (that job is the real cross-platform gate — do not bother with a local
  `cargo check --target x86_64-pc-windows-msvc`, which needs a toolchain that isn't
  installed here and skips test compilation anyway). Test *count* is not sufficient: a
  test that lands in the wrong module still compiles and still counts. Check both —
  `cargo test -- --list | wc -l` unchanged, **and** the moved tests are actually in the
  new file (`grep -c outside_workspace src/permission/path_rules.rs` > 0).
- **Abort trigger**: if the moved functions need private state from `PermissionBridge`.

## Phase 3: Refactoring `handle_session_prompt` (adapter.rs)
Split the 317-line function into phases. Two structural problems the earlier draft did
not name, and which determine the shape of the split:

1. **The function has five early returns that each construct a full `JsonRpcResponse`.**
   Extracted phases cannot `return` on the caller's behalf. Thread failures as
   `Result<T, TurnFailure>` where `TurnFailure` carries the message, and build the
   response once at the top level from `id`. Anything else will silently change which
   teardown runs on the error paths — which is exactly the class of bug this function
   keeps producing.
2. **`child_guard` (`adapter.rs:926`) is an RAII registration** deliberately taken before
   anything can fail, and dropped at `adapter.rs:1015` — the moment the pid is reaped,
   with the comment there explaining why it cannot outlive that point (pid reuse). So the
   guard's life spans spawn→drain and **ends before** the bridge teardown at `:1047-1057`
   and the persist at `:1058-1070`. A `teardown_turn` phase therefore cannot own it. Put
   it in `RunningTurn` and have the drain phase consume the struct, or give `RunningTurn`
   a `Drop`; do not let the guard leak past the reap just because it is convenient for
   the phase boundary.
3. **There is one `result` match, and both `select!` arms must feed it.** The cancel arm
   (`adapter.rs:997-1012`) kills the tree and then calls `child.wait()`, so the kill path
   and the normal path produce the same `Result<ExitStatus, _>` consumed at `:1087`. If
   `drain_agy_io` returns anything that collapses those two into one shape, the wait-error
   return at `:1125` becomes unreachable on a kill. State explicitly that the kill and the
   subsequent `wait` both live inside `drain_agy_io`.
4. **`was_cancelled` is not "the loop exited".** It is `cancelled.load(...)` re-read at
   `:1004`, so the undrainable case is a failure, not a cancel, and `:1096` gates the
   error response on `!was_cancelled && !denied_by_user`. Whatever `drain_agy_io` returns
   must carry `was_cancelled` as its own value, not let the caller re-derive it from
   "did we take the kill path".

Suggested seams:
- `spawn_agy_process`: argument assembly (`adapter.rs:855-899`), spawn, `child_guard`
  registration. Returns `RunningTurn` or `TurnFailure`.
- `drain_agy_io`: the two reader tasks, the `undrainable` flag, and the `select!` racing
  the child against cancellation (`adapter.rs:928-1018`).
- `teardown_turn`: bind conversation id, bridge teardown (`refused_during_prompt`,
  `register_conversation`, `set_active_session(None)`, `abandon_pending`), session persist
  (`adapter.rs:1041-1070`). The kill lives in `drain_agy_io`, not here — see point 3.

Borrow-checker note: `RunningTurn` must **not** hold `&mut self`. The phases read
`self.working_dir`, `self.sessions`, `self.permission_bridge`, `self.hook_root_dir` and
`self.live_children`; pass an owned `TurnSpec` (cloned strings, `Arc` bridge handle) into
the phases so `&mut self` is free for the persist step. A struct borrowing `self` will
compile only after the split is contorted around it.

The single-writer invariant is about **`notify_tx`** (`adapter.rs:817`, doc comment at
`:817-819`) — the main loop is the only writer of stdout, and the stream reader never
touches the fd. The earlier draft called this `out_rx`, which does not exist. The
extracted `drain_agy_io` must keep `notify_tx` as its only output channel.

- **Exit Criterion**: the attribute at `adapter.rs:820` becomes
  `#[deny(clippy::cognitive_complexity, clippy::too_many_lines)]` and
  `cargo clippy --all-targets -- -W clippy::cognitive_complexity -A clippy::type_complexity`
  is clean for each extracted helper too, not just the parent (splitting a function can
  leave one helper over the threshold).
- **Also required, or the repo goes stale**: update the baseline comment at
  `adapter.rs:819`, delete the "until refactor" clippy-policy comment at `ci.yml:51-52`
  (this phase is what that comment is waiting on), and update the TODO.md entry. Record a
  per-helper complexity number the way `adapter.rs:819` records the current one —
  otherwise a helper that inherits the parent's complexity passes the gate unnoticed.
- **E2E is required here, not optional.** This phase rewrites exactly what e2e covers.
  Run `cargo test e2e -- --ignored --nocapture` locally. It needs `agy` on PATH and
  auth, but **not** a `GEMINI_API_KEY`: `prepare_auth` accepts an existing
  `~/.gemini/antigravity-cli/settings.json` keyring login, which is what a developer
  machine already has. The key is a CI requirement, because a runner has no keyring.
  Build `--release` first; the tests drive the built binary. Do not merge on unit tests
  alone. What the CI route costs, if the local one is unavailable:
  `e2e.yml` gates on the protected `e2e` environment and needs a maintainer to approve the
  run (`e2e.yml:1-4, 16-24`), and it skips entirely for fork PRs, which cannot receive the
  secret. So for this phase, plan on the local run as the primary path -- it needs only
  `agy` and an existing keyring login -- and treat the workflow as confirmation.
- **Abort trigger**: if `TurnFailure` threading turns into more code than it removes, keep
  the function whole and settle for extracting only `drain_agy_io`.

## Known coverage gaps after Phase 3
Named here rather than papered over, because the next person to touch the turn
loop needs to know which invariants the tests do not hold:

- **The refusal path.** `teardown_turn` reads `refused_during_prompt` before
  clearing the binding, and the window between the clear (which bumps the
  generation) and the read is where a genuine refusal could be lost. Pinning it
  needs `mark_user_refusal`, which is private and generation-gated; exposing it
  for a test would widen a security API for a test's benefit, which is the trade
  the `agy_bin` seam already refused. The ordering is carried by its comment.
- **`register_conversation` / `abandon_pending` on teardown** are unasserted. A
  regression skipping them leaves stale pending requests to land in the *next*
  turn as phantom refusals.
- **The wait-error arm** (`failed to wait for agy`) is unreachable from a stub:
  nothing a shell script does makes `child.wait()` fail.
- **`undrainable`** — see Phase 1. Testable only on its cancel half.
- **Persist-on-success**: no turn-lifecycle test asserts `conversation_id`,
  `last_step_idx` or the `persist_session` call.

## Phase 4: Splitting `tests.rs`
Note the tension the earlier draft missed: `permission.rs` is already the largest file in
the tree *because* 2372 of its lines are tests. Migrating more tests into it makes the
stated problem worse. So the target is not "tests live next to source" but **no file over
~1200 lines**, which means test modules get their own files:

- Give each subject a test file as a submodule of its source module —
  `src/permission/tests.rs`, `src/adapter/tests.rs`, `src/streaming/tests.rs`,
  `src/protobuf/tests.rs` — declared `#[cfg(test)] mod tests;` in the parent.
- Move `permission.rs`'s existing inline `mod tests` (`:1352`–end) into
  `src/permission/tests.rs` in the same pass. Without this, Phase 4 leaves the biggest
  file untouched.
- The 12 `#[ignore]`d I/O tests, four of which are the e2e tier (`tests.rs:1191`, `:1380`,
  `:1432`, `:1485` — matching the "four e2e tests" the comment at `ci.yml:35` names), drive
  the built binary rather than any one module. Put them in `src/e2e_tests.rs`, gated
  `#[cfg(test)]`, so that **`e2e` stays in the test path** — `ci.yml:37`'s `--skip e2e` and
  `e2e.yml:85`'s `e2e` filter both depend on that substring. Verify with
  `cargo test -- --list | grep e2e` before and after: same set. The substring cuts both
  ways — any *future* test whose path contains `e2e` (say `e2e_path_safety`) gets silently
  dropped from the ignored-tier job. `ci.yml:35` already carries a warning to that effect;
  do not weaken it. If that warning is worth enforcing, enforce it from CI or a script
  (`cargo test -- --list | grep e2e` compared against the four known names), not from a
  `#[test]` that shells out to `cargo` — a nested cargo invocation inside the test binary
  is slow and fights the parent for the build lock.
- The shared helpers at the top of `tests.rs` (`push_varint`, `push_len_field`,
  `send_prompt_wait` at `:1337`, …) span subjects. Check who actually uses each before
  building infrastructure: the varint pushers are protobuf-only and should just move into
  `protobuf/tests.rs`. Only promote to a `#[cfg(test)] mod test_support;` the ones with two
  or more consumers — a shared module every test file must import is worse than a helper
  sitting next to its single caller.
- Delete `src/tests.rs` and its `mod tests;` in `main.rs:20-21` once empty.
- **Exit Criterion**: `src/tests.rs` gone; `cargo test -- --list` yields the same 86 tests
  by name; `cargo test -- --list | grep -c e2e` unchanged; no source file over ~1200 lines.
- **Ordering**: Phase 4 must land after Phase 2 (both touch the permission test module)
  and after Phase 3 (Phase 3 adds `TurnFailure` and the tests that exercise it in
  `adapter.rs`; moving `adapter::tests` to a child module first means those tests get
  written in one file and immediately relocated). If phases are in flight together, rebase
  rather than merge — a conflict in a 2000-line move is not resolvable by hand.

## Phase 5: Verification
- **Unit Tests**: `cargo test` exits 0, and `cargo test --verbose -- --ignored --skip e2e`
  exits 0 (this is what CI runs; a plain `cargo test` skips the I/O tests).
- **Coverage**: compare `cargo llvm-cov report --summary-only` against the Phase 1
  baseline. Do not treat a small percentage move as a failure by itself — moving code
  between files changes the region denominator, and llvm-cov region coverage drifts for
  reasons unrelated to lost tests. The real check is: same test names pass, and the
  per-function coverage of the *moved* functions (`outside_workspace`, `is_inside`,
  `resolve`, `lexical_normalize`) is unchanged.
- **Linting**: run CI's exact command, `cargo clippy --all-targets -- -W clippy::all -D clippy::all`,
  plus the cognitive-complexity reporting step from `ci.yml:57`.
- **Cross-platform**: the `windows-latest` MSRV job is the only thing that catches a
  mis-gated module. It must be green — a green Linux build proves nothing about Phase 2.
- **E2E**: `cargo test e2e -- --ignored --nocapture` locally (keyring auth is enough --
  see Phase 3), or a green `e2e.yml` run. Required for Phase 3; nice-to-have for the
  others.

## Sequencing note
TODO.md is explicit that this must not run while a behavioural change is in flight. Check
for open PRs touching `adapter.rs` or `permission.rs` before starting Phase 2 or 3.
