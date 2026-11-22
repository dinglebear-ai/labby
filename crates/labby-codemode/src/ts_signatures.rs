//! TypeScript signature and `.d.ts` generation for Code Mode catalog entries.
//!
//! Given a tool's namespace name, tool name, description, and JSON Schema for
//! its input/output, this module produces two strings:
//!
//! - `signature` — a human-readable one-liner shown in `search` results,
//!   e.g. `codemode.github.list_tags(params: GithubListTagsInput): Promise<GithubListTagsOutput>`
//! - `dts` — a TypeScript declaration block (`.d.ts`) that IDEs and the
//!   in-browser Monaco editor can use for auto-complete and type checking.
//!
//! This module is the **live** TypeScript generator called from `types.rs` via
//! `CatalogDescriptor::tool`. It is NOT backward-compat shims.

mod schema;

#[cfg(test)]
pub(crate) use schema::json_schema_to_type;
use schema::json_schema_to_type_labeled;

use serde_json::Value;

use super::namespaced_tool_id;
use super::preamble::{namespace_segment, tool_name_to_snake};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolTypes {
    pub(crate) signature: String,
    pub(crate) dts: String,
}

pub(crate) fn generate_tool_types(
    namespace: &str,
    tool: &str,
    description: &str,
    input_schema: Option<&Value>,
    output_schema: Option<&Value>,
) -> ToolTypes {
    let base = format!(
        "{}{}",
        to_pascal_identifier(namespace),
        to_pascal_identifier(tool)
    );
    let input_name = format!("{base}Input");
    let output_name = format!("{base}Output");
    let namespace_interface = format!("Codemode{}Tools", to_pascal_identifier(namespace));
    let namespace_method = namespace_segment(namespace);
    let tool_method = tool_name_to_snake(tool);
    let tool_id = namespaced_tool_id(namespace, tool);
    let tool_id_literal = serde_json::to_string(&tool_id).unwrap_or_else(|_| "\"\"".to_string());
    let input_type = json_schema_to_type_labeled(input_schema, Some(&tool_id));
    let output_type = json_schema_to_type_labeled(output_schema, Some(&tool_id));
    let signature = format!(
        "codemode.{namespace_method}.{tool_method}(params: {input_name}): Promise<{output_name}>"
    );

    let mut dts = String::new();
    dts.push_str(&format!("type {input_name} = {input_type};\n"));
    dts.push_str(&format!("type {output_name} = {output_type};\n"));
    dts.push_str(&format!("interface {namespace_interface} {{\n"));
    if let Some(comment) = jsdoc_block(description, 4) {
        dts.push_str(&comment);
    }
    dts.push_str(&format!(
        "  {tool_method}(params: {input_name}): Promise<{output_name}>;\n"
    ));
    dts.push_str("}\n");
    dts.push_str("interface CodemodeTools {\n");
    dts.push_str(&format!("  callToolResult(id: {tool_id_literal}, params: {input_name}): Promise<{{ content: Array<{{ type: string; [key: string]: unknown }}>; structuredContent?: {output_name}; isError?: boolean; _meta?: Record<string, unknown> }} | {{ host_value: {output_name} }}>;\n"));
    dts.push_str(&format!("  {namespace_method}: {namespace_interface};\n"));
    dts.push_str("}\n");
    dts.push_str("declare var codemode: CodemodeTools;\n");
    dts.push_str(
        "// Keep the final execution return within the configured envelope budget; project, filter, or slice large tool results before returning.\n",
    );
    dts.push_str(&format!(
        "declare function callTool(id: {tool_id_literal}, params: {input_name}): Promise<{output_name}>;\n"
    ));

    ToolTypes { signature, dts }
}

fn quote_prop(key: &str) -> String {
    if is_identifier(key) {
        key.to_string()
    } else {
        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string())
    }
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn jsdoc_block(text: &str, indent: usize) -> Option<String> {
    let text = escape_jsdoc(text.trim());
    if text.is_empty() {
        return None;
    }
    let pad = " ".repeat(indent);
    Some(format!("{pad}/** {text} */\n"))
}

fn property_jsdoc(schema: &Value, indent: usize) -> Option<String> {
    let object = schema.as_object()?;
    let description = object.get("description").and_then(Value::as_str);
    let format = object.get("format").and_then(Value::as_str);
    if description.is_none() && format.is_none() {
        return None;
    }
    if let (Some(description), None) = (description, format) {
        return jsdoc_block(description, indent);
    }
    let pad = " ".repeat(indent);
    let mut lines = Vec::new();
    lines.push(format!("{pad}/**"));
    if let Some(description) = description {
        lines.push(format!("{pad} * {}", escape_jsdoc(description.trim())));
    }
    if let Some(format) = format {
        lines.push(format!("{pad} * @format {}", escape_jsdoc(format.trim())));
    }
    lines.push(format!("{pad} */\n"));
    Some(lines.join("\n"))
}

fn escape_jsdoc(text: &str) -> String {
    text.replace("*/", "* /").replace('\n', " ")
}

fn to_pascal_identifier(value: &str) -> String {
    let mut out = String::new();
    for segment in value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|segment| !segment.is_empty())
    {
        let mut chars = segment.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars.map(|ch| ch.to_ascii_lowercase()));
        }
    }
    if out.is_empty() {
        "Tool".to_string()
    } else if out.starts_with(|ch: char| ch.is_ascii_digit()) {
        format!("_{out}")
    } else {
        out
    }
}
