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
- `adapter.rs` — core logic: session lifecycle, spawning `agy` subprocess, state persistence. `Adapter::new()` reads `HOME` for the state dir.
- `streaming.rs` — parses `agy --output-format stream-json` NDJSON (`init`, `step_update`, `result`) into ACP `session/update` notifications via `StreamProcessor`, which runs in a background task reading the `agy` subprocess's stdout as it streams.
- `tools.rs` — maps agy tool names/parameters/output into ACP tool-call fields (`kind`, locations, content).
- `types.rs` — JSON-RPC types, `SessionStore` for persistence.
- `permission.rs` — `--permission-prompts` only. Unix socket server turning agy's `PreToolUse` hook into ACP `session/request_permission`, plus the `agy-acp permission-hook` subcommand agy invokes.
- `hook_root.rs` — `--permission-prompts` only. Writes that hook into a private temp dir handed to agy as an extra `--add-dir`.

## Key paths

| Path | Purpose |
|---|---|
| `~/.openab/agy-acp/sessions.json` | Persisted session→conversation mapping (with `.lock` file for mutual exclusion) |

## Test tiers

1. **Unit tests** (`cargo test`) — stream-json parsing, narration filtering, JSON-RPC response shape. No filesystem or network I/O.
2. **Ignored I/O tests** (`-- --include-ignored`) — session persist/restore. Create temp dirs in `$TMPDIR`.
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

- State persistence uses write-to-tmp-then-rename pattern under an exclusive file lock (`fs2`).
- Streaming writes JSON-RPC notifications directly to stdout from the `agy` stdout reader (not through the main channel). The main loop may still write concurrently if other requests arrive during a prompt.
- `handle_session_load` returns a `Vec<String>`: the replayed history as `session/update` notifications, then the response. Replay reads agy's SQLite conversation DB, which is the only place past turns exist — streaming never touches SQLite.
- Conversation binding: the `init` / `result` stream-json events include `conversation_id`, which is persisted and passed back as `--conversation` on subsequent prompts.
- `fetch_available_models()` runs `agy models` synchronously during `Adapter::new()`. If `agy` isn't installed, models list is empty (no error).
- `agy models` prints `id<TAB>Human Label` on stdout and its "Fetching available models..." banner on stderr. Only the id is a valid `--model` argument; ACP gets the id as `modelId`/`value` and the label as `name`. Ids arriving from a client are checked against that list, and a `id<TAB>label` string left in an old `sessions.json` is repaired on restore.
- `session/cancel` returns `{}` immediately but sets an `AtomicBool` flag that the prompt task polls; when set, it kills the in-flight `agy` subprocess and the turn ends with `stopReason: "cancelled"`.
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
- [x] **Completion gating:** a stream reaching EOF without its terminal `result`
  event is reported as a failed turn, as is a failure after partial output. The
  latter used to be swallowed: the error response was gated on no updates having
  been emitted, so a turn that streamed one chunk and then failed returned
  `end_turn`.
- [ ] **Replay without agy's private schema:** replay works, but only by parsing
  agy's undocumented conversation DB — the dependency upstream just walked away
  from. The adapter already sees every update it emits during a turn; persisting
  those and replaying its own transcript would drop `db.rs`/`protobuf.rs`
  entirely. The gap is history the adapter never streamed (older threads, other
  clients), so any switch needs a fallback or a migration.

#### Consider after validation

- [ ] **More ACP configuration:** selectively expose supported `agy` options
  (mode, model, reasoning effort, and sandbox) with validation and session
  persistence. Keep `--dangerously-skip-permissions` under the bridge's own
  fail-closed permission policy rather than exposing it as an ordinary bypass
  mode.
- [ ] **Per-session workspace roots:** assess ACP `cwd` and
  `additionalDirectories` support from upstream PR
  [#18](https://github.com/hicder/agy-acp/pull/18), including its interaction
  with the private hook directory and workspace-bound read policy.
- [ ] **Provider robustness:** confirm `agy` backend errors reach the user —
  the stream's `result` event carries `status` and `error`, and the adapter now
  reads both — and consider a configurable `agy` binary path.
- [ ] **PTY fallback:** reproduce the non-TTY and thinking-model failures that
  motivated `agy-acp-bridge`; only add a PTY path if current `agy` versions still
  need it and it preserves multi-session streaming and permissions.

#### Paseo-only candidates

- [ ] **Native agy subagent visibility:** the stream-json switch this called for
  has landed — upstream did it, and `StreamProcessor` already turns
  `subagent_info` into a tool call. What is left is the part that spike was
  really about: verify the child `conversation_id` and `log_uri` carry enough
  lifecycle information to expose child progress as more than one opaque tool
  call, and that subagent events stay ordered with `step_update`/`result`.
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

- [ ] **`test_read_response_from_db` disagrees with the code.** The only red test
  in the suite, and it was red before the stream-json port too, so it is not a
  regression from it. `read_delta_from_db` advances `max_step_idx` over every row
  it read, including the trailing user-message row, and returns 2; the test
  expects 1, the last row it takes text from. As a cursor, 2 looks right and the
  assertion looks stale, but the helper is `#[cfg(test)]`-only now, so nothing in
  production depends on either answer. Decide which semantics were meant, then fix
  the test or the code — do not just change the number to match.

### Protocol and lifecycle leads

- [ ] **One stdout owner:** the stream-reader task writes JSON-RPC directly to
  stdout while the main loop also writes there. Large writes can interleave and
  corrupt line-delimited JSON-RPC. Route every notification through the main
  output channel and add a concurrent-streaming framing test. The stream-json
  port removed the final-drain writer but not the shared-stdout problem.
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
- [ ] **Unbounded input/output work:** stdin JSON-RPC lines, hook payloads,
  pending permission requests, and stream-json lines are not size- or
  count-bounded. Establish host limits and add practical frame and queue
  safeguards to prevent a malformed client or provider data from exhausting
  memory.

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

