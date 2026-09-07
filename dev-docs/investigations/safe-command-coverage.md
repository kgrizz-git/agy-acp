# Safe-command classifier: measured v1 coverage

**Date:** 2026-09-07
**Scope:** how much of real-world `run_command` traffic the safe-command
classifier would cover, as scoped in `plans/safe-command-classifier.md`.
Kept so the README's "v1 widens a minority of prompts" claim has auditable
numbers behind it rather than being adjective-only.

## What was measured

Two independent sources, one user (this machine), measured with a read-only
agent (opencode/mimo-v2.5-free) on 2026-09-07:

1. **`~/.gemini/antigravity-cli/settings.json`** — the 223 `command(...)` rules
   in `permissions.allow`: what this user has chosen to stop prompting for in
   native agy. A grant's *existence* says nothing about how often it fires, so
   this source measures breadth of consent, not traffic.
2. **The conversation DBs** — the 10 most recent SQLite conversation databases
   under `~/.gemini/antigravity-cli/conversations/`. Raw query:
   `SELECT step_payload FROM steps`, payload decoded as latin-1 (protobuf
   field, not UTF-8), tool calls extracted with
   `re.findall(r'"CommandLine":"((?:[^"\\]|\\.)*)"', text)`. 1,070 steps in
   total, 988 with a `toolAction`, 782 `run_command` calls among them
   (identified by the `CommandLine` JSON key — keyword-based; steps whose
   model-authored text mentions a tool's argument field are misattributed, so
   second-order numbers like view_file below are lower bounds).

## The headline table

| Metric | Count | Share |
|--------|------:|-------|
| Total steps (10 sessions) | 1,070 | — |
| Tool-call steps | 988 | 100% |
| `run_command` | 782 | 79.1% of tool calls |
| Plan-family classifiable (`ls`/`cat`/`head`/`wc` and kin, v1 rules) | 99 | **10.0% of all tool calls** |
| Already tool-level keyed today (view_file, replace_file_content, grep_search, list_dir) | ≥170 | ≥17.2% |
| Everything else (git, pipes, chains, redirects, quoted args, other programs, schedule) | ≥719 | ≥72.8% |

**v1 makes 10–12% of all tool calls one-and-done** (12.7% of `run_command`
specifically; 12.1% of the grants in `settings.json` — three views agreeing).

## What's in `run_command` in practice

| Shape | Count | % of 782 | v1 classifiable |
|-------|------:|---------:|---|
| pipe (`\|`) | 239 | 30.6% | no — charset |
| `git`/`gh` | 213 | 27.2% | no — DP2 exclusion |
| plan-family, clean | 99 | 12.7% | **yes** |
| quoted arguments | 90 | 11.5% | no — OQ2 exclusion |
| other programs (`paseo`, `ruff`, `python`, `npm`, …) | 137 | 17.5% | no — not allowlisted |
| redirect (`>`) | 34 | 4.3% | no — charset |
| chain (`&&`, `;`) | 32 | 4.1% | no — charset |

Composition of the 99 classifiable calls: `cat` 61, `ls` 26 (`-la`/` -l`
bundled short flags, relative operand paths), `wc` 10 (`-l`), `head` 2 (`-n
N`). No zero-argument calls occurred. Every one passed the v1 character set and
flag tables as written in the plan.

## What one "Always allow" actually buys

The point of the feature is prompt *reduction*, not call coverage, and the
repeat structure decides it. Counting distinct `(session, program)` first
occurrences against later uses of the same program in the same session:

| Program | Calls | First prompts | Silent repeats | Silent % |
|---------|------:|--------------:|---------------:|---------:|
| cat | 61 | 7 | 54 | 88.5% |
| ls | 26 | 3 | 23 | 88.5% |
| wc | 10 | 2 | 8 | 80.0% |
| head | 2 | 1 | 1 | 50.0% |
| **total** | **99** | **13** | **86** | **86.9%** |

**13 prompts across 10 sessions would have silenced 86 calls** — roughly a 6.7×
reduction in prompting for the covered programs. `cat` and `ls` alone are 87
of the 99.

## What would move the headline number, and the recommendation

Estimated coverage if scope were widened:

| Scenario | Share of all tool calls |
|----------|------------------------:|
| v1 as scoped | ~10% |
| v1 + quoting support (OQ2) | ~14.9% |
| v1 + scoped `git status`/`diff`/`log`/`show` subcommands | ~23% |
| v1 + both | ~28% |

The measurement supports leaving both where the plan puts them:

- **`git` is the largest single lever (≈+13 points), but it is v2 work.** Done
  right it needs per-subcommand flag rulings and a carve-out between
  `git diff --stat` and `git diff --word-diff`; the key format already
  reserves `safe:<program>:<subcommand>` for it. A 23%-coverage v1 is not
  worth one wrong subcommand ruling.
- **Quoting buys ~5 points for real tokenizer risk.** Allowing `"`/`'` into
  the charset means parsing quoting and escaping correctly under zsh, which is
  exactly the shape of parser the plan's fail-closed rules exist to avoid
  shipping half-verified. Follow-up, not v1.

## Edge case worth saying out loud

A tilde path classifies through the charset (`ls -la ~/.paseo`) and is then
refused by containment as home-relative — correctly, but it means `ls ~`
commands never widen even after the user approved `ls`. Documented behaviour,
one README line, not a bug.

## Reproducibility

Every number above traces to an ad-hoc read-only sqlite3/python3 query of the
same shape as the extraction shown under "What was measured". The sample is
one user's 10 most recent sessions and one settings.json; percentages should
be read as "on this machine", not as population statistics. Re-run the queries
to refresh rather than editing the numbers in place.
