# To do

Every active piece of work has an entry here. An entry is deleted when the work
lands — not ticked off — and the change is recorded in [CHANGELOG.md](CHANGELOG.md):
under a release heading if it changed behaviour anyone can see, under
**Maintenance** if it did not. `AGENTS.md` describes how the code works today and
carries no work items.

## Next Up

The few things worth picking up next. Each is a pointer; the detail lives below.

- [Land the stream-json port](#land-the-stream-json-port) — the open PR, and what
  is still unverified about it.
- [`sessions.json` grows without bound](#sessionsjson-grows-without-bound) —
  affects every turn today, and gets worse.
- [One stdout owner](#one-stdout-owner) — two writers on the ACP transport can
  corrupt a frame.
- [Rename the binary and crate](#rename-the-binary-and-crate) — cheaper now than
  after anyone else installs it.
- [Relative-path auto-allow escape](#relative-path-auto-allow-escape) — the
  permission boundary this fork exists to provide.

## Active

### Land the stream-json port

[PR #1](https://github.com/kgrizz-git/agy-acp/pull/1) takes upstream's stream-json
rewrite and ports the permission bridge onto it. Merging is a judgement call, not
a blocked task: the permission flows, session/load replay and the test suite are
verified, but nothing has run under Paseo itself — only a scripted ACP client.
Cancellation (upstream's new `child.kill()` path), concurrent sessions and
subagent events are untested. Remaining: merge, install to `~/.local/bin` with a
`codesign -f -s -` and a daemon restart, then exercise one real agent through a
permission prompt and a reopened thread.

### Security and permission boundaries

#### Relative-path auto-allow escape

`outside_workspace()` only evaluates strings beginning with `/`, while `reads`
and `searches` may accept a relative path such as `../../some-readable-file`.
Confirm how each `agy` read/search tool resolves relative paths; if it resolves
them from the workspace, normalize relative path arguments against that root
before auto-allowing, and prompt on any escape. Do not rely on the sensitive-
name denylist for containment.

#### Always allow bypasses safety checks

remembered choices are keyed only by `(session, tool name)` and are evaluated
after the hook-root check but without workspace or sensitive-path checks. Verify
whether a benign "Always allow view_file" can later read an external or
credential-looking path. If so, retain a per-tool preference but keep path
containment and sensitive-path checks non-bypassable.

#### Workspace-supplied hooks

the adapter passes the user workspace as an `--add-dir`, and `agy` discovers
`.agents/hooks.json` in every workspace root. Determine whether opening an
untrusted repository can execute its hook commands outside the ACP permission
bridge. If yes, document the trust boundary and consider an opt-in
allowlist/isolated hook discovery strategy.

#### Permission socket hardening

the Unix-socket pathname is predictable from the adapter PID and hook
connections/tasks have no explicit peer or concurrency limit. Measure socket
permissions and test same-user spoofing or connection exhaustion; use a private
`0700` directory, an unguessable path, framing limits, and bounded connection
handling if the threat is realistic.

#### Hook-root temporary-directory race

the private hook root is also a predictable `$TMPDIR/agy-acp-hooks-<pid>` path,
created with `create_dir_all` and made read-only only after writing
`hooks.json`. Replace it with an exclusive random `0700` temporary directory;
never recursively delete a merely prefix-matching stale directory without
proving it was created by this adapter.

### Reliability and lifecycle

#### One stdout owner

the stream-reader task writes JSON-RPC directly to stdout while the main loop
also writes there. Large writes can interleave and corrupt line-delimited JSON-
RPC. Route every notification through the main output channel and add a
concurrent-streaming framing test. The stream-json port removed the final-drain
writer but not the shared-stdout problem.

#### Cancellation map race

a second `session/prompt` for the same session overwrites the first cancellation
token before the global adapter lock admits it; the first task can then remove
the second token. Define whether concurrent prompts are rejected, queued, or
supersede one another, and test cancellation in each state.

#### Global prompt serialization

`handle_session_prompt()` holds the one adapter mutex through the entire child
process, so every session is serialized and state operations queue behind a
long-running prompt. Decide whether this is intentional; if not, split short
state mutations from per-session runtime state without weakening permission
routing.

#### Unbounded input/output work

stdin JSON-RPC lines, hook payloads, pending permission requests, and stream-
json lines are not size- or count-bounded. Establish host limits and add
practical frame and queue safeguards to prevent a malformed client or provider
data from exhausting memory.

#### Permission-denial race

verify whether a late successful provider row can overwrite or visually
contradict an ACP rejection. If it can, retain the bridge's deny decision as
authoritative and suppress the contradictory update. This is the most relevant
idea from `paseo-agy-acp`.

#### Provider robustness

confirm `agy` backend errors reach the user — the stream's `result` event
carries `status` and `error`, and the adapter now reads both — and consider a
configurable `agy` binary path.

### Fork maintenance

#### sessions.json grows without bound

`evict_if_needed` caps the in-memory map at 64, but nothing caps the file: it
holds 910 entries / 150 KB on one developer machine, 553 of them with no
`conversation_id` (sessions created and never prompted). Every `persist_session`
rewrites the whole file under the lock, so the cost of a turn grows with every
session ever created. Entries carry no timestamp, so pruning needs one added
first; decide between a cap, a TTL, and dropping unbound entries. Note also that
`evict_if_needed` removes an arbitrary `HashMap` key, not the least recently
used, so it can evict a live session while keeping a dead one.

#### Rename the binary and crate

It is a hard fork with different behaviour (permission bridge, load replay,
model id handling), and sharing `agy-acp` with upstream makes bug reports and
installs ambiguous. Renaming touches `Cargo.toml`, the `agy-acp permission-hook`
subcommand the hook shells out to, the Paseo provider command in
`~/.paseo/config.json`, and the README. Existing state lives in `~/.openab/agy-
acp/`, so decide whether to migrate it or leave it in place.

#### Replay without agy's private schema

replay works, but only by parsing agy's undocumented conversation DB — the
dependency upstream just walked away from. The adapter already sees every update
it emits during a turn; persisting those and replaying its own transcript would
drop `db.rs`/`protobuf.rs` entirely. The gap is history the adapter never
streamed (older threads, other clients), so any switch needs a fallback or a
migration.

#### test_read_response_from_db disagrees with the code

The only red test in the suite, and it was red before the stream-json port too,
so it is not a regression from it. `read_delta_from_db` advances `max_step_idx`
over every row it read, including the trailing user-message row, and returns 2;
the test expects 1, the last row it takes text from. As a cursor, 2 looks right
and the assertion looks stale, but the helper is `#[cfg(test)]`-only now, so
nothing in production depends on either answer. Decide which semantics were
meant, then fix the test or the code — do not just change the number to match.

### Upstream and ecosystem

#### Native agy subagent visibility

the stream-json switch this called for has landed — upstream did it, and
`StreamProcessor` already turns `subagent_info` into a tool call. What is left
is the part that spike was really about: verify the child `conversation_id` and
`log_uri` carry enough lifecycle information to expose child progress as more
than one opaque tool call, and that subagent events stay ordered with
`step_update`/`result`.

#### Paseo child-agent representation

determine whether Paseo has a supported provider-extension or external-child API
that accepts a stable child ID, lifecycle updates, logs, and cancellation. If it
does, map agy's `conversation_id` to that API and preserve the parent/child
relationship. If it does not, present native children as ACP progress/activity
updates only; do not create synthetic independent Paseo agents or use `paseo
import`, which imports sessions only for Paseo-owned providers and would give
incorrect lifecycle/cancellation semantics. Keep this opt-in and version-gated
(agy 1.1.8+) until an end-to-end fixture proves it.

#### More ACP configuration

selectively expose supported `agy` options (mode, model, reasoning effort, and
sandbox) with validation and session persistence. Keep `--dangerously-skip-
permissions` under the bridge's own fail-closed permission policy rather than
exposing it as an ordinary bypass mode.

#### Per-session workspace roots

assess ACP `cwd` and `additionalDirectories` support from upstream PR
[#18](https://github.com/hicder/agy-acp/pull/18), including its interaction with
the private hook directory and workspace-bound read policy.

#### Paseo task/revert edge cases

reproduce the foreground task-state and trailing-newline whole-file-revert
issues reported by `paseo-agy-acp` before adopting their fixes.

## Icebox

Deliberately deferred. Not a backlog to grind through — each of these needs a
reason to come back before it is worth doing.

#### PTY fallback

reproduce the non-TTY and thinking-model failures that motivated `agy-acp-
bridge`; only add a PTY path if current `agy` versions still need it and it
preserves multi-session streaming and permissions.

#### Daemon context bridge

investigate Paseo's appended system context only if Paseo proves it is
unavailable to `agy`. Treat it as trusted host data and make it opt-in,
observable, and isolated from general ACP hosts.

#### Do not adopt from the community forks

Two related projects were assessed and their approaches deliberately rejected:
`agy-acp-bridge`'s single-session, non-streaming design with an unconditional
permission bypass, and `paseo-agy-acp`'s direct permission-bypass mode and
Paseo-specific prompt injection. Neither is a general ACP behaviour. Individual
ideas from them may still be worth porting; these two designs are not.
