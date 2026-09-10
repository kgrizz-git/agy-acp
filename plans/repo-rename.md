# Rename the GitHub repository

## Outcome

Make the fork visibly distinct from `hicder/agy-acp` at the repository level,
without disturbing history, the crate/binary name decision, or the fork guards.
Fold in a fork-etiquette docs pass so every user-facing surface names the fork
honestly and keeps attribution to upstream.

## Can the rename just be done on GitHub?

Yes. A GitHub repository rename is low-drama by design:

- GitHub keeps redirects from the old name for web traffic and `git` operations,
  and issues, PRs, and stars move with the repo automatically.
- It does **not** detach the repo from the fork network. GitHub still records
  this repo as a fork of `hicder/agy-acp`, PR bases still default upstream, and
  Actions/issues settings stay as they are. All the guards in AGENTS.md
  (`gh repo set-default`, the pre-push fork guard, fetch-upstream-by-URL) remain
  necessary and must be updated, not removed.
- Local clones keep working through the redirect, but every clone's `origin`
  should be pointed at the new URL so nobody depends on the redirect forever.

So the rename itself is one settings change; the work is updating every place
that pins the old name, in the same pull request, so nothing references a name
that no longer exists.

## Work

1. Pick the new name first. Constraints: clearly not `agy-acp` (that string is
   upstream's identity — crate, binary, and repo all collide with it today);
   ideally a name the future crate/binary rename from
   [fork-maintenance](fork-maintenance.md) can share, so the repo rename does
   not have to be revisited. Check the name against crates.io and GitHub for
   collisions before committing to it.
   - **Chosen: `agy-gated-acp`.** Verified free on crates.io, free as
     `kgrizz-git/agy-gated-acp`, and zero exact GitHub matches (2026-09-10).
     Note the accepted tradeoff: it retains the `agy`/`acp` tokens for
     discoverability at the cost of staying visually adjacent to upstream's
     `agy-acp`; the fork notice and H1 change carry the distinctness.
2. Rename in GitHub settings, then update the tree:
   - `.githooks/pre-push`: the three canonical remote URL forms (each with
     and without `.git`) and the denial message.
   - `AGENTS.md`: the `# agy-acp` H1 (becomes the new repo name), origin
     remote, `gh repo set-default`, fork-guard URL forms,
     and the relationship-to-upstream section.
   - `pr_compliance_checklist.yaml`: the "changes target" rule and hard-fork
     title.
   - `README.md`: the `# agy-acp` H1 (becomes the new repo name),
     install/clone URLs, badges, and any "differences from
     upstream" wording (see etiquette pass below).
   - Workflows: confirm nothing hardcodes the repo slug (`upstream-watch.yml`
     watches `hicder/agy-acp`, which is unaffected; the `upstream-watch` issue
     title is unaffected).
   - `scripts/check-upstream.sh`: unaffected (upstream slug), verify only.
   - `dev-docs/` is explicitly exempt: its `agy-acp` references name the
     binary, hook invocation, and Paseo provider command, none of which change
     in this plan, and its `hicder/agy-acp` links are historical evidence.
     Upstream-attribution wording there stays as written.
3. Fork-etiquette docs pass, same PR:
   - `README.md` currently never says it is a fork — a reader arriving from
     crates, search, or a fresh clone cannot tell. Add a short fork notice up
     top: hard fork of `hicder/agy-acp`, what this fork adds (the ACP
     permission-prompt bridge and its hardening) in one paragraph, and links
     to all three upstreams: `hicder/agy-acp` (origin), plus the assessed
     community projects `javimosch/agy-acp-bridge` and `tiezbro/paseo-agy-acp`
     with one line each on what was taken or deliberately not (mirroring the
     "Related community projects" section in AGENTS.md and the TODO.md entry
     that tracks them).
   - Sweep stale claims while there: any remaining "only gate" language must
     keep the tool-calls qualifier from the README boundary note; install and
     clone commands must use the new slug. There are currently no badges to
     update — verify that is still true rather than assuming it.
   - `AGENTS.md` relationship section: new origin slug, unchanged upstream
     instructions (fetch by URL, never re-add the remote).
   - No code changes in this PR: binary, crate, state directory
     (`~/.openab/agy-acp`), hook invocation, and Paseo provider command are all
     untouched — those belong to the crate/binary rename.
4. Local follow-through after merge: `git remote set-url origin <new-url>` in
   every clone, `gh repo set-default <new-slug>`, and re-verify a fresh clone
   pushes cleanly through the updated pre-push guard.

## Acceptance

- A fresh clone of the new URL builds, pushes a scratch branch through the
  fork guard, and opens a PR defaulting to this repo.
- No live (non-historical) reference to `kgrizz-git/agy-acp` remains outside
  frozen history: `plans/completed/` and past CHANGELOG entries keep the old
  name as record; everything else reads the new one.
- README and AGENTS.md each state the fork relationship and the fork's
  difference from upstream in their own words, with upstream attribution
  intact.

## Not in scope

The crate and binary rename (see [fork-maintenance](fork-maintenance.md)):
new repo, old binary until that plan lands. Also out of scope: leaving the
fork network (not self-serve) and enabling issues/Actions as part of this —
those are separate settings decisions.
