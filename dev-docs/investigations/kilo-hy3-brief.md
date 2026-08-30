# Brief: "Permission decisions ignore what a command actually does" + Paseo context

**Date:** 2026-08-29
**Scope:** `src/permission.rs` "Always" sticky-answer logic and how Paseo drives
the adapter. Source of truth: `TODO.md` → Security and permission boundaries →
*Permission decisions ignore what a command actually does*, plus the README
section *What "Always" remembers*.

---

## 1. The problem, restated

When the user answers **Always allow** (or **Always reject**) to a
`session/request_permission` prompt, the adapter records the decision in
`BridgeState.always`, keyed only by `(session_id, tool_name)`
(`permission.rs:75`, `permission.rs:286`, `permission.rs:441`).

Consequences:

1. **One "Always allow" on `run_command` covers every later command.** Approving
   `ls` once lets `rm -rf build` run unprompted (TODO.md, `permission.rs:1816`
   test `always_allow_is_remembered_per_tool_not_per_command`).
2. **The containment and sensitive-path checks are inert for command tools.**
   On a remembered allow the bridge re-checks `escapes_containment`
   (`permission.rs:303`), but that function reads arguments *as paths*. A shell
   command is one opaque `CommandLine` string, so `cat /etc/shadow` is never
   recognised as naming `/etc/shadow`. It only survives because
   `cat /etc/passwd` trips the `passwd` sensitive substring by luck
   (`permission.rs:1841` test
   `a_path_inside_a_command_string_is_invisible_to_the_containment_check`).
3. **No revocation and no expiry.** An "Always" answer cannot be undone within a
   session (`BridgeState.always` only ever grows; only `pending` is cleaned up,
   TODO.md lines ~108-125).

Both gaps (1) and (2) are **intentionally pinned by tests** that assert today's
behaviour. Closing the gap turns those tests red, which TODO.md says is the
intended signal that a doc change is also required.

Path-argument tools (`view_file`, `write_to_file`, etc.) are *not* affected the
same way: there the containment and sensitive-path checks do constrain a
remembered allow, so tool-level keying is defensible for them.

## 2. Paseo — how it relates

Paseo is a remote daemon that manages coding agents, workspaces, terminals and
schedules, controlled over MCP tools or a CLI (`paseo` skill). For this adapter,
the relevant facts:

- Paseo runs the adapter as
  `["agy-acp", "--permission-prompts"]` in `~/.paseo/config.json`
  (AGENTS.md, Local gotchas). So the bridge is **on** for Paseo by default.
- The adapter is a **host-neutral** ACP stdio server. Paseo is just one ACP host
  that answers `session/request_permission`; the design must stay usable from
  Zed and any other ACP client (AGENTS.md, *Keep it host-neutral*).
- Paseo surfaces those permission requests through its own UI; the adapter's
  `permission_options` (`permission.rs:468`) are the four buttons the user gets
  (**Allow once / Always allow / Reject / Always reject**). So whatever keying
  the bridge uses is what every Paseo user experiences.
- Paseo owns agent lifecycle, cancellation, concurrent sessions and subagents.
  The TODO already flags that cancellation, concurrent sessions and subagent
  events are untested under real Paseo (TODO.md, *Verify the port under Paseo*).
  This brief covers only the permission-keying gap; lifecycle verification is a
  separate, larger item.

Practical implication: because the bridge is the *sole* gate under Paseo
(`--dangerously-skip-permissions` is passed to agy when the bridge is on,
`adapter.rs:839`), a too-broad "Always allow" on `run_command` is a real
silent-allow of arbitrary commands in a Paseo session. This is not a Zed-only
edge case.

## 3. Recommendations (in order of value)

### R1 — Key the sticky answer by command string for command-executing tools
**Highest value, lowest risk. Recommended primary fix.**

Change the `AlwaysKey` from `(session, tool)` to `(session, tool, command_string)`
*only* for command tools (`run_command`, `command_status`), leaving path-argument
tools keyed by `(session, tool)` as today.

- This is an **equivalence test**, not parsing. Minimum version is exact `CommandLine`
  string equality (`permission.rs:848` already extracts `CommandLine`). No
  tokenization, no shell semantics. TODO.md explicitly calls keying-on-the-tool-name
  "the degenerate case of normalizing everything away."
- Over-normalizing is itself the bug class we are fixing (merges distinct
  commands). Stay at exact-match; treat any whitespace/shell normalization as an
  optional layer with the explicit rule: *the sticky key should be as specific as
  the checks that still apply to a remembered allow.* For command tools those
  checks are inert, so the command string must be in the key.
- Effect: approving `cat README.md` no longer approves `rm -rf build`. The two
  pinned tests flip because they relied on tool-level keying — expected; update
  the README warning and delete/rewrite those tests as part of the change.

### R2 — Extract paths from a command line so containment can apply (secondary)
**Harder, best-effort only. Do not depend on it as a boundary.**

Tokenize `CommandLine` and treat `/`-, `~`-, `../`-bearing tokens as paths, then
feed them through the existing `outside_workspace` / `is_sensitive` checks
(`permission.rs:620`, `permission.rs:598`). Must **fail toward prompting** when a
path cannot be confidently extracted.

- Incomplete by nature: `cat .en"v"`, `cat $HOME/.env`, `$(...)`, subcommands and
  pipes defeat it. Worth doing as *depth* behind R1, never as the main defence.
- This is the parsing problem R1 deliberately avoids — implement R1 first, R2
  later if coverage warrants it.

### R3 — Destructive-command denylist + widen sensitive substrings (cheapest, weakest)
Widen `SENSITIVE_PATTERNS` (`permission.rs:512`) and reject obviously destructive
patterns (`rm -rf`, `dd`, `mkfs`, `curl | sh`) outright for command tools.

- Cheapest to ship, but a denylist over a shell-reinterpreted string is evaded
  trivially (`cat .en"v"`). Keep as defence-in-depth only, behind R1/R2.

## 4. Two smaller fixes to land alongside R1

### R4 — Bound "Always" answers to session lifetime (cheap cleanup)
Today `BridgeState.always` grows forever and a reloaded session in the same
process inherits old answers (TODO.md lines ~105-125). The in-memory session map
already evicts least-recently-used sessions via `Adapter::evict_if_needed`
(`adapter.rs:396`), capped at 64. Have that eviction also tell the bridge to drop
that session's `(session, _)` entries.

- Gives "this session" a defensible meaning: answers last exactly as long as the
  session is live in memory. A session restored from `sessions.json` afterwards
  prompts again — the safe direction.
- Each entry is two short strings, so this is tidiness, not a leak fix; implement
  only if already touching the bridge.

### R5 — Offer revocation / make "Always" explicit per command
Once R1 exists, consider exposing the remembered set or expiring answers with the
turn. At minimum, Paseo/Zed users need a way to clear a stuck "Always". This is a
UI/option change beyond `permission.rs` and should follow R1/R4.

## 5. Suggested implementation order

1. **R1** — extend `AlwaysKey` for command tools with exact `CommandLine`
   equality; flip the two pinned tests; update README *What "Always" remembers*
   and the warning. This alone closes the headline bug.
2. **R4** — hook `evict_if_needed` → bridge cleanup (cheap, safe).
3. **R2** (later) — command-line path extraction, fail-closed.
4. **R3** (later) — denylist hardening, never the boundary.
5. **R5** (later) — revocation UI.

## 6. Verification notes

- The two pinned tests (`always_allow_is_remembered_per_tool_not_per_command`,
  `a_path_inside_a_command_string_is_invisible_to_the_containment_check`) are
  documented as *intended to go red* when the gap is closed. Replace them with
  tests asserting: an "Always allow" on `run_command "ls"` does **not** auto-allow
  `run_command "rm -rf build"`; and that the same command string *is* remembered.
- Re-run the permission unit tests (`cargo test permission`) and the four
  scripted permission scenarios from *Verify the port under Paseo*.
- Because the bridge is on under Paseo, the e2e path "one real agent through a
  permission prompt" is the best end-to-end check that R1 behaves in a real host
  (TODO.md, *Verify the port under Paseo* — still outstanding).

## 7. Open questions for the user

- Accept exact-string equality for R1 (recommended, simplest) vs. attempting any
  normalization? Default: exact match, no normalization.
- Should "Always allow" be **removed entirely** as an option for command tools
  (TODO.md option 1, strongest), or kept but keyed per command (R1)? Removing is
  safer; R1 preserves the UX. Recommend R1 unless you'd rather drop the option.
