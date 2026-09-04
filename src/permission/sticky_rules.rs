//! How broad a remembered "Always" answer is allowed to be.
//!
//! Split out of `permission.rs` when that file reached the length cap. The
//! grouping is not arbitrary: everything here answers one question -- given a
//! tool call the user has just answered for, which *later* calls does that
//! answer cover? Getting it wrong in the wide direction is how one "Always
//! allow" silently covers a call the user never saw, so these pieces are read
//! together or not at all.

use serde_json::Value;

/// Model-authored display and pacing fields, observed to differ between two
/// otherwise identical calls to agy 1.1.22. They cannot change what a command is
/// or where it runs, so they are excluded from the sticky key; leaving them in
/// would make "Always allow" never match on a repeat, and an option that visibly
/// does nothing teaches people to stop reading the prompt.
///
/// A denylist rather than an allowlist, and that is the load-bearing choice. An
/// allowlist of "the fields that decide what runs" would be `CommandLine` and
/// `Cwd` today, and a field agy adds later would fall outside the key silently,
/// which is a hole. With a denylist a new field lands *inside* the key; if it is
/// volatile the symptom is a reprompt, which is visible and harmless.
/// Under-normalizing costs a prompt, over-normalizing is a hole.
///
/// Every addition here is a normalization step and needs the same argument made:
/// that the field cannot affect what the tool does. `WaitMsBeforeAsync` is the
/// borderline one — it is behavioural, not presentational, but it can only change
/// how long the adapter waits before backgrounding a command, not what runs or
/// where. It is the entry most worth revisiting if this list ever grows.
pub(super) const UNKEYED_FIELDS: &[&str] = &["toolAction", "toolSummary", "WaitMsBeforeAsync"];

/// Fingerprints tool arguments for the sticky key: the argument object minus the
/// volatile fields, serialized.
///
/// Top level only, deliberately, and not a recursive strip. Recursion would be
/// over-normalization — it would remove a nested `toolSummary` living inside some
/// future structured argument where the value does matter, merging two argument
/// sets that are not the same into one key. Leaving a nested volatile field in
/// costs a reprompt instead. Note the contrast with [`path_rules::path_field_args`], which
/// does recurse: that one is looking for a reason to *ask*, so a deeper search is
/// the conservative direction there and the opposite direction here.
///
/// The filtering and the serialization live together on purpose. They must never
/// be reachable separately, or the next reader will fingerprint the unfiltered
/// form and quietly restore the bug this key exists to close.
pub(super) fn args_fingerprint(args: &Value) -> String {
    match args {
        Value::Object(map) => {
            let kept: serde_json::Map<String, Value> = map
                .iter()
                .filter(|(key, _)| !UNKEYED_FIELDS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            Value::Object(kept).to_string()
        }
        other => other.to_string(),
    }
}

/// How specific a remembered answer for this call has to be.
///
/// `None` is tool-level keying, and it has to be *earned*. A tool qualifies only
/// when the checks that still run on a remembered allow can actually read its
/// arguments: `escapes_containment` and the sensitive-path list read arguments as
/// paths, so for a path-argument tool like `view_file` they do constrain a
/// remembered allow and the tool name is a defensible scope. That is what
/// `KEYED_BY_TOOL_KINDS` names.
///
/// Everything else — including every tool this fork has never heard of — gets
/// `Some(fingerprint)`. The default direction is the whole point. An unknown tool
/// that turns out to execute something would otherwise land on the weaker key and
/// silently restore the bug this exists to close; being wrong the other way costs
/// a reprompt. Under-normalizing costs a prompt, over-normalizing is a hole.
///
/// [`has_unconstrained_reach`] stays as a second line, and it is not redundant
/// with the kind list: kind is a *display* classification, and a tool can be
/// classified `"read"` while reaching somewhere the path checks cannot see.
/// `read_url_content` is exactly that — kind `"read"`, but its argument is a
/// `Url`, which is not a path field, so containment and the sensitive-path list
/// are as inert against it as they are against a command line. Keying it by tool
/// would let one "Always allow" on a trusted URL cover every later URL. The walk
/// is nested rather than a top-level `args.get`, since a nested or renamed field
/// would otherwise inherit the path tool's weaker key.
pub(super) fn sticky_scope(tool_name: &str, args: &Value) -> Option<String> {
    if !KEYED_BY_TOOL_KINDS.contains(&tool_kind(tool_name)) || has_unconstrained_reach(args) {
        return Some(args_fingerprint(args));
    }
    None
}

/// Tool kinds whose remembered answers may be keyed by tool name alone, because
/// containment and the sensitive-path list still constrain them.
///
/// Deliberately not `"execute"`, `"fetch"`, or `"other"`. `"other"` is the
/// important one: it is what [`tool_kind`] returns for a name this fork does not
/// know, and an unknown tool must get the stronger key, not the weaker one.
pub(super) const KEYED_BY_TOOL_KINDS: &[&str] = &["read", "edit", "search"];

/// Whether a `CommandLine` field appears anywhere in the arguments.
///
/// Wording only: this decides whether the prompt says "command" or "call", never
/// how broad the remembered answer is. [`has_unconstrained_reach`] decides that.
pub(super) fn has_command_line(args: &Value) -> bool {
    match args {
        Value::Array(items) => items.iter().any(has_command_line),
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| key == "CommandLine" || has_command_line(value)),
        _ => false,
    }
}

/// Whether the arguments reach somewhere the path checks cannot follow.
///
/// Two shapes, both of which make `escapes_containment` and the sensitive-path
/// list inert: a `CommandLine`, which is one opaque string the shell will
/// reinterpret, and a `Url`, which names a resource that is not on the filesystem
/// at all. A bare `://` anywhere in a string value counts too, so a tool that
/// takes a URL under some other field name is still caught. That last test also
/// fires on a search query that happens to contain `://`, which costs a reprompt
/// and is the direction to be wrong in.
pub(super) fn has_unconstrained_reach(args: &Value) -> bool {
    match args {
        Value::String(s) => s.contains("://"),
        Value::Array(items) => items.iter().any(has_unconstrained_reach),
        Value::Object(map) => map.iter().any(|(key, value)| {
            key == "CommandLine" || key == "Url" || has_unconstrained_reach(value)
        }),
        _ => false,
    }
}

/// Maps an agy tool name onto the closest ACP tool kind.
///
/// Every arm names a tool observed in a captured payload -- see
/// dev-docs/agy-tool-surface.md -- and that is a requirement, not a
/// coincidence. Unlike the display-only classifier in `tools.rs`, a `"read"`,
/// `"edit"` or `"search"` here earns the weaker sticky key through
/// [`KEYED_BY_TOOL_KINDS`], so naming a tool nobody has seen would hand that key
/// to something whose arguments were never checked against the path rules.
///
/// The fallthrough is therefore the safe answer, and agy's self-reported tools
/// are left to reach it. `schedule` and `invoke_subagent` are why that is worth
/// stating rather than leaving to omission, and both were checked by capture
/// (agy 1.1.25/1.1.26; see dev-docs/agy-tool-surface.md) rather than assumed.
/// `invoke_subagent` spawns an agent whose own tool calls do reach this same
/// hook under their own conversationId, so keying it by name would let one
/// "Always allow" cover a later call that grants different capabilities.
/// `schedule` runs its work in-turn under headless `agy -p`, but a remembered
/// answer keyed only by name would still cover a later schedule of a different
/// duration or command, which is not what the user approved. Both stay "other".
pub(super) fn tool_kind(tool_name: &str) -> &'static str {
    match tool_name {
        "view_file" | "list_dir" | "read_url_content" => "read",
        "write_to_file" | "replace_file_content" => "edit",
        "grep_search" | "find_by_name" => "search",
        "run_command" => "execute",
        "search_web" => "fetch",
        _ => "other",
    }
}
