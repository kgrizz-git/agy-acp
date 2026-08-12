# agy-acp

Single Rust crate. ACP (Agent Client Protocol) stdio adapter for Google Antigravity CLI (`agy`). Bridges `agy` into OpenAB's JSON-RPC protocol.

## Commands

```bash
cargo build                    # debug build
cargo build --release          # release build (required for e2e tests)
cargo test                     # unit tests only (fast, no I/O)
cargo test -- --include-ignored  # all tests including filesystem I/O tests
cargo test e2e -- --ignored --nocapture  # e2e only (needs agy binary + auth)
```

No separate lint/typecheck/format commands — just `cargo build` and `cargo test`.

## Architecture

- `main.rs` — stdin/stdout JSON-RPC loop. Reads lines, dispatches to adapter methods, writes responses.
- `adapter.rs` — core logic: session lifecycle, spawning `agy` subprocess, state persistence. `Adapter::new()` reads `HOME` for state/conv dirs.
- `db.rs` — reads agy's SQLite conversation DBs (read-only). Table: `steps` with columns `idx`, `step_type`, `step_payload`.
- `protobuf.rs` — hand-rolled protobuf varint/field extraction (no prost/protobuf dependency). Extracts text from `step_payload` field 20 → sub-field 1.
- `streaming.rs` — polls SQLite every 500ms during `session/prompt`, emits incremental `session/update` notifications to stdout.
- `types.rs` — JSON-RPC types, `SessionStore` for persistence, `StreamingState`.
- `permission.rs` — `--permission-prompts` only. Unix socket server turning agy's `PreToolUse` hook into ACP `session/request_permission`, plus the `agy-acp permission-hook` subcommand agy invokes.
- `hook_root.rs` — `--permission-prompts` only. Writes that hook into a private temp dir handed to agy as an extra `--add-dir`.

## Key paths

| Path | Purpose |
|---|---|
| `~/.openab/agy-acp/sessions.json` | Persisted session→conversation mapping (with `.lock` file for mutual exclusion) |
| `~/.gemini/antigravity-cli/conversations/*.db` | agy's SQLite conversation databases |

## Test tiers

1. **Unit tests** (`cargo test`) — protobuf parsing, narration filtering, JSON-RPC response shape. No filesystem or network I/O.
2. **Ignored I/O tests** (`-- --include-ignored`) — session persist/restore, SQLite read, conversation snapshot. Create temp dirs in `$TMPDIR`.
3. **E2E tests** (`e2e -- --ignored`) — spawn the release binary, send JSON-RPC over stdin, verify responses. Requires:
   - `agy` in `PATH` (install from `google-antigravity/antigravity-cli` releases)
   - Auth via `GEMINI_API_KEY` env var or macOS Keychain (`~/.gemini/antigravity-cli/settings.json`)
   - `cargo build --release` must have been run first

## Environment variables

| Var | Effect |
|---|---|
| `AGY_EXTRA_ARGS` | Space-separated extra args passed to every `agy` invocation |
| `GEMINI_API_KEY` | API key for e2e tests and CI |
| `AGY_ACP_AUTO_ALLOW` | What may run without asking. Tool names plus the groups `reads`, `searches`, `none`. Default `ask_question` |
| `AGY_ACP_SENSITIVE_PATTERNS` | Extra comma-separated substrings marking a path as too sensitive to read without asking |
| `AGY_ACP_PERMISSION_TIMEOUT_SECS` | How long a permission request waits before denying. Default `540` |
| `AGY_ACP_PERMISSION_SOCKET` | Set by the adapter on the `agy` subprocess; tells the hook where to reach the bridge. Not for users |

## Quirks

- `rusqlite` uses `bundled` feature — no system SQLite dependency needed.
- SQLite reads use `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX` — single-threaded access assumed per conversation DB.
- State persistence uses write-to-tmp-then-rename pattern under an exclusive file lock (`fs2`).
- Streaming writes JSON-RPC notifications directly to stdout from a background polling thread (not through the main channel). Both the main loop and the poller write to stdout concurrently.
- `handle_session_load` returns a `Vec<String>` (multiple notifications + final response), not a single response like other methods.
- Conversation binding: on first prompt for a new session, the adapter snapshots conversation DB filenames, then diffs after `agy` exits to discover the new conversation ID. Refuses to bind if multiple new DBs appear simultaneously.
- `fetch_available_models()` runs `agy models` synchronously during `Adapter::new()`. If `agy` isn't installed, models list is empty (no error).
- `session/cancel` is a no-op — always returns `{}`.
- Both `session/set_model` and `session/setConfigOption` are accepted for model selection.

### Permission bridge (`--permission-prompts`)

All of these were established experimentally against agy 1.1.12 and are easy to get wrong:

- A `PreToolUse` hook can only **veto** while agy's own permission checks are active. `{"decision":"allow"}` and `permissionOverrides` both lose to the headless soft-deny — verified with wildcard, literal and symlink-resolved paths. This is why the bridge runs agy with `--dangerously-skip-permissions` and becomes the sole gate, and why every unresolvable case must deny.
- A hook response with **no `decision` field** (`{}`) makes agy wait on the tool call until print mode times out. Always answer with an explicit decision.
- Three timeouts stack around a pending request and the order matters: the bridge's wait must expire before the hook's `timeout`, which must expire before agy's `--print-timeout`. Only the innermost yields a clean deny the model can continue from; if an outer one fires first, agy aborts the whole turn. Print mode defaults to 5m, so the adapter raises it when prompts are on.
- agy treats **every `--add-dir` as a workspace root**, so the hook directory is visible to the model, which will try to work in it after a refusal. Tool calls naming that directory are refused without prompting.
- Hooks are discovered in `.agents/hooks.json` under any workspace root, including secondary `--add-dir` ones. That is what keeps the hook out of the user's repo and global config.
- `{"decision":"ask"}` is a safe passthrough — it defers to agy's normal handling rather than forcing a prompt or a deny.
