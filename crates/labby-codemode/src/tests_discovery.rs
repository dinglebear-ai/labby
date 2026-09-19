use super::*;

fn tool(namespace: &str, name: &str, description: &str) -> CatalogDescriptor {
    CatalogDescriptor::tool(namespace, name, description, None, None)
}

fn capability(kind: CodeModeCatalogKind, namespace: &str, name: &str) -> CatalogDescriptor {
    let mut entry = tool(namespace, name, "catalog capability");
    entry.kind = kind;
    entry.id = format!("{}::{namespace}::{name}", kind.as_str());
    entry
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

#[test]
fn catalog_kind_is_source_neutral_and_extensible() {
    let kinds = [
        CodeModeCatalogKind::Tool,
        CodeModeCatalogKind::Snippet,
        CodeModeCatalogKind::Resource,
        CodeModeCatalogKind::Prompt,
        CodeModeCatalogKind::Skill,
        CodeModeCatalogKind::Agent,
    ];
    assert_eq!(
        kinds.map(CodeModeCatalogKind::as_str),
        ["tool", "snippet", "resource", "prompt", "skill", "agent"]
    );
}

#[test]
fn generic_search_and_describe_cover_future_catalog_kinds() {
    let entries = vec![
        tool("github", "issues", "catalog capability"),
        capability(CodeModeCatalogKind::Resource, "github", "readme"),
        capability(CodeModeCatalogKind::Prompt, "github", "review"),
        capability(CodeModeCatalogKind::Skill, "labby", "adversarial_review"),
        capability(CodeModeCatalogKind::Agent, "labby", "reviewer"),
    ];

    let response =
        search_visible_catalog(&entries, &ToolScope::default(), "catalog capability", 50).unwrap();
    assert_eq!(response.total, 5);
    assert_eq!(response.results.len(), 5);

    let skill = describe_visible_catalog(
        &entries,
        &ToolScope::default(),
        "skill.labby.adversarial_review",
    )
    .unwrap();
    assert_eq!(skill.id, "skill::labby::adversarial_review");
    assert_eq!(skill.kind, CodeModeCatalogKind::Skill);
    assert_eq!(skill.typescript, None);
    assert_eq!(skill.typescript_omitted, None);
}

#[test]
fn progressive_disclosure_helpers_preserve_retrieval_identity() {
    let resource = CatalogDescriptor::metadata(
        CodeModeCatalogKind::Resource,
        "github",
        "resource::github::file:///README.md",
        "README",
        "Repository README",
        vec!["uri:lab://upstream/github/file:///README.md".to_string()],
    );
    let prompt = CatalogDescriptor::metadata(
        CodeModeCatalogKind::Prompt,
        "github",
        "prompt::github::review",
        "review",
        "Review changes",
        Vec::new(),
    );
    let skill = CatalogDescriptor::metadata(
        CodeModeCatalogKind::Skill,
        "labby",
        "skill::skill://labby/adversarial-review",
        "adversarial-review",
        "Review code adversarially",
        vec!["uri:skill://labby/adversarial-review".to_string()],
    );

    assert_eq!(
        resource.discovery_helper(),
        r#"codemode.readResource("lab://upstream/github/file:///README.md")"#
    );
    assert_eq!(
        prompt.discovery_helper(),
        r#"codemode.getPrompt("prompt::github::review", args)"#
    );
    assert_eq!(
        skill.discovery_helper(),
        r#"codemode.getSkill("skill://labby/adversarial-review")"#
    );
}

#[test]
fn generic_search_can_filter_kinds_without_granting_capabilities() {
    let entries = vec![
        tool("github", "issues", "catalog capability"),
        capability(CodeModeCatalogKind::Skill, "labby", "reviewer"),
        capability(CodeModeCatalogKind::Agent, "labby", "reviewer"),
    ];
    let response = search_visible_catalog_with_kinds(
        &entries,
        &ToolScope::default(),
        "catalog capability",
        50,
        &[CodeModeCatalogKind::Skill],
    )
    .unwrap();
    assert_eq!(response.total, 1);
    assert_eq!(response.results[0].kind, CodeModeCatalogKind::Skill);
}

#[test]
fn legacy_tool_describe_preserves_ambiguous_bare_name_behavior() {
    let entries = vec![
        tool("github", "issues", "GitHub issues"),
        tool("linear", "issues", "Linear issues"),
    ];

    let error = describe_visible_tool(&entries, &ToolScope::default(), "issues")
        .expect_err("bare duplicate tool name must remain ambiguous");
    assert_eq!(error.kind(), "ambiguous_tool");
    let valid = match error {
        error::ToolError::AmbiguousTool { valid, .. } => valid,
        _ => Vec::new(),
    };
    assert_eq!(valid, vec!["github.issues", "linear.issues"]);
}
