# agy-acp

Single Rust crate. ACP (Agent Client Protocol) stdio adapter for Google Antigravity CLI (`agy`). Bridges `agy` into OpenAB's JSON-RPC protocol.

## Commands

```bash
cargo build                    # debug build
cargo build --release          # release build (required for e2e tests)
cargo fmt                      # format (CI runs `cargo fmt --check`)
cargo clippy --all-targets -- -W clippy::all -D clippy::all  # lint (same bar as CI)
cargo test                     # unit tests (fast; some use a scratch dir in $TMPDIR)
cargo test -- --include-ignored  # adds session persist/restore and DB tests
cargo test e2e -- --ignored --nocapture  # e2e only (needs agy binary + auth)
```

No separate typecheck command — `cargo build` and `cargo clippy` cover it. The
tree is rustfmt-clean and CI enforces it, so run `cargo fmt` before pushing.

### Branch protection

Never bypass branch protection, including a direct push that GitHub reports as
bypassing pull-request, required-check, review, merge-queue, or similar rules,
unless the user has explicitly approved bypassing that exact protection after
its risk has been explained. A request to change, commit, or push code does not
authorize the bypass; create or update a pull request instead.

Work items live in [TODO.md](TODO.md), not here — this file describes how the
code works today. Completed work is recorded in [CHANGELOG.md](CHANGELOG.md).

### Plans and TODO discipline

TODO.md entries are the source of truth for what's next; a *plan* is not
completion. When a piece of work gets a plan (kept under `plans/`):

- Keep the TODO.md entry and its "Next Up" pointer **on the board while the work
  is being planned and implemented**. Do not delete them early.
- **Link** the plan from the TODO entry (a one-line `Plan: plans/<name>.md`
  pointer) so the entry and the plan cross-reference.
- **Delete** the entry with the plan move and CHANGELOG entry in the same pull
  request — not after the merge. A cleanup that happens after landing has no
  owner: the branch is gone, the reviewer has moved on, and what is left is a
  TODO entry for work that shipped and a plan sitting in `plans/` claiming to be
  in flight. Putting it in the PR lets a reviewer see the claim that the work is
  done in the same diff as the work.
- This applies symmetrically: if an entry is removed before its work ships, the
  work becomes untracked. Premature deletion is still the bug to avoid.
- Preserve reviewable history: make follow-up fixes as ordinary commits. Do not
  squash, amend, rebase, or force-push a pull-request branch unless the user has
  explicitly asked for that rewrite. A close-out commit need not remain the
  branch tip if later review work is required.

**Code does not cite plans.** A comment in `src/` never points at
`plans/<name>.md`. Plans move (`plans/` → `plans/completed/`), so the path rots;
worse, a plan is a snapshot of intent that the work is free to contradict, and
completed plans are frozen as historical record — so a reader who follows the
pointer can land on an argument for something that never happened. If a comment
needs a reason, the comment carries the reason. `git blame` reaches the commit
and the PR, which is where the discussion actually lives, and neither of those
paths can go stale. Citing an immutable id (a PR number, a commit hash) is fine
when the history genuinely matters; citing `dev-docs/` is fine too, since it is
maintained as current reference rather than as a record of a decision.

### Plans and CHANGELOG discipline

Plans live in three buckets:

- `plans/` — in-flight work being planned or implemented. The matching `TODO.md`
  entry still exists.
- `plans/completed/` — work that landed on `main`. Historical record; edit only
  for typo fixes that change no meaning. If a completed plan turns out wrong,
  write a new plan instead of rewriting the old one.
- `plans/deferred/` — work explicitly parked. Each file carries a one-line
  "Why deferred" section.

Filenames keep their topic (`permission-command-keying.md`), not a status
prefix; the directory carries the status. In-flight plans are linked from
`TODO.md` as `Plan: plans/<name>.md`. Move the plan to
`plans/completed/<name>.md` in the same pull request as the TODO deletion and
CHANGELOG line. A plan still sitting in `plans/` after its work merged reads as
in-flight to everyone who comes next; later review commits do not require
rewriting the close-out commit.

**CHANGELOG** entries are bullets only — one short clause per observable change.
Categories under each version, in order: **Added**, **Changed**, **Fixed**,
**Removed**, **Maintenance**. Omit empty categories. Citations go at the end of
the bullet in parentheses (`(PR #9)`). No "Known issues" section — open problems
belong in `TODO.md`. `## Unreleased` collects entries for the next version and
is renamed to that semver heading when the release is cut.

### Documentation gardening

`TODO.md` is a concise work board: every entry names an outcome, its next
concrete step, and a plan or reference link. Do not keep experiment transcripts,
threat-model alternatives, implementation detail, or completed-work narratives
there. Put active implementation design and acceptance criteria in `plans/`;
move explicitly parked approaches to `plans/deferred/`; keep durable observed
behavior and research in `dev-docs/`; and record shipped effects only in the
CHANGELOG. Preserve useful evidence by moving it, not deleting it.

Every in-flight file directly under `plans/` must have a current `TODO.md` entry
linking to it. Reference documents track no work: when their findings imply a
next action, add that action to `TODO.md` and create or link its plan. Before
moving a plan to `plans/completed/` or `plans/deferred/`, update or remove the
corresponding TODO link in the same pull request.

Documentation-only or internal-maintenance work does not trigger a semver bump,
but it gets one short `## Unreleased` **Maintenance** entry when it materially
changes how the project is maintained. A user-visible behavior, feature, or
security change follows the versioning rule below: bump `Cargo.toml` in that PR
and move its CHANGELOG entries beneath the new semver heading. Never use the
CHANGELOG to describe a plan or a rejected alternative.

**Versioning.** Keep the current `0.1.0` history as-is; do not retroactively
version already-merged work. Starting with the next meaningful user-visible
feature, behaviour, or security change, bump `Cargo.toml` in the same pull
request and move its CHANGELOG entries from `## Unreleased` under that version
heading. Use patch releases for compatible bug fixes and minor releases for
new or materially changed behaviour; decide any breaking-change version before
implementation. Documentation-only and internal-maintenance changes do not by
themselves require a bump. Tag and publish an artifact when there is a public
release channel, but versioned local builds are useful before then: `--version`
must identify the software that was installed. The next versioned delivery will
be `0.2.0`.

> This is a **hard fork** of `hicder/agy-acp`: no upstream remote, no pull requests
> filed there. Fork-specific context and workflow are in the second half of this
> file, from "What this fork is" onward.

## Architecture

- `main.rs` — stdin/stdout JSON-RPC loop. Reads lines, dispatches to adapter methods, writes responses.
- `adapter.rs` — core logic: session lifecycle, spawning `agy` subprocess, state persistence. `Adapter::new()` reads `HOME` for the state dir.
- `streaming.rs` — parses `agy --output-format stream-json` NDJSON (`init`, `step_update`, `result`) into ACP `session/update` notifications via `StreamProcessor`, which runs in a background task reading the `agy` subprocess's stdout as it streams.
- `tools.rs` — maps agy tool names/parameters/output into ACP tool-call fields (`kind`, locations, content).
- `types.rs` — JSON-RPC types, `SessionStore` for persistence.
- `permission.rs` — `--permission-prompts` only. Unix socket server turning agy's `PreToolUse` hook into ACP `session/request_permission`, plus the `agy-acp permission-hook` subcommand agy invokes.
- `hook_root.rs` — `--permission-prompts` only. Writes that hook into a private temp dir handed to agy as an extra `--add-dir`.

## Key paths

| Path | Purpose |
|---|---|
| `~/.openab/agy-acp/sessions.json` | Persisted session→conversation mapping (with `.lock` file for mutual exclusion). Capped at 256 entries, rewritten whole on every turn |
| `~/.gemini/antigravity-cli/brain/<conversation-id>/` | Where agy writes generated artifacts. Not the workspace, and not visible to the bridge — `generate_image` takes no destination argument |
| `src/proc.rs` | Killing agy's process tree. agy puts each command it runs in its own process group, so a cancel stops agy, walks a process-table snapshot for descendants and kills those, rather than signalling a group; shutdown kills the same trees through `LiveChildren` |
| `scripts/probe-cancel.py` | Manual check that a cancel stops the command agy is running. Needs `agy` and auth, so it is not in CI; it is the probe that caught the first attempt at this fix aiming at the wrong mechanism, kept so the check is repeatable |
| `dev-docs/agy-tool-surface.md` | What agy actually sends the permission bridge: its tools, their argument keys, which are paths, and how that was captured. Reference for `PATH_FIELDS` and the auto-allow groups |

## Test tiers

1. **Unit tests** (`cargo test`) — stream-json parsing, narration filtering,
   JSON-RPC response shape, permission decisions. No network and no reads of the
   real `$HOME`; several do create a scratch directory under `$TMPDIR`.
2. **Ignored I/O tests** (`-- --include-ignored`) — session persist/restore and
   conversation-DB reads. They are `#[ignore]`d by inheritance, not because they
   touch anything tier 1 does not; the split is worth revisiting when CI lands.
3. **E2E tests** (`e2e -- --ignored`) — spawn the release binary, send JSON-RPC over stdin, verify responses. Requires:
   - `agy` in `PATH` (install from `google-antigravity/antigravity-cli` releases)
   - Auth via `GEMINI_API_KEY` env var or macOS Keychain (`~/.gemini/antigravity-cli/settings.json`)
   - `cargo build --release` must have been run first
   - In CI, `E2E_MODEL_ROSTER` (comma-separated `gemini-*-flash-low` slugs) and
     `E2E_MODEL_OFFSET` (`github.run_number`) rotate model-issuing tests via
     `session/set_model`; with at least two roster entries, those tests use
     different models in a run and a failed turn advances to the next entry
     before each retry. This mitigates the observed daily per-model quota and
     transient per-model capacity failures (probes 2026-09-08: 1 request per
     no-tool turn, base-model metering, no wider ceiling observed at ~30 project
     requests — see plans/completed/e2e-quota-rotation.md). Unset locally,
     tests fall through to the `settings.json` default. `error_paths` does not
     call the model. A failed turn retries twice, after 30s then 60s, failing
     over to the next roster model first: the first backoff absorbs short 503
     capacity spikes and the second exceeds the observed ~37s per-minute-429
     retryDelay; daily 429s fail again fast, with a hint pointing at the agy log.
   - Local runs: `scripts/e2e-local.sh [filter] [args...]` sources the token
     from `.env.e2e.local` (gitignored) and runs everything under a throwaway
     `HOME`, so the real `~/.gemini` state (OAuth login, settings, session
     history) is never touched; `RUSTUP_HOME`/`CARGO_HOME` are pinned before
     the switch so the toolchain keeps resolving. `E2E_HOME` reuses a sandbox
     across runs. Caveat: the macOS keychain is global, so agy may still see
     an OAuth token there. `agy models` needs `settings.json` present to list
     under a bare key (it prints nothing without it) — the script and the CI
     configure step both write it first for that reason.

CI (`ci.yml`) enforces `cargo build`, unit tests, the ignored I/O tier
(`cargo test -- --ignored --skip e2e`), clippy (`-W clippy::all -D clippy::all`;
`handle_session_prompt` has `#[warn]` on complexity lints until refactor), and a
`cargo llvm-cov` coverage report (artifact + job summary; no threshold).
Rust 1.70 is the tested MSRV on Linux and Windows. The Unix-socket `--permission-prompts` bridge is intentionally
unavailable on Windows and fails closed there. E2e (`e2e.yml`) runs only after
approval of the protected `e2e` GitHub environment for same-repository PRs;
fork PRs skip before requesting approval. The environment holds
`E2E_GEMINI_API_KEY`, and the workflow uses a pinned agy release. Do not use a
repository-level e2e key: the workflow checks out PR code.

## Environment variables

| Var | Effect |
|---|---|
| `AGY_EXTRA_ARGS` | Space-separated extra args passed to every `agy` invocation |
| `GEMINI_API_KEY` | API key for e2e tests and CI |
| `AGY_ACP_AUTO_ALLOW` | What may run without asking. Tool names plus the groups `reads`, `searches`, `none`. Default `ask_question` |
| `AGY_ACP_SENSITIVE_PATTERNS` | Extra comma-separated substrings marking a path as too sensitive to read without asking |
| `AGY_ACP_PERMISSION_TIMEOUT_SECS` | How long a permission request waits before denying. Default `540` |
| `AGY_ACP_PERMISSION_SOCKET` | Set by the adapter on the `agy` subprocess; tells the hook where to reach the bridge. Not for users |

## Quirks

- State persistence uses write-to-tmp-then-rename pattern under an exclusive file lock (`fs2`).
- stdout has exactly one writer: the main loop in `main.rs`, draining `out_rx`. The stream reader and the permission bridge both publish through that channel rather than touching the fd, because two writers can interleave mid-line and corrupt line-delimited JSON-RPC. Anything new that emits to the client must go the same way.
- `handle_session_load` returns a `Vec<String>`: the replayed history as `session/update` notifications, then the response. Replay reads agy's SQLite conversation DB, which is the only place past turns exist — streaming never touches SQLite.
- Conversation binding: the `init` / `result` stream-json events include `conversation_id`, which is persisted and passed back as `--conversation` on subsequent prompts.
- `fetch_available_models()` runs `agy models` synchronously during `Adapter::new()`. If `agy` isn't installed, models list is empty (no error).
- `agy models` prints `id<TAB>Human Label` on stdout and its "Fetching available models..." banner on stderr. Only the id is a valid `--model` argument; ACP gets the id as `modelId`/`value` and the label as `name`. Ids arriving from a client are checked against that list, and a `id<TAB>label` string left in an old `sessions.json` is repaired on restore.
- `session/cancel` returns `{}` immediately but sets an `AtomicBool` flag that the prompt task polls; when set, it kills the in-flight `agy` subprocess *and every process agy started* (Unix only — see `src/proc.rs`) — agy shells out to run a tool call, so killing the pid alone left the command orphaned and running to completion — and the turn ends with `stopReason: "cancelled"`. A cancel — and ordinary turn teardown, and the start of the next turn — answers any permission request the turn left outstanding, so its timeout cannot fire during a later turn and mark that one a refusal. `cancel.rs` holds one token per in-flight turn rather than one per session — a host may send a second prompt before the first finishes, and a cancel stops every turn in that session.
- Permission answers marked "Always" are keyed by `(session, tool name, Option<args fingerprint>)`. The fingerprint is the default: `sticky_scope()` returns `None` — tool-level keying — only for a tool whose kind is `read`, `edit` or `search` *and* whose arguments do *not* trip `has_unconstrained_reach` — no `CommandLine`, no `Url`, no `://` in any string value, at any depth. Kind alone is not sufficient evidence, because kind is a display classification: `read_url_content` is kind `read` but its `Url` is not a path field, so containment and the sensitive-path list are as inert against it as against a command line. Anything with unconstrained reach, and any tool whose kind is not on the list, is keyed by the arguments. The fingerprint is the argument object serialized minus `UNKEYED_FIELDS` (`toolAction`, `toolSummary`, `WaitMsBeforeAsync`, all presentational); comparison is exact, because under-matching costs a prompt and over-matching is a hole.
  A fourth scope — **program keying** — widens `run_command` only when the command line classifies as a single invocation of an allowlisted read-only program *and* every other argument key is `Cwd` or presentational: the key becomes `safe:<program>` (see `src/permission/safe_command.rs`). A classified command with any other field present, or one the tokenizer cannot account for, falls back to the fingerprint exactly as today. **Allows widen, denies narrow:** a remembered allow may cover the program, but a remembered reject is always stored under the fingerprint and always wins over a later program allow for that string. The prompt labels name whichever scope applies, via `AlwaysScope`: the tool, the program ("Always allow `ls` commands this session"), "this exact command" where a `CommandLine` is present, or "this exact call" otherwise — `read_url_content` and any unknown tool land on the last, since calling their arguments a command would be false. `AlwaysScope` is derived once in `decide` from the same `sticky_scope` result the key is built from, and passed to both the prompt and `apply_outcome`, so the button, the key and the reason string cannot disagree. A remembered allow is only honoured when both containment checks pass on the current call — the existing `escapes_containment`, plus `classified_paths_escape`, which re-joins the command's extracted paths against its `Cwd`, so `ls /etc` still prompts after an `ls` approval. `evict_if_needed` queues the evicted session id on `Adapter.pending_forget` (a `std::sync::Mutex`, not the adapter mutex, which `session/prompt` holds for a whole turn) and the `main.rs` dispatcher drains it into `PermissionBridge::forget_session`; re-admitting the id first cancels the forget.
- Both `session/set_model` and `session/setConfigOption` are accepted for model selection.

### Permission bridge (`--permission-prompts`)

All of these were established experimentally against agy 1.1.12 and are easy to get wrong:

- A `PreToolUse` hook can only **veto** while agy's own permission checks are active. `{"decision":"allow"}` and `permissionOverrides` both lose to the headless soft-deny — verified with wildcard, literal and symlink-resolved paths. This is why the bridge runs agy with `--dangerously-skip-permissions` and becomes the sole gate, and why every unresolvable case must deny.
- A hook response with **no `decision` field** (`{}`) makes agy wait on the tool call until print mode times out. Always answer with an explicit decision.
- Three timeouts stack around a pending request and the order matters: the bridge's wait must expire before the hook's `timeout`, which must expire before agy's `--print-timeout`. Only the innermost yields a clean deny the model can continue from; if an outer one fires first, agy aborts the whole turn. Print mode defaults to 5m, so the adapter raises it when prompts are on.
- agy treats **every `--add-dir` as a workspace root**, so the hook directory is visible to the model, which will try to work in it after a refusal. Tool calls naming that directory are refused without prompting.
- Hooks are discovered in `.agents/hooks.json` under any workspace root, including secondary `--add-dir` ones. That is what keeps the hook out of the user's repo and global config.
- `{"decision":"ask"}` is a safe passthrough — it defers to agy's normal handling rather than forcing a prompt or a deny.

## What this fork is

A hard fork of `hicder/agy-acp` carrying the ACP permission-prompt bridge: agy runs
headless under the adapter and headless agy cannot prompt for tool permissions, so
tool calls silently failed. The bridge routes them to the ACP host instead. The
sections above describe how it works and the agy behaviours it is built around.

Used with Paseo, though nothing in the code is Paseo-specific —
`session/request_permission` is standard ACP and Zed implements it too. Keep it
host-neutral: that is what makes the adapter usable from more than one host, and it
costs nothing.

## Related community projects

- [javimosch/agy-acp-bridge](https://github.com/javimosch/agy-acp-bridge) — ACP stdio bridge for `agy`.
- [tiezbro/paseo-agy-acp](https://github.com/tiezbro/paseo-agy-acp) — Paseo-focused ACP adapter for `agy`.

Both were assessed; what is worth taking, and what is deliberately not, is in
[TODO.md](TODO.md).

## Branches

| Branch | Purpose |
|---|---|
| `main` | Default branch. Where all work lands. |
| `feat/*` | Topic branches, cut from `main` and merged back into it. |

Before the hard fork, `main` was a clean mirror of `hicder/agy-acp`, the real work
lived on `mine`, and feature branches had to be cut from `upstream/main` so they
could become pull requests. None of that applies now: `main` carries the fork's
own history and there is no upstream to cut against.

## Relationship to upstream

Hard fork as of August 2026. Concretely:

- No `upstream` remote. `origin` is `kgrizz-git/agy-acp` and is the only remote.
- `gh repo set-default kgrizz-git/agy-acp`, so `gh pr create` targets this repo
  rather than the parent — being a GitHub fork, it would otherwise default the PR
  base to `hicder/agy-acp` no matter what the git remotes say.
- `.githooks/pre-push` denies by default: only the canonical
  `git@github.com:kgrizz-git/agy-acp`, `https://github.com/kgrizz-git/agy-acp`, and
  `ssh://git@github.com/kgrizz-git/agy-acp` remote forms (with or without `.git`)
  are allowed. After cloning, run
  `git config core.hooksPath .githooks` once. Set `SKIP_LOCAL_GATES=1` to skip
  the clippy and unit-test checks for a single push; the fork guard always runs.

GitHub still records this repo as a fork; leaving the fork network is not
self-serve. The guards above are what actually prevent a stray pull request.

To read upstream work without re-establishing the link, fetch by URL instead of
adding a remote:

```bash
git fetch https://github.com/hicder/agy-acp main:refs/heads/hicder-snapshot
```

Cherry-pick what is worth having. Do not add the remote back.

### Watching upstream

`scripts/check-upstream.sh` reports commits on `hicder/agy-acp@main` that this
fork has not taken, comparing against the sha in `.upstream-watermark`. It exits
1 when there is something new, 0 when there is not.
`.github/workflows/upstream-watch.yml` runs it weekly and keeps a single
`upstream-watch` issue in sync with the result.

The watermark moves **only in a commit a human made** (`--update` writes it, you
commit it). It records what has been reviewed, not what exists. Upstream's
stream-json rewrite is the example: it deleted `db.rs` and `protobuf.rs`, which
the permission bridge, conversation binding and model handling were all built
on, so taking it was a port rather than a merge. A watermark that advanced by
itself would have claimed that was absorbed.

The report always goes to the run's job summary. Forks also have issues disabled
by default; the workflow detects that and skips the issue steps rather than
failing, so turning issues on in the repository settings is what upgrades it from
"summary only" to a tracked issue. Note the repository is public, so enabling
issues lets anyone file one.

GitHub disables Actions on new forks too; if the workflow never runs, enable them
in the repository settings.

## Local gotchas

- **Re-sign the binary after copying it.** macOS invalidates the signature on `cp`
  and SIGKILLs the result, with no useful error (exit 137):
  ```bash
  cp target/release/agy-acp ~/.local/bin/agy-acp
  codesign -f -s - ~/.local/bin/agy-acp
  ```
- **Do not name notes files `*.local.md`.** `~/.config/git/ignore` ignores that
  pattern globally, so such a file is silently never committed: `git status` stays
  clean and `git add -A` skips it, which reads exactly like success. These notes
  started as `AGENTS.local.md` and sat uncommitted for exactly that reason, then
  lived in `AGENTS.fork.md` until the hard fork folded them into this file.
- **Pre-push hook.** After cloning, run `git config core.hooksPath .githooks`
  once. The hook runs `cargo fmt --check`, the same clippy bar as CI
  (`-W clippy::all -D clippy::all`; complexity lints on `handle_session_prompt`
  are `#[warn]` only), and the unit tier (`cargo test`). Set
  `SKIP_LOCAL_GATES=1` to bypass those for one push; the fork-guard URL check
  always runs.
- **Blame skips the formatting commit.** Run
  `git config blame.ignoreRevsFile .git-blame-ignore-revs` once, so `git blame`
  reaches the commit that wrote a line rather than the one that rewrapped it.
  GitHub's blame view honours the file without any configuration.
- **Local coverage.** `cargo-llvm-cov` is not a dev-dependency. Install with
  `cargo install cargo-llvm-cov --locked` to reproduce the CI coverage report.
- Paseo runs the adapter as `["agy-acp", "--permission-prompts"]` in
  `~/.paseo/config.json`. Provider command changes need a daemon restart.
- The permission flag is off by default. Without it the adapter behaves as the
  original upstream code did.

## Testing the permission flow

Unit tests cover the bridge, but the interesting failures are end-to-end and need a
real ACP client driving real agy. A scripted client that answers
`session/request_permission` is the cheapest way to exercise it.

Things worth re-checking after any change, because each one was a real bug:

- **Reject**, not just approve — the approve path looked perfect while rejection was
  broken.
- **No answer at all** — should end as a clean deny, not a failed turn.
- **A read of `.env`** with `AGY_ACP_AUTO_ALLOW=reads` — must still prompt.

`AGY_ACP_PERMISSION_TIMEOUT_SECS` exists mainly so the timeout ordering can be
tested in seconds rather than nine minutes.
