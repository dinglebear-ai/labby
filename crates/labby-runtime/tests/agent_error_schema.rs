//! Drift protection binding `build_agent_error_value` output to the published
//! agent-error JSON Schema.
//!
//! The schema file is read as plain JSON data (no schema-validation
//! dependency): the test asserts every `required` field is present and that
//! every emitted enum value for `origin`, `recovery.action`,
//! `recovery.same_arguments`, and `side_effects` is a member of the schema's
//! enum lists — for every kind in the classification tables plus the
//! unknown-kind catch-all.

use std::path::Path;

use labby_runtime::agent_error::{AgentErrorContext, build_agent_error_value};
use serde_json::Value;

/// Every kind named in `origin_for_kind` / `recovery_for_kind`, plus a probe
/// for the unknown-kind catch-all.
const KINDS: &[&str] = &[
    // validation
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
    // skills extension (SEP-2640)
    "skill_digest_mismatch",
    "skill_manifest_stale",
    // policy
    "forbidden",
    "permission_denied",
    "confirmation_required",
    "auth_failed",
    "auth_required",
    "oauth_needs_reauth",
    "route_scope_denied",
    // budget
    "rate_limited",
    "queue_saturated",
    "quota_exceeded",
    "budget_exceeded",
    "call_budget_exceeded",
    "result_too_large",
    "artifact_too_large",
    "response_too_large",
    "snippet_budget_exceeded",
    "snippet_resolve_limit",
    // discovery
    "unknown_action",
    "unknown_subaction",
    "unknown_tool",
    "unknown_upstream",
    "unknown_instance",
    "ambiguous_tool",
    "not_found",
    "snippet_not_found",
    // tool execution
    "tool_error",
    // upstream transport
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
    // bridge
    "bridge_transport_error",
    // runtime / internal
    "internal_error",
    "server_error",
    "decode_error",
    "invalid_provider_output",
    "cancelled",
    // unknown-kind catch-all
    "totally_unknown_kind",
];

fn published_schema() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/contracts/schemas/agent-error.schema.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&raw).expect("agent-error schema is valid JSON")
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

#[test]
fn build_agent_error_value_satisfies_published_schema_for_all_kinds() {
    let schema = published_schema();
    let required = string_list(&schema, "/required");
    let recovery_required = string_list(&schema, "/$defs/recovery/required");
    let origin_enum = string_list(&schema, "/properties/origin/enum");
    let side_effects_enum = string_list(&schema, "/properties/side_effects/enum");
    let action_enum = string_list(&schema, "/$defs/recovery/properties/action/enum");
    let same_arguments_enum =
        string_list(&schema, "/$defs/recovery/properties/same_arguments/enum");

    for kind in KINDS {
        let value =
            build_agent_error_value(kind, "test message", None, &AgentErrorContext::default());
        let object = value.as_object().expect("agent error is an object");

        for field in &required {
            assert!(
                object.contains_key(*field),
                "kind `{kind}`: missing required field `{field}`"
            );
        }
        for field in &recovery_required {
            assert!(
                value["recovery"].get(field).is_some(),
                "kind `{kind}`: missing required recovery field `{field}`"
            );
        }

        let origin = value["origin"].as_str().expect("origin is a string");
        assert!(
            origin_enum.contains(&origin),
            "kind `{kind}`: origin `{origin}` not in schema enum {origin_enum:?}"
        );
        let side_effects = value["side_effects"]
            .as_str()
            .expect("side_effects is a string");
        assert!(
            side_effects_enum.contains(&side_effects),
            "kind `{kind}`: side_effects `{side_effects}` not in schema enum"
        );
        let action = value["recovery"]["action"]
            .as_str()
            .expect("recovery.action is a string");
        assert!(
            action_enum.contains(&action),
            "kind `{kind}`: recovery.action `{action}` not in schema enum"
        );
        let same_arguments = value["recovery"]["same_arguments"]
            .as_str()
            .expect("recovery.same_arguments is a string");
        assert!(
            same_arguments_enum.contains(&same_arguments),
            "kind `{kind}`: recovery.same_arguments `{same_arguments}` not in schema enum"
        );

        assert_eq!(
            value["contract_version"], schema["properties"]["contract_version"]["const"],
            "kind `{kind}`: contract_version drifted from the published const"
        );
    }
}
