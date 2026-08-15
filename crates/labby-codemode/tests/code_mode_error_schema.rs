//! Drift protection binding serialized `CodeModeCallError`s to the published
//! Code Mode call-error JSON Schema, read as plain JSON data.

// `panic!` is how tests assert; `panic = "warn"` targets production paths.
#![allow(clippy::panic)]

use std::path::Path;

use labby_codemode::{CodeModeCallError, CodeModeErrorEvidence, CodeModeToolSafetyHints};
use serde_json::Value;

/// Every kind named in the shared classification tables plus the unknown-kind
/// catch-all (mirrors the list in `labby-runtime/tests/agent_error_schema.rs`).
const KINDS: &[&str] = &[
    "missing_param",
    "invalid_param",
    "validation_failed",
    "invalid_hint",
    "conflict",
    "path_traversal",
    "symlink_rejected",
    "invalid_encoding",
    "ssrf_blocked",
    "content_too_large",
    "relay_invalid_target",
    "invalid_code_mode_id",
    "forbidden",
    "permission_denied",
    "confirmation_required",
    "auth_failed",
    "auth_required",
    "oauth_needs_reauth",
    "route_scope_denied",
    "rate_limited",
    "queue_saturated",
    "quota_exceeded",
    "budget_exceeded",
    "call_budget_exceeded",
    "result_too_large",
    "artifact_too_large",
    "snippet_budget_exceeded",
    "snippet_resolve_limit",
    "unknown_action",
    "unknown_subaction",
    "unknown_tool",
    "unknown_upstream",
    "unknown_instance",
    "ambiguous_tool",
    "not_found",
    "snippet_not_found",
    "tool_error",
    "upstream_error",
    "network_error",
    "timeout",
    "bad_gateway",
    "service_unavailable",
    "provider_unavailable",
    "provider_timeout",
    "not_connected",
    "connection_error",
    "relay_forwarder_init_failed",
    "bridge_transport_error",
    "internal_error",
    "server_error",
    "decode_error",
    "invalid_provider_output",
    "cancelled",
    "totally_unknown_kind",
];

fn published_schema() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/contracts/schemas/code-mode-call-error.schema.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&raw).expect("code-mode-call-error schema is valid JSON")
}

fn string_list<'a>(schema: &'a Value, pointer: &str) -> Vec<&'a str> {
    schema
        .pointer(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("schema array at `{pointer}`"))
        .iter()
        .map(|value| value.as_str().expect("schema list entries are strings"))
        .collect()
}

fn assert_conforms(error: &CodeModeCallError, schema: &Value, label: &str) {
    let value = serde_json::to_value(error).expect("serializable");
    let object = value.as_object().expect("call error is an object");

    for field in string_list(schema, "/required") {
        assert!(
            object.contains_key(field),
            "{label}: missing required field `{field}`"
        );
    }
    for field in string_list(schema, "/$defs/recovery/required") {
        assert!(
            value["recovery"].get(field).is_some(),
            "{label}: missing required recovery field `{field}`"
        );
    }

    let origin = value["origin"].as_str().expect("origin is a string");
    assert!(
        string_list(schema, "/properties/origin/enum").contains(&origin),
        "{label}: origin `{origin}` not in schema enum"
    );
    let side_effects = value["side_effects"]
        .as_str()
        .expect("side_effects is a string");
    assert!(
        string_list(schema, "/properties/side_effects/enum").contains(&side_effects),
        "{label}: side_effects `{side_effects}` not in schema enum"
    );
    let action = value["recovery"]["action"]
        .as_str()
        .expect("recovery.action is a string");
    assert!(
        string_list(schema, "/$defs/recovery/properties/action/enum").contains(&action),
        "{label}: recovery.action `{action}` not in schema enum"
    );
    let same_arguments = value["recovery"]["same_arguments"]
        .as_str()
        .expect("recovery.same_arguments is a string");
    assert!(
        string_list(schema, "/$defs/recovery/properties/same_arguments/enum")
            .contains(&same_arguments),
        "{label}: recovery.same_arguments `{same_arguments}` not in schema enum"
    );
    assert_eq!(
        value["contract_version"], schema["properties"]["contract_version"]["const"],
        "{label}: contract_version drifted from the published const"
    );
}

#[test]
fn serialized_call_errors_satisfy_published_schema_for_all_kinds() {
    let schema = published_schema();
    for kind in KINDS {
        let error = CodeModeCallError::new(*kind, "test message");
        assert_conforms(&error, &schema, &format!("new({kind})"));
    }
}

#[test]
fn constructor_variants_satisfy_published_schema() {
    let schema = published_schema();

    let execution = CodeModeCallError::tool_execution(
        "alpha::demo",
        "tool_error",
        Some("upstream_error".to_string()),
        "Exit code 7",
        CodeModeErrorEvidence::default(),
        CodeModeToolSafetyHints {
            read_only_hint: Some(true),
            ..CodeModeToolSafetyHints::default()
        },
        Some(2000),
    );
    assert_conforms(&execution, &schema, "tool_execution");

    let transport = CodeModeCallError::upstream_transport("alpha::demo", "connection reset");
    assert_conforms(&transport, &schema, "upstream_transport");
}
