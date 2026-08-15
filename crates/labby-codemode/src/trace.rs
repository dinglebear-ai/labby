//! Redacted Code Mode trace helpers.
//!
//! Raw tool-call params are only available at the broker boundary. Everything
//! this module returns is safe to place in public response structs, history,
//! structured content, resources, UI state, and tests.

use serde_json::{Map, Value, json};

use super::types::{CodeModeExecutionResponse, split_code_mode_call_id};

const DEFAULT_PARAM_BYTES: usize = 4096;

/// Canonical JSON-tree redaction now lives in `labby_runtime::redact` (the
/// crate's redaction charter home). Re-exported here so hosts (e.g. the
/// durable Code Mode pause store in the `labby` binary crate) keep their
/// existing `labby_codemode::redact_trace_value` import path.
pub use labby_runtime::redact::redact_trace_value;

#[must_use]
pub(crate) fn redact_trace_params(params: &Value, enabled: bool) -> Option<Value> {
    if !enabled {
        return None;
    }
    Some(redact_trace_value(params, DEFAULT_PARAM_BYTES))
}

/// Build the structured-content trace for a Code Mode result.
///
/// The trace carries the **actual** `result` verbatim — not just its shape —
/// because most MCP clients (Claude Code included) surface `structuredContent`
/// over the text content block. Emitting only a `result_shape` here means a
/// structured-content-preferring client never sees the value the model asked
/// for. This matches Cloudflare's Code Mode, where the executed function's
/// return value is surfaced verbatim and bounded only by the response budget
/// (`packages/codemode/src/mcp.ts::truncateResponse`).
///
/// `response.result` is already capped to the response budget
/// (`max_response_bytes` / `max_response_tokens`) by `truncate_execution_response`
/// before it reaches here, so it is embedded as-is rather than run through the
/// per-string `redact_trace_value` cap (which would truncate a valid answer to
/// 512 chars). The secret-bearing channel is per-call `params` — those remain
/// redacted below. `result_shape` is retained as a cheap descriptor the inline
/// UI app and tooling read.
#[must_use]
pub fn code_mode_execute_trace(response: &CodeModeExecutionResponse) -> Value {
    let calls = response
        .calls
        .iter()
        .map(|call| {
            let (namespace, tool) = split_code_mode_call_id(&call.id);
            let mut entry = Map::from_iter([
                ("id".to_string(), json!(call.id)),
                ("namespace".to_string(), json!(namespace)),
                ("tool".to_string(), json!(tool)),
                ("ok".to_string(), json!(call.ok)),
                ("elapsed_ms".to_string(), json!(call.elapsed_ms)),
            ]);
            if let Some(start_ms) = call.start_ms {
                entry.insert("start_ms".to_string(), json!(start_ms));
            }
            if let Some(params) = &call.params {
                entry.insert("params".to_string(), params.clone());
            }
            if let Some(error_kind) = &call.error_kind {
                entry.insert("error_kind".to_string(), json!(error_kind));
            }
            if let Some(ui) = &call.ui {
                entry.insert("ui".to_string(), ui.ui_meta.clone());
            }
            Value::Object(entry)
        })
        .collect::<Vec<_>>();

    let mut trace = Map::new();
    trace.insert("kind".to_string(), json!("code_mode_execute_trace"));
    trace.insert("call_count".to_string(), json!(response.calls.len()));
    trace.insert("calls".to_string(), json!(calls));
    // Embed the real return value. Omit the field entirely when the function
    // returned `undefined` (mirrors `CodeModeExecutionResponse::result`'s
    // skip-if-none serialization); an explicit JS `null` is `Some(Value::Null)`
    // and is preserved as `"result": null`.
    if let Some(result) = &response.result {
        trace.insert("result".to_string(), result.clone());
    }
    trace.insert(
        "result_shape".to_string(),
        response
            .result
            .as_ref()
            .map(compact_result_shape)
            .unwrap_or_else(|| json!({ "type": "undefined" })),
    );
    if let Some(shape) = &response.result_shaping {
        trace.insert(
            "result_shaping".to_string(),
            serde_json::to_value(shape).unwrap_or_else(|_| json!({ "type": "unknown" })),
        );
    }
    // Surface artifact receipts so a structured-content-only client can follow
    // the "write large payloads to an artifact and read them back" path.
    if !response.artifacts.is_empty() {
        trace.insert(
            "artifacts".to_string(),
            Value::Array(
                response
                    .artifacts
                    .iter()
                    .map(|artifact| {
                        json!({
                            "path": artifact.path.as_str(),
                            "content_type": artifact.content_type.as_str(),
                            "bytes": artifact.bytes,
                            "sha256": artifact.sha256.as_str(),
                        })
                    })
                    .collect(),
            ),
        );
    }
    trace.insert("logs_count".to_string(), json!(response.logs.len()));
    Value::Object(trace)
}

#[must_use]
pub(crate) fn compact_result_shape(value: &Value) -> Value {
    let size_bytes = serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    match value {
        Value::Null => json!({ "type": "null", "size_bytes": size_bytes }),
        Value::Bool(_) => json!({ "type": "boolean", "size_bytes": size_bytes }),
        Value::Number(_) => json!({ "type": "number", "size_bytes": size_bytes }),
        Value::String(s) => json!({
            "type": "string",
            "size_bytes": size_bytes,
            "length": s.chars().count(),
        }),
        Value::Array(items) => json!({
            "type": "array",
            "size_bytes": size_bytes,
            "length": items.len(),
            "item_types": compact_item_types(items),
        }),
        Value::Object(object) => {
            let mut keys = object.keys().take(16).cloned().collect::<Vec<_>>();
            keys.sort();
            json!({
                "type": "object",
                "size_bytes": size_bytes,
                "keys": keys,
                "key_count": object.len(),
                "truncated": object.get("truncated").and_then(Value::as_bool).unwrap_or(false),
                "content_block_kinds": content_block_kinds(value),
            })
        }
    }
}

fn compact_item_types(items: &[Value]) -> Vec<&'static str> {
    let mut types = items.iter().take(16).map(value_type).collect::<Vec<_>>();
    types.sort_unstable();
    types.dedup();
    types
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn content_block_kinds(value: &Value) -> Vec<String> {
    value
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(16)
        .filter_map(|block| {
            block
                .get("type")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::CodeModeArtifactReceipt;
    use crate::types::{CodeModeExecutedCall, UiLink};
    use serde_json::json;

    #[test]
    fn trace_params_can_be_disabled() {
        assert_eq!(
            redact_trace_params(&json!({"token": "secret"}), false),
            None
        );
    }

    #[test]
    fn execute_trace_omits_artifact_absolute_paths() {
        let trace = code_mode_execute_trace(&CodeModeExecutionResponse {
            execution_id: None,
            result: None,
            result_shaping: None,
            ui: None,
            calls: Vec::new(),
            logs: Vec::new(),
            artifacts: vec![CodeModeArtifactReceipt {
                path: "reports/result.md".to_string(),
                absolute_path: "/home/jmagar/.labby/code-mode-artifacts/run/reports/result.md"
                    .to_string(),
                content_type: "text/markdown".to_string(),
                bytes: 42,
                sha256: "abc123".to_string(),
            }],
        });

        assert_eq!(trace["artifacts"][0]["path"], json!("reports/result.md"));
        assert_eq!(trace["artifacts"][0]["bytes"], json!(42));
        assert!(trace["artifacts"][0].get("absolute_path").is_none());
    }

    #[test]
    fn execute_trace_preserves_per_call_mcp_ui_resource() {
        let trace = code_mode_execute_trace(&CodeModeExecutionResponse {
            execution_id: None,
            result: None,
            result_shaping: None,
            ui: None,
            calls: vec![CodeModeExecutedCall {
                id: "quick-shell::run_command".to_string(),
                ok: true,
                elapsed_ms: 12,
                start_ms: Some(4),
                params: None,
                error_kind: None,
                ui: Some(UiLink {
                    ui_meta: json!({
                        "resourceUri": "ui://quick-shell/app.html",
                        "preferredSize": { "height": 420 },
                    }),
                }),
            }],
            logs: Vec::new(),
            artifacts: Vec::new(),
        });

        assert_eq!(
            trace["calls"][0]["ui"]["resourceUri"],
            json!("ui://quick-shell/app.html")
        );
        assert_eq!(
            trace["calls"][0]["ui"]["preferredSize"]["height"],
            json!(420)
        );
    }
}
