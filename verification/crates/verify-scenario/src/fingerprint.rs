use serde_json::{Value, json};

use crate::Scenario;

/// BLAKE3 identity over canonical JSON of schema, project, model, invariant,
/// initial state, steps and expectation. Provenance, discovery bounds, status,
/// and the supplied fingerprint do not affect replay identity. Array order does.
/// This does not normalize opaque IDs or interleavings; the runner does that
/// with an opt-in target hook before sealing normalized scenarios.
pub fn content_fingerprint(scenario: &Scenario) -> String {
    let value = json!({
        "schema": scenario.schema, "project": scenario.project,
        "model": scenario.model, "invariant": scenario.invariant,
        "initial": scenario.initial, "steps": scenario.steps, "expect": scenario.expect,
    });
    let mut bytes = Vec::new();
    canonical(&value, &mut bytes);
    format!("b3:{}", blake3::hash(&bytes).to_hex())
}

fn canonical(value: &Value, output: &mut Vec<u8>) {
    match value {
        Value::Object(map) => {
            output.push(b'{');
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                output.extend(serde_json::to_vec(key).expect("JSON string serialization"));
                output.push(b':');
                canonical(value, output);
            }
            output.push(b'}');
        }
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                canonical(value, output);
            }
            output.push(b']');
        }
        _ => output.extend(serde_json::to_vec(value).expect("JSON value serialization")),
    }
}
