//! Retained host trace admission, including rejected and artifact call rows.

use super::*;
use std::collections::HashMap;
use std::io::{self, Write};
use std::ops::Deref;

pub(super) const MAX_HOST_TRACE_ROWS: usize = 4096;
pub(super) const MAX_HOST_TRACE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub(super) struct HostTrace {
    calls: Vec<(u64, CodeModeExecutedCall)>,
    bytes: usize,
    reservations: HashMap<u64, usize>,
    reserved_bytes: usize,
}

impl Deref for HostTrace {
    type Target = [(u64, CodeModeExecutedCall)];

    fn deref(&self) -> &Self::Target {
        &self.calls
    }
}

impl HostTrace {
    /// Reserve a row and its minimum encoded bytes before dispatch/publication.
    /// Optional completion metadata can still exhaust the byte budget later;
    /// that fails explicitly with possible prior side effects and a partial trace.
    pub(super) fn reserve(&mut self, seq: u64, id: &str) -> Result<(), ToolError> {
        if self.reservations.contains_key(&seq) || self.calls.iter().any(|(seen, _)| *seen == seq) {
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: "Code Mode runner reused a traced protocol sequence".into(),
            });
        }
        if self.calls.len() + self.reservations.len() >= MAX_HOST_TRACE_ROWS {
            return Err(trace_budget_error());
        }
        let minimum = CodeModeExecutedCall {
            id: id.into(),
            ok: false,
            elapsed_ms: 0,
            start_ms: None,
            params: None,
            params_digest: None,
            error_kind: None,
            ui: None,
        };
        let bytes = encoded_bytes(&minimum, self.remaining_bytes())?;
        self.reservations.insert(seq, bytes);
        self.reserved_bytes += bytes;
        Ok(())
    }

    pub(super) fn try_push(&mut self, call: (u64, CodeModeExecutedCall)) -> Result<(), ToolError> {
        let reserved = self.reservations.remove(&call.0).unwrap_or(0);
        self.reserved_bytes -= reserved;
        if self.calls.len() + self.reservations.len() >= MAX_HOST_TRACE_ROWS {
            return Err(trace_budget_error());
        }
        let bytes = encoded_bytes(&call.1, self.remaining_bytes()).map_err(|error| {
            if error.kind() != "trace_budget_exceeded" {
                return error;
            }
            let mut error = CodeModeCallError::from(error).with_tool(call.1.id.clone());
            // Retain bounded completed-outcome provenance without the oversized
            // params/UI metadata that exhausted the trace. Prior work can have
            // effects even though this row could not be retained.
            error.evidence = Some(crate::CodeModeErrorEvidence {
                structured_content: Some(json!({"trace_event": {
                    "seq": call.0, "id": call.1.id, "ok": call.1.ok,
                    "elapsed_ms": call.1.elapsed_ms,
                    "error_kind": call.1.error_kind.as_deref().map(|kind| labby_runtime::redact::sanitize_error_text(kind, 128)),
                    "row_retained": false,
                }})),
                ..crate::CodeModeErrorEvidence::default()
            });
            error.into_contract_tool_error()
        })?;
        self.bytes += bytes;
        self.calls.push(call);
        Ok(())
    }

    fn remaining_bytes(&self) -> usize {
        // Include brackets and commas for the prospective calls[] array, not
        // just row payloads. Pending reservations already consume row capacity.
        let array_overhead = 2 + self.calls.len() + self.reservations.len();
        MAX_HOST_TRACE_BYTES.saturating_sub(self.bytes + self.reserved_bytes + array_overhead)
    }
}

fn trace_budget_error() -> ToolError {
    let mut error = CodeModeCallError::new(
        "trace_budget_exceeded",
        format!(
            "Code Mode host trace exceeds {MAX_HOST_TRACE_ROWS} rows or {MAX_HOST_TRACE_BYTES} serialized row bytes"
        ),
    );
    error.side_effects = crate::CodeModeSideEffectRisk::Possible;
    error.recovery.guidance = "Execution stopped at the host trace limit. Earlier tool calls or artifact writes may have completed; inspect the partial trace and effects before reducing or repeating the work.".into();
    error.into_contract_tool_error()
}

/// Count serialized row bytes without allocating another complete JSON buffer.
fn encoded_bytes(call: &CodeModeExecutedCall, limit: usize) -> Result<usize, ToolError> {
    let mut counter = ByteCounter {
        bytes: 0,
        limit,
        exceeded: false,
    };
    if serde_json::to_writer(&mut counter, call).is_err() {
        return if counter.exceeded {
            Err(trace_budget_error())
        } else {
            Err(ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: "failed to encode a Code Mode trace row".into(),
            })
        };
    }
    Ok(counter.bytes)
}

struct ByteCounter {
    bytes: usize,
    limit: usize,
    exceeded: bool,
}

impl Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() > self.limit.saturating_sub(self.bytes) {
            self.exceeded = true;
            return Err(io::Error::other("host trace byte limit"));
        }
        self.bytes += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(id: String) -> CodeModeExecutedCall {
        CodeModeExecutedCall {
            id,
            ok: false,
            elapsed_ms: 0,
            start_ms: None,
            params: None,
            params_digest: None,
            error_kind: Some("call_budget_exceeded".into()),
            ui: None,
        }
    }

    #[test]
    fn rejected_rows_and_pending_reservations_share_the_trace_count_limit() {
        let mut trace = HostTrace::default();
        for seq in 0..MAX_HOST_TRACE_ROWS as u64 {
            trace.reserve(seq, "fixture::rejected").expect("capacity");
            if seq % 2 == 0 {
                trace
                    .try_push((seq, rejected("fixture::rejected".into())))
                    .expect("failure row");
            }
        }
        assert_eq!(
            trace
                .reserve(MAX_HOST_TRACE_ROWS as u64, "fixture::extra")
                .unwrap_err()
                .kind(),
            "trace_budget_exceeded"
        );
        assert_eq!(
            trace.calls.len() + trace.reservations.len(),
            MAX_HOST_TRACE_ROWS
        );
    }

    #[test]
    fn all_failure_metadata_counts_toward_serialized_trace_bytes() {
        let mut trace = HostTrace::default();
        let row = rejected("é".repeat(crate::protocol::MAX_TOOL_ID_BYTES / 2));
        trace.reserve(0, &row.id).expect("capacity");
        let expected = serde_json::to_vec(&row).expect("row JSON").len();
        trace.try_push((0, row)).expect("failure row");
        assert_eq!(trace.bytes, expected);
        let mut oversized = rejected("fixture::large-metadata".into());
        oversized.params = Some(json!({"message": "x".repeat(MAX_HOST_TRACE_BYTES)}));
        assert_eq!(
            trace.try_push((1, oversized)).unwrap_err().kind(),
            "trace_budget_exceeded"
        );
        assert_eq!(trace.len(), 1);
        assert_eq!(trace.bytes, expected, "overflow must not retain its row");
        assert!(serde_json::to_vec(&sorted_calls(&trace)).unwrap().len() <= MAX_HOST_TRACE_BYTES);
    }

    #[test]
    fn overflow_contract_preserves_completed_provenance_and_possible_effects() {
        let mut trace = HostTrace::default();
        trace
            .reserve(7, "fixture::completed")
            .expect("row reservation");
        let mut completed = rejected("fixture::completed".into());
        completed.ok = true;
        completed.error_kind = None;
        completed.params = Some(json!({"metadata": "x".repeat(MAX_HOST_TRACE_BYTES)}));
        let error = CodeModeExecutionError::with_trace(
            trace
                .try_push((7, completed))
                .expect_err("metadata overflow"),
            Vec::new(),
        )
        .into_call_error();
        assert_eq!(error.kind(), "trace_budget_exceeded");
        assert_eq!(error.origin, crate::CodeModeErrorOrigin::Budget);
        assert_eq!(error.side_effects, crate::CodeModeSideEffectRisk::Possible);
        assert_eq!(
            error
                .evidence
                .as_ref()
                .unwrap()
                .structured_content
                .as_ref()
                .unwrap()["trace_event"]["ok"],
            true
        );
        let restored = CodeModeCallError::from(error.clone().into_contract_tool_error());
        assert_eq!(restored, error);
        assert!(serde_json::to_vec(&error).expect("contract JSON").len() < 4096);
        assert!(trace.is_empty());
    }
}
