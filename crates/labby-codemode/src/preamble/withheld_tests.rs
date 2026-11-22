use super::generate_discovery_js;
use crate::types::{CatalogDescriptor, CodeModeDiscoveryEntry};

fn run_withheld_discovery_script(body: &str) -> serde_json::Value {
    run_withheld_discovery_script_with("annotated", body)
}

/// `visible_namespace` hosts the one visible tool; `claude-macpoo` and
/// `annotated` are both reported as withheld.
fn run_withheld_discovery_script_with(visible_namespace: &str, body: &str) -> serde_json::Value {
    let tool = CodeModeDiscoveryEntry::from_catalog(&CatalogDescriptor::tool(
        visible_namespace,
        "lookup",
        "Look up a record",
        None,
        None,
    ));
    let withheld = [
        crate::host::WithheldTools::new("claude-macpoo", 25).expect("nonzero"),
        crate::host::WithheldTools::new("annotated", 1).expect("nonzero"),
    ];
    let preamble = generate_discovery_js(&[tool], 0.5, &withheld).unwrap();
    let script = format!(
        "{preamble}\n\
         globalThis.callTool = async () => ({{ranked: []}});\n\
         globalThis.result = null;\n\
         (async () => {{ {body} }})().catch(error => {{ globalThis.result = JSON.stringify({{error: String(error)}}); }});"
    );
    let mut config = javy::Config::default();
    config.memory_limit(8 * 1024 * 1024);
    let runtime = javy::Runtime::new(config).unwrap();
    runtime
        .context()
        .with(|cx| cx.eval::<(), _>(script))
        .unwrap();
    runtime.resolve_pending_jobs().unwrap();
    let result: String = runtime
        .context()
        .with(|cx| cx.globals().get("result"))
        .unwrap();
    serde_json::from_str(&result).unwrap()
}

#[test]
fn javy_empty_read_only_search_keeps_no_match_hint_first() {
    let value = run_withheld_discovery_script(
        "globalThis.result = JSON.stringify(await codemode.search({query: 'Bash'}));",
    );

    assert_eq!(value["total"], 0, "{value}");
    let hint = value["hint"].as_str().unwrap();
    assert!(hint.starts_with("No matches."), "{hint}");
    assert!(hint.contains("Use the `codemode` tool"), "{hint}");
    // A zero-result query is most likely the query's fault; the possible
    // gate is mentioned without blaming any particular upstream.
    assert!(!hint.contains("claude-macpoo"), "{hint}");
    assert_eq!(value["withheld"][0]["namespace"], "claude-macpoo");
    assert_eq!(value["withheld"][0]["tool_count"], 25);
    assert!(value["withheld"][0].get("guidance").is_none(), "{value}");
}

#[test]
fn javy_snippet_only_search_never_blames_the_read_only_gate() {
    let value = run_withheld_discovery_script(
        "globalThis.result = JSON.stringify(await codemode.search({query: 'claude_macpoo', kinds: ['snippet']}));",
    );

    assert_eq!(value["total"], 0, "{value}");
    assert!(value.get("withheld").is_none(), "{value}");
    assert!(
        !value["hint"].as_str().unwrap().contains("codemode_read"),
        "{value}"
    );
}

#[test]
fn javy_search_naming_withheld_namespace_reports_it_alongside_results() {
    let value = run_withheld_discovery_script(
        "globalThis.result = JSON.stringify(await codemode.search({query: 'claude_macpoo lookup'}));",
    );

    assert_eq!(
        value["withheld"][0]["namespace"], "claude-macpoo",
        "{value}"
    );
    assert!(value["hint"].as_str().unwrap().contains("codemode_read"));
}

#[test]
fn javy_search_without_withheld_relevance_keeps_plain_shape() {
    let value = run_withheld_discovery_script(
        "globalThis.result = JSON.stringify(await codemode.search({query: 'lookup record'}));",
    );

    assert_eq!(value["total"], 1, "{value}");
    assert!(value.get("withheld").is_none(), "{value}");
    assert!(value.get("hint").is_none(), "{value}");
}

#[test]
fn javy_describe_withheld_tool_explains_read_only_gate() {
    for target in [
        "claude_macpoo.Bash",
        "claude-macpoo::Bash",
        "Claude-MacPoo::Read",
    ] {
        let value = run_withheld_discovery_script(&format!(
            "try {{ await codemode.describe({target:?}); globalThis.result = JSON.stringify({{unexpected: true}}); }} \
             catch (error) {{ globalThis.result = error.message; }}"
        ));

        assert_eq!(value["kind"], "forbidden", "{target}: {value}");
        assert_eq!(value["reason"], "read_only_withheld");
        assert_eq!(value["namespace"], "claude-macpoo");
        assert!(
            value["message"]
                .as_str()
                .unwrap()
                .contains("Use the `codemode` tool")
        );
    }
}

#[test]
fn javy_describe_typo_in_partly_visible_namespace_is_unknown_tool() {
    let value = run_withheld_discovery_script_with(
        "annotated",
        "try { await codemode.describe('annotated.lookupp'); } catch (error) { globalThis.result = error.message; }",
    );

    assert_eq!(value["kind"], "unknown_tool", "{value}");
    let message = value["message"].as_str().unwrap();
    assert!(message.contains("codemode.search"), "{message}");
    assert!(message.contains("also hidden"), "{message}");
}

#[test]
fn javy_alias_key_matches_rust_namespace_alias_key() {
    let cases = [
        "claude-macpoo",
        "Claude_MacPoo",
        " a.b c ",
        "UPPER-lower_mixed.dots",
        "plain",
    ];
    let preamble = generate_discovery_js(&[], 0.5, &[]).unwrap();
    let script = format!(
        "{preamble}\nglobalThis.result = JSON.stringify({}.map(__codemodeAliasKey));",
        serde_json::to_string(&cases).unwrap()
    );
    let mut config = javy::Config::default();
    config.memory_limit(8 * 1024 * 1024);
    let runtime = javy::Runtime::new(config).unwrap();
    runtime
        .context()
        .with(|cx| cx.eval::<(), _>(script))
        .unwrap();
    let result: String = runtime
        .context()
        .with(|cx| cx.globals().get("result"))
        .unwrap();
    let js: Vec<String> = serde_json::from_str(&result).unwrap();
    let rust: Vec<String> = cases
        .iter()
        .map(|case| crate::namespace_alias_key(case))
        .collect();
    assert_eq!(js, rust);
}

#[test]
fn javy_proxy_accepts_raw_hyphenated_names_without_clobbering_helpers() {
    let tools = [
        CatalogDescriptor::tool("claude-macpoo", "get-issue", "fixture", None, None),
        CatalogDescriptor::tool("search", "lookup", "fixture", None, None),
    ];
    let refs = tools.iter().collect::<Vec<_>>();
    let discovery = generate_discovery_js(&[], 0.5, &[]).unwrap();
    let proxy = super::generate_js_proxy_from_catalog(&refs).unwrap();
    let script = format!(
        "{discovery}\n{proxy}\n\
         globalThis.calls = [];\n\
         globalThis.callTool = async (id) => {{ calls.push(id); return {{}}; }};\n\
         (async () => {{\n\
           await codemode.claude_macpoo.get_issue({{}});\n\
           await codemode['claude-macpoo']['get-issue']({{}});\n\
           globalThis.result = JSON.stringify({{ calls, searchIsHelper: typeof codemode.search === 'function' }});\n\
         }})().catch(error => {{ globalThis.result = JSON.stringify({{error: String(error)}}); }});"
    );
    let mut config = javy::Config::default();
    config.memory_limit(8 * 1024 * 1024);
    let runtime = javy::Runtime::new(config).unwrap();
    runtime
        .context()
        .with(|cx| cx.eval::<(), _>(script))
        .unwrap();
    runtime.resolve_pending_jobs().unwrap();
    let result: String = runtime
        .context()
        .with(|cx| cx.globals().get("result"))
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(value.get("error").is_none(), "{value}");
    assert_eq!(
        value["calls"],
        serde_json::json!(["claude-macpoo::get-issue", "claude-macpoo::get-issue"])
    );
    assert_eq!(value["searchIsHelper"], true);
}

#[test]
fn javy_describe_unknown_target_points_at_search() {
    let value = run_withheld_discovery_script(
        "try { await codemode.describe('nope.missing'); } catch (error) { globalThis.result = error.message; }",
    );

    assert_eq!(value["kind"], "unknown_tool", "{value}");
    assert!(
        value["message"]
            .as_str()
            .unwrap()
            .contains("codemode.search")
    );
}

#[test]
fn proxy_skips_raw_aliases_for_namespaces_sharing_a_sanitized_key() {
    let tools = [
        CatalogDescriptor::tool("foo-bar", "alpha", "fixture", None, None),
        CatalogDescriptor::tool("foo_bar", "beta", "fixture", None, None),
        CatalogDescriptor::tool("solo-ns", "gamma", "fixture", None, None),
    ];
    let refs = tools.iter().collect::<Vec<_>>();
    let proxy = super::generate_js_proxy_from_catalog(&refs).unwrap();

    assert!(!proxy.contains("codemode[\"foo-bar\"] ="), "{proxy}");
    assert!(
        proxy.contains("codemode[\"solo-ns\"] = codemode[\"solo_ns\"]"),
        "{proxy}"
    );
}
