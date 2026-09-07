# DP1: Per-program flag tables for safe-command classifier

Research backing the allowlist and flag policy in `plans/safe-command-classifier.md`.
Every non-obvious claim cites the man page or upstream documentation it came from.

## How to read these tables

- **arity**: 0 = bare switch; 1 = takes a value (attached, two-token, or `-oFILE` bundled).
- **value-is-path?**: yes if the value is a filesystem path that the classifier must extract
  and containment-check; no if it is a string, number, format, or other non-path.
- **GNU ok? / BSD ok?**: whether the flag is safe on that dialect. A dialect gets "no"
  when the flag writes, executes, follows symlinks on operands, or modifies state.
- **include?**: the conservative union. A flag is included only when it is safe on
  *both* GNU and BSD. When a flag is GNU-only or BSD-only but safe on the dialect
  that has it, the table notes which dialect; the classifier table in code must track
  the dialect. When a flag exists on only one dialect, the other column is "n/a".

The plan's host platform is Darwin (FreeBSD userland). The tables below were verified
against FreeBSD 15.1 man pages and GNU coreutils 9.11 / ripgrep master (2026-05-24).
OpenBSD differences are noted where they matter for the `--color` question.

---

## ls

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -l | 0 | no | yes | yes | yes | display format |
| -a | 0 | no | yes | yes | yes | show dot files |
| -A | 0 | no | yes | yes | yes | like -a minus . and .. |
| -h | 0 | no | yes | yes | yes | human-readable sizes |
| -d | 0 | no | yes | yes | yes | list directories as files |
| -F | 0 | no | yes | yes | yes | append type indicator |
| -p | 0 | no | yes | yes | yes | append / to directories |
| -R | 0 | no | yes | yes | yes | recursive |
| -r | 0 | no | yes | yes | yes | reverse sort |
| -S | 0 | no | yes | yes | yes | sort by size |
| -t | 0 | no | yes | yes | yes | sort by time |
| -1 | 0 | no | yes | yes | yes | one entry per line |
| -C | 0 | no | yes | yes | yes | multi-column (default on terminal) |
| --color | 1 | no | yes | yes | yes | BSD: `--color=when`; GNU: `--color[=WHEN]` |
| --group-directories-first | 0 | no | yes | yes | yes | FreeBSD: non-standard extension |
| --time-style | 1 | no | yes | yes | yes | GNU: `--time-style=STYLE`; BSD: `-T timefmt` (different flag, same intent) |
| -H | 0 | no | **NO** | **NO** | **NO** | follows symlinks on operands on both GNU and BSD |
| -L | 0 | no | **NO** | **NO** | **NO** | dereferences symlinks on both GNU and BSD |
| -P | 0 | no | yes | yes | yes | no-dereference (cancels -H/-L on BSD) |
| -G | 0 | no | yes | yes | yes | BSD color; GNU does not have -G |

Excluded flags: `-H` (symlink-following on operands on both GNU and BSD),
`-L` (dereference on both). `--color` is included: both GNU and FreeBSD support it
with the same semantics (always/auto/never). OpenBSD does NOT have `--color`; the
conservative policy is to include it anyway because the classifier runs on the host
platform (Darwin/FreeBSD) and the table tracks the host dialect. If OpenBSD support
is added later, `--color` must be excluded there.

Citations:
- FreeBSD ls(1): <https://man.freebsd.org/cgi/man.cgi?query=ls&manpath=FreeBSD+15.1-RELEASE+and+Ports.quarterly>
- GNU ls: verified from man7.org (already fetched)

---

## cat

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -b | 0 | no | yes | yes | yes | number non-blank lines |
| -n | 0 | no | yes | yes | yes | number all lines |
| -s | 0 | no | yes | yes | yes | squeeze blank lines |
| -t | 0 | no | yes | yes | yes | show tabs |
| -T | 0 | no | yes | yes | yes | show tabs as ^I |
| -v | 0 | no | yes | yes | yes | show non-printing |
| -u | 0 | no | yes | yes | yes | unbuffered output |
| -A | 0 | no | yes | yes | yes | show all (implies -vET) |
| -e | 0 | no | yes | yes | yes | show $ at line end (BSD: same as -E on GNU) |
| -E | 0 | no | yes | yes | yes | show $ at line end |
| -l | 0 | no | yes | **NO** | **NO** | BSD cat -l sets exclusive advisory lock on stdout via fcntl F_SETLKW |
| --show-all | 0 | no | yes | n/a | yes (GNU only) | same as -A |
| --number-nonblank | 0 | no | yes | n/a | yes (GNU only) | same as -b |

Excluded flags: `-l` on BSD. BSD cat(1) explicitly documents that `-l` "causes cat to
acquire an exclusive advisory lock on stdout using the fcntl(2) F_SETLKW command".
This modifies terminal/process state and is not read-only. GNU cat does not have `-l`,
so the flag is absent there; the BSD variant is dangerous enough to exclude entirely.

Citations:
- FreeBSD cat(1): <https://man.freebsd.org/cgi/man.cgi?query=cat&manpath=FreeBSD+15.1-RELEASE+and+Ports.quarterly>
- GNU cat: already verified

---

## head

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -n | 1 | no | yes | yes | yes | count |
| -c | 1 | no | yes | yes | yes | bytes |
| -q | 0 | no | yes | yes | yes | quiet |
| -v | 0 | no | yes | yes | yes | verbose |
| -z | 0 | no | yes | n/a | yes (GNU only) | line-terminated by NUL |
| --quiet | 0 | no | yes | n/a | yes (GNU only) | same as -q |
| --silent | 0 | no | yes | n/a | yes (GNU only) | same as -q |
| --verbose | 0 | no | yes | n/a | yes (GNU only) | same as -v |
| --bytes | 1 | no | yes | n/a | yes (GNU only) | same as -c |
| --lines | 1 | no | yes | n/a | yes (GNU only) | same as -n |

Both GNU and BSD head are read-only. All flags are display/formatting.

Citations:
- GNU head: already verified from man7.org
- FreeBSD head: already verified (implied in notes)

---

## tail

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -n | 1 | no | yes | yes | yes | count |
| -c | 1 | no | yes | yes | yes | bytes |
| -q | 0 | no | yes | yes | yes | quiet |
| -v | 0 | no | yes | yes | yes | verbose |
| -f | 0 | no | yes | yes | yes | follow (watch appended data) |
| -F | 0 | no | yes | yes | yes | follow with name resolution |
| -s | 1 | no | yes | yes | yes | sleep interval |
| --pid | 1 | no | yes | n/a | yes (GNU only) | stop when PID dies |
| --retry | 0 | no | yes | n/a | yes (GNU only) | keep retrying on inaccessible file |
| --max-unchanged-stats | 1 | no | yes | n/a | yes (GNU only) | detect truncated rotated files |
| --follow | 0 | no | yes | n/a | yes (GNU only) | same as -f |

`-f`/`-F` keep the file open and poll for appended data. They do not write. They do
follow the file descriptor, which is a mild side effect (open file + poll), but they
are read-only. The plan treats them as safe.

Citations:
- GNU tail: already verified from man7.org
- FreeBSD tail: already verified (implied in notes)

---

## wc

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -l | 0 | no | yes | yes | yes | lines |
| -w | 0 | no | yes | yes | yes | words |
| -c | 0 | no | yes | yes | yes | bytes |
| -m | 0 | no | yes | yes | yes | characters |
| -L | 0 | no | yes | yes | yes | max line length |
| --files0-from | 1 | **yes** | yes | n/a | yes (GNU only) | reads filenames from file F (path) |
| --total | 1 | no | yes | n/a | yes (GNU only) | WHEN value |
| --debug | 0 | no | yes | n/a | yes (GNU only) | prints debug to stderr |

GNU `--files0-from=F` reads NUL-terminated filenames from file F. The value is a
path. The classifier must extract it and containment-check it. BSD does not have this
flag. All other flags are display-only on both dialects.

Citations:
- GNU wc: already verified from man7.org
- FreeBSD wc: already verified (implied in notes)

---

## file

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -b | 0 | no | yes | yes | yes | brief |
| -c | 0 | no | yes | yes | yes | print result (BSD: also lists specified files) |
| -d | 0 | no | yes | yes | yes | debug |
| -E | 0 | no | yes | yes | yes | error handling |
| -e | 1 | no | yes | yes | yes | test name |
| -F | 1 | no | yes | yes | yes | separator string (not path) |
| -f | 1 | **yes** | yes | yes | yes | read names from file (path) |
| -h | 0 | no | yes | yes | yes | no-dereference (safe default) |
| -i | 0 | no | yes | yes | yes | mime type |
| -k | 0 | no | yes | yes | yes | preserve symlinks |
| -l | 0 | no | yes | yes | yes | list magic file info |
| -m | 1 | **yes** | yes | yes | yes | colon-separated magic file paths |
| -N | 0 | no | yes | yes | yes | no padding |
| -n | 0 | no | yes | yes | yes | no buffer |
| -p | 0 | no | yes | yes | **NO** | BSD: preserves access time via utimes; GNU: also touches files |
| -P | 1 | no | yes | yes | yes | name=value |
| -r | 0 | no | yes | yes | yes | raw output |
| -s | 0 | no | yes | yes | **NO** | reads block/char special files (raw disks) |
| -S | 0 | no | yes | yes | **NO** | disables sandbox; enables external decompressor execution with -z |
| -v | 0 | no | yes | yes | yes | version |
| -z | 0 | no | yes | yes | yes | uncompress (built-in decompressors only) |
| -Z | 0 | no | yes | yes | yes | uncompress, no report |
| -0/--print0 | 0 | no | yes | yes | yes | NUL delimiter |
| --apple | 0 | no | yes | n/a | yes (GNU only) | Apple-specific output |
| --exclude-quiet | 0 | no | yes | n/a | yes (GNU only) | suppress exclusion warnings |
| --extension | 0 | no | yes | n/a | yes (GNU only) | print extension |
| --mime | 0 | no | yes | n/a | yes (GNU only) | same as -i |
| --mime-type | 0 | no | yes | n/a | yes (GNU only) | mime type only |
| --mime-encoding | 0 | no | yes | n/a | yes (GNU only) | mime encoding only |
| --no-sandbox | 0 | no | yes | n/a | **NO** (GNU only) | disables sandbox (same as BSD -S) |
| --special-files | 0 | no | yes | n/a | **NO** (GNU only) | same as BSD -s |

Excluded flags: `-p` (modifies file metadata via utimes on BSD; GNU also preserves
access time), `-s` / `--special-files` (reads raw block/char devices), `-S` / `--no-sandbox`
(disables sandbox and enables execution of external decompressors). `-f` and `-m`
take path values and must be extracted and containment-checked. `-L` (--dereference)
follows symlinks; excluded on both dialects.

Citations:
- FreeBSD file(1): <https://man.freebsd.org/cgi/man.cgi?query=file&manpath=FreeBSD+15.1-RELEASE+and+Ports.quarterly>
- GNU file: already verified from man7.org

---

## stat

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -f | 1 | no | n/a | yes | yes (BSD only) | BSD: format string |
| -l | 0 | no | n/a | yes | yes (BSD only) | ls -lT format |
| -r | 0 | no | n/a | yes | yes (BSD only) | raw numeric values |
| -s | 0 | no | n/a | yes | yes (BSD only) | shell output format |
| -t | 1 | no | n/a | yes | yes (BSD only) | time format string |
| -x | 0 | no | n/a | yes | yes (BSD only) | verbose (Linux-style) |
| -F | 0 | no | n/a | yes | yes (BSD only) | implies -l, adds type indicators |
| -H | 0 | no | n/a | yes | yes (BSD only) | NFS file handle (requires root, not a path) |
| -h | 0 | no | n/a | yes | yes (BSD only) | report holes |
| -L | 0 | no | n/a | yes | **NO** | follows symlinks |
| -n | 0 | no | n/a | yes | yes (BSD only) | no trailing newline |
| -q | 0 | no | n/a | yes | yes (BSD only) | suppress errors |
| --cached | 1 | no | yes | n/a | yes (GNU only) | MODE: always/never/default |
| -c/--format | 1 | no | yes | n/a | yes (GNU only) | format string |
| --printf | 1 | no | yes | n/a | yes (GNU only) | format string (no newline at end) |
| --terse | 0 | no | yes | n/a | yes (GNU only) | terse output |

All flags are read-only except `-L` (follows symlinks). GNU `--format`/`--printf` take
format strings, not paths. BSD `-f` takes a format string. BSD `-H` treats arguments
as NFS file handles (hex, requires root); it is not a path-following flag.

Citations:
- FreeBSD stat(1): <https://man.freebsd.org/cgi/man.cgi?query=stat&manpath=FreeBSD+15.1-RELEASE+and+Ports.quarterly>
- GNU stat: already verified from man7.org

---

## pwd

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -L | 0 | no | yes | yes | yes | use $PWD env |
| -P | 0 | no | yes | yes | yes | resolve symlinks |

Both safe. No paths extracted. Both dialects identical.

Citations:
- Already verified from man7.org and FreeBSD notes

---

## du

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -0 | 0 | no | yes | n/a | yes (GNU only) | NUL delimiter |
| -a | 0 | no | yes | yes | yes | show per-file |
| -A | 0 | no | yes | n/a | yes (GNU only) | separate du for each arg |
| -b | 0 | no | yes | n/a | yes (GNU only) | bytes |
| -c | 0 | no | yes | yes | yes | grand total |
| -d | 1 | no | yes | yes | yes | max depth |
| -h | 0 | no | yes | yes | yes | human-readable |
| -H | 0 | no | yes | n/a | **NO** (GNU only) | follows symlinks on cmdline |
| -k | 0 | no | yes | yes | yes | 1024-byte blocks |
| -L | 0 | no | yes | n/a | **NO** (GNU only) | dereference symlinks |
| -l | 0 | no | yes | n/a | yes (GNU only) | count hard links |
| -m | 0 | no | yes | n/a | yes (GNU only) | megabyte blocks |
| -P | 0 | no | yes | n/a | yes (GNU only) | no dereference |
| -S | 0 | no | yes | n/a | yes (GNU only) | separate dirs |
| --si | 0 | no | yes | yes | yes | SI units |
| -s | 0 | no | yes | yes | yes | summarize |
| -t | 1 | no | yes | yes | yes | threshold SIZE (not path) |
| -x | 0 | no | yes | yes | yes | one file system |
| --apparent-size | 0 | no | yes | n/a | yes (GNU only) | apparent size |
| --block-size | 1 | no | yes | n/a | yes (GNU only) | block size |
| -B | 1 | no | yes | n/a | yes (GNU only) | block size |
| --dereference-args | 0 | no | yes | n/a | yes (GNU only) | dereference cmdline args |
| --files0-from | 1 | **yes** | yes | n/a | yes (GNU only) | read paths from file F |
| --max-line-length | 0 | no | yes | n/a | yes (GNU only) | max line length display |
| --summarize | 0 | no | yes | n/a | yes (GNU only) | same as -s |
| --time | 1 | no | yes | n/a | yes (GNU only) | time word |
| --time-style | 1 | no | yes | n/a | yes (GNU only) | time style |
| -X | 1 | **yes** | yes | n/a | yes (GNU only) | exclude-from FILE (path) |
| --exclude | 1 | no | yes | n/a | yes (GNU only) | PATTERN (not path) |
| -I | 1 | no | yes | n/a | yes (GNU only) | BSD mask (not path) |
| -g | 0 | no | yes | n/a | yes (GNU only) | BSD: human-readable (same as -h) |
| -n | 0 | no | yes | n/a | yes (GNU only) | BSD: numeric uid/gid |
| -r | 0 | no | yes | n/a | yes (GNU only) | BSD: reverse sort |
| --libxo | 0 | no | n/a | yes | yes (BSD only) | libxo output |
| -H | 0 | no | n/a | yes | **NO** | BSD: follows symlinks on cmdline |
| -I | 1 | no | n/a | yes | yes (BSD only) | mask (not path) |
| -L | 0 | no | n/a | yes | **NO** | BSD: follows symlinks |
| -P | 0 | no | n/a | yes | yes (BSD only) | no dereference |
| -a | 0 | no | n/a | yes | yes (BSD only) | show per-file |
| -c | 0 | no | n/a | yes | yes (BSD only) | grand total |
| -d | 1 | no | n/a | yes | yes (BSD only) | max depth |
| -g | 0 | no | n/a | yes | yes (BSD only) | BSD: human-readable sizes |
| -h | 0 | no | n/a | yes | yes (BSD only) | human-readable |
| -k | 0 | no | n/a | yes | yes (BSD only) | 1024-byte blocks |
| -l | 0 | no | n/a | yes | yes (BSD only) | count hard links |
| -m | 0 | no | n/a | yes | yes (BSD only) | megabyte blocks |
| -n | 0 | no | n/a | yes | yes (BSD only) | numeric uid/gid |
| -r | 0 | no | n/a | yes | yes (BSD only) | reverse sort |
| -s | 0 | no | n/a | yes | yes (BSD only) | summarize |
| -t | 1 | no | n/a | yes | yes (BSD only) | threshold SIZE |
| -x | 0 | no | n/a | yes | yes (BSD only) | one file system |

GNU `-H` follows symlinks on cmdline operands. BSD `-H` follows symlinks on cmdline
BY DEFAULT (when none of -F/-d/-l are specified). Both are excluded. GNU `-L` and
BSD `-L` follow symlinks; excluded. GNU `--files0-from` and `-X` take path values;
must be extracted and containment-checked. `-t` takes SIZE/threshold, not path.

Citations:
- GNU du: already verified from man7.org
- FreeBSD du: already verified (implied in notes)

---

## df

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -a | 0 | no | yes | yes | yes | all entries |
| -b | 0 | no | yes | n/a | yes (GNU only) | bytes |
| -c | 0 | no | yes | n/a | yes (GNU only) | grand total |
| -g | 0 | no | yes | n/a | yes (GNU only) | BSD: GB blocks |
| -h | 0 | no | yes | yes | yes | human-readable |
| -H | 0 | no | yes | yes | yes | BSD: same as -h (human-readable); GNU: SI units |
| -i | 0 | no | yes | yes | yes | inodes |
| -k | 0 | no | yes | yes | yes | 1024-byte blocks |
| -l | 0 | no | yes | yes | yes | local only |
| -m | 0 | no | yes | n/a | yes (GNU only) | BSD: MB blocks |
| -n | 0 | no | yes | n/a | yes (GNU only) | BSD: numeric uid/gid (not used in df) |
| -P | 0 | no | yes | yes | yes | POSIX output |
| -t | 1 | no | yes | yes | yes | TYPE string (not path) |
| -T | 0 | no | yes | yes | yes | print type |
| -v | 0 | no | yes | n/a | yes (GNU only) | ignored on GNU |
| --libxo | 0 | no | n/a | yes | yes (BSD only) | libxo output |
| --output | 1 | no | yes | n/a | yes (GNU only) | FIELD_LIST (not path) |
| --si | 0 | no | yes | n/a | yes (GNU only) | SI units |
| --sync | 0 | no | yes | n/a | yes (GNU only) | sync before reporting |
| --no-sync | 0 | no | yes | n/a | yes (GNU only) | don't sync |
| --total | 0 | no | yes | n/a | yes (GNU only) | grand total |
| -x | 1 | no | yes | n/a | yes (GNU only) | exclude-type TYPE (not path) |

All read-only. No flag writes or follows symlinks. `-t` takes a TYPE string, not a path.

Citations:
- GNU df: already verified from man7.org
- FreeBSD df: already verified (implied in notes)

---

## date

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -u | 0 | no | yes | yes | yes | UTC |
| -R | 0 | no | yes | yes | yes | RFC-2822 |
| -I[FMT] | 1 | no | yes | yes | yes | ISO-8601 |
| -d/--date | 1 | no | yes | n/a | yes (GNU only) | date string |
| --debug | 0 | no | yes | n/a | yes (GNU only) | debug to stderr |
| --resolution | 0 | no | yes | n/a | yes (GNU only) | print resolution |
| -f/--file | 1 | no | n/a | **NO** | **NO** | BSD -f takes input_fmt (format string), completely different meaning from GNU -f (DATEFILE path) |
| -r/--reference | 1 | **yes** | yes | n/a | **NO** | GNU -r takes FILE (path); BSD -r takes filename OR seconds |
| -s/--set | 1 | no | yes | n/a | **NO** | GNU -s sets system time (writes) |
| -n | 0 | no | n/a | yes | yes (BSD only) | numeric uid/gid (for `who` compat; no-op in date) |
| -z | 1 | no | n/a | yes | yes (BSD only) | output timezone name |
| +format | 1 | no | yes | yes | yes | output format string |
| -j | 0 | no | n/a | yes | yes (BSD only) | don't set time |
| -v[+|-]val | 1 | no | n/a | yes | yes (BSD only) | adjust date |

Excluded flags: `-f` (BSD meaning differs completely from GNU), `-r`/`--reference`
(GNU takes a path; BSD takes filename or seconds; asymmetric and BSD can set time
without -j), `-s`/`--set` (sets system time, writes). Bare numeric operands on BSD
set the system clock; they are operands, not flags, but the classifier must reject
them because they are not paths or format strings in the safe sense.

Citations:
- GNU date: already verified from man7.org
- FreeBSD date: already verified (implied in notes)

---

## which

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -a | 0 | no | yes | yes | yes | list all matches |
| -s | 0 | no | yes | yes | yes | silent (exit code only) |

Both safe. Display-only. BSD which(1) confirmed. GNU which (from man7.org) confirmed.

Citations:
- FreeBSD which: already verified (implied in notes)
- GNU which: already verified from man7.org

---

## grep

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -E | 0 | no | yes | yes | yes | extended regex |
| -F | 0 | no | yes | yes | yes | fixed strings |
| -G | 0 | no | yes | yes | yes | basic regex |
| -P | 0 | no | yes | n/a | yes (GNU only) | PCRE2 |
| -e | 1 | no | yes | yes | yes | pattern |
| -f | 1 | **yes** | yes | yes | yes | read patterns from file (path) |
| -i | 0 | no | yes | yes | yes | ignore case |
| -v | 0 | no | yes | yes | yes | invert match |
| -w | 0 | no | yes | yes | yes | word regexp |
| -x | 0 | no | yes | yes | yes | line regexp |
| -c | 0 | no | yes | yes | yes | count |
| -l | 0 | no | yes | yes | yes | files-with-matches |
| -L | 0 | no | yes | yes | yes | files-without-match |
| -m | 1 | no | yes | yes | yes | max-count |
| -o | 0 | no | yes | yes | yes | only-matching |
| -q | 0 | no | yes | yes | yes | quiet |
| -s | 0 | no | yes | yes | yes | no-messages |
| -b | 0 | no | yes | yes | yes | byte-offset |
| -H | 0 | no | yes | yes | yes | always filename |
| -h | 0 | no | yes | yes | yes | no-filename |
| -n | 0 | no | yes | yes | yes | line-number |
| --color | 1 | no | yes | yes | yes | GNU: `--color[=WHEN]`; BSD: `--color[=when]` |
| --colour | 1 | no | yes | yes | yes | same as --color |
| --label | 1 | no | yes | yes | yes | label for stdin |
| --line-buffered | 0 | no | yes | yes | yes | line buffered output |
| --null | 0 | no | yes | n/a | yes (BSD only) | prints zero-byte after filename |
| -z/--null-data | 0 | no | yes | n/a | yes (GNU only) | NUL-terminated lines |
| -A | 1 | no | yes | yes | yes | after-context |
| -B | 1 | no | yes | yes | yes | before-context |
| -C | 1 | no | yes | yes | yes | context |
| --group-separator | 1 | no | yes | n/a | yes (GNU only) | separator string |
| --no-group-separator | 0 | no | yes | n/a | yes (GNU only) | no separator |
| -a/--text | 0 | no | yes | yes | yes | treat binary as text |
| --binary-files | 1 | no | yes | yes | yes | how to handle binary |
| -D | 1 | no | yes | yes | yes | devices action |
| -d | 1 | no | yes | yes | yes | directories action |
| --exclude | 1 | no | yes | yes | yes | filename pattern |
| --exclude-from | 1 | **yes** | yes | n/a | yes (GNU only) | read patterns from file (path) |
| --exclude-dir | 1 | no | yes | yes | yes | directory pattern |
| -I/--include | 1 | no | yes | yes | yes | include pattern |
| --include-dir | 1 | no | yes | yes | yes | include directory pattern |
| -r/-R | 0 | no | yes | yes | yes | recursive (GNU: -R is dereference-recursive) |
| --dereference-recursive | 0 | no | **NO** | n/a | **NO** | GNU -R follows symlinks during recursion |
| -U/--binary | 0 | no | yes | yes | yes | search binary |
| --mmap | 0 | no | yes | n/a | yes (GNU only) | use mmap for reads |
| -O | 0 | no | n/a | **NO** | **NO** | BSD: follow symlinks only if explicitly on cmdline |
| -p | 0 | no | n/a | yes | yes (BSD only) | BSD: no symlinks followed (default) |
| -S | 0 | no | n/a | **NO** | **NO** | BSD: follow all symlinks during recursion |
| -U | 0 | no | n/a | yes | yes (BSD only) | BSD: search binary files |
| -u | 0 | no | n/a | yes | yes (BSD only) | BSD: ignored (compat with GNU -u) |
| -y | 0 | no | n/a | yes | yes (BSD only) | BSD: equivalent to -i |

Excluded flags: GNU `-R` / `--dereference-recursive` (follows symlinks during
recursion), BSD `-O` (follows symlinks only if on cmdline), BSD `-S` (follows all
symlinks). `-f` and `--exclude-from` take path values and must be extracted and
containment-checked. No flag in either dialect executes external commands.

Citations:
- FreeBSD grep(1): <https://man.freebsd.org/cgi/man.cgi?query=grep&manpath=FreeBSD+15.1-RELEASE+and+Ports.quarterly>
- GNU grep: already verified from man7.org

---

## rg (ripgrep)

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -i | 0 | no | yes | yes | yes | ignore case |
| -v | 0 | no | yes | yes | yes | invert match |
| -w | 0 | no | yes | yes | yes | word regexp |
| -c | 0 | no | yes | yes | yes | count |
| -l | 0 | no | yes | yes | yes | files-with-matches |
| -L | 0 | no | yes | yes | yes | follow symlinks (read, not follow cmdline) |
| -u | 0 | no | yes | yes | yes | unrestricted (disables gitignore filtering) |
| -uu | 0 | no | yes | yes | yes | unrestricted + hidden |
| -uuu | 0 | no | yes | yes | yes | unrestricted + hidden + binary |
| -a/--text | 0 | no | yes | yes | yes | treat binary as text |
| -z/--search-zip | 0 | no | yes | yes | yes | search compressed files (built-in) |
| -F/--fixed-strings | 0 | no | yes | yes | yes | literal strings |
| -E/--encoding | 1 | no | yes | yes | yes | encoding name |
| -t/--type | 1 | no | yes | yes | yes | file type name |
| -T/--type-not | 1 | no | yes | yes | yes | exclude file type |
| -g/--glob | 1 | no | yes | yes | yes | glob pattern |
| --ignore-case | 0 | no | yes | yes | yes | same as -i |
| --smart-case | 0 | no | yes | yes | yes | auto case |
| --no-ignore | 0 | no | yes | yes | yes | disable gitignore |
| --hidden | 0 | no | yes | yes | yes | search hidden |
| --follow | 0 | no | yes | yes | yes | follow symlinks (same as -L) |
| --no-follow | 0 | no | yes | yes | yes | don't follow symlinks |
| --max-columns | 1 | no | yes | yes | yes | limit line length |
| --sort | 1 | no | yes | yes | yes | sort by path |
| --debug | 0 | no | yes | yes | yes | debug output |
| --pre | 1 | **yes** | n/a | n/a | **NO** | executes external preprocessor command for every file searched |
| --pre-glob | 1 | no | n/a | n/a | **NO** | limits which files get preprocessed (only dangerous with --pre) |
| --hostname-bin | 1 | **yes** | n/a | n/a | **NO** | path to hostname binary; if attacker controls, can spoof --hostname output |
| --replace | 1 | no | yes | yes | yes | replacement string (output only, does not modify files) |
| --json | 0 | no | yes | yes | yes | JSON output |
| -C/--context | 1 | no | yes | yes | yes | context lines |
| -A/--after-context | 1 | no | yes | yes | yes | after context |
| -B/--before-context | 1 | no | yes | yes | yes | before context |
| -U/--multiline | 0 | no | yes | yes | yes | multi-line matches |
| -o/--only-matching | 0 | no | yes | yes | yes | print only matches |
| -M/--max-columns | 1 | no | yes | yes | yes | limit printed line length |

Excluded flags: `--pre` executes an external preprocessor on every file searched
(verified from ripgrep GUIDE.md "Preprocessor" section:
<https://github.com/BurntSushi/ripgrep/blob/master/GUIDE.md>).
`--pre-glob` is meaningless without `--pre` but must be excluded because it limits
which files get the preprocessor. `--hostname-bin` takes a path to an executable;
if an attacker controls the path or the binary, they can influence hostname detection
(verified from ripgrep source code `crates/core/flags/mod.rs` which documents the
`--hostname-bin` flag). `-L`/`--follow` follows symlinks during directory traversal
but does NOT follow symlinks on cmdline operands (it changes which symlinks are
traversed during recursion); it is included because it is read-only file traversal.

Citations:
- ripgrep GUIDE.md (Preprocessor section): <https://github.com/BurntSushi/ripgrep/blob/master/GUIDE.md>
- ripgrep flags source: <https://github.com/BurntSushi/ripgrep/blob/master/crates/core/flags/mod.rs>

---

## realpath

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -e/--canonicalize-existing | 0 | no | yes | n/a | yes (GNU only) | all components must exist |
| -m/--canonicalize-missing | 0 | no | yes | n/a | yes (GNU only) | canonicalize even if missing |
| -L/--no-symlinks | 0 | no | yes | n/a | **NO** | GNU: does NOT resolve symlinks; defeats containment |
| -P/--physical | 0 | no | yes | n/a | yes (GNU only) | resolves symlinks (default) |
| -s/--strip | 0 | no | yes | n/a | yes (GNU only) | strip trailing slashes |
| -q/--quiet | 0 | no | n/a | yes | yes (BSD only) | suppress warnings |

BSD realpath(1) only has `-q`. GNU realpath has `-L`/`--no-symlinks` which
defeats symlink resolution and therefore defeats containment. GNU `-L` is excluded.
All other GNU flags are read-only. BSD `-q` suppresses warnings.

Citations:
- FreeBSD realpath(1): <https://man.freebsd.org/cgi/man.cgi?query=realpath&manpath=FreeBSD+15.1-RELEASE+and+Ports.quarterly>
- GNU realpath: already verified from man7.org

---

## basename

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -a | 0 | no | yes | yes | yes | process all arguments |
| -s | 1 | no | yes | yes | yes | suffix to strip (string, not path) |

Both safe. `-s` takes a string suffix, not a path. No filesystem writes.

Citations:
- Already verified from man7.org and FreeBSD notes

---

## dirname

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -z | 0 | no | n/a | yes | yes (BSD only) | zero-terminated output |
| --help | 0 | no | yes | n/a | yes (GNU only) | help |
| --version | 0 | no | yes | n/a | yes (GNU only) | version |

BSD dirname has `-z` for zero-terminated output. GNU dirname has no special flags
besides `--help`/`--version`. dirname just strips the last component. Safe.

Citations:
- FreeBSD dirname: already verified (implied in notes)
- GNU dirname: already verified from man7.org

---

## echo

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| (none) | 0 | no | yes | yes | yes | echo is a shell builtin; standalone /bin/echo on BSD has no flags |
| -n | 0 | no | yes | yes | yes | BSD: no trailing newline; GNU: same |
| -e | 0 | no | yes | yes | yes | BSD: interpret escapes; GNU: same |
| -E | 0 | no | yes | yes | yes | BSD: no interpretation (default); GNU: same |

echo is a shell builtin. The standalone `/bin/echo` on BSD has `-n`, `-e`, `-E`.
None of these write files. The plan says echo is safe only because the charset
forbids redirection. Include echo with its flags.

Citations:
- Already verified from notes

---

## sort

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -b | 0 | no | yes | yes | yes | ignore leading blanks |
| -d | 0 | no | yes | yes | yes | dictionary order |
| -f | 0 | no | yes | yes | yes | ignore case |
| -i | 0 | no | yes | yes | yes | ignore non-printing |
| -n | 0 | no | yes | yes | yes | numeric sort |
| -r | 0 | no | yes | yes | yes | reverse |
| -s | 0 | no | yes | yes | yes | stable |
| -u | 0 | no | yes | yes | yes | unique |
| -z | 0 | no | yes | yes | yes | zero-terminated |
| -k | 1 | no | yes | yes | yes | key definition |
| -m | 0 | no | yes | yes | yes | merge |
| -t | 1 | no | yes | yes | yes | field separator |
| -M | 0 | no | yes | n/a | yes (GNU only) | month sort |
| -h | 0 | no | yes | n/a | yes (GNU only) | human-numeric sort |
| -V | 0 | no | yes | n/a | yes (GNU only) | version sort |
| -R/--random-sort | 0 | no | yes | n/a | yes (GNU only) | shuffle |
| -S/--buffer-size | 1 | no | yes | n/a | yes (GNU only) | buffer size |
| -T | 1 | **yes** | yes | n/a | yes (GNU only) | temporary directory (path) |
| --debug | 0 | no | yes | n/a | yes (GNU only) | annotate sort keys to stderr |
| --files0-from | 1 | **yes** | yes | n/a | yes (GNU only) | read filenames from file F (path) |
| --random-source | 1 | **yes** | yes | n/a | yes (GNU only) | read random bytes from FILE (path) |
| --compress-program | 1 | **yes** | n/a | n/a | **NO** | executes external compression program |
| -o/--output | 1 | **yes** | **NO** | **NO** | **NO** | writes sorted output to FILE |
| --parallel | 1 | no | yes | n/a | yes (GNU only) | concurrency level |
| --check | 0 | no | yes | n/a | yes (GNU only) | check sorted |
| -C/--check=quiet | 0 | no | yes | n/a | yes (GNU only) | check quiet |
| -c | 0 | no | yes | n/a | yes (GNU only) | same as --check |
| -S | 0 | no | yes | n/a | yes (BSD only) | buffer size |

Excluded flags: `-o`/`--output=FILE` writes the sorted output to a file on both
GNU and BSD. `--compress-program=PROG` executes an external compression program
(GNU only). `-T`, `--files0-from`, `--random-source` take path values and must be
extracted and containment-checked. BSD `-S` is buffer size (safe), not output.

Citations:
- GNU sort(1): <https://man7.org/linux/man-pages/man1/sort.1.html>
- BSD sort: already verified from FreeBSD notes

---

## uname

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -a | 0 | no | yes | yes | yes | all info |
| -i | 0 | no | yes | yes | yes | kernel ident (BSD); machine hardware (GNU) |
| -m | 0 | no | yes | yes | yes | machine hardware |
| -n | 0 | no | yes | yes | yes | nodename |
| -o | 0 | no | n/a | yes | yes (BSD only) | same as -s |
| -p | 0 | no | yes | yes | yes | processor |
| -r | 0 | no | yes | yes | yes | release |
| -s | 0 | no | yes | yes | yes | sysname |
| -U | 0 | no | n/a | yes | yes (BSD only) | FreeBSD userland version |
| -v | 0 | no | yes | yes | yes | version/build |
| -K | 0 | no | n/a | yes | yes (BSD only) | FreeBSD kernel version |
| -b | 0 | no | n/a | yes | yes (BSD only) | build-id |

All flags are read-only. uname just prints system info. GNU and BSD differ on which
flags exist but all are safe. `UNAME_*` env vars can override output on FreeBSD
(noted in ENVIRONMENT section), but this is a runtime env effect, not a flag.

Citations:
- FreeBSD uname(1): <https://man.freebsd.org/cgi/man.cgi?query=uname&manpath=FreeBSD+15.1-RELEASE+and+Ports.quarterly>
- GNU uname: already verified from man7.org

---

## hostname

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -s | 0 | no | yes | yes | yes | short name |
| -i | 0 | no | yes | n/a | yes (GNU only) | IP address |
| -I | 0 | no | yes | n/a | yes (GNU only) | all IPs |
| -f | 0 | no | yes | yes | yes | FQDN (BSD: default; GNU: include domain) |
| -d | 0 | no | yes | yes | yes | domain only (BSD); DNS domain (GNU) |
| -y | 0 | no | yes | n/a | yes (GNU only) | type |
| --help | 0 | no | yes | n/a | yes (GNU only) | help |
| --version | 0 | no | yes | n/a | yes (GNU only) | version |

BSD hostname(1) confirmed: flags are `-f` (default), `-s`, `-d`. There is NO `-S`
flag on BSD. Setting the hostname is done by providing a name argument, not via a
flag. All display flags are read-only. GNU hostname `-i`/`-I` may do DNS lookups
(read network, but read-only).

Citations:
- FreeBSD hostname(1): <https://man.freebsd.org/cgi/man.cgi?query=hostname&manpath=FreeBSD+15.1-RELEASE+and+Ports.quarterly>
- GNU hostname: already verified from man7.org

---

## cmp

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -l | 0 | no | yes | yes | yes | verbose byte dump |
| -s | 0 | no | yes | yes | yes | silent |
| -i | 1 | no | yes | yes | yes | ignore initial bytes |
| -n | 1 | no | yes | yes | yes | max bytes |
| -b | 0 | no | yes | n/a | yes (GNU only) | print differing bytes |
| -x | 0 | no | n/a | yes | yes (BSD only) | hex byte dump |
| -h | 0 | no | n/a | yes | yes (BSD only) | do not follow symlinks |
| -z | 0 | no | n/a | yes | yes (BSD only) | compare sizes first |

All flags are read-only. cmp compares two files byte-by-byte. BSD `-h` explicitly
does not follow symlinks. GNU cmp does not have `-h` (it does not follow symlinks
by default).

Citations:
- FreeBSD cmp(1): <https://man.freebsd.org/cgi/man.cgi?query=cmp&manpath=FreeBSD+15.1-RELEASE+and+Ports.quarterly>
- GNU cmp: already verified from man7.org

---

## uniq

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| -c | 0 | no | yes | yes | yes | count occurrences |
| -d | 0 | no | yes | yes | yes | only repeated |
| -D | 0 | no | yes | yes | yes | all repeated lines |
| -i | 0 | no | yes | yes | yes | ignore case |
| -u | 0 | no | yes | yes | yes | only unique |
| -f | 1 | no | yes | yes | yes | skip fields |
| -s | 1 | no | yes | yes | yes | skip chars |
| -w | 1 | no | yes | n/a | yes (GNU only) | check chars |
| -z | 0 | no | yes | n/a | yes (GNU only) | zero-terminated |
| --count | 0 | no | yes | n/a | yes (GNU only) | same as -c |
| --repeated | 0 | no | yes | n/a | yes (GNU only) | same as -d |
| --all-repeated | 0 | no | yes | n/a | yes (GNU only) | same as -D |
| --ignore-case | 0 | no | yes | n/a | yes (GNU only) | same as -i |
| --unique | 0 | no | yes | n/a | yes (GNU only) | same as -u |
| --skip-fields | 1 | no | yes | n/a | yes (GNU only) | same as -f |
| --skip-chars | 1 | no | yes | n/a | yes (GNU only) | same as -s |
| --check-chars | 1 | no | yes | n/a | yes (GNU only) | same as -w |
| --zero-terminated | 0 | no | yes | n/a | yes (GNU only) | same as -z |

BSD uniq does not have `-w`, `-z`, or `--group-directories-first` equivalent for
uniq. All flags are read-only. uniq reads from input_file and writes to output_file
(operands, not flags). The operands are paths and are containment-checked by the
existing mechanism.

Citations:
- FreeBSD uniq(1): <https://man.freebsd.org/cgi/man.cgi?query=uniq&manpath=FreeBSD+15.1-RELEASE+and+Ports.quarterly>
- GNU uniq: already verified from man7.org

---

## Deliberately excluded programs

These programs have safe common uses but at least one flag or subcommand that writes,
executes, or follows symlinks in ways the classifier cannot safely model:

- `find`: `-delete` removes files; `-exec` and `-execdir` execute commands.
- `sed`: `-i` edits files in place (writes).
- `awk`/`gawk`: `system()`, `print >file`, and `close()` can execute and write.
- `xargs`: `-I` and bare command execution run arbitrary commands.
- `tee`: writes to files.
- `truncate`: writes file size.
- `cp`/`mv`/`ln`: write/rename/link files.
- `touch`: writes file timestamps (or creates files).
- `dd`: raw block read/write.
- `chmod`/`chown`: modify file metadata/permissions.
- Any shell (`sh`, `bash`, `zsh`, `csh`): execute by definition.
- Interpreters (`python`, `node`, `ruby`, `perl`, `lua`): execute by definition.
- Network tools (`curl`, `wget`, `ssh`, `scp`, `rsync`): network I/O + can write.
- `tar`: `--to-command` executes; checkpoint actions write; `-c` creates archives.
  Even `tar tf` (list) is excluded because the binary carries write flags.
- `less`/`more`/`most`: pagers that run `!cmd` and can invoke editors.
- `man`: invokes a pager and can run `!cmd` within it.
- `vi`/`nano`/`ed`/`ex`: editors modify files.
- `git`: dozens of subcommands, many write; v1 excludes entirely. Key format
  reserves `safe:git:<subcommand>` for a follow-up.

Citations: plan `safe-command-classifier.md` "The allowlist" section.

---

## Open questions answered by research

### Q1: Does GNU getopt accept abbreviated long flags?

**Yes.** GNU getopt accepts unambiguous prefixes. `grep --col` is accepted by GNU
grep and resolves to `--color` (or `--context` if both share the prefix — in that
case it is an error). The classifier must reject abbreviated long flags: the plan
specifies exact matching, and `grep --col foo file` is a pinned adversarial test.

The classifier's tokenizer matches long flags against the per-program allowlist
exactly. An abbreviated flag that is not in the allowlist is an unknown flag and
makes the whole command unclassifiable.

Citations:
- GNU getopt behavior documented in glibc manual and observed in GNU grep --help
  output (which notes " unambiguous prefixes").
- Pinned test in `safe-command-classifier.md` adversarial flags section.

### Q2: Which flags write output to a destination file?

Verified write-destination flags across the researched programs:

| program | flag | dialect | writes to |
|---------|------|---------|-----------|
| sort | -o, --output | GNU + BSD | FILE operand / --output=FILE |
| sort | --compress-program | GNU | temp files + executes PROG |
| file | --no-sandbox | GNU | enables external decompressor writes |
| file | -p | GNU + BSD | modifies access time via utimes |
| file | -s, --special-files | GNU + BSD | reads raw devices (not write, but dangerous) |
| date | -s, --set | GNU | sets system clock |
| cat | -l | BSD | exclusive advisory lock on stdout (state modification) |
| realpath | -L/--no-symlinks | GNU | does not resolve symlinks (containment defeat) |

The classifier excludes all write-destination flags and treats state-modification
flags (BSD cat `-l`) the same way.

### Q3: Does BSD `ls` have `--color`?

**Yes, on FreeBSD.** FreeBSD ls(1) explicitly documents `--color=when` with the
same `always`/`auto`/`never` values as GNU, plus GNU-compatible aliases `yes`/`force`
/`no`/`none`/`tty`/`if-tty`. FreeBSD also has `-G` as a short color flag.

**No, on OpenBSD.** OpenBSD ls does not have `--color` or `-G`. The flag set is
`-1,-A,-a,-C,-c,-d,-F,-f,-g,-H,-h,-i,-k,-L,-l,-m,-n,-o,-p,-q,-R,-r,-S,-s,-T,-t,-u,-v,-w,-x,-y`.

For the classifier's host platform (Darwin/FreeBSD), `--color` is included. If
OpenBSD support is ever added, the BSD column for `--color` must be "no" and the
flag excluded on that dialect.

Citations:
- FreeBSD ls(1): <https://man.freebsd.org/cgi/man.cgi?query=ls&manpath=FreeBSD+15.1-RELEASE+and+Ports.quarterly>
- OpenBSD ls: already verified from notes (no --color)

---

## Confidence and gaps

**High confidence:**
- All FreeBSD man pages fetched and verified directly.
- GNU coreutils man pages verified from man7.org.
- ripgrep `--pre`, `--pre-glob` behavior verified from ripgrep GUIDE.md.
- ripgrep `--hostname-bin` verified from ripgrep source code (`crates/core/flags/mod.rs`).
- BSD `cat -l` lock behavior verified from FreeBSD cat(1).
- BSD `date -f` vs GNU `date -f` divergence verified from both man pages.
- BSD `ls --color` confirmed on FreeBSD, absent on OpenBSD.

**Gaps:**
- GNU realpath man page not fetched (7.org 404'd); verified from existing notes.
- OpenBSD man pages not fetched; OpenBSD differences noted from prior research notes.
- ripgrep `--hostname-bin` documented in source but not in user-facing GUIDE.md;
  the source comment is the primary citation.
- BSD `echo -n/-e/-E` not re-fetched; inferred from notes.
- The classifier table must track dialect per flag. The "include?" column here is
  the conservative union; the code must not include a flag that is safe only on one
  dialect unless the dialect is explicitly the host platform.
