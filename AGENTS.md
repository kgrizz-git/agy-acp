# agy-acp

Single Rust crate. ACP (Agent Client Protocol) stdio adapter for Google Antigravity CLI (`agy`). Bridges `agy` into OpenAB's JSON-RPC protocol.

## Commands

```bash
cargo build                    # debug build
cargo build --release          # release build (required for e2e tests)
cargo clippy --all-targets -- -D warnings  # lint (same bar as CI)
cargo test                     # unit tests (fast; some use a scratch dir in $TMPDIR)
cargo test -- --include-ignored  # adds session persist/restore and DB tests
cargo test e2e -- --ignored --nocapture  # e2e only (needs agy binary + auth)
```

No separate typecheck/format commands — `cargo build` and `cargo clippy` cover
those. The tree is not rustfmt-clean; do not run bare `cargo fmt`.

Work items live in [TODO.md](TODO.md), not here — this file describes how the
code works today. Completed work is recorded in [CHANGELOG.md](CHANGELOG.md).

### Plans and TODO discipline

TODO.md entries are the source of truth for what's next; a *plan* is not
completion. When a piece of work gets a plan (kept under `plans/`):

- Keep the TODO.md entry and its "Next Up" pointer **on the board until the work
  actually lands** (PR merged). Do not delete them while planning or implementing.
- **Link** the plan from the TODO entry (a one-line `Plan: plans/<name>.md`
  pointer) so the entry and the plan cross-reference.
- **Delete** the entry as the **final step of implementation** — never during
  planning. Per TODO.md's own rule, entries are deleted on landing, not ticked.
- This applies symmetrically: if an entry is removed before its work ships, the
  work becomes untracked. Premature deletion is the bug to avoid.

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
`TODO.md` as `Plan: plans/<name>.md`. On landing, move the plan to
`plans/completed/<name>.md` and delete the TODO entry.

**CHANGELOG** entries are bullets only — one short clause per observable change.
Categories under each version, in order: **Added**, **Changed**, **Fixed**,
**Removed**, **Maintenance**. Omit empty categories. Citations go at the end of
the bullet in parentheses (`(PR #9)`). No "Known issues" section — open problems
belong in `TODO.md`. An `## Unreleased` section exists only while a release is
being cut.

**Semver is deferred.** `Cargo.toml` stays at `0.1.0` with no tags and no
release workflow until there is an external reason to cut a release (public
announcement, `cargo install` use case, downstream version requirement). Three
options when that happens: stay on `## Unreleased`, adopt date-stamped headings,
or adopt semver with tags and a release workflow. The first release under semver
would be `0.2.0`.

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

CI (`ci.yml`) enforces `cargo build`, unit tests, the ignored I/O tier
(`cargo test -- --ignored --skip e2e`), `cargo clippy --all-targets -- -D warnings`,
and a `cargo llvm-cov` coverage report (artifact + job summary; no threshold).
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
- Permission answers marked "Always" are keyed by `(session, tool name, Option<args fingerprint>)`. The fingerprint is the default: `sticky_scope()` returns `None` — tool-level keying — only for a tool whose kind is `read`, `edit` or `search` *and* whose arguments do *not* trip `has_unconstrained_reach` — no `CommandLine`, no `Url`, no `://` in any string value, at any depth. Kind alone is not sufficient evidence, because kind is a display classification: `read_url_content` is kind `read` but its `Url` is not a path field, so containment and the sensitive-path list are as inert against it as against a command line. Anything with unconstrained reach, and any tool whose kind is not on the list, is keyed by the arguments. The fingerprint is the argument object serialized minus `UNKEYED_FIELDS` (`toolAction`, `toolSummary`, `WaitMsBeforeAsync`, all presentational); comparison is exact, because under-matching costs a prompt and over-matching is a hole. Rejects narrow the same way. The prompt labels name whichever scope applies, via `AlwaysScope`: the tool, "this exact command" where a `CommandLine` is present, or "this exact call" otherwise — `read_url_content` and any unknown tool land on the last, since calling their arguments a command would be false. `AlwaysScope` is derived once in `decide` from the same `sticky_scope` result the key is built from, and passed to both the prompt and `apply_outcome`, so the button, the key and the reason string cannot disagree. `evict_if_needed` queues the evicted session id on `Adapter.pending_forget` (a `std::sync::Mutex`, not the adapter mutex, which `session/prompt` holds for a whole turn) and the `main.rs` dispatcher drains it into `PermissionBridge::forget_session`; re-admitting the id first cancels the forget.
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
- `.githooks/pre-push` denies by default: any push whose URL is not
  `kgrizz-git/agy-acp` is refused. After cloning, run
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
- **Do not run bare `cargo fmt`.** The repo is not kept rustfmt-clean and has no
  format command, so it reflows files a change does not touch. Format specific
  files, or restore the rest afterwards with `git checkout HEAD -- <path>`.
- **Pre-push hook.** After cloning, run `git config core.hooksPath .githooks`
  once. The hook runs `cargo clippy --all-targets -- -D warnings` and the unit
  tier (`cargo test`). Set `SKIP_LOCAL_GATES=1` to bypass those for one push;
  the fork-guard URL check always runs.
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
