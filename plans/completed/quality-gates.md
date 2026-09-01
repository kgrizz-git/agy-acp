# Plan: Automated quality gates — coverage, lint, complexity, size

> **TODO discipline:** Keep the corresponding `TODO.md` entry and its Next Up
> pointer until this work lands. They are linked from this plan; delete them
> only in the landing change.

## Objective

Stop the situation `TODO.md` records: every quality property this fork cares
about — tests, lint, complexity, file size — is held up by review habit alone.
The TODO entry has the receipts: a test passed while proving nothing on the
way to PR #7, and the only thing that caught the gap was the rule that
"remembered answers must not widen what is approved without asking."

This plan wires four gates in `ci.yml` plus a local pre-push, in the order
they reinforce each other:

1. **`cargo clippy --all-targets -- -D warnings`** as a CI job. **As landed
   (PR #10):** `-W clippy::all -D clippy::all` instead — `handle_session_prompt`
   uses `#[warn(clippy::cognitive_complexity, clippy::too_many_lines)]`, and
   `-D warnings` would deny those function-level warnings.
2. **`cargo llvm-cov` coverage report** as a CI job, posted to the job summary.
3. **Clippy nursery lints** for `cognitive_complexity` and `too_many_lines`,
   pinned against `handle_session_prompt` as `#![warn]` only — `#![deny]` is
   the move the refactor entry owns.
4. **Pre-push local hook** scoped to what is fast: clippy + unit tests, with
   the existing fork-guard rule folded into the same file. Skips on
   `SKIP_LOCAL_GATES=1`.

The plan **does not** include the `handle_session_prompt` refactor itself, a
file-size cap, a formatting decision, or SonarCloud. Those are recorded in
the TODO and called out under "Out of scope, deliberately" below.

## Why the order

- **Clippy first, then coverage** — clippy turns up structural problems (the
  `type_complexity` warning surfaces today; another will show up the day
  `llvm-cov` is wired in and the workflow file gets reviewed). Fixing them
  with `-D warnings` already in place is cheaper than fixing them after a
  coverage threshold has gone in.
- **Coverage as a report, not a threshold** — `TODO.md` says it directly:
  "a threshold set today would mostly measure the non-Unix fallbacks that
  cannot run on the Linux runner and the e2e tier CI skips, and would push
  toward tests written to move a percentage. What is actually wanted is a
  check that new code arrives with tests, which a raw number only
  approximates." Reporting the LCOV is the right shape: it makes *uncovered
  lines* visible to a reviewer, who can ask "is that an architectural seam
  or a gap?" — a per-PR judgement, not a number.
- **Cognitive complexity at warn-only on `handle_session_prompt`** — the TODO
  says it is the only function that has grown past easy reading. Promoting
  the lint to `#![deny]` is the move that would make a future growth a build
  break; that belongs with the refactor (TODO #2), not the gate.
- **Local hook last and scoped** — once the CI gates exist, the local hook
  exists to fail fast *while writing*. The TODO says explicitly: "A hook that
  duplicates CI slowly is worse than no hook." So the local hook is
  clippy + unit tier only; ignored I/O and e2e are out.

## Correcting the TODO entry

Two of the TODO entry's claims do not match the current tree and need to be
corrected in the same PR that does the work:

1. `TODO.md` says `cargo clippy` reports "`very complex type`, from the
   `(u32, u32)` process-table pair" — neither is right. The actual warning
   is `clippy::type_complexity` on the return type of a protobuf walker
   (`src/protobuf.rs:435`):

   ```rust
   ) -> Option<(
       Option<String>,
       Option<String>,
       Option<Value>,
       Option<String>,
   )> {
   ```

   Fix is a type alias or struct for the 4-tuple of optionals, named for what
   the walker returns (something like `WalkedToolFields`). The `(u32, u32)`
   description was the right *shape* of fix for a different warning that
   must have existed at some point but is no longer reported.
2. `TODO.md` says the existing `Adapter` struct literal count is 15. As of
   `f60efc9` (the post-PR #9 tree this plan lands against) it is **18**:
   `src/tests.rs:643, 682, 799, 946, 992, 1037, 1075, 1112, 1147, 1699,
   1721, 2130, 2187, 2239, 2268, 2325, 2380` plus the `test_adapter` helper
   signature at `src/tests.rs:105` which `grep -c` counts but is not a
   literal. The 15 was from an earlier tree. Mentioned in the plan because
   any new `Adapter` field from the refactor entry will have to update all
    18 sites; adding a docstring on `Adapter` (`src/adapter.rs:58`, which is
    currently bare) noting the count and referencing this plan is a useful
    drive-by.

`handle_session_prompt` is 319 lines, not 322. The 322 was correct at the
time the TODO entry was written and the plan keeps the same shape of
description ("the one function that has grown past easy reading") rather
than the byte count.

The plan **does not** update the TODO entry to fix the numbers — that is a
**landing** change, per the discipline this plan and the TODO both state.
The plan *records* the corrections here so the landing change has them.

## What the plan touches

| File | Change |
|---|---|
| `src/protobuf.rs` | Add a type alias (or struct) for the 4-tuple of optionals the protobuf walker returns at `src/protobuf.rs:435`. Single source of the warning. |
| `.github/workflows/ci.yml` | Add a `clippy` job (no `needs:`; clippy compiles its own artefacts), add a `coverage` job that depends on `test` and runs `cargo llvm-cov --workspace --lcov --output-path lcov.info` and posts the LCOV report to the job summary. Both gated the same way as `ci.yml`'s existing jobs (push, PR, manual dispatch). |
| `src/adapter.rs` | Add `#[warn(clippy::cognitive_complexity, clippy::too_many_lines)]` on `handle_session_prompt` only. **Warn, not deny** — the refactor entry promotes to deny. Crate-wide `#![warn]` in `main.rs` plus per-function `#![allow]` across the tree is the wrong default here: hundreds of functions would need an allow, and the lint only needs to fire on the one function the TODO names. |
| `.githooks/pre-push` (update / expand) | The local gate. Adds `cargo clippy --all-targets -- -D warnings` and `cargo test` (unit tier) around the existing fork-guard URL check. `SKIP_LOCAL_GATES=1` skips clippy and tests only; the fork guard always runs. Rewrite the header comments — they still say the canonical copy lives in `.git/hooks/pre-push`, which this change supersedes. |
| `AGENTS.md` | **Commands:** add `cargo clippy --all-targets -- -D warnings`. **Test tiers / CI:** note the new `clippy` and `coverage` jobs. **Local gotchas:** replace the `cp .githooks/pre-push .git/hooks/...` paragraph with `git config core.hooksPath .githooks`; document `SKIP_LOCAL_GATES=1`; document `cargo install cargo-llvm-cov --locked` for local coverage (not a dev-dependency). **Relationship to upstream:** update the pre-push bullet to match the `core.hooksPath` install. |
| `TODO.md` | **Landing change, not this PR.** Delete the "Automated quality gates" entry and its "Next Up" pointer. Keep the **SonarCloud** entry — the decision that goes with these gates is its own PR. The **Formatting** paragraph (no rustfmt gate today) is also a separate decision and stays. Move `plans/quality-gates.md` to `plans/completed/quality-gates.md` in the same landing commit, per the convention the harness plan establishes. |
| `CHANGELOG.md` | **Landing change, not this PR.** Under `## Unreleased` → **Maintenance**: the four gates, the local-hook installation step, and the `WalkedToolFields` rename as a small drive-by. |
| `pr_compliance_checklist.yaml` | **Landing change, not this PR.** No rule changes today; the new gates are the enforcement of rules that already exist. Add a one-line note that clippy and the coverage report are now part of the standard PR review checklist. |

The TODO/CHANGELOG/checklist edits are explicitly *not* in this PR because
the discipline (`TODO.md` and `AGENTS.md` both state it) is: plans land
together with their work, not before. Doing the doc edits in this PR would
leave the working tree in a state where the gates described don't exist,
which is exactly what the discipline exists to prevent.

## Step-by-step

### 1. Make `cargo clippy --all-targets -- -D warnings` clean

Run `cargo clippy --all-targets -- -D warnings 2>&1 | tee /tmp/clippy.log`
locally. Exactly one warning is expected: `clippy::type_complexity` on
`src/protobuf.rs:435`. If anything else shows up it is new and is fixed or
suppressed in this PR with a comment that names the reason, in the same
form the existing protobuf warnings carry.

The `(u32, u32)` description in the TODO is wrong; the actual fix is a
type alias for the 4-tuple. Naming it `WalkedToolFields` (a tuple struct or
`pub type` is fine; tuple struct is preferable because the walker's call
sites use it positionally today) and threading the new name through the
one or two return sites makes the diff mechanical. **Verify after**: the
function signature at `src/protobuf.rs:435` reads with a named type, the
`Some((x, y, z, w))` construction sites become `Some(WalkedToolFields(x, y,
z, w))` or `Some(WalkedToolFields { … })`, and the destructuring consumers
either destructure by position or by field name.

Pin a `cargo-llvm-cov` install step. No workflow uses `taiki-e/install-action`
today; follow the same SHA-pinning discipline as `actions/checkout` and
`dtolnay/rust-toolchain` already in `ci.yml`. Options, in preference order:
`taiki-e/install-action@<sha> # cargo-llvm-cov`, or
`cargo install cargo-llvm-cov --locked --version <x.y.z>` with `<x.y.z>`
resolved at implementation time. `cargo-llvm-cov` is a binary, not a library —
it does not belong in `Cargo.toml` dev-dependencies.

Wire the clippy job into `ci.yml` as a separate job with **no `needs:`**.
`cargo clippy --all-targets` compiles its own artefacts; waiting on `test`
or `msrv` only adds latency. Same runner image and SHA-pinned toolchain as
`test`. The `msrv` job already tests 1.70; clippy runs on stable, and the
`-D warnings` bar applies to the same code stable compiles.

### 2. Wire `cargo llvm-cov` as a report, not a gate

`ci.yml` gains a `coverage` job that runs `cargo llvm-cov --workspace
--lcov --output-path lcov.info` and posts the LCOV report to the job
summary. **No threshold**, per the TODO argument above.

The reason "report, not gate" matters concretely: today `tests.rs` has
2543 lines and the non-Unix `proc.rs` paths are unreachable on the Linux
runner. A threshold set against this tree would either pass (and tell
nobody anything) or fail (and force tests written to move a percentage
rather than cover a risk). The point of a coverage report is to make the
*uncovered* lines visible so a reviewer can ask "is that an architectural
seam or a gap?" — and the answer to that question is a PR-by-PR judgement,
not a number.

Future: a follow-up PR can promote the report to a gate once a meaningful
baseline has been established. The TODO entry records the threshold problem
so a follow-up author can find it.

The coverage job uses the same `actions/checkout` SHA pin and the same
toolchain as the existing jobs. Outputs `lcov.info` as a workflow
artifact (named with the run id). **Do not paste raw LCOV into
`$GITHUB_STEP_SUMMARY`** — it is unreadable there. Post a one-line
summary built from `cargo llvm-cov --workspace --summary-only` (or
`llvm-cov report` on the generated file) naming the headline line/region
percentages plus a pointer at the artifact, so a reviewer can download
the LCOV and open it in an editor or IDE plugin.

### 3. Pin the cognitive-complexity lint on `handle_session_prompt`

Apply `#[warn(clippy::cognitive_complexity, clippy::too_many_lines)]`
directly on `handle_session_prompt` in `src/adapter.rs`. **Warn, not
deny** — surfaces the problem without failing CI, and the refactor entry
(TODO #2) is the PR that promotes to deny.

`too_many_lines` defaults to 100; the function is ~318 lines today, so
both lints will fire immediately. That is intentional: the baseline is the
score *before* the refactor, not a clean tree.

Record the cognitive-complexity score as the baseline for the refactor
entry:

```bash
cargo clippy --all-targets --message-format=json -- \
  -W clippy::cognitive_complexity -A clippy::type_complexity \
  | jq 'select(.reason=="compiler-message") | .message'
```

(`-A clippy::type_complexity` keeps the output focused until the
`WalkedToolFields` fix from step 1 lands.) The number goes in the landing
commit's CHANGELOG entry so the refactor has a measurement to beat.

**No new unit tests for the lint wiring.** The gate is configuration;
proving it works is the verification bullets below. Optional: `shellcheck
.githooks/pre-push` in verification if `shellcheck` is available locally.

### 4. Add the local gate

Two pieces.

First, a `.githooks/pre-push` that runs:

1. The existing fork-guard URL check (refuses any push whose URL is not
   `kgrizz-git/agy-acp`). Already lives in the file; the expansion adds
   the clippy and test calls around it.
2. `cargo clippy --all-targets -- -D warnings`. Times out at 5 minutes;
   gate exists to fail fast, not to wait for a slow check.
3. `cargo test` — unit tier only. The ignored and e2e tiers need a network
   or `agy`, which the TODO's hook guidance explicitly excludes.

Honor `SKIP_LOCAL_GATES=1` for one-off bypasses of clippy and tests only;
the fork-guard URL check always runs. Exit non-zero on any failure.

Second, a `core.hooksPath` instruction in `AGENTS.md` "Local gotchas":

```text
After cloning, run `git config core.hooksPath .githooks` once. The
pre-push hook runs cargo clippy and the unit-tier tests; set
`SKIP_LOCAL_GATES=1` to bypass for a single push.
```

The pre-push already lives at `.githooks/pre-push` (committed) and runs
the fork-guard; the expansion folds the clippy and unit-test calls into
the same hook. `core.hooksPath` makes the install a one-line step. The
older `AGENTS.md` paragraph about a hook at `.git/hooks/pre-push` was
written before the file moved into the repo; once this lands, the local
gotcha is `git config core.hooksPath .githooks` and nothing else.

## Verification

- `cargo clippy --all-targets -- -D warnings` exits 0 from a clean tree.
- `cargo test` exits 0 (already does — 150 passed, 12 ignored, as of
  `f60efc9`).
- `cargo test -- --include-ignored` (the I/O tier) exits 0 on a local
  macOS runner — sanity check that the gate work did not disturb the
  ignored tier.
- The CI workflow, run via `act` if available locally or pushed to a draft
  branch, shows the four jobs green: `test`, `msrv`, `clippy`, `coverage`.
- `cargo llvm-cov` produces a `lcov.info` and the workflow posts the
  coverage summary; **manually read the output** for one or two high-risk
  lines (`handle_session_prompt` is the obvious target) and confirm the
  report names them. This is the only test that proves the report is
  non-vacuous — the rest is configuration.
- Pre-push hook: with a known-failing test temporarily inserted, `git push`
  exits non-zero before any network call. With `SKIP_LOCAL_GATES=1` set,
  the same push succeeds.
- `handle_session_prompt` produces a clippy `cognitive_complexity` warning
  in the JSON output. The default threshold is 25; record the actual score
  in the landing commit so the refactor entry has a number to beat.

## Coordination with harness

Both plans land in the **same PR**. The harness plan must create
`plans/completed/` before this plan's landing step moves
`plans/quality-gates.md` there. See `plans/completed/harness.md` → "Coordination
with quality-gates" for the full commit order.

## Out of scope, deliberately

- **The `handle_session_prompt` refactor** — TODO #2. This plan only adds
  the lint that will catch the next growth; the refactor itself is its own
  PR because it would touch the function's body and obscure the gate
  change. The TODO says it directly: "Do not do this while a behavioural
  change is in flight."
- **File-size cap** — TODO says the cap is only worth setting once the
  *shape the code should end up in* is decided. The refactor decides the
  shape.
- **New Rust unit tests** — the gates are CI/hook configuration. Proving
  they work is the verification section (clippy clean, workflow green,
  hook blocks a bad push). No `#[test]` for "clippy passes."
- **rustfmt gate** — separate TODO decision; pre-push is clippy + unit
  tier only.
- **SonarCloud** — the TODO entry on it is the deliberate *not-taken*
  decision that goes with these gates. Resolve as a follow-up: either turn
  it on and own two sources of truth for Clippy findings, or leave it off
  and add a one-line CHANGELOG note pointing at the cargo clippy/llvm-cov
  coverage this PR provides. Per the TODO: "The decision, before any of
  the above: Sonar would run Clippy and import coverage, which is most of
  what the quality-gates entry above wants from `cargo clippy` and `cargo
  llvm-cov` directly. Running both means two sources of truth for the same
  findings and a second place to silence a lint. Worth picking one
  deliberately rather than adding Sonar because the account is already
  there."
- **Bounding `always` by entry count** — TODO says per-command keying makes
  the map slightly larger but still bounded by user clicks and live
  sessions. Not a gate problem.
- **TODO/CHANGELOG/checklist edits** — landing changes per the discipline
  this plan and the TODO both state. Recorded here so a follow-up author
  has the list.

## How the four gates interact with the existing CI

The existing `ci.yml` runs `cargo build`, `cargo test` (unit tier), and
`cargo test -- --ignored --skip e2e` (the ignored I/O tier without e2e),
on stable, plus an MSRV build of both on 1.70 Linux and 1.70 Windows. All
jobs already use SHA-pinned `actions/checkout` and `dtolnay/rust-toolchain`,
and the workflow's permissions are `contents: read` with
`persist-credentials: false` — the same pattern is what the two new jobs
inherit.

The two new jobs:

- `clippy`: no `needs:`. `cargo clippy --all-targets` compiles its own
  artefacts. Uses stable. Runs `cargo clippy --all-targets -- -D warnings`.
- `coverage`: depends on `test`. Runs `cargo llvm-cov --workspace --lcov
  --output-path lcov.info` and `actions/upload-artifact` with the LCOV
  file. Posts a one-line summary via `$GITHUB_STEP_SUMMARY`.

The `msrv` job continues to use 1.70 with the same matrix; the clippy
`type_complexity` warning was present under stable today and the `WalkedToolFields`
rename must build under both stable and 1.70.

## Host constraints the implementation has to respect

Carried over from the permission-command-keying plan, because the same
host has the same ACP wire format:

- **One writer to stdout.** None of the four gates touches the wire
  format, but if a clippy lint ever suggested "consolidate these two
  writers" against `main.rs`, the response is no — AGENTS.md and the
  permission-command-keying plan both record why.
- **The TODO/CHANGELOG discipline is a hard rule.** No TODO deletions in
  this PR, no CHANGELOG entries in this PR. They land in the same commit
  as the work.
- **No bare `cargo fmt`.** `AGENTS.md` says the repo is not rustfmt-clean.
  The `WalkedToolFields` rename may produce one or two lines that exceed
  the formatter's idea of width; `rustfmt --check` against the touched
  file only, hand-fix any drift, and confirm the file is otherwise
  unchanged. The new pre-push hook is `clippy + test`, not `fmt`, for
  exactly this reason.
- **Pre-push hook is a fast signal, not a slow duplicate of CI.** Clippy
  and the unit tier only. The ignored and e2e tiers need agy + a key; they
  belong in CI, not in the pre-push.
