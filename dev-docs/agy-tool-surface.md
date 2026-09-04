# agy's tool surface

What `agy` actually sends the permission bridge. The bridge's containment checks
depend on knowing which arguments are paths, so this is reference material for
`PATH_FIELDS` in `src/permission/path_rules.rs`, and for `READ_TOOLS`,
`SEARCH_TOOLS` and `tool_kind` in `src/permission.rs`.

**Provenance.** agy 1.1.22 captured 2026-08-30, extended on agy 1.1.25
captured 2026-09-03. Everything is captured one of three ways:

- *Observed* — a `PreToolUse` hook that appends the payload and allows, driven by
  `agy -p '<prompt>' --add-dir <ws>`. Needs no adapter build and no Paseo. The
  hook fires at `PreToolUse` and captures the payload whether or not agy then
  executes the tool, so this enumerates the tool *surface* — names and arguments —
  without `--dangerously-skip-permissions`. It is **not** evidence a tool ran.
  Verified on agy 1.1.26 (2026-09-04): with no flag, a `write_to_file` whose hook
  returned `{"decision":"allow"}` was still auto-denied — agy printed *"a tool
  required the 'write_file' permission that headless mode cannot prompt for, so it
  was auto-denied"* — identically whether the hook allowed, denied, or was absent.
  So a hook `allow` does not override the headless soft-deny; only a `deny` is
  honoured, matching the AGENTS.md finding. The one thing that *does* run without
  the flag is a command agy's own `permissions.allow` already covers: `touch`
  executed in an earlier capture only because `command(touch)` is in that list, not
  because the hook allowed it. Ground truth for names and arguments; not for
  outcomes.
- *Read out of the binary* — `strings` over the `agy` executable. It carries the
  `exa.cortex_pb` protobuf descriptors, so a name found there is a name the
  binary knows. It says nothing about whether the name is enabled.
- *Self-reported* — agy asked to enumerate its tools and their parameter names.
  Useful for finding things to look for, **not** authoritative: it reported
  `find_by_name.FullPath` in a list of parameter names, and observation showed it
  is a boolean, not a path.

Anything below marked self-reported has not been seen in a payload.

## Tools

Seventeen in this configuration, self-reported and unchanged between 1.1.22 and
1.1.25:

```
view_file    run_command           manage_task      send_message      schedule
list_dir     write_to_file         invoke_subagent  define_subagent   manage_subagents
grep_search  replace_file_content  generate_image   read_url_content  search_web
find_by_name ask_question
```

## Observed arguments

Every tool below was triggered and its payload captured. `toolAction` and
`toolSummary` are on every call and omitted here; both are model-authored display
text that changes between otherwise identical calls.

| tool | arguments | path fields |
|---|---|---|
| `run_command` | `CommandLine`, `Cwd`, `WaitMsBeforeAsync` | `Cwd` |
| `view_file` | `AbsolutePath` | `AbsolutePath` |
| `list_dir` | `DirectoryPath` | `DirectoryPath` |
| `grep_search` | `Query`, `SearchPath` | `SearchPath` |
| `find_by_name` | `Pattern`, `SearchDirectory`, `Extensions`, `Type`, `MaxDepth`, `FullPath` | `SearchDirectory` |
| `write_to_file` | `TargetFile`, `CodeContent`, `Description`, `Overwrite`, `ArtifactMetadata` | `TargetFile` |
| `replace_file_content` | `TargetFile`, `TargetContent`, `ReplacementContent`, `Instruction`, `StartLine`, `EndLine`, `AllowMultiple`, `Description` | `TargetFile` |
| `read_url_content` | `Url` | none |
| `search_web` | `query` | none |
| `generate_image` | `ImageName`, `Prompt`, `AspectRatio`, `ImagePaths` | `ImagePaths` — see below |
| `ask_question` | `questions[]` (`question`, `options`, `is_multi_select`) | none |
| `schedule` | `CronExpression`, `DurationSeconds`, `MaxIterations`, `Prompt`, `TimerCondition` | none |
| `send_message` | `Message`, `Recipient` | none |
| `define_subagent` | `name`, `description`, `system_prompt`, `enable_write_tools`, `enable_mcp_tools`, `enable_subagent_tools` | none |
| `invoke_subagent` | `Subagents[]` (`TypeName`, `Role`, `Prompt`, `Workspace`) | `Subagents[].Workspace` |
| `manage_subagents` | `Action`, `ConversationIds` | none |

Deliberately *not* path fields: `Url`, `query`, `Pattern`, and `FullPath`, which
is a boolean despite its name. Note the casing is inconsistent — `grep_search`
uses `Query`, `search_web` uses lowercase `query`.

`ArtifactMetadata` is a nested object (`RequestFeedback`, `Summary`,
`UserFacing`) holding model-authored prose, and it is not always present: two
calls that were the same logical write, an original and its retry, differed by
whether it appeared at all.

## Self-reported arguments, unobserved

One tool is left unobserved, and a handful of optional parameters on observed
tools have never appeared in a payload. Self-reported on 1.1.25.

| tool | reported parameters not yet seen |
|---|---|
| `manage_task` | `Action` (`list`/`kill`/`status`/`send_input`), `Input`, `TaskId` — whole tool unobserved |
| `view_file` | `ContentOffset`, `StartLine`, `EndLine` |
| `grep_search` | `CaseInsensitive`, `Includes`, `IsRegex`, `MatchPerLine` |
| `find_by_name` | `Excludes` |
| `replace_file_content` | `TargetLintErrorIds` |
| `search_web` | `domain` |
| `invoke_subagent` | `Subagents[].Model` |

None of them names a path a containment check would run on. `Includes`/`Excludes`
are the borderline pair — they hold glob patterns, which can look path-like — but
they filter *within* `SearchDirectory`/`SearchPath`, which are already path-checked,
so they do not independently reach the filesystem. They are the entry to revisit
if a capture ever shows an absolute glob.

`ImagePaths` has now been observed holding an absolute path
(`["/tmp/agycap/img/seed.txt"]`), so the question it used to raise is settled: it
is a path field. So is `Subagents[].Workspace`, observed carrying
`/tmp/agycap/outside`. Antigravity also documents `inherit`, `branch` and `share`
as values for that field; a bare word resolves inside the workspace and does not
prompt, so accepting both shapes costs nothing.

`manage_task` takes `Action` in `list`, `kill`, `status`, `send_input` — there is
no create action, and `schedule` is what creates a task. `schedule` rejects
`MaxIterations` with `DurationSeconds`, and `TimerCondition` with
`CronExpression`.

On whether to add a candidate before seeing it: the two failure directions are
not symmetric, but neither is free. A missing name fails **silently** — the value
is judged only by shape, so a relative path that escapes through a symlink is
never checked. An extra name fails **loudly**, as a spurious prompt, and only for
values that resolve outside the workspace; a plain relative value resolves inside
and is not prompted. (Strictly, that holds once a workspace root is set. With no
root the bridge fails closed and returns the first path-field value it finds
without a containment check, so there an extra name prompts on anything — still
the safe direction, and the adapter sets a root before the first hook call.) So
over-inclusion is the safer error, not a costless one, and
it is worth it when something independent says the field names a path — as with
`FilePath`, which `tools.rs` and `protobuf.rs` already treat as a location. agy
describing its own schema is not that: `FullPath` reads exactly like a path field
and holds a boolean.

## How agy runs a command

`run_command` shells out, and agy puts each command it starts into a **process
group of its own** rather than leaving it in agy's. Observed on agy 1.1.22 with
the adapter deliberately spawning agy as a group leader:

```
PID   PPID  PGID
73606 73578 73606   agy                                  <- leads its own group
73687 73606 73687   zsh -c 'sleep 45 && touch marker'    <- and its own, not agy's
73688 73687 73687   sleep 45
```

This is why cancelling a turn kills agy's process *tree* rather than its process
group: `killpg` on agy reaches agy alone. agy is still spawned into a group of
its own, but only so a signal aimed at the adapter's group cannot kill agy before
the tree under it can be read. `scripts/probe-cancel.py` reproduces
the whole thing end to end. It also means the command survives agy
by default — reparented to PID 1 — so anything that stops a turn has to stop the
command explicitly. See `src/proc.rs`.

Note the shell here was `zsh` rather than `sh`. Presumably `$SHELL`, though that
was not tested — do not assume `sh` when matching on the command line.

## Where agy writes

`generate_image` takes no destination argument. Asked to save into the
workspace, it wrote to `~/.gemini/antigravity-cli/brain/<conversation-id>/` and
reported that it had saved into the workspace.

This is normal Antigravity behaviour rather than a fault — Gemini writes into its
own internal workspace unless told otherwise — but it matters here for two
reasons. The bridge cannot constrain a destination that is not in the arguments,
so the README's "only inside the workspace" limit does not describe this tool.
And the model's own account of where it wrote was wrong, so a transcript is not
evidence of where a file landed.

The adapter passes the user's workspace with `--add-dir` but does not instruct
agy to prefer it for generated artifacts. If artifacts should land in the
workspace, that has to be said explicitly.

## How subagents reach the bridge

`define_subagent` then `invoke_subagent` produces four hook payloads, and the
fourth is the subagent's own work:

```
e995df2e step 2  define_subagent   {name: scribe, enable_write_tools: true, ...}
e995df2e step 4  invoke_subagent   {Subagents: [{TypeName, Role, Prompt}]}
e995df2e step 6  manage_subagents  {Action: list}
b414bc71 step 2  write_to_file     {TargetFile: .../scribe.txt}   <- the subagent
```

So a subagent is gated by the same hook as its parent: the bridge is still one
chokepoint. Three details matter to the code.

The subagent runs under its **own `conversationId`**, which the bridge has never
registered. `decide` misses in `state.conversations` and falls back to
`active_session`, which is the parent — correct because the adapter spawns one
`agy` per turn and serializes prompts, not because anything checks.

The subagent's payload reports the **parent's** `workspacePaths` even when it was
given `Workspace: /tmp/agycap/outside` and writes there. The bridge does not read
that field; it checks argument paths against its own `workspace_roots`, so the
outside write is caught. A containment check that trusted `workspacePaths` would
have been wrong here.

A subagent does not outlive the `agy` process. In two captures the subagent's
`write_to_file` was allowed by the hook but never produced a file, and the parent
reported the subagent "canceled by the system" — consistent with the subagent
being killed when the parent turn ended, though these runs had no
`--dangerously-skip-permissions`, so a headless soft-deny of the write cannot be
ruled out as the cause. Either way the file did not land.

## What `schedule` actually does

In every call observed it did not defer work past the turn: it parked the turn
and ran the work as further steps of the same conversation. Only a couple of
parameter shapes were tried, so read this as "not observed to defer", not a proof
it cannot for some interval or duration:

```
1788490605 e1e54c9a step 2  schedule    {DurationSeconds: 45, Prompt: "Run ... touch fired.txt"}
1788490656 e1e54c9a step 6  run_command {CommandLine: "touch fired.txt"}
```

A cron is the same: `"*/1 * * * *"` with `MaxIterations 5` fired in process and
held the turn open for about five minutes. So a `schedule` call is a
turn-duration decision, bounded by `PERMISSION_PRINT_TIMEOUT` (60 minutes in
`adapter.rs`), and the scheduled work arrives at the hook as ordinary tool calls.

This is also how the model waits. Told to wait for a subagent it called
`schedule` with `DurationSeconds: 600`, `TimerCondition: "any"` and
`Prompt: "Wait for subagent"`, so the tool is routine rather than exotic.

Asked directly, agy describes `schedule` the other way — as deferring work to a
*new* turn, run as a background task that wakes it by notification when the timer
fires. That is its daemon/IDE behaviour (self-reported, so not authoritative).
The two reconcile: headless `agy -p`, which is what the adapter runs, has no
daemon to wake it later, so the deferral collapses into keeping the one turn open
and running the work as continuation steps — the in-turn behaviour observed
above. So `schedule` *can* defer past a turn in daemon mode; under the adapter it
does not, which is why the hedge above ("not observed to defer") is the right
framing rather than a flat "cannot".

## Why the list cannot be closed

The binary carries an `exa.cortex_pb.CascadeToolConfig` — a per-tool enable map
whose fields name about thirty-five tools, against the seventeen this
configuration exposes. Among them: `view_code_item`, `command_status`,
`code_search`, `internal_search`, `knowledge_base_search`, `notebook_edit`,
`browser_subagent`, `antigravity_browser`, `memory`, `skill_search`, `mquery`,
`workspace_api`, `ask_permission`. Those are tools this binary can be configured
to turn on, so "never observed" means "not enabled here", not "cannot be
emitted".

Beyond that map the binary also carries baked-in few-shot prompt examples that
*show the model* tool calls by name — `codebase_search` with `Query` and
`TargetDirectories`, `edit_file` with `TargetFile`, `CodeEdit`,
`CodeMarkdownLanguage` and `Blocking` — and Cortex step types such as
`CortexStepProposeCode`. A name in the exemplars is a name the model has been
taught to write, which is a second route by which something outside the
seventeen could arrive at the bridge.

`agy mcp add` puts the other end of it beyond enumeration entirely: under an MCP
server both the tool name and its argument names are whatever a third party
chose. `PATH_FIELDS` therefore cannot be completed by listing, and the `"other"`
fallthrough in `tool_kind` — unknown tool, argument-keyed sticky, always prompt —
is the contract rather than a gap waiting to be filled.

## Mismatches with this fork's lists

Five names once in `src/permission.rs` — `view_code_item`, `codebase_search`,
`edit_file`, `propose_code`, `command_status` — were absent from the self-reported
list *and* never appeared in a captured payload, including on prompts that should
have drawn them out: a request for semantic search produced `grep_search`, one to
view a specific code item produced `view_file`, and one to edit produced
`replace_file_content`. The binary knows all five, in three different senses, and the distinction is
worth keeping straight. `view_code_item` and `command_status` are fields in the
`CascadeToolConfig` enable map above, so they are **config-gated tools**.
`codebase_search` and `edit_file` appear in the **few-shot prompt examples**,
with full argument shapes. `propose_code` is a **step type**
(`CortexStepProposeCode`), behind a `use_replace_content_propose_code` flag.

So the earlier reading — "upstream vocabulary this fork inherited" — was too
comfortable. None of the three senses says agy cannot emit these under another
configuration; two of them say it plainly could.

**They have been removed**, and that uncertainty is the reason rather than an
argument against. Being wrong in each direction costs something different. A name
kept costs nothing *if* agy never sends it — but `tool_kind` is not only a label:
`sticky_scope` asks `KEYED_BY_TOOL_KINDS` whether a kind may be remembered by
tool name alone, and `"read"`, `"edit"` and `"search"` may. Pre-classifying a
tool nobody has seen therefore promises that its arguments are constrained by the
path checks, on no evidence that they are, and one "Always allow" would then
cover every later call to it. A name removed costs a prompt: an unknown tool
falls to `"other"`, is keyed by its arguments, and asks. So the wrong guess is
the cheap one only in the direction of removal, which is why the removal does not
wait for proof that agy cannot emit them.

Seven tools in the self-reported list are unclassified here — `manage_task`,
`send_message`, `schedule`, `invoke_subagent`, `define_subagent`,
`manage_subagents`, `generate_image`. All seven have since been observed in a
payload except `manage_task` (see "How subagents reach the bridge" and "What
`schedule` actually does" above). All fall through `tool_kind` to `"other"` and
belong to no auto-allow group, so they always prompt. That was already the
behaviour; what changed is that it is now a decision rather than an omission,
recorded in `tool_kind`'s doc comment and pinned by a test. `schedule` and
`invoke_subagent` are why it was worth deciding: one holds the turn open past the
call the permission was scoped to and the other spawns an agent, so neither
should ever inherit an answer the user gave about something else.

agy has **no dedicated delete tool**: asked to delete a file it shells out to
`rm` via `run_command`, so deletion is governed by the command path.

## Reproducing

```sh
W=$(mktemp -d)
mkdir -p "$W/.agents"
# hook that appends each payload and allows
printf '{"probe":{"PreToolUse":[{"matcher":"*","hooks":[{"type":"command","command":"%s/dump.sh","timeout":600}]}]}}\n' "$W" > "$W/.agents/hooks.json"
agy -p '<prompt that triggers the tool>' --add-dir "$W" --dangerously-skip-permissions
```

The hook must `cat` stdin to a file and echo `{"decision":"allow"}`. agy only
discovers `.agents/hooks.json` in a directory passed with `--add-dir`, so the
workspace has to be added explicitly even when it is the working directory.
