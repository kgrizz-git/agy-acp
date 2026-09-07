# DP1 flag-table synthesis (Darwin v1)

**Date:** 2026-09-07
**What this is:** the ruling that resolves DP1 in `plans/safe-command-classifier.md`
into concrete per-program flag tables for the safe-command classifier. Two
independent research memos — `dp1-flag-tables-stepfun.md` and
`dp1-flag-tables-glm5.md` in this directory — were cross-compared and their
disagreements adjudicated against primary sources, then spot-verified on this
machine against the local Darwin man pages.

## Verification method

- Disagreements between the memos were resolved against the **Darwin man pages on
  this host** (macOS), checking GNU claims against man7.org where the memos were
  uncertain.
- Spot-verification after synthesis: `ls --color=when` is real on Darwin with an
  `always`/`auto`/`never` value (arity 1); `cat -l` takes an fcntl(2) lock on
  stdout; Darwin `cat` has `-e`/`-t` only, not separate `-E`/`-T`/`-A`; Darwin
  `date -f` is a format string (the GNU reads-a-file asymmetry is confirmed
  real); Darwin `stat -f` is a format string.
- Synthesis rule: a flag is IN only when at least one memo included it, the other
  showed no evidence against it, and the Darwin cross-check found nothing
  against. Unresolved items are at the end.

## Format

`arity 0` = bare switch; `arity 1` = takes a value, which the classifier must
extract. `value-is-path: yes` means the value goes through the bridge's
containment checks. Operand policy and the tokenizer rules are per
`plans/safe-command-classifier.md` (unknown flag → unclassifiable; long flags
exact-match; operands are paths unless the program entry says otherwise).

### ls

| flag | arity | value-is-path? |
|------|-------|----------------|
| `-l -a -A -h -d -F -p -R -r -S -t -1 -C -i -n -v -P` | 0 | — |
| `--color` | 1 | no |
| `--group-directories-first` | 0 | — |

Notable exclusions: `-H -L` (symlink-following), `-G` (asymmetric — Darwin "color
auto" vs GNU `--no-group`), `--time-style` (GNU-only; Darwin uses `-D format`).

### cat

| flag | arity | value-is-path? |
|------|-------|----------------|
| `-n -b -s -v -e -t -u` | 0 | — |

Notable exclusion: `-l` (fcntl lock on stdout — confirmed on Darwin).

### head

| flag | arity | value-is-path? |
|------|-------|----------------|
| `-n` | 1 | no |
| `-c` | 1 | no |
| `--lines` | 1 | no |
| `--bytes` | 1 | no |

Nothing else — Darwin `head` has no `-q`/`-v`/`-z`.

### tail

| flag | arity | value-is-path? |
|------|-------|----------------|
| `-n` | 1 | no |
| `-c` | 1 | no |
| `-q -v` | 0 | — |
| `--lines` | 1 | no |
| `--bytes` | 1 | no |
| `--quiet` `--silent` | 0 | — |

Notable exclusions: `-f -F` (not a write hazard but changes process lifetime —
blocks forever; glm's conservative argument won), `-r` (Darwin reverse order —
unresolved, low traffic), `-s` (GNU-only).

### wc

| flag | arity | value-is-path? |
|------|-------|----------------|
| `-l -w -c -m -L` | 0 | — |

`--files0-from` is GNU-only (reads a file list → path) and excluded on Darwin.

### file

| flag | arity | value-is-path? |
|------|-------|----------------|
| `-b -i -I -N -n -0 -h -k` | 0 | — |
| `--mime-type` `--mime-encoding` `--extension` `--exclude-quiet` | 0 | — |
| `-f` | 1 | yes (operand-list file) |
| `-m` | 1 | yes (alternate magic file) |

Notable exclusions: `-L` (Darwin default dereferences), `-p` (utimes "pretend
never read"), `-s` (raw block/char devices), `-z`/`-Z` (decompression path
unverified on Darwin), `-C` (writes `magic.mgc`), `-S`/`--no-sandbox`.

### stat

| flag | arity | value-is-path? |
|------|-------|----------------|
| `-l -r -s -x -F -n -q -h` | 0 | — |
| `-f` | 1 | no (format string) |
| `-t` | 1 | no (time format) |

Notable exclusion: `-L` (dereference). Cross-dialect trap documented: GNU `stat
-f` means `--file-system` — same argv, different semantics. The Darwin table is
correct for this host; a Homebrew-shadowed GNU `stat` would break it (see
unresolved #2).

### pwd

`-L -P` (arity 0). `pwd -L` prints `$PWD`; it has no operands to follow.

### du

| flag | arity | value-is-path? |
|------|-------|----------------|
| `-a -A -c -h -k -m -g -s -x -P -n` | 0 | — |
| `-d -B -I -t` | 1 | no |

Notable exclusions: `-H -L` (symlink), GNU-only `--files0-from`/`-X`.

### df

| flag | arity | value-is-path? |
|------|-------|----------------|
| `-a -c -h -H -k -m -g -b -P -i -I -l -n` | 0 | — |
| `-T` | 1 | no (filesystem type filter on Darwin) |

Notable exclusions: `-t` under any per-dialect table that can't distinguish GNU
from BSD (cross-dialect asymmetry).

### date

| flag | arity | value-is-path? |
|------|-------|----------------|
| `-u -R -j -n` | 0 | — |
| `-I` | 0 or 1 | no |
| `-z -v` | 1 | no |

`+format` operand allowed (not a path). Bare `date` without `-j` is
unclassifiable — on BSD an unflagged two-field operand can attempt to set the
clock.

### which

`-a -s` (arity 0). Operands are command names, not paths — recorded in the
classifier entry so the tokenizer does not path-check them.

### grep

| flag | arity | value-is-path? |
|------|-------|----------------|
| `-i -v -c -l -L -n -q -o -w -x -F -E -G -s -h -H -b -a -U -p` | 0 | — |
| `-e` | 1 | no |
| `-f` | 1 | yes (pattern file) |
| `-m` | 1 | no |
| `-A -B -C` | 1 | no |
| `--colour` `--color` | 0 or 1 | no |
| `--label` `--line-buffered` `--null` | 0 or 1 | no |
| `--exclude` `--exclude-dir` `--include` `--include-dir` | 1 | no |

Confirmed on both dialects: grep has no execution flags. Excluded: `-r`/`-R`
(GNU `-r` follows symlinks during recursion; BSD `-O`/`-S` do too).

### rg

| flag | arity | value-is-path? |
|------|-------|----------------|
| `-i -v -c -l -n -q -o -w -x -F -a -u -uu -uuu` | 0 | — |
| `-e` | 1 | no |
| `-t -T -g -E` | 1 | no |
| `-C -A -B -M` | 1 | no |
| `--hidden --no-ignore --smart-case --json --debug` | 0 | — |
| `--max-columns --sort --replace` | 1 | no |

Execution flags confirmed OUT by both memos and verified against `rg --help` on
this host: `--pre`, `--pre-glob`, `--hostname-bin`. Also out: `-L`/`--follow`
(symlink), `-z`/`--search-zip` (may invoke external decompressors), `-f`.

### basename / dirname

- `basename`: `-a` arity 0, `-s` arity 1 (not a path). Darwin has `-a`.
- `dirname`: no flags.

### Excluded from the v1 program allowlist

`realpath` (symlink resolution *is* the program — defeatable by design), `sort`
(`-o` writes), and `echo`/`uname`/`hostname`/`cmp`/`uniq` (not in the v1 scope
of the plan; candidates for a follow-up table, each needing the same treatment —
e.g. Darwin `echo` has only `-n`, no `-e`/`-E`).

## Unresolved — verify during implementation

1. **PATH shadowing**: Homebrew coreutils can put GNU binaries ahead of
   `/usr/bin`. The tables assume Darwin userland at resolve time. Mitigation is
   the plan's per-platform note; verify at implementation time what `PATH` the
   spawned agy actually inherits.
2. **`stat -f` under GNU shadowing**: same argv means format on Darwin and
   `--file-system` on GNU; if Homebrew `stat` wins, the table's `-f` line is
   wrong. Test in implementation against the real spawn environment.
3. **`tail -r`** (Darwin reverse order): safe-looking, glm excluded, stepfun
   silent. Omitted; add only on evidence.
4. **`tail -f/-F`**: re-include only if "blocking read" is an acceptable
   behaviour for a classified command (holds the turn open).
5. **`file -z`**: decompresses in-process vs spawning — unverified; left OUT.
6. **grep/rg table breadth**: synthesized sets are deliberately wide for Darwin;
   trim to match the traffic sample in investigations/safe-command-coverage.md
   if table size matters at implementation time.
7. **rg version drift**: `rg` flags change; the exclusion set must be re-verified
   against the installed `rg --help` in CI or at build time.
8. **`date` operand grammar**: `-v`/`+format`/positional parsing needs the
   adversary tests from the plan beyond the flag table.
9. **Bundled short flags** (`-la`, `-uu`): expanded character-by-character per
   the plan; the test matrix is enumerated in the plan, not here.
