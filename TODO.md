# To do

Every active piece of work has an entry here. An entry is deleted when the work
lands — not ticked off — and the change is recorded in [CHANGELOG.md](CHANGELOG.md):
under a release heading if it changed behaviour anyone can see, under
**Maintenance** if it did not. `AGENTS.md` describes how the code works today and
carries no work items.

## Next Up

The few things worth picking up next. Each is a pointer; the detail lives below.

- [Split the two files and the one function that have outgrown reading](#split-the-two-files-and-the-one-function-that-have-outgrown-reading)
  — path containment and the turn phases are split out; `tests.rs` is the remainder.
- [Verify the port under Paseo](#verify-the-port-under-paseo) — done except the
  reopened-thread path.
- [Reconcile the tool lists with agy's real toolset](#reconcile-the-tool-lists-with-agys-real-toolset)
  — five names are for tools agy lacks; seven of its tools are unclassified.
- [SonarCloud analyses nothing today](#sonarcloud-analyses-nothing-today-and-only-ci-based-analysis-can-change-that)
  — overlaps with the cargo clippy/llvm-cov gates now in CI; running both gives
  two sources of truth for the same findings.
- [Rename the binary and crate](#rename-the-binary-and-crate) — cheaper now than
  after anyone else installs it.
- [Configure the protected e2e environment](#configure-the-protected-e2e-environment)
  — key-backed PR e2e is intentionally deferred; deterministic CI already runs.

## Active

### Verify the port under Paseo

Mostly done on 2026-08-30, against the binary built from `11e2b48` and installed
to `~/.local/bin/agy-acp`. Driven through Paseo 0.6.1 as a real `agy` agent
(`gemini-3.7-flash-low`) in a scratch workspace.

Confirmed end to end, by checking the filesystem rather than trusting the
transcript: the permission bridge, `run_command` (`rm` deleted its file),
`write_to_file` and `replace_file_content` (`draft.txt` was created reading
`first draft` and then edited to `second draft`), `view_file`, and remembered
"Always allow". Conversation *continuation* within one session works — the child
was invoked with `--conversation <id>` across turns — which is not the same as the
reopened-thread path below. The live process tree showed
`agy --add-dir <ws> --add-dir <hook root> --output-format stream-json
--conversation <id> --dangerously-skip-permissions`, which is the port running
for real rather than inferred from the binary's contents.

Two things came out of it: the cancellation defect recorded below, and a live
reproduction of the "Always allow" gap — approving `echo verification-one`
auto-approved a later `rm -f other.txt` with no prompt, and the file was deleted.

Session restore across an adapter restart also works. Killing the `agy-acp`
process out from under a live agent made Paseo respawn it, and the agent then
answered two questions from the earlier transcript — the first command's output
and a file's edited contents — with no tool calls. So the session was restored and
agy's conversation continued.

That is narrower than it sounds, and the remaining gap is worth stating exactly.
It proves continuity via `--conversation`; it does *not* separately prove the
adapter's own replay path (`db.rs`/`protobuf.rs` parsing agy's conversation DB to
re-send history to the host), because Paseo keeps its own timeline and would show
the prior messages either way. Nor does it establish which method Paseo sends —
the adapter advertises both `loadSession` and `sessionCapabilities.resume`, and
Paseo's ACP client knows both `session/load` and `session/resume`. To close it
properly, tee the adapter's stdio and read which method arrives, then confirm the
replayed updates match the DB. Concurrent sessions are also still untested.

One cosmetic Paseo quirk, noted so it is not mistaken for an adapter bug: Paseo's
normalized permission payload puts the tool-call *title* into the structured
field, so a shell request arrives as `detail.command: "Run \`echo hi\`"` and an
edit as `detail.filePath: "write_to_file /path/to/x"`, rather than the bare
command or path. Our `rawInput` is passed through untouched.

### Configure the protected e2e environment

`e2e.yml` deliberately reads its key only from the approval-gated GitHub
environment named `e2e`; the environment has not been created yet, so the e2e
job currently skips after its secret gate. Fork pull requests skip before
requesting environment approval because they cannot receive Actions secrets.
This does not weaken the deterministic CI jobs or expose a repository secret to
pull-request code.

When it becomes useful to run paid e2e on pull requests:

1. Create the `e2e` GitHub environment and require reviewer approval before a
   job can use it.
2. Add `E2E_GEMINI_API_KEY` as an environment secret. Do not use a
   repository-level e2e key; the job checks out pull-request code.
3. If the existing repository-level `GEMINI_API_KEY` is only for this workflow,
   remove it after the environment secret works.
4. Re-run e2e on a same-repository PR or use `workflow_dispatch`, and confirm
   the gate proceeds, the pinned agy archive verifies, and all four e2e tests
   run.

### Security and permission boundaries

#### An "Always" answer cannot be revoked within a session

Once given, an "Always allow" or "Always reject" holds until the session leaves
the adapter's in-memory map. There is no way for the user to take it back, see
what has been remembered, or expire it at the end of the turn that granted it.

Two things bound it today, both indirect. The answer is keyed by
`(session, tool, Option<argument fingerprint>)`, so wherever the fingerprint is
present -- a command, a URL, or any tool this fork does not recognise -- the
answer covers that exact call rather than the tool. And eviction forgets: `evict_if_needed`
tells the bridge to drop the victim's answers, so nothing outlives the 64-session
map, and a session restored from `sessions.json` afterwards prompts again.

Neither is revocation. Worth considering: expose the remembered set over ACP, or
expire answers with the turn that granted them.

Note the consequence that remains: a session reloaded in the same process, while
still resident in the map, inherits the "Always" answers it was given earlier.
That is within what the README promises, but it is worth deciding rather than
inheriting.

#### Deliberately not taken: parsing what a command does

Recorded so they are not re-proposed as fixes for a gap that is closed. Keying a
remembered answer by the exact command settled the granularity problem; neither
of these was needed for it, and both remain available as *depth* if the threat
model ever changes.

**Extract paths from a command line** so containment applies to command tools at
all — tokenize the string and treat `/`-, `~`- and `../`-bearing tokens as paths.
Real parsing, and a harder job than keying: it has to find every path the command
touches, through pipes, `$VAR`, `$(...)`, subcommands and attached flag values,
and a path it fails to extract is a path that gets allowed. It can never be
complete, because the shell re-interprets the string afterwards. Best-effort
only, and it would have to fail toward prompting.

**Refuse destructive commands outright** (`rm -rf`, `dd`, `mkfs`, `curl | sh`).
Cheapest and weakest: a denylist over a string the shell will re-interpret is
evaded by `cat .en"v"` or `cat $HOME/.env`. Worth doing as depth, never as the
boundary.

#### Reconcile the tool lists with agy's real toolset

Reference: [dev-docs/agy-tool-surface.md](dev-docs/agy-tool-surface.md), which
records what agy 1.1.22 actually sends and how it was captured.

Capturing it closed the path-field question — `SearchDirectory` was missing and is
now fixed — but turned up a mismatch in both directions that is still open.

Five names in `permission.rs` match no tool agy was observed to emit and none it
self-reports: `view_code_item`, `codebase_search`, `edit_file`, `propose_code`,
`command_status`. They sit in `READ_TOOLS`, `SEARCH_TOOLS` and `tool_kind`, make
the auto-allow groups look broader than they are, and cost real time — they sent
one investigation chasing tools that were never produced. Delete them, or comment
them as deliberate forward-compatibility.

Seven tools in agy's self-reported list are unclassified here: `manage_task`,
`send_message`, `schedule`, `invoke_subagent`, `define_subagent`,
`manage_subagents`, `generate_image`. They fall to `"other"` and always prompt,
which is the right default but is reached by omission rather than decision.
`schedule` and `invoke_subagent` most deserve a deliberate call, since one defers
work past the current turn and the other spawns another agent.

#### Generated artifacts land outside the workspace

`generate_image` takes no destination argument and writes to
`~/.gemini/antigravity-cli/brain/<conversation-id>/`. This is normal Antigravity
behaviour — Gemini writes to its own internal workspace unless told otherwise —
so the work here is not to treat it as a defect but to decide what this adapter
says and does about it.

Two consequences. The bridge cannot constrain a destination that is not in the
arguments, so the README's "only inside the workspace" limit does not describe
this tool and should say so. And the adapter passes the user's workspace with
`--add-dir` without instructing agy to prefer it for artifacts; if generated files
should land in the workspace, that has to be stated explicitly somewhere agy
reads.

Worth checking which other tools share the behaviour before deciding — anything
that produces a file without taking a path is in the same position.

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

Related: the hook root is deleted by `HookRoot`'s `Drop`, and the signal handler
added with the cancellation fix ends the process with `std::process::exit`, which
runs no destructors. A signalled adapter therefore leaves its hook root behind,
and the bridge's socket with it. This is not a regression — an unhandled
`SIGTERM` skipped the same `Drop` — but handling the signal is what makes
cleaning up there possible at all.

### Reliability and lifecycle

#### Global prompt serialization

`handle_session_prompt()` holds the one adapter mutex through the entire child
process, so every session is serialized and state operations queue behind a
long-running prompt. Decide whether this is intentional; if not, split short
state mutations from per-session runtime state without weakening permission
routing.

Anything done here has to account for the permission bridge, which now depends on
this serialization rather than merely tolerating it. `set_active_session` drains
*every* session's pending requests at turn start, and that is only safe because
one turn runs at a time adapter-wide. `refused_during_prompt` is likewise one flag
for the adapter, not one per session. Allowing turns to run concurrently means
making both per-session first, and the session-scoped drain that follows reopens
a hole of its own — a request stranded by one session times out into whichever
turn is running nine minutes later. `permission.rs` says this at the line that
depends on it; it is repeated here because this is the entry that would break it.

#### A hook cannot say which turn it belongs to

Within one session, consecutive turns share an agy conversation id, and the hook
payload carries nothing else that identifies a turn. So a hook task delayed across
a turn boundary *of the same session* is indistinguishable from one belonging to
the turn now running: it passes the active-session check and adopts the running
turn's generation. Its denial can mark that turn refused, and an "always" can
stick for it.

Everything else in this area is closed — a request for a different session, or
arriving when no turn is running, is denied without asking. This case is the
residue, and it cannot be closed with information the adapter currently has.

The fix is to stamp turn identity into the hook environment when agy is spawned
(agy spawns the hook as a child, so an env var set on agy's `Command` is
inherited) and have the hook echo it in its payload. That is a hook protocol
change, it depends on agy propagating the environment rather than sanitising it,
and it needs the version skew handled — an old hook binary sends no stamp. Worth
doing when this area is next opened, not on its own.

Reachability is narrow: agy from the previous turn must be gone (it exits, or the
cancel path kills its tree) while the accepted socket task lingers into the next
turn of the same session.

#### Unbounded input/output work

stdin JSON-RPC lines, hook payloads, pending permission requests, and stream-
json lines are not size- or count-bounded. Remembered permission answers now
retain a copy of each approved argument object as its key fingerprint
(`args_fingerprint`), so an unbounded hook payload becomes an unbounded map key —
the same exposure, widened. The bound belongs on the payload, and fixing it there
fixes both; do not bound the key separately. Establish host limits and add
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

#### Rename the binary and crate

It is a hard fork with different behaviour (permission bridge, load replay,
model id handling), and sharing `agy-acp` with upstream makes bug reports and
installs ambiguous. Renaming touches `Cargo.toml`, the `agy-acp permission-hook`
subcommand the hook shells out to, the Paseo provider command in
`~/.paseo/config.json`, and the README. Existing state lives in `~/.openab/agy-
acp/`, so decide whether to migrate it or leave it in place.

#### SonarCloud analyses nothing today, and only CI-based analysis can change that

The account has Automatic Analysis switched on and the project has never been
analyzed. That is not a misconfiguration to hunt for — it cannot work here, for
two independent reasons in Sonar's own documentation:

- Automatic Analysis "is available for all of SonarQube Cloud's supported
  languages except for Objective-C, Dart, and Rust".
- Eligibility separately requires at least 20% of the project to be in a
  supported language. GitHub measures this repo as 96% Rust, with Python and
  Shell together under 3%.

Rust *is* analyzed by SonarQube Cloud, but only through CI-based analysis. What
that needs, from the Rust language page:

- The SonarScanner CLI on the runner, plus `cargo` and `clippy`
  (`rustup component add clippy`) — the analyzer shells out to Clippy itself
  rather than importing a report, and `sonar.rust.clippy.enabled=false` turns
  that off.
- `SONAR_TOKEN` as a repository secret, and a project key and organization.
  `sonar.rust.cargo.manifestPaths` is only needed when the manifest is not at the
  root, which here it is.
- Coverage import accepts LCOV and Cobertura. The exact property name was not
  confirmed from the docs — check it before writing the workflow rather than
  guessing at `sonar.rust.lcov.reportPaths`.
- Automatic Analysis has to be turned off first. With CI-based analysis
  configured as well, Sonar fails the build.

One caveat found while checking: the Rust language page reads as though Clippy
runs under automatic analysis when a root `Cargo.toml` is present, which
contradicts the automatic-analysis page's explicit exclusion of Rust. The
evidence here favours the exclusion — automatic analysis is enabled and has
produced nothing.

The decision, before any of the above: Sonar would run Clippy and import coverage,
which is most of what the quality-gates entry above wants from `cargo clippy` and
`cargo llvm-cov` directly. Running both means two sources of truth for the same
findings and a second place to silence a lint. Worth picking one deliberately
rather than adding Sonar because the account is already there.

#### Split the two files and the one function that have outgrown reading

Plan: plans/split-large-files.md. Done. The turn lifecycle has tests driven by
stub binaries, `permission.rs` gave up its path containment to
`permission/path_rules.rs`, `handle_session_prompt` is spawn/drain/teardown phases
with the complexity lints denied module-wide, and the flat `tests.rs` is gone --
tests now sit in their own files beside the module they exercise.

Sizes after the split, from `wc -l` and a scan of function lengths. The first two
numbers in the original entry were taken before the inline test modules existed
and undercounted by ~1500 lines:

| Unit | Before | After |
|---|---|---|
| `src/tests.rs` | 2879 | gone -- split by subject |
| `src/permission.rs` | 3724 | 1175 plus five test files |
| `adapter.rs::handle_session_prompt` | 317 | 61, over four phases |
| `permission.rs::decide` | 141 | 141, unchanged and deliberately so |

`handle_session_prompt` is the one that actually hurts. It spawns agy, wires two
reader tasks, runs the `select!` that races the child against cancellation, kills
the tree, drains both readers, binds the conversation id, tears down the bridge,
persists the session and builds the response — and it holds the single adapter
mutex across all of it, which is its own entry above. Every recent bug in the
turn lifecycle has been somewhere in this function, and each fix has had to be
argued against the whole of it. Splitting the spawn/drain/teardown phases apart
would let them be tested without a real agy, which is the gap review keeps
finding: the call sites in there are covered by nothing.

`decide` is long but linear — a policy cascade, read top to bottom. Lower value.

The two big files are cohesive, so a split is only worth it with a real seam.
`permission.rs` has an obvious one: the containment and path logic
(`outside_workspace`, `is_inside`, `resolve`, `lexical_normalize`, `PATH_FIELDS`)
is self-contained and heavily tested, and would move out whole. `tests.rs` is
large because it is one flat module per subject; splitting it by subject is
mechanical and would make the permission tests findable, which they currently are
not.

Do not do this while a behavioural change is in flight -- a move that touches
every line makes the next real diff unreviewable.

#### Replay without agy's private schema

replay works, but only by parsing agy's undocumented conversation DB — the
dependency upstream just walked away from. The adapter already sees every update
it emits during a turn; persisting those and replaying its own transcript would
drop `db.rs`/`protobuf.rs` entirely. The gap is history the adapter never
streamed (older threads, other clients), so any switch needs a fallback or a
migration.

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
