//! Response assembly and stable deadline error construction.

use super::*;

pub(super) fn finalize_done(
    result: super::super::protocol::CodeModeRunnerResult,
    logs: Vec<String>,
    state: &DriveState,
) -> CodeModeExecutionResponse {
    CodeModeExecutionResponse {
        execution_id: None,
        result: result.into_response_result(),
        result_shaping: None,
        ui: None,
        calls: sorted_calls(&state.calls),
        logs,
        artifacts: state.artifacts.clone(),
    }
}

pub(super) fn sorted_calls(calls: &[(u64, CodeModeExecutedCall)]) -> Vec<CodeModeExecutedCall> {
    let mut calls = calls.to_vec();
    calls.sort_by_key(|(seq, _)| *seq);
    calls.into_iter().map(|(_, call)| call).collect()
}

pub(super) fn code_mode_timeout_error(
    calls: &[(u64, CodeModeExecutedCall)],
) -> CodeModeExecutionError {
    CodeModeExecutionError::with_trace(
        ToolError::Sdk {
            sdk_kind: "timeout".to_string(),
            message: "Code Mode execution timed out".to_string(),
        },
        sorted_calls(calls),
    )
}
