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

mod evaluate;

use std::collections::{BTreeSet, HashMap};

use evaluate::validate_json_schema_value_inner;

use serde_json::Value;

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

/// A work allowance shared across validations; exhausted allowances fail closed.
pub(crate) struct SchemaValidationBudget {
    visits: usize,
    limit: usize,
    deadline: Option<std::time::Instant>,
    patterns: HashMap<String, std::sync::Arc<jsonschema::Validator>>,
}

impl SchemaValidationBudget {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            visits: 0,
            deadline: None,
            patterns: HashMap::new(),
            limit: limit.min(MAX_SCHEMA_VISITS),
        }
    }
    pub(crate) fn until(mut self, deadline: std::time::Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    pub(crate) fn visit(&mut self) -> bool {
        self.visits = self.visits.saturating_add(1);
        !self.is_exhausted()
    }

    pub(crate) fn pattern(
        &mut self,
        text: &str,
    ) -> Result<std::sync::Arc<jsonschema::Validator>, &'static str> {
        if text.len() > 4096 {
            return Err("pattern exceeds the schema pattern length limit");
        }
        if let Some(cached) = self.patterns.get(text) {
            return Ok(cached.clone());
        }
        if self.patterns.len() >= 64 {
            return Err("schema exceeds the distinct pattern limit");
        }
        let options = jsonschema::PatternOptions::fancy_regex()
            .backtrack_limit(10_000)
            .size_limit(256 * 1024)
            .dfa_size_limit(256 * 1024);
        let validator = jsonschema::options()
            .with_pattern_options(options)
            .build(&serde_json::json!({"pattern":text}))
            .map_err(|_| "has an invalid or oversized pattern in inputSchema")?;
        let validator = std::sync::Arc::new(validator);
        self.patterns.insert(text.to_owned(), validator.clone());
        Ok(validator)
    }

    pub(crate) fn is_exhausted(&self) -> bool {
        self.visits > self.limit
            || self
                .deadline
                .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    }
}

fn pattern_matches(
    validator: &jsonschema::Validator,
    value: &Value,
    path: &str,
) -> Result<bool, SchemaCheck> {
    match validator.validate(value) {
        Ok(()) => Ok(true),
        Err(error)
            if matches!(
                error.kind(),
                jsonschema::error::ValidationErrorKind::Pattern { .. }
            ) =>
        {
            Ok(false)
        }
        Err(_) => Err(defect(
            path,
            "exceeds pattern matching work limit in inputSchema",
        )),
    }
}

pub(crate) fn validate_json_schema_value_with_budget(
    value: &Value,
    schema: &Value,
    budget: &mut SchemaValidationBudget,
) -> Result<(), ToolError> {
    validate_json_schema_value_inner(
        value,
        schema,
        schema,
        "params",
        0,
        budget,
        &mut BTreeSet::new(),
    )
    .map_err(SchemaCheck::into_tool_error)
}

fn validate_json_schema_value(value: &Value, schema: &Value, path: &str) -> Result<(), ToolError> {
    let mut seen_refs = BTreeSet::new();
    let mut budget = SchemaValidationBudget::new(MAX_SCHEMA_VISITS);
    validate_json_schema_value_inner(value, schema, schema, path, 0, &mut budget, &mut seen_refs)
        .map_err(SchemaCheck::into_tool_error)
}

fn invalid_schema_param(path: &str, detail: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("callTool params `{path}` {detail}"),
    }
}
