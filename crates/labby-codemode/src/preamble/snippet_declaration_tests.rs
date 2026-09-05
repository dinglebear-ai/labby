use super::generate_discovery_js;
use crate::snippet::store::{SnippetInfo, SnippetSource};
use crate::snippet::tool_declarations::SnippetToolDeclarations;
use crate::types::{CodeModeDiscoveryEntry, ToolDescriptor};

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
        let entry = CodeModeDiscoveryEntry::from_catalog(&ToolDescriptor::snippet(&info));
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
