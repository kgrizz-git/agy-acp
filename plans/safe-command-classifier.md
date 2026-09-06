# Safe-command classifier

Plan for the `TODO.md` entry "One-and-done approval for safe commands (backlog
goal)". Written before any implementation; decision points and open questions are
collected at the end and each is flagged **(DP)** or **(OQ)** where it arises.

Revised once against an external read-only review (kilo step-3.7-flash, 2026-09-06).
Three of its findings were load-bearing and changed the plan: the tokenizer must
be a character *allowlist*, never a metacharacter denylist (zsh here-strings and
process substitution slip any reject-list); widening must apply to allows only,
with denials still keyed by the exact `args_fingerprint` — the shared `always`
map would otherwise widen rejects too; and the new `AlwaysScope::Program` variant
has to be threaded through `decide`, `noun()`, and `permission_options` as one
derivation or the label and the key can disagree. All three are resolved below in
place rather than in an appendix.

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

Recognition criteria, all required. **The tokenizer is a character
*allowlist*, not a metacharacter *denylist*.** An earlier draft of this section
listed the metacharacters to reject ("no `;`, `&&`, `|(`, backticks, `$(...)`…")
and a review showed why that shape is a hole: the list said nothing about zsh
here-strings (`ls <<< /etc/shadow`), zsh process substitution
(`ls =(cat /etc/shadow)`), `${=var}` word-splitting, or glob qualifiers — none
contain any of the rejected substrings, so all would classify as safe. A list of
known-bad constructs can never be complete against a shell the classifier does
not model, so it works the other way:

- Split the command line on unquoted ASCII whitespace only. A token is valid
  only if every character comes from a small allowlisted set — roughly
  alphanumerics plus `_ - . / ~ + , : = @ %` — with `=` and `~` further
  constrained below. Anything outside the set (`*`, `?`, `$`, backquote, quotes,
  backslash, `(`, `)`, `<`, `>`, `|`, `;`, `&`, `{`, `}` — and therefore every
  glob, expansion, redirection, chain, and zsh-ism without having to name them)
  makes the whole command unclassifiable. New zsh syntax fails closed by
  construction, because a construct zsh adds tomorrow contains characters this
  set has never heard of.
- Exactly one command results → one token list, no separators possible by
  construction.
- The program token is a bare name on the program allowlist: no `/` in it (so
  `./ls`, `/bin/ls`, `~/bin/ls` are out), no wrapper prefix (`sudo`, `env`,
  `time`, `nice`, `command`, `builtin`, `exec`), no environment-assignment
  prefix (a token matching `NAME=value` before the program disqualifies the
  whole line — `FOO=bar ls` is out).
- Every remaining token is either a flag the program's flag allowlist permits or
  an argument; every argument-valued token is extracted as a candidate path and
  handed to the containment machinery. The candidate-path test is deliberately
  the widest one this codebase already uses, not a new one: any token that
  *could* be a path (`/`-, `~`-, or `..`-shaped, plus any non-flag argument
  whatsoever for tools like `cat` whose bare arguments are filenames) is
  treated as one.

**PATH resolution is out of the boundary, and the plan must say so.** The
allowlist constrains the *string* the model typed; `ls` still resolves through
whatever `PATH` the agy process inherits, so "always allow `ls`" means "always
allow whatever `ls` resolves to on this machine". A user whose PATH is hostile
has larger problems, and the full fingerprint keying in place today has exactly
the same exposure. Recorded as an inherent limitation, in the README when this
lands, not as a hole this plan closes.

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
  Short flags may bundle (`-la` iff `-l` and `-a` are each allowed); long flags
  match the per-program allowlist exactly; a flag that takes a value is only
  allowed when its value is itself extracted and contained (a `--file=X` whose
  `X` is not checked would be a path that never reaches the containment
  machinery).
- Starting flag table so the DP1 ruling has something concrete to rule on
  (review flagged the absence of any example): for `ls`,
  `-l -a -A -h -H -L -d -F -p -R -r -S -t -1 -C --color --color=WHEN
  --group-directories-first --time-style=STYLE` — display and ordering only,
  each verified against the GNU and BSD `ls` man pages before landing; any flag
  that writes (`--output` is not a thing `ls` has, which is the point of
  verifying rather than assuming) is absent. Each other program in the list
  needs the same per-flag verification before the list ships; the DP1 ruling
  *is* this table.

### What the sticky key becomes

This is where the review found the plan under-specified; this revision spells it
out. Today `sticky_scope(tool_name, args)` returns `Some(args_fingerprint(args))`
for `run_command` because the fingerprint is the whole argument object in one
string. `sticky_scope` gains a classifier step, and it happens there — *inside*
`sticky_scope`, so the key derivation stays one function, not two places that
can drift:

1. If `tool_kind(tool_name)` is not `"execute"` or the args carry no
   `CommandLine`, today's logic is untouched.
2. Otherwise extract the `CommandLine` string and call `classify`. On `None`
   (unclassifiable), return `Some(args_fingerprint(args))` exactly as today —
   the fallback path is byte-identical to current behaviour.
3. On `Some(SafeCommand { program, .. })`, return `Some(format!("safe:{program}"))`.

`safe:` keys are only ever produced by step 3. An unknown tool kind, a
fingerprinted fallback, and any future caller that constructs `always_key` by
hand all keep producing fingerprints; a `safe:` string can only come from a
successful classification. That is the structural argument that a command whose
paths could not be extracted can never reach the widened key.

`AlwaysScope` gains a `Program(String)` variant, and its derivation is fixed so
label, key, and reason still come from one source. `AlwaysScope::of` today picks
`Command` whenever `has_command_line(args)` holds, which would mislabel every
classified command as "this exact command". Fix: `decide` computes the classifier
outcome once (it already needs `scope`), and `AlwaysScope::of` takes it as input
rather than re-inferring scope from the raw key string. `Some("safe:ls")` →
`Program("ls")`; the variant carries the program so `noun()` can produce the
pre-formatted phrase `` `ls` commands `` (not the bare name — the review noted
`permission_options` formats `Always allow {noun} this session`, so the noun
itself must be "`ls` commands" to get the intended label). `apply_outcome`'s
`debug_assert` extends: `Program` is a non-`Tool` scope, so the
`is_some()`/non-`Tool` correspondence still holds for keys, with the added pin
that a `Program` scope implies its key string starts with `safe:`. **(DP3)**

Exactly one `allow_always` option is still offered; for a classified command the
program-wide label *replaces* "this exact command" rather than joining it —
offering both would violate the one-`allow_always` host constraint. The
consequence to call out in the README: for a classified command there is no way
to say "this exact string only", which is a small narrowing of
choice in exchange for one-and-done behaviour. **(DP4)**

**Denies stay narrow — and here is the mechanism, which the first draft of this
plan asserted without supplying.** `apply_outcome` stores allows and denies in
the one `always` map under the same `always_key`, so widening the key for a
classified command would widen a "Always reject" too: one rejection of a weird
`ls` invocation would block every later `ls`, the opposite of what was promised.
The fix is a derivation split at the single store site: a sticky **deny** is
always keyed by `args_fingerprint`, never by `safe:<program>` — the reject
records exactly the call the user rejected, while the allow records the
classifier's widened key. One line of the split lands in `apply_outcome` where
the decision kind is already known, and the label still says what it does
today for the reject side ("Always reject this exact command this session") —
the reject side keeps the `Command`-scope wording even on classified commands,
which is honest because it is literally true. **(DP5: resolved this way; the
reviewer independently recommended the same split or a split map, and the
single-store-site split is the smaller change.)**

**Key-format extensibility, for DP2's future:** `safe:<program>` leaves
`safe:<program>:<subcommand>` free for a later `git status`-style scope; the two
can never collide because one always and one never contains a further `:`, and
a Remembered `safe:git` would never exist in v1 anyway since `git` is not
allowlisted.

### Containment, per matching call

Before a remembered `safe:<program>` allow is honoured, two checks run (the
review flagged this section as ambiguous — there are two, and they exist
because they see different things):

1. The existing `escapes_containment(&args)` over the raw payload. This already
   catches the workspace-judging shapes it always could — `Cwd` is in
   `PATH_FIELDS`, so a bare `ls` with no arguments is judged by its working
   directory on every matching call — and it is what guards `ls` with no
   explicit path at all. It cannot see a path *inside* `CommandLine`: the
   string `ls /etc/shadow` is one opaque value to it.
2. A new check over the **classifier-extracted paths** from the current call's
   `CommandLine`, run through the same `outside_workspace` /
   sensitive-pattern machinery. This is the one that catches `ls /etc/shadow`:
   the extraction that produced the widened key is run again on the current
   string, and only if every extracted path is contained does the remembered
   allow apply.

Because `sticky_scope` re-classifies on every call and only emits `safe:` when
extraction succeeded, a call whose paths cannot be extracted can never reach
the widened key at all — that is what makes the pair of checks sufficient: no
matching key exists for a call the second check cannot judge. **(OQ1, resolved:
`Cwd` stays out of the key. The review agreed: it is a path field, so check 1
already judges it per call, which is the same instrument that protects
tool-level keying today; `ls` with no arguments is judged by `Cwd` on every
hit, so two working directories do not share one approval in the meaningful
sense.)**

## Test plan

- Classifier unit tests, adversarial-first: `ls; rm -rf x`, `ls && rm x`,
  `ls | tee /etc/x`, ``ls `id` ``, `ls $(id)`, `ls $HOME`, `FOO=bar ls`,
  `ls;` (trailing separator), `ls > out`, `ls\\; x` (escapes), quoting
  ("`ls 'my dir'`" — see OQ2), `find . -delete`, `sed -i`, `awk
  'BEGIN{system("id")}'`, `tar tf x --to-command=id`, `./ls`, `/bin/ls`,
  `sudo ls`, `env ls`, and absence of every separator in a genuinely simple
  `ls -la src/`. Plus the zsh natives a character-allowlist exists to kill by
  construction, tested explicitly so the property is pinned rather than
  inferred: `ls <<< x` (here-string), `ls =(id)` (process substitution),
  `ls ${=foo}` (word-splitting expansion), `ls *.zwc/*.old` (globs), a bare `*`
  argument, and Unicode/non-ASCII tokens.
- Bridge-level: "Always allow `ls`" covers `ls src/` on the next call with no
  prompt; does *not* cover `ls /etc` (extracted-path re-check fires — this one
  pins that the *second* containment check actually runs, not just
  `escapes_containment` on the payload); does *not* cover `rm x` (program not
  allowlisted); the fingerprint path is untouched for unclassifiable commands
  (`cat >x` still reprompts per exact string).
- Deny-split: "Always reject `ls -z`" blocks only that exact invocation and a
  successor "Always allow `ls`" on a later call is neither pre-blocked nor
  contaminated by it. This is the test the reviewer showed was load-bearing,
  and it fails against any implementation that keys the deny by `safe:ls`.
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
  denylist. Candidates listed above; `grep`, `tar tf` need a ruling. The
  review agreed with the lean, on condition the ruling arrive with a concrete
  starting table — now sketched for `ls` above; every other program still
  needs its own per-flag verification.
- **DP2 — `git`.** `git status`/`log`/`diff`/`show` are read-only and the most
  common commands a coding agent runs, but `git` is one binary with dozens of
  subcommands, several write (`commit`, `checkout`, `clean`, `push` is also
  networked). Options: exclude `git` entirely (simplest, safest); allowlist
  *subcommands* (`safe:git:status`) with per-subcommand flag allowlists
  (costlier, but this is where most of the real ergonomic win is); punt `git`
  to a follow-up. Lean: exclude in v1, and the key format now reserves
  `safe:<program>:<subcommand>` for exactly this (the review agreed with the
  exclusion and asked that the extension shape be decided before the key
  format froze — it is, above).
- **DP3 — Prompt wording.** Resolved by review: the label is "Always allow `ls`
  commands this session" and the noun is a pre-formatted phrase (`` `ls`
  commands ``) produced by `AlwaysScope::Program`'s `noun()`, not a bare name
  handed to the existing `Always allow {noun}` format — the format would lose
  the backticks and the plural. Program name itself is from the static
  allowlist, so interpolation is safe. Review concurred with a "counter-propose
  the pre-formatted phrase" verdict, which this adopts.
- **DP4 — One "always" option, not two.** For classified commands the
  program-wide label *replaces* "this exact command" (host constraint: one
  `allow_always` per prompt), removing the exact-string option from the UI for
  those commands. Review agreed with this lean.
- **DP5 — Denies stay narrow.** Resolved by mechanism, not just intent: the
  deny is always stored under `args_fingerprint(args)` at the single store
  site in `apply_outcome`; only an allow uses `safe:<program>`. The review
  verified that without this split the shared map makes the widening apply to
  rejects too, and endorsed the split. Tested by the deny-split case above.
- **OQ1 — `Cwd` in the key?** Resolved by review in favour of the lean: per-call
  re-check, not keying on `Cwd`. `Cwd` is in `PATH_FIELDS`
  (`path_rules.rs`), so `outside_workspace` judges it on every matching call —
  including bare `ls` with no arguments — which is the same instrument that
  protects tool-level keying today. Two working directories already cannot share
  one approval in the containment-violating sense.
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
