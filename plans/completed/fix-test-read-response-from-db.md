# Plan: Retire the stale DB-delta test and pin replay's row watermark

> **TODO discipline:** Keep the corresponding `TODO.md` entry until this work
> lands. It is now linked from that entry; deleting it is the final landing step,
> not a planning or implementation-preparation step.

## Decision

`read_delta_from_db` and `ConversationDelta` are test-only remnants of the
pre-streaming polling implementation. Production replay calls
`read_replay_updates_from_db` once with `after_step_idx == -1`; no production
code performs incremental delta reads or consumes the helper's cursor.

The helper's current result is internally correct: its cursor is the greatest
database row index read, not the greatest response row index. In the existing
fixture that is `2`, not `1`. A response-only cursor would re-read an
interleaved non-response row.

Rather than preserve a dead helper solely to test its own contract, remove it
and put the assertion on the production replay path. That gives the same
all-row watermark coverage where it actually matters.

## Scope

1. Remove test-only delta code.

   - Delete `ConversationDelta` from `src/types.rs`.
   - Delete the `#[cfg(test)]` import and `read_delta_from_db` from `src/db.rs`.
   - Delete all three tests that only exercise that helper:
     `test_read_response_from_db`,
     `test_read_response_multi_step_no_skip_no_duplicate`, and
     `test_read_response_missing_steps_table`.
   - Do not alter `read_replay_updates_from_db`'s row-watermark behavior.

2. Put the regression coverage on real replay.

   - In `test_session_load_replays_conversation_history`, append a final
     `step_type == 14` user-message row after the final assistant response.
   - Update its expected replayed update order and text to include that final
     user message.
   - Preserve the existing seeded persisted value of `8`, then call
     `handle_session_load` before reading either assertion target. Assert that
     `adapter.sessions["sess-replay"].last_step_idx` is the final row index
     after that call. Also read the persisted session and assert the same value,
     proving load replay overwrote the stale value with the all-row watermark.
   - This final non-response row is essential: a response-only maximum would be
     the preceding assistant row and would fail the assertion.

3. Document only the live contract.

   - Add a concise doc comment to `read_rows_from_db`: `after_step_idx` is an
     exclusive cursor over every `steps.idx` row, and an incremental caller must
     advance it to the largest returned row index.
   - Leave the `Session.last_step_idx` and `StoredSession.last_step_idx`
     docstrings alone. They describe stream-json indices; current replay does
     not use them as a cursor and can overwrite them from the DB, so calling
     them a common replay watermark would be inaccurate.

4. Update tracking documentation.

   - Remove the obsolete known-issue entry in `CHANGELOG.md`.
   - Under `Maintenance`, record that a stale test-only delta reader was
     removed and production load-replay now asserts the all-row DB watermark.
   - Keep the TODO entry through review and merge. Delete it only in the landing
     change, as required by `TODO.md`.

## Verification

- First, preserve the current evidence:
  `cargo test -- --ignored test_read_response_from_db` fails with actual `2`
  versus expected `1`.
- After the change:
  `cargo test -- --ignored --skip e2e` passes. `--ignored` runs only the
  ignored-I/O tier without needlessly repeating unit tests; `--skip e2e`
  excludes the network/auth-dependent e2e tests.
- `cargo build` passes.
- The enhanced replay test proves that a trailing user row advances the
  persisted `last_step_idx` past the preceding response row.

## Landing order

Land this change before enabling the CI ignored tier. The proposed CI command
runs the enhanced replay test, whereas CI added first would fail on the known
stale delta assertion.

## Non-goals

- Do not claim that this fixes runtime behavior; it removes dead test code and
  strengthens coverage of behavior production already has.
- Do not use `--include-ignored` for the deterministic ignored-I/O tier: it
  reruns the unit suite and still needs an e2e exclusion.
