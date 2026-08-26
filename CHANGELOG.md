# Changelog

Notable changes to this fork. Entries land here when work leaves
[TODO.md](TODO.md): under a version heading if the behaviour is visible to anyone
using the adapter, under **Maintenance** if it only matters to whoever works on
it next.

This fork of [hicder/agy-acp](https://github.com/hicder/agy-acp) has no releases
of its own yet, so everything below is unreleased.

## Unreleased

The stream-json port merged as `bf6e81b` (PR #1). It has not been installed
anywhere yet; see "Verify the port under Paseo" in [TODO.md](TODO.md).

### Maintenance

- Keep the Windows build portable by failing closed when the Unix-socket-based
  `--permission-prompts` feature is requested there.

### Added

- `--permission-prompts` routes agy's tool permission checks to the ACP host.
  agy runs headless under this adapter and cannot ask, so without the bridge it
  auto-denies and tool calls fail silently. The bridge is the sole gate: agy runs
  with `--dangerously-skip-permissions` because its own checks would otherwise
  deny before a `PreToolUse` hook decision could take effect, so every case the
  bridge cannot resolve denies.
- Streaming reads `agy --output-format stream-json` instead of polling agy's
  SQLite conversation DB, adopted from upstream. Live updates arrive as agy
  writes them, and the conversation id comes from the stream rather than from
  diffing DB filenames.

### Changed

- Permission prompt options say what they cover: "Always allow run_command this
  session" rather than "Always allow". The answer applies to every later call to
  that tool for the rest of the session, and the prompt -- which shows one
  command -- is where someone decides. The ACP `kind` values are unchanged, so
  hosts style and bind them as before.

### Fixed

- Two paths reached around the permission boundary. `outside_workspace()` only
  looked at arguments beginning with `/`, so `../../secret` and `~/.ssh/id_rsa`
  were never judged against the workspace and were auto-allowed; relative and
  home-relative arguments are now resolved from each root and normalized
  lexically, so `sub/../file.txt` is judged inside and `../secret` outside. And a
  remembered "Always allow" was consulted before any containment or
  sensitive-path check, so one approval of `view_file` opened `.env` for the rest
  of the session; a sticky allow now falls through to asking when the call leaves
  the workspace or names something sensitive. A sticky reject still applies
  immediately.
- Containment had two more holes of the same kind. With no workspace root
  registered the check looked only at absolute arguments, so `~/.ssh/id_rsa`
  counted as contained; an unset root now contains nothing. And traversal was
  detected by searching for the two characters `..`, so an ordinary query like
  `foo..bar` was read as a path leaving the workspace; it must now be a path
  component.
- A plain relative argument was never judged a path. Containment looked at a
  value's shape -- a leading `/` or `~`, or a `..` component -- so that a search
  query would not be mistaken for a file, which left `link/secret.txt` escaping
  through an in-workspace symlink without being checked at all. agy's arguments
  are a fixed schema, so the known path fields (`AbsolutePath`, `TargetFile`,
  `DirectoryPath`, `SearchPath`, `Cwd`, `Paths`) are now judged whatever their
  value looks like, and `Query` is still left alone. A field missing from that
  list keeps the shape tests and nothing else, so an omission costs coverage
  rather than raising a false prompt.
- A symlink out of the workspace was contained. `is_inside()` accepted a path
  either as written or resolved, and the as-written form matched on its first
  component: `<workspace>/link/../secret` looked inside even where `link` points
  out of the workspace and the kernel follows it there. Only the resolved form
  counts now, falling back to lexical normalization for a file that does not
  exist yet -- which at least cancels the `..` that `starts_with` ignores.
- A failed drain could still hang the turn. When a stdout read error was
  followed by a fallback `tokio::io::copy` that also failed, nothing was reading
  agy's stdout and `child.wait()` waited on a child blocked writing to a full
  pipe -- the exact hang the byte-framed read was added to prevent. An
  undrainable pipe now kills the child, and is reported as a failed turn rather
  than a cancelled one.
- Persisted sessions were pruned on a whole-second timestamp, so entries written
  within the same second tied and were evicted in `HashMap` order, which could
  drop a just-refreshed resumable session and keep an older one. `updated_at` is
  milliseconds.
- A prompt carrying no `sessionId` could not be cancelled at all: its token was
  deliberately left out of the registry, while the turn itself ran a full agy
  process. It is now registered under the id it was given -- the empty one -- so
  a cancel naming that id reaches it.
- A remembered "Always reject" deadlocked the bridge. The branch holding it took
  the state mutex in an `if let` scrutinee, whose guard lives to the end of the
  body, and the body awaited the same mutex. It had no test until now, so it went
  unnoticed since the feature landed.
- A single malformed byte on agy's stdout hung the turn. The drain loop ended on
  the first read error -- and invalid UTF-8 is a read error -- after which nothing
  read the pipe, so the child blocked writing and `child.wait()` never returned.
  Frames are read as bytes and decoded lossily, and a genuine I/O error drains
  the remainder before giving up.
- `session/update` notifications could corrupt a response. The stream reader held
  its own `io::stdout()` handle while the main loop wrote the same fd, so two
  writers could interleave mid-line. Every notification now goes through the
  main loop's output channel, as the permission bridge already did.
- Cancelling a turn could cancel the wrong one. The cancellation map held one
  token per session, so a second prompt for that session overwrote the first's
  token and a cancel flipped the wrong flag; whichever turn finished first
  removed the other's token entirely. Tokens are now per turn, removed by
  identity, and a cancel stops every turn in the session.
- `sessions.json` grew without bound -- 910 entries on one machine, 553 never
  bound to a conversation -- and every turn rewrote the whole file. It is capped
  at 256 entries, dropping unresumable ones first and oldest first within each
  group. In-memory eviction also picked an arbitrary `HashMap` key, so it could
  drop a live session and keep a dead one; it is now least-recently-used.
- Model selection sent agy a model name it rejects. `agy models` prints
  `id<TAB>Human Label`; the whole line was being used as the id, so `--model`
  received `gemini-3.7-flash-high\tGemini 3.7 Flash (High)`. ACP now gets the id
  as `modelId` and the label as `name`, ids from a client are checked against
  what agy offers, and a tab-joined value left in an old `sessions.json` is
  repaired on restore. Upstream splits the tab but keeps the label
  (`parse_model_line`, `hicder/agy-acp` at 858041c, `src/adapter.rs:702-706`),
  which is the same defect from the other end.
- A failed turn could report success. The error response was gated on no updates
  having been emitted, so a turn that streamed one chunk and then failed returned
  `stopReason: "end_turn"`. A stream reaching EOF without its terminal `result`
  event was also treated as completion.
- A tool call the user refused is reported as `stopReason: "refusal"` rather than
  a provider error. agy reports a refusal as a failed turn, and only the bridge
  knows the difference; its own fail-closed denials deliberately do not count.
- `session/load` replays conversation history again. Upstream's stream-json
  rewrite dropped it along with the SQLite reader, leaving a reopened thread with
  an empty transcript while agy still had the context. SQLite is read for this
  path and nothing else.
- The protobuf walkers could panic on a corrupted or hostile conversation DB. A
  length field of `u64::MAX` wrapped `i + len`, turning the bounds check into a
  pass and panicking on the slice. All offset arithmetic is checked.
- The README's Zed example recommends `--permission-prompts`. The instruction it
  replaces -- that you **must** set `AGY_EXTRA_ARGS="--dangerously-skip-permissions"`
  -- came in with upstream's README in this same change and never described this
  fork, which has had the bridge all along. The bypass is still documented, as an
  opt-in with a warning.

### Known issues

- An "Always allow" answer is keyed by tool name, not by arguments, so approving
  `run_command` once approves every later command in that session -- `rm -rf` and
  all. The containment and sensitive-path checks still run on a remembered allow,
  but they read arguments as paths and a command line is one opaque string, so
  `cat /etc/shadow` is not recognised as naming a path at all. Documented in the
  README under "What 'Always' remembers", pinned by two tests that assert the
  current behaviour on purpose, and tracked in TODO.md.
- The output channel is unbounded. Every notification now goes through one
  `mpsc` to the single stdout writer, so a host that reads its end of the pipe
  more slowly than agy produces events makes that queue grow without a ceiling.
  Bounding it would push the backpressure onto agy, which is the right shape but
  couples a stalled host to agy's progress, so it is measured and decided rather
  than swapped in. Tracked in TODO.md.
### Maintenance

- CI: `ci.yml` runs `cargo build`, unit tests, and the ignored I/O tier with
  `--ignored --skip e2e` to exclude the four e2e tests; Rust 1.70 runs on Linux
  and Windows. All Actions are SHA-pinned, checkout credentials are not
  persisted, and e2e is protected by the approval-gated `e2e` environment with
  its own `E2E_GEMINI_API_KEY`. No formatting gate — the tree is not
  rustfmt-clean.
- Hard fork: the `upstream` remote is removed, `gh repo set-default` points at
  this fork, and `pre-push` refuses any target but `kgrizz-git/agy-acp`. Note
  that `gh pr create` in a fork defaults its base to the parent repo regardless
  of git remotes, which is what that second guard is for.
- `scripts/check-upstream.sh` and a weekly workflow report commits on
  `hicder/agy-acp` that this fork has not taken, comparing against
  `.upstream-watermark`. The watermark moves only in a commit a human made.
- `pr_compliance_checklist.yaml` describes this fork's invariants for automated
  review, after two review findings cited rules describing the architecture the
  stream-json port replaced.
- Fork notes folded from `AGENTS.fork.md` into `AGENTS.md`; work items moved to
  `TODO.md`.
- `AGENTS.md` claimed `cargo test` does no filesystem I/O and that the `#[ignore]`d
  tier is what touches disk. Neither has been true for a while: tier-1 permission
  and persistence tests create scratch directories under `$TMPDIR`, and the
  ignored set is ignored by inheritance. The compliance checklist's record of
  known permission gaps is likewise updated, since one of the two it listed is
  closed by this branch.
- Removed the stale test-only conversation-DB delta reader. Load-replay coverage
  now proves that its persisted all-row watermark advances past a trailing user
  message.
