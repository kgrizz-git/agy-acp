//! Presentation-only wording for ACP permission prompts.

use serde_json::Value;

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
