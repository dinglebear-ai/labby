//! Native framed IO and reply consumption; caller JS cannot acknowledge frames.

use super::*;
use std::io::{BufRead, Write};

pub(super) fn runner_emit(output: CodeModeRunnerOutput) -> Result<(), String> {
    RUNNER_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state
            .as_mut()
            .ok_or_else(|| "runner state is not initialized".to_string())?;
        state.replies.record_request(&output);
        serde_json::to_writer(&mut state.writer, &output).map_err(|err| err.to_string())?;
        state
            .writer
            .write_all(b"\n")
            .map_err(|err| err.to_string())?;
        state.writer.flush().map_err(|err| err.to_string())
    })
}

/// EOF on an idle runner is clean shutdown; EOF during execution is a fault.
pub(super) enum RunnerReadError {
    InputClosed,
    Other(String),
}

impl RunnerReadError {
    pub(super) fn into_runner_error(self) -> CodeModeRunnerError {
        match self {
            Self::InputClosed => "runner input closed".to_string().into(),
            Self::Other(message) => message.into(),
        }
    }
}

pub(super) fn runner_read_input() -> Result<CodeModeRunnerInput, RunnerReadError> {
    RUNNER_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state
            .as_mut()
            .ok_or_else(|| RunnerReadError::Other("runner state is not initialized".to_string()))?;
        let mut line = String::new();
        let read = state
            .reader
            .read_line(&mut line)
            .map_err(|err| RunnerReadError::Other(err.to_string()))?;
        if read == 0 {
            return Err(RunnerReadError::InputClosed);
        }
        let input =
            serde_json::from_str(&line).map_err(|err| RunnerReadError::Other(err.to_string()))?;
        Ok(input)
    })
}

pub(super) fn runner_read_reply() -> Result<CodeModeRunnerInput, RunnerReadError> {
    let input = runner_read_input()?;
    // Record actual native consumption BEFORE JS settlement can emit the next
    // StepResult using the same seq as its just-acknowledged StepBegin. Start is
    // valid only at admission; receiving it here disqualifies this execution.
    RUNNER_STATE.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.replies.record_reply(&input);
        }
    });
    Ok(input)
}
