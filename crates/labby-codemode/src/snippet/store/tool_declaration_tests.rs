use super::*;
use serde_json::json;

fn body(declaration: &str) -> String {
    format!(
        "---\nname: declared\ndescription: Declared tools\n{declaration}---\n\n```js\nasync () => ({{ ok: true }})\n```\n"
    )
}

#[test]
fn declarations_preserve_omission_empty_and_exact_ids_through_store_surfaces() {
    for (declaration, expected) in [
        ("", None),
        ("tools: []\n", Some(json!([]))),
        ("tools : []\n", Some(json!([]))),
        ("tools : [\"alpha::ping\"]\n", Some(json!(["alpha::ping"]))),
        (
            "tools:\n  - alpha::ping\n  - \"beta::read\"\n",
            Some(json!(["alpha::ping", "beta::read"])),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let builtins = tempfile::tempdir().unwrap();
        let info =
            create_user_snippet(root.path(), "declared", &body(declaration), None, false).unwrap();
        let resolved = resolve_snippet(root.path(), builtins.path(), "declared").unwrap();
        let listed = list_snippets(root.path(), builtins.path()).unwrap();
        for value in [
            serde_json::to_value(crate::types::CodeModeDiscoveryEntry::from_catalog(
                &crate::ToolDescriptor::snippet(&info),
            ))
            .unwrap(),
            serde_json::to_value(crate::ToolDescriptor::snippet(&info)).unwrap(),
            serde_json::to_value(info).unwrap(),
            serde_json::to_value(resolved).unwrap(),
            serde_json::to_value(&listed[0]).unwrap(),
        ] {
            assert_eq!(value.get("tools"), expected.as_ref(), "{declaration}");
        }
    }
}

#[test]
fn malformed_declarations_fail_validation_before_publication() {
    for declaration in [
        "tools: unrestricted\n",
        "tools: null\n",
        "tools:\n  - bare_name\n",
        "tools:\n  - a::b::c\n",
        "tools:\n  - \"\"\n",
        "tools:\n  - alpha::ping\n  - alpha::ping\n",
        "tools: []\ntools:\n  - alpha::ping\n",
        "tools: []\ntools : [\"alpha::ping\"]\n",
        "tools : []\ntools: [\"alpha::ping\"]\n",
    ] {
        let root = tempfile::tempdir().unwrap();
        assert!(
            create_user_snippet(root.path(), "declared", &body(declaration), None, false).is_err(),
            "invalid declaration accepted: {declaration}"
        );
        assert!(!user_snippet_dir(root.path()).join("declared.md").exists());
    }
}

#[test]
fn legacy_javascript_and_promoted_snippets_do_not_acquire_a_deny_all_policy() {
    let root = tempfile::tempdir().unwrap();
    let builtins = tempfile::tempdir().unwrap();
    let info = create_promoted_user_snippet(
        root.path(),
        builtins.path(),
        "legacy",
        "async () => ({ ok: true })",
        None,
        false,
        false,
    )
    .unwrap();
    assert!(serde_json::to_value(info).unwrap().get("tools").is_none());
    fs::write(
        user_snippet_dir(root.path()).join("plain.js"),
        "async () => ({ ok: true })",
    )
    .unwrap();
    let resolved = resolve_snippet(root.path(), builtins.path(), "plain").unwrap();
    assert!(
        serde_json::to_value(resolved)
            .unwrap()
            .get("tools")
            .is_none()
    );
}
