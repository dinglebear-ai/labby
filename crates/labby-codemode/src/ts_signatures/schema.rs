//! JSON Schema to TypeScript type expression rendering.

use std::collections::{BTreeSet, HashSet};

use serde_json::Value;

use super::{property_jsdoc, quote_prop};

/// Test-only unlabeled entry point; production callers go through
/// `generate_tool_types`, which labels renders with the tool id.
#[cfg(test)]
pub(crate) fn json_schema_to_type(schema: Option<&Value>) -> String {
    json_schema_to_type_labeled(schema, None)
}

/// Render a schema to a TS type, attributing any budget exhaustion to
/// `label` (a `namespace::tool` id) in the warning log.
pub(super) fn json_schema_to_type_labeled(schema: Option<&Value>, label: Option<&str>) -> String {
    let Some(schema) = schema else {
        return "unknown".to_string();
    };
    let mut budget = TypeRenderBudget::new();
    let rendered = schema_to_type(schema, schema, 0, &mut budget);
    if budget.exhausted {
        // FR-9b: a partial render can silently misdescribe the tool, and the
        // unbounded alternative is an attacker-controlled multi-GB String.
        // `unknown` is always truthful.
        tracing::warn!(
            surface = "dispatch",
            service = "code_mode",
            action = "type.render_budget_exceeded",
            tool = label.unwrap_or("<unlabeled>"),
            nodes = budget.nodes,
            rendered_bytes = budget.bytes,
            "schema type render exceeded its expansion budget; publishing `unknown` instead"
        );
        return "unknown".to_string();
    }
    rendered
}

/// FR-9b (issue #210, lab-41e7m.7): recursion state for one type render.
///
/// `schema_to_type_unbudgeted` removes a `$ref` from `seen_refs` on return, so shared
/// non-cyclic refs re-expand at every occurrence — O(B^depth). The depth cap
/// alone does not bound the OUTPUT: the function returns a `String` built by
/// concatenation, so a hostile wide-and-deep `$defs` graph produces a
/// multi-gigabyte allocation (an OOM kill, not a slow request). The budget
/// bounds both node visits and accumulated rendered bytes; exhaustion makes
/// the whole render collapse to `unknown`.
///
/// Deliberately NOT a `(ref, root)` memo cache: expansion depends on the
/// current `seen_refs` set, so a cached entry would return a cycle-truncated
/// result where full expansion was correct, and it would bound no output.
struct TypeRenderBudget {
    seen_refs: HashSet<String>,
    nodes: usize,
    bytes: usize,
    exhausted: bool,
}

/// Node-visit cap. Legitimate schemas render in hundreds to low thousands of
/// visits; the 512 KB input gate (`MAX_SCHEMA_BYTES`) bounds honest inputs
/// long before this.
const MAX_TYPE_RENDER_NODES: usize = 100_000;

/// Accumulated rendered bytes, charged per node and therefore depth-weighted:
/// a final string of `n` bytes at nesting depth `d` charges ~`n × d`.
///
/// Sized against the input gate, not picked round. Schemas reaching this
/// renderer already passed `sanitize_schema`'s 512 KB ceiling
/// (`MAX_SCHEMA_BYTES` in labby-gateway), and that ceiling was itself *raised*
/// from 16 KB precisely because the smaller value collapsed legitimate
/// action-routed schemas (cortex, axon) to `unknown`. A 512 KB schema can
/// render a few hundred KB of TypeScript at depth ~10, which charges several
/// MB — so a 4 MB budget would have re-introduced that same collapse for the
/// same tools. 64 MB leaves ~2 orders of magnitude of headroom over any
/// legitimate input while still bounding the hostile case, which heads to
/// gigabytes (an OOM kill, not a slow request).
const MAX_TYPE_RENDER_BYTES: usize = 64 * 1024 * 1024;

impl TypeRenderBudget {
    fn new() -> Self {
        Self {
            seen_refs: HashSet::new(),
            nodes: 0,
            bytes: 0,
            exhausted: false,
        }
    }

    /// Charge one node visit; `false` once the budget is exhausted.
    fn enter(&mut self) -> bool {
        if self.exhausted {
            return false;
        }
        self.nodes += 1;
        if self.nodes > MAX_TYPE_RENDER_NODES {
            self.exhausted = true;
            return false;
        }
        true
    }

    fn charge_bytes(&mut self, len: usize) {
        self.bytes = self.bytes.saturating_add(len);
        if self.bytes > MAX_TYPE_RENDER_BYTES {
            self.exhausted = true;
        }
    }
}

fn schema_to_type(
    schema: &Value,
    root: &Value,
    depth: usize,
    budget: &mut TypeRenderBudget,
) -> String {
    if !budget.enter() {
        return "unknown".to_string();
    }
    let rendered = schema_to_type_unbudgeted(schema, root, depth, budget);
    budget.charge_bytes(rendered.len());
    rendered
}

fn schema_to_type_unbudgeted(
    schema: &Value,
    root: &Value,
    depth: usize,
    budget: &mut TypeRenderBudget,
) -> String {
    if let Some(value) = schema.as_bool() {
        return if value { "unknown" } else { "never" }.to_string();
    }
    if depth > 20 {
        return "unknown".to_string();
    }
    let Some(object) = schema.as_object() else {
        return "unknown".to_string();
    };

    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        if !budget.seen_refs.insert(reference.to_string()) {
            return "unknown".to_string();
        }
        let resolved = resolve_ref(root, reference)
            .map(|schema| schema_to_type(schema, root, depth + 1, budget))
            .unwrap_or_else(|| "unknown".to_string());
        budget.seen_refs.remove(reference);
        return resolved;
    }

    let has_composition = ["anyOf", "oneOf", "allOf"]
        .iter()
        .any(|key| object.get(*key).and_then(Value::as_array).is_some());
    if has_composition {
        let mut parts = Vec::new();
        let mut base = object.clone();
        base.remove("anyOf");
        base.remove("oneOf");
        base.remove("allOf");
        base.remove("nullable");
        let base_type = schema_to_type(&Value::Object(base), root, depth + 1, budget);
        if base_type != "unknown" {
            parts.push((base_type, false));
        }

        for key in ["anyOf", "oneOf"] {
            if let Some(values) = object.get(key).and_then(Value::as_array) {
                let rendered = union(
                    values
                        .iter()
                        .map(|value| schema_to_type(value, root, depth + 1, budget)),
                );
                if rendered != "unknown" {
                    parts.push((rendered, true));
                }
            }
        }
        if let Some(values) = object.get("allOf").and_then(Value::as_array) {
            parts.extend(values.iter().filter_map(|value| {
                let rendered = schema_to_type(value, root, depth + 1, budget);
                (rendered != "unknown").then_some((rendered, false))
            }));
        }

        let mut rendered = match parts.len() {
            0 => "unknown".to_string(),
            1 => parts.pop().expect("single composition part").0,
            _ => intersection(parts.into_iter().map(|(part, is_union)| {
                if is_union && part.contains(" | ") {
                    format!("({part})")
                } else {
                    part
                }
            })),
        };
        if object
            .get("nullable")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && !rendered.split('|').any(|part| part.trim() == "null")
        {
            rendered.push_str(" | null");
        }
        return rendered;
    }

    if let Some(value) = object.get("const") {
        return literal_type(value);
    }
    if let Some(values) = object.get("enum").and_then(Value::as_array) {
        return union(values.iter().map(literal_type));
    }

    let mut rendered = match object.get("type") {
        Some(Value::Array(types)) => union(types.iter().map(|value| {
            value
                .as_str()
                .map(|kind| schema_type_to_type(kind, schema, root, depth, budget))
                .unwrap_or_else(|| "unknown".to_string())
        })),
        Some(Value::String(kind)) => schema_type_to_type(kind, schema, root, depth, budget),
        _ if object.contains_key("properties") || object.contains_key("additionalProperties") => {
            object_type(schema, root, depth, budget)
        }
        _ if object.contains_key("items") || object.contains_key("prefixItems") => {
            array_type(schema, root, depth, budget)
        }
        _ => "unknown".to_string(),
    };

    if object
        .get("nullable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && !rendered.split('|').any(|part| part.trim() == "null")
    {
        rendered.push_str(" | null");
    }

    rendered
}

fn schema_type_to_type(
    kind: &str,
    schema: &Value,
    root: &Value,
    depth: usize,
    budget: &mut TypeRenderBudget,
) -> String {
    match kind {
        "object" => object_type(schema, root, depth, budget),
        "array" => array_type(schema, root, depth, budget),
        "string" if schema.get("format").and_then(Value::as_str) == Some("binary") => {
            "Uint8Array | ArrayBuffer".to_string()
        }
        "string" => "string".to_string(),
        "integer" | "number" => "number".to_string(),
        "boolean" => "boolean".to_string(),
        "null" => "null".to_string(),
        _ => "unknown".to_string(),
    }
}

fn object_type(
    schema: &Value,
    root: &Value,
    depth: usize,
    budget: &mut TypeRenderBudget,
) -> String {
    let Some(object) = schema.as_object() else {
        return "Record<string, unknown>".to_string();
    };

    let required = object
        .get("required")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();

    let mut lines = Vec::new();
    let mut property_index_types = Vec::new();
    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        for key in properties.keys() {
            let property = &properties[key];
            if let Some(comment) = property_jsdoc(property, 2) {
                lines.push(comment.trim_end().to_string());
            }
            let property_type = schema_to_type(property, root, depth + 1, budget);
            let is_required = required.contains(key.as_str());
            let optional = if is_required { "" } else { "?" };
            push_union_parts(&mut property_index_types, &property_type);
            if !is_required {
                property_index_types.push("undefined".to_string());
            }
            lines.push(format!(
                "  {}{}: {};",
                quote_prop(key),
                optional,
                property_type
            ));
        }
    }

    match object.get("additionalProperties") {
        Some(Value::Object(_)) => {
            let additional_type =
                schema_to_type(&object["additionalProperties"], root, depth + 1, budget);
            if property_index_types.is_empty() {
                lines.push(format!("  [key: string]: {additional_type};"));
            } else {
                lines.push(format!(
                    "  /** Additional properties match: {additional_type} */"
                ));
                let mut index_types = Vec::new();
                push_union_parts(&mut index_types, &additional_type);
                index_types.extend(property_index_types);
                lines.push(format!(
                    "  [key: string]: {};",
                    union(index_types.into_iter())
                ));
            }
        }
        Some(Value::Bool(true)) => lines.push("  [key: string]: unknown;".to_string()),
        Some(Value::Bool(false)) => {}
        _ => {}
    }

    if lines.is_empty() {
        if object.get("additionalProperties").and_then(Value::as_bool) == Some(false) {
            return "Record<string, never>".to_string();
        }
        return "Record<string, unknown>".to_string();
    }

    format!("{{\n{}\n}}", lines.join("\n"))
}

fn array_type(schema: &Value, root: &Value, depth: usize, budget: &mut TypeRenderBudget) -> String {
    let Some(object) = schema.as_object() else {
        return "unknown[]".to_string();
    };

    // Tuple form: `prefixItems` (draft 2020-12) or a legacy array-valued `items`.
    if let Some(tuple) = object
        .get("prefixItems")
        .or_else(|| object.get("items"))
        .and_then(Value::as_array)
    {
        let items = tuple
            .iter()
            .map(|item| schema_to_type(item, root, depth + 1, budget))
            .collect::<Vec<_>>()
            .join(", ");
        return format!("[{items}]");
    }

    let item_type = object
        .get("items")
        .map(|items| schema_to_type(items, root, depth + 1, budget))
        .unwrap_or_else(|| "unknown".to_string());
    format!("Array<{item_type}>")
}

fn resolve_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    reference
        .strip_prefix('#')
        .and_then(|pointer| root.pointer(pointer))
}

fn union(types: impl Iterator<Item = String>) -> String {
    let mut seen = BTreeSet::new();
    let types = types
        .filter(|ty| seen.insert(ty.clone()))
        .collect::<Vec<_>>();
    if types.is_empty() {
        "unknown".to_string()
    } else {
        types.join(" | ")
    }
}

fn intersection(types: impl Iterator<Item = String>) -> String {
    let types = types.collect::<Vec<_>>();
    if types.is_empty() {
        "unknown".to_string()
    } else {
        types.join(" & ")
    }
}

fn push_union_parts(types: &mut Vec<String>, ty: &str) {
    types.extend(
        ty.split('|')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(ToString::to_string),
    );
}

fn literal_type(value: &Value) -> String {
    match value {
        Value::String(text) => serde_json::to_string(text).unwrap_or_else(|_| "string".to_string()),
        Value::Number(number) => number.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(_) | Value::Object(_) => {
            serde_json::to_string(value).unwrap_or_else(|_| "unknown".to_string())
        }
    }
}
