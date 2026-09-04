# Decide what `schedule` and `invoke_subagent` get

## Why this is a plan and not an edit

The TODO entry asked for two things. Deleting the five phantom names was
mechanical and is done in PR #16. The second half — "`schedule` and
`invoke_subagent` most deserve a deliberate call, since one defers work past the
current turn and the other spawns another agent" — is not mechanical, and PR #16
did not do it. It documented that both fall through to `"other"` and pinned that
with a test, which converts an omission into a recorded omission. That is not a
decision about whether `"other"` is *right*.

The distinction matters because the two tools break the bridge's two assumptions,
one each, and neither has ever been seen in a captured payload.

## What the bridge assumes

1. **A permission is scoped to a turn.** Pending requests are drained when the
   turn ends; a late answer cannot become sticky. This holds because the tool
   call and its effect happen inside the turn.
2. **Every tool call reaches the bridge.** Containment, the sensitive-path list,
   and sticky scoping are all applied at one chokepoint.

`schedule` violates (1) by construction: the call happens in the turn, the work
does not. `invoke_subagent` threatens (2): if a spawned agent's own tool calls do
not route back through this adapter, the chokepoint is not one.

## The three questions

**Q1. Is `"other"` plus argument-keyed sticky sufficient for `schedule`?**
`"other"` means always prompt, and the sticky key is the argument fingerprint, so
one "Always allow" covers only an identical later call. Argument *for*
sufficiency: an identical call is the same job, so re-arming it silently is
defensible. Argument *against*: the approval the user gave was for work that runs
later, possibly after the session that scoped the answer has moved on, and the
prompt text does not say so. This may be a wording problem rather than a scoping
one — decide which.

**Q2. Is `"other"` sufficient for `invoke_subagent`?** Depends entirely on Q3. If
subagent tool calls route through the bridge, `"other"` is fine and this is
closed. If they do not, no classification of `invoke_subagent` fixes anything,
because the exposure is not in how this call is keyed — it is that approving it
once opens a channel the bridge never sees again.

**Q3. Do a subagent's tool calls reach this adapter's hook?** Unknown, and the
only question here that observation can settle. This is a containment question,
not a tool-list one, and it does not belong in this entry.

## Steps

1. Answer Q3 by capture, using the method in `dev-docs/agy-tool-surface.md`:
   drive `agy` with the dumping `PreToolUse` hook and a prompt that forces a
   subagent, then check whether the subagent's tool calls appear in the dump.
   This also produces the first observed payloads for `invoke_subagent` and
   `define_subagent`, which is worth having regardless of the answer.
2. If they do not appear, stop and open a TODO entry for the containment gap.
   That is a larger finding than this entry and must not be buried inside it.
3. Answer Q1 and Q2 in light of step 1. Record the verdict where the code will
   be read — `tool_kind`'s doc comment for the classification, the prompt-wording
   code if Q1 turns out to be about wording — and in
   `dev-docs/agy-tool-surface.md`.
4. Only then delete the TODO entry.

## Not in scope

Special-casing either tool in `KEYED_BY_TOOL_KINDS` or adding a never-sticky
list. Both are plausible outcomes of step 3, not premises of it. Nothing here
should change behaviour before step 1 produces evidence, since every option is
currently being argued from a self-reported tool list that has already been wrong
once (`find_by_name.FullPath` was reported as a path and is a boolean).

## Honest status of PR #16

PR #16's body claims the seven-tool half is "now a recorded decision rather than
an omission". That oversells it and should be corrected to say the classification
is recorded and the deliberate call is deferred to this plan.
