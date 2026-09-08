//! One-and-done approval for invocations of read-only programs.
//!
//! `run_command` answers used to be keyed by the exact argument string, so
//! `ls` approved once reprompted on the next path. This module recognizes when
//! a raw `CommandLine` is a single invocation of an allowlisted read-only
//! program with no shell interpretation in play, and extracts its path
//! arguments so the bridge can re-check them against containment on every
//! later call. A remembered "Always" may then cover the *program* instead of
//! the string.
//!
//! The tokenizer is a character allowlist, not a metacharacter denylist: any
//! token carrying a character outside the admitted set makes the whole command
//! unclassifiable, so shell constructs the classifier never heard of —
//! including ones zsh adds tomorrow — fail closed by construction. Every parse
//! error, ambiguity, or unknown flag or program means "not classifiable" and
//! falls back to today's exact-string keying; a miss is never an allow.
//!
//! What this does *not* promise, by design: the allowlist constrains the
//! command *string*; what `ls` resolves to comes from the machine's `PATH`,
//! which the bridge does not control (agy inherits the ambient environment),
//! so PATH shadowing is outside the boundary and stated as such wherever this
//! is described. Containment is judged at authorize time while the program
//! opens paths later, so a workspace symlink swapped between the check and the
//! run is the same TOCTOU gap the path tools already have — a property of the
//! bridge's containment model, not of this widening.
//!
//! If the deferred work of parsing what a command *does* (containment depth)
//! is ever taken, it reuses this parser rather than growing a second one.

use serde_json::Value;

/// A classified command: which allowlisted program it invokes, and every path
/// argument the bridge must re-check on each matching call. `program` points
/// into the static table below, never into the model's input.
pub(super) struct SafeCommand {
    pub(super) program: &'static ProgramDef,
    pub(super) paths: Vec<String>,
}

/// One allowlisted program. `noun` is the pre-formatted prompt phrase so the
/// label carries no model-authored text even if table lookup were bypassed.
pub(super) struct ProgramDef {
    pub(super) name: &'static str,
    pub(super) noun: &'static str,
    pub(super) flags: &'static [FlagDef],
    pub(super) operands: OperandPolicy,
    /// Least operands that still read something the bridge can see. `cat`
    /// with none reads stdin, which is opaque to containment.
    pub(super) min_operands: usize,
    /// Most operands allowed. `pwd` takes none; anything else is rejected.
    pub(super) max_operands: Option<usize>,
    /// For `grep`/`rg`: the first operand is the pattern when no `-e`/`-f`
    /// was given, so one more operand is needed before any file is read.
    pub(super) pattern_first: bool,
}

/// How leftover (non-flag) tokens are treated.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OperandPolicy {
    /// Every operand is a candidate path (checked per call).
    Paths,
    /// Operands are command names, not paths (`which`): validated as tokens,
    /// never containment-checked.
    Names,
    /// Operands must be `+format` strings (`date`): validated, never paths.
    Formats,
}

/// One permitted flag. Unknown flags unclassify: a new flag nobody classified
/// must not silently widen.
pub(super) struct FlagDef {
    /// Long form without dashes (`Some("color")`); `None` for short-only.
    pub(super) long: Option<&'static str>,
    /// Short form (`Some('l')`); `None` for long-only.
    pub(super) short: Option<char>,
    /// 0 = bare switch; 1 = takes a value, which is extracted.
    pub(super) arity: u8,
    /// Whether an arity-1 value is a path the bridge must check.
    pub(super) value_is_path: bool,
    /// Only the attached spelling is admitted (`--color=WHEN`, `-IFMT`): a
    /// bare occurrence would swallow the following operand as its "value" and
    /// skip path-checking it.
    pub(super) attached_only: bool,
}

const fn flag(long: Option<&'static str>, short: Option<char>) -> FlagDef {
    FlagDef {
        long,
        short,
        arity: 0,
        value_is_path: false,
        attached_only: false,
    }
}

const fn value_flag(
    long: Option<&'static str>,
    short: Option<char>,
    value_is_path: bool,
) -> FlagDef {
    FlagDef {
        long,
        short,
        arity: 1,
        value_is_path,
        attached_only: false,
    }
}

const fn attached_flag(long: Option<&'static str>, short: Option<char>) -> FlagDef {
    FlagDef {
        long,
        short,
        arity: 1,
        value_is_path: false,
        attached_only: true,
    }
}

// Short-flag clusters, written out per table entry in
// dev-docs/research/dp1-flag-tables.md. Single letters below are arity 0.
const fn shorts(short: char) -> FlagDef {
    FlagDef {
        long: None,
        short: Some(short),
        arity: 0,
        value_is_path: false,
        attached_only: false,
    }
}

const LS_FLAGS: &[FlagDef] = &[
    shorts('l'),
    shorts('a'),
    shorts('A'),
    shorts('h'),
    shorts('d'),
    shorts('F'),
    shorts('p'),
    shorts('R'),
    shorts('r'),
    shorts('S'),
    shorts('t'),
    shorts('1'),
    shorts('C'),
    shorts('i'),
    shorts('n'),
    shorts('v'),
    shorts('P'),
    attached_flag(Some("color"), None),
    flag(Some("group-directories-first"), None),
];

const CAT_FLAGS: &[FlagDef] = &[
    shorts('n'),
    shorts('b'),
    shorts('s'),
    shorts('v'),
    shorts('e'),
    shorts('t'),
    shorts('u'),
];

const HEAD_FLAGS: &[FlagDef] = &[
    value_flag(None, Some('n'), false),
    value_flag(None, Some('c'), false),
    value_flag(Some("lines"), None, false),
    value_flag(Some("bytes"), None, false),
];

const TAIL_FLAGS: &[FlagDef] = &[
    value_flag(None, Some('n'), false),
    value_flag(None, Some('c'), false),
    shorts('q'),
    shorts('v'),
    value_flag(Some("lines"), None, false),
    value_flag(Some("bytes"), None, false),
    flag(Some("quiet"), None),
    flag(Some("silent"), None),
];

const WC_FLAGS: &[FlagDef] = &[
    shorts('l'),
    shorts('w'),
    shorts('c'),
    shorts('m'),
    shorts('L'),
];

const FILE_FLAGS: &[FlagDef] = &[
    shorts('b'),
    shorts('i'),
    shorts('I'),
    shorts('N'),
    shorts('n'),
    shorts('0'),
    shorts('h'),
    shorts('k'),
    flag(Some("mime-type"), None),
    flag(Some("mime-encoding"), None),
    flag(Some("extension"), None),
    flag(Some("exclude-quiet"), None),
];

const STAT_FLAGS: &[FlagDef] = &[
    shorts('l'),
    shorts('r'),
    shorts('s'),
    shorts('x'),
    shorts('F'),
    shorts('n'),
    shorts('q'),
    shorts('h'),
    value_flag(None, Some('t'), false),
];

const PWD_FLAGS: &[FlagDef] = &[shorts('L'), shorts('P')];

const DU_FLAGS: &[FlagDef] = &[
    shorts('a'),
    shorts('A'),
    shorts('c'),
    shorts('h'),
    shorts('k'),
    shorts('m'),
    shorts('g'),
    shorts('s'),
    shorts('x'),
    shorts('P'),
    shorts('n'),
    value_flag(None, Some('d'), false),
    value_flag(None, Some('B'), false),
    value_flag(None, Some('I'), false),
    value_flag(None, Some('t'), false),
];

const DF_FLAGS: &[FlagDef] = &[
    shorts('a'),
    shorts('c'),
    shorts('h'),
    shorts('H'),
    shorts('k'),
    shorts('m'),
    shorts('g'),
    shorts('b'),
    shorts('P'),
    shorts('i'),
    shorts('I'),
    shorts('l'),
    shorts('n'),
];

const DATE_FLAGS: &[FlagDef] = &[
    shorts('u'),
    shorts('R'),
    shorts('j'),
    shorts('n'),
    attached_flag(None, Some('I')),
    value_flag(None, Some('z'), false),
    value_flag(None, Some('v'), false),
];

const WHICH_FLAGS: &[FlagDef] = &[shorts('a'), shorts('s')];

const BASENAME_FLAGS: &[FlagDef] = &[shorts('a'), value_flag(None, Some('s'), false)];

const GREP_FLAGS: &[FlagDef] = &[
    shorts('i'),
    shorts('v'),
    shorts('c'),
    shorts('l'),
    shorts('L'),
    shorts('n'),
    shorts('q'),
    shorts('o'),
    shorts('w'),
    shorts('x'),
    shorts('F'),
    shorts('E'),
    shorts('G'),
    shorts('s'),
    shorts('h'),
    shorts('H'),
    shorts('b'),
    shorts('a'),
    shorts('U'),
    shorts('p'),
    value_flag(None, Some('e'), false),
    value_flag(None, Some('f'), true),
    value_flag(None, Some('m'), false),
    value_flag(None, Some('A'), false),
    value_flag(None, Some('B'), false),
    value_flag(None, Some('C'), false),
    value_flag(Some("label"), None, false),
    flag(Some("line-buffered"), None),
    flag(Some("null"), None),
    value_flag(Some("exclude"), None, false),
    value_flag(Some("exclude-dir"), None, false),
    value_flag(Some("include"), None, false),
    value_flag(Some("include-dir"), None, false),
];

const RG_FLAGS: &[FlagDef] = &[
    shorts('i'),
    shorts('v'),
    shorts('c'),
    shorts('l'),
    shorts('n'),
    shorts('q'),
    shorts('o'),
    shorts('w'),
    shorts('x'),
    shorts('F'),
    shorts('a'),
    shorts('u'),
    value_flag(None, Some('e'), false),
    value_flag(None, Some('t'), false),
    value_flag(None, Some('T'), false),
    value_flag(None, Some('g'), false),
    value_flag(None, Some('E'), false),
    value_flag(None, Some('C'), false),
    value_flag(None, Some('A'), false),
    value_flag(None, Some('B'), false),
    value_flag(None, Some('M'), false),
    flag(Some("hidden"), None),
    flag(Some("no-ignore"), None),
    flag(Some("smart-case"), None),
    flag(Some("json"), None),
    flag(Some("debug"), None),
    value_flag(Some("max-columns"), None, false),
    value_flag(Some("sort"), None, false),
    value_flag(Some("replace"), None, false),
];

/// Programs that read and cannot write or execute. Read the per-program
/// rulings, exclusions, and open items alongside the tables, not instead of
/// them: a name here is a claim its flags were audited.
static PROGRAMS: &[ProgramDef] = &[
    ProgramDef {
        name: "ls",
        noun: "`ls` commands",
        flags: LS_FLAGS,
        operands: OperandPolicy::Paths,
        min_operands: 0,
        max_operands: None,
        pattern_first: false,
    },
    ProgramDef {
        name: "cat",
        noun: "`cat` commands",
        flags: CAT_FLAGS,
        operands: OperandPolicy::Paths,
        min_operands: 1,
        max_operands: None,
        pattern_first: false,
    },
    ProgramDef {
        name: "head",
        noun: "`head` commands",
        flags: HEAD_FLAGS,
        operands: OperandPolicy::Paths,
        min_operands: 1,
        max_operands: None,
        pattern_first: false,
    },
    ProgramDef {
        name: "tail",
        noun: "`tail` commands",
        flags: TAIL_FLAGS,
        operands: OperandPolicy::Paths,
        min_operands: 1,
        max_operands: None,
        pattern_first: false,
    },
    ProgramDef {
        name: "wc",
        noun: "`wc` commands",
        flags: WC_FLAGS,
        operands: OperandPolicy::Paths,
        min_operands: 1,
        max_operands: None,
        pattern_first: false,
    },
    ProgramDef {
        name: "file",
        noun: "`file` commands",
        flags: FILE_FLAGS,
        operands: OperandPolicy::Paths,
        min_operands: 0,
        max_operands: None,
        pattern_first: false,
    },
    ProgramDef {
        name: "stat",
        noun: "`stat` commands",
        flags: STAT_FLAGS,
        operands: OperandPolicy::Paths,
        min_operands: 0,
        max_operands: None,
        pattern_first: false,
    },
    ProgramDef {
        name: "pwd",
        noun: "`pwd` commands",
        flags: PWD_FLAGS,
        operands: OperandPolicy::Paths,
        min_operands: 0,
        max_operands: Some(0),
        pattern_first: false,
    },
    ProgramDef {
        name: "du",
        noun: "`du` commands",
        flags: DU_FLAGS,
        operands: OperandPolicy::Paths,
        min_operands: 0,
        max_operands: None,
        pattern_first: false,
    },
    ProgramDef {
        name: "df",
        noun: "`df` commands",
        flags: DF_FLAGS,
        operands: OperandPolicy::Paths,
        min_operands: 0,
        max_operands: None,
        pattern_first: false,
    },
    ProgramDef {
        name: "date",
        noun: "`date` commands",
        flags: DATE_FLAGS,
        operands: OperandPolicy::Formats,
        min_operands: 0,
        max_operands: None,
        pattern_first: false,
    },
    ProgramDef {
        name: "which",
        noun: "`which` commands",
        flags: WHICH_FLAGS,
        operands: OperandPolicy::Names,
        min_operands: 0,
        max_operands: None,
        pattern_first: false,
    },
    ProgramDef {
        name: "basename",
        noun: "`basename` commands",
        flags: BASENAME_FLAGS,
        operands: OperandPolicy::Paths,
        min_operands: 0,
        max_operands: None,
        pattern_first: false,
    },
    ProgramDef {
        name: "dirname",
        noun: "`dirname` commands",
        flags: &[],
        operands: OperandPolicy::Paths,
        min_operands: 0,
        max_operands: None,
        pattern_first: false,
    },
    ProgramDef {
        name: "grep",
        noun: "`grep` commands",
        flags: GREP_FLAGS,
        operands: OperandPolicy::Paths,
        min_operands: 0,
        max_operands: None,
        pattern_first: true,
    },
    ProgramDef {
        name: "rg",
        noun: "`rg` commands",
        flags: RG_FLAGS,
        operands: OperandPolicy::Paths,
        min_operands: 0,
        max_operands: None,
        pattern_first: true,
    },
];

/// Characters admitted anywhere in a token: alphanumerics plus the punctuation
/// a plain path or flag needs. Everything the shell interprets — and every
/// zsh-ism built from those characters — is absent, so unknown constructs
/// fail the charset before any rule names them.
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '+' | ',' | ':' | '@' | '%')
}

/// Long-flag names are letters, digits, and interior dashes.
fn is_long_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Whether a standalone word is shell-inert enough to appear where the
/// grammar expects an operand. Strict: `=` words are zsh assignment or
/// command substitution when leading, and the only `=` spelling admitted is
/// `--flag=value` (handled before this runs), so any other token carrying
/// one is out — even `ls FOO=bar`, which the shell would pass literally.
/// Under-matching costs a prompt; the asymmetry with [`valid_value`] is
/// deliberate. `~` expands anywhere but first: a leading `~` is
/// home-relative and refused by containment, anything else is out.
fn valid_word(token: &str) -> bool {
    if token.contains('=') {
        return false;
    }
    if let Some(rest) = token.strip_prefix('~') {
        return !rest.contains('~') && rest.chars().all(is_token_char);
    }
    token.chars().all(is_token_char)
}

/// Whether a flag value is shell-inert enough to consume. Looser than
/// [`valid_word`] on purpose: a value is consumed in a known-flag context,
/// where an interior `=` is inert data (`grep -e foo=bar`), while a leading
/// `=` is still zsh command substitution and `-` alone is still stdin.
fn valid_value(value: &str) -> bool {
    if value.is_empty() || value == "-" || value.starts_with('=') {
        return false;
    }
    if let Some(rest) = value.strip_prefix('~') {
        return !rest.contains('~') && rest.chars().all(|c| is_token_char(c) || c == '=');
    }
    value.chars().all(|c| is_token_char(c) || c == '=')
}

/// `Some` only when the raw command line is a single invocation of an
/// allowlisted read-only program, with no shell interpretation in play.
///
/// Anything else — separators, expansions, redirections, chains, quoting,
/// unknown programs or flags, a flag value the table does not account for —
/// is `None`, and the caller falls back to exact-string keying.
pub(super) fn classify(command_line: &str) -> Option<SafeCommand> {
    // Separator-whitespace and non-ASCII are refused before splitting: in zsh
    // a newline is a command separator, so splitting first would erase it.
    if command_line
        .bytes()
        .any(|b| matches!(b, b'\n' | b'\r' | 0x0b | b'\x0c' | 0) || b >= 0x80)
    {
        return None;
    }
    let mut tokens = command_line.split([' ', '\t']).filter(|t| !t.is_empty());
    let program_name = tokens.next()?;
    let program = PROGRAMS.iter().find(|p| p.name == program_name)?;

    let mut parser = Parser {
        program,
        saw_pattern_flag: false,
        operands: Vec::new(),
        paths: Vec::new(),
    };
    let mut tokens = tokens.peekable();
    while let Some(token) = tokens.next() {
        parser.token(token, &mut tokens)?;
    }
    parser.finish()
}

/// Classify the `CommandLine` of a `run_command` call, if it has one.
/// Unknown tools carrying a `CommandLine` stay out: only the audited
/// `execute` kind may widen.
pub(super) fn classify_call(tool_name: &str, args: &Value) -> Option<SafeCommand> {
    if super::sticky_rules::tool_kind(tool_name) != "execute" {
        return None;
    }
    classify(args.get("CommandLine")?.as_str()?)
}

struct Parser {
    program: &'static ProgramDef,
    saw_pattern_flag: bool,
    operands: Vec<String>,
    paths: Vec<String>,
}

impl Parser {
    /// Parse one token. `None` is unclassifiable, never a partial result.
    fn token<'a>(
        &mut self,
        token: &str,
        rest: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
    ) -> Option<()> {
        // `-` alone is stdin to half the table; `--` is given no semantics.
        // Either anywhere unclassifies the whole command.
        if token == "-" || token == "--" {
            return None;
        }
        if let Some(long) = token.strip_prefix("--") {
            return self.long_flag(long, rest);
        }
        if let Some(cluster) = token.strip_prefix('-') {
            // `strip_prefix('-')` on "--x" never reaches here; on "-" the
            // early return above fired. A lone "-" plus cluster logic below
            // would misread stdin as flags.
            if cluster.is_empty() {
                return None;
            }
            return self.short_cluster(cluster, rest);
        }
        self.operand(token)
    }

    /// A `--name` or `--name=value` token. Long flags match exactly: GNU
    /// getopt accepts unambiguous prefixes, and the classifier must not.
    fn long_flag<'a>(
        &mut self,
        long: &str,
        rest: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
    ) -> Option<()> {
        // `=` is allowed only in this shape. The value half obeys
        // [`valid_value`]: interior `=` is inert data in value position.
        if long.chars().filter(|&c| c == '=').count() > 1 {
            return None;
        }
        let (name, attached) = match long.split_once('=') {
            Some((name, value)) => {
                if !is_long_name(name) || !valid_value(value) {
                    return None;
                }
                (name, Some(value))
            }
            None => {
                if !is_long_name(long) {
                    return None;
                }
                (long, None)
            }
        };
        let def = self.program.flags.iter().find(|f| f.long == Some(name))?;
        if def.arity == 0 {
            return if attached.is_none() { Some(()) } else { None };
        }
        match attached {
            Some(value) => self.flag_value(def, value),
            None if def.attached_only => None,
            None => {
                let value = rest.next()?;
                self.separate_value(def, value)
            }
        }
    }

    /// A `-xyz` cluster, expanded character by character. Every character but
    /// the last must be an arity-0 short flag; the last may be arity 0, or
    /// arity 1 with the remainder of the token attached (`-n5`) or the next
    /// token separate (`-n 5`).
    fn short_cluster<'a>(
        &mut self,
        cluster: &str,
        rest: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
    ) -> Option<()> {
        let mut chars = cluster.chars().peekable();
        while let Some(c) = chars.next() {
            let def = self.program.flags.iter().find(|f| f.short == Some(c))?;
            if def.arity == 0 {
                continue;
            }
            let attached: String = chars.collect();
            if !attached.is_empty() {
                if def.attached_only {
                    // Attached-only flags (`-IFMT`) admit just this spelling.
                    return self.flag_value(def, &attached);
                }
                // A normal arity-1 flag also admits the attached spelling.
                return self.flag_value(def, &attached);
            }
            if def.attached_only {
                // A bare attached-only flag would swallow the next operand
                // as its "value" and skip path-checking it.
                return None;
            }
            let value = rest.next()?;
            return self.separate_value(def, value);
        }
        Some(())
    }

    /// A flag value in either spelling, after the word rules above. `-` alone
    /// is stdin, opaque to containment; a `-e` pattern starting with `-`
    /// mirrors getopt, which consumes the next token regardless.
    fn flag_value(&mut self, def: &FlagDef, value: &str) -> Option<()> {
        if !valid_value(value) {
            return None;
        }
        // `-e`/`-f` choose the pattern source for grep-likes; only `-f`
        // values name a file the bridge must check.
        if def.short == Some('e') || def.short == Some('f') {
            self.saw_pattern_flag = true;
        }
        if def.value_is_path {
            self.paths.push(value.to_string());
        }
        Some(())
    }

    /// A separate-token value (`--flag value`, `-o value`): same rules as the
    /// attached spelling, since getopt consumes the next token regardless of
    /// what it looks like. Only `-` alone is refused — stdin, opaque to
    /// containment.
    fn separate_value(&mut self, def: &FlagDef, value: &str) -> Option<()> {
        self.flag_value(def, value)
    }

    /// A positional token: validated, then treated per the program's operand
    /// policy. A token the policy cannot account for unclassifies.
    fn operand(&mut self, token: &str) -> Option<()> {
        if !valid_word(token) {
            return None;
        }
        match self.program.operands {
            OperandPolicy::Paths => {
                self.operands.push(token.to_string());
                self.paths.push(token.to_string());
            }
            OperandPolicy::Names | OperandPolicy::Formats => {
                if self.program.operands == OperandPolicy::Formats && !token.starts_with('+') {
                    // Setting the clock takes a non-`+` operand; only `+format`
                    // strings display without writing.
                    return None;
                }
                self.operands.push(token.to_string());
            }
        }
        Some(())
    }

    /// All tokens accounted for: enforce the operand counts that close the
    /// stdin hole, then emit.
    fn finish(self) -> Option<SafeCommand> {
        if let Some(max) = self.program.max_operands {
            if self.operands.len() > max {
                return None;
            }
        }
        if self.program.pattern_first {
            // Without `-e`/`-f` the first operand is the pattern, and the
            // command reads stdin unless a file operand follows.
            let files = if self.saw_pattern_flag {
                self.operands.len()
            } else {
                self.operands.len().saturating_sub(1)
            };
            if files == 0 {
                return None;
            }
        } else if self.operands.len() < self.program.min_operands {
            return None;
        }
        Some(SafeCommand {
            program: self.program,
            paths: self.paths,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The allowlisted program name, or `None`.
    fn program_of(cmd: &str) -> Option<&'static str> {
        classify(cmd).map(|c| c.program.name)
    }

    /// Extracted paths, or `None` when unclassifiable.
    fn paths_of(cmd: &str) -> Option<Vec<String>> {
        classify(cmd).map(|c| c.paths)
    }

    fn paths(cmd: &str) -> Vec<String> {
        paths_of(cmd).unwrap_or_else(|| panic!("expected {cmd:?} to classify"))
    }

    #[test]
    fn separators_and_chains_are_unclassifiable() {
        for cmd in [
            "ls; rm -rf x",
            "ls && rm x",
            "ls || rm x",
            "ls | tee /etc/x",
            "ls | cat",
            "ls & rm x",
            "ls;",
            "; ls",
            "ls > out",
            "ls >> out",
            "ls < in",
            "ls 2> err",
            "ls `id`",
            "ls $(id)",
            "ls $HOME",
            "ls ${HOME}/x",
            "ls $foo",
            "FOO=bar ls",
            "ls FOO=bar",
            "VAR = ls",
        ] {
            assert!(classify(cmd).is_none(), "{cmd:?} must not classify");
        }
    }

    #[test]
    fn separator_whitespace_is_unclassifiable() {
        for cmd in [
            "ls\nrm target",
            "ls\rrm",
            "ls\x0brm",
            "ls\x0crm",
            "ls\n",
            "\nls",
        ] {
            assert!(classify(cmd).is_none(), "{cmd:?} must not classify");
        }
    }

    #[test]
    fn quoting_escapes_and_nonascii_are_unclassifiable() {
        for cmd in [
            "ls 'my dir'",
            "ls \"my dir\"",
            "ls\\ x",
            "ls\\; x",
            "ls ''",
            "ls café",
            "ls naïve/x",
            "ls \u{0}",
        ] {
            assert!(classify(cmd).is_none(), "{cmd:?} must not classify");
        }
    }

    #[test]
    fn zsh_natives_are_unclassifiable_by_construction() {
        for cmd in [
            "ls <<< x",
            "ls =(id)",
            "ls ${=foo}",
            "ls *.zwc",
            "ls *",
            "ls foo?.txt",
            "ls [abc]",
            "ls {a,b}",
            "cat *.rs",
        ] {
            assert!(classify(cmd).is_none(), "{cmd:?} must not classify");
        }
    }

    #[test]
    fn unknown_programs_wrappers_and_paths_are_unclassifiable() {
        for cmd in [
            "frobnicate x",
            "find . -delete",
            "sed -i",
            "awk 'BEGIN{system(\"id\")}'",
            "tar tf x",
            "git status",
            "sudo ls",
            "env ls",
            "time ls",
            "nice ls",
            "command ls",
            "builtin ls",
            "exec ls",
            "./ls",
            "/bin/ls",
            "~/bin/ls",
            "LS",
            "Ls -l",
            "curl example.com",
            "python3 -c 1",
            "sh -c ls",
            "less file",
            "man ls",
            "vi file",
            "",
            "   ",
            "\t",
        ] {
            assert!(classify(cmd).is_none(), "{cmd:?} must not classify");
        }
    }

    #[test]
    fn dash_alone_is_never_classified() {
        for cmd in ["cat -", "cat file -", "head -", "ls -", "grep -e - f", "-"] {
            assert!(classify(cmd).is_none(), "{cmd:?} must not classify");
        }
    }

    #[test]
    fn zero_operand_stdin_readers_are_unclassifiable() {
        for cmd in [
            "cat",
            "head",
            "tail",
            "wc",
            "cat -n",
            "grep foo",
            "rg foo",
            "grep -e foo",
        ] {
            assert!(
                classify(cmd).is_none(),
                "{cmd:?} reads stdin or a pattern only"
            );
        }
        // These read the working directory or the clock, not stdin.
        for cmd in ["ls", "pwd", "du", "df", "date", "which", "file", "stat"] {
            assert!(classify(cmd).is_some(), "{cmd:?} must classify");
        }
    }

    #[test]
    fn equals_expansion_is_unclassifiable() {
        for cmd in [
            "ls =id",
            "cat =foo/bar",
            "ls FOO=bar",
            "FOO=bar ls",
            "head -n =5",
        ] {
            assert!(classify(cmd).is_none(), "{cmd:?} must not classify");
        }
        // Interior `=` is inert data in a consumed flag-value position.
        assert_eq!(program_of("grep -e foo=bar file"), Some("grep"));
    }

    #[test]
    fn tilde_paths_extract_for_containment_to_refuse() {
        // The classifier extracts; containment refuses. `~` never widens.
        assert_eq!(paths("ls ~/x"), vec!["~/x".to_string()]);
        assert_eq!(paths("ls ~"), vec!["~".to_string()]);
        assert_eq!(
            paths("cat ~/.ssh/id_rsa"),
            vec!["~/.ssh/id_rsa".to_string()]
        );
        for cmd in ["ls a~b", "ls ~~", "ls ~/a~b"] {
            assert!(classify(cmd).is_none(), "{cmd:?} must not classify");
        }
    }

    #[test]
    fn simple_invocations_classify_with_paths() {
        assert_eq!(program_of("ls -la src/"), Some("ls"));
        assert_eq!(paths("ls -la src/"), vec!["src/".to_string()]);
        assert_eq!(paths("cat a b"), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            paths("ls -la src/"),
            vec!["src/".to_string()],
            "one operand, one path"
        );
        assert_eq!(paths("ls"), Vec::<String>::new());
        assert_eq!(paths("  ls   -l  "), Vec::<String>::new());
        assert_eq!(program_of("ls\t-l"), Some("ls"), "tab splits");
        assert_eq!(
            paths("ls /etc/hostname"),
            vec!["/etc/hostname".to_string()],
            "extraction is not judgment; containment decides later"
        );
        assert_eq!(
            paths("ls sub/../x"),
            vec!["sub/../x".to_string()],
            "kept raw for the resolver, not normalized here"
        );
    }

    #[test]
    fn short_flags_bundle_character_by_character() {
        assert_eq!(program_of("ls -la"), Some("ls"));
        assert_eq!(program_of("du -sh"), Some("du"));
        assert_eq!(program_of("rg -uuu pat f"), Some("rg"));
        assert_eq!(program_of("ls -lZ"), None, "unknown bundling member");
        assert_eq!(program_of("tail -L f"), None, "no such flag");
        assert_eq!(program_of("ls -5"), None, "digit flags are table-driven");
    }

    #[test]
    fn arity_one_flags_take_values_in_both_spellings() {
        assert_eq!(paths("head -n 5 file"), vec!["file".to_string()]);
        assert_eq!(paths("head -n5 file"), vec!["file".to_string()]);
        assert_eq!(paths("head --lines=5 file"), vec!["file".to_string()]);
        assert_eq!(paths("tail -c +5 file"), vec!["file".to_string()]);
        assert_eq!(
            paths("grep -f patterns.txt src"),
            vec!["patterns.txt".to_string(), "src".to_string()]
        );
        assert_eq!(paths("stat -t %Y file"), vec!["file".to_string()]);
        assert_eq!(paths("du -d 2 dir"), vec!["dir".to_string()]);
        assert_eq!(program_of("head -n"), None, "missing value");
        assert_eq!(program_of("du -d"), None, "missing value");
        assert_eq!(program_of("grep --lines"), None, "missing value");
        assert_eq!(program_of("head --lines="), None, "empty attached value");
    }

    #[test]
    fn attached_only_flags_reject_the_bare_spelling() {
        // Bare `--color` would swallow the next operand as its "value" and
        // skip path-checking it; only `--color=WHEN` is admitted.
        assert_eq!(program_of("ls --color"), None);
        assert_eq!(program_of("ls --color src"), None);
        assert_eq!(paths("ls --color=always src"), vec!["src".to_string()]);
        assert_eq!(program_of("date -I"), None);
        assert_eq!(program_of("date -I src"), None);
        assert_eq!(program_of("date -IFMT"), Some("date"));
    }

    #[test]
    fn long_flags_match_exactly_not_by_prefix() {
        assert_eq!(program_of("grep --col foo file"), None);
        assert_eq!(program_of("ls --group-directories-first src"), Some("ls"));
        assert_eq!(program_of("ls --group-directories- src"), None);
        assert_eq!(program_of("tail --quiet f"), Some("tail"));
    }

    #[test]
    fn double_dash_has_no_semantics() {
        assert_eq!(program_of("ls --"), None);
        assert_eq!(program_of("ls -- -l"), None);
        assert_eq!(program_of("cat -- file"), None);
    }

    #[test]
    fn execution_and_symlink_flags_are_out() {
        for cmd in [
            "rg --pre id",
            "rg --pre-glob x",
            "rg --hostname-bin h",
            "rg -z f",
            "rg --search-zip f",
            "rg -f pats",
            "rg -L dir",
            "rg --follow dir",
            "ls -L link",
            "ls -H link",
            "file -f list f",
            "file -m x.magic f",
            "file -z f",
            "file -C",
            "tail -f f",
            "tail -F f",
            "tail -r f",
            "grep -r pat dir",
            "grep -R pat dir",
            "df -T x",
            "stat -f %Y f",
            "ls --time-style=long-iso",
            "grep --exclude=*.rs pat dir",
        ] {
            assert!(classify(cmd).is_none(), "{cmd:?} must not classify");
        }
    }

    #[test]
    fn date_operands_are_formats_not_paths() {
        assert_eq!(paths("date -u +%Y"), Vec::<String>::new());
        assert_eq!(paths("date"), Vec::<String>::new());
        assert_eq!(program_of("date -j"), Some("date"));
        assert_eq!(program_of("date 09081200"), None, "could set the clock");
        assert_eq!(program_of("date -u 09081200"), None);
    }

    #[test]
    fn which_operands_are_names_not_paths() {
        assert_eq!(paths("which ls grep"), Vec::<String>::new());
        assert_eq!(program_of("which -a"), Some("which"));
    }

    #[test]
    fn pwd_takes_no_operands() {
        assert_eq!(program_of("pwd -L"), Some("pwd"));
        assert_eq!(program_of("pwd /tmp"), None);
    }

    #[test]
    fn grep_pattern_position_is_conservatively_a_path() {
        // Without -e/-f the first operand is the pattern; every operand is
        // still extracted, so a pattern shaped like an outside path prompts
        // rather than slipping through unjudged.
        assert_eq!(
            paths("grep pattern file"),
            vec!["pattern".to_string(), "file".to_string()]
        );
        assert_eq!(paths("grep -e pattern file"), vec!["file".to_string()]);
        assert_eq!(
            paths("grep -A 3 -B 2 pat f"),
            vec!["pat".to_string(), "f".to_string()]
        );
        assert_eq!(
            paths("rg -t rs pat dir"),
            vec!["pat".to_string(), "dir".to_string()]
        );
        assert_eq!(
            paths("basename -s .rs file.rs"),
            vec!["file.rs".to_string()]
        );
        assert_eq!(paths("dirname a/b"), vec!["a/b".to_string()]);
    }

    #[test]
    fn noun_comes_from_the_table_not_the_input() {
        let cmd = classify("ls").unwrap();
        assert_eq!(cmd.program.name, "ls");
        assert_eq!(cmd.program.noun, "`ls` commands");
    }

    #[test]
    fn classify_call_gates_on_execute_kind() {
        use serde_json::json;
        assert!(classify_call("run_command", &json!({ "CommandLine": "ls" })).is_some());
        assert!(classify_call("run_command", &json!({ "CommandLine": "ls; rm x" })).is_none());
        // Unknown tools carrying a CommandLine stay fingerprinted even when
        // the string itself would classify: only audited kinds widen.
        assert!(classify_call("mystery_tool", &json!({ "CommandLine": "ls" })).is_none());
        assert!(classify_call("view_file", &json!({ "AbsolutePath": "x" })).is_none());
        assert!(classify_call("run_command", &json!({ "Cwd": "/tmp" })).is_none());
    }
}
