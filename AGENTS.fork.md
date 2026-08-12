# Fork-local notes (never goes upstream)

Context for agents working in **this fork**. `AGENTS.md` is upstream's file and is
shared with them; this one is ours and exists only on the `mine` branch.

> [!IMPORTANT]
> Nothing in this file may reach an upstream PR. That is handled by where it lives,
> not by luck: PR branches are cut from `upstream/main`, which has never contained
> it. Before opening any PR, confirm with
> `git diff --stat upstream/main...HEAD` — if `AGENTS.fork.md` appears, the branch
> was cut from the wrong place.
>
> `.git/hooks/pre-push` enforces this. It blocks pushing any branch other than
> `mine`/`main` that contains this file, and refuses to push to `upstream` at all.
> It lives in `.git/hooks` deliberately — that is outside the working tree, so it
> survives checking out a branch cut from `upstream/main`, which is exactly when it
> has to work. A copy is kept in `.githooks/` for reinstallation:
>
> ```bash
> cp .githooks/pre-push .git/hooks/pre-push && chmod +x .git/hooks/pre-push
> ```

## What this fork is

A fork of `hicder/agy-acp` carrying the ACP permission-prompt bridge: agy runs
headless under the adapter and headless agy cannot prompt for tool permissions, so
tool calls silently failed. The bridge routes them to the ACP host instead. See
`AGENTS.md` for how it works and the agy behaviours it is built around.

Used with Paseo, though nothing in the code is Paseo-specific — `session/request_permission`
is standard ACP and Zed implements it too. Keep it host-neutral so it stays
upstreamable.

## Related community projects

- [javimosch/agy-acp-bridge](https://github.com/javimosch/agy-acp-bridge) — ACP stdio bridge for `agy`.
- [tiezbro/paseo-agy-acp](https://github.com/tiezbro/paseo-agy-acp) — Paseo-focused ACP adapter for `agy`.

### To do

Compare these projects with this fork before porting anything. Identify ideas we
can adapt, and assess whether either is a better fit for Paseo or the broader
ACP use case. Do not assume an implementation is better without a concrete
feature, maintenance, and security comparison.

#### Investigate first

- [ ] **Permission-denial race:** verify whether a late successful provider row
  can overwrite or visually contradict an ACP rejection. If it can, retain the
  bridge's deny decision as authoritative and suppress the contradictory update.
  This is the most relevant idea from `paseo-agy-acp`.
- [ ] **Completion gating:** confirm that a turn is not completed after progress,
  idle, or tool lifecycle rows alone; require final visible assistant output after
  the last tool boundary. Add regression fixtures before changing the poller.
- [ ] **Missing streaming tool types:** assess upstream PR
  [#15](https://github.com/hicder/agy-acp/pull/15) and add fixtures for observed
  Antigravity step types before expanding narration/tool classification.

#### Consider after validation

- [ ] **Robust conversation binding:** evaluate PID-based database discovery,
  with the existing before/after database snapshot as a fallback. Upstream PR
  [#20](https://github.com/hicder/agy-acp/pull/20) has an implementation, but is
  conflicting and unreviewed; independently validate macOS behavior first.
- [ ] **More ACP configuration:** selectively expose supported `agy` options
  (mode, model, reasoning effort, and sandbox) with validation and session
  persistence. Keep `--dangerously-skip-permissions` under the bridge's own
  fail-closed permission policy rather than exposing it as an ordinary bypass
  mode.
- [ ] **Per-session workspace roots:** assess ACP `cwd` and
  `additionalDirectories` support from upstream PR
  [#18](https://github.com/hicder/agy-acp/pull/18), including its interaction
  with the private hook directory and workspace-bound read policy.
- [ ] **Provider robustness:** test newest-first protobuf field-20 extraction,
  clear surfacing of `agy` backend errors, and configurable `agy` binary paths.
  These are parts of upstream PR #20, not yet a reviewed upstream baseline.
- [ ] **PTY fallback:** reproduce the non-TTY and thinking-model failures that
  motivated `agy-acp-bridge`; only add a PTY path if current `agy` versions still
  need it and it preserves multi-session streaming and permissions.

#### Paseo-only candidates

- [ ] **Daemon context bridge:** investigate Paseo's appended system context only
  if Paseo proves it is unavailable to `agy`. Treat it as trusted host data and
  make it opt-in, observable, and isolated from general ACP hosts.
- [ ] **Paseo task/revert edge cases:** reproduce the foreground task-state and
  trailing-newline whole-file-revert issues reported by `paseo-agy-acp` before
  adopting their fixes.

#### Do not adopt as-is

- [ ] Do not replace this adapter with `agy-acp-bridge`'s single-session,
  non-streaming, unconditional permission-bypass design.
- [ ] Do not treat `paseo-agy-acp`'s direct permission-bypass mode or its
  Paseo-specific prompt injection as general ACP behavior.

## Branches

| Branch | Purpose |
|---|---|
| `main` | Clean mirror of upstream. Tracks `upstream/main`. Never commit here. |
| `mine` | Our version. Default branch of the fork. Where local-only work lives. |
| `feat/*` | One per upstream PR. **Always cut from `upstream/main`, never from `mine`.** |

Cutting a feature branch from `mine` is the one mistake that matters: the PR would
silently carry every divergent commit.

```bash
git fetch upstream
git checkout -b feat/whatever upstream/main   # for an upstream PR
git checkout mine && git merge main           # to absorb upstream into ours
```

## Local gotchas

- **Re-sign the binary after copying it.** macOS invalidates the signature on `cp`
  and SIGKILLs the result, with no useful error (exit 137):
  ```bash
  cp target/release/agy-acp ~/.local/bin/agy-acp
  codesign -f -s - ~/.local/bin/agy-acp
  ```
- **Do not name fork-local files `*.local.md`.** `~/.config/git/ignore` ignores that
  pattern globally, so such a file is silently never committed: `git status` stays
  clean and `git add -A` skips it, which reads exactly like success. This file was
  originally `AGENTS.local.md` and sat uncommitted until the pre-push hook that
  depends on it failed to fire. Hence `AGENTS.fork.md`.
- **Do not run bare `cargo fmt`.** The repo is not kept rustfmt-clean and upstream
  has no format command, so it reflows files this branch does not touch —
  `src/protobuf.rs` especially. Format specific files, or restore afterwards with
  `git checkout upstream/main -- src/protobuf.rs`.
- Paseo runs the adapter as `["agy-acp", "--permission-prompts"]` in
  `~/.paseo/config.json`. Provider command changes need a daemon restart.
- The permission flag is off by default. Without it the adapter behaves exactly as
  upstream does.

## Testing the permission flow

Unit tests cover the bridge, but the interesting failures are end-to-end and need a
real ACP client driving real agy. A scripted client that answers
`session/request_permission` is the cheapest way to exercise it.

Things worth re-checking after any change, because each one was a real bug:

- **Reject**, not just approve — the approve path looked perfect while rejection was
  broken.
- **No answer at all** — should end as a clean deny, not a failed turn.
- **A read of `.env`** with `AGY_ACP_AUTO_ALLOW=reads` — must still prompt.

`AGY_ACP_PERMISSION_TIMEOUT_SECS` exists mainly so the timeout ordering can be
tested in seconds rather than nine minutes.

## Upstreaming

Not yet filed. Open an issue before the PR: it is a large change from a stranger
that turns agy's own permission gate off, and that trade is the crux of the review,
not a footnote. Lead with it. Note also that this has been exercised against Paseo
and a scripted client, **not** Zed, which is upstream's actual target.
