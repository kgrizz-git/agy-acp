use serde_json::Value;
use std::path::{Path, PathBuf};

/// Returns the first argument path in `args` that falls outside every root.
///
/// Two classes of argument are not absolute but still must be contained here, not
/// in `absolute_paths`: a `~`-prefixed string is always outside the workspace
/// (home-relative), and a string carrying a `..` component escapes unless it
/// resolves lexically inside a root. Plain strings like a search query are left
/// alone — only `/`-, `~`-prefixed, and `..`-bearing arguments are treated as paths.
///
/// Paths are compared after resolving symlinks where possible, since macOS reports
/// `/tmp/x` to agy but `/private/tmp/x` to the permission layer.
pub(super) fn outside_workspace(args: &Value, roots: &[PathBuf]) -> Option<String> {
    let absolute = absolute_paths(args);

    if roots.is_empty() {
        // Without a known workspace nothing can be judged inside it, and that has
        // to cover all three shapes: `~/.ssh/id_rsa` is no more contained for the
        // roots being unset than it is with them set.
        return absolute
            .into_iter()
            .next()
            .or_else(|| path_field_args(args).into_iter().next())
            .or_else(|| {
                string_args(args)
                    .into_iter()
                    .find(|s| s.starts_with('~') || has_parent_component(s))
            });
    }

    // Absolute paths that escape are the common case.
    if let Some(escaped) = absolute
        .iter()
        .find(|path| !roots.iter().any(|root| is_inside(path, root)))
    {
        return Some(escaped.clone());
    }

    // A field that names a path names one whatever its value looks like, so a
    // plain relative value is judged too: `link/secret.txt` carries no `/`, `~`
    // or `..` and can still leave the workspace through a symlink.
    if let Some(escaped) = path_field_args(args)
        .into_iter()
        .find(|path| !roots.iter().any(|root| is_inside_from(path, root)))
    {
        return Some(escaped);
    }

    // `~` is home-relative and therefore never inside the workspace.
    let home_relative = string_args(args).into_iter().find(|s| s.starts_with('~'));
    if home_relative.is_some() {
        return home_relative;
    }

    // A `..` component can escape, and does so through symlinks as well as
    // textually, so this goes through the same resolving check as an absolute
    // path rather than trusting normalization.
    string_args(args)
        .into_iter()
        .filter(|s| has_parent_component(s))
        .find(|s| !roots.iter().any(|root| is_inside_from(s, root)))
}

/// True if `path` has a `..` component, rather than merely the two characters
/// somewhere in it: `sub/../x` is a traversal, `foo..bar` is an ordinary name.
fn has_parent_component(path: &str) -> bool {
    path.split('/').any(|component| component == "..")
}

/// True if `path` is inside `root`, taking a relative path as relative to it.
///
/// The adapter runs with the workspace as its working directory, so that is what
/// a relative argument is relative to: `sub/../file.txt` stays inside, `../secret`
/// does not.
fn is_inside_from(path: &str, root: &Path) -> bool {
    if path.starts_with('/') {
        return is_inside(path, root);
    }
    let root_norm = lexical_normalize(&root.display().to_string());
    is_inside(&format!("{root_norm}/{path}"), root)
}

/// Resolves `.` and `..` components textually without touching the filesystem.
fn lexical_normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if matches!(parts.last(), Some(&"..")) || parts.is_empty() {
                    parts.push("..");
                } else {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return "/".to_string();
    }
    let joined = parts.join("/");
    if path.starts_with('/') {
        format!("/{joined}")
    } else {
        joined
    }
}

fn is_inside(path: &str, root: &Path) -> bool {
    // One candidate, and the resolved one wherever it exists. Accepting the path
    // as written too would call `<root>/link/../secret` contained on the strength
    // of its first component, even where `link` points out of the workspace and
    // the kernel follows it there. Where nothing can be resolved -- a file not
    // created yet -- normalizing at least cancels the `..` that `starts_with`
    // would otherwise ignore.
    let candidate =
        resolve(Path::new(path)).unwrap_or_else(|| PathBuf::from(lexical_normalize(path)));
    let roots = [Some(root.to_path_buf()), resolve(root)];
    roots
        .iter()
        .flatten()
        .any(|root| candidate == *root || candidate.starts_with(root))
}

/// Resolves a path, falling back to resolving the nearest existing ancestor so
/// that not-yet-created files can still be placed.
fn resolve(path: &Path) -> Option<PathBuf> {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return Some(resolved);
    }
    let parent = path.parent()?;
    let name = path.file_name()?;
    std::fs::canonicalize(parent).ok().map(|p| p.join(name))
}

/// Argument fields whose value is a path whatever it looks like.
///
/// agy's tool arguments are a fixed schema and `tool_title` already leans on it.
/// Judging these by name is what lets a plain relative path be checked without
/// having to guess that an arbitrary string is a path -- a `Query` of
/// `src/main.rs` must not start prompting.
///
/// A field missing from this list keeps the shape-based checks and nothing else,
/// which is what every field had before it existed: an omission costs coverage,
/// never a false prompt. It has to track agy.
pub(super) const PATH_FIELDS: &[&str] = &[
    "AbsolutePath",
    "TargetFile",
    "FilePath",
    "DirectoryPath",
    "SearchPath",
    "SearchDirectory",
    "Cwd",
    "Paths",
    "ImagePaths",
    "Workspace",
];

/// Collects every string sitting under a `PATH_FIELDS` key, at any depth.
pub(super) fn path_field_args(args: &Value) -> Vec<String> {
    fn walk(value: &Value, under_path_field: bool, found: &mut Vec<String>) {
        match value {
            Value::String(s) if under_path_field => found.push(s.clone()),
            Value::Array(items) => items.iter().for_each(|v| walk(v, under_path_field, found)),
            Value::Object(map) => map.iter().for_each(|(key, v)| {
                walk(
                    v,
                    under_path_field || PATH_FIELDS.contains(&key.as_str()),
                    found,
                )
            }),
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(args, false, &mut found);
    found
}

/// Collects every absolute-looking path appearing in the tool arguments.
fn absolute_paths(args: &Value) -> Vec<String> {
    string_args(args)
        .into_iter()
        .filter(|s| s.starts_with('/'))
        .collect()
}

/// Collects every string value anywhere in the tool arguments.
pub(super) fn string_args(args: &Value) -> Vec<String> {
    fn walk(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::String(s) => found.push(s.clone()),
            Value::Array(items) => items.iter().for_each(|v| walk(v, found)),
            Value::Object(map) => map.values().for_each(|v| walk(v, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(args, &mut found);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `outside_workspace` is pure, so these go straight at it: they cover the
    /// shapes that never reach a bridge test because they are decided before the
    /// prompt.
    #[test]
    fn without_a_workspace_root_nothing_is_contained() {
        let none: &[PathBuf] = &[];
        let outside = |args| outside_workspace(&args, none);

        assert_eq!(
            outside(json!({ "AbsolutePath": "/etc/passwd" })).as_deref(),
            Some("/etc/passwd")
        );
        // These two used to slip through: the empty-roots branch looked only at
        // arguments starting with `/`, so an unset workspace made them contained.
        assert_eq!(
            outside(json!({ "AbsolutePath": "~/.ssh/id_rsa" })).as_deref(),
            Some("~/.ssh/id_rsa")
        );
        assert_eq!(
            outside(json!({ "AbsolutePath": "../../secret" })).as_deref(),
            Some("../../secret")
        );
        assert_eq!(
            outside(json!({ "Query": "foo..bar" })),
            None,
            "a query is not a path, with or without a root"
        );
    }

    /// The new path fields must be judged by `outside_workspace`, not just
    /// collected. This pins the case only the path-field branch catches: a plain
    /// relative value under `ImagePaths` — no leading `/`, no `~`, no `..`, so
    /// `absolute_paths` and the `..` check both miss it — that leaves the
    /// workspace through a symlink. If `ImagePaths` were dropped from
    /// `PATH_FIELDS`, this would wrongly read as contained.
    #[test]
    fn a_new_path_field_catches_a_relative_symlink_escape() {
        let base = std::env::temp_dir().join(format!("agy-acp-imgpath-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let workspace = base.join("work");
        let outside = base.join("outside");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.png"), "s").unwrap();
        std::os::unix::fs::symlink(&outside, workspace.join("link")).unwrap();
        let roots = vec![workspace.clone()];

        assert!(
            outside_workspace(&json!({ "ImagePaths": ["link/secret.png"] }), &roots).is_some(),
            "a relative ImagePaths entry escaping through a symlink must be caught"
        );
        // Control: the same value under a non-path key is invisible to the shape
        // checks, which is exactly why the field has to be in PATH_FIELDS.
        assert_eq!(
            outside_workspace(&json!({ "NotAPath": "link/secret.png" }), &roots),
            None,
            "the same string under a non-path key is not judged"
        );
    }

    /// The two path fields added on evidence (agy 1.1.26): `ImagePaths` holds an
    /// array of paths, and `Workspace` sits nested inside `invoke_subagent`'s
    /// `Subagents[]`. Both must be collected so containment sees them.
    #[test]
    fn image_paths_and_nested_subagent_workspace_are_path_fields() {
        let found = path_field_args(&json!({
            "ImagePaths": ["/tmp/a.png", "/tmp/b.png"],
            "Subagents": [{ "TypeName": "general", "Workspace": "/tmp/outside" }],
        }));
        assert!(found.contains(&"/tmp/a.png".to_string()));
        assert!(found.contains(&"/tmp/b.png".to_string()));
        assert!(
            found.contains(&"/tmp/outside".to_string()),
            "a subagent Workspace nested in Subagents[] must be seen as a path"
        );
    }

    /// A symlink is the case lexical normalization cannot see: `link/..` cancels
    /// on paper, but the kernel resolves `link` first and `..` then leaves from
    /// wherever it landed.
    #[test]
    fn a_symlink_out_of_the_workspace_is_not_contained() {
        let base = std::env::temp_dir().join(format!("agy-acp-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let workspace = base.join("work");
        let outside = base.join("outside");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "s").unwrap();
        std::fs::write(workspace.join("file.txt"), "f").unwrap();
        std::os::unix::fs::symlink(&outside, workspace.join("link")).unwrap();

        let roots = vec![workspace.clone()];
        let escaping = format!("{}/link/../outside/secret.txt", workspace.display());

        assert_eq!(
            outside_workspace(&json!({ "AbsolutePath": escaping }), &roots).as_deref(),
            Some(escaping.as_str()),
            "an absolute path that leaves through a symlink is outside"
        );
        assert!(
            outside_workspace(
                &json!({ "AbsolutePath": "link/../outside/secret.txt" }),
                &roots
            )
            .is_some(),
            "and so is the same path written relative to the workspace"
        );
        assert_eq!(
            outside_workspace(
                &json!({ "AbsolutePath": format!("{}/sub/../file.txt", workspace.display()) }),
                &roots
            ),
            None,
            "a `..` over a directory that does not exist still resolves inside"
        );

        // The shape-based tests see nothing wrong with this one: no leading `/`,
        // no `~`, no `..`. It is judged because `AbsolutePath` is a path field.
        assert_eq!(
            outside_workspace(&json!({ "AbsolutePath": "link/secret.txt" }), &roots).as_deref(),
            Some("link/secret.txt"),
            "a plain relative path field still leaves through the symlink"
        );
        // `find_by_name` names its directory `SearchDirectory`, seen in real agy
        // traffic. A relative value has no leading `/`, no `~` and no `..`, so the
        // shape tests pass it; only the field name catches it.
        assert_eq!(
            outside_workspace(&json!({ "SearchDirectory": "link" }), &roots).as_deref(),
            Some("link"),
            "a relative SearchDirectory that leaves through a symlink is outside"
        );
        // `FilePath` has not been seen in agy traffic, but `tools.rs` and
        // `protobuf.rs` both already treat it as naming a location, and the two
        // lists disagreeing is the kind of gap this test exists to catch.
        assert_eq!(
            outside_workspace(&json!({ "FilePath": "link" }), &roots).as_deref(),
            Some("link"),
            "a relative FilePath that leaves through a symlink is outside"
        );
        assert_eq!(
            outside_workspace(&json!({ "Query": "link/secret.txt" }), &roots),
            None,
            "the same string in a query field is a search term, not a path"
        );
        assert_eq!(
            outside_workspace(&json!({ "TargetFile": "notes.txt" }), &roots),
            None,
            "a relative path field inside the workspace stays silent"
        );
        assert_eq!(
            outside_workspace(
                &json!({ "Paths": ["notes.txt", "link/secret.txt"] }),
                &roots
            )
            .as_deref(),
            Some("link/secret.txt"),
            "path fields are judged through arrays too"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_double_dot_inside_a_name_is_not_a_traversal() {
        let roots = vec![PathBuf::from("/work")];
        assert_eq!(
            outside_workspace(&json!({ "Query": "foo..bar" }), &roots),
            None,
            "`..` must be a path component to count, not two characters"
        );
        assert_eq!(
            outside_workspace(&json!({ "AbsolutePath": "../secret" }), &roots).as_deref(),
            Some("../secret")
        );
    }

    #[test]
    fn absolute_paths_are_collected_from_anywhere_in_the_arguments() {
        let args = json!({
            "TargetFile": "/a/b.txt",
            "Nested": { "Other": "/c/d.txt" },
            "List": ["/e/f.txt", "relative.txt"],
            "Count": 3,
        });
        let mut found = absolute_paths(&args);
        found.sort();
        assert_eq!(found, vec!["/a/b.txt", "/c/d.txt", "/e/f.txt"]);
    }
}
