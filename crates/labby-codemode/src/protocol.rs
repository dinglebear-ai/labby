//! Code Mode runner stdio protocol types, shared runner state, and tuning consts.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::RefCell;
use std::io::{self, BufReader, BufWriter};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CodeModeRunnerInput {
    Start {
        code: String,
        /// Auto-generated `var codemode = {...}` proxy JS (see
        /// `code_mode_preamble::generate_js_proxy`). Injected into the sandbox
        /// after `callTool` is defined so the user code can call
        /// `codemode.<namespace>.<tool>(params)`.
        ///
        /// `#[serde(default)]` keeps the search path and older Start messages
        /// (which carry only `code`) forward-compatible — they deserialize to
        /// an empty proxy, leaving `codemode` undefined exactly as before.
        #[serde(default)]
        proxy: String,
        /// Internal child-side deadline budget. The parent remains authoritative
        /// and kills the subprocess on wall-clock expiry; this lets Wasmtime
        /// fuel/epoch deadlines use the same per-run timeout instead of a
        /// hardcoded child default. Older Start messages deserialize to `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    ToolResult {
        seq: u64,
        result: Value,
    },
    SnippetResolved {
        seq: u64,
        code: String,
        input: Value,
    },
    ToolError {
        seq: u64,
        kind: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CodeModeRunnerOutput {
    ToolCall {
        seq: u64,
        id: String,
        params: Value,
    },
    /// The sandbox called `writeArtifact(path, content, options?)`. The host
    /// validates `path`, writes `content` under the per-run artifact root, and
    /// settles the matching promise with a receipt (or a structured error).
    /// `#[serde(default)]` on `content_type` keeps the field optional so a
    /// caller that omits `options.contentType` deserializes to `None` (the host
    /// then defaults it to `text/plain`).
    ArtifactWrite {
        seq: u64,
        path: String,
        content: String,
        #[serde(default)]
        content_type: Option<String>,
    },
    SnippetResolve {
        seq: u64,
        name: String,
        #[serde(default)]
        input: Value,
    },
    /// Runner completed successfully. `result` is the serialized return value of
    /// the async function (`Undefined` when the function returns undefined).
    /// `logs` carries captured console output (Boa path) or redirected stderr (Javy path).
    Done {
        // #[serde(default)] makes this variant forward-compatible: old runner binaries
        // that emit {"type":"done"} without these fields deserialize to Undefined/[] instead
        // of failing with a missing-field error.
        #[serde(default)]
        result: CodeModeRunnerResult,
        #[serde(default)]
        logs: Vec<String>,
    },
    Error {
        kind: String,
        message: String,
        #[serde(default, skip_serializing_if = "is_false")]
        runner_unhealthy: bool,
    },
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub(crate) enum CodeModeRunnerResult {
    #[default]
    Undefined,
    Json(Value),
}

impl CodeModeRunnerResult {
    #[must_use]
    pub(crate) fn into_response_result(self) -> Option<Value> {
        match self {
            Self::Undefined => None,
            Self::Json(value) => Some(value),
        }
    }
}

pub(crate) struct CodeModeRunnerState {
    pub(crate) reader: BufReader<io::Stdin>,
    pub(crate) writer: BufWriter<io::Stdout>,
    pub(crate) next_seq: u64,
}

thread_local! {
    pub(crate) static RUNNER_STATE: RefCell<Option<CodeModeRunnerState>> = const { RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_defaults_to_reusable_for_older_runner_frames() {
        let output: CodeModeRunnerOutput =
            serde_json::from_str(r#"{"type":"error","kind":"timeout","message":"old"}"#).unwrap();
        assert_eq!(
            output,
            CodeModeRunnerOutput::Error {
                kind: "timeout".to_string(),
                message: "old".to_string(),
                runner_unhealthy: false,
            }
        );
    }

    #[test]
    fn error_serializes_runner_unhealthy_only_when_true() {
        let reusable = serde_json::to_value(CodeModeRunnerOutput::Error {
            kind: "timeout".to_string(),
            message: "user timeout".to_string(),
            runner_unhealthy: false,
        })
        .unwrap();
        assert!(reusable.get("runner_unhealthy").is_none());

        let unhealthy = serde_json::to_value(CodeModeRunnerOutput::Error {
            kind: "timeout".to_string(),
            message: "codegen timeout".to_string(),
            runner_unhealthy: true,
        })
        .unwrap();
        assert_eq!(unhealthy.get("runner_unhealthy"), Some(&Value::Bool(true)));
    }
}
