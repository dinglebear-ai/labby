//! Admission and catchable replies at the untrusted runner tool-call boundary.

use super::*;

pub(super) const OVERSIZED_TOOL_CALL_ID: &str = "code_mode::oversized_tool_id";

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_tool_call_event<'a, H: CodeModeHost>(
    broker: &'a CodeModeBroker<'a, H>,
    seq: u64,
    id: String,
    params: Value,
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
    let is_internal = id.starts_with("__lab_internal::");
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
                    id: OVERSIZED_TOOL_CALL_ID.into(),
                    ok: false,
                    elapsed_ms: 0,
                    start_ms: None,
                    params: None,
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
            &internal_call_over_ceiling_response(seq, id),
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
fn internal_call_over_ceiling_response(seq: u64, id: String) -> CodeModeRunnerInput {
    let value = match id.as_str() {
        "__lab_internal::semantic_rank" => Some(json!({"ranked": []})),
        "__lab_internal::describe_types" => Some(json!({"dts": null})),
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
                internal_call_over_ceiling_response(7, id.into()),
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
            let (seq, error) = match internal_call_over_ceiling_response(7, id.into()) {
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
