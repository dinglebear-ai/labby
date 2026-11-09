//! Admission and catchable replies at the untrusted runner tool-call boundary.

use super::*;

pub(super) const OVERSIZED_TOOL_CALL_ID: &str = "code_mode::oversized_tool_id";

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_tool_call_event<'a, H: CodeModeHost>(
    broker: &'a CodeModeBroker<'a, H>,
    seq: u64,
    id: String,
    params: Value,
    full_result: bool,
    execution_start: std::time::Instant,
    cfg: &RunnerConfig,
    deadline: tokio::time::Instant,
    cancellation: &CancellationToken,
    pending_tool_calls: &mut FuturesUnordered<ToolCallFut<'a>>,
    stdin: &mut ChildStdin,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    state: &mut DriveState,
) -> Result<(), CodeModeExecutionError> {
    let is_internal = is_internal_tool_id(&id);
    if is_internal {
        state.internal_calls_enqueued = state.internal_calls_enqueued.saturating_add(1);
    } else {
        state.calls_enqueued = state.calls_enqueued.saturating_add(1);
    }

    if let Err(error) = crate::protocol::check_tool_id_bytes(&id) {
        // Bound errors and rejected trace rows as well as dispatched IDs.
        // Drop the raw ID/params before writeback rather than retain them in a
        // pending future or echo them in the error's tool/message fields.
        drop(id);
        drop(params);
        if !is_internal {
            state.reserve_trace_call(seq, OVERSIZED_TOOL_CALL_ID)?;
            state.record_call((
                seq,
                CodeModeExecutedCall {
                    upstream_content: None,
                    id: OVERSIZED_TOOL_CALL_ID.into(),
                    ok: false,
                    elapsed_ms: 0,
                    start_ms: None,
                    params: None,
                    params_digest: None,
                    error_kind: Some(error.kind.clone()),
                    ui: None,
                },
            ))?;
        }
        return write_runner_input_by_deadline(
            stdin,
            &CodeModeRunnerInput::ToolError {
                seq,
                error: Box::new(error),
            },
            deadline,
            child,
            child_pid,
            &state.calls,
        )
        .await;
    }

    let call_ordinal = if is_internal {
        None
    } else {
        Some(state.reserve_trace_call(seq, &id)?)
    };
    if is_internal && state.internal_calls_enqueued > MAX_INTERNAL_CALLS_PER_RUN {
        if state.internal_calls_enqueued == MAX_INTERNAL_CALLS_PER_RUN.saturating_add(1) {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "codemode",
                kind = "internal_call_budget_exceeded",
                budget = MAX_INTERNAL_CALLS_PER_RUN,
                "Code Mode internal call ceiling exceeded; using discovery fallback or rejecting retrieval"
            );
        }
        // Settle immediately with bounded writeback. Queuing unlimited ready
        // rejection/fallback futures would itself retain caller IDs indefinitely.
        return write_runner_input_by_deadline(
            stdin,
            &internal_call_over_ceiling_response(seq, id, full_result),
            deadline,
            child,
            child_pid,
            &state.calls,
        )
        .await;
    }
    if !is_internal && state.calls_enqueued > state.max_calls_per_run {
        return reject_tool_call_over_budget(
            seq,
            id,
            state.max_calls_per_run,
            stdin,
            child,
            child_pid,
            deadline,
            state,
        )
        .await;
    }

    // Full-result calls cannot enter reserved/local providers or snippet routing.
    // Admit/count first, then reject before any host-visible operation.
    if full_result
        && (is_internal
            || crate::local_provider::try_parse_local_provider_call(&id)
                .ok()
                .flatten()
                .is_some()
            || id.starts_with("snippet::"))
    {
        enqueue_rejected_tool_call(
            seq,
            id,
            params,
            CodeModeCallError::new(
                "invalid_param",
                "callToolResult requires an MCP tool, not a reserved provider or snippet",
            )
            .into_tool_error(),
            cfg,
            pending_tool_calls,
        );
        return Ok(());
    }

    match crate::local_provider::try_parse_local_provider_call(&id) {
        Ok(Some(local)) => enqueue_local_provider_call(
            broker,
            seq,
            call_ordinal,
            id,
            local,
            params,
            execution_start,
            cfg,
            pending_tool_calls,
        ),
        Ok(None) => {
            let tool_deadline =
                external_tool_deadline(tokio::time::Instant::now(), deadline, state.calls_enqueued);
            if tool_deadline < deadline && !state.result_ack_reserve_observed {
                state.result_ack_reserve_observed = true;
                let result_ack_reserve_use_count = record_result_ack_reserve_use();
                tracing::debug!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "codemode.result_ack.reserve",
                    event = "armed",
                    reserve_ms = result_ack_reserve(state.calls_enqueued).as_millis(),
                    result_ack_reserve_use_count,
                    "reserved Code Mode result acknowledgement budget"
                );
            }
            enqueue_tool_call(
                broker,
                seq,
                call_ordinal,
                id,
                params,
                full_result,
                tool_deadline,
                execution_start,
                cfg,
                cancellation,
                pending_tool_calls,
            );
        }
        Err(error) => enqueue_rejected_tool_call(seq, id, params, error, cfg, pending_tool_calls),
    }
    Ok(())
}

/// Only search/describe have a defined fail-open discovery result. Retrieval
/// promises must reject explicitly instead of receiving an unrelated shape.
fn internal_call_over_ceiling_response(
    seq: u64,
    id: String,
    full_result: bool,
) -> CodeModeRunnerInput {
    let value = match (full_result, id.as_str()) {
        (false, "__lab_internal::semantic_rank") => Some(json!({"ranked": []})),
        (false, "__lab_internal::describe_types") => Some(json!({"dts": null})),
        _ => None,
    };
    if let Some(result) = value {
        CodeModeRunnerInput::ToolResult { seq, result }
    } else {
        let error = CodeModeCallError::new(
            "internal_call_budget_exceeded",
            format!("per-run internal call budget of {MAX_INTERNAL_CALLS_PER_RUN} exceeded"),
        )
        .with_tool(id);
        CodeModeRunnerInput::ToolError {
            seq,
            error: Box::new(error),
        }
    }
}

/// Keep admission and completion aligned with the namespace used by dispatch.
pub(super) fn is_internal_tool_id(id: &str) -> bool {
    // Use dispatch's canonical, allocation-free namespace splitter. Parsing is
    // bounded by the byte guard; oversized IDs keep their bounded rejection path.
    if id.len() <= crate::protocol::MAX_TOOL_ID_BYTES {
        crate::types::split_namespaced_id(id)
            .is_some_and(|(namespace, _)| namespace == "__lab_internal")
    } else {
        id.starts_with("__lab_internal::")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_ceiling_fallback_is_limited_to_discovery_contracts() {
        for (id, expected) in [
            ("__lab_internal::semantic_rank", json!({"ranked": []})),
            ("__lab_internal::describe_types", json!({"dts": null})),
        ] {
            assert_eq!(
                internal_call_over_ceiling_response(7, id.into(), false),
                CodeModeRunnerInput::ToolResult {
                    seq: 7,
                    result: expected
                }
            );
        }
        for id in [
            "__lab_internal::read_resource",
            "__lab_internal::read_artifact",
            "__lab_internal::get_prompt",
            "__lab_internal::get_skill",
            "__lab_internal::unknown",
        ] {
            let (seq, error) = match internal_call_over_ceiling_response(7, id.into(), false) {
                CodeModeRunnerInput::ToolError { seq, error } => Some((seq, error)),
                _ => None,
            }
            .expect("retrieval must reject");
            assert_eq!(seq, 7);
            assert_eq!(error.kind(), "internal_call_budget_exceeded");
            assert_eq!(error.origin, crate::CodeModeErrorOrigin::Budget);
            assert_eq!(error.tool.as_deref(), Some(id));
        }
    }
}
