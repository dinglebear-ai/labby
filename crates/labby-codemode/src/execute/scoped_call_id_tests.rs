use super::scoped_call_id;
use crate::ToolScope;

fn scope(namespaces: &[&str], tools: &[&str]) -> ToolScope {
    ToolScope::new(
        namespaces.iter().map(|name| (*name).to_string()).collect(),
        tools.iter().map(|name| (*name).to_string()).collect(),
    )
}

#[test]
fn in_scope_id_is_unchanged() {
    let id = scoped_call_id(
        &scope(&["claude-macpoo"], &[]),
        "claude-macpoo::Bash",
        "claude-macpoo",
        "Bash",
    )
    .expect("in scope");
    assert_eq!(id, "claude-macpoo::Bash");
}

#[test]
fn separator_alias_resolves_to_scoped_namespace() {
    let id = scoped_call_id(
        &scope(&["claude-macpoo"], &[]),
        "claude_macpoo::Bash",
        "claude_macpoo",
        "Bash",
    )
    .expect("alias resolves");
    assert_eq!(id, "claude-macpoo::Bash");
}

#[test]
fn out_of_scope_namespace_suggests_similar_scoped_upstream() {
    let err = scoped_call_id(
        &scope(&["claude-macpoo"], &[]),
        "claude-macpo::Bash",
        "claude-macpo",
        "Bash",
    )
    .expect_err("typo is outside scope");
    let message = err.to_string();
    assert!(
        message.contains("Did you mean `claude-macpoo`"),
        "{message}"
    );
    assert!(message.contains("codemode.search()"), "{message}");
}

#[test]
fn alias_resolves_against_namespaces_named_in_the_tools_allowlist() {
    let id = scoped_call_id(
        &scope(&[], &["claude-macpoo::Bash"]),
        "claude_macpoo::Bash",
        "claude_macpoo",
        "Bash",
    )
    .expect("alias of an allowlisted id");
    assert_eq!(id, "claude-macpoo::Bash");
}

#[test]
fn ambiguous_scoped_alias_is_rejected() {
    let err = scoped_call_id(&scope(&["a-b", "a_b"], &[]), "A-B::x", "A-B", "x")
        .expect_err("two scoped namespaces share the alias key");
    assert_eq!(err.kind(), "unknown_tool");
}

#[test]
fn tool_outside_tools_allowlist_says_so() {
    let err = scoped_call_id(
        &scope(&["claude-macpoo"], &["claude-macpoo::Read"]),
        "claude_macpoo::Bash",
        "claude_macpoo",
        "Bash",
    )
    .expect_err("tool not allowlisted");
    assert!(err.to_string().contains("`tools` allowlist"), "{err}");
    assert!(
        err.to_string().contains("Allowed: `claude-macpoo::Read`"),
        "{err}"
    );
}
