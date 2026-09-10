# Fork maintenance decisions

## Outcome

Make the fork independently identifiable and keep maintenance tooling justified
by a clear benefit.

## Work

1. Choose a distinct crate and binary name; define state-directory migration,
   hook invocation, provider configuration, and documentation changes.
2. Decide whether CI-based SonarCloud supplies findings not already covered by
   Clippy and llvm-cov before adding another required analysis system.
3. Assess adapter-owned transcript persistence as a replacement for replaying
   agy's undocumented conversation database, including migration/fallback needs.

## Acceptance

- A rename has an explicit compatibility/migration plan before implementation.
- Any new analysis gate has a non-duplicative purpose and reliable CI setup.
- A replay redesign accounts for history the adapter did not observe.

## Evidence

Background research is preserved in
[`dev-docs/research/investigation-notes.md`](../dev-docs/research/investigation-notes.md#fork-maintenance).
