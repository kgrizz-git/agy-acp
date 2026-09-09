# Ecosystem and ACP follow-ups

## Outcome

Extend integration behavior only when agy and ACP/Paseo semantics can be proven
without inventing incorrect lifecycle or permission behavior.

## Work

1. Determine the supported Paseo representation for native agy child agents;
   validate lifecycle, ordering, logs, and cancellation before exposing it.
2. Add a clear prompt label for subagent-origin requests while preserving their
   existing permission scope.
3. Evaluate selected agy configuration, per-session workspace roots, and
   reported Paseo edge cases with compatibility tests.

## Acceptance

- Any exposed capability is version-gated where necessary and covered by an
  end-to-end fixture.
- Child agents are not represented as synthetic independent Paseo agents unless
  the host supports their lifecycle semantics.
- Configuration cannot weaken the bridge's fail-closed permission policy.

## Evidence

The prior investigations are preserved in
[`dev-docs/research/backlog-notes.md`](../dev-docs/research/backlog-notes.md#upstream-and-ecosystem).
