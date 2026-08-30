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

Five names in `src/permission.rs` are for tools agy does not have —
`view_code_item`, `codebase_search`, `edit_file`, `propose_code`,
`command_status`. They are upstream vocabulary this fork inherited.

Seven tools agy does have are unclassified — `manage_task`, `send_message`,
`schedule`, `invoke_subagent`, `define_subagent`, `manage_subagents`,
`generate_image`. They fall through `tool_kind` to `"other"` and belong to no
auto-allow group, so they always prompt. That is the right default, but it is
reached by omission.

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
