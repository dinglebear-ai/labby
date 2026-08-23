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
    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["results"][0]["tags"], serde_json::json!([]));
}

#[test]
fn blank_query_browses_the_first_visible_tools() {
    let entries = vec![
        tool("alpha", "ping", "ping"),
        tool("beta", "status", "status"),
    ];

    let response = search_visible_tools(&entries, &ToolScope::default(), "", 50).unwrap();

    assert_eq!(response.total, 2);
    assert_eq!(response.results.len(), 2);
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

#[test]
fn query_and_target_enforce_utf8_byte_boundaries() {
    let entries = vec![tool("github", "search", "search")];
    assert!(
        search_visible_tools(
            &entries,
            &ToolScope::default(),
            &"x".repeat(QUERY_MAX_BYTES),
            1
        )
        .is_ok()
    );
    let query_error = search_visible_tools(
        &entries,
        &ToolScope::default(),
        &"x".repeat(QUERY_MAX_BYTES + 1),
        1,
    )
    .unwrap_err();
    assert_eq!(query_error.kind(), "invalid_param");

    let target_error = describe_visible_tool(
        &entries,
        &ToolScope::default(),
        &"x".repeat(TARGET_MAX_BYTES + 1),
    )
    .unwrap_err();
    assert_eq!(target_error.kind(), "invalid_param");
}

#[test]
fn public_fields_are_truncated_without_splitting_utf8() {
    let mut entry = tool("github", "search", &"é".repeat(DESCRIPTION_MAX_BYTES));
    entry.signature = "λ".repeat(SIGNATURE_MAX_BYTES);
    entry.tags = (0..TAGS_MAX + 5)
        .map(|index| format!("tag-{index}-{}", "界".repeat(TAG_MAX_BYTES)))
        .collect();
    let response =
        describe_visible_tool(&[entry], &ToolScope::default(), "github::search").unwrap();
    assert!(response.description.len() <= DESCRIPTION_MAX_BYTES);
    assert!(response.signature.len() <= SIGNATURE_MAX_BYTES);
    assert_eq!(response.tags.len(), TAGS_MAX);
    assert!(response.tags.iter().all(|tag| tag.len() <= TAG_MAX_BYTES));
}

#[test]
fn lexical_ranking_is_weighted_coverage_aware_and_deterministic() {
    let entries = vec![
        tool("other", "issues", "search unrelated data"),
        tool("github", "search_issues", "find repository issues"),
        tool("github", "issues_search", "find repository issues"),
        tool("github", "partial", "issues only"),
    ];
    let response =
        search_visible_tools(&entries, &ToolScope::default(), "github issues search", 50).unwrap();
    let paths = response
        .results
        .iter()
        .map(|hit| hit.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![
            "github.issues_search",
            "github.search_issues",
            "github.partial",
            "other.issues",
        ]
    );
}
