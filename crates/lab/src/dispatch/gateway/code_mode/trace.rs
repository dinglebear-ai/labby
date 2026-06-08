//! Redacted Code Mode trace helpers.
//!
//! Raw tool-call params are only available at the broker boundary. Everything
//! this module returns is safe to place in public response structs, history,
//! structured content, resources, UI state, and tests.

use serde_json::{Map, Value, json};

const REDACTED: &str = "[redacted]";
const TRUNCATED_STRING: &str = "[truncated]";
const MAX_DEPTH: usize = 16;
const MAX_COLLECTION_ITEMS: usize = 64;
const MAX_STRING_CHARS: usize = 512;
const DEFAULT_PARAM_BYTES: usize = 4096;

#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn redact_trace_params(
    params: &Value,
    enabled: bool,
) -> Option<Value> {
    if !enabled {
        return None;
    }
    Some(redact_trace_value(params, DEFAULT_PARAM_BYTES))
}

#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn redact_trace_value(
    value: &Value,
    max_bytes: usize,
) -> Value {
    let redacted = redact_value(value, 0);
    let size = serde_json::to_vec(&redacted)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    if size <= max_bytes {
        return redacted;
    }

    json!({
        "truncated": true,
        "reason": "redacted_params_exceeded_cap",
        "original_size_bytes": size,
        "max_size_bytes": max_bytes,
    })
}

fn redact_value(value: &Value, depth: usize) -> Value {
    if depth >= MAX_DEPTH {
        return json!({
            "truncated": true,
            "reason": "max_depth_exceeded",
        });
    }

    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        Value::String(s) => Value::String(redact_string(s)),
        Value::Array(items) => {
            let mut out = items
                .iter()
                .take(MAX_COLLECTION_ITEMS)
                .map(|item| redact_value(item, depth + 1))
                .collect::<Vec<_>>();
            if items.len() > MAX_COLLECTION_ITEMS {
                out.push(json!({
                    "truncated": true,
                    "reason": "array_item_limit_exceeded",
                    "omitted": items.len() - MAX_COLLECTION_ITEMS,
                }));
            }
            Value::Array(out)
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (idx, (key, child)) in map.iter().enumerate() {
                if idx >= MAX_COLLECTION_ITEMS {
                    out.insert(
                        "_truncated".to_string(),
                        json!({
                            "reason": "object_key_limit_exceeded",
                            "omitted": map.len() - MAX_COLLECTION_ITEMS,
                        }),
                    );
                    break;
                }
                if crate::dispatch::redact::is_sensitive_key(key) {
                    out.insert(key.clone(), Value::String(REDACTED.to_string()));
                } else {
                    out.insert(key.clone(), redact_value(child, depth + 1));
                }
            }
            Value::Object(out)
        }
    }
}

fn redact_string(value: &str) -> String {
    if looks_sensitive_value(value) {
        return REDACTED.to_string();
    }

    let url_redacted = redact_url_like(value);
    truncate_string(&url_redacted)
}

fn redact_url_like(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        return crate::dispatch::redact::redact_url(value);
    }
    value.to_string()
}

fn truncate_string(value: &str) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(MAX_STRING_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!(
            "{prefix}{TRUNCATED_STRING} ({} chars)",
            value.chars().count()
        )
    } else {
        value.to_string()
    }
}

fn looks_sensitive_value(value: &str) -> bool {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();

    lower.starts_with("bearer ")
        || lower.starts_with("basic ")
        || lower.contains("-----begin ")
        || lower.contains("authorization:")
        || lower.contains("cookie:")
        || looks_like_jwt(trimmed)
        || looks_like_sensitive_assignment(trimmed)
        || looks_like_base64_blob(trimmed)
}

fn looks_like_sensitive_assignment(value: &str) -> bool {
    value.lines().any(|line| {
        let trimmed = line.trim();
        let Some((key, _)) = trimmed.split_once('=') else {
            return false;
        };
        crate::dispatch::redact::is_sensitive_key(key.trim_start_matches("--"))
    })
}

fn looks_like_jwt(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| part.len() >= 10 && part.chars().all(is_base64url_char))
}

fn looks_like_base64_blob(value: &str) -> bool {
    value.len() >= 160 && value.chars().all(is_base64ish_char)
}

fn is_base64url_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')
}

fn is_base64ish_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '-' | '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_nested_sensitive_keys_and_values() {
        let raw = json!({
            "query": "matrix",
            "nested": {
                "authorization": "Bearer secret-token",
                "items": [
                    {"api_key": "sk-secret"},
                    "https://user:pass@example.com/path?token=secret&page=2",
                    "OPENAI_API_KEY=sk-secret"
                ]
            }
        });

        let redacted = redact_trace_value(&raw, 4096);
        let serialized = redacted.to_string();

        assert_eq!(redacted["query"], json!("matrix"));
        assert_eq!(redacted["nested"]["authorization"], json!(REDACTED));
        assert_eq!(redacted["nested"]["items"][0]["api_key"], json!(REDACTED));
        assert!(
            serialized.contains("token=[redacted]"),
            "credential URL query token must be redacted: {serialized}"
        );
        assert!(!serialized.contains("secret-token"));
        assert!(!serialized.contains("sk-secret"));
        assert!(!serialized.contains("user:pass"));
    }

    #[test]
    fn redacts_sensitive_key_variants() {
        let raw = json!({
            "token": "a",
            "secret": "b",
            "authorization": "c",
            "password": "d",
            "apikey": "e",
            "api_key": "f",
            "service-key": "g",
            "cookie": "h"
        });

        let redacted = redact_trace_value(&raw, 4096);
        for key in [
            "token",
            "secret",
            "authorization",
            "password",
            "apikey",
            "api_key",
            "service-key",
            "cookie",
        ] {
            assert_eq!(redacted[key], json!(REDACTED), "{key} must be redacted");
        }
    }

    #[test]
    fn caps_long_strings_and_large_objects_deterministically() {
        let long = "x".repeat(MAX_STRING_CHARS + 100);
        let raw = json!({
            "safe": long,
            "many": (0..200).map(|i| json!({ "idx": i })).collect::<Vec<_>>()
        });

        let redacted = redact_trace_value(&raw, 512);
        let serialized = redacted.to_string();
        assert!(
            serialized.len() <= 512,
            "redacted params must respect byte cap, got {} bytes: {serialized}",
            serialized.len()
        );
        assert!(serialized.contains("redacted_params_exceeded_cap"));

        let string_capped = redact_trace_value(
            &json!({"safe": "safe words ".repeat(MAX_STRING_CHARS / 5)}),
            4096,
        );
        assert!(
            string_capped["safe"]
                .as_str()
                .expect("string")
                .contains(TRUNCATED_STRING)
        );
    }

    #[test]
    fn trace_params_can_be_disabled() {
        assert_eq!(
            redact_trace_params(&json!({"token": "secret"}), false),
            None
        );
    }
}
