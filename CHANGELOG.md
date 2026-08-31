# Changelog

Notable changes to this fork. Entries land here when work leaves
[TODO.md](TODO.md): under a version heading if the behaviour is visible to anyone
using the adapter, under **Maintenance** if it only matters to whoever works on
it next.

This fork of [hicder/agy-acp](https://github.com/hicder/agy-acp) has no releases
of its own yet, so everything below is unreleased.

## Unreleased

The stream-json port merged as `bf6e81b` (PR #1) and was first installed and
exercised under Paseo on 2026-08-30. The permission bridge and the read, write and
edit tools all work, and a conversation continues correctly across turns within a
session. The reopened-thread path (`session/load` or `session/resume` — which
Paseo sends is not yet established) is still untested; it is in
[TODO.md](TODO.md). The cancellation defect the verification turned up is fixed
below.

### Fixed

- One "Always allow" on a command tool no longer approves every later command.
  Remembered answers were keyed by `(session, tool name)`, so approving
  `echo verification-one` silently approved a later `rm -f other.txt` — reproduced
  live under Paseo on 2026-08-30, with the file deleted and no second prompt. The
  key now carries a fingerprint of the arguments as well, and the narrow key is
  the default: a tool has to *earn* tool-level keying by being a read, edit or
  search tool whose arguments name nothing but paths, which is exactly the case
  where the containment and sensitive-path checks still constrain a remembered
  allow. An unknown tool gets the stronger key, and so does any call carrying a
  command line or a URL — `read_url_content` is classified as a read, but a `Url`
  is not a path field, so keying it by tool would have let one "Always allow" on
  a trusted URL cover every later one. Comparison is exact — no
  tokenizing, no shell semantics — because under-matching costs a prompt while
  over-matching is a hole; only agy's own presentational fields (`toolAction`,
  `toolSummary`, `WaitMsBeforeAsync`) are excluded, since they do not change what
  runs. Remembered **rejects** narrow identically, so rejecting one command
  forever rejects that command rather than the tool. The prompt labels say which
  is which, and say it in the right vocabulary: **Always allow this exact command
  this session** where the call carries a command line, **Always allow this exact
  call this session** where it is keyed by its arguments but is not a command
  (`read_url_content`, `search_web`, anything unrecognised), and **Always allow
  \<tool\> this session** where the answer really does cover the tool. The label,
  the stored key and the reason string are all derived from one `AlwaysScope`
  computed in `decide`, because they were previously computed in three places
  with nothing tying them together.

- A session's remembered answers are now forgotten when the session is evicted.
  `BridgeState.always` and `BridgeState.conversations` accumulated for the life of
  the process and nothing ever removed an entry, so a session id recycled after
  eviction inherited answers it was never given. `evict_if_needed` is synchronous
  and cannot await the bridge's lock, so it queues the victim id and the
  dispatcher drains the queue between requests; re-admitting the same id before
  the drain cancels the pending forget. It drops both maps: the remembered
  answers, and the agy-conversation-to-session binding, so a hook still arriving
  for a forgotten conversation is resolved by the running session instead. Every
  path this opens still ends at a prompt or a denial -- forgetting can only
  remove an answer, never add one -- so the worst case is being asked again. This also bounds the bridge by the
  same 64 sessions as the map, and gives "this session" a defensible meaning: the
  answer lasts as long as the session is live in memory.

- A turn no longer leaves its permission request behind when it ends. An
  outstanding `session/request_permission` was keyed only by its JSON-RPC id, so
  nothing could find it again: the call sat waiting for its full 540-second
  timeout, and that timeout marks a refusal, landing in whatever turn happened to
  be running nine minutes later and reporting `stopReason: "refusal"` for a turn
  nobody refused. Pending requests now carry the session that asked, and both
  cancellation and ordinary turn teardown answer their own — denying, because agy
  must not run the tool, but not as a refusal, since nobody declined anything.
  Clearing it at teardown as well as on cancel matters: a turn that ends because
  agy died or its output became unreadable left the same request behind, with the
  same consequence, and nothing else would ever have cleared it. A late answer
  from the host is dropped rather than applied, so an "always allow" arriving
  afterwards cannot become sticky for the rest of the session. Starting a turn
  clears everything still pending as well, in the same place the refusal flag is
  reset — one turn runs at a time across the whole adapter, so a leftover there
  can only belong to a turn that is over, and routing it through the one point
  every turn must pass keeps a dropped teardown call from being enough to bring
  the bug back. It clears every session's, not just the starting session's:
  the refusal flag is one flag for the adapter rather than one per session, so a
  request stranded by one session times out into whichever turn is running nine
  minutes later, which is somebody else's. The host may still be showing the
  prompt: ACP has no way to retract a request, and a host that cancels is
  expected to dismiss its own.

  Applying a decision is gated on the turn that asked for it, which closes the
  same leak by its other route. The host's answer resolves the request, but the
  hook task that acts on that answer runs whenever the runtime next polls it —
  possibly after the turn ended, by which point the pending entry is long gone
  and draining it cannot help. A refusal applied then set the adapter-wide flag
  after the turn that asked had already read it, so the *next* turn reported
  `stopReason: "refusal"` having asked nobody anything, and an "always" applied
  then became a standing permission for the turns that followed. Both are now
  dropped unless the turn that asked is still the turn that is running — where
  "running" excludes the gap between one turn's teardown and the next turn's
  start, since `always` is not reset by anything and a sticky answer applied in
  that gap would outlive it.

  Nothing is decided on behalf of a turn that is not the one running. A hook task
  is not polled on any schedule of the adapter's: it can first reach the decision
  path after its own turn tore down, or after the next turn started. Left alone it
  would raise a prompt for a turn that no longer exists — and, worse, adopt the
  running turn's identity, so answering it counted against a turn that never asked.
  Such a request is now denied without asking anyone, and registering the question
  revalidates that rather than trusting the check: the two are separate lock
  acquisitions with work in between, so the turn can end in the gap, and teardown's
  drain would run before the entry exists to be drained.

- Cancelling a turn stops the command, not just agy. `session/cancel` killed the
  `agy` process alone, and agy runs a tool call by shelling out, so the shell and
  its command were reparented to PID 1 and ran to completion — verified against
  agy 1.1.22 by cancelling `sleep 45 && touch marker` and watching the marker
  appear 45 seconds later, and by the same route a build, a `curl` or an `rm -rf`
  would have finished too. A cancel now kills agy's whole process tree: agy is
  stopped so it cannot start anything else, the process table is read while agy
  is still alive to hold the parent links, whatever is found is stopped too and
  the table read again until a read turns up nothing new — stopping agy does not
  stop the shell it already started, and that shell can fork its next command
  between two reads — and then the lot is killed, agy last. Killing agy's *process group* would have been the obvious
  fix and does not work: agy puts each command it runs into a process group of
  its own, so `killpg` on agy reaches agy and nothing else. agy is still spawned
  into its own group, but only so that a signal aimed at the adapter's group
  cannot kill agy first — which would erase the parent links the walk needs. The
  adapter also kills those trees on `SIGTERM`, `SIGINT` and `SIGHUP`; previously
  there was no kill on exit at all, no signal handler and no `Drop`, so
  terminating the adapter orphaned the whole tree silently. On a non-Unix target there is no process table to
  walk, so a cancel kills the direct child as it always did, and there is no
  shutdown kill at all.

- Judge `find_by_name`'s `SearchDirectory`, and `FilePath`, as paths.
  `SearchDirectory` was missing from `PATH_FIELDS`, so a relative value — with no
  leading `/`, no `~` and no `..` — was judged by neither the field-name test nor
  the shape tests, and a search directory that left the workspace through a
  symlink would not have been prompted. Found by capturing real agy traffic.
  `FilePath` was added on separate evidence: `tools.rs` and `protobuf.rs` already
  treated it as naming a location while `PATH_FIELDS` did not.

### Maintenance

- `pr_compliance_checklist.yaml` gains a rule for what a cancel has to reach, so
  an automated review that sees `child.kill()` reappear on a kill path, or the
  walk swapped back for `killpg`, has the measurement to judge it by.
- Check `PATH_FIELDS` against real agy 1.1.22 traffic. One field was missing (see
  Fixed above); every other path argument observed is covered, and `Url`, `query`
  and the boolean `FullPath` are correctly not treated as paths. Also established
  agy's tool surface as observed in 1.1.22, which does not match this fork's
  assumptions: five tool names in `permission.rs` match nothing agy emitted or
  self-reported, and seven tools it does report are unclassified here. Both
  recorded in [TODO.md](TODO.md).
- Keep the Windows build portable by failing closed when the Unix-socket-based
  `--permission-prompts` feature is requested there.
- Keep fork PRs from waiting on an e2e-environment approval they cannot use, and
  make unit-test scratch homes collision-resistant across test processes.

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
