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

```
Error 429: You exceeded your current quota
quotaId: GenerateRequestsPerDayPerProjectPerModel-FreeTier
quotaMetric: generativelanguage.googleapis.com/generate_content_free_tier_requests
limit: 20, model: gemini-3.6-flash
```

Two facts matter for the design:

- **Per model.** `quotaDimensions` names `model:gemini-3.6-flash`, and
  `quotaId` says `PerProjectPerModel-FreeTier`. Distinct flash models (`3.6`,
  `3.7`, `3.8`) are separate buckets of 20/day each.
- **Per day, not per minute.** `.github/workflows/e2e.yml:121-126` explains that
  `--test-threads=1` was added for the *per-minute* free-tier Flash quota
  ("a handful of requests per minute"). That is a real but different constraint;
  this daily ceiling is what failed. Nothing serial or retried changes it.

The four e2e tests (`src/e2e_tests.rs`) issue real agy turns against the model:

| test | turns | calls the model? |
|---|---|---|
| `test_e2e_agy_acp_full_round_trip` (`:27`) | 1 | yes |
| `test_e2e_error_paths` (`:407`) | 0 | no — unknown-session and unknown-method errors are answered locally |
| `test_e2e_multi_turn` (`:286`) | 2 | yes |
| `test_e2e_session_load` (`:338`) | 2 | yes |

So a clean run is **5 model turns**, against a 20/day bucket. Three PRs in one
day (~15 turns) approached the ceiling; the run that failed crossed it. Because
all five turns currently land on the *one* model the configure step pins
(`.github/workflows/e2e.yml:80-113`), a single busy day exhausts the bucket.

## Decisions

### DP1 — reduce to three model turns by folding `multi_turn` into `session_load`

The two-turn tests overlap in exactly the assertion that costs each an extra
turn. `session_load`'s second prompt (`src/e2e_tests.rs:387`) already proves
conversation continuity through the same `--conversation` binding that
`multi_turn` exists to test (`:317-330`); `session_load` just does not assert
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
the three turns across the three flash models so one run costs each model a
single turn, and the suite survives ~20 runs/day. Rotation is a mitigation, not
a cure — it multiplies headroom by the number of flash models, which is small.

Mechanism, using an existing code path rather than new infra:

- The configure step already queries the live model list
  (`.github/workflows/e2e.yml:101-102`). Extend it to collect the **slug ids**
  (column 1, the only value valid as `--model`/`set_model`), not just the single
  newest display label, and expose them as an ordered list the tests read.
- Each model-issuing test selects a distinct model before its first prompt via
  `session/set_model` (accepted per `AGENTS.md` "Both `session/set_model` and
  `session/setConfigOption` are accepted"), so the adapter passes `--model <id>`.
- Assignment is deterministic: `full_round_trip` takes id 0 and `session_load`
  takes id 1 (or the tests round-robin an env-provided list by test index),
  so a given model is never hit twice in one run and the workload is stable.

The existing `settings.json` fallback stays as the model-selection floor for
anything that does not pick a model explicitly; the roster query above must keep
its Gemini-family guard (`.github/workflows/e2e.yml:98-99`, `gemini-<ver>-flash`)
so a future non-free row is never selected.

### DP3 — discover the request-per-turn ratio before committing to a number

"Turn" ≠ "request." Each model turn may issue several `generate_content` calls
(reasoning + answer), so 3 turns are not necessarily 3 requests against the
bucket. The plan must not assume 1:1. The log capture added for the diagnosis is
also the instrument: one debug run that counts `generate_content_free_tier_requests`
credits per turn gives the true per-run cost, which determines whether rotation
across three models is sufficient or whether more trimming (OQ2) is required.

## Risks and open questions

### OQ1 — is there a *shared* daily aggregate above the per-model limit?

The captured quota is per-project-per-model, but free keys historically also
carry a project/day aggregate in some plans. We have direct evidence only of the
per-model floor. Before trusting rotation to buy full N× headroom, run one probe:
two turns against two different models on the same day, and read whether the
second model's `429` names the same `quotaId` (per-model) or a wider
`...PerProject` bucket. If an aggregate exists, rotation degrades toward DP1
alone and the honest fix is a non-free key or fewer runs.

### OQ2 — keep `multi_turn`, or accept the folded coverage?

DP1 removes a two-turn test. The residual is narrow — uninterrupted (no-load)
binding — but it is a decision to record, not a footnote. If the probe in OQ1
shows an aggregate quota, reconsider keeping `multi_turn` and instead dropping
part of `session_load`, so the 3-turn budget is spent on the strongest tests.

### Note — pin drift, not part of this work

The captured log shows `Language server version: 1.1.27` while the workflow pins
agy `1.1.26` (`.github/workflows/e2e.yml:63`). That is the language-server
component's own version, not a binary/pin mismatch, but it is easy to misread as
one; surface it in a comment only if the pin gets touched for another reason.

## What lands with this plan

1. `src/e2e_tests.rs`: fold `multi_turn`'s memory assertion into
   `session_load`, delete `test_e2e_multi_turn`; add per-test model selection via
   `session/set_model` reading an env-provided roster.
2. `.github/workflows/e2e.yml`: extend the configure step to emit the ordered
   flash slug-id roster; keep the `settings.json` fallback.
3. A debug run (DP3) recording requests-per-turn, and the OQ1 probe, before the
   final turn/model assignment is considered settled.

## TODO discipline

Links `TODO.md` → this plan (`Plan: plans/e2e-quota-rotation.md`). The TODO
entry and its Next Up pointer stay until the landing change deletes them
together with the CHANGELOG entry, per `AGENTS.md`.