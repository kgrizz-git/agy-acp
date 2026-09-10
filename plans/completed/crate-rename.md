# Rename the crate and binary to `agy-gated-acp`

## Outcome

The installed artifact matches the repository: crate, binary, state directory,
hook invocation, and all docs say `agy-gated-acp`. Bundled with the README
refresh (Paseo usage, TODO-pointer fix, upstream credits) so the README is
written once against the final names. Lands as `0.3.0`: a renamed binary is a
materially changed delivery.

## Work

1. `Cargo.toml`: package (and binary) name → `agy-gated-acp`, version →
   `0.3.0`. No code changes beyond name-derived strings below.
2. State-dir migration (`src/adapter.rs:153`): new dir
   `~/.openab/agy-gated-acp`. On startup, if the new `sessions.json` is absent
   and the old `~/.openab/agy-acp/sessions.json` exists, move it across (with
   a stderr note); if both exist, keep the new one and leave the old alone.
   Add a unit test for move/absent/present cases using the existing scratch-dir
   pattern. Update the `types.rs:29` doc comment.
3. Hook invocation: `hook_root.rs:68` derives the command from the current exe
   path, so it follows the rename with no change; update the hardcoded
   `/usr/local/bin/agy-acp permission-hook` expectation in its test (`:103`).
4. Grep sweep for remaining `agy-acp`-as-binary references outside frozen
   history (`plans/completed/`, past CHANGELOG entries) and ignored scratch:
   `src/` doc comments, `scripts/` (e2e-local, probe-cancel), workflows,
   `AGENTS.md` key paths and Paseo command, `dev-docs/` current references.
   Historical prose keeps the old name as record.
5. README refresh, same PR:
   - New **Paseo usage** section (provider command
     `["agy-gated-acp", "--permission-prompts"]` in `~/.paseo/config.json`,
     daemon restart on command changes); the doc stays host-neutral, Zed keeps
     its section.
   - Fix the fork-notice pointer: assessment detail lives in AGENTS.md and the
     ecosystem plan, not TODO.md.
   - Upstream chain made explicit: Google Antigravity CLI (`agy`) as the
     underlying tool, `hicder/agy-acp` as the fork origin, both community
     projects as assessed — each already linked, stated as a chain.
   - Every install path, config snippet, and state-dir reference uses the new
     binary name.
6. CHANGELOG: `0.3.0` heading, one **Changed** bullet for the rename (binary,
   state dir with automatic sessions migration, docs), citing the PR.

## Acceptance

- Fresh `cargo build --release` produces `agy-gated-acp`; old binary name
  appears nowhere in `cargo run -- --help` output or the hook file the bridge
  writes (covered by the updated hook-root test).
- A pre-rename `sessions.json` is picked up once at the new location; a second
  start performs no move.
- `cargo fmt`, clippy (`-D clippy::all`), unit + ignored-I/O tiers green.
- No live stale `agy-acp`-as-artifact reference outside frozen history.

## Not in scope

Leaving the fork network, changing hosts' configs for the user (documented
manual steps only), and touching `~/.gemini` state. The `0.2.0` history keeps
the old name as record.
