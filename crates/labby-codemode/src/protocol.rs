//! Code Mode runner stdio protocol types, shared runner state, and tuning consts.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::RefCell;
use std::io::{self, BufReader, BufWriter};

use crate::CodeModeCallError;

/// Checked by both the native JS bridge and the host before dispatch or tracing.
/// This is a UTF-8 byte limit, independent of the framed protocol line cap.
pub(crate) const MAX_TOOL_ID_BYTES: usize = 1024;

pub(crate) fn check_tool_id_bytes(id: &str) -> Result<(), CodeModeCallError> {
    within_byte_limit(id.len(), MAX_TOOL_ID_BYTES, || {
        format!("callTool id exceeds the {MAX_TOOL_ID_BYTES}-byte UTF-8 limit")
    })
}

/// `Ok` when `len <= max`; otherwise a `budget_exceeded` error whose message
/// names only the limit. `message` must never format caller-controlled text,
/// so a rejected payload is never echoed in the error contract.
fn within_byte_limit(
    len: usize,
    max: usize,
    message: impl FnOnce() -> String,
) -> Result<(), CodeModeCallError> {
    if len <= max {
        Ok(())
    } else {
        Err(CodeModeCallError::new("budget_exceeded", message()))
    }
}

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
        // No `#[serde(default)]` needed on the flattened error:
        // `CodeModeCallError`'s custom `Deserialize` back-fills
        // `contract_version`/`origin`/`recovery`/`side_effects` from `kind`,
        // so a legacy `{type, seq, kind, message}` wire message still
        // deserializes (locked by `error_contract.rs`'s
        // `legacy_kind_message_payload_upgrades_to_current_contract`).
        #[serde(flatten)]
        error: Box<CodeModeCallError>,
    },
    /// Reply to a `StepBegin` request with the host decision for this step.
    ///
    /// `replay: Some(value)` is reserved for future caller-initiated replay;
    /// the current host always sends `None`, executes `fn`, and emits
    /// `StepResult` to journal its value.
    StepDecision {
        seq: u64,
        #[serde(default)]
        replay: Option<Value>,
    },
    /// Ack a `StepResult`: the host accepted the step value for recording. The
    /// sandbox then returns the value from `codemode.step`; the ack does not
    /// prove that detached persistent storage has flushed.
    StepRecorded {
        seq: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CodeModeRunnerOutput {
    ToolCall {
        seq: u64,
        id: String,
        params: Value,
        /// Request the original successful MCP envelope, with identical dispatch.
        /// Omit false so ordinary calls keep their original wire shape.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        full_result: bool,
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
    /// The sandbox entered `codemode.step(name, fn)`. The host decides whether
    /// to execute BEFORE `fn` runs and replies with a `StepDecision`. The
    /// current host always executes; the decision field is reserved for future
    /// replay work. This consumes a `seq` from the same monotonic spine as tool
    /// calls.
    StepBegin { seq: u64, name: String },
    /// The sandbox ran the step `fn` (decision was execute) and is journaling
    /// its result. The host records it and acks with `StepRecorded`.
    StepResult { seq: u64, value: Value },
    /// Runner completed successfully. `result` is the serialized return value of
    /// the async function (`Undefined` when the function returns undefined).
    /// `logs` carries captured console output or redirected runner stderr.
    Done {
        // #[serde(default)] makes this variant forward-compatible: old runner binaries
        // that emit {"type":"done"} without these fields deserialize to Undefined/[] instead
        // of failing with a missing-field error.
        #[serde(default)]
        result: CodeModeRunnerResult,
        #[serde(default)]
        logs: Vec<String>,
        /// Native proof that all emitted requests' replies were consumed. A
        /// caught or unawaited operation can return successfully without this
        /// proof; with no pending host calls the parent preserves its response
        /// and evicts without replay. Pending host calls remain a protocol fault.
        /// Missing proof defaults to no reuse for older runner compatibility.
        #[serde(default)]
        reuse_ready: bool,
    },
    Error {
        // As with `CodeModeRunnerInput::ToolError`, no `#[serde(default)]`:
        // the flattened `CodeModeCallError`'s custom `Deserialize` upgrades a
        // bare `{type, kind, message}` message from an older runner binary to
        // the current contract defaults.
        #[serde(flatten)]
        error: Box<CodeModeCallError>,
    },
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
    pub(crate) fn from_response_result(result: Option<Value>) -> Self {
        match result {
            Some(value) => Self::Json(value),
            None => Self::Undefined,
        }
    }

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
    pub(crate) replies: crate::runner::reply_ownership::ReplyOwnership,
}

thread_local! {
    pub(crate) static RUNNER_STATE: RefCell<Option<CodeModeRunnerState>> = const { RefCell::new(None) };
}

/// Per-call cap on the serialized size of `callTool` params. Checked by the
/// native JS bridge before emission and again by the host before the params
/// enter a pending call, so a compromised runner cannot bypass it. Sized above
/// the largest local-provider payload (state files default to 1 MiB) with room
/// for JSON escaping.
pub(crate) const MAX_TOOL_PARAMS_BYTES: usize = 8 * 1024 * 1024;

/// Per-run cap on the serialized params held by queued or in-flight host calls.
/// Without it, `max_calls_per_run` under-cap calls queued behind the
/// per-upstream semaphore would keep calls x params bytes in host memory.
pub(crate) const MAX_PENDING_TOOL_PARAMS_BYTES_PER_RUN: usize = 64 * 1024 * 1024;

/// UTF-8 byte cap on a `codemode.step` name. The host retains each in-flight
/// step's name until its result arrives.
pub(crate) const MAX_STEP_NAME_BYTES: usize = 1024;

// A call admitted under the per-call cap must always fit an empty run.
const _: () = assert!(MAX_TOOL_PARAMS_BYTES <= MAX_PENDING_TOOL_PARAMS_BYTES_PER_RUN);

pub(crate) fn check_tool_params_bytes(len: usize) -> Result<(), CodeModeCallError> {
    within_byte_limit(len, MAX_TOOL_PARAMS_BYTES, || {
        format!("callTool params exceed the {MAX_TOOL_PARAMS_BYTES}-byte serialized limit")
    })
}

pub(crate) fn check_pending_tool_params_bytes(
    pending: usize,
    len: usize,
) -> Result<(), CodeModeCallError> {
    within_byte_limit(
        pending.saturating_add(len),
        MAX_PENDING_TOOL_PARAMS_BYTES_PER_RUN,
        || {
            format!(
                "pending callTool params exceed the {MAX_PENDING_TOOL_PARAMS_BYTES_PER_RUN}-byte per-run limit; await earlier calls before issuing more"
            )
        },
    )
}

pub(crate) fn check_step_name_bytes(name: &str) -> Result<(), CodeModeCallError> {
    within_byte_limit(name.len(), MAX_STEP_NAME_BYTES, || {
        format!("codemode.step name exceeds the {MAX_STEP_NAME_BYTES}-byte UTF-8 limit")
    })
}

/// Admit one call's params against the per-call cap and the run's `pending`
/// params bytes, returning the serialized size to reserve. Measurement stops
/// at the per-call cap, so an oversized payload is never walked in full.
pub(crate) fn admit_tool_params(
    params: &Value,
    pending: usize,
) -> Result<usize, CodeModeCallError> {
    let bytes = match serialized_len_within(params, MAX_TOOL_PARAMS_BYTES) {
        Ok(bytes) => bytes,
        Err(SerializedLenError::Exceeded) => {
            return check_tool_params_bytes(usize::MAX).map(|()| usize::MAX);
        }
        Err(SerializedLenError::Encode) => {
            return Err(CodeModeCallError::new(
                "internal_error",
                "callTool params could not be serialized",
            ));
        }
    };
    check_pending_tool_params_bytes(pending, bytes)?;
    Ok(bytes)
}

/// Why [`serialized_len_within`] could not report a length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SerializedLenError {
    /// The encoding passed `limit`; counting stopped there.
    Exceeded,
    /// The value could not be encoded.
    Encode,
}

/// Serialized JSON length of `value`, counted without allocating the text and
/// abandoned as soon as it passes `limit`.
pub(crate) fn serialized_len_within(
    value: &impl Serialize,
    limit: usize,
) -> Result<usize, SerializedLenError> {
    struct ByteCounter {
        bytes: usize,
        limit: usize,
        exceeded: bool,
    }
    impl io::Write for ByteCounter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.len() > self.limit.saturating_sub(self.bytes) {
                self.exceeded = true;
                return Err(io::Error::other("serialized byte limit"));
            }
            self.bytes += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = ByteCounter {
        bytes: 0,
        limit,
        exceeded: false,
    };
    match serde_json::to_writer(&mut counter, value) {
        Ok(()) => Ok(counter.bytes),
        Err(_) if counter.exceeded => Err(SerializedLenError::Exceeded),
        Err(_) => Err(SerializedLenError::Encode),
    }
}

// Javy interprets this as the native stack size in bytes. The runtime
// `codemode.*` proxy preamble (one method per tool, ~140+ across the
// catalog) plus await/Promise machinery needs ample headroom; 256 KiB avoids
// operand-stack overflow on a single callTool.
pub(crate) const CODE_MODE_STACK_SIZE_LIMIT: usize = 256 * 1024;

#[cfg(test)]
mod id_tests {
    use super::*;

    #[test]
    fn params_and_step_name_limits_never_echo_rejected_input() {
        assert!(check_tool_params_bytes(MAX_TOOL_PARAMS_BYTES).is_ok());
        let error = check_tool_params_bytes(MAX_TOOL_PARAMS_BYTES + 1).expect_err("oversized");
        assert_eq!(error.kind(), "budget_exceeded");

        assert!(check_pending_tool_params_bytes(0, MAX_PENDING_TOOL_PARAMS_BYTES_PER_RUN).is_ok());
        let error = check_pending_tool_params_bytes(MAX_PENDING_TOOL_PARAMS_BYTES_PER_RUN, 1)
            .expect_err("over the pending budget");
        assert_eq!(error.kind(), "budget_exceeded");

        assert!(check_step_name_bytes(&"s".repeat(MAX_STEP_NAME_BYTES)).is_ok());
        let name = "é".repeat(MAX_STEP_NAME_BYTES / 2 + 1);
        let error = check_step_name_bytes(&name).expect_err("oversized step name");
        assert_eq!(error.kind(), "budget_exceeded");
        assert!(!serde_json::to_string(&error).unwrap().contains(&name));
    }

    #[test]
    fn serialized_len_within_matches_serde_output_and_stops_at_the_limit() {
        let value =
            serde_json::json!({"a": "x\"y", "b": [1, 2.5, null, true], "c": {"é": "\u{1}"}});
        let len = serde_json::to_vec(&value).unwrap().len();
        assert_eq!(serialized_len_within(&value, len), Ok(len));
        assert_eq!(
            serialized_len_within(&value, len - 1),
            Err(SerializedLenError::Exceeded)
        );
    }

    #[test]
    fn tool_id_limit_counts_utf8_bytes_and_never_echoes_rejected_input() {
        assert!(check_tool_id_bytes(&"x".repeat(MAX_TOOL_ID_BYTES)).is_ok());
        assert!(check_tool_id_bytes(&"é".repeat(MAX_TOOL_ID_BYTES / 2)).is_ok());
        for id in [
            "x".repeat(MAX_TOOL_ID_BYTES + 1),
            "é".repeat(MAX_TOOL_ID_BYTES / 2 + 1),
        ] {
            let error = check_tool_id_bytes(&id).expect_err("oversized ID");
            let wire = serde_json::to_string(&error).expect("error JSON");
            assert_eq!(error.kind(), "budget_exceeded");
            assert_eq!(error.origin, crate::CodeModeErrorOrigin::Budget);
            assert!(error.tool.is_none());
            assert!(!wire.contains(&id));
        }
    }
    #[test]
    fn full_result_is_opt_in_on_the_wire_and_defaults_false_on_decode() {
        let ordinary =
            serde_json::json!({"type":"tool_call","seq":0,"id":"fixture::lookup","params":{}});
        for flag in [None, Some(false), Some(true)] {
            let mut wire = ordinary.clone();
            if let Some(value) = flag {
                wire["full_result"] = serde_json::json!(value);
            }
            let parsed: CodeModeRunnerOutput = serde_json::from_value(wire).unwrap();
            assert!(
                matches!(&parsed, CodeModeRunnerOutput::ToolCall { full_result, .. } if *full_result == flag.unwrap_or(false))
            );
            let mut expected = ordinary.clone();
            if flag == Some(true) {
                expected["full_result"] = serde_json::json!(true);
            }
            assert_eq!(serde_json::to_value(parsed).unwrap(), expected);
        }
    }
}
