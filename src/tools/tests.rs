//! Tests for tool classification.


use crate::tools::tool_kind;

#[test]
fn test_tool_kind_mapping() {
    assert_eq!(tool_kind("run_command"), "execute");
    assert_eq!(tool_kind("view_file"), "read");
    assert_eq!(tool_kind("write_to_file"), "edit");
    assert_eq!(tool_kind("grep_search"), "search");
    assert_eq!(tool_kind("thinking"), "think");
}
