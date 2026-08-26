# To do

Every active piece of work has an entry here. An entry is deleted when the work
lands — not ticked off — and the change is recorded in [CHANGELOG.md](CHANGELOG.md):
under a release heading if it changed behaviour anyone can see, under
**Maintenance** if it did not. `AGENTS.md` describes how the code works today and
carries no work items.

## Next Up

The few things worth picking up next. Each is a pointer; the detail lives below.

- [Verify the port under Paseo](#verify-the-port-under-paseo) — merged and
  installed nowhere yet; only a scripted client has exercised it.
- [Permission decisions ignore what a command actually does](#permission-decisions-ignore-what-a-command-actually-does)
  — one "Always allow" on `run_command` covers every later command.
- [Confirm the path-field list against real agy traffic](#confirm-the-path-field-list-against-real-agy-traffic)
  — a path field this fork has not seen is judged only by how its value looks.
- [Build and test in CI](#build-and-test-in-ci) — there is none, which is why an
  ignored test rotted for months. Plan:
  [plans/ci-workflow.md](plans/ci-workflow.md).
- [Rename the binary and crate](#rename-the-binary-and-crate) — cheaper now than
  after anyone else installs it.

## Active

### Verify the port under Paseo

The stream-json port merged as `bf6e81b`. Everything verified so far was driven by
a scripted ACP client: the four permission scenarios, load replay across
processes, and the test suite. Nothing has run under Paseo itself, and
cancellation (upstream's `child.kill()` path), concurrent sessions and subagent
events are untested anywhere. Install the built binary to `~/.local/bin` with
`codesign -f -s -`, restart the daemon, then drive one real agent through a
permission prompt, a reopened thread, and a cancellation.

### Security and permission boundaries

#### Permission decisions ignore what a command actually does

Remembered answers are keyed by `(session, tool name)`, so one "Always allow" on
`run_command` approves every later command in that session. The containment and
sensitive-path checks do still run on a remembered allow, but they read arguments
as paths and a command line is a single opaque string: `cat /etc/shadow` is not
recognised as naming `/etc/shadow` at all. `cat /etc/passwd` is caught only
because `passwd` is a sensitive substring — luck, not containment. Both halves are
pinned by tests that assert today's behaviour deliberately
(`always_allow_is_remembered_per_tool_not_per_command` and
`a_path_inside_a_command_string_is_invisible_to_the_containment_check`), so
closing the gap turns them red and forces the README to be updated with it.

Documented in the README under "What 'Always' remembers", with a warning to
prefer **Allow** over **Always allow** for `run_command`. Three ways out, roughly
in order of value:

1. Make the key as specific as the prompt. Either do not offer "Always allow"
   for command-executing tools at all, or key the sticky answer by
   `(session, tool, command string)` so approving `cat README.md` does not
   approve `cat` on anything else. Note this is *not* the parsing problem in (2):
   it needs only an equivalence test, and the minimum version is exact string
   equality — no tokenization, no shell semantics. Normalization beyond
   whitespace is an optional ergonomic layer and each step of it merges commands
   that are not identical, which is how the current bug arose (keying on the tool
   name is the degenerate case of normalizing everything away). Under-normalizing
   costs a prompt; over-normalizing is a hole. The general rule: the sticky key
   should be as specific as the checks that still apply to a remembered allow.
   Tool-level keying is defensible for path-argument tools like `view_file`,
   where containment and the sensitive-path list do constrain it; for command
   tools those checks are inert, so the key must carry the command.
2. Extract paths from a command line so containment applies to `run_command` at
   all — tokenize the string and treat `/`-, `~`- and `../`-bearing tokens as
   paths. This one really is parsing, and a harder job than (1): it has to find
   every path the command touches, through pipes, `$VAR`, `$(...)`, subcommands
   and attached flag values, and a path it fails to extract is a path that gets
   allowed. It can never be complete, because the shell re-interprets the string
   afterwards. Best-effort only, and it must fail toward prompting.
3. Refuse destructive commands outright (`rm -rf`, `dd`, `mkfs`, `curl | sh`) and
   widen the sensitive-path list. Cheapest, and the weakest: a denylist over a
   string the shell will re-interpret is evaded by `cat .en"v"` or
   `cat $HOME/.env`. Worth doing as depth, never as the boundary.

Two smaller things about the same map, worth fixing alongside whichever option
is taken.

An "Always" answer cannot be revoked within a session — consider exposing the
remembered set, or expiring answers with the turn.

And nothing ever removes an entry: `BridgeState.always` and
`BridgeState.conversations` both accumulate for the life of the process, and only
`pending` is ever cleaned up. There is no session-end hook to hang a cleanup on —
ACP sends no close, and this adapter handles no session-end method, so a session
simply stops being used and "its last turn" is not knowable. Each entry is two
short strings, and the count is bounded by the sessions one adapter process
serves, so this is untidiness rather than a leak that will bite.

The cheap fix, if it is worth doing at all, is to hang it off the in-memory
session map instead: `evict_if_needed` already drops the least recently used
`Session`, so have it tell the bridge to forget that session's answers too. That
bounds the bridge by the same 64 and gives "this session" a defensible meaning —
answers last as long as the session is live in memory. A session restored from
`sessions.json` afterwards would prompt again, which is the safe direction.

Note the same absence has a visible consequence today: a session reloaded in the
same process inherits the "Always" answers it was given earlier. That is within
what the README promises, but it is worth deciding rather than inheriting.

#### Confirm the path-field list against real agy traffic

`PATH_FIELDS` in `permission.rs` decides which arguments are judged as paths
whatever their value looks like. It was assembled from the field names this
repository already handles — `AbsolutePath`, `TargetFile`, `DirectoryPath`,
`SearchPath`, `Cwd`, `Paths` — not from agy's schema, which is not published.
A field it does not know keeps the shape-based tests and nothing else, so a
miss costs coverage quietly and never announces itself.

Capture the `PreToolUse` payloads from a real session and diff the argument keys
against the list. Same trip as the Paseo verification above, since both need one
real agy run.

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

The output channel is the concrete case. Every notification now goes through one
unbounded `mpsc` to the single stdout writer, so if a host reads its side of the
pipe more slowly than agy emits events, the writer blocks in `writeln!` and the
queue grows with no ceiling. A bounded channel is the obvious answer and is not
a free swap: a full queue would stop the drain task reading agy's stdout, which
is the backpressure we want, but it puts a blocked host in the same call graph
as agy's own progress, so the deadlock risk has to be reasoned through before
the bound goes in. Measure a realistic burst first — these are small strings and
the ceiling may not be worth the coupling.

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

#### Build and test in CI

Plan: [plans/ci-workflow.md](plans/ci-workflow.md).

There is no build or test workflow — only `upstream-watch.yml`. That absence is
why `test_read_response_from_db` rotted unnoticed: it is `#[ignore]`d, and nobody
runs `--include-ignored` by hand. A workflow should run `cargo build`,
`cargo test`, and `cargo test -- --include-ignored` with the four e2e tests
skipped, since those need `agy` and auth. Gate e2e behind a `GEMINI_API_KEY`
secret so they skip rather than fail when it is absent. Do not add a bare
`cargo fmt --check`: this tree is not rustfmt-clean and it would reflow files no
change touched.

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

Plan: [plans/fix-test-read-response-from-db.md](plans/fix-test-read-response-from-db.md).

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
