# Plan: Investigate and fix `test_read_response_from_db`

> **TODO discipline:** The corresponding TODO.md entry ("test_read_response_from_db
> disagrees with the code", lines 214–222) and its Next Up pointer stay on the
> board **until this work lands** — a plan is not completion. This plan *links* to
> the entry; deletion is the **final step of implementation** (the PR that merges
> the fix), never a planning step. Per repo rule, entries are deleted on landing,
> not ticked.

## Context

`test_read_response_from_db` is the suite's only red test. It is `#[ignore]`d, so
it does not run under `cargo test` or in CI today, and it has been red in both
this fork and upstream for months. From `TODO.md` (Fork maintenance →
"test_read_response_from_db disagrees with the code") and the matching
`CHANGELOG.md` note:

- `read_delta_from_db` advances `max_step_idx` over **every** row it read,
  including the trailing user-message row, and returns `2`.
- The test expects `1` — the last row it actually takes text from.

The helper under test, `read_delta_from_db`, is `#[cfg(test)]`-only. **Nothing in
production depends on either answer** — production uses
`read_replay_updates_from_db`, which starts at `-1` and never calls the delta
helper incrementally. So this is a correctness/clarity task, not a behaviour
regression.

### What the code actually does (verified by reading)

- `db.rs:14` `read_rows_from_db(dir, conv, after)` → `SELECT idx, step_type,
  step_payload FROM steps WHERE idx > after ORDER BY idx`.
- `db.rs:93` `read_delta_from_db(dir, conv, after)`:
  - `max_idx = after`
  - for each returned row: `max_idx = max(max_idx, idx)`.
  - collects text only from rows where `step_type == 15`.
  - returns `ConversationDelta { text, max_step_idx: max_idx }`.

### The failing case, concretely

`test_read_response_from_db` inserts:

| idx | step_type | payload |
|-----|-----------|---------|
| 1   | 15        | "hello world" |
| 2   | 14        | user message (no response text) |

It calls `read_delta_from_db(dir, "test-conv", -1)`:
- both rows are returned (`idx > -1`).
- `text` = `"hello world"` (only the step_type 15 row contributes). ✓ matches
  assertion `delta.text == Some("hello world")`.
- `max_idx = max(-1, 1, 2) = 2`.
- assertion `delta.max_step_idx == 1` **fails**; code returns `2`.

So the disagreement is entirely about the cursor: should `max_step_idx` be the
highest *response* row index seen (`1`), or the highest *row* index seen (`2`)?

### Two defensible semantics

1. **Cursor = highest row read (`2`).** As a *resume cursor* this is correct: the
   next call should pass `2` so it skips both the response and the trailing
   user-message row. The delta helper's `after` parameter only makes sense as a
   watermark over all rows, because the producer interleaves user (14) and
   response (15) steps and any later call must not re-read either. TODO.md itself
   says "As a cursor, 2 looks right."
2. **Cursor = highest response row (`1`).** Matches the test's stale assertion,
   but is wrong as a watermark: the next call passing `1` would re-read the
   user-message row at `idx == 2`. This is the bug-prone semantic and should not
   be chosen.

**Decision to make during implementation:** adopt semantic (1). It is the only
one that keeps the `after` cursor correct for an incremental replay use, and it
matches the existing `read_replay_updates_from_db` convention (which also tracks
the highest row, `db.rs:65` `max_idx = max_idx.max(*idx)` over *all* step types).
The test assertion is the stale half. We fix the **test**, not the code — and we
add a docstring making the cursor's meaning explicit so it can't rot again.

> Note: if, during review, we decide the delta helper is genuinely dead and only
> misleads, the alternative is to *delete* `read_delta_from_db` and the two
> delta-only tests. That is a valid outcome but loses the one piece of coverage
> for the `after` watermark. Prefer fixing + documenting unless review finds the
> helper has zero future value.

## Steps

1. **Reproduce and pin the current failure.**
   - `cargo test -- --ignored test_read_response_from_db` → confirm it fails on
     `max_step_idx` (expect observed `2`, got assertion `1`).
   - Capture the exact panic message as evidence for the PR.

2. **Decide and record the semantic.** Confirm option (1) above (cursor = highest
   row read). Write the reasoning into the test's own comment block so the next
   reader sees *why* `2` and not `1`.

3. **Fix the test, not the code — and commit to retention.**
   - Decision: **keep** `read_delta_from_db`. Although it is `#[cfg(test)]`-only
     and production never calls it incrementally (`read_replay_updates_from_db`
     at `db.rs:54` starts at `-1` and never calls the delta helper), deleting it
     would remove the only coverage of the `after` watermark that
     `read_rows_from_db` exposes — and the watermark convention is shared with
     production (`db.rs:65` uses the same `max_idx.max(*idx)` over all step
     types). Retention is justified; record that justification in the commit
     message and reference `TODO.md:220` and `db.rs:59`. (If review later
     concludes the helper is dead weight, deletion is a valid alternative, but it
     removes the only incremental-watermark coverage, so default to keeping.)
   - In `test_read_response_from_db`, change
     `assert_eq!(delta.max_step_idx, 1);` → `assert_eq!(delta.max_step_idx, 2);`
     with an inline comment: "cursor is the highest row *read* (idx 1 response +
     idx 2 user message), not the highest response row — the next call must skip
     both."
   - Leave `delta.text == Some("hello world")` unchanged (already correct).

4. **Document the durable contract in compiled code only.** A docstring on a
   `#[cfg(test)]` fn is invisible to `cargo doc` and to production, so place the
   contract solely on code that actually ships:
   - `read_rows_from_db` (`db.rs:14`): state that `after_step_idx` is an
     **exclusive watermark over all `steps.idx` rows, not just `step_type == 15`
     (response) rows**, and that the caller must pass back the highest `idx`
     returned to avoid re-reading interleaved user/response steps.
    - `Session.last_step_idx` / `StoredSession.last_step_idx` (`types.rs:40`,
      `70`): **edit the existing docstrings** (do not append a second paragraph).
      `StoredSession.last_step_idx` already has a doc (`types.rs:38`); reword it to
      capture both sources: "advanced by load-replay to the highest `steps.idx` row
      read, and by the live stream to the highest step index observed; both feed
      the same persisted watermark used to resume replay." (The field is written
      from replay's DB max idx at `adapter.rs:525` *and* from the streaming step
      index at `adapter.rs:946→962` / `streaming.rs:117`, so claiming unconditional
      identity of the two index spaces would overstate the code.)
    - Do **not** add any docstring to the `#[cfg(test)]` `read_delta_from_db`
      (`db.rs:93`) or `ConversationDelta.max_step_idx` (`types.rs:56`); doc there
      is dead weight (invisible to `cargo doc`). If a pointer is wanted, a
      `/// see \`read_rows_from_db\`` on the test helper is harmless but optional.
    - Optional test-comment: note that `after==4 → None` comes from the no-text
      path, not the no-rows path — they are indistinguishable to callers, which
      is itself part of the contract.

5. **Strengthen the test with a fixture that actually discriminates the two
   cursors.** The 2-row fixture can't (any `after > 0` returns `None` because there
   is no response text past idx1), and a naive 3-row fixture
   (`idx1=15, idx2=14, idx3=15`) does **not** discriminate either: a *response-only*
   cursor also yields `max==3` (response rows are {1,3}), so `after==2 → Some("world"),
   max==3` passes under BOTH semantics. The fixture must have a non-response row as
   the **last** row so the two maxima differ. Extend `test_read_response_from_db`
   to four rows: `idx1=15 ("hello")`, `idx2=14` (user msg, no text),
   `idx3=15 ("world")`, `idx4=14` (trailing non-response, no text). Assert:
   - `read_delta_from_db(dir, conv, -1)` → `text == "hello\nworld"`,
     `max_step_idx == 4`. ← the discriminating assertion: a response-only cursor
     would give `3`, so `== 4` proves the all-row watermark.
   - `read_delta_from_db(dir, conv, 2)` → `Some("world")`, `max_step_idx == 4`
     (reads idx3 + idx4; proves the watermark consumes the non-response tail).
   - `read_delta_from_db(dir, conv, 3)` → `None` (idx4 has no response text).
   - `read_delta_from_db(dir, conv, 4)` → `None`.
   Drop the old `after==1 → None` call (with idx3 present, `after==1` reads
   idx2+idx3 and returns `Some("world")`, not `None`). Mirror the incremental
   pattern in `test_read_response_multi_step_no_skip_no_duplicate`
   (`tests.rs:1222-1233`). Note: the existing multi-step test ends on a
   `step_type == 15` row (idx5), so it does NOT discriminate either — this 4-row
   design is the first test that does.

6. **Run the ignored tier to confirm green.**
   - `cargo test -- --include-ignored` → all pass, including the other
     db/state tests.

7. **Update docs.**
   - `CHANGELOG.md`: **delete** the "Known issues" entry at lines 150–155 that
     says `test_read_response_from_db` fails — it is no longer true once this
     lands. Then, under the **Maintenance** heading, add: "Fixed
     `test_read_response_from_db`: the assertion expected the highest *response*
     row as the resume cursor; the code returns the highest *row* read, which is
     the correct watermark over interleaved user/response steps. The test now uses
     a 4-row fixture whose trailing non-response row makes `max_step_idx` diverge
     from a response-only cursor, so it actually guards the contract. The helper's
     cursor contract is now documented on `read_rows_from_db`. This test was
     `#[ignore]`d and red in both lineages; nothing in production used the helper."
    - `TODO.md`: **leave the "test_read_response_from_db disagrees with the code"
      subsection (lines 214–222) in place until this work actually lands.** This
      plan is the implementation, so the work item stays on the board as "Next Up"
      until the PR is merged. The *final* step of implementation (step 8) deletes
      it — per repo rule, entries are deleted on landing, not ticked. Link the
      entry to this plan: add a one-line pointer at the top of that subsection,
      e.g. "Plan: `plans/fix-test-read-response-from-db.md`." Do not delete it now.
    - `AGENTS.md`: no change needed (it describes behaviour, not this test), but
      verify nothing references the old `max_step_idx == 1` expectation.

8. **Commit and open a PR** (only if asked). Stage `src/db.rs`, `src/types.rs`,
   `src/tests.rs`, `CHANGELOG.md`, `TODO.md`. Message in repo style, e.g.
   "Fix test_read_response_from_db cursor assertion".

## Verification

- `cargo test -- --include-ignored` is fully green.
- The `after == -1 → max_step_idx == 4` assertion passes, proving the watermark
  counts the trailing non-response row (a response-only cursor would yield 3).
- `cargo build` clean (the `#[cfg(test)]` changes don't affect the release
  binary).

## Risks / things the reviewer should challenge

- Are we sure the cursor should count user-message rows? The strongest
  counter-argument is "the consumer only cares about response text, so the cursor
  should track that." We reject it because the `after` parameter is a DB-row
  watermark and the producer interleaves step types; a response-only cursor
  re-reads user rows. State this explicitly so the reviewer can attack it.
- Is `read_delta_from_db` worth keeping at all? If review concludes it's dead
  weight, deleting it (and the two delta-only tests) is the cleaner fix — but
  that removes coverage of the `after` watermark, so prefer documenting unless
  convinced otherwise.
