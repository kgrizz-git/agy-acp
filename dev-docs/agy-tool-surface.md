# agy's tool surface

What `agy` actually sends the permission bridge. The bridge's containment checks
depend on knowing which arguments are paths, so this is reference material for
`PATH_FIELDS`, `READ_TOOLS`, `SEARCH_TOOLS` and `tool_kind` in
`src/permission.rs`.

**Provenance.** agy 1.1.22, captured 2026-08-30, two ways:

- *Observed* — a `PreToolUse` hook that appends the payload and allows, driven by
  `agy -p '<prompt>' --add-dir <ws> --dangerously-skip-permissions`. Needs no
  adapter build and no Paseo. This is ground truth.
- *Self-reported* — agy asked to enumerate its tools and their parameter names.
  Useful for finding things to look for, **not** authoritative: it reported
  `find_by_name.FullPath` in a list of parameter names, and observation showed it
  is a boolean, not a path.

Anything below marked self-reported has not been seen in a payload.

## Tools

Seventeen, self-reported:

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
| `generate_image` | `ImageName`, `Prompt` | none — see below |

Deliberately *not* path fields: `Url`, `query`, `Pattern`, and `FullPath`, which
is a boolean despite its name. Note the casing is inconsistent — `grep_search`
uses `Query`, `search_web` uses lowercase `query`.

`ArtifactMetadata` is a nested object (`RequestFeedback`, `Summary`,
`UserFacing`) holding model-authored prose, and it is not always present: two
calls that were the same logical write, an original and its retry, differed by
whether it appeared at all.

## Self-reported arguments, unobserved

| tool | reported parameters |
|---|---|
| `generate_image` | `AspectRatio`, `ImageName`, `ImagePaths`, `Prompt` |
| `manage_task` | `Action`, `Input`, `TaskId` |
| `send_message` | `Message`, `Recipient` |
| `schedule` | `CronExpression`, `DurationSeconds`, `MaxIterations`, `Prompt`, `TimerCondition` |
| `invoke_subagent` | `Subagents` |
| `define_subagent` | `description`, `enable_mcp_tools`, `enable_subagent_tools`, `enable_write_tools`, `name`, `system_prompt` |
| `manage_subagents` | `Action`, `ConversationIds` |

`ImagePaths` is the one worth watching: if it holds paths it belongs in
`PATH_FIELDS`, but no call has produced it, so it is not there yet.

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

## Mismatches with this fork's lists

Five names in `src/permission.rs` — `view_code_item`, `codebase_search`,
`edit_file`, `propose_code`, `command_status` — are absent from the self-reported
list *and* never appeared in a captured payload, including on prompts that should
have drawn them out: a request for semantic search produced `grep_search`, one to
view a specific code item produced `view_file`, and one to edit produced
`replace_file_content`. They read as upstream vocabulary this fork inherited.
Stated that way deliberately: absence of evidence across two sources is strong,
but it is not the same as knowing agy cannot emit them under some other
configuration or version.

Seven tools in the self-reported list are unclassified here — `manage_task`,
`send_message`, `schedule`, `invoke_subagent`, `define_subagent`,
`manage_subagents`, `generate_image`. Only `generate_image` has been observed in
a payload; the rest are self-reported and unobserved. All seven fall through
`tool_kind` to `"other"` and belong to no auto-allow group, so they always
prompt — the right default, but reached by omission, and that is true whether or
not the self-report is complete.

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
