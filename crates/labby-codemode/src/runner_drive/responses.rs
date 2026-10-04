use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn reject_tool_call_over_budget(
    seq: u64,
    id: String,
    budget: u64,
    stdin: &mut ChildStdin,
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
    state.calls.push((
        seq,
        CodeModeExecutedCall {
            id,
            ok: false,
            elapsed_ms: 0,
            start_ms: None,
            params: None,
            error_kind: Some("call_budget_exceeded".to_string()),
            ui: None,
        },
    ));
    Ok(())
}

/// Write a message back to the runner bounded by the execution deadline.
///
/// `write_runner_input`'s bare `write_all` + `flush` can block indefinitely if
/// the child stops draining its stdin while the parent is mid-write — the classic
/// two-pipe deadlock (child flooding stdout, which the parent isn't reading while
/// it's blocked writing a large `ToolResult` to stdin). The read side of the loop
/// is already guarded by `timeout_at(deadline, lines.next())`; without this, the
/// parent→child writeback path was the one reachable *in-loop* `await` the 30 s
/// wall-clock backstop did not cover (the pre-deadline `Start` write is excluded:
/// it runs before the deadline exists and against a freshly-parked child that
/// cannot yet be flooding stdout), so a deadlocked child could hang the drive
/// loop and leak the pool slot forever. On expiry we kill the child (killpg) so
/// the pooled slot respawns, mirroring the read-timeout path, and surface the
/// stable `timeout` kind — carrying the partial call trace like the other
/// timeout paths. A plain write I/O error (not a timeout) propagates without a
/// trace, matching the pre-existing bare-write behavior.
pub(super) async fn write_runner_input_by_deadline(
    stdin: &mut ChildStdin,
    input: &CodeModeRunnerInput,
    deadline: tokio::time::Instant,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    calls: &[(u64, CodeModeExecutedCall)],
) -> Result<(), CodeModeExecutionError> {
    match tokio::time::timeout_at(deadline, write_runner_input(stdin, input)).await {
        Ok(result) => result.map_err(Into::into),
        Err(_) => {
            terminate_code_mode_runner(child, child_pid).await;
            Err(code_mode_timeout_error(calls))
        }
    }
}

/// Handle a completed tool-call future from `pending_tool_calls`.
pub(super) async fn handle_completed_tool_call(
    completed: Option<(
        u64,
        String,
        Option<Value>,
        Result<ToolCallOutcome, CodeModeCallError>,
        u128,
        u128,
    )>,
    stdin: &mut ChildStdin,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    deadline: tokio::time::Instant,
    state: &mut DriveState,
    cfg: &RunnerConfig,
) -> Result<(), CodeModeExecutionError> {
    let Some((seq, id, params, result, elapsed_ms, start_ms)) = completed else {
        return Ok(());
    };
    // Reserved host-internal pseudo-tool calls never appear in the call
    // trace (`state.calls`) — but their ToolResult/ToolError responses are
    // still written back unconditionally so the sandbox's `callTool(...)`
    // promise settles normally.
    let is_internal = id.starts_with("__lab_internal::");
    match result {
        Ok(outcome) => {
            let serialized_len = serde_json::to_vec(&outcome.value)
                .map(|v| v.len())
                .unwrap_or(0);
            let ui = outcome.ui;
            let receipt =
                if !is_internal && serialized_len > crate::response_artifacts::inline_threshold() {
                    tokio::time::timeout_at(
                        deadline,
                        crate::response_artifacts::preserve(
                            &state.artifact_root,
                            format!("automatic/tool-{seq}.json"),
                            &outcome.value,
                            &cfg.caller,
                            &cfg.capability_filter,
                        ),
                    )
                    .await
                    .ok()
                    .flatten()
                } else {
                    None
                };
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
                    state.calls.push((
                        seq,
                        CodeModeExecutedCall {
                            id,
                            ok: false,
                            elapsed_ms,
                            start_ms: Some(start_ms),
                            params,
                            error_kind: Some("result_too_large".to_string()),
                            ui,
                        },
                    ));
                }
                return Ok(());
            }
            if !is_internal {
                state.calls.push((
                    seq,
                    CodeModeExecutedCall {
                        id,
                        ok: true,
                        elapsed_ms,
                        start_ms: Some(start_ms),
                        params,
                        error_kind: None,
                        ui,
                    },
                ));
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
                state.calls.push((
                    seq,
                    CodeModeExecutedCall {
                        id,
                        ok: false,
                        elapsed_ms,
                        start_ms: Some(start_ms),
                        params,
                        error_kind: Some(kind),
                        ui: None,
                    },
                ));
            }
        }
    }
    Ok(())
}
