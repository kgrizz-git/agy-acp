# Plan: Harness — plans layout, CHANGELOG style, semver

> **TODO discipline:** This plan covers *workflow* changes, not behaviour. No
> `TODO.md` entry exists for harness updates, so none is created and none is
> deleted. The only TODO-adjacent edit is the cross-link in the
> *quality-gates* entry, which already lives in the previous commit on this
> branch.

## Objective

Tighten the repo's three self-management conventions so they read as
intentional rather than grown:

1. **Plans live in three buckets.** `plans/` for in-flight, `plans/completed/`
   for landed, `plans/deferred/` for explicitly parked. Today, every plan —
   landed or not — sits in `plans/`, which makes the directory's purpose
   ambiguous. Landed plans accumulate without the same reflective
   re-reading an in-flight plan gets.
2. **CHANGELOG is bullets-only and terse.** Today it mixes paragraphs of
   explanation with bullets, has duplicate `### Fixed` and `### Maintenance`
   headings under one `## Unreleased`, and lists a "Known issues" entry that
   PR #9 already closed. A user reading the CHANGELOG should see what
   changed, not be talked through why.
3. **Semver is explicitly deferred.** The version is `0.1.0`, there are no
   tags, no release workflow, and the CHANGELOG itself says "no releases of
   its own yet." The decision is *not* "use semver", it is *not* "do not use
   semver" — it is "do not decide until there is a reason to."

The quality-gates plan from the prior commit lands alongside this one, in
the same PR. Both update `AGENTS.md` and `CHANGELOG.md`; both are
documentation-and-workflow changes that the user owns the review of.

## Why now

- The `plans/` directory is one commit away from a fourth plan. Switching
  buckets while there is one in-flight plan and three landed is a
  mechanical move; switching after ten plans have accumulated is a
  archaeological project.
- The CHANGELOG already has a structural defect (duplicated headings)
  *and* a stale "Known issues" entry. Both are part of the post-PR #9
  clean-up that should land soon, and a cleaner convention is easier to
  enforce against a fixed tree than against one that still needs a sweep.
- Semver at `0.1.0` with no tags and no release workflow is not an
  ambiguous state — the codebase has not chosen a versioning model. The
  plan records that, the options, and the trigger that would force a
  choice. Documenting an absent policy is cheaper than picking one and
  walking it back.

## What the plan touches

| Path | Change |
|---|---|
| `plans/ci-workflow.md` | Move to `plans/completed/ci-workflow.md`. Landed PR #4 (commit `959a666`). |
| `plans/fix-test-read-response-from-db.md` | Move to `plans/completed/fix-test-read-response-from-db.md`. Landed commit `661d391`. |
| `plans/permission-command-keying.md` | Move to `plans/completed/permission-command-keying.md`. Landed PR #9 (commit `f60efc9`). |
| `plans/deferred/` | New directory. Empty in this PR. The first entry lands with whichever work explicitly defers something (e.g., the harness plan's own "Open questions" below, if the user takes that route). |
| `plans/completed/` | New directory. Holds the three moved plans, plus the completed quality-gates and harness plans once their work ships. |
| `AGENTS.md` | New section: **Plans and CHANGELOG discipline** (extends the existing **Plans and TODO discipline** subsection at `AGENTS.md:20-32`). Documents the three-bucket plan layout, the CHANGELOG style, and the semver-deferred decision. |
| `CHANGELOG.md` | Two landing-time edits: collapse the duplicate headings under `## Unreleased`, delete the stale "Known issues" section (the two items it lists are both already fixed or moved into `## Unreleased` → `### Fixed`). **In this PR** the harness plan records the convention; the actual reformat happens at the next landing commit that touches CHANGELOG, since the discipline says "no doc edits without the work they describe". |
| `Cargo.toml` | No change. `version = "0.1.0"` stays. |

The discipline rule, restated: a convention's *first* example in the
CHANGELOG is the next landing commit, not this one. This PR lays the
rules in `AGENTS.md` and moves the plans; the convention's first output
is the next PR that lands a behavioural change.

## Design decisions

### D1. Three-bucket plan layout

```
plans/                   # in-flight: work currently being planned or implemented
plans/completed/         # work that has landed on main via a merged PR
plans/deferred/          # work explicitly parked, with the reason in the plan
```

- **In-flight** plans are the ones under active review or implementation.
  Their `TODO.md` entry still exists; the file is being read by whoever is
  about to do the work.
- **Completed** plans are historical record. They describe a decision that
  already shipped, kept so the next reader can find the reasoning without
  re-deriving it. They are not modified after landing except for typo fixes
  that change no meaning.
- **Deferred** plans are explicitly parked. Each carries a one-line "Why
  deferred" section so the next reader can decide whether the reason still
  applies. They are *not* deleted; a future PR may revive them.

The directory purpose is read at a glance: `ls plans/` shows what is in
flight; `ls plans/completed/` shows what is done; `ls plans/deferred/`
shows what was considered and parked.

### D2. The "completed" prefix is not added to filenames

Plan filenames keep their current names inside `plans/completed/`. The
*directory* carries the status, the *filename* keeps the topic. So
`plans/permission-command-keying.md` becomes
`plans/completed/permission-command-keying.md`, not
`plans/completed/permission-command-keying-done.md`. Reasons:

- File names are referenced from `TODO.md` as `Plan: plans/<name>.md`.
  Moving the file preserves the cross-link; renaming it would break the
  link in every TODO entry that points at a completed plan.
- The completed/deferred distinction is *first-class*; it does not need
  a redundant suffix in the filename. Three plans in `plans/completed/`
  and zero in `plans/deferred/` is a real, readable signal.

### D3. CHANGELOG style

Bullets, terse, no prose. One bullet per observable change. Categories
under each version: **Added**, **Changed**, **Fixed**, **Removed**,
**Maintenance**, in that order. No "Known issues" — that is what
`TODO.md` is for, and a Known-issue entry that has not been fixed
belongs there, not in the changelog of a release that contains the fix.

Style rules:

- One bullet per change. The bullet is a short clause ("`session/cancel`
  now kills agy's process tree" is good; "Fixed a bug where `session/cancel`
  did not properly handle the case where agy had spawned a process group"
  is bad).
- Citations go at the end of the bullet, parenthesised: `(PR #9)`,
  `(#7, 2026-08-30)`. Not in a footnote, not in a heading.
- Group bullets by category. Empty categories are omitted, not left as
  empty headings (the current `### Maintenance` under "Unreleased" is
  the bug — it has been duplicated).
- The version section header is `## <version>` (semver) or
  `## <date>` (date-stamped). Both are valid; see D4 for which this repo
  uses.
- An `## Unreleased` section is only present while a release is being
  cut. Once `## 0.2.0` lands, the `## Unreleased` section is gone.

### D4. Semver: defer the decision

The current state: `Cargo.toml` says `version = "0.1.0"`, no tags exist,
no release workflow exists, and the CHANGELOG itself says "no releases
of its own yet." This is a stable *absent* policy.

The plan explicitly does not adopt semver and explicitly does not adopt
date-stamping either. It records the three options and the trigger that
would force a choice:

1. **Stay on 0.1.0, no tags.** Continue accumulating work in
   `## Unreleased`; cut a release when there is a reason (a public
   announcement, a `cargo install` use case, an external consumer).
2. **Adopt date-stamping.** `## Unreleased` becomes `## 2026-08-31` on
   each landing commit. No semver, no tags, no release workflow. The
   CHANGELOG is the source of truth.
3. **Adopt semver.** Add a release workflow, tag each release, and move
   `Cargo.toml` per the semver rules. The first release under this
   option is `0.2.0` because `0.1.0` is the pre-policy state.

The trigger to *pick* one: a reason outside the repo's control — a
public release, a `cargo install agy-acp` instruction, a dependency
that requires `>= 0.2`. Until then, the deferred state is fine, the
work is the same, and the convention can be chosen under less time
pressure.

The plan does not pick. It records the three options and the trigger
in `AGENTS.md` so the next person to ask has the answer at hand.

### D5. No new TODO entry

This plan covers *workflow* changes, not behaviour. There is no
`TODO.md` entry to create and no `TODO.md` entry to delete. The only
TODO-adjacent edit is the cross-link on the *quality-gates* entry,
which already exists in the previous commit on this branch.

This is stated in the plan so a reviewer does not have to look for a
TODO entry that does not exist and conclude the plan is missing one.

## Step-by-step

### 1. Create the directories and move the landed plans

```bash
mkdir -p plans/completed plans/deferred
git mv plans/ci-workflow.md                     plans/completed/ci-workflow.md
git mv plans/fix-test-read-response-from-db.md  plans/completed/fix-test-read-response-from-db.md
git mv plans/permission-command-keying.md       plans/completed/permission-command-keying.md
```

`plans/deferred/` is empty in this PR; the directory is created so the
convention is enforced by the directory's presence, not by an
*ad-hoc* `mkdir` at the point of the first deferral.

### 2. Update `AGENTS.md`

Extend `### Plans and TODO discipline` (currently lines 20–32) with a
follow-on subsection `### Plans and CHANGELOG discipline` (or a
sibling top-level section; pick during implementation, the location
matters less than the content).

Content to add:

- The three-bucket rule from D1.
- The "no `completed-` filename prefix" rule from D2.
- The CHANGELOG style rules from D3 (bullets, citations, no "Known
  issues", `## Unreleased` only while a release is in flight).
- The semver deferral from D4 — the three options, the trigger, and
  the version stays at `0.1.0` until a release is cut.

The `### Plans and TODO discipline` section already in `AGENTS.md` does
not change. The new subsection is added after it. A reviewer can grep
for "discipline" and find both.

### 3. No CHANGELOG edit in this PR

Per the discipline: the first output of the new convention is the next
landing commit, not this one. The plan *records* the convention in
`AGENTS.md`; the convention's first example in the CHANGELOG lands with
the next PR that ships a behaviour change.

This is the same rule that kept the quality-gates plan from
double-landing the `WalkedToolFields` rename in its own commit. Plans
and the doc they describe are the same change.

If the user wants the CHANGELOG swept *now* — the duplicated headings
and the stale "Known issues" entry — that is a separate commit, not
this plan. It can land in this PR as a follow-on commit, but it should
be a deliberate "sweep the CHANGELOG" commit, not bundled with the
harness plan. The plan names it as a follow-up so it does not get
forgotten.

### 4. No semver change in this PR

`Cargo.toml` keeps `version = "0.1.0"`. No tag, no release workflow.
The D4 decision is *defer*.

## Verification

- `git status` after the moves: `plans/completed/` contains the three
  moved files; `plans/` contains only `quality-gates.md` and
  `harness.md` (the two plans currently in flight on the branch); an
  empty `plans/deferred/` exists.
- `grep -r "plans/ci-workflow" .` returns no hits — the file moved.
- `grep -r "plans/permission-command-keying" .` returns only
  self-references inside the plan itself.
- `grep -E "^## Unreleased$|^### Fixed$|^### Maintenance$" CHANGELOG.md`
  shows the duplicated-heading bug is still present — this PR does not
  fix it, and the verification *confirms* the bug is preserved for the
  next landing commit to fix. (The absence of a fix here is the
  discipline working.)
- `cargo build` exits 0. The plan changes are doc and file moves only;
  no code is touched.
- `cargo test` exits 0. Same reason.

## Out of scope, deliberately

- **The CHANGELOG sweep** — the duplicated headings, the stale "Known
  issues" entry. Mentioned in step 3 as a follow-up; not in this plan's
  diff because the discipline says it lands with its work, not
  alongside a workflow change.
- **A release workflow, tags, or `cargo release` config.** These are the
  artefacts of an *adopted* versioning model. The plan defers the
  decision, so it cannot adopt the artefacts.
- **Renaming plan files** — see D2. The convention is directories, not
  prefixes.
- **Removing the existing CHANGELOG `## Unreleased` content** — that
  history stays. The structural fix is in step 3's follow-up; the
  bullets rewrite is also in the follow-up.
- **A `CHANGELOG_REVIEW.md` or similar** that documents the new style
  rules in a separate file. `AGENTS.md` is the right place — it already
  documents the architecture and the discipline; the CHANGELOG style
  rules live with the rest.
- **A contributing guide** (`CONTRIBUTING.md`). The repo is a hard fork
  with a single maintainer; a separate contributing file is overhead
  that buys nothing the `AGENTS.md` discipline section doesn't already
  provide.

## Open questions for the next landing commit

These are decisions the harness plan defers because they do not need
to be made to land the workflow change:

- **What to call the `## Unreleased` section in the date-stamped
  future.** If the user picks date-stamping (option 2 in D4) the
  sections become `## YYYY-MM-DD`; the first one is the date of the
  first landing commit after this PR. Today's date is 2026-08-31.
- **Whether the `## Unreleased` section's existing prose — the
  one-paragraph summary at lines 12–18 of `CHANGELOG.md` — survives
  the bullet rewrite.** My recommendation: drop it. The bullets are
  the summary; the paragraph restates them in prose. But the user may
  want the narrative framing for the post-streaming era; that is a
  call worth making deliberately.
- **Whether the "Maintenance" heading survives in the new convention.**
  The current convention is "Added / Changed / Fixed / Maintenance";
  the proposed new convention is "Added / Changed / Fixed / Removed /
  Maintenance" in that order. The order is a small thing but it is
  the kind of thing that should be a conscious choice, not a
  copy-paste.

## Plan:host constraints the implementation has to respect

Carried over from the quality-gates plan on the same branch, because
the same repo has the same harness:

- **The TODO/CHANGELOG discipline is a hard rule.** No TODO deletions
  in this PR, no CHANGELOG edits in this PR. The plan describes both;
  the next landing commit does them.
- **No bare `cargo fmt`.** `AGENTS.md` says the repo is not
  rustfmt-clean. The AGENTS.md prose edits may produce one or two
  lines that exceed the formatter's idea of width; `rustfmt --check`
  against the touched file only, hand-fix any drift.
- **One writer to stdout.** Not relevant to this plan; recorded so
  the next reviewer does not suggest consolidating plan-writing
  paths.
- **No new `TODO.md` entries.** This is a workflow change, not
  behaviour. The D5 decision is *no entry*.
