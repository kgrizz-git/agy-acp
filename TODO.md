# To do

This is a work board, not a design journal. Each entry states an outcome and the
next concrete step; supporting evidence belongs in a linked plan or reference
document. Delete an entry when it lands, rather than checking it off.

## Next Up

- [Harden the permission socket and hook-root creation/cleanup](#security-and-permission-boundaries).
- [Verify Paseo's reopened-thread path](#paseo-integration).
- [Detect and surface workspace hook directories](#security-and-permission-boundaries).
- [Carry a turn identity through the hook protocol](#reliability-and-lifecycle).
- [Choose a distinct crate and binary name](#fork-maintenance).

## Active

### Paseo integration

- Verify the ACP method and replayed updates used after reopening a thread; then
  exercise concurrent sessions. Plan: [plans/paseo-port-verification.md](plans/paseo-port-verification.md).
- Label subagent-origin permission requests without weakening their containment
  or sticky-answer scope. Plan: [plans/ecosystem-followups.md](plans/ecosystem-followups.md).
- Establish the supported representation for native agy child agents, then
  validate ordering, lifecycle, logs, and cancellation. Plan: [plans/ecosystem-followups.md](plans/ecosystem-followups.md).
- Reproduce Paseo task-state and whole-file-revert reports before adopting any
  community-fork fix. Plan: [plans/ecosystem-followups.md](plans/ecosystem-followups.md).

### Security and permission boundaries

- Decide and implement the lifetime, inspection, and revocation model for
  remembered permission answers. Plan: [plans/permission-boundaries.md](plans/permission-boundaries.md).
- Decide whether an opt-in may use native agy permission grants, while retaining
  exact command keying, containment, and sensitive-path checks. Plan: [plans/permission-boundaries.md](plans/permission-boundaries.md).
- Detect and surface workspace hook directories before the first turn; pursue an
  upstream isolation option separately. Plan: [plans/workspace-hook-trust-boundary.md](plans/workspace-hook-trust-boundary.md).
- Assess and harden the permission socket and hook-root creation/cleanup paths.
  Plan: [plans/permission-ipc-hardening.md](plans/permission-ipc-hardening.md).

### Reliability and lifecycle

- Decide whether global prompt serialization remains intentional; make any
  concurrency change safe for permission routing. Plan: [plans/reliability-lifecycle.md](plans/reliability-lifecycle.md).
- Carry a turn identity through the hook protocol to reject stale same-session
  requests. Plan: [plans/reliability-lifecycle.md](plans/reliability-lifecycle.md).
- Bound IPC frames, pending requests, and output buffering without introducing
  deadlock. Plan: [plans/reliability-lifecycle.md](plans/reliability-lifecycle.md).
- Verify denial/result ordering and provider-error presentation. Plan: [plans/reliability-lifecycle.md](plans/reliability-lifecycle.md).

### Fork maintenance

- Rename the crate and binary, including migration and provider configuration.
  Plan: [plans/fork-maintenance.md](plans/fork-maintenance.md).
- Decide whether CI-based SonarCloud supplies value beyond Clippy and llvm-cov.
  Plan: [plans/fork-maintenance.md](plans/fork-maintenance.md).
- Assess replacing private-schema conversation replay with adapter-owned history.
  Plan: [plans/fork-maintenance.md](plans/fork-maintenance.md).
- Evaluate ACP configuration, per-session workspace roots, and upstream changes
  only with an explicit compatibility plan. Plan: [plans/ecosystem-followups.md](plans/ecosystem-followups.md).

## Icebox

- Revisit a PTY fallback only if current agy versions reproduce the original
  non-TTY or thinking-model failures. Reference: [investigation notes](dev-docs/research/investigation-notes.md#pty-fallback).
- Investigate a Paseo context bridge only if required host context is unavailable
  to agy. Reference: [investigation notes](dev-docs/research/investigation-notes.md#daemon-context-bridge).
- Do not adopt the rejected community-fork designs without a new threat-model
  decision. Reference: [deferred record](plans/deferred/community-and-command-parsing.md).
