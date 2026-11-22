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
    "internal_call_budget_exceeded",
    "trace_budget_exceeded",
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

#[test]
fn native_host_budget_kinds_keep_their_contract_through_dispatch_conversion() {
    for kind in ["internal_call_budget_exceeded", "trace_budget_exceeded"] {
        let error = CodeModeCallError::new(kind, "host budget exhausted");
        assert_eq!(error.origin, labby_codemode::CodeModeErrorOrigin::Budget);
        assert_eq!(
            error.recovery.action,
            labby_codemode::CodeModeRecoveryAction::ReduceWork
        );
        assert_eq!(
            error.side_effects,
            labby_codemode::CodeModeSideEffectRisk::Possible
        );
        let restored = CodeModeCallError::from(error.clone().into_contract_tool_error());
        assert_eq!(restored, error);
        assert_conforms(&restored, &published_schema(), kind);
    }
}

fn assert_conforms(error: &CodeModeCallError, schema: &Value, label: &str) {
    let value = serde_json::to_value(error).expect("serializable");
    let validator = jsonschema::validator_for(schema).expect("valid published error schema");
    if let Err(error) = validator.validate(&value) {
        panic!("{label}: {error}");
    }
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

#[test]
fn reloading_host_defaults_replaces_and_clears_budget_values() {
    use labby_codemode::{
        effective_storage_limits, install_artifact_config_defaults,
        install_call_budget_config_defaults,
    };
    for value in [3, 7] {
        install_call_budget_config_defaults(Some(value), Some(value as usize));
        install_artifact_config_defaults(Some(value as usize), Some(value as usize), Some(value));
        assert_eq!(
            effective_storage_limits(),
            serde_json::json!({
                "max_calls_per_run": value,
                "calltool_result_max_bytes": value * 1024 * 1024,
                "artifact_retention_runs": value,
                "artifact_max_bytes": value * 1024 * 1024,
                "artifact_max_store_bytes": value * 1024 * 1024,
            })
        );
    }
    install_call_budget_config_defaults(None, None);
    install_artifact_config_defaults(None, None, None);
    assert_eq!(effective_storage_limits()["max_calls_per_run"], 512);
}
