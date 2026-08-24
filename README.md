# agy-acp

An [Agent Client Protocol (ACP)](https://agentclientprotocol.com) stdio adapter for [Google Antigravity CLI](https://github.com/google-antigravity/antigravity-cli) (`agy`). It bridges `agy` into any ACP-compatible host like [Zed](https://zed.dev), enabling you to use Gemini models through `agy` inside Zed's Agent Panel.

## Features

- **Real-Time Streaming**: Directly streams NDJSON events from `agy --output-format stream-json` to deliver fast, incremental text updates.
- **Thinking / Thought Streaming**: Streams model reasoning blocks as ACP thought updates, allowing compatible hosts to render the model's thought process in real time.
- **Rich Tool Execution**: Maps `agy` tool operations (`read`, `edit`, `delete`, `move`, `search`, `execute`, `fetch`, etc.) into structured ACP tool calls with target file paths, line ranges, and formatted outputs (such as directory listings and grep search results).
- **Session Cancellation**: Handles `session/cancel` by cleanly aborting in-flight prompts and terminating the underlying `agy` subprocess.
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

Tool calls then arrive as ACP `session/request_permission` requests, with **Allow** / **Always allow** / **Reject** / **Always reject** options. "Always" answers are remembered per tool for the rest of the session.

This works by installing a `PreToolUse` hook for `agy` in a private directory of the adapter's own — nothing is written to your workspace or to your global `agy` config, so plain `agy` use in a terminal is unaffected.

> [!IMPORTANT]
> Enabling this runs `agy` with `--dangerously-skip-permissions`, because a hook cannot grant a permission that `agy`'s own checks have already denied — while they are active a hook can only veto. The adapter becomes the only gate on tool execution, so anything it cannot resolve (no host to ask, host disconnected, no answer in time) is denied.

### What runs without asking

Only `ask_question` by default: it asks you something and cannot touch the filesystem. Reads are *not* auto-allowed out of the box — `agy`'s own checks are off, so a read you never see is a read of anything the process can reach.

Opt in with `AGY_ACP_AUTO_ALLOW`, which takes tool names and the groups `reads` (`view_file`, `view_code_item`, `list_dir`), `searches` (`grep_search`, `codebase_search`, `find_by_name`) and `none`:

```json
"env": { "AGY_ACP_AUTO_ALLOW": "ask_question,reads,searches" }
```

Whatever is enabled, three limits still apply:

- **Only inside the workspace** — an absolute path outside the workspace root is still prompted.
- **No network reads** — `read_url_content` and `search_web` are outside both groups. They only read, but a URL carries data out.
- **Credential-looking paths are still prompted** — `.env`, `.pem`/`.key`/`id_rsa`, `.ssh`/`.aws`/`.gnupg`/`.kube`, `.netrc`/`.npmrc`/`.git-credentials`, and names containing `token`, `secret`, `password` or `credential`. Extend with `AGY_ACP_SENSITIVE_PATTERNS`. This list cannot be complete and is not what makes the feature safe — the narrow default is.

## Configuration & Environment

| Setting / Variable | Description |
|---|---|
| `--skip-naration` | CLI flag to filter out leading narrative preamble messages |
| `GEMINI_API_KEY` | API key for Gemini (passed through to `agy`) |
| `AGY_EXTRA_ARGS` | Space-separated extra args passed to every `agy` invocation |
| `AGY_ACP_AUTO_ALLOW` | What may run without asking. Tool names plus the groups `reads`, `searches`, `none`. Default `ask_question` |
| `AGY_ACP_SENSITIVE_PATTERNS` | Extra comma-separated substrings marking a path as too sensitive to read without asking |
| `AGY_ACP_PERMISSION_TIMEOUT_SECS` | How long a permission request waits for an answer before denying. Default `540` |

## Session Persistence

Sessions are persisted to `~/.openab/agy-acp/sessions.json`. When you resume a session in Zed, `agy-acp` restores the conversation binding and continues it with `agy --conversation <id>`. State persistence uses atomic write-to-temp-and-rename under an exclusive file lock to avoid data corruption.

## Debugging

To inspect the JSON-RPC messages between Zed and `agy-acp`, run `dev: open acp logs` from Zed's Command Palette.

## License

MIT

