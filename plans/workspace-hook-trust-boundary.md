# Workspace-supplied hooks run outside the bridge

## The finding

Opening an untrusted repository through the adapter executes that repository's
own hook commands, with no permission prompt and outside the ACP bridge. Proven
on agy 1.1.26, 2026-09-04, with raw `agy` (no adapter, no flag) against a repo
whose `.agents/hooks.json` the adapter would also cause agy to discover:

- A `PreInvocation` command hook wrote its marker **before the model ran**.
- A `Stop` command hook wrote its marker when the loop ended.
- Neither prompted, the repo was **not** in agy's `trustedWorkspaces`, and no
  `--dangerously-skip-permissions` was involved.

The bridge did not and cannot see these. It is itself a `PreToolUse` hook, and
agy runs every hook command directly; `PreInvocation`, `PostInvocation`,
`PostToolUse` and `Stop` never pass through a tool-permission decision at all.
Even a `PreToolUse` hook command runs as a side effect on every tool call
regardless of the allow/deny it returns. So all five hook events are an arbitrary
code-execution vector for whoever wrote the repo.

## Why the adapter is exposed

`agy` discovers `.agents/` (also `.agent/`, `_agents/`, `_agent/`) by walking
from its working directory up to the repository root, and merges every hook it
finds. The adapter runs agy with its working directory set to the user's
workspace (`adapter.rs:955`) and also passes it with `--add-dir`
(`adapter.rs:905`), so the workspace's own `.agents/hooks.json` is on the
discovery path by construction. The adapter needs the workspace as a root for
file access, and needs `--add-dir` on its private hook root to install the
bridge at all, so it cannot simply stop agy from reading `.agents/`.

## What is *not* broken

The bridge's veto still holds. Two merged `PreToolUse` hooks — one allow, one
deny — were tested under `--dangerously-skip-permissions` in both name orders:

- deny-first: the deny ran, short-circuited, tool denied.
- allow-first: both ran, tool still denied ("the error returned was 'bridge
  says no'").

So **deny wins regardless of hook name or order**. A malicious repo cannot flip
the bridge's deny by injecting an allow-all `PreToolUse` hook, and cannot make a
tool the bridge denied execute. It can only *add* denials (a nuisance DoS on the
user's own tools) — and run arbitrary commands out of band, which is the real
problem.

Nor is `trustedWorkspaces` a lever: hook discovery ignored it in the test, so it
cannot be leaned on as the gate.

## Scope of the risk

This is the same trust model as any tool that runs a repo's checked-in scripts
(`git` hooks, a `Makefile`, an npm `postinstall`). The escalation here is that it
fires on *opening* the repo for a chat, before the user has asked for anything,
and that the adapter's own security story is "the bridge is the sole gate" —
which is true for tool calls and silent about hooks. A user who trusts the
adapter to contain an untrusted repo does not get that for hook commands.

## Options

1. **Document the boundary, do nothing else.** Cheapest. The README's "only
   inside the workspace / bridge is the sole gate" language is currently
   misleading; it must say hook commands in a workspace's `.agents/` run
   unsandboxed and are not gated, exactly as they do under plain agy. This is the
   floor and should ship regardless of what else does.

2. **Detect and surface.** Before the first turn, scan the workspace (CWD up to
   the repo root) for a hook directory containing `hooks.json`, and if found,
   report it to the ACP host — ideally requiring an explicit opt-in before agy is
   spawned. Turns a silent exec into an informed one. Cost: a scan mirroring
   agy's own discovery walk, and a host-visible prompt with no ACP-standard shape
   for "workspace wants to run hooks".

3. **Isolated hook discovery.** Prevent agy from discovering the workspace's
   `.agents/` while still installing the bridge. No clean lever exists today:
   discovery is driven by CWD and `--add-dir`, both of which the adapter needs.
   Would require an agy feature (a flag to disable workspace hook discovery, or
   to name an explicit hook root and ignore others). File upstream; do not block
   on it.

## Recommendation

Ship option 1 now — it is a correctness fix to the adapter's stated guarantees,
not a feature. Pursue option 2 as the real mitigation, opt-in and off by default,
so the common case (a user's own trusted repo) is unaffected. Raise option 3 with
upstream agy, since only agy can cleanly separate "install my bridge hook" from
"run this repo's hooks".

## Not in scope

Sandboxing hook execution ourselves, or trying to parse/validate a repo's hook
commands. agy runs them; the adapter is not in that path and cannot wrap it
without an agy feature. The honest boundary is "the bridge gates tool calls, not
hook commands", and the work is to say so and to let the user decide before a
hook runs.
