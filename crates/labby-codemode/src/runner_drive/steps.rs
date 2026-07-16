//! Durable-step protocol handlers and per-execution journal budgets.

use super::*;

const MAX_STEPS_PER_RUN: u64 = 256;
const MAX_STEP_VALUE_BYTES: usize = 256 * 1024;
const MAX_STEP_VALUE_BYTES_PER_RUN: usize = 8 * 1024 * 1024;

impl DriveState {
    fn allocate_step(&mut self, seq: u64, name: String) -> Result<u64, ToolError> {
        if self.next_step_ordinal >= MAX_STEPS_PER_RUN {
            return Err(ToolError::Sdk {
                sdk_kind: "budget_exceeded".to_string(),
                message: format!("Code Mode step count exceeds {MAX_STEPS_PER_RUN} per run"),
            });
        }
        let ordinal = self.next_step_ordinal;
        self.next_step_ordinal += 1;
        self.step_ordinals.insert(seq, (ordinal, name));
        Ok(ordinal)
    }

    fn complete_step(&mut self, seq: u64, value: &Value) -> Result<(u64, String), ToolError> {
        let pair = self
            .step_ordinals
            .remove(&seq)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("Code Mode step_result {seq} has no matching step_begin"),
            })?;
        let bytes = serde_json::to_vec(value)
            .map_err(|error| ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: format!("Code Mode step result is not serializable: {error}"),
            })?
            .len();
        if bytes > MAX_STEP_VALUE_BYTES {
            return Err(ToolError::Sdk {
                sdk_kind: "budget_exceeded".to_string(),
                message: format!(
                    "Code Mode step result is {bytes} bytes; maximum is {MAX_STEP_VALUE_BYTES}"
                ),
            });
        }
        let aggregate = self.step_value_bytes.saturating_add(bytes);
        if aggregate > MAX_STEP_VALUE_BYTES_PER_RUN {
            return Err(ToolError::Sdk {
                sdk_kind: "budget_exceeded".to_string(),
                message: format!(
                    "Code Mode step journal exceeds {MAX_STEP_VALUE_BYTES_PER_RUN} bytes per run"
                ),
            });
        }
        self.step_value_bytes = aggregate;
        Ok(pair)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_step_begin_event<H: CodeModeHost>(
    broker: &CodeModeBroker<'_, H>,
    seq: u64,
    name: String,
    execution_id: Option<Arc<str>>,
    stdin: &mut ChildStdin,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    deadline: tokio::time::Instant,
    state: &mut DriveState,
) -> Result<(), CodeModeExecutionError> {
    let step_ordinal = match state.allocate_step(seq, name.clone()) {
        Ok(ordinal) => ordinal,
        Err(error) => {
            let reply = CodeModeRunnerInput::ToolError {
                seq,
                kind: error.kind().to_string(),
                message: error.user_message().to_string(),
            };
            return write_runner_input_by_deadline(
                stdin,
                &reply,
                deadline,
                child,
                child_pid,
                &state.calls,
            )
            .await;
        }
    };
    let ctx = ExecCtx {
        seq,
        execution_id,
        step_ordinal: Some(step_ordinal),
    };
    let decision = match broker.host {
        Some(host) => host.decide_step(ctx, &name).await,
        None => StepDecision::Execute,
    };
    let reply = match decision {
        StepDecision::Replay(value) => {
            state.step_ordinals.remove(&seq);
            CodeModeRunnerInput::StepDecision {
                seq,
                replay: Some(value),
            }
        }
        StepDecision::Execute => CodeModeRunnerInput::StepDecision { seq, replay: None },
        StepDecision::Error { kind, message } => {
            state.step_ordinals.remove(&seq);
            CodeModeRunnerInput::ToolError { seq, kind, message }
        }
    };
    write_runner_input_by_deadline(stdin, &reply, deadline, child, child_pid, &state.calls).await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_step_result_event<H: CodeModeHost>(
    broker: &CodeModeBroker<'_, H>,
    seq: u64,
    value: Value,
    execution_id: Option<Arc<str>>,
    stdin: &mut ChildStdin,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    deadline: tokio::time::Instant,
    state: &mut DriveState,
) -> Result<(), CodeModeExecutionError> {
    let record = match state.complete_step(seq, &value) {
        Ok((step_ordinal, name)) => {
            let ctx = ExecCtx {
                seq,
                execution_id,
                step_ordinal: Some(step_ordinal),
            };
            match broker.host {
                Some(host) => host.record_step(ctx, &name, &value).await,
                None => Ok(()),
            }
        }
        Err(error) => Err(error),
    };
    let reply = match record {
        Ok(()) => CodeModeRunnerInput::StepRecorded { seq },
        Err(error) => CodeModeRunnerInput::ToolError {
            seq,
            kind: error.kind().to_string(),
            message: error.user_message().to_string(),
        },
    };
    write_runner_input_by_deadline(stdin, &reply, deadline, child, child_pid, &state.calls).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_count_is_hard_bounded() {
        let mut state = DriveState::new("budget-count");
        for seq in 0..MAX_STEPS_PER_RUN {
            state
                .allocate_step(seq, format!("step-{seq}"))
                .expect("within budget");
            state.step_ordinals.remove(&seq);
        }
        let error = state
            .allocate_step(MAX_STEPS_PER_RUN, "overflow".into())
            .unwrap_err();
        assert_eq!(error.kind(), "budget_exceeded");
    }

    #[test]
    fn step_results_release_ordinals_and_enforce_payload_budget() {
        let mut state = DriveState::new("budget-bytes");
        state.allocate_step(7, "small".into()).unwrap();
        state
            .complete_step(7, &serde_json::json!({"ok": true}))
            .unwrap();
        assert!(state.step_ordinals.is_empty());

        state.allocate_step(8, "huge".into()).unwrap();
        let huge = Value::String("x".repeat(MAX_STEP_VALUE_BYTES));
        let error = state.complete_step(8, &huge).unwrap_err();
        assert_eq!(error.kind(), "budget_exceeded");
        assert!(state.step_ordinals.is_empty());
    }
}
