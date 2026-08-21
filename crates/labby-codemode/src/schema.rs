//! JSON Schema validation for Code Mode `callTool` params.
//!
//! This is deliberately a small MCP `inputSchema` validator, not a full JSON
//! Schema implementation. It enforces the assertion keywords Lab currently
//! depends on for tool-call safety:
//!
//! - structure: `$ref` for local JSON pointers, `type`, `properties`,
//!   `required`, `additionalProperties`, `patternProperties`, and `items`
//! - composition: `anyOf`, `oneOf`, and `allOf`
//! - values: `enum`, `const`, `minimum`, `maximum`, `minLength`, `maxLength`,
//!   `pattern`, `minItems`, `maxItems`, and `uniqueItems`
//!
//! Annotation-only keywords such as `title`, `description`, `default`, and
//! `examples` are intentionally ignored. Other JSON Schema assertion keywords
//! are also ignored rather than treated as validation failures, so adding a new
//! assertion to this file must include a focused test that proves the supported
//! subset changed. OpenAPI 3.0-style `nullable: true` is honored as `type ∪
//! null`, matching the `T | null` rendering in `ts_signatures.rs`.
//!
//! Schema *defects* — non-local/unresolved/cyclic `$ref`s, invalid patterns,
//! malformed subschemas, and depth/work-budget exhaustion — fail closed: they
//! always surface as structured `invalid_param` rejections and are never
//! swallowed by composition keywords (`not`/`if`/`anyOf`/`oneOf`).
#![deny(clippy::expect_used, clippy::unwrap_used)]

use std::collections::{BTreeSet, HashSet};

use serde_json::{Map, Value};

use crate::error::ToolError;

/// Serialize `value` with object keys sorted recursively, so two `Value`-equal
/// inputs always produce the same string. Used to key `uniqueItems` dedup in a
/// `HashSet` without depending on object key insertion order (serde_json runs
/// with `preserve_order` in this crate).
fn canonical_json(value: &Value) -> String {
    fn write(value: &Value, out: &mut String) {
        match value {
            Value::Object(map) => {
                out.push('{');
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort_unstable();
                for (index, key) in keys.into_iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    // Quote the key via serde so it is escaped identically to a
                    // normal JSON string.
                    out.push_str(&Value::String(key.clone()).to_string());
                    out.push(':');
                    write(&map[key], out);
                }
                out.push('}');
            }
            Value::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write(item, out);
                }
                out.push(']');
            }
            // Scalars serialize unambiguously; reuse serde for correct escaping
            // and number formatting.
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out
}

/// Validate Code Mode tool parameters against an optional JSON Schema.
pub fn validate_code_mode_params_against_schema(
    params: &Value,
    schema: Option<&Value>,
) -> Result<(), ToolError> {
    if let Some(schema) = schema {
        validate_json_schema_value(params, schema, "params")?;
    }
    Ok(())
}

/// Maximum schema recursion depth (mirrors the `ts_signatures.rs` depth guard).
/// Legitimate MCP `inputSchema`s nest a handful of levels; anything deeper is a
/// hostile or broken schema and is rejected as a schema defect.
const MAX_SCHEMA_DEPTH: usize = 64;

/// Total node-visit budget for one validation run. Composition keywords
/// (`anyOf`/`oneOf`/`not`/`if`) clone `seen_refs` per branch, so a crafted
/// `$ref` fan-out can otherwise explore exponentially many schema paths.
/// The budget is calibrated well above value-linear validation of even very
/// large params (each value node against a simple schema costs one visit) while
/// stopping exponential blowups within milliseconds.
const MAX_SCHEMA_VISITS: usize = 262_144;

/// Internal validation failure classification.
///
/// A `Mismatch` means the *value* does not satisfy the schema — composition
/// keywords (`not`, `if`, `anyOf`, `oneOf`) may swallow it as "branch did not
/// match". A `Defect` means the *schema itself* is broken (non-local,
/// unresolved, or cyclic `$ref`; invalid `pattern`; malformed subschema;
/// depth/budget exhaustion) and must fail closed: it always propagates to the
/// caller instead of silently flipping a branch decision.
enum SchemaCheck {
    Mismatch(ToolError),
    Defect(ToolError),
}

impl SchemaCheck {
    fn into_tool_error(self) -> ToolError {
        match self {
            Self::Mismatch(error) | Self::Defect(error) => error,
        }
    }
}

fn mismatch(path: &str, detail: &str) -> SchemaCheck {
    SchemaCheck::Mismatch(invalid_schema_param(path, detail))
}

fn defect(path: &str, detail: &str) -> SchemaCheck {
    SchemaCheck::Defect(invalid_schema_param(path, detail))
}

fn json_value_matches_schema_type(value: &Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn validate_json_schema_value(value: &Value, schema: &Value, path: &str) -> Result<(), ToolError> {
    let mut seen_refs = BTreeSet::new();
    let mut visits = 0usize;
    validate_json_schema_value_inner(value, schema, schema, path, 0, &mut visits, &mut seen_refs)
        .map_err(SchemaCheck::into_tool_error)
}

fn validate_json_schema_value_inner(
    value: &Value,
    schema: &Value,
    root_schema: &Value,
    path: &str,
    depth: usize,
    visits: &mut usize,
    seen_refs: &mut BTreeSet<String>,
) -> Result<(), SchemaCheck> {
    // Recursion protection: `seen_refs` is cloned per composition branch (so a
    // ref may legitimately reappear on a sibling path), which means cycle
    // detection alone cannot bound the work. The shared visit budget and the
    // depth cap turn any schema bomb into a fast structured rejection.
    *visits += 1;
    if *visits > MAX_SCHEMA_VISITS {
        return Err(defect(
            path,
            "exceeds the schema validation work budget in inputSchema",
        ));
    }
    if depth > MAX_SCHEMA_DEPTH {
        return Err(defect(
            path,
            "exceeds the supported schema nesting depth in inputSchema",
        ));
    }
    if let Some(allowed) = schema.as_bool() {
        return if allowed {
            Ok(())
        } else {
            Err(mismatch(path, "is rejected by false schema"))
        };
    }
    let Some(schema_object) = schema.as_object() else {
        // A schema position holding a non-object, non-boolean value is a
        // schema defect, not a value mismatch: fail closed instead of silently
        // accepting every value.
        return Err(defect(
            path,
            "has a malformed (non-object, non-boolean) subschema in inputSchema",
        ));
    };

    if let Some(reference) = schema_object.get("$ref").and_then(Value::as_str) {
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| defect(path, "uses an unsupported non-local $ref in inputSchema"))?;
        if !seen_refs.insert(reference.to_string()) {
            return Err(defect(path, "contains a cyclic $ref in inputSchema"));
        }
        let referenced_schema = root_schema
            .pointer(pointer)
            .ok_or_else(|| defect(path, "uses an unresolved local $ref in inputSchema"))?;
        validate_json_schema_value_inner(
            value,
            referenced_schema,
            root_schema,
            path,
            depth + 1,
            visits,
            seen_refs,
        )?;
        seen_refs.remove(reference);
    }

    if let Some(not_schema) = schema_object.get("not") {
        let mut branch_refs = seen_refs.clone();
        match validate_json_schema_value_inner(
            value,
            not_schema,
            root_schema,
            path,
            depth + 1,
            visits,
            &mut branch_refs,
        ) {
            Ok(()) => return Err(mismatch(path, "must not match schema")),
            // A defective `not` subschema must not silently accept the value.
            Err(SchemaCheck::Defect(error)) => return Err(SchemaCheck::Defect(error)),
            Err(SchemaCheck::Mismatch(_)) => {}
        }
    }

    if let Some(if_schema) = schema_object.get("if") {
        let mut condition_refs = seen_refs.clone();
        let condition_matches = match validate_json_schema_value_inner(
            value,
            if_schema,
            root_schema,
            path,
            depth + 1,
            visits,
            &mut condition_refs,
        ) {
            Ok(()) => true,
            // A defective `if` condition must not silently route to `else`.
            Err(SchemaCheck::Defect(error)) => return Err(SchemaCheck::Defect(error)),
            Err(SchemaCheck::Mismatch(_)) => false,
        };
        let branch = if condition_matches {
            schema_object.get("then")
        } else {
            schema_object.get("else")
        };
        if let Some(branch_schema) = branch {
            validate_json_schema_value_inner(
                value,
                branch_schema,
                root_schema,
                path,
                depth + 1,
                visits,
                seen_refs,
            )?;
        }
    }

    if let Some(values) = schema_object.get("enum").and_then(Value::as_array)
        && !values.iter().any(|candidate| candidate == value)
    {
        return Err(mismatch(path, "must match enum"));
    }
    if let Some(const_value) = schema_object.get("const")
        && const_value != value
    {
        return Err(mismatch(path, "must match const"));
    }

    if let Some(variants) = schema_object.get("anyOf").and_then(Value::as_array) {
        let mut any_matched = false;
        for variant in variants {
            match validate_json_schema_value_inner(
                value,
                variant,
                root_schema,
                path,
                depth + 1,
                visits,
                &mut seen_refs.clone(),
            ) {
                Ok(()) => {
                    any_matched = true;
                    break;
                }
                Err(SchemaCheck::Defect(error)) => return Err(SchemaCheck::Defect(error)),
                Err(SchemaCheck::Mismatch(_)) => {}
            }
        }
        if !any_matched {
            return Err(mismatch(path, "must match at least one schema"));
        }
    }
    if let Some(variants) = schema_object.get("oneOf").and_then(Value::as_array) {
        let mut matches = 0usize;
        for variant in variants {
            match validate_json_schema_value_inner(
                value,
                variant,
                root_schema,
                path,
                depth + 1,
                visits,
                &mut seen_refs.clone(),
            ) {
                Ok(()) => matches += 1,
                Err(SchemaCheck::Defect(error)) => return Err(SchemaCheck::Defect(error)),
                Err(SchemaCheck::Mismatch(_)) => {}
            }
        }
        if matches != 1 {
            return Err(mismatch(path, "must match exactly one schema"));
        }
    }
    if let Some(variants) = schema_object.get("allOf").and_then(Value::as_array) {
        for variant in variants {
            validate_json_schema_value_inner(
                value,
                variant,
                root_schema,
                path,
                depth + 1,
                visits,
                seen_refs,
            )?;
        }
    }

    if let Some(type_value) = schema_object.get("type") {
        // OpenAPI 3.0-style `nullable: true` widens the declared type to
        // `type ∪ null`. `ts_signatures.rs` renders these schemas as
        // `T | null`, so the validator must accept the null the generated
        // `.d.ts` advertises.
        let nullable = schema_object.get("nullable").and_then(Value::as_bool) == Some(true);
        let matches_type = (nullable && value.is_null())
            || match type_value {
                Value::String(expected) => {
                    json_value_matches_schema_type(value, expected)
                        || schema_accepts_binary_sentinel(value, schema_object, expected)
                }
                Value::Array(types) => types.iter().filter_map(Value::as_str).any(|expected| {
                    json_value_matches_schema_type(value, expected)
                        || schema_accepts_binary_sentinel(value, schema_object, expected)
                }),
                _ => true,
            };
        if !matches_type {
            return Err(mismatch(path, "has wrong type"));
        }
    }

    if let Some(minimum) = schema_object.get("minimum").and_then(Value::as_f64)
        && value.as_f64().is_some_and(|actual| actual < minimum)
    {
        return Err(mismatch(path, "is below minimum"));
    }
    if let Some(maximum) = schema_object.get("maximum").and_then(Value::as_f64)
        && value.as_f64().is_some_and(|actual| actual > maximum)
    {
        return Err(mismatch(path, "is above maximum"));
    }

    if let Some(actual) = value.as_str() {
        if let Some(min_length) = schema_object.get("minLength").and_then(Value::as_u64)
            && actual.chars().count() < min_length as usize
        {
            return Err(mismatch(path, "is shorter than minLength"));
        }
        if let Some(max_length) = schema_object.get("maxLength").and_then(Value::as_u64)
            && actual.chars().count() > max_length as usize
        {
            return Err(mismatch(path, "is longer than maxLength"));
        }
        if let Some(pattern) = schema_object.get("pattern").and_then(Value::as_str) {
            let regex = regex::Regex::new(pattern)
                .map_err(|_| defect(path, "has an invalid pattern in inputSchema"))?;
            if !regex.is_match(actual) {
                return Err(mismatch(path, "does not match pattern"));
            }
        }
    }

    if let Some(object) = value.as_object() {
        if let Some(required) = schema_object.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(key) {
                    return Err(if path == "params" {
                        SchemaCheck::Mismatch(ToolError::Sdk {
                            sdk_kind: "missing_param".to_string(),
                            message: format!("callTool params missing required field `{key}`"),
                        })
                    } else {
                        mismatch(&format!("{path}.{key}"), "is required")
                    });
                }
            }
        }
        let properties = schema_object.get("properties").and_then(Value::as_object);
        let pattern_properties = schema_object
            .get("patternProperties")
            .and_then(Value::as_object);
        let mut matched_pattern_keys = BTreeSet::new();
        if let Some(pattern_properties) = pattern_properties {
            for (pattern, pattern_schema) in pattern_properties {
                let regex = regex::Regex::new(pattern).map_err(|_| {
                    defect(path, "has an invalid patternProperties key in inputSchema")
                })?;
                for (key, property_value) in object {
                    if regex.is_match(key) {
                        matched_pattern_keys.insert(key.clone());
                        validate_json_schema_value_inner(
                            property_value,
                            pattern_schema,
                            root_schema,
                            &format!("{path}.{key}"),
                            depth + 1,
                            visits,
                            seen_refs,
                        )?;
                    }
                }
            }
        }
        let additional_properties = schema_object.get("additionalProperties");
        if additional_properties.and_then(Value::as_bool) == Some(false) {
            for key in object.keys() {
                if properties.is_none_or(|properties| !properties.contains_key(key))
                    && !matched_pattern_keys.contains(key)
                {
                    return Err(mismatch(
                        &format!("{path}.{key}"),
                        "is not allowed by inputSchema",
                    ));
                }
            }
        }
        if let Some(properties) = properties {
            for (key, property_schema) in properties {
                if let Some(property_value) = object.get(key) {
                    validate_json_schema_value_inner(
                        property_value,
                        property_schema,
                        root_schema,
                        &format!("{path}.{key}"),
                        depth + 1,
                        visits,
                        seen_refs,
                    )?;
                }
            }
        }
        if let Some(additional_schema) = additional_properties.filter(|value| value.is_object()) {
            for (key, property_value) in object {
                if properties.is_some_and(|properties| properties.contains_key(key))
                    || matched_pattern_keys.contains(key)
                {
                    continue;
                }
                validate_json_schema_value_inner(
                    property_value,
                    additional_schema,
                    root_schema,
                    &format!("{path}.{key}"),
                    depth + 1,
                    visits,
                    seen_refs,
                )?;
            }
        }
    }

    if let Some(array) = value.as_array() {
        if let Some(min_items) = schema_object.get("minItems").and_then(Value::as_u64)
            && array.len() < min_items as usize
        {
            return Err(mismatch(path, "has fewer items than minItems"));
        }
        if let Some(max_items) = schema_object.get("maxItems").and_then(Value::as_u64)
            && array.len() > max_items as usize
        {
            return Err(mismatch(path, "has more items than maxItems"));
        }
        if schema_object
            .get("uniqueItems")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            // O(n) dedup via a set of canonical (sorted-key) serializations.
            // Canonicalization makes the string key match `Value` equality even
            // though `serde_json` preserves object key insertion order, so this
            // is semantically identical to the previous O(n²) `right == left`
            // pairwise scan (same accept/reject, same error).
            let mut seen = HashSet::with_capacity(array.len());
            for item in array {
                if !seen.insert(canonical_json(item)) {
                    return Err(mismatch(path, "must contain unique items"));
                }
            }
        }
        if let Some(items) = schema_object.get("items") {
            if let Some(tuple_items) = items.as_array() {
                for (index, item_schema) in tuple_items.iter().enumerate() {
                    if let Some(item_value) = array.get(index) {
                        validate_json_schema_value_inner(
                            item_value,
                            item_schema,
                            root_schema,
                            &format!("{path}[{index}]"),
                            depth + 1,
                            visits,
                            seen_refs,
                        )?;
                    }
                }
            } else {
                for (index, item_value) in array.iter().enumerate() {
                    validate_json_schema_value_inner(
                        item_value,
                        items,
                        root_schema,
                        &format!("{path}[{index}]"),
                        depth + 1,
                        visits,
                        seen_refs,
                    )?;
                }
            }
        }
    }

    Ok(())
}

fn schema_accepts_binary_sentinel(
    value: &Value,
    schema_object: &Map<String, Value>,
    expected_type: &str,
) -> bool {
    expected_type == "string"
        && schema_object.get("format").and_then(Value::as_str) == Some("binary")
        && is_lab_binary_sentinel(value)
}

fn is_lab_binary_sentinel(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.get("__labBinary").and_then(Value::as_str) == Some("base64")
        && object.get("data").and_then(Value::as_str).is_some()
        && matches!(
            object.get("type").and_then(Value::as_str),
            Some("Uint8Array" | "ArrayBuffer")
        )
}

fn invalid_schema_param(path: &str, detail: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("callTool params `{path}` {detail}"),
    }
}
