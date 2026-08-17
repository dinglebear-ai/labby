use super::*;

fn tool(namespace: &str, name: &str, description: &str) -> ToolDescriptor {
    ToolDescriptor::tool(namespace, name, description, None, None)
}

#[test]
fn bounded_search_excludes_types_and_snippets() {
    let mut entries = (0..100)
        .map(|i| tool("github", &format!("tool_{i:03}"), "tool search"))
        .collect::<Vec<_>>();
    let mut snippet = tool("snippet", "hidden", "tool snippet");
    snippet.kind = CodeModeCatalogKind::Snippet;
    entries.push(snippet);
    let response = search_visible_tools(&entries, &ToolScope::default(), "tool", 50).unwrap();
    assert_eq!(response.results.len(), 50);
    assert_eq!(response.total, 100);
    assert!(response.truncated);
    let json = serde_json::to_string(&response).unwrap();
    assert!(!json.contains("typescript"));
    assert!(!json.contains("snippet"));
    assert!(json.len() <= SEARCH_RESPONSE_MAX_BYTES);
}

#[test]
fn hidden_and_random_describe_are_identical() {
    let entries = vec![
        tool("github", "issues", "issues"),
        tool("admin", "rotate_key", "rotate"),
    ];
    let scope = ToolScope::scoped_namespaces(vec!["github".into()], Vec::new());
    let hidden = describe_visible_tool(&entries, &scope, "admin::rotate_key").unwrap_err();
    let random = describe_visible_tool(&entries, &scope, "missing::tool").unwrap_err();
    assert_eq!(
        serde_json::to_value(hidden).unwrap(),
        serde_json::to_value(random).unwrap()
    );
}

#[test]
fn oversized_typescript_is_omitted_whole() {
    let mut entry = tool("github", "search", "search");
    entry.dts = "x".repeat(DTS_MAX_BYTES + 1);
    let response =
        describe_visible_tool(&[entry], &ToolScope::default(), "github::search").unwrap();
    assert_eq!(response.typescript, None);
    assert_eq!(response.typescript_omitted, Some("size_limit"));
}

#[test]
fn api_surface_is_not_trusted_local() {
    let caller = CodeModeCaller::Scoped {
        capabilities: CodeModeCallerCapabilities::default(),
        sub: None,
    };
    assert_eq!(CodeModeSurface::Api.tag(), "api");
    assert!(!destructive_permitted(CodeModeSurface::Api, &caller));
}
