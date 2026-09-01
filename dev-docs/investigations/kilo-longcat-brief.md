# Investigation Brief: Permission Decisions Ignore Command Content

**Date:** 2026-08-30
**Author:** kilo (longcat-2.0)
**Scope:** `src/permission.rs` sticky-answer keying + Paseo integration context
**Source of truth:** `TODO.md` → *Permission decisions ignore what a command actually does*

---

> **Superseded on one point.** This brief is kept as the investigation record, not
> as the specification. Its R1 keys the sticky answer by the `CommandLine` string
> alone; [plans/completed/permission-command-keying.md](../../plans/completed/permission-command-keying.md)
> supersedes that with a fingerprint of the *whole* argument object, because
> keying on `CommandLine` alone drops security-relevant siblings such as `Cwd`
> from the scope of what the user approved. Implement the plan, not this. The
> rest of the analysis here stands, and the plan is built on it.

## 1. Problem Statement

The permission bridge's "Always allow" / "Always reject" feature remembers decisions
by `(session_id, tool_name)` only. For command-executing tools (`run_command`,
`command_status`) this is a **silent arbitrary-command bypass**: one "Always allow"
on `ls` approves `rm -rf /` on the next turn, with no prompt and no effective
containment check.

Two compounding factors make this worse than a simple over-broad key:

1. **Containment is inert for command strings.** `escapes_containment()` (line 303)
   reads arguments *as paths*. A shell command arrives as a single `CommandLine`
   string, so `cat /etc/shadow` is never recognised as naming `/etc/shadow`. The
   only thing that catches `cat /etc/passwd` is the `passwd` sensitive substring —
   luck, not design.

2. **No revocation, no expiry.** `BridgeState.always` only grows. There is no
   session-end hook in ACP, so entries live for the process lifetime. A session
   reloaded in the same process inherits its old "Always" answers.

Both halves are **intentionally pinned by tests** that assert today's broken
behavior. Closing the gap turns those tests red — which TODO.md says is the
intended signal that the README must be updated alongside the fix.

### What is NOT affected

Path-argument tools (`view_file`, `write_to_file`, `list_dir`, etc.) are
constrained correctly: containment and the sensitive-path list *do* apply to a
remembered allow for them, so tool-level keying is defensible there. The bug is
specific to tools whose arguments are opaque strings.

---

## 2. Paseo Context

### 2.1 How Paseo drives the adapter

Paseo is a remote daemon that manages coding agents, workspaces, terminals, and
schedules. It is controlled over MCP tools or a CLI. For this adapter, the
relevant integration points are:

- **Provider config** (`~/.paseo/config.json` → `agents.providers.agy`):
  ```json
  {
    "extends": "acp",
    "label": "agy (Gemini via antigravity)",
    "command": ["agy-acp", "--permission-prompts"],
    "env": {}
  }
  ```
  Paseo spawns the adapter as a child process, speaking JSON-RPC over stdio.
  `--permission-prompts` is always on for the agy provider — the bridge is not
  opt-out at the Paseo level.

- **Sole gate.** When the bridge is on, agy is spawned with
  `--dangerously-skip-permissions` (`adapter.rs:839`). A `PreToolUse` hook can
  only *veto* while agy's own checks are active, so the adapter disables them and
  becomes the **only** gate on tool execution. Anything the bridge cannot resolve
  (no host, host disconnected, timeout) is denied. This means a too-broad
  "Always allow" is not a soft failure — it is a silent allow of arbitrary
  commands.

- **Permission UI.** Paseo surfaces `session/request_permission` to the user.
  The adapter's `permission_options()` (line 468) defines the four buttons:
  **Allow once**, **Always allow `<tool>` this session**, **Reject**, **Always
  reject `<tool>` this session**. The "always" labels name the tool because that
  is what the answer covers. Whatever keying the bridge uses is what every Paseo
  user experiences — there is no per-command granularity visible in the UI today.

- **Host neutrality.** The adapter is a generic ACP stdio server. Paseo is one
  host; Zed is another. The fix must not be Paseo-specific. This is already the
  project's stated principle and it costs nothing here.

### 2.2 What this means for the bug

Because the bridge is the sole gate under Paseo and the "Always" answer is keyed
by tool name, a Paseo user who clicks "Always allow run_command" on a benign `ls`
has silently authorized every subsequent command in that session — including
anything the model chooses to run. This is not a theoretical edge case; it is the
default Paseo experience.

### 2.3 Paseo agent lifecycle (relevant to R4)

Paseo owns agent lifecycle: sessions map to agy conversations, and the adapter
keeps an in-memory session map capped at 64 (`Adapter::evict_if_needed`,
`adapter.rs:396`). Sessions are evicted least-recently-used. This is the natural
place to hang "Always" answer cleanup — see R4.

---

## 3. Recommendations

### R1 — Key the sticky answer by command string for command tools ⭐ PRIMARY

**Highest value, lowest risk. This alone closes the headline bug.**

Change `AlwaysKey` from `(String, String)` to `(String, String, Option<String>)`
where the third element is the `CommandLine` for command tools and `None` for
path-argument tools. Two entries are equal only if all three fields match.

- This is an **equivalence test, not parsing.** Exact `CommandLine` string
  equality. `permission.rs:848` already extracts `CommandLine` for the title.
  No tokenization, no shell semantics.
- **No normalization.** TODO.md is explicit: over-normalizing is the bug class
  we are fixing (keying on the tool name is the degenerate case of normalizing
  everything away). Whitespace normalization is an optional future layer, not a
  v1 feature.
- **Effect:** approving `cat README.md` no longer approves `rm -rf build`. The
  two pinned tests flip — expected. Update the README *What "Always" remembers*
  section and rewrite those tests to assert per-command keying.
- **Scope:** only `run_command` and `command_status` get the command in the key.
  Path-argument tools stay keyed by `(session, tool)` — their containment checks
  are meaningful, so tool-level keying is already correct for them.

**Implementation sketch:**
```rust
// Before:
type AlwaysKey = (String, String);
let always_key = (session_id.clone(), tool_name.clone());

// After:
type AlwaysKey = (String, String, Option<String>); // (session, tool, command)
let command = match tool_name.as_str() {
    "run_command" | "command_status" => {
        args.get("CommandLine").and_then(|v| v.as_str()).map(String::from)
    }
    _ => None,
};
let always_key = (session_id.clone(), tool_name.clone(), command);
```

### R2 — Extract paths from command lines (secondary, best-effort)

Tokenize `CommandLine` and treat `/`-, `~`-, `../`-bearing tokens as paths, then
feed them through `outside_workspace` / `is_sensitive`. Must **fail toward
prompting** when a path cannot be confidently extracted.

- Incomplete by nature: `cat .en"v"`, `cat $HOME/.env`, `$(...)`, pipes,
  subcommands all defeat it. Worth doing as *depth* behind R1, never as the
  boundary.
- This is the parsing problem R1 deliberately avoids. Implement R1 first.

### R3 — Destructive-command denylist (cheapest, weakest)

Widen `SENSITIVE_PATTERNS` and reject obviously destructive patterns (`rm -rf`,
`dd`, `mkfs`, `curl | sh`) for command tools.

- A denylist over a shell-reinterpreted string is evaded trivially. Keep as
  defence-in-depth only, behind R1/R2.

### R4 — Bound "Always" answers to session lifetime (cheap, alongside R1)

Today `BridgeState.always` grows forever. Have `Adapter::evict_if_needed`
(`adapter.rs:396`) also tell the bridge to drop that session's entries.

- Gives "this session" a defensible meaning: answers last exactly as long as the
  session is live in memory. A session restored from `sessions.json` afterwards
  prompts again — the safe direction.
- Each entry is three short strings, bounded by 64 sessions. This is tidiness,
  not a leak fix. Implement only if already touching the bridge.
- Requires the bridge to expose a `forget_session(session_id)` method and the
  adapter to call it from `evict_if_needed`.

### R5 — Revocation / per-command "Always" (later, UI)

Once R1 exists, consider exposing the remembered set or expiring answers with the
turn. At minimum, users need a way to clear a stuck "Always". This is a host UI
change (Paseo/Zed) beyond `permission.rs` and should follow R1/R4.

---

## 4. Suggested Implementation Order

| Step | Change | Tests affected |
|------|--------|----------------|
| 1 | R1: extend `AlwaysKey` with `CommandLine` for command tools | `always_allow_is_remembered_per_tool_not_per_command` → rewrite to assert per-command; `a_path_inside_a_command_string_is_invisible_to_the_containment_check` → rewrite to assert command-level keying |
| 2 | R1: update README *What "Always" remembers* and the warning | — |
| 3 | R4: `forget_session` hook from `evict_if_needed` | new test: evicting a session clears its "Always" answers |
| 4 | R2 (later): command-line path extraction, fail-closed | new tests for extraction edge cases |
| 5 | R3 (later): denylist hardening | new tests for destructive patterns |
| 6 | R5 (later): revocation UI | host-side |

---

## 5. Verification Plan

1. **Unit tests:** `cargo test permission` — the two pinned tests must be rewritten
   to assert:
   - "Always allow" on `run_command "ls"` does NOT auto-allow `run_command "rm -rf build"`.
   - "Always allow" on `run_command "ls"` DOES auto-allow `run_command "ls"` again.
   - Path-argument tools still honor tool-level "Always allow".
2. **E2e under Paseo:** drive one real agent through a permission prompt with
   "Always allow" on a command, then issue a different command — verify the
   second command prompts. (TODO.md *Verify the port under Paseo* — still
   outstanding; this is the best end-to-end check.)
3. **Regression:** the four scripted permission scenarios from the Paseo
   verification doc.

---

## 6. Open Questions

| Question | Recommendation |
|----------|---------------|
| Exact-string equality vs. any normalization for R1? | **Exact match.** No normalization. |
| Keep "Always allow" per-command (R1) or remove it entirely for command tools? | **R1** preserves UX. Removing is safer but a bigger UX change. |
| Should R4 be in the same PR as R1? | Yes — same area, low risk, and it prevents the "Always" map from growing unbounded. |
