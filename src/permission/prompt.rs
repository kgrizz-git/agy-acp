//! Presentation-only wording and options for ACP permission prompts.

use serde_json::{json, Value};

use super::safe_command::ProgramDef;

pub(super) const OPTION_ALLOW_ONCE: &str = "allow_once";
pub(super) const OPTION_ALLOW_ALWAYS: &str = "allow_always";
pub(super) const OPTION_REJECT_ONCE: &str = "reject_once";
pub(super) const OPTION_REJECT_ALWAYS: &str = "reject_always";

/// What the two "always" labels claim the answer covers.
///
/// Derived once, in `decide`, from the same `sticky_scope` result that builds
/// the key, and then handed to both the prompt and the outcome application so
/// the button, stored key, and reason string cannot disagree about scope.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AlwaysScope {
    /// Every later call to this tool, for this session.
    Tool,
    /// This exact command line and no other.
    Command,
    /// This exact call -- same tool, same arguments.
    Call,
    /// Every later invocation of one allowlisted program, for this session.
    Program(&'static ProgramDef),
}

impl AlwaysScope {
    /// `scope` is the `sticky_scope` result the key is built from; `args`
    /// decides only the wording, never the breadth.
    pub(super) fn of(scope: Option<&String>, args: &Value) -> Self {
        match scope {
            None => AlwaysScope::Tool,
            Some(_) if super::has_command_line(args) => AlwaysScope::Command,
            Some(_) => AlwaysScope::Call,
        }
    }

    pub(super) fn noun(self) -> Option<&'static str> {
        match self {
            AlwaysScope::Tool => None,
            AlwaysScope::Command => Some("this exact command"),
            AlwaysScope::Call => Some("this exact call"),
            AlwaysScope::Program(program) => Some(program.noun),
        }
    }
}

/// Builds the four answers offered with every prompt.
///
/// Allow and deny wordings arrive as separate scopes because denies stay
/// narrow while a classified command allow may widen to its program.
pub(super) fn permission_options(tool_name: &str, allow: AlwaysScope, deny: AlwaysScope) -> Value {
    let allow_always = match allow.noun() {
        Some(noun) => format!("Always allow {noun} this session"),
        None => format!("Always allow {tool_name} this session"),
    };
    let reject_always = match deny.noun() {
        Some(noun) => format!("Always reject {noun} this session"),
        None => format!("Always reject {tool_name} this session"),
    };
    json!([
        { "optionId": OPTION_ALLOW_ONCE, "name": "Allow once", "kind": "allow_once" },
        {
            "optionId": OPTION_ALLOW_ALWAYS,
            "name": allow_always,
            "kind": "allow_always",
        },
        { "optionId": OPTION_REJECT_ONCE, "name": "Reject", "kind": "reject_once" },
        {
            "optionId": OPTION_REJECT_ALWAYS,
            "name": reject_always,
            "kind": "reject_always",
        },
    ])
}

/// Builds the one-line summary the ACP client shows in the prompt, including
/// the longer explanation for a `schedule` call.
pub(super) fn tool_title(tool_name: &str, args: &Value) -> String {
    let field = |key: &str| args.get(key).and_then(|v| v.as_str());

    if let Some(command) = field("CommandLine") {
        return format!("Run `{command}`");
    }
    if let Some(target) = field("TargetFile") {
        return format!("{tool_name} {target}");
    }
    if let Some(path) = field("AbsolutePath").or_else(|| field("DirectoryPath")) {
        return format!("{tool_name} {path}");
    }
    if let Some(query) = field("Query").or_else(|| field("SearchTerm")) {
        return format!("{tool_name} {query}");
    }
    // `schedule` runs its work as later steps of this same turn under headless
    // agy, so approving it approves holding the turn open, not just one call.
    if tool_name == "schedule" {
        return "schedule (runs in this turn; may hold it open until the timer or iterations finish)".to_string();
    }
    tool_name.to_string()
}
