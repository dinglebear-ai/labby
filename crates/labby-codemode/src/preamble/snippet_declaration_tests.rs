use super::generate_discovery_js;
use crate::snippet::store::{SnippetInfo, SnippetSource};
use crate::snippet::tool_declarations::SnippetToolDeclarations;
use crate::types::{CatalogDescriptor, CodeModeCatalogKind, CodeModeDiscoveryEntry};

#[test]
fn javy_search_and_describe_preserve_declaration_presence() {
    for (tools, expected, description) in [
        (None, None, "omitted (caller policy unchanged)"),
        (
            Some(vec![]),
            Some(serde_json::json!([])),
            "[] (intended deny-all)",
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
        let preamble = generate_discovery_js(&[entry], 0.5).unwrap();
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
        assert!(
            rendered.contains("Metadata only: declarations do not currently restrict execution.")
        );
    }
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
    let preamble = generate_discovery_js(&[tool, snippet], 0.5).unwrap();
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
    let preamble = generate_discovery_js(&[], 0.5).unwrap();
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
    let preamble = generate_discovery_js(&[entry], 0.5).unwrap();
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
