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
