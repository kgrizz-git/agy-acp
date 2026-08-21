# agy-acp

Single Rust crate. ACP (Agent Client Protocol) stdio adapter for Google Antigravity CLI (`agy`). Bridges `agy` into OpenAB's JSON-RPC protocol.

## Commands

```bash
cargo build                    # debug build
cargo build --release          # release build (required for e2e tests)
cargo test                     # unit tests only (fast, no I/O)
cargo test -- --include-ignored  # all tests including filesystem I/O tests
cargo test e2e -- --ignored --nocapture  # e2e only (needs agy binary + auth)
```

No separate lint/typecheck/format commands — just `cargo build` and `cargo test`.

> This is a **hard fork** of `hicder/agy-acp`: no upstream remote, no pull requests
> filed there. Fork-specific context and workflow are in the second half of this
> file, from "What this fork is" onward.

## Architecture

- `main.rs` — stdin/stdout JSON-RPC loop. Reads lines, dispatches to adapter methods, writes responses.
- `adapter.rs` — core logic: session lifecycle, spawning `agy` subprocess, state persistence. `Adapter::new()` reads `HOME` for state/conv dirs.
- `db.rs` — reads agy's SQLite conversation DBs (read-only). Table: `steps` with columns `idx`, `step_type`, `step_payload`.
- `protobuf.rs` — hand-rolled protobuf varint/field extraction (no prost/protobuf dependency). Extracts text from `step_payload` field 20 → sub-field 1.
- `streaming.rs` — polls SQLite every 500ms during `session/prompt`, emits incremental `session/update` notifications to stdout.
- `types.rs` — JSON-RPC types, `SessionStore` for persistence, `StreamingState`.
- `permission.rs` — `--permission-prompts` only. Unix socket server turning agy's `PreToolUse` hook into ACP `session/request_permission`, plus the `agy-acp permission-hook` subcommand agy invokes.
- `hook_root.rs` — `--permission-prompts` only. Writes that hook into a private temp dir handed to agy as an extra `--add-dir`.

## Key paths

| Path | Purpose |
|---|---|
| `~/.openab/agy-acp/sessions.json` | Persisted session→conversation mapping (with `.lock` file for mutual exclusion) |
| `~/.gemini/antigravity-cli/conversations/*.db` | agy's SQLite conversation databases |

## Test tiers

1. **Unit tests** (`cargo test`) — protobuf parsing, narration filtering, JSON-RPC response shape. No filesystem or network I/O.
2. **Ignored I/O tests** (`-- --include-ignored`) — session persist/restore, SQLite read, conversation snapshot. Create temp dirs in `$TMPDIR`.
3. **E2E tests** (`e2e -- --ignored`) — spawn the release binary, send JSON-RPC over stdin, verify responses. Requires:
   - `agy` in `PATH` (install from `google-antigravity/antigravity-cli` releases)
   - Auth via `GEMINI_API_KEY` env var or macOS Keychain (`~/.gemini/antigravity-cli/settings.json`)
   - `cargo build --release` must have been run first

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

- `rusqlite` uses `bundled` feature — no system SQLite dependency needed.
- SQLite reads use `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX` — single-threaded access assumed per conversation DB.
- State persistence uses write-to-tmp-then-rename pattern under an exclusive file lock (`fs2`).
- Streaming writes JSON-RPC notifications directly to stdout from a background polling thread (not through the main channel). Both the main loop and the poller write to stdout concurrently.
- `handle_session_load` returns a `Vec<String>` (multiple notifications + final response), not a single response like other methods.
- Conversation binding: on first prompt for a new session, the adapter snapshots conversation DB filenames, then diffs after `agy` exits to discover the new conversation ID. Refuses to bind if multiple new DBs appear simultaneously.
- `fetch_available_models()` runs `agy models` synchronously during `Adapter::new()`. If `agy` isn't installed, models list is empty (no error).
- `agy models` prints `id<TAB>Human Label` on stdout and its "Fetching available models..." banner on stderr. Only the id is a valid `--model` argument; ACP gets the id as `modelId`/`value` and the label as `name`. Ids arriving from a client are checked against that list, and a `id<TAB>label` string left in an old `sessions.json` is repaired on restore.
- `session/cancel` is a no-op — always returns `{}`.
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

### To do

Compare these projects with this fork before porting anything. Identify ideas we
can adapt, and assess whether either is a better fit for Paseo or the broader
ACP use case. Do not assume an implementation is better without a concrete
feature, maintenance, and security comparison.

#### Investigate first

- [ ] **Permission-denial race:** verify whether a late successful provider row
  can overwrite or visually contradict an ACP rejection. If it can, retain the
  bridge's deny decision as authoritative and suppress the contradictory update.
  This is the most relevant idea from `paseo-agy-acp`.
- [ ] **Completion gating:** confirm that a turn is not completed after progress,
  idle, or tool lifecycle rows alone; require final visible assistant output after
  the last tool boundary. Add regression fixtures before changing the poller.
- [ ] **Missing streaming tool types:** assess upstream PR
  [#15](https://github.com/hicder/agy-acp/pull/15) and add fixtures for observed
  Antigravity step types before expanding narration/tool classification.

#### Consider after validation

- [ ] **Robust conversation binding:** evaluate PID-based database discovery,
  with the existing before/after database snapshot as a fallback. Upstream PR
  [#20](https://github.com/hicder/agy-acp/pull/20) has an implementation, but is
  conflicting and unreviewed; independently validate macOS behavior first.
- [ ] **More ACP configuration:** selectively expose supported `agy` options
  (mode, model, reasoning effort, and sandbox) with validation and session
  persistence. Keep `--dangerously-skip-permissions` under the bridge's own
  fail-closed permission policy rather than exposing it as an ordinary bypass
  mode.
- [ ] **Per-session workspace roots:** assess ACP `cwd` and
  `additionalDirectories` support from upstream PR
  [#18](https://github.com/hicder/agy-acp/pull/18), including its interaction
  with the private hook directory and workspace-bound read policy.
- [ ] **Provider robustness:** test newest-first protobuf field-20 extraction,
  clear surfacing of `agy` backend errors, and configurable `agy` binary paths.
  These are parts of upstream PR #20, not yet a reviewed upstream baseline.
- [ ] **PTY fallback:** reproduce the non-TTY and thinking-model failures that
  motivated `agy-acp-bridge`; only add a PTY path if current `agy` versions still
  need it and it preserves multi-session streaming and permissions.

#### Paseo-only candidates

- [ ] **Native agy subagent visibility:** evaluate switching the adapter's agy
  invocation to `--output-format stream-json` in a feature spike. agy 1.1.8+
  documents a `subagent_info` event with the child `conversation_id` and
  `log_uri`; verify it is emitted in `--print` mode, remains ordered with
  `step_update`/`result`, and carries enough lifecycle information to expose
  child progress safely. Today the adapter polls only the root SQLite
  conversation and cannot identify a child as a distinct agent.
- [ ] **Paseo child-agent representation:** determine whether Paseo has a
  supported provider-extension or external-child API that accepts a stable
  child ID, lifecycle updates, logs, and cancellation. If it does, map agy's
  `conversation_id` to that API and preserve the parent/child relationship. If
  it does not, present native children as ACP progress/activity updates only;
  do not create synthetic independent Paseo agents or use `paseo import`, which
  imports sessions only for Paseo-owned providers and would give incorrect
  lifecycle/cancellation semantics. Keep this opt-in and version-gated (agy
  1.1.8+) until an end-to-end fixture proves it.
- [ ] **Daemon context bridge:** investigate Paseo's appended system context only
  if Paseo proves it is unavailable to `agy`. Treat it as trusted host data and
  make it opt-in, observable, and isolated from general ACP hosts.
- [ ] **Paseo task/revert edge cases:** reproduce the foreground task-state and
  trailing-newline whole-file-revert issues reported by `paseo-agy-acp` before
  adopting their fixes.

#### Do not adopt as-is

- [ ] Do not replace this adapter with `agy-acp-bridge`'s single-session,
  non-streaming, unconditional permission-bypass design.
- [ ] Do not treat `paseo-agy-acp`'s direct permission-bypass mode or its
  Paseo-specific prompt injection as general ACP behavior.

## Local code-risk to do

These are source-audit leads for `mine`, not confirmed regressions. Reproduce
and add a regression test before changing behavior.

### Security and permission-boundary leads

- [ ] **Relative-path auto-allow escape:** `outside_workspace()` only evaluates
  strings beginning with `/`, while `reads` and `searches` may accept a relative
  path such as `../../some-readable-file`. Confirm how each `agy` read/search
  tool resolves relative paths; if it resolves them from the workspace, normalize
  relative path arguments against that root before auto-allowing, and prompt on
  any escape. Do not rely on the sensitive-name denylist for containment.
- [ ] **“Always allow” bypasses safety checks:** remembered choices are keyed
  only by `(session, tool name)` and are evaluated after the hook-root check but
  without workspace or sensitive-path checks. Verify whether a benign "Always
  allow view_file" can later read an external or credential-looking path. If so,
  retain a per-tool preference but keep path containment and sensitive-path
  checks non-bypassable.
- [ ] **Workspace-supplied hooks:** the adapter passes the user workspace as an
  `--add-dir`, and `agy` discovers `.agents/hooks.json` in every workspace root.
  Determine whether opening an untrusted repository can execute its hook commands
  outside the ACP permission bridge. If yes, document the trust boundary and
  consider an opt-in allowlist/isolated hook discovery strategy.
- [ ] **Permission socket hardening:** the Unix-socket pathname is predictable
  from the adapter PID and hook connections/tasks have no explicit peer or
  concurrency limit. Measure socket permissions and test same-user spoofing or
  connection exhaustion; use a private `0700` directory, an unguessable path,
  framing limits, and bounded connection handling if the threat is realistic.
- [ ] **Hook-root temporary-directory race:** the private hook root is also a
  predictable `$TMPDIR/agy-acp-hooks-<pid>` path, created with `create_dir_all`
  and made read-only only after writing `hooks.json`. Replace it with an exclusive
  random `0700` temporary directory; never recursively delete a merely
  prefix-matching stale directory without proving it was created by this adapter.

### Protocol and lifecycle leads

- [ ] **One stdout owner:** the streaming poller writes JSON-RPC directly to
  stdout while the main loop and final-drain path also write there. Large writes
  can interleave and corrupt line-delimited JSON-RPC. Route every notification
  through the main output channel and add a concurrent-streaming framing test.
- [ ] **Cancellation map race:** a second `session/prompt` for the same session
  overwrites the first cancellation token before the global adapter lock admits
  it; the first task can then remove the second token. Define whether concurrent
  prompts are rejected, queued, or supersede one another, and test cancellation
  in each state.
- [ ] **Global prompt serialization:** `handle_session_prompt()` holds the one
  adapter mutex through the entire child process, so every session is serialized
  and state operations queue behind a long-running prompt. Decide whether this
  is intentional; if not, split short state mutations from per-session runtime
  state without weakening permission routing.
- [ ] **Conversation binding collision:** discovery by diffing all conversation
  DB filenames refuses to bind if any other `agy` process creates a DB at the
  same time, but can also bind to the wrong sole new DB and replay its private
  conversation into this ACP session. Reproduce alongside an interactive `agy`
  run; then assess PID-based binding with a snapshot fallback.
- [ ] **Unbounded input/output work:** stdin JSON-RPC lines, hook payloads,
  pending permission requests, and SQLite rows are not size- or count-bounded.
  Establish host limits and add practical frame, queue, and database-poll
  safeguards to prevent a malformed client or provider data from exhausting
  memory.
- [ ] **Defensive protobuf bounds checks:** hand-rolled field walkers convert
  provider-controlled varint lengths to `usize` and use `i + len` before slicing.
  Replace all index arithmetic with checked operations and add maximal-varint /
  malformed-length regression cases so a corrupted conversation DB cannot panic
  the adapter.
- [ ] **Streaming work grows with turn size:** each 500 ms poll queries and
  reparses all DB rows after the pre-prompt index, even though `last_step_idx`
  advances. Benchmark a long tool-heavy turn, then incrementally query new rows
  and separately track the one growing agent-message row.

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
- `.git/hooks/pre-push` denies by default: any push whose URL is not
  `kgrizz-git/agy-acp` is refused. Reinstall it with
  `cp .githooks/pre-push .git/hooks/pre-push && chmod +x .git/hooks/pre-push`.

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
commit it). It records what has been reviewed, not what exists: upstream's
stream-json rewrite deletes `db.rs` and `protobuf.rs`, which the permission
bridge, conversation binding and model handling here are all built on. Adopting
that is a port, not a merge, and a watermark that advanced by itself would
quietly claim otherwise.

GitHub disables Actions on new forks; if the workflow never runs, enable them in
the repository settings.

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
  format command, so it reflows files a change does not touch — `src/protobuf.rs`
  especially. Format specific files, or restore afterwards with
  `git checkout HEAD -- src/protobuf.rs`.
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

