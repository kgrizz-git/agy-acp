# Verify the port under Paseo

## Outcome

Prove the adapter's reopened-thread and concurrent-session behavior through
Paseo, rather than inferring it from an in-session continuation.

## Work

1. Tee adapter stdio during a reopen and record whether Paseo sends
   `session/load` or `session/resume`.
2. If it loads, compare replayed updates with the conversation database; if it
   resumes, verify the host receives the expected continuity without replay.
3. Run two sessions concurrently and verify isolation of conversation binding,
   cancellation, and permission prompts.

## Acceptance

- The observed ACP method and replay behavior are recorded in the relevant test
  or reference document.
- A regression test covers any adapter behavior exposed by the experiment.

## Evidence

The prior end-to-end results and their limits are preserved in
[`dev-docs/research/investigation-notes.md`](../dev-docs/research/investigation-notes.md#paseo-verification-notes).
