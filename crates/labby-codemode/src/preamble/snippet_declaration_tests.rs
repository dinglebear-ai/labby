use super::generate_discovery_js;
use crate::snippet::store::{SnippetInfo, SnippetSource};
use crate::snippet::tool_declarations::SnippetToolDeclarations;
use crate::types::{CatalogDescriptor, CodeModeCatalogKind, CodeModeDiscoveryEntry};

#[test]
fn javy_search_and_describe_preserve_declaration_presence() {
    const POLICY: &str = "Execution policy: native snippets.exec/test intersects a nonempty declaration with caller authority and never grants authority. Nested codemode.run retains the enclosing run scope; it does not reapply the declaration.";
    for (tools, expected, description) in [
        (
            None,
            None,
            "omitted (native exec/test inherits caller scope)",
        ),
        (
            Some(vec![]),
            Some(serde_json::json!([])),
            "[] (native exec/test denies all upstream tools)",
        ),
        (
            Some(vec!["alpha::read".to_owned(), "beta::list".to_owned()]),
            Some(serde_json::json!(["alpha::read", "beta::list"])),
            "alpha::read, beta::list",
        ),
    ] {
        let info = SnippetInfo {
            name: "declaration".into(),
            description: Some("Declaration presentation".into()),
            tags: vec![],
            inputs: Default::default(),
            tools: tools.map(|ids| SnippetToolDeclarations::try_from(ids).unwrap()),
            source: SnippetSource::User,
            path: "declaration.md".into(),
            shadowed: false,
        };
        let entry = CodeModeDiscoveryEntry::from_catalog(&CatalogDescriptor::snippet(&info));
        let preamble = generate_discovery_js(&[entry], 0.5, &[]).unwrap();
        let script = format!(
            "{preamble}\n\
             globalThis.callTool = async () => {{ throw new Error('no host discovery needed'); }};\n\
             globalThis.result = null;\n\
             (async () => {{\n\
               const search = await codemode.search('declaration');\n\
               const description = await codemode.describe('snippet::declaration');\n\
               globalThis.callTool = async () => ({{ranked: [{{id: 'snippet::declaration', score: 1}}]}});\n\
               const semantic = await codemode.search('unrelatedsynonym');\n\
               globalThis.result = JSON.stringify({{search, semantic, description}});\n\
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
        for mode in ["search", "semantic"] {
            assert_eq!(value[mode]["results"].as_array().unwrap().len(), 1);
            assert_eq!(value[mode]["results"][0].get("tools"), expected.as_ref());
        }
        let rendered = value["description"]["markdown"].as_str().unwrap();
        assert!(rendered.contains(description), "{rendered}");
        assert!(rendered.contains(POLICY));
    }
}

#[test]
fn javy_batch_preserves_structured_errors_and_rejects_invalid_jobs() {
    let preamble = generate_discovery_js(&[], 0.5, &[]).unwrap();
    let script = format!(
        "{preamble}\n\
         globalThis.result = null;\n\
         (async () => {{\n\
           const outcome = await codemode.batch([\n\
             () => Promise.resolve({{ok: true}}),\n\
             Promise.resolve('started'),\n\
             () => Promise.reject(new Error(JSON.stringify({{kind: 'timeout', side_effects: 'possible', recovery: {{same_arguments: 'never'}}}}))),\n\
             () => Promise.reject(new Error(JSON.stringify({{message: 'partial'}}))),\n\
             () => Promise.reject(new Error('plain text failure')),\n\
             {{get then() {{ throw new Error('throwing then getter'); }}}},\n\
             undefined, null, 7\n\
           ]);\n\
           globalThis.result = JSON.stringify(outcome);\n\
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
    assert_eq!(value["ok"].as_array().unwrap().len(), 2);
    assert_eq!(value["failed"].as_array().unwrap().len(), 7);
    assert_eq!(value["failed"][0]["error"]["kind"], "timeout");
    assert_eq!(value["failed"][0]["error"]["side_effects"], "possible");
    for failure in value["failed"].as_array().unwrap().iter().take(4).skip(1) {
        assert_eq!(failure["error"]["side_effects"], "unknown");
        assert_eq!(
            failure["error"]["recovery"]["same_arguments"],
            "discouraged"
        );
    }
    assert_eq!(value["failed"][2]["error"]["message"], "plain text failure");
    assert_eq!(
        value["failed"][3]["error"]["message"],
        "throwing then getter"
    );
    for failure in value["failed"].as_array().unwrap().iter().skip(4) {
        assert_eq!(failure["error"]["kind"], "invalid_param");
        assert_eq!(failure["error"]["side_effects"], "none_expected");
        assert_eq!(failure["error"]["recovery"]["same_arguments"], "never");
    }
    assert_eq!(value["all_ok"], false);
}

#[test]
fn javy_search_filters_lexical_and_semantic_results_by_kind() {
    let tool = CodeModeDiscoveryEntry::from_catalog(&CatalogDescriptor::tool(
        "alpha",
        "catalog_tool",
        "catalog kind filter",
        None,
        None,
    ));
    let snippet = CodeModeDiscoveryEntry::from_catalog(&CatalogDescriptor::snippet(&SnippetInfo {
        name: "catalog_snippet".into(),
        description: Some("catalog kind filter".into()),
        tags: vec![],
        inputs: Default::default(),
        tools: None,
        source: SnippetSource::User,
        path: "catalog-snippet.md".into(),
        shadowed: false,
    }));
    let preamble = generate_discovery_js(&[tool, snippet], 0.5, &[]).unwrap();
    let script = format!(
        "{preamble}\n\
         globalThis.callTool = async (_id, params) => {{\n\
           globalThis.semanticParams = params;\n\
           return {{ranked: [\n\
             {{id: 'alpha::catalog_tool', score: 1}},\n\
             {{id: 'snippet::catalog_snippet', score: 0.9}}\n\
           ]}};\n\
         }};\n\
         globalThis.result = null;\n\
         (async () => {{\n\
           const all = await codemode.search({{query: 'catalog kind filter'}});\n\
           const tools = await codemode.search({{query: 'catalog kind filter', kinds: ['tool']}});\n\
           const snippets = await codemode.search({{query: 'catalog kind filter', kinds: [' SNIPPET ']}});\n\
           const unknown = await codemode.search({{query: 'catalog kind filter', kinds: ['__proto__']}});\n\
           const semantic = await codemode.search({{query: 'unrelatedsynonym', kinds: [' SNIPPET ']}});\n\
           globalThis.result = JSON.stringify({{all, tools, snippets, unknown, semantic, semanticParams}});\n\
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
    assert_eq!(value["all"]["results"].as_array().unwrap().len(), 2);
    assert_eq!(value["tools"]["results"].as_array().unwrap().len(), 1);
    assert_eq!(value["tools"]["results"][0]["kind"], "tool");
    assert_eq!(value["snippets"]["results"].as_array().unwrap().len(), 1);
    assert_eq!(value["snippets"]["results"][0]["kind"], "snippet");
    assert_eq!(value["semantic"]["results"].as_array().unwrap().len(), 1);
    assert_eq!(value["unknown"]["results"].as_array().unwrap().len(), 0);
    assert_eq!(value["semantic"]["results"][0]["kind"], "snippet");
    assert_eq!(
        value["semanticParams"]["kinds"],
        serde_json::json!(["snippet"])
    );
}

#[test]
fn javy_prompt_and_skill_helpers_dispatch_exact_internal_calls() {
    let preamble = generate_discovery_js(&[], 0.5, &[]).unwrap();
    let script = format!(
        "{preamble}
         globalThis.calls = [];
         globalThis.callTool = async (id, params) => {{
           globalThis.calls.push({{id, params}});
           return {{id, params}};
         }};
         globalThis.result = null;
         (async () => {{
           const prompt = await codemode.getPrompt('prompt::alpha::review', {{tone: 'strict'}});
           const skills = await codemode.listSkills();
           const skill = await codemode.getSkill('skill://labby/fixture');
           const read = await codemode.readSkill('skill://labby/fixture/SKILL.md');
           const validation = [];
           try {{ await codemode.getPrompt('prompt::alpha::review', []); }}
           catch (error) {{ validation.push(String(error)); }}
           try {{ await codemode.getSkill('   '); }}
           catch (error) {{ validation.push(String(error)); }}
           globalThis.result = JSON.stringify({{prompt, skills, skill, read, calls, validation}});
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
    assert_eq!(value["calls"].as_array().unwrap().len(), 4);
    assert_eq!(
        value["calls"],
        serde_json::json!([
            {
                "id": "__lab_internal::get_prompt",
                "params": {
                    "prompt": "prompt::alpha::review",
                    "arguments": { "tone": "strict" }
                }
            },
            {
                "id": "__lab_internal::list_skills",
                "params": {}
            },
            {
                "id": "__lab_internal::get_skill",
                "params": { "uri": "skill://labby/fixture" }
            },
            {
                "id": "__lab_internal::read_skill",
                "params": { "uri": "skill://labby/fixture/SKILL.md" }
            }
        ])
    );
    assert_eq!(
        value["prompt"]["params"]["arguments"],
        serde_json::json!({ "tone": "strict" })
    );
    assert_eq!(
        value["read"]["params"]["uri"],
        "skill://labby/fixture/SKILL.md"
    );
    let validation = value["validation"].as_array().unwrap();
    assert_eq!(validation.len(), 2);
    assert!(
        validation[0]
            .as_str()
            .unwrap()
            .contains("arguments must be an object")
    );
    assert!(
        validation[1]
            .as_str()
            .unwrap()
            .contains("requires a non-empty Skill URI")
    );
    assert_eq!(
        value["calls"].as_array().unwrap().len(),
        4,
        "validation failures must not dispatch extra internal calls"
    );
}

#[test]
fn javy_describe_keeps_future_catalog_kinds_metadata_only() {
    let mut skill = CatalogDescriptor::metadata(
        CodeModeCatalogKind::Skill,
        "labby",
        "skill::skill://labby/adversarial-review",
        "adversarial_review",
        "Review code adversarially",
        vec!["uri:skill://labby/adversarial-review".to_string()],
    );
    skill.dts = "this must never be fetched for metadata-only kinds".into();

    let entry = CodeModeDiscoveryEntry::from_catalog(&skill);
    let preamble = generate_discovery_js(&[entry], 0.5, &[]).unwrap();
    let script = format!(
        "{preamble}\n\
         globalThis.describeTypeCalls = 0;\n\
         globalThis.callTool = async (id) => {{\n\
           if (id === '__lab_internal::describe_types') globalThis.describeTypeCalls++;\n\
           throw new Error('metadata-only describe must not call the host');\n\
         }};\n\
         globalThis.result = null;\n\
         (async () => {{\n\
           const description = await codemode.describe('skill.labby.adversarial_review');\n\
           globalThis.result = JSON.stringify({{description, describeTypeCalls}});\n\
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
    assert_eq!(value["description"]["kind"], "skill");
    assert_eq!(
        value["description"]["id"],
        "skill::skill://labby/adversarial-review"
    );
    assert_eq!(
        value["description"]["helper"],
        r#"codemode.getSkill("skill://labby/adversarial-review")"#
    );
    assert_eq!(
        value["description"]["tags"],
        serde_json::json!(["uri:skill://labby/adversarial-review"])
    );
    assert_eq!(value["describeTypeCalls"], 0);
    let markdown = value["description"]["markdown"].as_str().unwrap();
    assert!(markdown.contains("- kind: `skill`"), "{markdown}");
    assert!(!markdown.contains("Parameters (TypeScript)"), "{markdown}");
}

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
