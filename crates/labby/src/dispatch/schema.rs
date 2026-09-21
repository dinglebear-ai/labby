use schemars::JsonSchema;
use serde_json::Value;

/// Generate the complete JSON Schema for a typed action payload.
#[must_use]
pub fn for_type<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T))
        .expect("schemars-generated schema must serialize to JSON")
}
