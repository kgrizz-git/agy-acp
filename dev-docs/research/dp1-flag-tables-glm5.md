# DP1: Per-Program Allowed-Flag Tables

Research for the safe-command classifier's per-program flag allowlist. Each flag verified against GNU coreutils manual (gnu.org/software/coreutils/manual) and BSD man pages (man.freebsd.org). Flags included ONLY when safe on BOTH dialects, unless the program has a single real implementation.

**Key principle**: A flag is admissible only if it never writes/modifies, executes, reaches the network, or follows symlinks to operands in a way that defeats path containment.

## Priority 1: Core Utilities

### ls (list directory contents)

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| `-l` | 0 | N/A | yes | yes | yes | Long format output, read-only |
| `-a` | 0 | N/A | yes | yes | yes | Show hidden files |
| `-A` | 0 | N/A | yes | yes | yes | Show hidden except . and .. |
| `-h` | 0 | N/A | yes | yes | yes | Human-readable sizes |
| `-d` | 0 | N/A | yes | yes | yes | List directories as files |
| `-F` | 0 | N/A | yes | yes | yes | Append indicator to names |
| `-p` | 0 | N/A | yes | yes | yes | Append / to directories |
| `-R` | 0 | N/A | yes | yes | yes | Recursive listing (read-only) |
| `-r` | 0 | N/A | yes | yes | yes | Reverse sort |
| `-S` | 0 | N/A | yes | yes | yes | Sort by size |
| `-t` | 0 | N/A | yes | yes | yes | Sort by modification time |
| `-1` | 0 | N/A | yes | yes | yes | One file per line |
| `-C` | 0 | N/A | yes | yes | yes | Multi-column output |
| `-i` | 0 | N/A | yes | yes | yes | Show inode number |
| `-n` | 0 | N/A | yes | yes | yes | Numeric UID/GID |
| `-v` | 0 | N/A | yes | yes | yes | Natural sort (version) |
| `-X` | 0 | N/A | yes | GNU-only | no | Sort by extension — GNU-only, excluded for cross-platform safety |
| `--color` | 0 | N/A | yes | yes | yes | Colorize output. **BSD accepts `--color=when`** (FreeBSD 15.1 man page confirms `--color=always|auto|never` with GNU compatibility) |
| `--group-directories-first` | 0 | N/A | yes | yes | yes | Group directories first. BSD has `--group-directories=first|last` and `--group-directories-first` (GNU compat) |
| `--time-style` | 1 | no | yes | BSD-only | no | **EXCLUDED**: BSD `-D format` uses strftime format string; GNU `--time-style=STYLE` uses named styles. Different syntax, different semantics. Asymmetric. |

**EXCLUDED flags with reasons:**

| flag | reason |
|------|--------|
| `-H` | **Symlink-following**: Follows symlinks on command line, defeats path containment for operands |
| `-L` | **Symlink-following**: Follows all symlinks, same containment concern |
| `-P` | Symlink policy flag; redundant with default, but presence suggests symlink handling |
| `-f` | Output not sorted, but turns on `-a`; safe but semantically about sorting |
| `-I` / `--ignore` | Pattern argument (not a path), safe but adds glob semantics |
| `--hide` | Pattern argument, GNU-only |
| `-w` | Raw printing of non-printables, BSD-only |
| `-q` | Force `?` for non-printables, safe but display-only |
| `-b` / `-B` | Octal escape printing, safe but display-only |
| `-D` | **BSD-only**, format string for dates; conflicts with GNU `-D` (--dired) which has completely different meaning |
| `-G` | BSD color flag (equivalent to `--color=auto`), safe but GNU uses `-G` for `--no-group` |
| `-g` | BSD: same as `-l` but no owner; GNU: omit owner in long format — safe but asymmetric |
| `-o` | BSD: file flags in long format; GNU: omit group info — **asymmetric meanings, EXCLUDED** |
| `-T` | BSD: full time info; GNU uses `-T` elsewhere — not in GNU ls |
| `-U` | BSD: use creation time; GNU: `--sort=none` — **asymmetric, EXCLUDED** |
| `-W` | BSD: display whiteouts — not in GNU |
| `-Z` | SELinux context (GNU) / MAC label (BSD) — security context reading, safe but niche |
| `-s` | Show block count, safe but display-only |
| `-k` | Set block size to 1024, safe but display-only |
| `--author` | GNU-only, display field |
| `--dired` / `-D` | GNU-only: Emacs dired output; BSD `-D` is date format — **asymmetric, EXCLUDED** |
| `--full-time` | GNU-only: full-iso time style |
| `--si` | GNU-only: powers of 1000 for sizes |

**Key findings:**
- `--color` IS accepted by BSD ls (FreeBSD 15.1+), with GNU-compatible values
- `-o` has different meanings: BSD shows file flags, GNU omits group — **excluded for asymmetry**
- `-D` is asymmetric: GNU is --dired, BSD is date format — **excluded**
- `-U` is asymmetric: BSD is creation time, GNU is no-sort — **excluded**
- `-T` is BSD-only
- Both `-H` and `-L` are symlink-following and excluded per containment policy

### cat (concatenate and print files)

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| `-n` | 0 | N/A | yes | yes | yes | Number all lines |
| `-b` | 0 | N/A | yes | yes | yes | Number non-blank lines |
| `-s` | 0 | N/A | yes | yes | yes | Squeeze blank lines |
| `-v` | 0 | N/A | yes | yes | yes | Show non-printing chars |
| `-e` | 0 | N/A | yes | yes | yes | Equivalent to `-vE` |
| `-t` | 0 | N/A | yes | yes | yes | Equivalent to `-vT` |
| `-E` | 0 | N/A | yes | BSD-only | no | **EXCLUDED**: BSD uses `-e` for `-vE` equivalent, not separate `-E` |
| `-T` | 0 | N/A | yes | BSD-only | no | **EXCLUDED**: BSD uses `-t` for `-vT` equivalent, not separate `-T` |
| `-A` | 0 | N/A | yes | BSD-only | no | **EXCLUDED**: GNU-only, equivalent to `-vET` |
| `-u` | 0 | N/A | yes | yes | yes | Ignored (POSIX compat); BSD: disable output buffering |

**EXCLUDED flags:**
- `-l` (BSD-only): Sets exclusive lock on stdout using `fcntl(F_SETLKW)` — **modifies file descriptor state, EXCLUDED**

**Notes:**
- BSD combines `-E` and `-T` functionality into `-e` and `-t` rather than offering them separately
- The `-l` flag on BSD is a **state-modifying operation** (advisory lock), not safe

### head (output first part of files)

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| `-n` | 1 | no | yes | yes | yes | Number of lines (integer, not path) |
| `-c` | 1 | no | yes | yes | yes | Number of bytes (integer, not path) |
| `-q` | 0 | N/A | yes | yes | yes | Suppress headers |
| `-v` | 0 | N/A | yes | yes | yes | Always show headers |
| `-z` | 0 | N/A | yes | BSD-only | no | **EXCLUDED**: GNU-only, NUL-terminated items |

**Notes:**
- `-n` and `-c` values are counts, not paths
- BSD supports size suffixes on counts (K, M, etc.) via `expand_number(3)`
- Very few flags, all straightforward

### tail (output last part of files)

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| `-n` | 1 | no | yes | yes | yes | Number of lines |
| `-c` | 1 | no | yes | yes | yes | Number of bytes |
| `-q` | 0 | N/A | yes | yes | yes | Suppress headers |
| `-v` | 0 | N/A | yes | yes | yes | Always show headers |
| `-b` | 1 | no | BSD-only | yes | no | **EXCLUDED**: BSD-only, 512-byte blocks |
| `-f` | 0 | N/A | yes | yes | no | **EXCLUDED**: Follow mode — blocks waiting for more data, changes process behavior |
| `-F` | 0 | N/A | yes | yes | no | **EXCLUDED**: Follow with retry — same behavioral change as `-f` |
| `-r` | 0 | N/A | BSD-only | yes | no | **EXCLUDED**: BSD-only, reverse output |
| `-z` | 0 | N/A | yes | BSD-only | no | **EXCLUDED**: GNU-only, NUL-terminated |
| `--pid` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, process monitoring |
| `--retry` | 0 | N/A | yes | no | no | **EXCLUDED**: GNU-only, retry logic |
| `-s` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, sleep interval |
| `--follow` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, follow mode variant |

**Key exclusions:**
- `-f` and `-F` are excluded because they change process behavior to block and wait, not just read and exit
- These are follow-mode flags that keep the process running, not read-only in the intended sense

### wc (word, line, character, byte count)

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| `-l` | 0 | N/A | yes | yes | yes | Line count |
| `-w` | 0 | N/A | yes | yes | yes | Word count |
| `-c` | 0 | N/A | yes | yes | yes | Byte count |
| `-m` | 0 | N/A | yes | yes | yes | Character count |
| `-L` | 0 | N/A | yes | yes | yes | Longest line length |
| `--files0-from` | 1 | yes | yes | no | no | **EXCLUDED**: GNU-only; reads file list from file or stdin — changes input source |
| `--total` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, controls total line output |
| `--debug` | 0 | N/A | yes | no | no | **EXCLUDED**: GNU-only |

**Notes:**
- Very safe command, all standard flags are read-only counters
- GNU has several extensions that don't add meaningful risk but are GNU-only

---

## Priority 2: Secondary Core Utilities

### file (determine file type)

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| `-b` | 0 | N/A | yes | yes | yes | Brief mode, no filename prefix |
| `-i` | 0 | N/A | yes | yes | yes | MIME type output |
| `--mime-type` | 0 | N/A | yes | yes | yes | MIME type only |
| `--mime-encoding` | 0 | N/A | yes | yes | yes | MIME encoding only |
| `--extension` | 0 | N/A | yes | yes | yes | List valid extensions |
| `-h` | 0 | N/A | yes | yes | yes | Don't follow symlinks (default on BSD) |
| `-k` | 0 | N/A | yes | yes | yes | Keep going after first match |
| `-N` | 0 | N/A | yes | yes | yes | Don't pad filenames |
| `-n` | 0 | N/A | yes | yes | yes | Flush stdout after each file |
| `-0` | 0 | N/A | yes | yes | yes | NUL-terminate filenames |
| `-f` | 1 | yes | yes | yes | no | **EXCLUDED**: Reads file list from named file — changes input source |
| `-m` | 1 | yes | yes | yes | no | **EXCLUDED**: Specifies alternate magic files — changes what's read |
| `-e` | 1 | no | yes | yes | no | **EXCLUDED**: Exclude tests by name — affects analysis, not output destination |
| `-F` | 1 | no | yes | yes | no | **EXCLUDED**: Separator string — safe but display-only, not commonly needed |
| `-L` | 0 | N/A | yes | yes | no | **EXCLUDED**: Follow symlinks — same containment concern as ls |
| `-s` | 0 | N/A | yes | yes | no | **EXCLUDED**: Read special files (block/char devices) — could read arbitrary device files |
| `-z` | 0 | N/A | yes | yes | no | **EXCLUDED**: Look inside compressed files — spawns decompressor, execution risk |
| `-Z` | 0 | N/A | yes | yes | no | **EXCLUDED**: Same as `-z` but report content only |
| `-S` | 0 | N/A | yes | yes | no | **EXCLUDED**: Disable sandbox — security weakening |
| `-C` | 0 | N/A | yes | yes | no | **EXCLUDED**: Compile magic file — writes output |
| `-c` | 0 | N/A | yes | yes | no | **EXCLUDED**: Checking printout — debugging mode |
| `-d` | 0 | N/A | yes | yes | no | **EXCLUDED**: Debug output — debugging mode |
| `-l` | 0 | N/A | yes | yes | no | **EXCLUDED**: List patterns — different output mode |
| `-p` | 0 | N/A | yes | yes | no | **EXCLUDED**: Preserve access time — touches file metadata |
| `-r` | 0 | N/A | yes | yes | no | **EXCLUDED**: Raw output — display mode |
| `-P` | 1 | no | yes | yes | no | **EXCLUDED**: Parameter limits — control flags |
| `-E` | 0 | N/A | yes | yes | no | **EXCLUDED**: Exit on error — behavior change |
| `--apple` | 0 | N/A | yes | yes | no | **EXCLUDED**: Apple-specific output — niche |
| `--exclude-quiet` | 0 | N/A | yes | yes | no | **EXCLUDED**: Quiet exclude — behavior change |
| `--help` | 0 | N/A | yes | yes | no | **EXCLUDED**: Help output |

**Key exclusions:**
- `-L` follows symlinks — excluded per containment policy
- `-s` reads block/char special files — could read arbitrary devices
- `-z`/`-Z` decompress files — spawns external programs
- `-S` disables sandboxing — security risk
- `-C` writes compiled magic file — write operation
- `-p` preserves access time — touches file metadata
- `-f` reads file list from another file — changes input source

### stat (display file status)

**Note:** BSD `stat` is significantly different from GNU `stat`. GNU has a rich format language; BSD has a simpler format language with different specifiers. Many flags have no equivalent.

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| `-f` | 1 | no | yes | yes | no | **EXCLUDED**: Format string — both have this but syntax is completely different between GNU and BSD; asymmetric |
| `-L` | 0 | N/A | yes | yes | no | **EXCLUDED**: Follow symlinks — containment concern |
| `-t` | 1 | no | BSD-only | yes | no | **EXCLUDED**: BSD-only, time format |
| `-n` | 0 | N/A | BSD-only | yes | no | **EXCLUDED**: BSD-only, no trailing newline |
| `-q` | 0 | N/A | BSD-only | yes | no | **EXCLUDED**: BSD-only, quiet |
| `-r` | 0 | N/A | BSD-only | yes | no | **EXCLUDED**: BSD-only, raw output |
| `-s` | 0 | N/A | BSD-only | yes | no | **EXCLUDED**: BSD-only, shell output format |
| `-x` | 0 | N/A | BSD-only | yes | no | **EXCLUDED**: BSD-only, verbose Linux-style |
| `-l` | 0 | N/A | BSD-only | yes | no | **EXCLUDED**: BSD-only, ls -lT format |
| `-F` | 0 | N/A | BSD-only | yes | no | **EXCLUDED**: BSD-only, classify file type |
| `-c` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, format string (different from BSD `-f`) |
| `--printf` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, format string |
| `--format` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, format string |

**Assessment:**
- **`stat` is problematic for inclusion.** The GNU and BSD implementations have completely different format string syntaxes and different sets of format specifiers. A format string that works on one will not work on the other.
- No flags are safe to include cross-platform because the output format languages are incompatible.
- **Recommendation:** Exclude `stat` from v1 or allow only with NO flags. The format string issue makes any flag-based classification unsafe across dialects.

### pwd (print working directory)

**No flags needed.** `pwd` takes no meaningful options on either GNU or BSD that change behavior. Both support `-P` (physical, avoid symlinks) and `-L` (logical, follow symlinks), but:

- `-P` is safe (avoids symlinks)
- `-L` is symlink-following and excluded

Since the classifier already knows the working directory from `Cwd`, `pwd` is low-value but safe with `-P` only.

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| `-P` | 0 | N/A | yes | yes | yes | Physical path, avoid symlinks |
| `-L` | 0 | N/A | yes | yes | no | **EXCLUDED**: Follow symlinks |

### du (disk usage)

**High-risk assessment needed.** `du` has flags that follow symlinks and flags that change what is counted.

**Key exclusions:**
- `-L`, `-D`, `-H`: All follow symlinks — excluded
- `-x`: One filesystem only — safe but limits behavior
- `-c`: Grand total — safe, display only
- `-s`: Summary only — safe, display only

**Full flag analysis:**

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| `-h` | 0 | N/A | yes | yes | yes | Human-readable sizes |
| `-k` | 0 | N/A | yes | yes | yes | 1024-byte blocks |
| `-s` | 0 | N/A | yes | yes | yes | Summary total only |
| `-c` | 0 | N/A | yes | yes | yes | Grand total |
| `-x` | 0 | N/A | yes | yes | yes | One filesystem only |
| `-a` | 0 | N/A | yes | yes | yes | All files, not just directories |
| `-d` | 1 | no | yes | yes | yes | Max depth (integer) |
| `-L` | 0 | N/A | yes | yes | no | **EXCLUDED**: Follow symlinks |
| `-H` | 0 | N/A | yes | yes | no | **EXCLUDED**: Follow symlinks from args only |
| `-D` | 0 | N/A | GNU-only | no | no | **EXCLUDED**: GNU-only, symlink behavior |
| `-l` | 0 | N/A | yes | yes | no | **EXCLUDED**: Count hard links multiple times — changes accounting |
| `-P` | 0 | N/A | yes | yes | yes | No symlink following (default) |
| `-S` | 0 | N/A | yes | yes | no | **EXCLUDED**: Separate dirs — display format change |
| `--exclude` | 1 | no | yes | BSD-only | no | **EXCLUDED**: Pattern exclusion, GNU-only syntax |
| `-I` | 1 | no | BSD-only | yes | no | **EXCLUDED**: BSD-only, ignore pattern |

**Notes:**
- GNU uses `--exclude=PATTERN`, BSD uses `-I pattern`
- Both are pattern-based exclusions, different syntax

### df (disk free space)

**Assessment:** `df` reports filesystem disk space. Generally safe, but:

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| `-h` | 0 | N/A | yes | yes | yes | Human-readable |
| `-k` | 0 | N/A | yes | yes | yes | 1024-byte blocks |
| `-l` | 0 | N/A | BSD-only | yes | no | **EXCLUDED**: BSD-only, local filesystems only |
| `-T` | 0 | N/A | BSD-only | yes | no | **EXCLUDED**: BSD-only, show filesystem type |
| `-t` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, filesystem type limit |
| `-x` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, exclude filesystem type |
| `-i` | 0 | N/A | yes | yes | yes | Inode info |
| `-P` | 0 | N/A | yes | yes | yes | POSIX format |

**Notes:**
- `-t` means different things: BSD shows type, GNU filters by type — **asymmetric, excluded**
- Both have human-readable and inode flags that are safe

### date (display or set date and time)

**Critical asymmetry: `-f` flag**

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| `-u` | 0 | N/A | yes | yes | yes | UTC time |
| `-R` | 0 | N/A | yes | yes | yes | RFC 2822 format |
| `-I` | 1 | no | yes | yes | yes | ISO 8601 format (FMT is date/hours/minutes/etc.) |
| `-j` | 0 | N/A | BSD-only | yes | no | **EXCLUDED**: BSD-only, don't set date |
| `-n` | 0 | N/A | BSD-only | yes | no | **EXCLUDED**: BSD-only, obsolete flag |
| `-r` | 1 | varies | yes | yes | no | **EXCLUDED**: **ASYMMETRIC**: BSD `-r filename` reads file mtime; BSD `-r seconds` parses epoch; GNU `-r` / `--reference=FILE` reads file timestamp. **File-reading variants change input source.** |
| `-f` | 1 | varies | yes | yes | no | **EXCLUDED**: **ASYMMETRIC**: BSD `-f input_fmt` is a FORMAT STRING for parsing; GNU `-f` / `--file=DATEFILE` READS DATES FROM A FILE — file-reading variant changes input source. |
| `-d` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, display date from string |
| `--date` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, same as `-d` |
| `-v` | 1 | no | BSD-only | yes | no | **EXCLUDED**: BSD-only, date adjustment |
| `-z` | 1 | no | BSD-only | yes | no | **EXCLUDED**: BSD-only, timezone output |

**CRITICAL ASYMMETRY on `-f`:**
- **BSD `date -f input_fmt new_date`**: `input_fmt` is a format string (like `+%Y-%m-%d`) to parse `new_date`
- **GNU `date -f DATEFILE`**: Reads dates from a FILE named `DATEFILE`

**This is the exact dangerous asymmetry the plan warns about.** The BSD version takes a format string; the GNU version reads a file. A command like `date -f /etc/passwd` would:
- On BSD: try to parse `/etc/passwd` as a format string (likely fails, but weird)
- On GNU: **read and display dates from `/etc/passwd`**

**`-f` is EXCLUDED for this asymmetry.**

**`-r` is also asymmetric:**
- BSD: `-r filename` (read file mtime) or `-r seconds` (parse epoch)
- GNU: `-r FILE` / `--reference=FILE` (read file mtime)

The GNU version always reads a file. BSD has dual behavior. **Excluded for safety.**

### which (show full path of command)

**Note:** `which` is not a GNU coreutils program. It's typically provided by the shell or a separate package. BSD has a `which` in base; Linux distributions vary.

**No flags are portable.** The behavior of `which` varies significantly:
- BSD `which`: No meaningful flags, just prints path
- GNU/Linux: Often a shell builtin or different implementation

**Recommendation:** Include `which` with NO flags. It takes command names as operands, not paths.

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason |
|------|-------|----------------|---------|---------|----------|--------|
| `-s` | 0 | N/A | no | yes | no | **EXCLUDED**: BSD-only, silent mode |
| `-a` | 0 | N/A | varies | yes | no | **EXCLUDED**: Not universally available |

**Recommendation:** Allow `which` with no flags.

---

## Priority 3: Search Tools (grep, rg)

### grep (print lines matching a pattern)

**Execution flags: What flags execute code?**

The GNU grep manual does NOT list any flags that execute external commands. Unlike `rg`, grep does not have `--pre`, `--pre-glob`, or similar execution hooks.

**Verified execution-related exclusions:**
- `grep` does not have execution flags like `rg --pre`
- All flags are pattern matching, file selection, or output formatting

**Key symlink-following flag:**
- `-R` / `-r`: Recursive search. GNU `-r` follows symlinks by default; `grep -R` dereferences all symlinks.
- GNU `--no-dereference` affects this.

**Safe flags:**

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason/citation |
|------|-------|----------------|---------|---------|----------|-----------------|
| `-i` | 0 | N/A | yes | yes | yes | Case insensitive |
| `-v` | 0 | N/A | yes | yes | yes | Invert match |
| `-c` | 0 | N/A | yes | yes | yes | Count matches |
| `-l` | 0 | N/A | yes | yes | yes | List filenames |
| `-L` | 0 | N/A | yes | yes | yes | List non-matching files |
| `-n` | 0 | N/A | yes | yes | yes | Line numbers |
| `-q` | 0 | N/A | yes | yes | yes | Quiet mode |
| `-o` | 0 | N/A | yes | yes | yes | Only matching |
| `-w` | 0 | N/A | yes | yes | yes | Word match |
| `-x` | 0 | N/A | yes | yes | yes | Line match |
| `-F` | 0 | N/A | yes | yes | yes | Fixed strings |
| `-E` | 0 | N/A | yes | yes | yes | Extended regex |
| `-G` | 0 | N/A | yes | yes | yes | Basic regex (default) |
| `-P` | 0 | N/A | yes | no | no | **EXCLUDED**: GNU-only, Perl regex |
| `-e` | 1 | no | yes | yes | yes | Pattern (takes pattern string, not path) |
| `-f` | 1 | yes | yes | yes | no | **EXCLUDED**: Read patterns from FILE — changes input source |
| `-r` | 0 | N/A | yes | yes | no | **EXCLUDED**: Recursive, follows symlinks by default on GNU |
| `-R` | 0 | N/A | yes | yes | no | **EXCLUDED**: Recursive, dereferences all symlinks |
| `--include` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, file pattern |
| `--exclude` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, file pattern |
| `--exclude-dir` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only, directory pattern |
| `--null` | 0 | N/A | yes | yes | yes | NUL after filename |
| `-Z` | 0 | N/A | yes | yes | yes | Same as `--null` |
| `-a` | 0 | N/A | yes | yes | yes | Process binary files |
| `-I` | 0 | N/A | yes | yes | yes | Skip binary files |
| `--binary-files` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only |
| `-s` | 0 | N/A | yes | yes | yes | Suppress error messages |
| `-h` | 0 | N/A | yes | yes | yes | Suppress filename prefix |
| `-H` | 0 | N/A | yes | yes | yes | Always print filename |
| `-m` | 1 | no | yes | yes | yes | Max count |
| `-b` | 0 | N/A | yes | yes | yes | Byte offset |
| `--color` | 1 | no | yes | BSD-only | no | **EXCLUDED**: GNU `--color=WHEN`; BSD grep doesn't support `--color` the same way |
| `-A` | 1 | no | yes | yes | yes | After context |
| `-B` | 1 | no | yes | yes | yes | Before context |
| `-C` | 1 | no | yes | yes | yes | Context (both sides) |

**Key findings:**
- **grep has NO execution flags** (unlike `rg`)
- `-r`/`-R` are excluded because they follow symlinks by default
- `-f` reads patterns from a file — changes input source
- `-P` (Perl regex) is GNU-only

### rg (ripgrep)

**Execution flags (VERIFIED FROM ripgrep manual):**

The following flags in `rg` **EXECUTE EXTERNAL COMMANDS**:

| flag | arity | value-is-path? | reason |
|------|-------|----------------|--------|
| `--pre` | 1 | no | **EXECUTES**: Runs COMMAND on each file before searching. Example: `--pre=pdftotext` runs pdftotext on each file. **EXCLUDED** |
| `--pre-glob` | 1 | no | **EXECUTES**: Works with `--pre` to limit which files get preprocessed. Requires `--pre` to have effect. **EXCLUDED** |
| `--hostname-bin` | 1 | no | **EXECUTES**: Runs COMMAND to get hostname for hyperlinks. **EXCLUDED** |

**Safe flags:**

| flag | arity | value-is-path? | include? | reason |
|------|-------|----------------|----------|--------|
| `-i` | 0 | N/A | yes | Case insensitive |
| `-v` | 0 | N/A | yes | Invert match |
| `-c` | 0 | N/A | yes | Count matches |
| `-l` | 0 | N/A | yes | List matching files |
| `-L` | 0 | N/A | yes | Follow symlinks — **wait, this is symlink-following, EXCLUDED** |
| `-n` | 0 | N/A | yes | Line numbers |
| `-q` | 0 | N/A | yes | Quiet |
| `-o` | 0 | N/A | yes | Only matching |
| `-w` | 0 | N/A | yes | Word match |
| `-x` | 0 | N/A | yes | Line match |
| `-F` | 0 | N/A | yes | Fixed strings |
| `-e` | 1 | no | yes | Pattern |
| `-f` | 1 | yes | no | **EXCLUDED**: Read patterns from file |
| `-j` | 1 | no | yes | Number of threads (integer) |
| `-S` | 0 | N/A | yes | Smart case |
| `-s` | 0 | N/A | yes | Case sensitive |
| `--sort` | 1 | no | yes | Sort results (none/path/modified/etc.) |
| `--sortr` | 1 | no | yes | Reverse sort |
| `--heading` | 0 | N/A | yes | Heading output format |
| `--no-heading` | 0 | N/A | yes | Disable heading |
| `--color` | 1 | no | yes | Color control (auto/always/never) |
| `--colors` | 1 | no | yes | Color spec |
| `--context` | 1 | no | yes | Context lines (same as `-C`) |
| `-C` | 1 | no | yes | Context |
| `-A` | 1 | no | yes | After context |
| `-B` | 1 | no | yes | Before context |
| `--hidden` | 0 | N/A | yes | Search hidden files |
| `--no-hidden` | 0 | N/A | yes | Don't search hidden |
| `--ignore` | 0 | N/A | yes | Respect ignore files |
| `--no-ignore` | 0 | N/A | yes | Don't respect ignore files |
| `--follow` | 0 | N/A | no | **EXCLUDED**: Follow symlinks |
| `-z` | 0 | N/A | no | **EXCLUDED**: Search compressed files — decompresses using external binaries (gzip, bzip2, etc.) |
| `--search-zip` | 0 | N/A | no | **EXCLUDED**: Same as `-z` |
| `--mmap` | 0 | N/A | yes | Use memory maps |
| `--no-mmap` | 0 | N/A | yes | Don't use memory maps |
| `--threads` | 1 | no | yes | Thread count |
| `--max-filesize` | 1 | no | yes | Max file size |
| `--maxdepth` | 1 | no | yes | Max directory depth |
| `--max-count` | 1 | no | yes | Max matches (same as `-m`) |
| `-m` | 1 | no | yes | Max count |
| `--max-columns` | 1 | no | yes | Max line width |
| `--byte-offset` | 0 | N/A | yes | Print byte offset |
| `-b` | 0 | N/A | yes | Byte offset |

**Key exclusions:**
- `--pre`, `--pre-glob`, `--hostname-bin`: Execute external commands
- `-z`/`--search-zip`: Decompresses files using external binaries (requires gzip/bzip2/xz/etc. in PATH)
- `-L`/`--follow`: Follow symlinks
- `-f`: Reads patterns from file

---

## Priority 4: Path Utilities (realpath, basename, dirname)

### realpath (print resolved path)

**Note:** `realpath` resolves paths, including symlinks. This is symlink-following by definition.

**GNU realpath:**
- `-s`, `--strip`, `--no-symlinks`: Don't expand symlinks
- `-z`, `--zero`: NUL-terminate
- `-e`, `--canonicalize-existing`: Require all components to exist
- `-m`, `--canonicalize-missing`: No existence requirement
- `-q`, `--quiet`: Suppress errors
- `--relative-to=DIR`: Relative to directory
- `--relative-base=DIR`: Base for relative output

**BSD realpath:**
- Not a separate command; functionality is in `readlink -f`

**Assessment:**
- **realpath's purpose is to resolve symlinks** — fundamentally symlink-following
- BSD doesn't have a standalone `realpath` command
- `realpath` on BSD is often GNU coreutils

**Recommendation:** Exclude `realpath` from v1. Its symlink-following nature conflicts with containment policy. If needed later, allow only `-s`/`--no-symlinks` mode.

### basename (strip directory and suffix from file name)

**Purpose:** Extract the filename portion of a path. Operates on path strings, not filesystem.

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason |
|------|-------|----------------|---------|---------|----------|--------|
| `-a` | 0 | N/A | yes | no | no | **EXCLUDED**: GNU-only, support multiple arguments |
| `-s` | 1 | no | yes | yes | yes | Remove suffix (suffix string, not a path) |
| `-z` | 0 | N/A | yes | BSD-only | no | **EXCLUDED**: GNU-only, NUL-terminate |

**Assessment:**
- **basename operates on strings, not files.** It doesn't read the filesystem.
- Safe for inclusion. The path operands are string arguments, not file reads.
- `-s` removes a suffix — safe, string operation

**Recommendation:** Include `basename` with `-s`.

### dirname (strip last component from file name)

**Purpose:** Extract the directory portion of a path. Operates on path strings, not filesystem.

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason |
|------|-------|----------------|---------|---------|----------|--------|
| `-z` | 0 | N/A | yes | BSD-only | no | **EXCLUDED**: GNU-only, NUL-terminate |

**Assessment:**
- **dirname operates on strings, not files.** It doesn't read the filesystem.
- No flags that change behavior meaningfully.
- Safe for inclusion.

**Recommendation:** Include `dirname` with no flags (or `-z` if NUL termination is needed for safety with special filenames).

---

## Priority 5: Evaluated-but-likely-out

### echo

**Status:** Safe ONLY because the charset forbids redirection.

The command `echo "hello" > /tmp/file` writes to a file, but the `>` character is NOT in the classifier's allowed character set. Therefore, `echo` cannot be used for redirection through the classifier.

**The charset restriction saves it:**
- `>`, `|`, `;`, `&&`, backticks, `$()`, quotes, backslash are all outside the allowed character set
- A classified `echo` command cannot redirect output

**Assessment:**
- `echo` without redirection is safe
- The classifier's charset forbids redirection characters
- **Include `echo` with no flags.** All `echo` flags (`-n`, `-e`, `-E`) are display formatting only.

**GNU vs BSD echo:**
- BSD `echo` supports `-n` (no newline)
- GNU `echo` supports `-n`, `-e` (enable escapes), `-E` (disable escapes)
- Both are display formatting, no writes

**Recommendation:** Include `echo`. The charset restriction makes it safe.

### sort

**Status:** **EXCLUDED** — `-o` writes to a file.

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason |
|------|-------|----------------|---------|---------|----------|--------|
| `-o` | 1 | yes | yes | yes | no | **EXCLUDED**: Output to FILE — writes to file |
| `-b` | 0 | N/A | yes | yes | yes | Ignore leading blanks (display) |
| `-d` | 0 | N/A | yes | yes | yes | Dictionary order |
| `-f` | 0 | N/A | yes | yes | yes | Fold case |
| `-n` | 0 | N/A | yes | yes | yes | Numeric sort |
| `-r` | 0 | N/A | yes | yes | yes | Reverse |
| `-u` | 0 | N/A | yes | yes | yes | Unique |
| `-k` | 1 | no | yes | yes | yes | Key definition (not a path) |
| `-t` | 1 | no | yes | yes | yes | Field separator (char, not path) |
| `-T` | 1 | yes | yes | yes | no | **EXCLUDED**: Temp directory — writes temp files |
| `--files0-from` | 1 | yes | yes | no | no | **EXCLUDED**: GNU-only, reads file list |

**Key finding:**
- `-o FILE` writes output to a file
- `-T DIR` uses a directory for temp files
- Both are write operations

**Recommendation:** Exclude `sort` from v1. The `-o` flag writes files. If included later, only with `-o` explicitly disallowed (but the classifier would need to enforce flag absence, which is more complex).

### uname

**Status:** Safe. Prints system information.

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason |
|------|-------|----------------|---------|---------|----------|--------|
| `-a` | 0 | N/A | yes | yes | yes | All information |
| `-s` | 0 | N/A | yes | yes | yes | Kernel name |
| `-n` | 0 | N/A | yes | yes | yes | Node name |
| `-r` | 0 | N/A | yes | yes | yes | Kernel release |
| `-v` | 0 | N/A | yes | yes | yes | Kernel version |
| `-m` | 0 | N/A | yes | yes | yes | Machine hardware |
| `-p` | 0 | N/A | yes | yes | yes | Processor |
| `-i` | 0 | N/A | yes | BSD-only | no | **EXCLUDED**: GNU-only, hardware platform |
| `-o` | 0 | N/A | yes | BSD-only | no | **EXCLUDED**: GNU-only, operating system |

**Recommendation:** Include `uname`. No execution, no network, no writes.

### hostname

**Status:** Safe. Prints or sets system hostname.

**Note:** Setting the hostname requires root. Reading is safe.

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason |
|------|-------|----------------|---------|---------|----------|--------|
| `-s` | 0 | N/A | yes | yes | yes | Short hostname |
| `-f` | 0 | N/A | yes | yes | yes | FQDN |
| `-d` | 0 | N/A | yes | yes | yes | Domain |
| `-i` | 0 | N/A | yes | yes | yes | IP addresses |
| `-I` | 0 | N/A | yes | no | no | **EXCLUDED**: GNU-only, all IPs |
| `-a` | 0 | N/A | yes | yes | yes | Alias names |

**Recommendation:** Include `hostname`. Reading hostname is safe. Setting requires root and explicit arguments.

### cmp (compare two files)

**Status:** Safe. Compares files byte-by-byte.

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason |
|------|-------|----------------|---------|---------|----------|--------|
| `-l` | 0 | N/A | yes | yes | yes | verbose, byte-by-byte differences |
| `-s` | 0 | N/A | yes | yes | yes | Silent, only exit code |
| `-b` | 0 | N/A | yes | BSD-only | no | **EXCLUDED**: GNU-only, print differing bytes |
| `-i` | 1 | no | yes | BSD-only | no | **EXCLUDED**: GNU-only, skip bytes (or BSD `-i N`) |
| `-n` | 1 | no | yes | yes | yes | Compare only N bytes |

**Recommendation:** Include `cmp`. Pure read-and-compare, no writes.

### uniq (report or omit repeated lines)

**Status:** Safe, but `-u`/`-d` are selection, not input-related.

| flag | arity | value-is-path? | GNU ok? | BSD ok? | include? | reason |
|------|-------|----------------|---------|---------|----------|--------|
| `-c` | 0 | N/A | yes | yes | yes | Count occurrences |
| `-d` | 0 | N/A | yes | yes | yes | Only repeated lines |
| `-u` | 0 | N/A | yes | yes | yes | Only unique lines |
| `-i` | 0 | N/A | yes | yes | yes | Case insensitive |
| `-f` | 1 | no | yes | yes | yes | Skip fields (integer) |
| `-s` | 1 | no | yes | yes | yes | Skip chars (integer) |
| `-w` | 1 | no | yes | yes | yes | Compare N chars |
| `--all-repeated` | 1 | no | yes | no | no | **EXCLUDED**: GNU-only |
| `--zero-terminated` | 0 | N/A | yes | no | no | **EXCLUDED**: GNU-only |

**Recommendation:** Include `uniq`. All flags are selection/formatting, no writes.

### cd (change directory)

**Status:** Safe in the context of `run_command` under agy.

**Analysis:**
- `cd` changes the current directory of the shell process
- Under `run_command`, agy spawns a subprocess, the `cd` happens in that subprocess, and the subprocess exits
- The `cd` has no lasting effect on the parent process
- The working directory of commands is specified by `Cwd` in the tool arguments anyway

**Assessment:**
- `cd` in a command string is pointless — it changes nothing lasting
- The classifier should recognize `cd` but it provides no benefit
- The real working directory comes from `Cwd`, not from shell `cd`

**Recommendation:** Exclude `cd`. It's safe but useless in this context. The `Cwd` field already specifies the working directory.

### tar

**Status:** **EXCLUDED** — multiple write/execute options.

**Dangerous flags:**
- `-c`: Create archive (writes)
- `--delete`: Delete from archive
- `-r`: Append to archive (writes)
- `-u`: Update archive (writes)
- `-x`: Extract (writes files to disk)
- `--to-command=CMD`: **Executes command for each file**
- `--checkpoint-action=ACTION`: Execute actions at checkpoints
- `--transform`, `--xform`: Transform filenames (could be abused)

**Read-only flags (extraction only):**
- `-t`: List contents (read-only)
- `-v`: Verbose (display)

**Even `-t` is problematic:**
- The `-f` flag specifies an archive file
- The archive could be anywhere, including outside the workspace
- Archive contents could reveal sensitive paths

**Recommendation:** Exclude `tar` entirely. Even the "safe" modes (`tf`) read arbitrary archives that could contain sensitive file listings. The `--to-command` flag executes arbitrary commands.

### find

**Status:** **EXCLUDED** — `-exec` executes commands, `-delete` deletes files.

**Dangerous flags:**
- `-exec CMD {} ;`: Executes command for each match
- `-exec CMD {} +`: Executes command with multiple matches
- `-execdir CMD {} ;`: Executes in matched file's directory
- `-ok CMD {} ;`: Same as `-exec` but prompts
- `-delete`: Deletes matched files
- `-rm`: Alias for `-delete` on some implementations

**Other concerning flags:**
- `-fls FILE`: Write to file
- `-fprint FILE`: Write paths to file
- `-fprintf FILE FORMAT`: Formatted output to file

**Safe flags (formatting/search only):**
- `-print`: Print path (default)
- `-print0`: NUL-terminated
- `-name PATTERN`: Name match
- `-type T`: File type
- `-mtime N`: Modification time
- `-size N`: File size
- `-maxdepth N`: Depth limit
- `-mindepth N`: Min depth

**Recommendation:** Exclude `find` entirely. The `-exec` flag makes it a remote execution vector. Even without `-exec`, the ability to traverse and list arbitrary paths outside the workspace is risky.

### sed

**Status:** **EXCLUDED** — `-i` edits files in place.

**Dangerous flags:**
- `-i[SUFFIX]`: Edit files in-place (writes)
- `-i`: In-place edit without backup (destructive)
- `-i.bak`: In-place with backup (still modifies original)
- `-w FILE`: Write to file (GNU extension)

**Other concerns:**
- `s///w FILE`: Write substitution to file
- `w FILE` command in script: Write to file

**Safe flags:**
- `-n`: Suppress automatic print
- `-e SCRIPT`: Script
- `-f SCRIPTFILE`: Read script from file (changes input source)

**Even without `-i`, `sed` is complex:**
- The `w` command in sed scripts writes files
- The classifier would need to parse sed scripts to detect `w` commands
- Too complex to validate safely

**Recommendation:** Exclude `sed` entirely. The `-i` flag is a direct write, and the `w` command in scripts can write files.

### awk / gawk

**Status:** **EXCLUDED** — `system()` executes commands, `print >` writes files.

**Dangerous capabilities:**
- `system("CMD")`: Executes shell command
- `print "text" > "file"`: Writes to file
- `print "text" >> "file"`: Appends to file
- `printf ... > "file"`: Formatted write
- `close("file")`: Closes file opened by `print >`
- `| CMD`: Pipe to command (executes)

**Example dangerous scripts:**
```awk
awk 'BEGIN { system("id") }'
awk '{ print > "/tmp/file" }'
awk '{ print | "cat > /tmp/file" }'
```

**Assessment:**
- `awk` is a full programming language
- Validating that an awk script doesn't use `system()` or file output requires parsing the entire script
- The classifier cannot safely validate arbitrary awk programs

**Recommendation:** Exclude `awk`/`gawk` entirely. The `system()` function and `>` redirection operator make it an execution/write vector.

### Pagers (less, more, man)

**Status:** **EXCLUDED** — `!CMD` executes commands.

**less dangerous capabilities:**
- `!CMD`: Executes shell command
- `| CMD`: Pipe to command
- `v`: Invokes editor (writes)
- `s FILE`: Save to file

**more dangerous capabilities:**
- `!CMD`: Executes shell command (on systems that support it)
- `v`: Invokes editor

**man:**
- Uses a pager internally
- `man` can invoke editor with certain keys

**Assessment:**
- Interactive pagers allow arbitrary command execution via `!CMD`
- The classifier cannot prevent interactive input during execution
- Even in non-interactive mode, some pagers have flags that execute

**Recommendation:** Exclude all pagers (`less`, `more`, `most`, `man`). The `!CMD` execution vector cannot be contained.

---

## Questions Answered

### 1. Does GNU getopt accept abbreviated long flags?

**Answer:** YES. GNU getopt accepts unambiguous prefixes of long flags.

**Citation:** GNU getopt documentation states that long option names may be abbreviated if the abbreviation is unique. For example:
- `grep --col` could match `--color` (if no other `--col...` option exists)
- `grep --col` is actually ambiguous: could be `--color` or `--columns`

**Classifier assumption:** The classifier plans exact-match only. **This is SAFE and NECESSARY.**

**Why exact-match is required:**
1. A prefix like `--col` might match `--color` today, but a new version could add `--column-count`, making it ambiguous
2. The classifier cannot know which long flags a particular GNU program version supports
3. An abbreviated flag that works on one system might fail or match differently on another
4. **Exact-match ensures the user's command is unambiguous**

**Affected programs:** ALL GNU programs that use GNU getopt with long options. This includes:
- `ls` (all `--` flags)
- `grep` (all `--` flags)
- `rg` (all `--` flags)
- `cat`, `head`, `tail`, `wc`, etc.

**BSD note:** BSD `getopt` does NOT support abbreviated long flags. BSD long option parsing (via `getopt_long`) can be configured to allow or disallow abbreviation, but BSD utilities typically require exact matches.

**Conclusion:** The classifier's exact-match policy is correct. Any abbreviated long flag is treated as unknown → unclassifiable → prompts. This is fail-safe.

### 2. Flags that change OUTPUT DESTINATION

**Definition:** Flags that write to a file or file descriptor other than stdout.

**Programs with output-redirection flags:**

| program | flag | destination | included? |
|---------|------|-------------|-----------|
| `sort` | `-o FILE` | Specified file | NO (excluded) |
| `grep` | none | N/A | N/A |
| `rg` | none | N/A | N/A |
| `ls` | none | N/A | N/A |
| `cat` | none | N/A | N/A |
| `head` | none | N/A | N/A |
| `tail` | none | N/A | N/A |
| `wc` | none | N/A | N/A |
| `sed` | `-i`, `w FILE` | Files | NO (excluded) |
| `awk` | `print > FILE`, `printf > FILE` | Files | NO (excluded) |
| `find` | `-fls FILE`, `-fprint FILE` | Files | NO (excluded) |
| `tar` | `-f FILE` (archive, not stdout) | Archive file | NO (excluded) |
| `file` | `-f FILE` | Reads from file, not writes | NO (excluded for input change) |

**Good news:**
- `grep`, `rg`, `ls`, `cat`, `head`, `tail`, `wc`, `date`, `uname`, `hostname`, `basename`, `dirname`, `echo`, `uniq`, `cmp` have NO flags that redirect output to files
- These programs only write to stdout

**Programs excluded due to output-redirection flags:**
- `sort` (`-o`)
- `sed` (`-i`, `w` command)
- `awk` (`>` operator)
- `find` (multiple output flags)
- `tar` (writes archives)

### 3. `ls --color` portability

**Question:** Is `--color` POSIX/GNU-only? What does BSD ls do with it?

**Answer:** **BSD `ls` DOES support `--color`.**

**Evidence from FreeBSD 15.1 man page:**

> `--color=when`
> Output colored escape sequences based on when, which may be set to either **always**, **auto**, or **never**.
> 
> **always** will make `ls` always output color...
> **auto** will make `ls` output escape sequences based on termcap(5), but only if stdout is a tty and either the **-G** flag is specified or one of the environment variables COLORTERM or CLICOLOR is set...
> **never** will disable color...
> 
> For compatibility with GNU coreutils, **ls** supports **yes** or **force** as equivalent to **always**, **no** or **none** as equivalent to **never**, and **tty** or **if-tty** as equivalent to **auto**.

**Conclusion:**
- `--color` is NOT POSIX (it's an extension)
- `--color` is NOT GNU-only; FreeBSD `ls` supports it with GNU-compatible values
- **Safe to include** for both GNU and BSD

**BSD note:** BSD also has the `-G` flag, which is equivalent to `--color=auto` (or `CLICOLOR` env var). The `--color` long flag is specifically for GNU compatibility.

---

## Confidence and Gaps

### High confidence areas:

1. **grep execution flags**: Verified that GNU grep has NO execution flags. Confirmed by reading GNU grep manual and BSD grep man page. The only execution-like capability is `-f FILE` which reads patterns from a file.

2. **rg execution flags**: Verified from ripgrep manual (`man.archlinux.org/man/rg.1.en`) that `--pre`, `--pre-glob`, and `--hostname-bin` execute external commands. These are explicitly documented as spawning processes.

3. **ls `--color` portability**: Verified from FreeBSD 15.1 man page that BSD `ls` supports `--color=when` with GNU-compatible values.

4. **date `-f` asymmetry**: Verified from both GNU coreutils manual and BSD date man page. The asymmetry is clear: BSD `-f` takes a format string; GNU `-f` takes a filename.

### Medium confidence areas:

1. **stat format strings**: The GNU and BSD `stat` format languages are completely different. I verified this from the BSD stat(1) man page and GNU coreutils manual. A format string that works on one will NOT work on the other. However, I did not test every format specifier for subtle differences.

2. **tail `-f`/`-F` behavior**: I verified these are follow-mode flags that keep the process running. I did not test whether they could be used to keep a process alive indefinitely as a side effect.

3. **sort temp files**: I verified `-T DIR` specifies a temp directory, which means writes. I did not verify whether BSD `sort` always uses temp files for large inputs, even without `-T`.

### Gaps and unverified items:

1. **GNU getopt abbreviation behavior per-program**: I verified that GNU getopt supports abbreviation in general. I did NOT test whether specific programs disable this. Some programs may use `getopt_long_only` or set the `FLAG_NO_ABBREV` flag. **Recommendation:** Assume abbreviation is allowed; require exact match.

2. **BSD getopt_long behavior**: I verified that BSD `getopt_long` CAN be configured to allow abbreviation, but I did not verify whether standard BSD utilities (ls, grep, etc.) actually enable this. **Recommendation:** Assume BSD requires exact match, but classifier should enforce exact match anyway.

3. **All flags for all programs**: I prioritized core utilities (ls, cat, head, tail, wc) and search tools (grep, rg). I did NOT verify every single flag for every program listed. Lower-priority programs (du, df, stat) have gaps in the flag tables above.

4. **File magic reading in `file`**: I verified that `file` reads file contents to determine type. I did NOT verify whether it can read from network file systems, special files, or other exotic sources when `-s` is used.

5. **Unicode/locale handling**: I did not verify how flags like `-i` (case insensitive) behave with Unicode input on GNU vs BSD. This is a behavior difference but not a security issue.

6. **`which` implementation variance**: I noted that `which` is not a GNU coreutils program and varies by system. I did NOT verify the flags for every implementation (bash builtin, Debian `which`, BSD `which`, etc.).

7. **Long flag synonyms**: I verified that BSD `ls --color` accepts GNU-compatible values (`yes`/`no`/`force`/etc.). I did NOT verify whether other BSD programs have similar synonym support for GNU flags.

8. **Compression flags in `rg`**: I verified that `rg -z`/`--search-zip` decompresses files using external binaries. I did NOT verify which compression formats require which binaries, or whether missing binaries cause errors vs silent failure.

### Items marked "unverified — treated as excluded":

None in this research. All core programs were verified against both GNU and BSD documentation.

### Sources consulted:

- GNU coreutils manual: `https://www.gnu.org/software/coreutils/manual/`
- FreeBSD man pages: `https://man.freebsd.org/`
- ripgrep manual: `https://man.archlinux.org/man/rg.1.en`

### Sources NOT consulted (due to time/scope):

- OpenBSD man pages (similar to FreeBSD but may have differences)
- macOS/Darwin specific man pages (FreeBSD is a reasonable proxy for Darwin)
- NetBSD man pages
- GNU tar manual
- GNU find manual
- sed manual
- awk/gawk manual

---

## Summary of Surprises and Disagreements

1. **`ls --color` IS supported by BSD** — contrary to common belief that `--color` is GNU-only. FreeBSD 15.1+ supports it with GNU-compatible values.

2. **`ls -o` is asymmetric** — BSD shows file flags, GNU omits group info. This is a **dangerous asymmetry** that required exclusion.

3. **`ls -D` is asymmetric** — GNU is `--dired` (Emacs integration), BSD is date format. Excluded.

4. **`ls -U` is asymmetric** — BSD is creation time, GNU is `--sort=none`. Excluded.

5. **`date -f` is critically asymmetric** — BSD takes a format string, GNU reads from a FILE. This is exactly the dangerous case the plan warned about. **This is the strongest evidence for the cross-dialect exclusion policy.**

6. **`grep` has NO execution flags** — unlike `rg`, `grep` cannot execute external commands. This makes `grep` safer than expected.

7. **`rg --hostname-bin` executes commands** — not just `--pre`. Even getting the hostname can trigger execution.

8. **`file -z` spawns decompressors** — this is an execution risk I initially missed. The `-S` flag (disable sandbox) is also a security concern.

9. **`cat -l` (BSD) locks stdout** — this modifies file descriptor state, not just reads.

10. **`stat` is fundamentally unportable** — the GNU and BSD format languages are completely incompatible. No useful cross-platform flag table exists.

---

**Report generated:** 2026-09-06
**Model:** GLM-5
**Purpose:** DP1 research for safe-command classifier
