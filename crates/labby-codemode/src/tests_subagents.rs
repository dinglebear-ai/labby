use crate::preamble::{generate_discovery_js, generate_js_proxy_from_catalog};
use crate::types::{CodeModeCatalogKind, CodeModeDiscoveryEntry, ToolDescriptor};

#[test]
fn subagent_descriptor_initialization() {
    let descriptor = ToolDescriptor::subagent(
        "researcher",
        "Researches complex codebase problems",
        Some("Codebase Researcher"),
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string" }
            },
            "required": ["prompt"]
        })),
    );

    assert_eq!(descriptor.kind, CodeModeCatalogKind::Subagent);
    assert_eq!(descriptor.id, "subagent::researcher");
    assert_eq!(descriptor.name, "researcher");
    assert_eq!(descriptor.namespace, "subagent");
    assert_eq!(descriptor.tags, vec!["Codebase Researcher".to_string()]);
    assert!(descriptor.signature.contains("invokeSubagent"));
    assert!(descriptor.dts.contains("declare function invokeSubagent"));
}

#[test]
fn subagent_proxy_generation_emits_helper() {
    let researcher = ToolDescriptor::subagent(
        "researcher",
        "Researches complex codebase problems",
        Some("Codebase Researcher"),
        None,
    );
    let writer = ToolDescriptor::subagent(
        "docs-writer",
        "Writes markdown documentation",
        Some("Documentation Specialist"),
        None,
    );
    let tool = ToolDescriptor::tool("github", "search", "Search issues", None, None);

    let entries = vec![&researcher, &writer, &tool];
    let proxy_js = generate_js_proxy_from_catalog(&entries).expect("proxy generation");

    assert!(proxy_js.contains("codemode[\"subagents\"] = {"));
    assert!(proxy_js.contains("\"researcher\": function(p) { return codemode.invokeSubagent(\"researcher\", p); }"));
    assert!(proxy_js.contains("\"docs_writer\": function(p) { return codemode.invokeSubagent(\"docs-writer\", p); }"));
}

#[test]
fn subagent_discovery_js_contains_invoke_subagent_and_scoring() {
    let researcher = ToolDescriptor::subagent(
        "researcher",
        "Researches complex codebase problems",
        Some("Codebase Researcher"),
        None,
    );
    let discovery_entry = CodeModeDiscoveryEntry::from_catalog(&researcher);
    let js = generate_discovery_js(&[discovery_entry], 0.5).expect("discovery js");

    assert!(js.contains("codemode.invokeSubagent = async function(name, input)"));
    assert!(js.contains("__lab_internal::invoke_subagent"));
    assert!(js.contains("subagent::"));
}
