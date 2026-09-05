# To do

Every active piece of work has an entry here. An entry is deleted when the work
lands — not ticked off — and the change is recorded in [CHANGELOG.md](CHANGELOG.md):
under a release heading if it changed behaviour anyone can see, under
**Maintenance** if it did not. `AGENTS.md` describes how the code works today and
carries no work items.

## Next Up

The few things worth picking up next. Each is a pointer; the detail lives below.

- [Verify the port under Paseo](#verify-the-port-under-paseo) — done except the
  reopened-thread path.
- [Label subagent-origin in the permission prompt](#label-subagent-origin-in-the-permission-prompt)
  — spun out of the now-landed schedule/invoke_subagent decision; low priority,
  a clarity improvement rather than a containment gap.
- [SonarCloud analyses nothing today](#sonarcloud-analyses-nothing-today-and-only-ci-based-analysis-can-change-that)
  — overlaps with the cargo clippy/llvm-cov gates now in CI; running both gives
  two sources of truth for the same findings.
- [Rename the binary and crate](#rename-the-binary-and-crate) — cheaper now than
  after anyone else installs it.
- [Confirm the e2e environment actually runs](#confirm-the-e2e-environment-actually-runs)
  — the environment, its secret and its approval rule exist now; no run has gone
  through them yet, so the gate is configured but unproven.

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

### Confirm the e2e environment actually runs

The `e2e` environment exists, holds `E2E_GEMINI_API_KEY` as an environment
secret, and requires reviewer approval; the repository-level `GEMINI_API_KEY`
that no workflow referenced has been removed. What has not happened is a run
through any of it.

That matters because the parts are only load-bearing together. `e2e.yml` skips
fork pull requests before they request the environment, since they cannot receive
secrets and would otherwise wait for an approval that could never help them; the
gate job then reads the secret and reports whether it is present; only then does
the e2e job check out pull-request code. A mistake anywhere in that chain reads
as *skipping*, which is exactly what a missing secret used to read as. Until a
run is watched end to end, "configured" and "working" are indistinguishable from
the outside.

Note that `deployment_branch_policy` is deliberately unset. `e2e.yml` triggers on
`pull_request`, so the ref requesting the environment is the PR's head branch,
which is never protected — a protected-branches-only policy would refuse every
PR and reproduce the skip it was meant to prevent.

Remaining: re-run e2e on a same-repository PR or via `workflow_dispatch`, approve
it, and confirm the gate proceeds, the pinned agy archive verifies, and all four
e2e tests run. It costs a paid API call, so it wants doing once, deliberately.

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

#### One-and-done approval for safe commands (backlog goal)

The ergonomic target the argument-keying deliberately does not reach: let a user
approve `ls` or `cat` *once* and not be prompted for every later variant. Reads
already have this — `view_file`, `list_dir`, `grep_search` are in
`KEYED_BY_TOOL_KINDS`, so one "Always allow" ends the prompting. `run_command`
does not, and cannot get it by the same route, because its reach is one opaque
`CommandLine` the shell re-interprets. A directory listing the model runs as the
`list_dir` tool is already one-and-done; the same listing run as
`run_command "ls"` is keyed by the exact string and reprompts on the next path.

Closing that means a **safe-command classifier**, not a wider sticky key: parse
the command line, prove it is a single invocation of an allowlisted read-only
program with no shell metacharacters, no chaining, no `$()`/backticks and no
redirection, extract its path arguments, and run *those* through the existing
containment and sensitive-path checks. Only then may the answer be remembered by
program prefix rather than by exact string. That last step is itself a new sticky
behaviour, not just a new decision: `run_command` is keyed today by the full
argument fingerprint (`args_fingerprint`), so the classifier would have to emit a
*normalised key* — e.g. `(session, "run_command", Some("ls"))` — or the sticky
logic learn to recognise its output, or "always allow ls" is not sticky at all and
reprompts on every new argument. Naming that scope change is part of the work, not
a footnote. Every parse error is a hole — a
missed `;` turns "always allow ls" into "always allow `ls; rm -rf`" — so it fails
toward prompting, and it must not trust the model's self-reported command. The
shell was observed to be `zsh`, so `sh` parsing cannot be assumed. This shares
the hazard recorded under "Deliberately not taken: parsing what a command does";
the difference is intent — that entry is about *containment depth*, this is about
*approval ergonomics* — but the parser is the same dangerous object and should be
built once if built at all.

Achievable, and there is a working reference for the allowlist half: agy already
maintains exactly this. In normal CLI use, "Always allow" writes a
`command(<glob>)` rule into `~/.gemini/antigravity-cli/settings.json` under
`permissions.allow` — 223 entries here, e.g. `command(ls)`, `command(ls .*)`,
`command(grep .*)`, alongside very specific one-off strings the user approved
once. So a user-curated safe-prefix list already exists on disk in plaintext and
is trivial to read. Whether to *seed* the classifier from it, or to honour it
directly, is the open design question — see "Does agy-acp use agy's own
permission grants?" below for why reading it is not automatically safe.

#### Label subagent-origin in the permission prompt

Spun out of the `schedule`/`invoke_subagent` decision (now landed; see
plans/completed/unclassified-tool-decision.md). That work added a `schedule`
note to the prompt title ("runs in this turn; may hold it open"), but the other
half — telling the user a call came from a spawned subagent rather than the agent
they are talking to — turned out to be more than wording. An unregistered
`conversationId` is *not* a reliable subagent signal: the parent's own first call
is also unregistered until `register_conversation` runs, so naive labelling would
mislabel the common case. Doing it right needs the bridge to track which
conversationIds belong to subagents (observed via `invoke_subagent`), which is
state, not a string. Low priority: it is an honesty/clarity improvement, not a
containment gap — a subagent's calls are still gated by the same hook and the
same path checks.

#### Characterize agy's full tool surface

Answered on 1.1.26, and the answer is that the *native* headless surface is
closed. Asked to enumerate its session tools, agy returned exactly the seventeen
and explicitly denied having `notebook_edit`; every one except `manage_task` has
also been observed in a payload. So the seventeen is the ceiling for a headless
`agy -p` client, not a lower bound.

Two channels add tools beyond the seventeen, both captured and both documented in
`dev-docs/agy-tool-surface.md` ("Extension channels"): MCP servers, whose tools
appear as `mcp_<server>_<tool>` with third-party argument names (captured
`mcp_chrome_devtools_new_page {url}`), and the `/browser` subagent, which is just
`invoke_subagent TypeName:"browser"` driving those MCP tools. The `CascadeToolConfig`
names, few-shot exemplars, and IDE-only tools (`read_terminal`, `workspace_api`,
`view_code_item`, `code_search`) are negotiated off for the headless client and
never appear — confirmed by agy failing to offer `notebook_edit` and falling back
to `run_command`.

So enumeration is done as far as it can be. The remaining work is what it always
was: keep the `"other"` fallthrough as the contract for MCP/unknown tools in
`tool_kind` and the README, and identify path fields for anything newly observed
(the part that fails silently). This entry can be closed once that README wording
lands.

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

#### Does agy-acp use agy's own permission grants? (no, by design — but worth deciding)

Recorded because it will be asked, and because the answer bears on the
safe-command goal above.

**Today the adapter reads none of agy's permission config.** It touches exactly
one thing agy owns: the conversation SQLite DBs under
`~/.gemini/antigravity-cli/conversations/<id>.db`, read-only, for replay
(`db.rs`, `adapter.rs`). It never reads `settings.json`. And it runs agy with
`--dangerously-skip-permissions`, so agy's own permission engine is bypassed
wholesale — the bridge is the only gate, on purpose. That means a grant the user
made in the normal agy CLI (`permissions.allow`, the file/URL/command rules, the
execution policy, the command allowlist/denylist) has **no effect** under the
adapter. The two permission systems are entirely separate.

**Could we read it? Trivially — it is plaintext JSON.** `permissions.allow` is a
flat list of `command(<glob>)` rules, and it is **global, not per-repo** — checked
by looking inside repos (`.agents/` there holds skills and hooks only), for any
project-keyed store under `~/.gemini/antigravity-cli`, and for any other json
carrying a `permissions` key. Only the one global file has them. The strongest
sign it is global by design: repo-specific one-offs the user approved while in a
particular repo (a `grep` on `coverage.xml`, `sed -n` on `mypyskindose/...`) all
landed in that one global list. agy's own settings docs agree — project-level
overrides exist for file/internet/sandbox/execution policy, but the command
allowlist is not among them. `trustedWorkspaces` is a plain list of trusted
roots. Nothing is encrypted. (CLI storage, this version; the desktop app may
differ, and absence is not proof.)

**Should we? Not without a decision, and not naively.** Three reasons honouring
it directly is not free. It is *global* — a user who once allowed `command(rm
.*)` anywhere would have it apply everywhere, and the bridge's whole point is that
the host, not a file agy writes, decides. It *mixes* durable safe prefixes with
one-off exact strings the user clicked through once, so the list is not uniformly
"safe". And it moves the trust boundary onto a file outside the ACP client's
view, which no ACP host has agreed to. The defensible use is narrow: read
`permissions.allow` as a *source of candidate safe prefixes* to seed the
classifier in the goal above, still subject to every containment check, never as
a grant that bypasses a prompt on its own.

**Resolution direction: an opt-in, default-off flag.** The counter-argument is
sound — a user who granted these in native agy has already consented, and
re-asking is friction they cleared. So the answer is a per-host flag (off by
default) that honours `permissions.allow`, not a refusal to read it. Two things
keep it honest and are why it stays opt-in. The grantor and the gate differ under
ACP: the human who approved `command(rm .*)` in their local agy did not thereby
agree to let a *remote* ACP host — a cloud agent, a teammate's session — invoke
it unprompted, so trusting the list is a decision per host, not a default.
And honouring the allowlist is not a faithful replay of native agy: there the
command allowlist sat *alongside* file-access, sandbox and internet policies that
`--dangerously-skip-permissions` removes, so a matched command must still pass the
bridge's containment and sensitive-path checks — command-consent from agy,
path-containment from the bridge. One caveat sharpens the sub-choice: `run_command`
is keyed today by the *full command string*, so honouring a broad glob like
`command(rm .*)` and leaning on containment to clean up is strictly weaker than the
bridge is now — it would let `rm <anything-in-workspace>` through on one grant where
today each distinct command reprompts. So even honour-all must stay keyed per
invocation and fully contained, never a blanket bypass. Open sub-choice: honour the
whole list (faithful to what the user clicked) versus only its safe-prefix subset;
leaning honour-safe-subset given that caveat.

#### Workspace-supplied hooks run outside the bridge

Plan: plans/workspace-hook-trust-boundary.md

Confirmed on agy 1.1.26 (2026-09-04): opening an untrusted repo executes its own
`.agents/hooks.json` commands with no prompt and outside the bridge. A
`PreInvocation` hook ran before the model was even called; a `Stop` hook ran at
loop end. The repo was not in `trustedWorkspaces` and no flag was set. The bridge
is a `PreToolUse` hook and cannot see these events; agy runs every hook command
directly. The adapter is exposed because it sets agy's CWD to the workspace
(`adapter.rs:955`) and adds it as a root (`:905`), both on agy's `.agents/`
discovery path.

The bridge's veto is *not* broken: two merged `PreToolUse` hooks (allow + deny)
tested in both name orders under `--dangerously-skip-permissions` denied every
time (n=2, one merging model), so a repo cannot flip a bridge deny to allow. The
exposure is out-of-band arbitrary execution, not a gate bypass. `trustedWorkspaces`
is not a lever: the test repo was absent from it and hooks ran anyway, so
membership is not required — whether agy reads the list for hooks at all was not
established. (One caveat found while a free-model review re-ran this: the firing
hook must use the flat `PreInvocation` shape, not the `PreToolUse` matcher
wrapper, or agy silently skips it — a repro footgun, not a mitigation.)

Next: fix the README's "bridge is the sole gate" language to exclude hook
commands (ship now), then add opt-in detect-and-surface of a workspace hook dir
before the first turn. Isolated hook discovery needs an upstream agy flag.

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
