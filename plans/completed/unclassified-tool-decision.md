# Decide what `schedule` and `invoke_subagent` get

## Status

The blocking observation is done. Capture on **agy 1.1.25, 2026-09-03**, using
the dumping `PreToolUse` hook from `dev-docs/agy-tool-surface.md`, answered Q3
and falsified the premise behind Q1. What is left is a decision, three
evidence-backed edits, and one new gap that has to be recorded separately.

One methodology note, corrected from an earlier draft of this plan: the capture
runs without `--dangerously-skip-permissions` only because the `PreToolUse` hook
fires and records the payload *before* agy decides whether to run the tool. That
enumerates names and arguments cheaply, but it does not show execution — AGENTS.md
records that without the flag a hook `allow` loses to agy's headless soft-deny, so
a captured payload is not proof the tool ran. Everything below uses the captures
only for tool names, arguments and call ordering, which the hook sees regardless.

## What the bridge assumes

1. **A permission is scoped to a turn.** Pending requests are drained when the
   turn ends; a late answer cannot become sticky.
2. **Every tool call reaches the bridge.** Containment, the sensitive-path list,
   and sticky scoping are all applied at one chokepoint.

Both survive. Neither survives for the reason the earlier draft assumed.

## Q3 — do a subagent's tool calls reach this adapter's hook? **Yes.**

Driving `define_subagent` then `invoke_subagent` produced four hook payloads, and
the subagent's own `write_to_file` was one of them:

```
e995df2e step 2  define_subagent   {name: scribe, enable_write_tools: true, ...}
e995df2e step 4  invoke_subagent   {Subagents: [{TypeName, Role, Prompt}]}
e995df2e step 6  manage_subagents  {Action: list}
b414bc71 step 2  write_to_file     {TargetFile: /tmp/.../scribe.txt, ...}   <- the subagent
```

Three things follow, and the second is the finding worth carrying forward.

**The chokepoint is one chokepoint.** A subagent is gated by the same hook, so
assumption (2) holds and no classification of `invoke_subagent` has to compensate
for anything.

**The subagent runs under its own `conversationId`, which the bridge has never
seen.** `decide` resolves it through `state.conversations`, misses, and falls
back to `active_session`. That lands on the right session today only because the
adapter spawns one `agy` per turn and serializes prompts, so the only active
session is the parent's. It is correct by circumstance, not by construction, and
nothing in the code says so. A subagent call arriving in the gap after teardown
is denied because `active_session` is `None` by then — the "no ACP session to
ask" path, not the turn-generation check; a call arriving once the *next* turn has
set its own active session is what the generation check catches. Both deny, which
is the safe direction.

**Containment still applies to the subagent, and not via the payload.** A
subagent given `Workspace: /tmp/agycap/outside` still reported the *parent's*
`workspacePaths` in its payload. The bridge does not read that field — it checks
argument paths against its own `workspace_roots` — so the outside `TargetFile`
would have prompted. Had the check trusted `workspacePaths`, it would have been
wrong here.

## Q1 — is `"other"` plus argument-keyed sticky sufficient for `schedule`? **Yes,
and the stated reason for doubting it was wrong.**

Every `schedule` call observed ran in-turn: it parked the turn and ran the work
as further steps of the same conversation, rather than deferring past it.
Deferral past the turn was never seen, but only a couple of parameter shapes were
tried, so this is "not observed to defer", not "cannot defer" for every interval
and duration:

```
1788490605 e1e54c9a step 2  schedule    {DurationSeconds: 45, Prompt: "Run ... touch fired.txt"}
1788490656 e1e54c9a step 6  run_command {CommandLine: "touch fired.txt", Cwd: ...}
```

Fifty-one seconds later, same `conversationId`, same process, same turn, and the
scheduled work arrived at the hook as an ordinary `run_command`. A cron behaved
the same way: `CronExpression "*/1 * * * *"` with `MaxIterations 5` fired in
process and held the turn open for about five minutes.

So assumption (1) is not violated by construction, and the approval the user gave
is not silently carried into a later session. Two real consequences replace the
imagined one.

*A `schedule` call is a turn-duration decision.* `MaxIterations` times the cron
interval, or `DurationSeconds`, is how long the turn stays open, bounded by
`PERMISSION_PRINT_TIMEOUT` — 60 minutes in `adapter.rs`. Approving one is
approving a wait, and the prompt does not say so.

*It is ordinary, not exotic.* Asked to wait for a subagent, the model called
`schedule` with `DurationSeconds: 600`, `TimerCondition: "any"` and
`Prompt: "Wait for subagent"`. `schedule` is how this model waits, so any
treatment that makes it noisy will be felt constantly.

## Q2 — is `"other"` sufficient for `invoke_subagent`? **Yes.**

Q3 closed it. The exposure the earlier draft feared — approving once opens a
channel the bridge never sees again — does not exist. `invoke_subagent` and
`define_subagent` keep the fingerprint key on their own merits: `define_subagent`
is a capability grant (`enable_write_tools`, `enable_mcp_tools`,
`enable_subagent_tools` were all in the observed payload), and one "Always allow"
on a read-only definition must not cover a later one that enables writes.

## The new gap: a subagent's call is indistinguishable in the prompt

The permission prompt shows tool name, arguments and kind. Nothing in it says the
call came from a spawned agent rather than the one the user is talking to. Worse
in the sticky direction: any tool in `KEYED_BY_TOOL_KINDS` — `write_to_file`
(`edit`), but equally `view_file` (`read`) and `grep_search` (`search`) — is keyed
by tool name alone, so an "Always allow" the user gave for the parent's call
silently covers the subagent's calls of the same tool too.

That is inside the contract `sticky_scope` documents — containment and the
sensitive-path list still constrain those calls, which is exactly what earns
tool-level keying — so it is not a hole today. Two things hold it there, and both
are worth stating. The sticky key is `(session_id, tool_name, None)`, so the
*only* thing separating parent from subagent is that they get different session
ids; if a future change ever gave a subagent the parent's session id, the
tool-level key would merge them silently. And it is an honesty problem regardless,
the same shape as the wording gap under Q1: the prompt should say what is being
approved and that it came from a subagent.

## The list is not closed, and never will be

Two things found while capturing say the seventeen-name list is a snapshot of one
configuration rather than an inventory.

The binary carries an `exa.cortex_pb.CascadeToolConfig` — a per-tool enable map
whose fields name about thirty-five tools, well beyond the seventeen this
configuration exposes: `view_code_item`, `command_status`, `code_search`,
`internal_search`, `knowledge_base_search`, `notebook_edit`, `browser_subagent`,
`antigravity_browser`, `memory`, `skill_search`, `mquery`, `workspace_api`,
`ask_permission`. Two of the five names this fork removed — `view_code_item` and
`command_status` — are fields in that map, so they are switchable, not absent.
The other three are known to the binary in a different way: `codebase_search` and
`edit_file` appear in the baked-in few-shot prompt examples, complete with
argument shapes (`TargetDirectories`; `TargetFile`, `CodeEdit`,
`CodeMarkdownLanguage`, `Blocking`), and `propose_code` is a Cortex step type
(`CortexStepProposeCode`) behind a `use_replace_content_propose_code` flag.

That is a weaker claim than "all five are switchable" and it still lands in the
same place. The removal from `tool_kind` stands, for the reason already recorded.
What has to change is the doc's account of *why* they were never seen: two are
disabled tools, two are vocabulary the model is shown by example, and one is a
step type. None of that is "upstream vocabulary this fork inherited".

And `agy mcp add` exists. Under an MCP server the tool names and argument names
are whatever a third party chose, so `PATH_FIELDS` can never be complete by
enumeration. The `"other"` fallthrough is not a gap to be closed one name at a
time; it is the contract, and the doc should say that plainly.

## Steps

1. Add `ImagePaths` and `Workspace` to `PATH_FIELDS`. Both are now observed
   rather than guessed: `generate_image` was captured with
   `ImagePaths: ["/tmp/agycap/img/seed.txt"]`, and `invoke_subagent` with
   `Subagents[].Workspace: "/tmp/agycap/outside"`. `path_field_args` already
   recurses, so the nested one is reached. The documented risk of over-inclusion
   is small here: Antigravity also accepts `inherit`, `branch` and `share` for
   `Workspace`, and a bare word resolves inside the workspace and does not
   prompt.
2. Record the Q1/Q2 verdict in `tool_kind`'s doc comment: both stay `"other"`,
   and say why — for `schedule`, because it runs in-turn and the fingerprint
   keeps a wait of one duration from covering a wait of another; for
   `invoke_subagent` and `define_subagent`, because the arguments carry a
   capability grant.
3. Pin the subagent conversation-id *fallback* path with a test — the one no
   existing test covers. `unknown_conversations_are_denied` and
   `only_the_users_own_refusal_counts_as_a_refusal` both exercise the no-active-
   session deny. The uncovered case is an unknown `conversationId` *with* an active
   session set: it must resolve to that session, and then be denied by the
   turn-generation check once the asking turn has ended. That behaviour exists;
   nothing states subagents are why it matters, and nothing tests the fallback
   arm specifically.
4. Fix the two wording gaps in the prompt text, together. A `schedule` call
   should say it holds the turn open, and a call whose `conversationId` is not
   the session's registered one should say it came from a subagent.
5. Open a TODO entry for step 4 if it grows past a wording change. It is a
   presentation defect, not a containment one, and it must not be smuggled in as
   part of a classification decision.
6. Fold the newly observed payloads into `dev-docs/agy-tool-surface.md`, correct
   the "upstream vocabulary" claim, and state the MCP argument for why the list
   cannot be closed.
7. Correct PR #16's body as previously noted, then delete both TODO entries —
   this one and "Characterize agy's full tool surface", which this capture
   answers as far as observation can answer it.

## Not in scope

Special-casing either tool in `KEYED_BY_TOOL_KINDS`, adding a never-sticky list,
or scoping sticky answers by `conversationId` so a parent's answer cannot cover a
subagent. The last is now a coherent proposal rather than a vague worry, but it
changes the sticky key for every tool, and the evidence says the current key is
inside its documented contract. It belongs in its own entry if anyone wants it.
