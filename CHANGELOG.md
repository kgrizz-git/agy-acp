# Changelog

Notable changes to this fork. Entries land here when work leaves
[TODO.md](TODO.md): under a version heading if the behaviour is visible to anyone
using the adapter, under **Maintenance** if it only matters to whoever works on
it next.

This fork of [hicder/agy-acp](https://github.com/hicder/agy-acp) has no releases
of its own yet, so everything below is unreleased.

## Unreleased

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

### Fixed

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

- `test_read_response_from_db` fails under `cargo test -- --include-ignored`. It
  is upstream's test and upstream's implementation, `#[ignore]`d since it was
  written, so it has not run in either lineage in months: it expects
  `max_step_idx == 1` where the code returns `2`, having advanced the cursor over
  a trailing user-message row. The helper it tests is `#[cfg(test)]`-only, so
  nothing in production depends on the answer. Tracked in TODO.md.

### Maintenance

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
