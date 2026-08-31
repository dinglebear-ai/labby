use std::collections::BTreeMap;

use serde_json::Value;

/// The semantic fields that may participate in a cross-transport comparison.
/// Transport envelopes, request IDs, timestamps, and presentation text are
/// intentionally absent. Adding a field requires an explicit review of its
/// product meaning rather than silently widening normalization.
pub(crate) const SEMANTIC_ALLOWLIST: &[&str] = &[
    "action",
    "destructive",
    "kind",
    "name",
    "requires_admin",
    "recovery",
    "service",
    "side_effects",
    "status",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SemanticObservation(pub(crate) BTreeMap<String, Value>);

pub(crate) fn normalize(value: &Value) -> SemanticObservation {
    let source = value
        .get("error")
        .or_else(|| value.get("result"))
        .unwrap_or(value);
    let mut normalized = BTreeMap::new();
    for field in SEMANTIC_ALLOWLIST {
        if let Some(value) = source.get(field) {
            normalized.insert((*field).to_owned(), value.clone());
        }
    }
    SemanticObservation(normalized)
}

pub(crate) fn assert_equivalent(left: &Value, right: &Value) {
    assert_eq!(normalize(left), normalize(right));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn golden_normalization_keeps_policy_semantics_and_drops_envelopes() {
        let cli = json!({
            "request_id":"cli-random",
            "error":{"kind":"not_found","recovery":"create it","side_effects":"none",
                     "requires_admin":false,"destructive":false,"message":"pretty CLI"}
        });
        let api = json!({
            "correlation_id":"api-random",
            "error":{"kind":"not_found","recovery":"create it","side_effects":"none",
                     "requires_admin":false,"destructive":false,"message":"HTTP wording"}
        });
        assert_equivalent(&cli, &api);
        assert_eq!(normalize(&cli).0.len(), 5);
    }

    #[test]
    fn recovery_and_policy_fields_cannot_be_silently_ignored() {
        let baseline = json!({"error":{"kind":"denied","recovery":"ask owner","side_effects":"none","requires_admin":true,"destructive":false}});
        for field in ["recovery", "side_effects", "requires_admin", "destructive"] {
            let mut changed = baseline.clone();
            changed["error"][field] = Value::Null;
            assert_ne!(normalize(&baseline), normalize(&changed), "lost {field}");
        }
    }
}
