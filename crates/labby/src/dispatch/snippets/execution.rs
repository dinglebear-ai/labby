//! Saved execution and scoped, payload-free reproducibility receipts.
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use labby_codemode::MAX_SOURCE_BYTES;
use labby_gateway::codemode_journal::receipts::{
    SnippetExecutionReceipt, SnippetReceiptArtifact, SnippetReceiptCall, SnippetReceiptOwner,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::dispatch::{SnippetDispatchContext, SnippetExecutionOutcome};
use super::store::{
    builtin_snippet_dir, code_for_snippet, merge_snippet_input, resolve_snippet,
    wrap_snippet_with_input_bounded,
};
use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::code_mode::{
    CodeModeBroker, CodeModeCaller, CodeModeSurface, JournalOwner, ToolScope,
};
use crate::dispatch::gateway::manager::GatewayManager;
use crate::dispatch::helpers::lab_home;

fn digest(bytes: &[u8]) -> String {
    format!(
        "sha256:{}",
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}
fn value_digest(value: &Value) -> String {
    digest(&serde_json::to_vec(value).expect("JSON Value serialization is infallible"))
}

pub(super) fn receipt_owner(context: &SnippetDispatchContext) -> SnippetReceiptOwner {
    SnippetReceiptOwner {
        owner_key: value_digest(
            &json!({"actor":context.actor_key,"subject":context.execution_caller.subject(),"trusted_local":matches!(context.execution_caller.without_authority(), CodeModeCaller::TrustedLocal)}),
        ),
        route_scope: context.route_scope.clone(),
        capability_fingerprint: context.capability_filter_fingerprint.clone(),
    }
}

fn receipt_calls(calls: &[labby_codemode::CodeModeExecutedCall]) -> Vec<SnippetReceiptCall> {
    calls
        .iter()
        .take(32)
        .map(|call| SnippetReceiptCall {
            tool: call.id.clone(),
            params_digest: call.params.as_ref().map(value_digest),
            ok: call.ok,
            elapsed_ms: call.elapsed_ms,
            error_kind: call.error_kind.clone(),
        })
        .collect()
}

async fn persist(
    manager: &GatewayManager,
    receipt: &SnippetExecutionReceipt,
    owner: &SnippetReceiptOwner,
) -> String {
    // Execution already has side effects; telemetry failure must never invite replay.
    match tokio::time::timeout(
        std::time::Duration::from_millis(500),
        manager.record_snippet_receipt(receipt.clone(), owner.clone()),
    )
    .await
    {
        Ok(Ok(true)) => "persisted".into(),
        Ok(Ok(false)) => "disabled".into(),
        _ => {
            tracing::warn!(execution_id = %receipt.execution_id, "snippet receipt persistence unavailable; do not replay execution to recover telemetry");
            "unavailable".into()
        }
    }
}

pub(super) fn snippet_execution_scope(
    snippet: &super::store::ResolvedSnippet,
    caller_scope: &ToolScope,
) -> ToolScope {
    snippet
        .tools
        .as_ref()
        .map(|tools| tools.intersect(caller_scope))
        .unwrap_or_else(|| caller_scope.clone())
}

pub(super) async fn execute_snippet_outcome(
    manager: Option<&GatewayManager>,
    name: &str,
    input: Value,
    caller_scope: &ToolScope,
    caller: &CodeModeCaller,
    surface: CodeModeSurface,
    context: Option<&SnippetDispatchContext>,
) -> Result<SnippetExecutionOutcome, ToolError> {
    let owned_manager;
    let manager = if let Some(manager) = manager {
        manager
    } else {
        owned_manager = crate::dispatch::gateway::require_gateway_manager()?;
        owned_manager.as_ref()
    };
    let broker = CodeModeBroker::new(Some(manager));
    let mut config = manager.code_mode_config().await;
    let return_trace_params = config.trace_params;
    // Capture argument digests for the receipt, then restore the response policy.
    config.trace_params = true;
    let snippet = resolve_snippet(&lab_home(), &builtin_snippet_dir(), name)?;
    let code = code_for_snippet(&snippet)?;
    let input = merge_snippet_input(&snippet, input)?;
    let wrapped = wrap_snippet_with_input_bounded(
        &code,
        &input,
        config.max_source_bytes.min(MAX_SOURCE_BYTES),
    )?;
    let scope = snippet_execution_scope(&snippet, caller_scope);
    let mut fallback = SnippetDispatchContext::trusted_local();
    fallback.execution_caller = caller.clone();
    fallback.execution_surface = surface;
    fallback.execution_scope = caller_scope.clone();
    fallback.capability_filter_fingerprint = caller_scope.fingerprint();
    let context = context.unwrap_or(&fallback);
    let owner = receipt_owner(context);
    let execution_id = format!("snippet_{}", uuid::Uuid::new_v4());
    let mut receipt = SnippetExecutionReceipt {
        execution_id: execution_id.clone(),
        snippet_name: name.to_owned(),
        snippet_digest: digest(snippet.body.as_bytes()),
        input_digest: value_digest(&input),
        effective_scope_fingerprint: scope.fingerprint(),
        runtime_version: format!("labby/{} (javy/quickjs)", env!("CARGO_PKG_VERSION")),
        surface: match surface {
            CodeModeSurface::Cli => "cli",
            CodeModeSurface::Mcp => "mcp",
            CodeModeSurface::Api => "api",
        }
        .into(),
        created_at_ms: i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        )
        .unwrap_or(i64::MAX),
        elapsed_ms: 0,
        status: "started".into(),
        error_kind: None,
        result_digest: None,
        result_bytes: None,
        calls: Vec::new(),
        artifacts: Vec::new(),
        tool_calls: 0,
        omitted_calls: 0,
    };
    persist(manager, &receipt, &owner).await;
    let started = Instant::now();
    let _step_buffer_guard = StepBufferGuard {
        manager,
        execution_id: &execution_id,
    };
    let mut outcome = broker
        .execute_with_raw_response(
            &wrapped,
            caller.clone(),
            surface,
            config,
            scope,
            Some(std::sync::Arc::from(execution_id.as_str())),
        )
        .await;
    receipt.elapsed_ms = started.elapsed().as_millis();
    match &mut outcome {
        Ok(outcome) => {
            let response = &mut outcome.raw_response;
            response.execution_id = Some(execution_id.clone());
            outcome.display_response.execution_id = Some(execution_id.clone());
            receipt.status = if response.calls.iter().all(|call| call.ok)
                && response
                    .result
                    .as_ref()
                    .and_then(|value| value.get("ok"))
                    .and_then(Value::as_bool)
                    != Some(false)
            {
                "succeeded"
            } else {
                "failed"
            }
            .into();
            receipt.calls = receipt_calls(&response.calls);
            receipt.tool_calls = response.calls.len();
            receipt.omitted_calls = response.calls.len().saturating_sub(receipt.calls.len());
            if let Some(result) = &response.result {
                let bytes =
                    serde_json::to_vec(result).expect("JSON Value serialization is infallible");
                receipt.result_digest = Some(digest(&bytes));
                receipt.result_bytes = Some(bytes.len());
            }
            for artifact in response.artifacts.iter().take(16) {
                // Receipt fields are private to the kernel; serialize only its safe references.
                let value = serde_json::to_value(artifact).map_err(|_| {
                    ToolError::internal_message("artifact receipt serialization failed")
                })?;
                receipt.artifacts.push(SnippetReceiptArtifact {
                    path: value["path"].as_str().unwrap_or_default().into(),
                    sha256: value["sha256"].as_str().unwrap_or_default().into(),
                    bytes: value["bytes"].as_u64().unwrap_or_default() as usize,
                    content_type: value["content_type"].as_str().unwrap_or_default().into(),
                    storage_run_id: artifact.storage_run_id(),
                });
            }
        }
        Err(error) => {
            receipt.status = "failed".into();
            receipt.error_kind = Some(error.kind().into());
            receipt.calls = receipt_calls(error.calls());
            receipt.tool_calls = error.calls().len();
            receipt.omitted_calls = error.calls().len().saturating_sub(receipt.calls.len());
            tracing::warn!(%execution_id, kind = %error.kind(), "saved snippet failed; receipt identity retained");
        }
    }
    let journal_owner = JournalOwner {
        actor_key: Some(owner.owner_key.clone()),
        route_scope: owner.route_scope.clone(),
        capability_filter_fingerprint: Some(owner.capability_fingerprint.clone()),
    };
    let flush = manager.flush_step_journal(&execution_id, &journal_owner);
    if tokio::time::timeout(std::time::Duration::from_millis(500), flush)
        .await
        .is_err()
    {
        manager.discard_step_buffer(&execution_id);
        tracing::warn!(%execution_id, "snippet journal flush timed out");
    }
    if !return_trace_params && let Ok(outcome) = &mut outcome {
        for call in outcome
            .raw_response
            .calls
            .iter_mut()
            .chain(outcome.display_response.calls.iter_mut())
        {
            call.params = None;
        }
    }
    let receipt_status = persist(manager, &receipt, &owner).await;
    let outcome = outcome.map_err(|error| {
        let mut error = error.into_contract_tool_error();
        if let ToolError::Contract { payload, .. } = &mut error {
            payload
                .extra
                .insert("execution_id".into(), json!(execution_id));
            payload
                .extra
                .insert("receipt_status".into(), json!(receipt_status));
        }
        error
    })?;
    Ok(SnippetExecutionOutcome {
        raw_response: outcome.raw_response,
        display_response: outcome.display_response,
        receipt_status,
    })
}

struct StepBufferGuard<'a> {
    manager: &'a GatewayManager,
    execution_id: &'a str,
}
impl Drop for StepBufferGuard<'_> {
    fn drop(&mut self) {
        self.manager.discard_step_buffer(self.execution_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owners_separate_authenticated_subjects_and_scopes() {
        let mut context = SnippetDispatchContext::trusted_local();
        let local = receipt_owner(&context);
        context.route_scope = "team-a".into();
        assert_ne!(local.route_scope, receipt_owner(&context).route_scope);
        context.execution_caller = CodeModeCaller::Scoped {
            sub: Some("alice".into()),
            capabilities: Default::default(),
        };
        let alice = receipt_owner(&context).owner_key;
        context.execution_caller = CodeModeCaller::Scoped {
            sub: Some("bob".into()),
            capabilities: Default::default(),
        };
        assert_ne!(alice, receipt_owner(&context).owner_key);
    }
}
