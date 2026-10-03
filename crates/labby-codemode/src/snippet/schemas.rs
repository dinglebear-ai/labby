//! Bounded, offline tool contracts and deterministic synthetic response generation.
use crate::error::ToolError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Saved tool contracts; no live catalog is consulted during fixture execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixtureSchemas {
    /// SHA-256 of the saved input and output contracts, for drift detection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// Optional input contract used to check actual calls, not subset match rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    /// Optional response contract used to check synthetic successful responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

impl FixtureSchemas {
    /// Stable fingerprint excluding this metadata field. Object key order is irrelevant.
    #[must_use]
    pub fn contract_fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        fn canonical(value: &Value) -> Value {
            match value {
                Value::Object(map) => {
                    let sorted = map.iter().collect::<std::collections::BTreeMap<_, _>>();
                    Value::Object(
                        sorted
                            .into_iter()
                            .map(|(key, value)| (key.clone(), canonical(value)))
                            .collect(),
                    )
                }
                Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
                other => other.clone(),
            }
        }
        let contracts = canonical(
            &json!({"input_schema":self.input_schema,"output_schema":self.output_schema}),
        );
        let bytes = contracts.to_string();
        format!("sha256:{}", hex::encode(Sha256::digest(bytes.as_bytes())))
    }
}

/// Whether to populate optional fields and an example array item.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FixtureVariant {
    /// Required fields and minimum array lengths only.
    Minimal,
    /// Include optional fields and one array item when permitted.
    #[default]
    Populated,
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: "schemas".into(),
    }
}

/// Reject assertions outside the runtime validator's supported subset instead of
/// presenting partial validation as a complete contract check.
pub fn check_schema(schema: &Value) -> Result<(), ToolError> {
    fn walk(
        schema: &Value,
        root: &Value,
        depth: usize,
        visits: &mut usize,
        references: &mut std::collections::BTreeSet<String>,
    ) -> Result<(), ToolError> {
        *visits += 1;
        if depth > 32 || *visits > 4096 {
            return Err(invalid("fixture schema exceeds nesting or work limit"));
        }
        if schema.is_boolean() {
            return Ok(());
        }
        let object = schema
            .as_object()
            .ok_or_else(|| invalid("schema must be an object or boolean"))?;
        for (key, value) in object {
            match key.as_str() {
                "$schema" | "$id" | "title" | "description" | "default" | "examples"
                | "deprecated" | "readOnly" | "writeOnly" | "$comment" => {}
                "$ref" => {
                    let reference = value
                        .as_str()
                        .filter(|s| *s == "#" || s.starts_with("#/"))
                        .ok_or_else(|| {
                            invalid("fixture schemas support only local JSON Pointer references")
                        })?;
                    let target = root
                        .pointer(&reference[1..])
                        .ok_or_else(|| invalid("unresolved fixture schema reference"))?;
                    if !references.insert(reference.to_owned()) {
                        return Err(invalid("cyclic fixture schema reference"));
                    }
                    walk(target, root, depth + 1, visits, references)?;
                    references.remove(reference);
                }
                "type" => {
                    let types = match value {
                        Value::Array(v) => v.clone(),
                        _ => vec![value.clone()],
                    };
                    if types.is_empty()
                        || types.iter().any(|v| {
                            !v.as_str().is_some_and(|s| {
                                [
                                    "object", "array", "string", "integer", "number", "boolean",
                                    "null",
                                ]
                                .contains(&s)
                            })
                        })
                    {
                        return Err(invalid("unsupported schema type"));
                    }
                }
                "properties" | "$defs" | "definitions" | "patternProperties" => {
                    let fields = value
                        .as_object()
                        .ok_or_else(|| invalid("schema field map must be an object"))?;
                    for (name, child) in fields {
                        if key == "patternProperties" {
                            regex::Regex::new(name)
                                .map_err(|_| invalid("invalid patternProperties schema pattern"))?;
                        }
                        walk(child, root, depth + 1, visits, references)?;
                    }
                }
                "items" | "additionalProperties" => {
                    walk(value, root, depth + 1, visits, references)?
                }
                "anyOf" | "oneOf" | "allOf" => {
                    let branches = value
                        .as_array()
                        .filter(|v| !v.is_empty())
                        .ok_or_else(|| invalid("schema composition must be a nonempty array"))?;
                    for child in branches {
                        walk(child, root, depth + 1, visits, references)?;
                    }
                }
                "required" => {
                    if !value
                        .as_array()
                        .is_some_and(|v| v.iter().all(Value::is_string))
                    {
                        return Err(invalid("required must contain property names"));
                    }
                }
                "enum" => {
                    if !value.as_array().is_some_and(|v| !v.is_empty()) {
                        return Err(invalid("enum must be nonempty"));
                    }
                }
                "const" => {}
                "minimum" | "maximum" => {
                    if !value.is_number() {
                        return Err(invalid("schema numeric bound must be a number"));
                    }
                }
                "minLength" | "maxLength" | "minItems" | "maxItems" => {
                    if value.as_u64().is_none() {
                        return Err(invalid("schema size bound must be a nonnegative integer"));
                    }
                }
                "pattern" => {
                    let pattern = value
                        .as_str()
                        .ok_or_else(|| invalid("schema pattern must be a string"))?;
                    regex::Regex::new(pattern).map_err(|_| invalid("invalid schema pattern"))?;
                }
                "uniqueItems" | "nullable" => {
                    if !value.is_boolean() {
                        return Err(invalid("schema flag must be boolean"));
                    }
                }
                _ => {
                    return Err(invalid(format!(
                        "unsupported fixture schema keyword: {key}"
                    )));
                }
            }
        }
        Ok(())
    }
    walk(
        schema,
        schema,
        0,
        &mut 0,
        &mut std::collections::BTreeSet::new(),
    )
}

/// Validate values using the same assertion engine as production tool arguments.
pub fn validate_value(value: &Value, schema: &Value) -> Result<(), ToolError> {
    check_schema(schema)?;
    validate_value_with_budget(value, schema, &mut validation_budget())
}

/// Shared work allowance for all successful responses or captured arguments in a fixture.
/// This is smaller than live argument admission, where each call has its own allowance.
pub(super) fn validation_budget() -> crate::schema::SchemaValidationBudget {
    crate::schema::SchemaValidationBudget::new(16_384)
}

/// The caller must structurally check the schema before using a shared allowance.
pub(super) fn validate_value_with_budget(
    value: &Value,
    schema: &Value,
    budget: &mut crate::schema::SchemaValidationBudget,
) -> Result<(), ToolError> {
    crate::schema::validate_json_schema_value_with_budget(value, schema, budget)
}

/// Generate synthetic values without copying schema defaults or example payloads.
/// Complex constraints that cannot be satisfied require an explicit override.
pub fn generate_response(schema: &Value, variant: FixtureVariant) -> Result<Value, ToolError> {
    check_schema(schema)?;
    fn sample(
        schema: &Value,
        root: &Value,
        variant: FixtureVariant,
        depth: usize,
        visits: &mut usize,
    ) -> Result<Value, ToolError> {
        *visits += 1;
        if depth > 32 || *visits > 4096 {
            return Err(invalid("response generation exceeds nesting or work limit"));
        }
        if schema == &Value::Bool(false) {
            return Err(invalid("false schema has no valid response"));
        }
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            let target = root
                .pointer(&reference[1..])
                .ok_or_else(|| invalid("unresolved response schema reference"))?;
            return sample(target, root, variant, depth + 1, visits);
        }
        if let Some(value) = schema.get("const") {
            return Ok(value.clone());
        }
        if let Some(values) = schema.get("enum").and_then(Value::as_array) {
            return Ok(values[0].clone());
        }
        if let Some(branches) = schema
            .get("anyOf")
            .or_else(|| schema.get("oneOf"))
            .and_then(Value::as_array)
        {
            for branch in branches {
                if let Ok(value) = sample(branch, root, variant, depth + 1, visits) {
                    return Ok(value);
                }
            }
            return Err(invalid(
                "cannot generate schema union; supply a response override",
            ));
        }
        if schema.get("allOf").is_some() {
            return Err(invalid(
                "allOf response generation requires an explicit override",
            ));
        }
        let kind = schema
            .get("type")
            .and_then(|v| {
                v.as_str()
                    .or_else(|| v.as_array()?.iter().find_map(Value::as_str))
            })
            .unwrap_or(if schema.get("properties").is_some() {
                "object"
            } else if schema.get("items").is_some() {
                "array"
            } else {
                "null"
            });
        match kind {
            "object" => {
                let mut result = Map::new();
                let required: Vec<&str> = schema
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|v| v.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                    let mut keys: Vec<_> = properties.keys().collect();
                    keys.sort();
                    for key in keys {
                        if matches!(variant, FixtureVariant::Populated)
                            || required.contains(&key.as_str())
                        {
                            result.insert(
                                key.clone(),
                                sample(&properties[key], root, variant, depth + 1, visits)?,
                            );
                        }
                    }
                }
                for key in required {
                    if !result.contains_key(key) {
                        return Err(invalid(
                            "required property lacks a schema; supply an override",
                        ));
                    }
                }
                Ok(Value::Object(result))
            }
            "array" => {
                let minimum = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0);
                let maximum = schema.get("maxItems").and_then(Value::as_u64).unwrap_or(64);
                let count = minimum
                    .max(u64::from(matches!(variant, FixtureVariant::Populated)))
                    .min(maximum);
                if minimum > maximum || count > 64 {
                    return Err(invalid(
                        "array response exceeds generation limit; supply an override",
                    ));
                }
                let fallback = Value::Bool(true);
                let items = schema.get("items").unwrap_or(&fallback);
                let mut result = Vec::new();
                for _ in 0..count {
                    result.push(sample(items, root, variant, depth + 1, visits)?);
                }
                Ok(Value::Array(result))
            }
            "string" => {
                let min = schema.get("minLength").and_then(Value::as_u64).unwrap_or(0);
                let max = schema
                    .get("maxLength")
                    .and_then(Value::as_u64)
                    .unwrap_or(1024);
                if min > max || min > 1024 {
                    return Err(invalid(
                        "string response exceeds generation limit; supply an override",
                    ));
                }
                let mut value = "synthetic"
                    .chars()
                    .take(max.min(1024) as usize)
                    .collect::<String>();
                while value.len() < min as usize {
                    value.push('x');
                }
                Ok(Value::String(value))
            }
            "integer" | "number" => {
                let min = schema
                    .get("minimum")
                    .and_then(Value::as_f64)
                    .unwrap_or(-f64::MAX);
                let max = schema
                    .get("maximum")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::MAX);
                let (lower, upper) = if kind == "integer" {
                    (min.ceil(), max.floor())
                } else {
                    (min, max)
                };
                if lower > upper {
                    return Err(invalid("numeric schema has no generated value"));
                }
                let number = 0.0_f64.max(lower).min(upper);
                if kind == "integer" {
                    // Keep generated integers exact across the JavaScript JSON boundary.
                    if !(-9_007_199_254_740_991.0..=9_007_199_254_740_991.0).contains(&number) {
                        return Err(invalid("integer response exceeds generation range"));
                    }
                    Ok(json!(number as i64))
                } else {
                    Ok(json!(number))
                }
            }
            "boolean" => Ok(Value::Bool(true)),
            _ => Ok(Value::Null),
        }
    }
    let value = sample(schema, schema, variant, 0, &mut 0)?;
    validate_value(&value, schema).map_err(|_| {
        invalid("generated response does not satisfy the schema; supply a response override")
    })?;
    Ok(value)
}

#[cfg(test)]
mod tests;
