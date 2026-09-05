# agy-acp

An [Agent Client Protocol (ACP)](https://agentclientprotocol.com) stdio adapter for [Google Antigravity CLI](https://github.com/google-antigravity/antigravity-cli) (`agy`). It bridges `agy` into any ACP-compatible host like [Zed](https://zed.dev), enabling you to use Gemini models through `agy` inside Zed's Agent Panel.

## Features

- **Real-Time Streaming**: Directly streams NDJSON events from `agy --output-format stream-json` to deliver fast, incremental text updates.
- **Thinking / Thought Streaming**: Streams model reasoning blocks as ACP thought updates, allowing compatible hosts to render the model's thought process in real time.
- **Rich Tool Execution**: Maps `agy` tool operations (`read`, `edit`, `delete`, `move`, `search`, `execute`, `fetch`, etc.) into structured ACP tool calls with target file paths, line ranges, and formatted outputs (such as directory listings and grep search results).
- **Session Cancellation**: Handles `session/cancel` by cleanly aborting in-flight prompts and terminating the underlying `agy` subprocess along with every process it started, so a command still running under it stops too. Reaching the whole tree needs a Unix process table; on other platforms only `agy` itself is killed, and a command it started outlives the cancel.
- **Dynamic Model Selection**: Automatically queries models via `agy models` on startup and exposes them as ACP configuration options. Supports both `session/set_model` and `session/setConfigOption`.
- **Session Persistence & Resume**: Saves conversation mappings to disk with atomic writes and file locking, allowing sessions to resume seamlessly across restarts.
- **Narration Filtering**: Provides a `--skip-naration` CLI flag to filter out leading narrative chatter (e.g., *"I will..."*) before model actions.

## How It Works

`agy-acp` speaks JSON-RPC over stdin/stdout (the ACP transport). When a host sends a prompt, `agy-acp` spawns `agy` in stream-json mode, streams the output incrementally back via `session/update` notifications, and binds the `conversation_id` so subsequent turns or resumed sessions retain context.

```
Zed (ACP host)  <--stdin/stdout JSON-RPC-->  agy-acp  <--subprocess-->  agy  <--API-->  Gemini
```

## Prerequisites

- **Rust** (1.70+) with Cargo
- **`agy`** installed and in your `PATH` — install from [google-antigravity/antigravity-cli releases](https://github.com/google-antigravity/antigravity-cli)
- **Authentication** — either set `GEMINI_API_KEY` or configure auth via `~/.gemini/antigravity-cli/settings.json`

## Build & Install

```bash
cargo build --release
```

The binary is generated at `target/release/agy-acp`. Copy it to a directory in your `PATH`:

```bash
cp target/release/agy-acp /usr/local/bin/
```

## Use with Zed

Add `agy-acp` as a custom agent server in your Zed settings (`~/.config/zed/settings.json`):

```json
{
  "agent_servers": {
    "agy": {
      "type": "custom",
      "command": "agy-acp",
      "args": ["--permission-prompts"],
      "env": {}
    }
  }
}
```

> [!IMPORTANT]
> **Tool Execution & Permissions:** `agy` runs headless under this adapter and cannot ask you about tool permissions itself, so without help it auto-denies them and tool calls fail silently. `--permission-prompts` routes each decision to your ACP host instead — see [Permission Prompts](#permission-prompts) for what it approves on its own and what it always asks about.
>
> The alternative is `AGY_EXTRA_ARGS="--dangerously-skip-permissions"`, which makes `agy` approve every tool call itself: file edits, command execution and network access, with nothing to refuse them. Only do that in a sandbox you are willing to lose.

Then open the Agent Panel in Zed (`Cmd-?` on macOS, `Ctrl-?` on Linux), select **agy** from the agent dropdown, and start chatting.

### Filtering Narration

To suppress leading narrative chatter from the model, pass `--skip-naration` in the arguments:

```json
{
  "agent_servers": {
    "agy": {
      "type": "custom",
      "command": "agy-acp",
      "args": ["--skip-naration"],
      "env": {}
    }
  }
}
```

### Passing Extra Arguments

Set the `AGY_EXTRA_ARGS` environment variable to pass additional arguments to every `agy` invocation:

```json
{
  "agent_servers": {
    "agy": {
      "type": "custom",
      "command": "agy-acp",
      "args": [],
      "env": {
        "AGY_EXTRA_ARGS": "--some-flag value"
      }
    }
  }
}
```

## Permission Prompts

`agy` runs headless under this adapter, and headless `agy` cannot prompt for tool permissions — it auto-denies anything that needs one, so the tool call fails silently and the model stops. Pass `--permission-prompts` to ask the ACP host instead:

> Permission prompts require a Unix platform (macOS or Linux). The rest of the
> adapter remains usable on Windows, but this opt-in mode is safely unavailable
> there because it relies on Unix-domain sockets.

```json
{
  "agent_servers": {
    "agy": {
      "type": "custom",
      "command": "agy-acp",
      "args": ["--permission-prompts"],
      "env": {}
    }
  }
}
```

Tool calls then arrive as ACP `session/request_permission` requests, with **Allow once** / **Always allow \<tool\> this session** / **Reject** / **Always reject \<tool\> this session** options. For a tool that runs a command the two "always" labels instead read **Always allow this exact command this session** and **Always reject this exact command this session**, because that is what they cover. Each label says what the answer is remembered by. See [What "Always" remembers](#what-always-remembers).

This works by installing a `PreToolUse` hook for `agy` in a private directory of the adapter's own — nothing is written to your workspace or to your global `agy` config, so plain `agy` use in a terminal is unaffected.

> [!IMPORTANT]
> Enabling this runs `agy` with `--dangerously-skip-permissions`, because a hook cannot grant a permission that `agy`'s own checks have already denied — while they are active a hook can only veto. The adapter becomes the only gate on the model's **tool calls**, so anything it cannot resolve (no host to ask, host disconnected, no answer in time) is denied.
>
> One thing it does *not* gate: `agy` also runs any lifecycle-hook commands a workspace ships in its own `.agents/hooks.json` — a `PreInvocation` hook before the turn, a `Stop` hook at the end — and those execute directly, outside this bridge. Opening an untrusted repository can therefore run its hook commands with no prompt, even when the repository is not in `trustedWorkspaces`. The bridge's veto over tool calls still holds regardless; the exposure is out-of-band command execution, not a way to flip a bridge deny to allow.

### What runs without asking

Only `ask_question` by default: it asks you something and cannot touch the filesystem. Reads are *not* auto-allowed out of the box — `agy`'s own checks are off, so a read you never see is a read of anything the process can reach.

Opt in with `AGY_ACP_AUTO_ALLOW`, which takes tool names and the groups `reads` (`view_file`, `list_dir`), `searches` (`grep_search`, `find_by_name`) and `none`:

```json
"env": { "AGY_ACP_AUTO_ALLOW": "ask_question,reads,searches" }
```

Whatever is enabled, three limits still apply:

- **Only inside the workspace** — an argument that names a path leaving the workspace root is still prompted, whether it is absolute, `~/...`, or `../` traversal, and whether it leaves textually or by following a symlink out. An argument counts as naming a path when it sits in one of agy's path fields (`AbsolutePath`, `TargetFile`, `DirectoryPath`, `SearchPath`, `Cwd`, `Paths`) whatever its value looks like, or when its value starts with `/` or `~` or carries a `..`. A search query is not mistaken for a file, and a path field that agy adds later is judged by value until this list catches up. A path inside a shell command string is not seen; see below.
- **No network reads** — `read_url_content` and `search_web` are outside both groups. They only read, but a URL carries data out.
- **Credential-looking paths are still prompted** — `.env`, `.pem`/`.key`/`id_rsa`, `.ssh`/`.aws`/`.gnupg`/`.kube`, `.netrc`/`.npmrc`/`.git-credentials`, and names containing `token`, `secret`, `password` or `credential`. Extend with `AGY_ACP_SENSITIVE_PATTERNS`. This list cannot be complete and is not what makes the feature safe — the narrow default is.

### What "Always" remembers

An "Always allow" or "Always reject" is remembered for **the rest of the
session**. What it is keyed by depends on the tool, and the button you press says
which:

- **Always allow \<tool\> this session** — keyed by the *tool*. Every later call
  to it is covered, whatever file it names.
- **Always allow this exact command this session** — keyed by the tool *and the
  arguments you were shown*. A later call with any different argument asks again.

The narrow, per-command key is the default, and a tool has to *earn* the broader
one. It earns it only by being a plain read, edit or search tool whose arguments
name nothing but paths — because those are exactly the calls the two checks below
still constrain. An argument that carries a command line or a URL reaches
somewhere those checks cannot follow, so the answer is pinned to the exact
arguments instead. That is why **Always allow** on `read_url_content` covers the
one URL you approved and not the next one.

A tool this fork does not recognise falls through to the same narrow treatment,
by design. An MCP server tool (`mcp_<server>_<tool>`), a subagent-driven call, or
anything new `agy` starts emitting is classed as **other**: it never earns the
broad per-tool key, is remembered only by its exact arguments, and is in no
auto-allow group, so it always prompts. `agy`'s native headless surface is a
fixed set, but MCP servers and the `/browser` subagent can extend it at runtime;
treating everything outside the known read/edit/search tools as **other** is the
contract that keeps those open-ended additions prompting rather than silently
allowed.

"Exact" means exact: the arguments are compared as-is, with no tokenizing and no
shell semantics. `ls -l` and `ls  -l` are different commands and each is asked
separately. Only presentational fields agy attaches to the call — its own summary
of the action and an async wait hint — are ignored, since they do not change what
runs. Under-matching costs you a prompt; over-matching would be a hole, so the
comparison errs toward asking.

This is a preference held in memory, not a stored grant: answers are scoped to
one session id, are never written to disk, and are forgotten when the adapter
process exits or when the session is evicted from the adapter's in-memory map.
Restart or reconnect your host — even reopening the same thread — and you will be
asked again.

Two checks still apply to a remembered **allow**, and will bring the prompt back:

- the path is outside the workspace, or
- the path looks credential-bearing (the list above).

Those checks read the tool's arguments as *paths*. A shell command is a single
opaque string — `cat /etc/shadow` is not recognised as naming `/etc/shadow` — and
a URL is not on the filesystem at all. That is exactly why those answers are
keyed by their arguments: the checks cannot constrain them, so the key has to. A
remembered allow for `cat README.md` grants `cat README.md` and nothing else.

A remembered **reject** narrows the same way, and applies immediately. Rejecting
one command forever rejects that command, not every command; rejecting a read
tool with **Always reject \<tool\>** rejects the tool.

There is no way to revoke an "Always" answer within a session; starting a new
one, or restarting the host, clears it.

## Configuration & Environment

| Setting / Variable | Description |
|---|---|
| `--skip-naration` | CLI flag to filter out leading narrative preamble messages |
| `GEMINI_API_KEY` | API key for Gemini (passed through to `agy`) |
| `AGY_EXTRA_ARGS` | Space-separated extra args passed to every `agy` invocation |
| `AGY_ACP_AUTO_ALLOW` | What may run without asking. Tool names plus the groups `reads`, `searches`, `none`. Default `ask_question` |
| `AGY_ACP_SENSITIVE_PATTERNS` | Extra comma-separated substrings marking a path as too sensitive to read without asking. Matched against every string argument, so it also catches substrings of a command line |
| `AGY_ACP_PERMISSION_TIMEOUT_SECS` | How long a permission request waits for an answer before denying. Default `540` |

## Session Persistence

Sessions are persisted to `~/.openab/agy-acp/sessions.json`. When you resume a session in Zed, `agy-acp` restores the conversation binding and continues it with `agy --conversation <id>`. State persistence uses atomic write-to-temp-and-rename under an exclusive file lock to avoid data corruption.

## Debugging

To inspect the JSON-RPC messages between Zed and `agy-acp`, run `dev: open acp logs` from Zed's Command Palette.

## License

MIT
