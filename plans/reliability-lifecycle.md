# Reliability and lifecycle follow-ups

## Outcome

Improve concurrency and resource handling while preserving the permission
bridge's turn/session isolation.

## Work

1. Decide whether prompt serialization is intentional; if not, make permission
   state per-session before allowing concurrent turns.
2. Add an inherited turn stamp to hook payloads and reject stale same-session
   requests, with a compatible behavior for older hooks.
3. Establish practical IPC frame, queue, and output-buffer limits; test
   backpressure so a slow host cannot cause unbounded memory or deadlock.
4. Verify that a bridge denial remains authoritative over late provider updates
   and that provider failures reach the host clearly.

## Acceptance

- New concurrent or delayed-request tests prove session and turn isolation.
- Resource limits reject oversized/malformed input predictably.
- Provider and denial ordering has a regression test.

## Evidence

The architecture constraints and observed races are preserved in
[`dev-docs/research/backlog-notes.md`](../dev-docs/research/backlog-notes.md#reliability-and-lifecycle).
