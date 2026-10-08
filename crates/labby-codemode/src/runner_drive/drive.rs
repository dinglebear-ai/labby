use super::*;

impl<H: CodeModeHost> CodeModeBroker<'_, H> {
    /// Drive the Start → tool-call/artifact → Done/Error protocol loop against a
    /// single runner. Returns a [`DriveOutcome`] classifying both the result and
    /// whether the runner is safe to reuse.
    pub(super) async fn drive_runner(
        &self,
        runner: &mut PooledRunner,
        cfg: &RunnerConfig,
        deadline: tokio::time::Instant,
    ) -> DriveOutcome {
        // Record the stderr buffer position before this execution so we capture
        // only the lines this run produces (a pooled runner's buffer carries
        // prior executions' lines).
        let stderr = runner.stderr.clone();
        let stderr_start = stderr.mark().await;

        let start = CodeModeRunnerInput::Start {
            code: cfg.code_to_run.clone(),
            proxy: cfg.proxy.clone(),
        };
        if let Err(err) =
            tokio::time::timeout_at(deadline, write_runner_input(&mut runner.stdin, &start))
                .await
                .map_err(|_| ToolError::Sdk {
                    sdk_kind: "timeout".to_string(),
                    message: "Code Mode execution timed out".to_string(),
                })
                .and_then(|result| result)
        {
            // Failed to even send Start — the runner is suspect; evict it.
            return DriveOutcome::RunnerUnhealthy(err.into());
        }

        let cancellation = CancellationToken::new();
        let _cancel_execution_on_drop = CancelExecutionOnDrop(cancellation.clone());
        let mut settlement_watch: Option<SettlementWatch> = None;
        // Epoch for per-call start offsets (waterfall timing in the trace).
        let execution_start = std::time::Instant::now();
        let artifact_run_id = &cfg.artifact_run_id;
        let mut state = DriveState::new(artifact_run_id);
        let mut saw_protocol_activity = false;
        // Mark this run active before any artifact dir exists, so a concurrent
        // run's first-write prune can never delete our directory mid-run. The
        // RAII guard clears the id on every exit path (including early returns).
        let _active_artifact_run = ActiveArtifactRun::register(artifact_run_id);

        // pending_tool_calls lives here (not in DriveState) so its lifetime is
        // tied to this async fn rather than being forced to 'static, allowing
        // futures to capture `self` (a non-'static reference) without error.
        let mut pending_tool_calls: FuturesUnordered<ToolCallFut<'_>> = FuturesUnordered::new();

        // Borrow the runner's components for the loop. The protocol loop owns
        // these references for its duration; the runner is parked afterwards.
        let child = &mut runner.child;
        let child_pid = runner.child_pid;
        let stdin = &mut runner.stdin;
        let lines = &mut runner.lines;

        loop {
            let read_deadline = settlement_watch.map_or(deadline, |watch| watch.deadline);
            tokio::select! {
                line = tokio::time::timeout_at(read_deadline, lines.next()) => {
                    let line = match line {
                        Ok(line) => line,
                        Err(_) => {
                            // Cancel every in-flight host future before waiting
                            // for runner teardown. Dropping the futures alone is
                            // not enough evidence that cancellation propagated.
                            cancellation.cancel();
                            let settlement_grace_timed_out = settlement_watch
                                .is_some_and(|watch| watch.grace_limited)
                                && pending_tool_calls.is_empty();
                            terminate_code_mode_runner(child, child_pid).await;
                            let error = if settlement_grace_timed_out {
                                let settlement_watchdog_expiry_count =
                                    record_settlement_watchdog_expiry();
                                tracing::warn!(
                                    surface = "dispatch",
                                    service = "code_mode",
                                    action = "codemode.settlement",
                                    event = "watchdog_expired",
                                    kind = "runner_settlement_timeout",
                                    call_count = state.calls.len(),
                                    grace_ms = RUNNER_SETTLEMENT_GRACE.as_millis(),
                                    settlement_watchdog_expiry_count,
                                    "Code Mode runner failed to settle after all tool calls completed"
                                );
                                CodeModeExecutionError::with_trace(
                                    ToolError::Sdk {
                                        sdk_kind: "timeout".to_string(),
                                        message: format!(
                                            "Code Mode runner did not settle within {}ms after all tool calls completed",
                                            RUNNER_SETTLEMENT_GRACE.as_millis()
                                        ),
                                    },
                                    sorted_calls(&state.calls),
                                )
                            } else {
                                code_mode_timeout_error(&state.calls)
                            };
                            return DriveOutcome::RunnerUnhealthy(error);
                        }
                    };
                    // `FramedRead::next()` yields `Option<Result<String, LinesCodecError>>`.
                    // `None` = EOF (runner crashed/exited); `Some(Err(_))` = I/O or line-too-long.
                    let Some(line_result) = line else {
                        // EOF: the runner process died unexpectedly. Surface a
                        // clean error and evict so a replacement spawns.
                        let status = child.wait().await;
                        stderr.flush_settle().await;
                        let diagnostics = stderr.take_since_and_clear(stderr_start).await;
                        let diagnostics = labby_runtime::redact::sanitize_error_text(
                            &diagnostics.join("\n"),
                            512,
                        );
                        let message = if diagnostics.is_empty() {
                            format!("Code Mode runner exited before completion ({status:?})")
                        } else {
                            format!(
                                "Code Mode runner exited before completion ({status:?}): {diagnostics}"
                            )
                        };
                        let error = CodeModeExecutionError::with_trace(
                            ToolError::Sdk {
                                sdk_kind: "server_error".to_string(),
                                message,
                            },
                            sorted_calls(&state.calls),
                        );
                        return if saw_protocol_activity {
                            DriveOutcome::RunnerUnhealthy(error)
                        } else {
                            DriveOutcome::RunnerUnavailableBeforeActivity(error)
                        };
                    };
                    let line = match classify_line_result(line_result) {
                        Ok(line) => line,
                        Err(err) => {
                            terminate_code_mode_runner(child, child_pid).await;
                            return DriveOutcome::RunnerUnhealthy(
                                CodeModeExecutionError::with_trace(err, sorted_calls(&state.calls)),
                            );
                        }
                    };

                    // Any new protocol activity ends the post-tool settlement
                    // watch. A fresh watch is armed when the pending call set
                    // becomes empty again.
                    settlement_watch = None;
                    let msg = match serde_json::from_str::<CodeModeRunnerOutput>(&line) {
                        Ok(msg) => msg,
                        Err(err) => {
                            terminate_code_mode_runner(child, child_pid).await;
                            return DriveOutcome::RunnerUnhealthy(
                                CodeModeExecutionError::with_trace(
                                    ToolError::Sdk {
                                        sdk_kind: "internal_error".to_string(),
                                        message: format!(
                                            "Code Mode runner emitted invalid protocol JSON: {err}"
                                        ),
                                    },
                                    sorted_calls(&state.calls),
                                ),
                            );
                        }
                    };
                    saw_protocol_activity = true;

                    match msg {
                        CodeModeRunnerOutput::ToolCall { seq, id, params } => {
                            // Reserved host-internal pseudo-tool calls (see
                            // `execute.rs`'s `LAB_INTERNAL_NAMESPACE`) are
                            // exempt from the ordinary per-run call budget and
                            // the call trace — but NOT from dispatch routing:
                            // they still flow through the normal enqueue
                            // path below so their promise settles normally.
                            // They ARE metered separately: past
                            // `MAX_INTERNAL_CALLS_PER_RUN` each internal call
                            // settles fail-open with an empty "nothing found"
                            // result shaped for whichever internal tool it is
                            // (see `enqueue_internal_call_over_ceiling`) so
                            // sandbox JS cannot loop them into unbounded
                            // host round trips.
                            let is_internal = id.starts_with("__lab_internal::");
                            let call_ordinal = if is_internal {
                                state.internal_calls_enqueued =
                                    state.internal_calls_enqueued.saturating_add(1);
                                None
                            } else {
                                state.calls_enqueued = state.calls_enqueued.saturating_add(1);
                                Some(state.calls_enqueued.saturating_sub(1))
                            };
                            if is_internal
                                && state.internal_calls_enqueued > MAX_INTERNAL_CALLS_PER_RUN
                            {
                                // Warn once per run, on the first over-ceiling
                                // call only.
                                if state.internal_calls_enqueued
                                    == MAX_INTERNAL_CALLS_PER_RUN.saturating_add(1)
                                {
                                    tracing::warn!(
                                        surface = "dispatch",
                                        service = "code_mode",
                                        action = "codemode",
                                        kind = "internal_call_budget_exceeded",
                                        budget = MAX_INTERNAL_CALLS_PER_RUN,
                                        "Code Mode run exceeded the internal pseudo-tool call ceiling; settling further internal calls with the fail-open empty result"
                                    );
                                }
                                enqueue_internal_call_over_ceiling(
                                    seq,
                                    id,
                                    params,
                                    cfg,
                                    &mut pending_tool_calls,
                                );
                            } else if !is_internal && state.calls_enqueued > state.max_calls_per_run
                            {
                                if let Err(err) = reject_tool_call_over_budget(
                                    seq,
                                    id,
                                    state.max_calls_per_run,
                                    stdin,
                                    child,
                                    child_pid,
                                    deadline,
                                    &mut state,
                                )
                                .await
                                {
                                    return DriveOutcome::RunnerUnhealthy(err);
                                }
                            } else {
                                match crate::local_provider::try_parse_local_provider_call(&id) {
                                    Ok(Some(local)) => {
                                        enqueue_local_provider_call(
                                            self,
                                            seq,
                                            call_ordinal,
                                            id,
                                            local,
                                            params,
                                            execution_start,
                                            cfg,
                                            &mut pending_tool_calls,
                                        );
                                    }
                                    Ok(None) => {
                                        let now = tokio::time::Instant::now();
                                        let tool_deadline = external_tool_deadline(
                                            now,
                                            deadline,
                                            state.calls_enqueued,
                                        );
                                        if tool_deadline < deadline
                                            && !state.result_ack_reserve_observed
                                        {
                                            state.result_ack_reserve_observed = true;
                                            let result_ack_reserve_use_count =
                                                record_result_ack_reserve_use();
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
                                            self,
                                            seq,
                                            call_ordinal,
                                            id,
                                            params,
                                            tool_deadline,
                                            execution_start,
                                            cfg,
                                            &cancellation,
                                            &mut pending_tool_calls,
                                        );
                                    }
                                    Err(err) => {
                                        enqueue_rejected_tool_call(
                                            seq,
                                            id,
                                            params,
                                            err,
                                            cfg,
                                            &mut pending_tool_calls,
                                        );
                                    }
                                }
                            }
                        }
                        CodeModeRunnerOutput::ArtifactWrite {
                            seq,
                            path,
                            content,
                            content_type,
                        } => {
                            if let Err(err) = handle_artifact_write_event(
                                seq,
                                path,
                                content,
                                content_type,
                                stdin,
                                child,
                                child_pid,
                                deadline,
                                cfg,
                                &mut state,
                            )
                            .await
                            {
                                return DriveOutcome::RunnerUnhealthy(err);
                            }
                        }
                        CodeModeRunnerOutput::SnippetResolve { seq, name, input } => {
                            if let Err(err) = handle_snippet_resolve_event(
                                self,
                                seq,
                                name,
                                input,
                                stdin,
                                child,
                                child_pid,
                                deadline,
                                cfg,
                                &mut state,
                            )
                            .await
                            {
                                return DriveOutcome::RunnerUnhealthy(err);
                            }
                        }
                        CodeModeRunnerOutput::StepBegin { seq, name } => {
                            if let Err(err) = handle_step_begin_event(
                                self,
                                seq,
                                name,
                                cfg.execution_id.clone(),
                                stdin,
                                child,
                                child_pid,
                                deadline,
                                &mut state,
                            )
                            .await
                            {
                                return DriveOutcome::RunnerUnhealthy(err);
                            }
                        }
                        CodeModeRunnerOutput::StepResult { seq, value } => {
                            if let Err(err) = handle_step_result_event(
                                self,
                                seq,
                                value,
                                cfg.execution_id.clone(),
                                stdin,
                                child,
                                child_pid,
                                deadline,
                                &mut state,
                            )
                            .await
                            {
                                return DriveOutcome::RunnerUnhealthy(err);
                            }
                        }
                        CodeModeRunnerOutput::Done { result, logs } => {
                            // Preserve original invariant: Done with in-flight
                            // tool calls is a protocol error → evict.
                            if !pending_tool_calls.is_empty() {
                                terminate_code_mode_runner(child, child_pid).await;
                                return DriveOutcome::RunnerUnhealthy(
                                    CodeModeExecutionError::with_trace(
                                        ToolError::Sdk {
                                            sdk_kind: "internal_error".to_string(),
                                            message:
                                                "Code Mode runner completed with pending tool calls"
                                                    .to_string(),
                                        },
                                        sorted_calls(&state.calls),
                                    ),
                                );
                            }
                            let response = finalize_done(result, logs, &state);
                            // Capture only this execution's stderr lines. The
                            // runner is parked (it loops), so do not wait on it;
                            // give the drain a brief window to flush console
                            // output emitted before Done.
                            stderr.flush_settle().await;
                            let mut all_logs = response.logs.clone();
                            all_logs.extend(stderr.take_since_and_clear(stderr_start).await);
                            let all_logs = apply_log_caps(
                                all_logs,
                                cfg.max_log_entries,
                                cfg.max_log_bytes,
                            );
                            let sanitized_logs = all_logs
                                .into_iter()
                                .map(|line| crate::truncate::sanitize_log_text(&line, 4096))
                                .collect();
                            return DriveOutcome::Completed(CodeModeExecutionResponse {
                                logs: sanitized_logs,
                                ..response
                            });
                        }
                        CodeModeRunnerOutput::Error { error } => {
                            // A per-execution error. The runner reset and parked
                            // (it does NOT exit), so it is safe to reuse — return
                            // ExecutionError so the pool releases rather than
                            // evicts. The complete structured error survives an
                            // uncaught JavaScript rejection unchanged.
                            stderr.flush_settle().await;
                            stderr.clear().await;
                            return DriveOutcome::ExecutionError(
                                CodeModeExecutionError::with_trace(
                                    *error,
                                    sorted_calls(&state.calls),
                                ),
                            );
                        }
                    }
                }
                completed = pending_tool_calls.next(),
                    if !pending_tool_calls.is_empty() =>
                {
                    if let Err(err) = handle_completed_tool_call(
                        completed, stdin, child, child_pid, deadline, &mut state, &cfg,
                    )
                    .await
                    {
                        // Failed to relay a tool result back to the runner (pipe
                        // error or write-deadline expiry) — the runner is killed
                        // on the deadline path; evict so a replacement spawns.
                        cancellation.cancel();
                        return DriveOutcome::RunnerUnhealthy(err);
                    }
                    if pending_tool_calls.is_empty()
                        && (state.calls_enqueued > 0 || state.internal_calls_enqueued > 0)
                    {
                        let now = tokio::time::Instant::now();
                        let watch = SettlementWatch::new(now, deadline);
                        let available_ms = watch
                            .deadline
                            .checked_duration_since(now)
                            .unwrap_or_default()
                            .as_millis();
                        settlement_watch = Some(watch);
                        tracing::debug!(
                            surface = "dispatch",
                            service = "code_mode",
                            action = "codemode.settlement",
                            call_count = state.calls.len(),
                            grace_ms = RUNNER_SETTLEMENT_GRACE.as_millis(),
                            available_ms,
                            grace_limited = watch.grace_limited,
                            "all Code Mode tool calls settled; awaiting runner completion"
                        );
                    }
                }
            }
        }
    }
}
