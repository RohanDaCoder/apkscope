use super::*;

/// Every listed tool has a match arm in `call` / `live_call`. Calling them would spawn the
/// CLI (this test binary, under test), so the dispatcher's source is checked instead.
#[test]
fn every_listed_tool_is_handled() {
    let src = include_str!("../../src/mcp.rs");
    for t in tools().as_array().unwrap() {
        let name = t["name"].as_str().unwrap();
        let arm = [
            format!("\"{name}\" =>"),
            format!("\"{name}\" |"),
            format!("| \"{name}\""),
            format!("\"{name}\" if"),
            format!("name == \"{name}\""),
        ];
        assert!(
            arm.iter().any(|a| src.contains(a.as_str())),
            "{name} is listed but has no handler"
        );
    }
}
