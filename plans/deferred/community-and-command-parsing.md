# Deferred: community designs and command interpretation

## Why deferred

Exact command keying and the safe-command classifier already provide the current
permission boundary. General shell parsing, command denylists, and the rejected
community-fork designs are not required to close a known gap; each can create a
false sense of containment if treated as complete.

Reconsider only after a new threat-model decision establishes a concrete benefit
and a fail-closed implementation path.

## Evidence

The original alternatives and rationale are preserved in
[`dev-docs/research/investigation-notes.md`](../../dev-docs/research/investigation-notes.md#deliberately-not-taken-parsing-what-a-command-does)
and its community-fork notes.
