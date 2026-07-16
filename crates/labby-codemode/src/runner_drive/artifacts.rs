//! Artifact and snippet protocol handlers for the runner driver.

use super::*;

pub(super) async fn handle_artifact_write_event(
    seq: u64,
    path: String,
    content: String,
    content_type: Option<String>,
    stdin: &mut ChildStdin,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    deadline: tokio::time::Instant,
    cfg: &RunnerConfig,
    state: &mut DriveState,
) -> Result<(), CodeModeExecutionError> {
    let artifact_root = state.artifact_root.clone();
    let artifact_max_bytes = state.artifact_max_bytes;
    let trace_params = cfg.trace_params;
    let artifact_op = async {
        if !state.artifact_store_pruned {
            super::super::artifacts::prune_artifact_runs(
                super::super::artifacts::artifact_retention_runs(),
            )
            .await;
            state.artifact_store_pruned = true;
        }
        handle_artifact_write(
            stdin,
            &artifact_root,
            &mut state.artifacts,
            &mut state.calls,
            seq,
            CodeModeArtifactWrite {
                path,
                content,
                content_type,
            },
            trace_params,
            artifact_max_bytes,
        )
        .await
    };
    match tokio::time::timeout_at(deadline, artifact_op).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            terminate_code_mode_runner(child, child_pid).await;
            Err(error.into())
        }
        Err(_) => {
            terminate_code_mode_runner(child, child_pid).await;
            Err(code_mode_timeout_error(&state.calls))
        }
    }
}

pub(super) async fn handle_snippet_resolve_event<H: CodeModeHost>(
    broker: &CodeModeBroker<'_, H>,
    seq: u64,
    name: String,
    input: Value,
    stdin: &mut ChildStdin,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    deadline: tokio::time::Instant,
    cfg: &RunnerConfig,
    state: &mut DriveState,
) -> Result<(), CodeModeExecutionError> {
    let op = resolve_snippet_for_runner(broker, &name, input, cfg, state);
    match tokio::time::timeout_at(deadline, op).await {
        Ok(Ok((code, input))) => {
            write_runner_input_by_deadline(
                stdin,
                &CodeModeRunnerInput::SnippetResolved { seq, code, input },
                deadline,
                child,
                child_pid,
                &state.calls,
            )
            .await
        }
        Ok(Err(error)) => {
            write_runner_input_by_deadline(
                stdin,
                &CodeModeRunnerInput::ToolError {
                    seq,
                    kind: error.kind().to_string(),
                    message: error.user_message().to_string(),
                },
                deadline,
                child,
                child_pid,
                &state.calls,
            )
            .await
        }
        Err(_) => {
            terminate_code_mode_runner(child, child_pid).await;
            Err(code_mode_timeout_error(&state.calls))
        }
    }
}

async fn resolve_snippet_for_runner<H: CodeModeHost>(
    broker: &CodeModeBroker<'_, H>,
    name: &str,
    input: Value,
    cfg: &RunnerConfig,
    state: &mut DriveState,
) -> Result<(String, Value), ToolError> {
    if !cfg.caller.can_use_snippets() {
        return Err(ToolError::Forbidden {
            message: "codemode.run requires lab:admin or trusted-local Code Mode".to_string(),
            required_scopes: vec!["lab:admin".to_string()],
        });
    }
    if cfg.capability_filter.is_scoped() {
        return Err(ToolError::Forbidden {
            message: "codemode.run is not available on route-scoped Code Mode surfaces".to_string(),
            required_scopes: vec!["lab:admin".to_string()],
        });
    }
    let Some(host) = broker.host else {
        return Err(ToolError::Sdk {
            sdk_kind: "tool_source_unavailable".to_string(),
            message: "codemode.run requires a live tool source".to_string(),
        });
    };
    if state.snippet_resolves >= MAX_SNIPPET_RESOLVES_PER_RUN {
        return Err(ToolError::Sdk {
            sdk_kind: "snippet_resolve_limit".to_string(),
            message: "snippet resolve limit exceeded".to_string(),
        });
    }
    state.snippet_resolves = state.snippet_resolves.saturating_add(1);
    let started = std::time::Instant::now();
    let resolved = host.resolve_snippet(name, input).await?;
    let (name, code, input) = (resolved.name, resolved.code, resolved.input);
    state.snippet_resolved_bytes = state.snippet_resolved_bytes.saturating_add(code.len());
    if state.snippet_resolved_bytes > MAX_SNIPPET_RESOLVED_BYTES_PER_RUN {
        return Err(ToolError::Sdk {
            sdk_kind: "snippet_budget_exceeded".to_string(),
            message: "resolved snippet code budget exceeded".to_string(),
        });
    }
    tracing::info!(
        surface = "dispatch",
        service = "code_mode",
        action = "snippet.resolve",
        snippet = %name,
        elapsed_ms = started.elapsed().as_millis(),
        "Code Mode snippet resolved"
    );
    Ok((code, input))
}

async fn handle_artifact_write(
    stdin: &mut ChildStdin,
    artifact_root: &Path,
    artifacts: &mut Vec<CodeModeArtifactReceipt>,
    calls: &mut Vec<(u64, CodeModeExecutedCall)>,
    seq: u64,
    request: CodeModeArtifactWrite,
    trace_params: bool,
    max_bytes: usize,
) -> Result<(), ToolError> {
    let started = std::time::Instant::now();
    let redacted_params = artifact_trace_params(&request, trace_params);
    match write_code_mode_artifact(artifact_root, &request, max_bytes).await {
        Ok(receipt) => {
            let result = json!(receipt);
            artifacts.push(receipt);
            calls.push(artifact_call(
                seq,
                true,
                started.elapsed().as_millis(),
                redacted_params,
                None,
            ));
            write_runner_input(stdin, &CodeModeRunnerInput::ToolResult { seq, result }).await
        }
        Err(error) => {
            let kind = error.kind().to_string();
            calls.push(artifact_call(
                seq,
                false,
                started.elapsed().as_millis(),
                redacted_params,
                Some(kind.clone()),
            ));
            write_runner_input(
                stdin,
                &CodeModeRunnerInput::ToolError {
                    seq,
                    kind,
                    message: error.user_message().to_string(),
                },
            )
            .await
        }
    }
}

fn artifact_trace_params(request: &CodeModeArtifactWrite, trace_params: bool) -> Option<Value> {
    super::super::trace::redact_trace_params(
        &json!({ "path": request.path.as_str(), "content_type": request.content_type.as_deref() }),
        trace_params,
    )
}

fn artifact_call(
    seq: u64,
    ok: bool,
    elapsed_ms: u128,
    params: Option<Value>,
    error_kind: Option<String>,
) -> (u64, CodeModeExecutedCall) {
    (
        seq,
        CodeModeExecutedCall {
            id: ARTIFACT_WRITE_CALL_ID.to_string(),
            ok,
            elapsed_ms,
            start_ms: None,
            params,
            error_kind,
            ui: None,
        },
    )
}
