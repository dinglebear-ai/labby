use super::{namespace_segment, tool_name_to_snake};
use crate::types::{CatalogDescriptor, CodeModeCatalogKind};

/// Generate a JavaScript preamble string that defines the `codemode` proxy
/// namespace for use inside the sandbox.
///
/// The output is a JS snippet (not TypeScript) injected into the user code before
/// being sent to the runner subprocess. It relies on `callTool` already being
/// registered in the sandbox.
///
/// The output ends with `var` declarations (no completion value), so when it is
/// concatenated in front of a trailing IIFE the IIFE's promise remains the
/// `eval` completion value.
///
pub(crate) fn generate_js_proxy_from_catalog(
    tools: &[&CatalogDescriptor],
) -> Result<String, String> {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;

    let mut by_namespace: BTreeMap<&str, Vec<&CatalogDescriptor>> = BTreeMap::new();
    for tool in tools {
        if tool.kind != CodeModeCatalogKind::Tool {
            continue;
        }
        by_namespace.entry(&tool.namespace).or_default().push(*tool);
    }

    let mut parts = String::new();
    // Raw-name aliases (`codemode["claude-macpoo"]["get-issue"]`) emitted
    // after every sanitized key exists; guarded so they never replace a
    // helper or a sanitized key.
    let mut raw_aliases = String::new();
    // Raw namespaces that sanitize to the same key (`foo-bar`, `foo_bar`)
    // share one proxy object; a raw alias would then expose both upstreams'
    // tools under one raw name, so such namespaces get no raw alias.
    let mut raw_per_snake: BTreeMap<String, usize> = BTreeMap::new();
    for namespace_name in by_namespace.keys() {
        *raw_per_snake
            .entry(namespace_segment(namespace_name))
            .or_default() += 1;
    }
    let mut by_snake_namespace: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut key_counts = BTreeMap::new();
    for (namespace, entries) in &by_namespace {
        for entry in entries {
            *key_counts
                .entry((
                    namespace_segment(namespace),
                    tool_name_to_snake(&entry.name),
                ))
                .or_insert(0) += 1;
        }
    }
    for (namespace_name, namespace_tools) in &by_namespace {
        let namespace_snake = namespace_segment(namespace_name);
        let method_defs = by_snake_namespace
            .entry(namespace_snake.clone())
            .or_default();
        let mut sorted_tools = namespace_tools.to_vec();
        sorted_tools.sort_by_cached_key(|tool| tool_name_to_snake(&tool.name));
        for tool in sorted_tools {
            let snake = tool_name_to_snake(&tool.name);
            let dotted = &tool.name;
            if key_counts[&(namespace_snake.clone(), snake.clone())] > 1 {
                // Ambiguous helpers have no target. Their raw ids remain callable.
                continue;
            }
            let tool_id = crate::types::namespaced_tool_id(namespace_name, dotted);
            let tool_id_json =
                serde_json::to_string(&tool_id).unwrap_or_else(|_| "\"unknown\"".to_string());
            let snake_json =
                serde_json::to_string(snake.as_str()).unwrap_or_else(|_| format!("\"{snake}\""));
            method_defs.push(format!(
                "    {snake_json}: function(p) {{ return callTool({tool_id_json}, p == null ? {{}} : p); }}"
            ));
            if dotted != &snake {
                let raw_tool_json = serde_json::to_string(dotted.as_str())
                    .unwrap_or_else(|_| "\"unknown\"".to_string());
                let namespace_json = serde_json::to_string(&namespace_snake)
                    .unwrap_or_else(|_| "\"unknown\"".to_string());
                let _ = writeln!(
                    raw_aliases,
                    "if (!Object.prototype.hasOwnProperty.call(codemode[{namespace_json}], {raw_tool_json})) codemode[{namespace_json}][{raw_tool_json}] = codemode[{namespace_json}][{snake_json}];"
                );
            }
        }
        if *namespace_name != namespace_snake
            && raw_per_snake.get(&namespace_snake).copied() == Some(1)
        {
            let raw_json =
                serde_json::to_string(namespace_name).unwrap_or_else(|_| "\"unknown\"".to_string());
            let snake_json = serde_json::to_string(&namespace_snake)
                .unwrap_or_else(|_| "\"unknown\"".to_string());
            let _ = writeln!(
                raw_aliases,
                "if (!Object.prototype.hasOwnProperty.call(codemode, {raw_json})) codemode[{raw_json}] = codemode[{snake_json}];"
            );
        }
    }

    for (namespace_snake, method_defs) in &by_snake_namespace {
        let namespace_snake_json =
            serde_json::to_string(namespace_snake).unwrap_or_else(|_| "\"unknown\"".to_string());
        let methods = method_defs.join(",\n");
        let _ = write!(
            parts,
            "codemode[{namespace_snake_json}] = {{\n{methods}\n}};\n"
        );
    }

    Ok(format!(
        "// Code Mode proxy — auto-generated\n\
         globalThis.codemode = globalThis.codemode || {{}};\n\
         var codemode = globalThis.codemode;\n\
         {parts}{raw_aliases}"
    ))
}
