# E2e quota rotation and turn reduction

Plan for the e2e daily-quota failures (first diagnosed 2026-09-07). Decision
points and open questions are flagged **(DP)** or **(OQ)** where they arise.
Factual claims are cited `file:line` or to the failing run's captured log.

## Objective

Keep the secret-gated e2e suite runnable on a free Gemini key. The limit that
actually failed is not the per-minute burst the serialization change addressed —
it is a hard **daily** ceiling, and neither serialization nor retry moves it. The
levers are (a) spread model usage across distinct models, which are separate
daily buckets, and (b) cut the number of model calls per run without weakening
what each test proves.

## Background — the binding constraint

The captured agy log (run 34150352103) shows the real error stderr never
carried, so it was invisible until `forward_stderr` and the `Capture agy logs`
step landed:

```text
Error 429: You exceeded your current quota
quotaId: GenerateRequestsPerDayPerProjectPerModel-FreeTier
quotaMetric: generativelanguage.googleapis.com/generate_content_free_tier_requests
limit: 20, model: gemini-3.6-flash
```

Two facts matter for the design:

- **Per model.** `quotaDimensions` names `model:gemini-3.6-flash`, and
  `quotaId` says `PerProjectPerModel-FreeTier`. Distinct flash models (`3.6`,
  `3.7`, `3.8`) are separate buckets of 20/day each. **Caveat:** the quota that
  fired names the bare `gemini-3.6-flash` slug, but the workflow's awk regex
  (`.github/workflows/e2e.yml:143-148`) selects `gemini-*-flash-low`, the
  reduced-reasoning variant. Whether `-flash` and `-flash-low` share one quota
  bucket or are counted separately is unverified — Google could meter by base
  model or by exact slug. The OQ1 probe should include a same-base different-
  reasoning-effort pair to settle this.
- **Per day, not per minute.** `.github/workflows/e2e.yml:190-197` explains that
  `--test-threads=1` was added for the *per-minute* free-tier Flash quota
  ("a handful of requests per minute"). That is a real but different constraint;
  this daily ceiling is what failed. Nothing serial or retried changes it.

Before this branch's implementation, the four e2e tests on `main` issued real
agy turns against the model:

| test | turns | calls the model? |
|---|---|---|
| `test_e2e_agy_acp_full_round_trip` (`main:src/e2e_tests.rs:27`) | 1 | yes |
| `test_e2e_error_paths` (`main:src/e2e_tests.rs:407`) | 0 | no — unknown-session and unknown-method errors are answered locally |
| `test_e2e_multi_turn` (`main:src/e2e_tests.rs:286`) | 2 | yes |
| `test_e2e_session_load` (`main:src/e2e_tests.rs:338`) | 2 | yes |

So a clean run is **5 model turns**, against a 20/day bucket. Three PRs in one
day (~15 turns) approached the ceiling; the run that failed crossed it. Because
all five turns landed on the *one* model the old configure step pinned
(`main:.github/workflows/e2e.yml:80-113`), a single busy day exhausted the bucket.

## Decisions

### DP1 — reduce to three model turns by folding `multi_turn` into `session_load`

The two-turn tests overlap in exactly the assertion that costs each an extra
turn. The old `session_load` second prompt (`main:src/e2e_tests.rs:387`) already proved
conversation continuity through the same `--conversation` binding that
`multi_turn` existed to test (`main:src/e2e_tests.rs:317-330`); `session_load`
just did not assert
that the model *remembers* the earlier turn. Folding the memory assertion in:

- Keep `session_load` at its two turns, but make turn 1 plant a token (as
  `multi_turn` does with "BANANA") and turn 2 both load and then ask for that
  token. The test then proves replay *and* live continuity in two turns.
- Delete `test_e2e_multi_turn`. The one thing it proves that `session_load`
  does not is *uninterrupted* binding without a load in between. That is a real
  distinction, but it is the same code path (`conversation_id` → `--conversation`,
  exercised on every prompt per `AGENTS.md` "Conversation binding"), so it is not
  lost coverage in practice; calling it out under OQ2 so it is a decision, not a
  drift.

Net: **3 model turns** (1 round-trip + 2 session-load), down from 5, with no
assertion removed — the merged test is strictly stronger than the old
`session_load`.

### DP2 — rotate models, do not just pin one

Even at 3 turns/run, one model's 20/day is ~6 runs/day before the ceiling. Spread
the turns across flash models to widen headroom. Rotation is a mitigation, not
a cure — it multiplies headroom by the number of available flash models.

**Turn distribution.** With two model-issuing tests and three turns total (1
from `full_round_trip`, 2 from `session_load`), per-test model assignment gives
one model 1 hit and another 2 hits in any single run. That imbalance is fine per
run; what matters is spreading the 2-turn load across runs so the same model
does not always absorb the heavier test. Rotate the starting offset by
`run_number % roster.len()`: each test picks
`roster[(offset + test_index) % roster.len()]`.

Example with 3 models (A, B, C):

| Run | `full_round_trip` (1 turn) | `session_load` (2 turns) |
|---|---|---|
| 0 | A | B |
| 1 | B | C |
| 2 | C | A |

After 3 runs: A 3, B 3, C 3 — even. Deterministic from the run number,
so a failure is reproducible from the CI run id.

**Edge cases by roster size:**

- **0 (empty):** `% 0` panics. If the awk query returns no matching slugs,
  tests must skip `set_model` and fall through to the `settings.json` default —
  the same path they take today. The roster env var being empty or unset is the
  signal.
- **1:** Both tests get the same model. Rotation is a no-op. Nothing breaks,
  but there is no quota relief — the suite is back to one bucket, same as today.
  Worth a CI warning so a catalog change that drops the roster to 1 is visible.
- **2+:** Rotation works. With 2 models, tests always get different models per
  run and the 2-turn load alternates between them; headroom is 2×. With 3+, the
  example above applies.

Mechanism, using an existing code path rather than new infra:

- The configure step writes the fallback `settings.json` first and then
  queries the live model list (`.github/workflows/e2e.yml:106-148`). The
  order matters: `agy models` skips fetchAvailableModels ("Auth mode is
  unspecified") and prints an empty list when no cli settings exist yet, so
  querying before writing always yields an empty roster under a bare
  `GEMINI_API_KEY` (spike 2026-09-08, agy 1.1.26/1.1.27). With the file
  present the same key lists the full catalog, which keeps the
  `gemini-<ver>-flash-*` slug order (3.6/3.7/3.8 × high/medium/low plus
  `gemini-3.1-pro-*`, 11 rows) — there was no `gemini-flash-<ver>-low`
  rename. The step collects the **slug ids** (column 1, the only value valid
  as `--model`/`set_model`), not just the single newest display label,
  exposing them as an ordered list plus the run offset (`E2E_MODEL_ROSTER`,
  `E2E_MODEL_OFFSET=${{ github.run_number }}`). An empty roster warns and
  falls through to the fallback instead of failing the run.
- Each model-issuing test selects a model before its first prompt via
  `session/set_model` (accepted per `AGENTS.md` "Both `session/set_model` and
  `session/setConfigOption` are accepted"), so the adapter passes `--model <id>`.
  With at least two roster entries the tests select distinct models in a run.
  If the roster is empty, a test does not call `set_model`.

The existing `settings.json` fallback stays as the model-selection floor for
anything that does not pick a model explicitly. Note: `settings.json` is keyed
by display label (column 2 of `agy models`), while `--model` accepts the slug
(column 1). The configure step currently writes the label; it must continue to
do so for the fallback path, while the roster env var carries slugs. The roster
query must keep its Gemini-family guard
(`.github/workflows/e2e.yml:143-148`, `gemini-<ver>-flash-low`) so a future
non-free row is never selected.

### DP3 — discover the request-per-turn ratio before committing to a number

"Turn" ≠ "request." Each model turn may issue several `generate_content` calls
(reasoning + answer), so 3 turns are not necessarily 3 requests against the
bucket. The plan must not assume 1:1. The log capture added for the diagnosis is
also the instrument: one debug run that counts `generate_content_free_tier_requests`
credits per turn gives the true per-run cost, which determines whether rotation
across the available models is sufficient or whether more trimming (OQ2) is required.

## Risks and open questions

### OQ1 — is there a *shared* daily aggregate above the per-model limit?

The captured quota is per-project-per-model, but free keys historically also
carry a project/day aggregate in some plans. We have direct evidence only of the
per-model floor. Before trusting rotation to buy full N× headroom, run one probe:
two turns against two different models on the same day, and read whether the
second model's `429` names the same `quotaId` (per-model) or a wider
`...PerProject` bucket. If an aggregate exists, rotation degrades toward DP1
alone and the honest fix is a non-free key or fewer runs.

The same probe should include a same-base different-reasoning-effort pair
(`gemini-3.6-flash` vs `gemini-3.6-flash-low`) to determine whether the quota
meters by base model or by exact slug. If they share a bucket, rotation across
reasoning-effort variants buys nothing and the headroom multiplier is the number
of *base* model versions, not the number of slug variants.

### OQ2 — keep `multi_turn`, or accept the folded coverage?

DP1 removes a two-turn test. The residual is narrow — uninterrupted (no-load)
binding — but it is a decision to record, not a footnote. If the probe in OQ1
shows an aggregate quota, reconsider keeping `multi_turn` and instead dropping
part of `session_load`, so the 3-turn budget is spent on the strongest tests.

### OQ3 — diagnosability under rotation

With all tests pinned to one model, a failure is unambiguous. Under rotation,
a failure may be model-specific — one model misunderstands a prompt while
another does not. The existing log capture (agy logs on failure) includes the
model slug in the agy invocation, so the information is available. Tests
should also log their assigned model at the start (`eprintln!("[e2e] model:
{slug}")`) so a failure's model assignment is visible in the test output without
digging into agy logs.

### Note — pin drift, not part of this work

The captured log shows `Language server version: 1.1.27` while the workflow pins
agy `1.1.26` (`.github/workflows/e2e.yml:63`). That is the language-server
component's own version, not a binary/pin mismatch, but it is easy to misread as
one; surface it in a comment only if the pin gets touched for another reason.

## What lands with this plan

Implemented experimentally in this PR:

1. `src/e2e_tests.rs`: fold `multi_turn`'s memory assertion into
   `session_load`, delete `test_e2e_multi_turn`; add per-test model selection via
   `session/set_model` reading an env-provided roster.
2. `.github/workflows/e2e.yml`: extend the configure step to emit the ordered
   flash slug-id roster; keep the `settings.json` fallback.

Still required before this plan is complete:

3. A debug run (DP3) recording requests-per-turn, and the OQ1 probe. Until
   those establish the actual bucket boundaries and any aggregate ceiling, the
   workflow and `AGENTS.md` label rotation as experimental, this plan remains
   in `plans/`, and the TODO entry remains active. The probes may require
   changing or removing the provisional assignment.

## TODO discipline

Links `TODO.md` → this plan (`Plan: plans/e2e-quota-rotation.md`). The TODO
entry and its Next Up pointer stay until the landing change deletes them
together with the CHANGELOG entry, per `AGENTS.md`.