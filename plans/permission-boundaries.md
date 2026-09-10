# Permission-boundary follow-ups

## Outcome

Strengthen permission-state lifecycle and local IPC boundaries without widening
what a remembered or imported approval can authorize.

## Work

1. Decide whether remembered answers need expiry, inspection, or explicit
   revocation, and implement the chosen session-scoped model.
2. Decide whether an opt-in may consume native agy grants. Any design retains
   exact command keying, workspace containment, and sensitive-path checks.
3. Measure the socket threat model; harden its directory, name, framing, and
   connection limits if justified.
4. Replace the predictable hook-root creation path with exclusive random private
   creation, and define cleanup behavior after signals.

## Acceptance

- The chosen authorization semantics are documented and tested fail-closed.
- IPC/resource limits have adversarial and normal-path tests.
- No imported or remembered answer can bypass containment or sensitive-path
  policy.

## Evidence

Detailed findings and alternatives are in
[`dev-docs/research/investigation-notes.md`](../dev-docs/research/investigation-notes.md#security-and-permission-boundaries).
