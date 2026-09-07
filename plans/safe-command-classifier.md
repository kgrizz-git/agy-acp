# Safe-command classifier

Plan for the `TODO.md` entry "One-and-done approval for safe commands (backlog
goal)". Written before any implementation; decision points and open questions are
collected at the end and each is flagged **(DP)** or **(OQ)** where it arises.
Every technical claim below was verified against the code it names; citations are
`file:line`.

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
- A remembered allow is honoured only when `escapes_containment(args)` is false;
  a remembered deny is applied with no containment check — correct, since a deny
  cannot widen by skipping a check that only adds prompts
  (`src/permission.rs:493-517`).
- `Cwd` is in `PATH_FIELDS` (`src/permission/path_rules.rs:148-159`), so the
  working directory of a command *is* containment-checked even today; the
  `CommandLine` string itself is opaque to the path checks (pinned by
  `src/permission/sticky_tests.rs:945-958`: `cat /etc/shadow` in a
  `CommandLine` produces no `outside_workspace` hit), which is exactly why
  keying had to carry the fingerprint. The sensitive-substring list does see
  the whole `CommandLine`, so `ls /etc/passwd` trips on `passwd` today while
  `ls /etc/shadow` does not — that is coincidence, not a control, and nothing
  below leans on it.
- `AlwaysScope` (Tool / Command / Call) is derived once in `decide` from the
  same `sticky_scope` result the key is built from, and feeds the prompt labels,
  the key, and the reason strings
  (`src/permission.rs:487-489, 550, 583, 654-658, 747-766`), so the button
  cannot underpromise what the key grants. The classifier's output has to flow
  through this same single derivation, or the label and the key can disagree
  about scope again.
- One detail that matters for the key choice below: `AlwaysScope` is `Copy` and
  `noun()` returns `Option<&'static str>` (`src/permission.rs:730, 760`), so any
  new variant that wants to carry a program name must borrow it from a static
  source or the enum stops being `Copy`.
- Host constraint already recorded in `plans/completed/permission-command-keying.md`:
  an ACP prompt may carry only **one** `allow_always` option (Paseo reclassifies
  a two-allow prompt as a chooser), and `allow_once` must never be dropped
  (Paseo auto-accept falls through to `allow_always` if it is). The classifier
  may change which "always" label appears, but may not add a second.

## Design

### Classifier shape

New module, `src/permission/safe_command.rs`, with one entry point:

```rust
/// `Some` only when the raw command line is provably a single invocation of an
/// allowlisted read-only program, with no shell interpretation in play.
fn classify(command_line: &str) -> Option<SafeCommand> { ... }

struct SafeCommand { program: &'static str, paths: Vec<PathArg> }
```

Recognition criteria, all required. **The tokenizer is a character
*allowlist*, not a metacharacter *denylist*.** A denylist — "no `;`, `&&`, `|`,
backticks, `$(...)`…" — is a hole by construction: it says nothing about zsh
here-strings (`ls <<< /etc/shadow`), zsh process substitution
(`ls =(cat /etc/shadow)`), `${=var}` word-splitting, or glob qualifiers, none of
which contain any of those reject strings, so all would classify as safe. A list
of known-bad constructs can never be complete against a shell the classifier
does not model, so it works the other way:

- Split the command line on ASCII whitespace only (`char::is_ascii_whitespace`,
  so tab and CR split just like space; a non-ASCII or NUL byte is outside the
  charset and fails the whole command). A token is valid only if every character
  comes from a small allowlisted set — alphanumerics plus `_ - . / + , : @ %`,
  with `=` and `~` admitted only under the two rules below. Anything else (`*`,
  `?`, `$`, backquote, quotes, backslash, `(`, `)`, `<`, `>`, `|`, `;`, `&`,
  `{`, `}` — and therefore every glob, expansion, redirection, chain, and
  zsh-ism without having to name them) makes the whole command unclassifiable.
  New zsh syntax fails closed by construction, because a construct zsh adds
  tomorrow contains characters this set has never heard of.
- **`=` is allowed *only* inside a `--flag=value` token** (a token that starts
  with `--` and matches an allowlisted long flag with an attached value).
  Nowhere else, because zsh expands a word that *starts* with `=` to the path of
  a command (`ls =id` → `ls /usr/bin/id`): a bare `=` in the charset is a
  read-outside-the-workspace that no path check sees — the extracted token is
  just `=id`, a harmless-looking relative name. A token starting with `=`
  (unclassifiable) and a `NAME=value` token before the program (also
  unclassifiable) are both pinned by tests.
- **`~` is allowed only as the first character of an argument token**, where it
  is home-relative and the containment check refuses it. `~` elsewhere is benign
  in zsh, but the charset keeps it out anyway so the rule has one case, not two.
- `--` (end-of-flags) is **not** given semantics: it is not a permitted flag for
  any program, so a command containing it is unclassifiable. That is the
  fail-closed default; if it is ever added, it must mean "everything after is an
  operand" and be documented as such.
- Long flags match the per-program allowlist **exactly**. GNU getopt accepts
  unambiguous prefixes (`grep --col` → `--color`/`--context` ambiguity); the
  classifier must not. An abbreviated long flag is an unknown flag, hence
  unclassifiable, and `grep --col` is a pinned test.
- Exactly one command results → one token list, no separators possible by
  construction.
- The program token is a bare name on the program allowlist: no `/` in it (so
  `./ls`, `/bin/ls`, `~/bin/ls` are out), no wrapper prefix (`sudo`, `env`,
  `time`, `nice`, `command`, `builtin`, `exec`), no environment-assignment
  prefix (a token matching `NAME=value` before the program disqualifies the
  whole line — `FOO=bar ls` is out).
- Every remaining token is either a flag the program's flag allowlist permits —
  **with a declared arity**: 0 (bare switch), or 1 with its value extracted as a
  candidate path, in all spellings (`--flag=value` attached, `--flag value`
  two-token, `-oFILE` attached, `-o FILE` two-token) — or an operand. Every
  operand is extracted as a candidate path. **A token the tokenizer cannot
  account for under one of those heads makes the whole command unclassifiable.**
  Without this, `ls --file=/etc/x` classifies while no path ever reaches the
  containment check: unknown flag, unknown arity, or an unextracted value is
  always a fail. Programs differ on whether a bare operand is a path
  (`cat`/`ls`: yes; `date`: operands are formats, and a file-reading flag
  changes that per flag — see DP1's arity table), so the per-program entry also
  declares how leftovers are treated. This accounting rule is what makes
  "classified" and "all paths judged" the same event, and the containment
  argument below depends on it.
- Every extracted candidate path, flag values included, is judged *as a path
  field, resolved against the command's `Cwd`* — not joined to the workspace
  root. The reason is in the containment section.

**PATH resolution is out of the boundary, and the plan must say so.** The
allowlist constrains the *string* the model typed; `ls` still resolves through
whatever `PATH` the agy process inherits, so "always allow `ls`" means "always
allow whatever `ls` resolves to on this machine". A user whose PATH is hostile
has larger problems, and the full fingerprint keying in place today has exactly
the same exposure. Recorded as an inherent limitation, in the README when this
lands, not as a hole this plan closes.

### The allowlist — **(DP1)**

Programs that read and cannot write or execute, with flags:

- Straightforward members: `ls`, `cat`, `head`, `tail`, `wc`, `file`, `stat`,
  `pwd`, `du`, `df`, `date`, `which`. Candidates needing the per-flag review
  before they join rather than after: `realpath`, `basename`, `dirname`.
- `grep`/`rg`-style tools: read-only, but "their flags change reach, not
  safety" is wrong as a blanket claim: `rg --pre CMD` executes a preprocessor
  on every match. They join only with a flag table that excludes the execution
  flags (`--pre`, `--pre-glob`, `--hostname-bin`…), not on the "read-only"
  intuition.
- Deliberately *out*, even though their common use looks read-only, because one
  flag or subcommand writes or executes: `find` (`-delete`, `-exec`), `sed`
  (`-i`), `awk`/`gawk` (`system()`, `print >`), `xargs`, `tee`, `truncate`,
  `cp`/`mv`/`ln`, `touch`, `dd`, `chmod`/`chown`, any shell, `python`/`node`/
  `ruby`/`perl`, `curl`/`wget` (network), `ssh`/`scp`/`rsync` (network),
  `tar` (`--to-command`, checkpoint actions; `tar tf` is tempting and stays
  out), **`less`/`more`/`most` and every pager or editor (`man`, `vi`, `nano`)**
  — `less` runs `!cmd`, editors edit — and `git` (see DP2).
- **Measured against real traffic and grants.** Full numbers, methodology, and
  the reproducibility recipe live in
  `dev-docs/investigations/safe-command-coverage.md` (2026-09-07). The
  headlines: v1 as scoped makes **10–12% of tool calls** one-and-done (11.7%
  of 844 sampled `run_command` calls, 12.1% of the 223 `command(...)` grants
  in this machine's `~/.gemini/antigravity-cli/settings.json`); `git` is the
  plurality of traffic (~27% of `run_command`, i.e. exactly what DP2 excludes
  in v1); and within the covered family the prompt reduction is large — 13
  first-time prompts would have silenced 86 of 99 calls (86.9% silent), with
  `cat` and `ls` buying most of it. Honest consequences the README must carry:
  v1 widens a minority of real-world prompts, `git` is the v2 lever (~+13
  points) and deliberately not in v1, quoting would add ~5 points for real
  tokenizer risk (OQ2 says no), and `cd` is a special case — it exits with the
  child and changes nothing lasting. One measured edge case to document
  rather than fix: `ls -la ~/.paseo` classifies through the charset but its
  `~` path is refused by containment, so tilde paths never widen even under a
  program approval — one README line, not a bug.
- Flag policy per program: **allowlist of permitted flags with declared
  arity**, not a denylist of dangerous ones. The asymmetry is the same one
  `UNKEYED_FIELDS` documents in reverse: an unknown flag should make the
  command unclassifiable (costs a prompt), because a new flag nobody classified
  must not silently widen. Short flags may bundle (`-la` iff `-l` and `-a` are
  each allowed, expanded character by character, attached-value flags like
  `-oFILE` recognized only when the table declares arity 1); long flags match
  exactly (no getopt prefix matching); a flag that takes a value is only
  allowed when its value is extracted and contained.
- **The table is per-platform, and the host platform here is Darwin.** A GNU
  `ls` table is not a BSD `ls` table (`--color` is not BSD `ls`; `-f` is a file
  argument to GNU `date` and a format string to BSD `date`). The shipped tables
  track the union conservatively — a flag allowed only when it is safe on
  *both* dialects — and the DP1 ruling names the dialect per entry.
- Starting flag table, for `ls`: `-l -a -A -h -d -F -p -R -r -S -t -1 -C
  --color --group-directories-first --time-style=STYLE` — display and ordering
  only. **Symlink-following flags (`-H`, `-L`) are excluded**, because they
  follow operands to their targets in the exact case the Cwd-relative
  canonicalize check adjudicates: `ls -L link` would dress a read of wherever
  `link` points up as an `ls` the user already approved. Symlink-following flags
  belong to no v1 program. Every entry is verified against both the GNU and BSD
  man pages before landing; each other program needs the same per-flag
  verification before it ships — the DP1 ruling *is* this table.

### What the sticky key becomes

Today `sticky_scope(tool_name, args)` returns `Some(args_fingerprint(args))` for
`run_command` because the fingerprint is the whole argument object in one
string. `sticky_scope` gains a classifier step, and it happens there — *inside*
`sticky_scope`, so the key derivation stays one function, not two places that
can drift:

1. If `tool_kind(tool_name)` is not `"execute"` or the args carry no
   `CommandLine`, today's logic is untouched. Do not classify on
   `has_command_line` alone: unknown tools carrying a `CommandLine` stay
   fingerprinted, and a successful classify must *preempt* the
   `has_unconstrained_reach` branch for `run_command` only — classifying every
   `CommandLine`-bearing tool would silently widen tools nobody audited.
2. Otherwise extract the `CommandLine` string and call `classify`. On `None`
   (unclassifiable), return `Some(args_fingerprint(args))` exactly as today —
   the fallback path is byte-identical to current behaviour.
3. On `Some(SafeCommand { program, .. })`, return `Some(format!("safe:{program}"))`.

`safe:` keys are only ever produced by step 3, so a command whose paths could
not be extracted can never reach the widened key — and that holds only because
the tokenizer's accounting rule (above) makes extraction total. `SafeCommand.
program` is `&'static str` pointing into the static allowlist entry rather than
slicing the model's input, so `AlwaysScope` stays `Copy` and the label provably
cannot carry model-authored text even if the allowlist check is ever bypassed.

`AlwaysScope` gains a `Program(&'static str)` variant, and its derivation is
fixed so label, key, and reason still come from one source. `AlwaysScope::of`
today picks `Command` whenever `has_command_line(args)` holds, which would
mislabel every classified command as "this exact command". Fix: `decide`
computes the classifier outcome once (it already needs `scope`), and
`AlwaysScope::of` takes it as input rather than re-inferring scope from the raw
key string: `Some("safe:ls")` → `Program("ls")`. `noun()` returns the
pre-formatted phrase `` `ls` commands ``, not the bare name —
`permission_options` formats `Always allow {noun} this session`
(`src/permission.rs:780-805`), so the noun itself has to carry the backticks and
the plural. **(DP3)**

Exactly one `allow_always` option is still offered; for a classified command the
program-wide label *replaces* "this exact command" rather than joining it —
offering both would violate the one-`allow_always` host constraint. The
consequence to call out in the README: for a classified command there is no way
to say "this exact string only", which is a small narrowing of choice in
exchange for one-and-done behaviour. **(DP4)**

**Denies stay narrow, and this takes three coordinated changes, not one.** A
sticky deny widened by program would let one rejection of a weird `ls`
invocation block every later `ls`, and the naive fix — split the key at the
store site — is by itself broken twice over: `decide` looks up one key
(`src/permission.rs:487-492`), so a fingerprint-keyed deny would never match
anything, and a single `noun()` shared by both buttons
(`src/permission.rs:780-789`) would print "Always reject `ls` commands" over a
store that only blocks one string. So:

1. **Store.** In `apply_outcome`, a sticky deny is keyed by
   `args_fingerprint(args)` even when the tool classified; only a sticky allow
   uses `safe:<program>`. The reject records exactly the call the user
   rejected.
2. **Lookup.** `decide` does a dual lookup: the fingerprint key for a
   remembered deny first (a repeated exact rejected command still never
   prompts), then the widened `safe:` key for a remembered allow.
3. **Labels and reasons.** `permission_options` and `apply_outcome`'s reason
   strings use mixed wording matched to what each store actually covers:
   allow-always says "`` `ls` `` commands", reject-always says "this exact
   command" — even on the same prompt for a classified command. One derivation
   (the decide-time `scope` plus classifier outcome) feeds both, so the two
   labels cannot drift from their keys.

The `debug_assert` extends on the allow path only: `Program` implies the stored
key starts with `safe:`; on the deny path a separate assert pins the key to
`args_fingerprint`. **(DP5)**

**Key-format extensibility, for DP2's future:** `safe:<program>` leaves
`safe:<program>:<subcommand>` free for a later `git status`-style scope; the
two can never collide because one always and one never contains a further `:`,
and a remembered `safe:git` cannot exist in v1 since `git` is not allowlisted.

### Containment, per matching call

Before a remembered `safe:<program>` allow is honoured, two checks run. They
are two and not one because they see different things:

1. The existing `escapes_containment(&args)` over the raw payload. This already
   catches the shapes it always could — `Cwd` is in `PATH_FIELDS`, so a bare
   `ls` with no arguments is judged by its working directory on every matching
   call. It cannot see a path *inside* `CommandLine`: `ls /etc/shadow` is one
   opaque string to it.
2. A new check, an explicit **second conjunct at the honor site** in `decide`
   (where today only check 1 runs, `src/permission.rs:509-516`): when a
   widened-key candidate survives check 1, the current call's extracted paths
   are wrapped as a fresh `PATH_FIELDS`-keyed args value — so they inherit the
   `outside_workspace` shape tests (including symlink resolution, which
   `path_field_args` already does per `src/permission/path_rules.rs:111-125`)
   and the sensitive-pattern list for free — with every relative entry joined
   **against `Cwd` before** judgment. The join is load-bearing: with
   `Cwd=<workspace>/sub` and `ls link` where `link` is a symlink out of the
   workspace, resolving `link` against the workspace root misses it while the
   shell, running from `Cwd`, hits it. Only if both checks pass does the
   remembered allow apply; either failing falls through to the full prompt
   path exactly as today. An empty extraction is only acceptable when the
   program declares that a no-operand call reads `Cwd` (which check 1 covers);
   otherwise it is unclassifiable, never vacuously contained.

Because `sticky_scope` re-classifies on every call and only emits `safe:` when
extraction succeeded, a call whose paths cannot be extracted can never reach
the widened key at all — that is what makes the pair of checks sufficient.
**(OQ1: `Cwd` stays out of the key.** The alternative — keying on
`safe:ls@<cwd>` — would add one prompt per new directory and buy nothing once
check 2 joins against `Cwd`: two contained working directories sharing one
approval is exactly as sound as tool-level keying already is for `view_file`.)

## Test plan

- Classifier unit tests, adversarial-first: `ls; rm -rf x`, `ls && rm x`,
  `ls | tee /etc/x`, ``ls `id` ``, `ls $(id)`, `ls $HOME`, `FOO=bar ls`,
  `ls;` (trailing separator), `ls > out`, `ls\; x` (escapes), quoting
  ("`ls 'my dir'`" — see OQ2), `find . -delete`, `sed -i`, `awk
  'BEGIN{system("id")}'`, `tar tf x --to-command=id`, `./ls`, `/bin/ls`,
  `sudo ls`, `env ls`, and absence of every separator in a genuinely simple
  `ls -la src/`. Plus the zsh natives the character-allowlist exists to kill by
  construction, tested explicitly so the property is pinned rather than
  inferred: `ls <<< x` (here-string), `ls =(id)` (process substitution),
  `ls ${=foo}` (word-splitting expansion), `ls *.zwc/*.old` (globs), a bare `*`
  argument, and Unicode/non-ASCII tokens.
- Adversarial flags, one per rule above: `ls =id` (zsh equals expansion),
  `ls --file=/etc/x` (arity/accounting), `rg --pre id` (execute flags are not
  "reach"), `ls -L link` / `ls -H link` (symlink-following flags out),
  `ls --` and `ls -- -l` (`--` given no semantics), `grep --col foo file`
  (exact long-flag match, not getopt prefix).
- Bridge-level: "Always allow `ls`" covers `ls src/` on the next call with no
  prompt; does *not* cover `ls /etc` (this pins that check 2 actually runs at
  the honor site — a full `decide` against a real `BridgeState` where check 1
  passes and check 2 fails, not a unit test of the classifier alone); does
  *not* cover `rm x` (program not allowlisted); the fingerprint path is
  untouched for unclassifiable commands (`cat >x` still reprompts per exact
  string).
- Deny-split: "Always reject `ls -z`" blocks only that exact invocation, and a
  later "Always allow `ls`" is neither pre-blocked nor contaminated by it.
  This fails against any implementation that keys the deny by `safe:ls` —
  including the store-only variant, because the dual lookup is asserted end to
  end.
- Non-vacuity, both directions: stub the classifier to return `None`
  unconditionally and confirm every pre-existing sticky test passes unchanged
  (fallback is today's behaviour with zero widening); and with the classifier
  live, confirm the two existing sticky tests that must change do change —
  `sticky_answers_are_not_normalized` (`ls` vs `ls ` were different keys under
  the fingerprint rule and classify identically now) and
  `the_always_options_name_the_command_for_command_tools` (pins the old "this
  exact command" wording for `ls`) — while every other pre-existing test stays
  green.
- Label/key coherence: extend the existing `debug_assert` pin to the program
  variant, and add a mixed-lineage test — a classified command's allow path
  labels "`` `ls` `` commands" while its deny path labels "this exact command",
  with the underlying stores keyed differently. This pins the three-part deny
  mechanism end to end.
- E2e (manual, Paseo, per `TODO.md`'s testing discipline): approve `ls` once,
  confirm a second `ls <other>` is silent, confirm `ls /etc` prompts, and
  confirm `ls; rm x` (if the model emits it) prompts.

## Docs and bookkeeping

- `TODO.md`: the "One-and-done approval for safe commands" entry is deleted and
  this file moves to `plans/completed/` in the last commit of the PR, with the
  `CHANGELOG.md` entry (a **Changed**/**Added** bullet — user-visible).
- `README.md`: "What 'Always' remembers" gains the program-keyed case and its
  boundary (first invocation always prompts; extracted paths re-checked per
  call; PATH resolution outside the boundary; v1 widens a minority of
  real-world commands — see the measured-grants note above — and quoting and
  `git` are out).
- `AGENTS.md`: the `sticky_scope`/`AlwaysScope` paragraph in Quirks describes
  three scopes; it must describe four when this lands, and its "Rejects narrow
  the same way" becomes false — the allow/deny asymmetry has to be stated.
- `pr_compliance_checklist.yaml`: the "Permission bridge fails closed" rule's
  `success_criteria` currently demands that a command-line key be keyed by the
  arguments and treats widening as a regression. This work widens deliberately,
  so the file is updated in the same PR: classified commands may use the
  program key *iff* their extracted paths are re-checked per call; everything
  else stays fingerprinted.
- Module placement: `permission.rs` is already the size cap's ceiling, and the
  module docs for `safe_command.rs` must follow the rule in `AGENTS.md` —
  no citing plan files from source comments. If the deferred containment-depth
  work under "Deliberately not taken: parsing what a command does" is ever
  taken (`TODO.md`), it reuses this parser rather than growing a second one;
  that belongs in the module docs.

## Decision points and open questions

- **DP1 — Allowlist membership and flag policy.** Resolved in shape: per-program
  flag *allowlist* with declared arity (unknown flag → unclassifiable), and the
  starting program list with the `ls` table above. Still open at implementation
  time: per-program verification against both GNU and BSD man pages, and the
  `grep`/`rg` ruling once their exec flags are enumerated.
- **DP2 — `git`.** `git status`/`log`/`diff`/`show` are read-only and the most
  common commands a coding agent runs (~40 of 223 grants on this machine), but
  `git` is one binary with dozens of subcommands and several write. Exclude in
  v1; the key format reserves `safe:<program>:<subcommand>` for a follow-up.
  The README carries the honest cost: v1 does not touch the most common case.
- **DP3 — Prompt wording.** Resolved: "Always allow `ls` commands this
  session", with `` `ls` commands `` as a pre-formatted `Program` noun rather
  than a bare name inside the existing `Always allow {noun}` format.
- **DP4 — One "always" option, not two.** Resolved: for classified commands the
  program-wide label *replaces* "this exact command" (the host permits one
  `allow_always` per prompt); the exact-string-only option disappears from the
  UI for those commands.
- **DP5 — Denies stay narrow.** Resolved by the three-part mechanism above:
  fingerprint-keyed store for denies, dual lookup, matched labels.
- **OQ1 — `Cwd` in the key?** Resolved: no. Check 2 joins relative extracted
  paths against `Cwd`, which closes the symlink-from-subdirectory case; two
  contained `Cwd`s sharing one approval is then no weaker than tool-level
  keying today.
- **OQ2 — Quoting.** Resolved: reject quoted or escaped tokens in v1 (fail
  closed; `ls "my folder"` keeps prompting). The measured cost is real — most
  one-off exact grants in `settings.json` involve quoting — so the README
  states it and a verified quoting subset is a plausible follow-up, not part
  of this work.
- **OQ3 — Interaction with `AGY_ACP_AUTO_ALLOW=reads`.** Out of scope: a
  classified safe command still prompts the first time; auto-allowing it before
  any prompt is a bigger behavioural step recorded here as a follow-up decision.
- **OQ4 — Seeding from agy's own grants.** Out of scope. The TODO entry "Does
  agy-acp use agy's own permission grants?" records an opt-in flag direction;
  the allowlist here is static and fork-controlled, and the seeding decision
  stands on its own.
