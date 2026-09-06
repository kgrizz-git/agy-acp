# Safe-command classifier

Initial plan for the `TODO.md` entry "One-and-done approval for safe commands
(backlog goal)". Written before any implementation; decision points and open
questions are collected at the end and each is flagged **(DP)** or **(OQ)** where
it arises.

## Objective

Let a user approve a *class* of provably-safe read-only shell commands once —
"Always allow `ls`" — instead of being reprompted for every new argument string,
which is what today's exact-fingerprint keying does to `run_command`. Reads
through path tools (`view_file`, `list_dir`, `grep_search`) already have this:
one "Always allow" covers the tool. The same listing run as `run_command "ls"`
reprompts on the next path, and `TODO.md` records this as the ergonomic target
the argument-keying work deliberately did not reach.

The scope change is real, not cosmetic: today every remembered `run_command`
answer is keyed by the full argument fingerprint (`args_fingerprint`), so this
work introduces a *new sticky scope* — keyed by program, not by exact string —
and the README, prompt wording, and cache-hit reasons all have to change with
it. Naming the scope change is part of the work, not a footnote.

## Safety invariants (the non-negotiables)

These come straight from the `TODO.md` entry and govern every decision below:

1. **Fail closed, toward prompting.** Every parse error, ambiguity, or
   unrecognized construct means "not classifiable" and falls back to *today's*
   behaviour — exact-string keying, prompt on every new variant. A miss is never
   a deny and never an allow; it is the status quo. A missed `;` must not turn
   "always allow `ls`" into "always allow `ls; rm -rf x`".
2. **Never trust the model's self-report.** The classifier reads the raw
   `CommandLine` string itself; nothing the model says about its own command
   (`toolSummary`, `toolAction`, a claimed "this is read-only") participates.
3. **The shell is zsh, not sh.** zsh was observed (`TODO.md`), and zsh has
   syntax POSIX sh does not (`<<<`, `=(...)`, `${=var}` word-splitting, glob
   qualifiers). The classifier does not parse "shell grammar"; it recognizes a
   deliberately tiny token shape and rejects everything else. "Rejects" here
   means "does not widen the key", not "blocks the command".
4. **Containment still runs, per call, on every matching invocation.** The whole
   reason tool-level keying is defensible for `view_file` is that
   `escapes_containment` and the sensitive-path list still constrain a remembered
   allow. Widened command keying is only defensible if the same holds: the
   extracted path arguments are re-checked against the workspace and the
   sensitive list on *every* cached hit, not just at approve time. An "always
   allow `ls`" that later matches `ls /etc/shadow` must prompt anew.
5. **The first invocation of a program is always shown.** The classifier widens
   what a remembered "Always" *covers*; it never auto-allows a command that was
   never prompted. This is a sticky-scope change, not an extension of
   `AGY_ACP_AUTO_ALLOW` (see OQ3).

## Where this sits in the existing design

Relevant today, from `src/permission.rs` and `src/permission/sticky_rules.rs`:

- `sticky_scope(tool_name, args)` returns `Some(args_fingerprint(args))` for any
  tool whose arguments the containment checks cannot read — every tool carrying
  a `CommandLine`, which means `run_command` always.
- A remembered allow is honoured only when `escapes_containment(args)` is false.
  `Cwd` is in `PATH_FIELDS`, so the working directory of a command *is*
  containment-checked even today; the `CommandLine` string itself is opaque to
  those checks, which is exactly why keying had to carry the fingerprint.
- `AlwaysScope` (Tool / Command / Call) is derived once in `decide` from the
  same `sticky_scope` result the key is built from, and feeds the prompt labels,
  the key, and the reason strings, so the button cannot underpromise what the
  key grants. The classifier's output has to flow through this same single
  derivation, or the label and the key can disagree about scope again.
- Host constraint already recorded in `plans/completed/permission-command-keying.md`:
  an ACP prompt may carry only **one** `allow_always` option (Paseo reclassifies
  a two-allow prompt as a chooser), and `allow_once` must never be dropped
  (Paseo auto-accept falls through to `allow_always` if it is). The classifier
  may change which "always" label appears, but may not add a second.

## Proposed design (initial; decisions below)

### Classifier shape

New module, `src/permission/safe_command.rs`, with one entry point:

```rust
/// `Some` only when the raw command line is provably a single invocation of an
/// allowlisted read-only program, with no shell interpretation in play.
fn classify(command_line: &str) -> Option<SafeCommand> { ... }

struct SafeCommand { program: &str, paths: Vec<PathArg> }
```

Recognition criteria, all required:

- Exactly one command: no `;` `&&` `||` `|` `>` `<` `(`, backticks, `$(...)`,
  `${...}`, `$VAR`, `\n`, `&`. Any of these → not classifiable.
- The program token is a bare name on the allowlist: no `/` in it (so `./ls`,
  `/bin/ls`, `~/bin/ls` are out), no wrapper prefix (`sudo`, `env`, `time`,
  `nice`, `command`, `builtin`, `exec`), no environment-assignment prefix
  (`FOO=bar ls`).
- Every remaining token is either a flag the allowlist entry permits or an
  argument; every argument-valued token is extracted as a candidate path and
  handed to the containment machinery.

### The allowlist

Programs that read and cannot write or execute, with flags: **(DP1)**

- Straightforward members: `ls`, `cat`, `head`, `tail`, `wc`, `file`, `stat`,
  `pwd`, `du`, `df`, `date`, `which`, `realpath`, `basename`, `dirname`.
- `grep`/`rg`-style tools: read-only, but flags like `-r`/`--include` change
  reach, not safety — likely in.
- Deliberately *out*, even though their common use looks read-only, because one
  flag or subcommand writes or executes: `find` (`-delete`, `-exec`), `sed`
  (`-i`), `awk`/`gawk` (`system()`, `print >`), `xargs`, `tee`, `truncate`,
  `cp`/`mv`/`ln`, `touch`, `dd`, `chmod`/`chown`, any shell, `python`/`node`/
  `ruby`/`perl`, `curl`/`wget` (network), `ssh`/`scp`/`rsync` (network),
  `tar` (`--to-command`, checkpoint actions; `tar tf` is tempting — flag),
  `git` (see DP2).
- Flag policy per program: **allowlist of permitted flags**, not a denylist of
  dangerous ones. The asymmetry is the same one `UNKEYED_FIELDS` documents in
  reverse: an unknown flag should make the command unclassifiable (costs a
  prompt), because a new flag nobody classified must not silently widen. **(DP1)**

### What the sticky key becomes

For a classified command, `sticky_scope` yields a normalized program key —
schematically `(session, "run_command", Some("safe:ls"))` — instead of the full
fingerprint. `AlwaysScope` gains a program variant; the prompt's allow-always
label becomes "Always allow `ls` commands this session" and the reason strings
match. The program name is safe to interpolate: by construction it equals an
entry in our static allowlist, not arbitrary model text. **(DP3)**

Exactly one `allow_always` option is still offered; for a classified command the
program-wide label *replaces* "this exact command" rather than joining it —
offering both would violate the one-`allow_always` host constraint. The
consequence to call out in the README: for a classified command there is no way
to say "this exact string only", which is a small narrowing of
choice in exchange for one-and-done behaviour. **(DP4)**

"Always reject" behaviour is a question: widening *denies* by program would make
one rejection of a weird `ls` block every later `ls`. Default position: denies
stay keyed by exact fingerprint; only allows widen. **(DP5)**

### Containment, per matching call

Before a remembered `safe:<program>` allow is honoured, the *current* call's
extracted paths and its `Cwd` are re-run through `outside_workspace` and the
sensitive-pattern list. The cleanest implementation is to make the unknown case
structurally impossible: the classifier is the only producer of the widened key,
so a call whose paths cannot be extracted and checked can never reach the
widened key at all. A classified command whose path arguments trip containment
does not match the remembered allow and prompts in full. **(OQ1: does `Cwd`
belong in the key itself, or is the per-call re-check sufficient? Leaning
per-call re-check — it is what protects tool-level keying today — but `ls` with
no arguments reads whatever `Cwd` names, so the argument is not frivolous.)**

## Test plan

- Classifier unit tests, adversarial-first: `ls; rm -rf x`, `ls && rm x`,
  `ls | tee /etc/x`, ``ls `id` ``, `ls $(id)`, `ls $HOME`, `FOO=bar ls`,
  `ls;` (trailing separator), `ls > out`, `ls\\; x` (escapes), quoting
  ("`ls 'my dir'`" — see OQ2), `find . -delete`, `sed -i`, `awk
  'BEGIN{system("id")}'`, `tar tf x --to-command=id`, `./ls`, `/bin/ls`,
  `sudo ls`, `env ls`, and absence of every separator in a genuinely simple
  `ls -la src/`.
- Bridge-level: "Always allow `ls`" covers `ls src/` on the next call with no
  prompt; does *not* cover `ls /etc` (containment re-check fires); does *not*
  cover `rm x` (program not allowlisted); the fingerprint path is untouched for
  unclassifiable commands (`cat >x` still reprompts per exact string).
- Non-vacuity: stub the classifier to return `None` unconditionally and confirm
  every pre-existing sticky test passes unchanged — proving the fallback really
  is today's behaviour with zero widening.
- Label/key coherence: extend the existing pin that label scope and key scope
  agree (`debug_assert` and its tests) to the program variant.
- E2e (manual, Paseo, per `TODO.md`'s testing discipline): approve `ls` once,
  confirm a second `ls <other>` is silent, confirm `ls /etc` prompts, and
  confirm `ls; rm x` (if the model emits it) prompts.

## Docs and bookkeeping

- `TODO.md`: the "One-and-done approval for safe commands" entry gains the
  `Plan:` pointer now; the entry is deleted and this file moves to
  `plans/completed/` in the last commit of the PR, with the `CHANGELOG.md`
  entry (a **Changed**/**Added** bullet — user-visible).
- `README.md`: "What 'Always' remembers" section grows the program-keyed case
  and its boundary (first invocation always prompts; paths re-checked per call).
- `AGENTS.md`: the `sticky_scope`/`AlwaysScope` paragraph in Quirks describes
  the current three scopes; it must describe four when this lands.
- The parser-hazard relationship: `TODO.md` notes this classifier shares the
  "Deliberately not taken: parsing what a command does" hazard and that "the
  parser is the same dangerous object and should be built once if built at all."
  If the deferred containment-depth work (path extraction from command lines)
  is ever taken, it must reuse this parser, not grow a second one. State that in
  the parser's module docs.

## Decision points and open questions

- **DP1 — Allowlist membership and flag policy.** Final program list; confirm
  per-program flag *allowlist* (unknown flag → unclassifiable) over a flag
  denylist. Candidates listed above; `grep`, `tar tf` need a ruling.
- **DP2 — `git`.** `git status`/`log`/`diff`/`show` are read-only and the most
  common commands a coding agent runs, but `git` is one binary with dozens of
  subcommands, several write (`commit`, `checkout`, `clean`, `push` is also
  networked). Options: exclude `git` entirely (simplest, safest); allowlist
  *subcommands* (`safe:git:status`) with per-subcommand flag allowlists
  (costlier, but this is where most of the real ergonomic win is); punt `git`
  to a follow-up. Lean: exclude in v1, design the key format so a subcommand
  scope can be added without changing existing keys.
- **DP3 — Prompt wording.** "Always allow `ls` commands this session"; confirm
  no host displays labels ambiguously, and that inter-polating the allowlisted
  (static) program name is accepted practice here.
- **DP4 — One "always" option, not two.** For classified commands the
  program-wide label *replaces* "this exact command" (host constraint: one
  `allow_always` per prompt), removing the exact-string option from the UI for
  those commands. Confirm acceptable.
- **DP5 — Denies stay narrow.** "Always reject" remains keyed by exact
  fingerprint even for classified commands, so one rejection of a strange `ls`
  invocation does not block all `ls`. Confirm.
- **OQ1 — `Cwd` in the key?** Include the working directory in the widened key
  (`safe:ls@<cwd>`), or rely on the per-call containment re-check of `Cwd` the
  way tool-level keying relies on it today? Lean: per-call re-check; the
  per-call check runs either way, so the question is only whether two different
  working directories deserve separate "Always" answers.
- **OQ2 — Quoting.** Reject any quoted or escaped token (fail closed, and `ls
  "my folder"` keeps prompting forever), or support a minimal, verified quoting
  subset? Lean: reject in v1; quoting is the easy place for a parser bug to
  hide, and the cost is a prompt per new quoted path.
- **OQ3 — Interaction with `AGY_ACP_AUTO_ALLOW=reads`.** Should a classified
  safe command auto-allow *before any prompt* when the user has already opted
  reads into auto-allow? That is a bigger behavioural step than sticky widening.
  Lean: no — out of scope for this work; record as a follow-up decision.
- **OQ4 — Seeding from agy's own grants.** `TODO.md` ("Does agy-acp use agy's
  own permission grants?") records an opt-in flag direction for honouring
  `permissions.allow`, and notes the classifier could be *seeded* from its
  `command(...)` prefixes. Treat as strictly out of scope here; the allowlist in
  this plan is static and fork-controlled. The seeding decision stands on its
  own and must not gate this work.
