# To do

Every active piece of work has an entry here. An entry is deleted when the work
lands — not ticked off — and the change is recorded in [CHANGELOG.md](CHANGELOG.md):
under a release heading if it changed behaviour anyone can see, under
**Maintenance** if it did not. `AGENTS.md` describes how the code works today and
carries no work items.

## Next Up

The few things worth picking up next. Each is a pointer; the detail lives below.

- [Cancelling a turn leaves the command running](#cancelling-a-turn-leaves-the-command-running)
  — verified under Paseo: `child.kill()` orphans the shell it spawned.
- [Verify the port under Paseo](#verify-the-port-under-paseo) — done except the
  reopened-thread path.
- [Permission decisions ignore what a command actually does](#permission-decisions-ignore-what-a-command-actually-does)
  — one "Always allow" on `run_command` covers every later command.
- [Path fields: what agy actually sends](#path-fields-what-agy-actually-sends)
  — one field was missing; five names in our lists are for tools agy does not have.
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

#### Permission decisions ignore what a command actually does

Plan: plans/permission-command-keying.md

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

Reproduced live under Paseo on 2026-08-30, which is worth more than the test that
asserts it: **Always allow** was given to `echo verification-one`, and a later
`rm -f other.txt` then ran with no prompt at all and deleted the file. Nothing
adversarial was involved — it was an ordinary follow-up request in the same
session, which is the point.

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

#### Path fields: what agy actually sends

`PATH_FIELDS` in `permission.rs` decides which arguments are judged as paths
whatever their value looks like; a field it does not know falls back to the shape
tests, so a miss costs coverage quietly.

Checking this against real agy 1.1.22 found a hole, now fixed: `find_by_name`
names its directory `SearchDirectory`, which was missing. The captured value was
absolute so the shape tests caught it, but a relative one would have been judged
by nothing.

**agy's real toolset is not the one this code assumes.** Asked to enumerate, agy
1.1.22 lists seventeen tools:

```
view_file   run_command      manage_task    send_message   schedule
list_dir    write_to_file    invoke_subagent define_subagent manage_subagents
grep_search replace_file_content generate_image read_url_content search_web
find_by_name ask_question
```

Two mismatches follow, and both are worth acting on.

*Five names in `permission.rs` are dead.* `view_code_item`, `codebase_search`,
`edit_file`, `propose_code` and `command_status` appear in `READ_TOOLS`,
`SEARCH_TOOLS` or `tool_kind` but agy never emits them — they are upstream
vocabulary this fork inherited. Harmless, but they make the auto-allow groups look
broader than they are, and they sent this investigation chasing tools that do not
exist. Worth deleting, or commenting as forward-compatibility.

*Seven agy tools are unclassified here:* `manage_task`, `send_message`,
`schedule`, `invoke_subagent`, `define_subagent`, `manage_subagents`,
`generate_image`. They fall through `tool_kind` to `"other"` and are in no
auto-allow group, so they always prompt — fail-safe, which is the right default,
but it is by omission rather than decision. `schedule` and `invoke_subagent`
especially deserve a deliberate classification.

Argument keys observed, all covered:

```
run_command Cwd    view_file AbsolutePath      grep_search SearchPath
list_dir DirectoryPath   write_to_file TargetFile   find_by_name SearchDirectory
replace_file_content TargetFile
```

Correctly *not* treated as paths: `read_url_content` takes `Url`, `search_web`
takes `query` (lowercase, where `grep_search` uses `Query` — agy's casing is not
consistent), and `find_by_name`'s `FullPath` is a **boolean**, not a path. That
last one is why schema names are verified rather than trusted: it reads like a
path field and is not one.

One candidate remains unverified. agy reports `generate_image` as taking
`ImagePaths`, which would be a path field this list does not have, but no call has
produced it — it is presumably for input images. Add it when a payload shows it.
`FilePath` was added without a sighting, but on stronger evidence: `tools.rs:179`
and `protobuf.rs:405` already treat it as naming a location, so this fork
contradicted itself. A model's description of its own schema is not that.

#### generate_image writes outside the workspace, invisibly

Found while enumerating the toolset. `generate_image` takes `ImageName` and
`Prompt` and **no destination argument at all**; asked to "save it in this
workspace" it wrote to `~/.gemini/antigravity-cli/brain/<conversation-id>/` and
then told the user it had saved into the workspace, which was false.

The bridge is not bypassed — `generate_image` is in no auto-allow group so the
user is prompted — but the prompt cannot say where the file goes, and the
workspace-containment promise in the README does not hold for it, because there is
no path in the arguments to contain. Decide whether to say so in the README or to
deny the tool by default. The same question applies to any future tool whose
destination is implicit.

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

#### Cancelling a turn leaves the command running

Found by the Paseo verification on 2026-08-30. `session/cancel` kills the `agy`
child with `child.kill()`, which
signals that one process and nothing beneath it. agy has already spawned a shell
for the tool call, so the shell and the command it is running survive, get
reparented to PID 1, and run to completion.

Observed directly. With `sleep 45 && echo finished-sleeping` running under a
cancelled turn:

```
after cancel:
  agy child procs:  0                                   <- killed
  adapter procs:    1                                   <- still serving, correct
  94091  ppid=1     zsh -c sleep 45 && echo ...         <- orphaned, still running
  94092  ppid=94091 sleep 45                            <- still running
```

`sleep` is harmless. A long build, a `curl`, a `rm -rf`, or anything with side
effects is not: the user is told the turn was cancelled while the work continues,
which is worse than not offering cancellation at all, because it is silent.

The fix is to put `agy` in its own process group at spawn and signal the group
rather than the pid — `process_group(0)` on the `Command`, then `killpg` — with
a fallback to the current single-process kill where that is unavailable. Note
Paseo hit this exact class of bug in its own Claude provider and solved it with a
`terminateWithTreeKill` helper, commented "the SDK's internal cleanup may only
kill the direct child process", so the shape of the fix is not controversial.

Adapter shutdown is worse, not merely the same. `child.kill()` at
`adapter.rs:954` is the *only* kill in the codebase — there is no signal handler,
no `Drop` impl and no `kill_on_drop`, so when the adapter exits it does not kill
agy at all and the whole tree is orphaned silently. The fix is therefore "every
kill path is a group kill, and there needs to be one on exit", not "add the group
flag to the existing shutdown kill" — a reader who goes looking for shutdown kill
code will not find any.

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
