# Plan: Add a CI workflow (build + test)

> **TODO discipline:** The corresponding TODO.md entry ("Build and test in CI",
> lines 185–194) and its Next Up bullet (lines 19–20) stay on the board **until
> this work lands** — a plan is not completion. This plan *links* to them;
> deletion is the **final step of implementation** (the PR that merges the
> workflow), never a planning step. Per repo rule, entries are deleted on
> landing, not ticked.

## Context

There is **no** build/test CI. Only `upstream-watch.yml` exists
(`.github/workflows/upstream-watch.yml`). From `TODO.md` (Fork maintenance →
"Build and test in CI") and `AGENTS.md`:

- The absence is *why* `test_read_response_from_db` rotted unnoticed: it is
  `#[ignore]`d and nobody runs `--include-ignored` by hand.
- A workflow should run `cargo build`, `cargo test`, and
  `cargo test -- --include-ignored`, **skipping the four e2e tests** (they need
  `agy` + auth).
- e2e must be gated behind a `GEMINI_API_KEY` secret so it *skips* (not fails)
  when absent.
- **Do NOT add `cargo fmt --check`** — the tree is not rustfmt-clean and it would
  reflow files no change touched (explicitly warned in both TODO.md and
  AGENTS.md "Local gotchas").

### Test tiers (from AGENTS.md)

1. **Unit** (`cargo test`) — fast, no network, no real `$HOME`.
2. **Ignored I/O** (`cargo test -- --include-ignored`) — session persist/restore
   and conversation-DB reads. `#[ignore]`d by inheritance.
3. **E2E** (`cargo test e2e -- --ignored`) — spawn release binary, need `agy` +
   auth + `cargo build --release` first. These are the ones to skip in CI.

### e2e gating mechanics

The e2e tests are selected by name (`e2e`). Running plain `cargo test` already
excludes `#[ignore]`d tests, but the e2e tests are *both* `e2e`-named and
`#[ignore]`d, so `cargo test -- --include-ignored` would pick them up. To skip
them in CI without a secret, run `cargo test -- --include-ignored --skip e2e`.
When `GEMINI_API_KEY` is present, run the e2e subset explicitly:
`cargo test e2e -- --ignored --nocapture` (after `cargo build --release`).

## Steps

1. **Create `.github/workflows/ci.yml`** (the always-on build/test gate). Triggers:
   `push: branches: [main]` only, plus `pull_request: branches: [main]` and
   `workflow_dispatch:`. Do **not** add `push: feat/**`: a `pull_request` to `main`
   already covers topic branches in this solo fork, and a push ref
   (`refs/heads/feat/x`) and its PR ref (`refs/pull/N/merge`) form *different*
   concurrency groups, so `cancel-in-progress` cannot dedupe them — keeping both
   triggers just double-runs, not saves. (If you later want push-on-branch
   cancellation without the double-run, normalize the group instead.) Add:
   ```yaml
   concurrency:
     group: ${{ github.workflow }}-${{ github.ref }}
     cancel-in-progress: true
   ```
   Add a top-of-file comment: `# Do NOT add cargo fmt --check or clippy here —
   the tree is not rustfmt-clean (AGENTS.md "Local gotchas"; TODO.md:194).`
   Use `ubuntu-latest`.

2. **Job: `test`.**
   - `actions/checkout@v4`.
   - Install Rust stable via `dtolnay/rust-toolchain@stable` with `profile:
     minimal` (no extra components). Add `Swatinem/rust-cache@v2` (or
     `actions/cache` on `~/.cargo`) so the cold `cargo build` is not slow/flaky.
   - `cargo build --verbose` — fails the build on any compile error.
   - `cargo test --verbose` — tier 1 (unit).
   - `cargo test --verbose -- --include-ignored --skip e2e` — tier 2 (ignored
     I/O). The `--skip e2e` substring excludes all four `test_e2e_*` tests
     (`tests.rs:1305,1494,1546,1599`); note in a workflow comment that the
     ignored tier needs only `$TMPDIR` (no `agy`, no network), which is why it is
     safe here, and that `--include-ignored` intentionally re-runs tier-1 too.

 3. **e2e is intentionally NOT in `ci.yml`.** `AGENTS.md` treats e2e as local-only
    (needs `agy` on PATH + `GEMINI_API_KEY`/Keychain + `cargo build --release`),
    and `TODO.md:191` says gate it so it *skips* rather than fails. Installing
    `agy` from a GitHub release inside CI is fragile (undocumented asset names,
    OS/arch detection, version pinning) and contrary to that intent. Instead:
     - **Create `.github/workflows/e2e.yml`** as an opt-in gate. Triggers:
       `workflow_dispatch:` and `pull_request: branches: [main]` (scoped to main,
       same as `ci.yml`; no `push`).
     - **Gate on the secret with a gate job**, because `secrets` is **not** a
       valid context in a job-level `if:` (it would fail parsing with
       "Unrecognized named-value: 'secrets'"). Use:
       ```yaml
       permissions:
         contents: read
       jobs:
         gate:
           runs-on: ubuntu-latest
           outputs:
             has_key: ${{ steps.k.outputs.has_key }}
           steps:
             - id: k
               env:
                 KEY: ${{ secrets.GEMINI_API_KEY }}
               run: echo "has_key=$([ -n "$KEY" ] && echo true || echo false)" >>"$GITHUB_OUTPUT"
         e2e:
           needs: gate
           if: needs.gate.outputs.has_key == 'true'
           permissions:
             contents: read
           steps:
             # ... checkout, rust toolchain, cargo build --release, agy install, cargo test e2e ...
       ```
       This makes the `e2e` job show as **Skipped** (not Failed, not a parse
       error) when `GEMINI_API_KEY` is absent. Note in a workflow comment: a
       `GEMINI_API_KEY`-set run whose `agy` install fails will make the e2e tests
       *self-skip* rather than fail, so a green opt-in run only proves coverage when
       `agy` installed correctly.
     - e2e steps: `actions/checkout@v4`, rust toolchain (as above),
       `cargo build --release --verbose`, install `agy` from a **pinned**
       `google-antigravity/antigravity-cli` release (document the exact asset URL +
       arch handling + `chmod +x` + PATH export, and the maintenance burden, in a
       workflow comment), set `AGY_EXTRA_ARGS=""`, then
       `cargo test e2e -- --ignored --nocapture` with `GEMINI_API_KEY` from
       secrets. This keeps e2e runnable on demand without risking the main gate.

 4. **Permissions.** `permissions: contents: read` for both `ci.yml` and the
    `e2e.yml` gate/e2e jobs. Scopes omitted from a `permissions:` block default to
    `none`, which is what we want (CI needs no `issues`/write scope, unlike
    `upstream-watch.yml` which writes a tracking issue). Matches the minimal style
    of `upstream-watch.yml`. (Note: `issues: none` *is* valid syntax — the reason
    to omit it rather than set it is just that omitted scopes are `none` anyway.)

5. **Deliberately omit** `cargo fmt --check` and any clippy-as-error gate (none
   currently exist; don't introduce one unasked). The comment added in step 1
   guards against a future editor re-adding it.

6. **Update docs.**
   - `TODO.md`: **leave the "Build and test in CI" subsection (lines 185–194) and
     the "Next Up" bullet (lines 19–20) in place until this work lands.** Both stay
     on the board as "Next Up" until the PR is merged. The *final* step of
     implementation (step 7) deletes them — per repo rule, entries are deleted on
     landing, not ticked. Link each to this plan: add a one-line pointer at the top
     of the subsection, e.g. "Plan: `plans/ci-workflow.md`." and annotate the Next
     Up bullet similarly. Do NOT delete them now. (Also do NOT touch the adjacent
     "test_read_response_from_db disagrees with the code" block at lines 214–222 —
     that is Plan A's work item.)
    - `CHANGELOG.md`: under **Maintenance**, add: "Added `.github/workflows/ci.yml`
      running `cargo build`, `cargo test`, and `cargo test -- --include-ignored`
      (e2e skipped via `--skip e2e`); added opt-in `.github/workflows/e2e.yml`
      gated on `GEMINI_API_KEY`. No `cargo fmt --check` (tree is not rustfmt-clean)."
   - `AGENTS.md`: the "Commands" section already lists the three tiers; add a one
     -line note that CI enforces tiers 1–2 and e2e is opt-in (GEMINI_API_KEY),
     so the "no CI" gap is reflected as closed. Keep the warning about not adding
     `cargo fmt --check`.

7. **Commit and open a PR** (only if asked). Stage `.github/workflows/ci.yml`,
   `.github/workflows/e2e.yml`, `CHANGELOG.md`, `TODO.md`, `AGENTS.md`.

## Verification

- **Enable Actions on the fork first** (AGENTS.md:178: GitHub disables them on new
  forks). Check `kgrizz-git/agy-acp` Settings → Actions → General. A merged
  workflow that never fires would recreate the rot this plan fixes.
- Push the branch / open a PR with `GEMINI_API_KEY` **unset** in the repo → the
  `test` job goes green, the `e2e` job is **Skipped** (not failed, not a parse
  error).
- Confirm the `test` job actually runs the ignored tier: the
  `test_read_response_from_db` fix (companion plan) should turn it from red to
  green in CI — that is the concrete proof CI catches what rotted.
- If a fork maintainer later adds `GEMINI_API_KEY` as a repo secret, the `e2e`
  job runs; verify it passes against a pinned `agy` version.

## Risks / things the reviewer should challenge

- **e2e install fragility — handled by splitting it out.** `agy` install in CI is
  fragile and contrary to `TODO.md:191`'s "skip, not fail" intent. Decision: e2e is
  NOT in `ci.yml`; it lives in opt-in `e2e.yml` (`workflow_dispatch` +
  `pull_request: [main]`), gated via a **gate job** emitting
  `has_key` (because `secrets` is not a valid context in a job-level `if:`). The
  reviewer's alternative ("omit e2e from CI entirely") is partially taken: it is
  omitted from the always-on gate but kept runnable on demand.
- **`--skip e2e` correctness — VERIFIED.** Grep of `src/tests.rs` finds exactly
  four e2e tests, all `test_e2e_`-prefixed:
  `test_e2e_agy_acp_full_round_trip`, `test_e2e_multi_turn`,
  `test_e2e_session_load`, `test_e2e_error_paths`. No e2e test lacks the prefix,
  so `--skip e2e` excludes all four. Substring match is documented as an
  assumption in the workflow comment.
- **Job-level `secrets` in `if:` — FIXED (was a hard bug).** A job-level
  `if: ${{ secrets.GEMINI_API_KEY != '' }}` fails parsing with "Unrecognized
  named-value: 'secrets'". Replaced with a `gate` job whose step computes
  `has_key` and the `e2e` job gates on `needs.gate.outputs.has_key == 'true'`, so
  the job shows as **Skipped**, never a parse error.
- **Concurrency does NOT dedupe push vs PR.** A branch push ref
  (`refs/heads/feat/x`) and its PR ref (`refs/pull/N/merge`) are different
  concurrency groups, so `cancel-in-progress` cannot collapse them. Fixed by
  dropping `push: feat/**` — `pull_request: [main]` already covers topic branches
  in this solo fork, so there is no double-run to dedupe. `push: [main]` remains
  for direct main pushes.
- **Enable Actions on the fork.** AGENTS.md:178 warns GitHub disables Actions on
  new forks. A merged workflow that never fires recreates the exact rot this plan
  exists to fix — so add a verification step: confirm Actions are enabled on
  `kgrizz-git/agy-acp` (Settings → Actions → General) before declaring the work
  done. The "open a PR" verification below will reveal it too.
- **`issues: none` is actually valid syntax.** Earlier drafts called it invalid;
  it is not. The conclusion (use `contents: read`, omit other scopes) still holds
  because omitted scopes default to `none`.
- **Toolchain drift.** No `rust-version` in `Cargo.toml`, so `dtolnay/rust-toolchain@stable`
  accepts `stable`. Fine.
- **Double-run cost.** Running `--include-ignored` re-runs unit tests too; that's
  fine and intended (it's the full tier-2 set) and is noted in a workflow comment.
