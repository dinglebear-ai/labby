use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn reject_tool_call_over_budget(
    seq: u64,
    id: String,
    budget: u64,
    stdin: &RunnerReplies,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    deadline: tokio::time::Instant,
    state: &mut DriveState,
) -> Result<(), CodeModeExecutionError> {
    if state.calls_enqueued == budget.saturating_add(1) {
        tracing::warn!(
            surface = "dispatch",
            service = "code_mode",
            action = "codemode",
            kind = "call_budget_exceeded",
            budget,
            "Code Mode run exceeded the per-run callTool fan-out budget; rejecting further calls"
        );
    }
    write_runner_input_by_deadline(
        stdin,
        &CodeModeRunnerInput::ToolError {
            seq,
            error: Box::new(
                CodeModeCallError::new(
                    "call_budget_exceeded",
                    format!(
                        "per-run callTool budget of {budget} exceeded; reduce fan-out or split the work across multiple codemode calls"
                    ),
                )
                .with_tool(id.clone()),
            ),
        },
        deadline,
        child,
        child_pid,
        &state.calls,
    )
    .await?;
    state.record_call((
        seq,
        CodeModeExecutedCall {
            upstream_content: None,
            id,
            ok: false,
            elapsed_ms: 0,
            start_ms: None,
            params: None,
            params_digest: None,
            error_kind: Some("call_budget_exceeded".to_string()),
            ui: None,
        },
    ))?;
    Ok(())
}

/// Enqueue a bounded reply without blocking reads from the child.
pub(super) async fn write_runner_input_by_deadline(
    stdin: &RunnerReplies,
    input: &CodeModeRunnerInput,
    deadline: tokio::time::Instant,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    calls: &[(u64, CodeModeExecutedCall)],
) -> Result<(), CodeModeExecutionError> {
    if tokio::time::Instant::now() >= deadline {
        terminate_code_mode_runner(child, child_pid).await;
        return Err(code_mode_timeout_error(calls));
    }
    stdin
        .enqueue(input)
        .map_err(|err| CodeModeExecutionError::with_trace(err, sorted_calls(calls)))
}

/// Handle a completed tool-call future from `pending_tool_calls`.
pub(super) async fn handle_completed_tool_call(
    completed: Option<(
        u64,
        String,
        Option<Value>,
        Option<String>,
        Result<PreparedOutcome, CodeModeCallError>,
        u128,
        u128,
    )>,
    stdin: &RunnerReplies,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    deadline: tokio::time::Instant,
    state: &mut DriveState,
) -> Result<(), CodeModeExecutionError> {
    let Some((seq, id, params, params_digest, result, elapsed_ms, start_ms)) = completed else {
        return Ok(());
    };
    state.release_pending_params(seq);
    // Reserved host-internal pseudo-tool calls never appear in the call
    // trace (`state.calls`) — but their ToolResult/ToolError responses are
    // still written back unconditionally so the sandbox's `callTool(...)`
    // promise settles normally.
    let is_internal = protocol_boundary::is_internal_tool_id(&id);
    match result {
        Ok(PreparedOutcome {
            outcome,
            serialized_len,
            receipt,
        }) => {
            let upstream_content = outcome.full_result.as_ref().and_then(|full| {
                crate::upstream_content::capture(full, &mut state.upstream_content_bytes)
            });
            let ui = outcome.ui;
            if let Some(receipt) = receipt.as_ref() {
                state.artifacts.push(receipt.clone());
            }
            if serialized_len > state.calltool_result_max_bytes {
                let max = state.calltool_result_max_bytes;
                let recovery = receipt.as_ref().and_then(|r| r.artifact_id.as_ref()).map(|artifact_id| format!(" Complete response saved; use codemode.readArtifact({artifact_id:?}) in a later run.")).unwrap_or_else(|| " Complete response was not saved (access policy, artifact size limit, or storage failure).".into());

                write_runner_input_by_deadline(
                    stdin,
                    &CodeModeRunnerInput::ToolError {
                        seq,
                        error: Box::new(
                            CodeModeCallError::new(
                                "result_too_large",
                                format!(
                                    "callTool result is {serialized_len} bytes; maximum is {max} bytes {recovery}"
                                ),
                            )
                            .with_tool(id.clone()),
                        ),
                    },
                    deadline,
                    child,
                    child_pid,
                    &state.calls,
                )
                .await?;
                if !is_internal {
                    state.record_call((
                        seq,
                        CodeModeExecutedCall {
                            upstream_content: upstream_content.clone(),
                            id,
                            ok: false,
                            elapsed_ms,
                            start_ms: Some(start_ms),
                            params,
                            params_digest,
                            error_kind: Some("result_too_large".to_string()),
                            ui,
                        },
                    ))?;
                }
                return Ok(());
            }
            if !is_internal {
                state.record_call((
                    seq,
                    CodeModeExecutedCall {
                        upstream_content,
                        id,
                        ok: true,
                        elapsed_ms,
                        start_ms: Some(start_ms),
                        params,
                        params_digest,
                        error_kind: None,
                        ui,
                    },
                ))?;
            }
            write_runner_input_by_deadline(
                stdin,
                &CodeModeRunnerInput::ToolResult {
                    seq,
                    result: outcome.value,
                },
                deadline,
                child,
                child_pid,
                &state.calls,
            )
            .await?;
        }
        Err(err) => {
            // Catchable tool errors (Cloudflare parity): a single failed
            // callTool must NOT abort the run. Reject the in-sandbox promise
            // with the complete structured contract so caller JavaScript can
            // inspect evidence, recovery guidance, and side-effect risk. If
            // uncaught, the runner returns the same object to the parent.
            let error = err.with_tool(id.clone());
            let kind = error.kind.clone();
            // This error settles the seq's promise in-sandbox; do NOT also send
            // a ToolResult for the same seq.
            write_runner_input_by_deadline(
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
            .await?;
            if !is_internal {
                state.record_call((
                    seq,
                    CodeModeExecutedCall {
                        upstream_content: None,
                        id,
                        ok: false,
                        elapsed_ms,
                        start_ms: Some(start_ms),
                        params,
                        params_digest,
                        error_kind: Some(kind),
                        ui: None,
                    },
                ))?;
            }
        }
    }
    Ok(())
}
