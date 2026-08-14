//! Response-budget truncation for Code Mode execution responses and log caps.

use serde_json::{Value, json};

use super::artifacts::CodeModeArtifactReceipt;
use super::types::CodeModeExecutionResponse;

/// Sanitize one line of runner-captured log/output text before it is returned
/// to the caller: strips control / bidi-override characters and common
/// prompt-injection markers, redacts secret-like segments, and caps length.
///
/// Thin crate-local alias of the canonical `labby_runtime` helper so runner
/// modules keep one import path. External consumers should depend on
/// `labby_runtime::agent_error` directly.
pub(crate) fn sanitize_log_text(input: &str, max_len: usize) -> String {
    labby_runtime::agent_error::sanitize_log_text(input, max_len)
}

pub(crate) fn truncate_execution_response(
    mut response: CodeModeExecutionResponse,
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> CodeModeExecutionResponse {
    if response_within_budget(
        &response,
        max_response_bytes,
        max_response_tokens,
        token_estimate_divisor,
    ) {
        return response;
    }

    // calls[] carries lightweight metadata only (no result payloads), so there
    // is nothing per-call to truncate. Cap the FINAL result first — but only
    // when doing so actually shrinks the envelope. The marker has a ~1 KB
    // preview floor, so markering an already-small result (e.g. `{"ok":true}`)
    // would *grow* it; in a logs-dominant response the result is innocent and
    // must be left intact so log trimming can do the work.
    if let Some(result) = response.result.as_ref() {
        let original_len = serde_json::to_string(result).map(|s| s.len()).unwrap_or(0);
        let marker = truncation_marker(result, token_estimate_divisor, &response.artifacts);
        let marker_len = serde_json::to_string(&marker).map(|s| s.len()).unwrap_or(0);
        if marker_len < original_len {
            response.result = Some(marker);
            response.result_shaping = None;
        }
    }

    // The result marker has a fixed ~1 KB preview floor, so a logs-dominant
    // response can still exceed budget after capping the result. Trim `logs`
    // oldest-first until within budget, keeping the newest lines that fit and
    // prepending a sentinel that records how many were dropped. Best-effort:
    // `calls[]` metadata alone can dominate a high fan-out run and is not
    // trimmed here, so the loop terminates on logs-exhaustion rather than
    // guaranteeing budget (see report — residual is a follow-up).
    if !response.logs.is_empty()
        && !response_within_budget(
            &response,
            max_response_bytes,
            max_response_tokens,
            token_estimate_divisor,
        )
    {
        let original = std::mem::take(&mut response.logs);
        let total = original.len();

        // Compute the drop point in a single pass: serialize the log-free base
        // response ONCE, precompute each line's serialized JSON length, then
        // walk candidate drop counts arithmetically. This replaces the earlier
        // binary search, which cloned and fully re-serialized the response per
        // probe (O(S log n) serialization work); the arithmetic scan does O(S)
        // serialization total and picks the same cut byte-for-byte.
        //
        // `drop_count = 0` means keep all lines (we already know that's over
        // budget). `drop_count = total` means drop everything (sentinel-only);
        // that is the fallback when even a single log line is too large.
        let drop_count = logs_drop_count(
            &original,
            &response,
            max_response_bytes,
            max_response_tokens,
            token_estimate_divisor,
        );

        let mut candidate = Vec::with_capacity(original.len() - drop_count + 1);
        if drop_count > 0 {
            candidate.push(format!(
                "[logs truncated to fit response budget — {drop_count} line(s) dropped]"
            ));
        }
        candidate.extend_from_slice(&original[drop_count..]);
        response.logs = candidate;
        debug_assert!(drop_count <= total);
    }

    response
}

/// Compute the minimum number of oldest log lines to drop so that the overall
/// response fits within the byte/token budget.
///
/// `response` must already have its `logs` emptied (the caller `take`s them
/// into `original`). Contract: the returned drop count is the smallest `k`
/// such that the response serialized with `original[k..]` as `logs` satisfies
/// [`response_within_budget`] — identical to serializing each candidate, but
/// derived arithmetically: `logs` is a mandatory `Vec<String>` field, so a
/// response with `k` kept lines serializes to exactly
/// `base_len + Σ len(line_i) + (k - 1)` bytes (`base_len` = the empty-logs
/// serialization, `len` = the line's serialized JSON string length, `k - 1`
/// commas; zero extra bytes when `k = 0`).
///
/// Returns the drop count (0 = drop nothing, `total` = drop everything).
/// The caller is responsible for prepending a sentinel when `drop_count > 0`.
fn logs_drop_count(
    original: &[String],
    response: &CodeModeExecutionResponse,
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> usize {
    let total = original.len();
    // Serialize the log-free base once. A serialization failure mirrors
    // `response_within_budget`'s treat-as-over-budget behavior → drop all.
    let Ok(base) = serde_json::to_vec(response) else {
        return total;
    };
    let base_len = base.len();

    let fits = |len: usize| {
        len <= max_response_bytes
            && estimated_tokens(len, token_estimate_divisor) <= max_response_tokens.max(1)
    };

    // Fast path: dropping everything still over budget → return total.
    if !fits(base_len) {
        return total;
    }

    // Per-line serialized lengths (quotes + JSON escapes included), summed once.
    let line_lens: Vec<usize> = original
        .iter()
        .map(|line| {
            serde_json::to_string(line).map_or_else(
                // Unreachable for valid UTF-8, but stay conservative: quote
                // bytes only, matching the raw payload floor.
                |_| line.len() + 2,
                |serialized| serialized.len(),
            )
        })
        .collect();
    let mut kept_sum: usize = line_lens.iter().sum();

    // Walk drop counts in ascending order, shrinking the running suffix sum;
    // the serialized length is monotonically non-increasing, so the first fit
    // is the minimal drop count.
    for (drop_count, line_len) in line_lens.iter().enumerate() {
        let kept = total - drop_count;
        let commas = kept.saturating_sub(1);
        if fits(base_len + kept_sum + commas) {
            return drop_count;
        }
        kept_sum -= line_len;
    }
    // Only the empty-logs shape fits (already verified above).
    total
}

pub(crate) fn response_within_budget(
    response: &CodeModeExecutionResponse,
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> bool {
    match serde_json::to_vec(response) {
        Ok(bytes) => {
            bytes.len() <= max_response_bytes
                && estimated_tokens(bytes.len(), token_estimate_divisor)
                    <= max_response_tokens.max(1)
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "response_within_budget: failed to serialize response; treating as over-budget"
            );
            false
        }
    }
}

fn estimated_tokens(byte_len: usize, divisor: u32) -> usize {
    byte_len.div_ceil(divisor.max(1) as usize).max(1)
}

fn truncation_marker(
    value: &Value,
    token_estimate_divisor: u32,
    artifacts: &[CodeModeArtifactReceipt],
) -> Value {
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    let preview = utf8_prefix_by_bytes(&serialized, 1024).to_string();
    json!({
        "truncated": true,
        "original_size": serialized.len(),
        "original_tokens": estimated_tokens(serialized.len(), token_estimate_divisor),
        "preview": preview,
        "artifacts": artifacts,
        "next_action": "Use a narrower query, request fewer fields, or split the work across multiple codemode calls."
    })
}

fn utf8_prefix_by_bytes(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }

    let end = value
        .char_indices()
        .map(|(idx, _)| idx)
        .take_while(|idx| *idx <= max_bytes)
        .last()
        .unwrap_or(0);
    &value[..end]
}

/// Enforce `max_log_entries` and `max_log_bytes` caps on captured log lines.
///
/// Returns the capped list. If either cap trips, appends a single sentinel line
/// `"[log output truncated at N lines / M bytes]"` as the last entry.
pub(crate) fn apply_log_caps(
    mut logs: Vec<String>,
    max_entries: usize,
    max_bytes: usize,
) -> Vec<String> {
    let max_entries = max_entries.max(1);
    let max_bytes = max_bytes.max(1);

    let mut kept_bytes: usize = 0;
    let mut kept = 0;
    let mut truncated = false;

    for (i, line) in logs.iter().enumerate() {
        if i >= max_entries {
            truncated = true;
            break;
        }
        // Check the prospective total before counting the line so a line that
        // would push us over the cap is dropped without inflating the reported
        // byte count — the sentinel reflects only the bytes actually kept.
        if kept_bytes + line.len() > max_bytes {
            truncated = true;
            break;
        }
        kept_bytes += line.len();
        kept = i + 1;
    }

    if truncated {
        logs.truncate(kept);
        logs.push(format!(
            "[log output truncated at {kept} lines / {kept_bytes} bytes]"
        ));
    }

    logs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::CodeModeResultShapeMetadata;
    use crate::types::CodeModeExecutedCall;
    use labby_runtime::CodeModeResultShapePolicy;

    fn response_with_logs(result: Value, logs: Vec<String>) -> CodeModeExecutionResponse {
        CodeModeExecutionResponse {
            execution_id: None,
            result: Some(result),
            result_shaping: None,
            ui: None,
            calls: Vec::new(),
            logs,
            artifacts: Vec::new(),
        }
    }

    /// Contract-derived expected cut: the smallest `k` such that the response
    /// serialized with `original[k..]` as `logs` passes
    /// [`response_within_budget`]. Probes real serde output per candidate, so
    /// it is independent of the arithmetic implementation under test.
    fn expected_drop_count(
        original: &[String],
        base: &CodeModeExecutionResponse,
        max_bytes: usize,
        max_tokens: usize,
        divisor: u32,
    ) -> usize {
        (0..=original.len())
            .find(|&k| {
                let mut probe = base.clone();
                probe.logs = original[k..].to_vec();
                response_within_budget(&probe, max_bytes, max_tokens, divisor)
            })
            .unwrap_or(original.len())
    }

    /// FR-5 (issue #210, lab-41e7m.2): the DEFAULT truncation path replaces an
    /// over-budget result with an OBJECT marker — `truncated: true`,
    /// `next_action`, a bounded `preview` — while `calls[]` metadata survives
    /// verbatim. Structure must never collapse to a bare string here; the
    /// string-marker path is `shape.rs`, which only runs under a non-`Off`
    /// result-shape policy.
    #[test]
    fn over_budget_result_becomes_object_marker_and_calls_survive() {
        let calls = vec![CodeModeExecutedCall {
            id: "demo::big_query".to_string(),
            ok: true,
            elapsed_ms: 42,
            start_ms: Some(1),
            params: Some(json!({"q": "everything"})),
            error_kind: None,
            ui: None,
        }];
        let mut response =
            response_with_logs(json!({"rows": vec!["r".repeat(64); 200]}), Vec::new());
        response.calls = calls.clone();
        response.result_shaping = Some(CodeModeResultShapeMetadata {
            policy: CodeModeResultShapePolicy::Off,
            changed: false,
            truncated: false,
            original_size_bytes: 0,
            shaped_size_bytes: 0,
            warning: None,
        });

        let truncated = truncate_execution_response(response, 4096, usize::MAX, 4);

        let marker = truncated.result.as_ref().expect("marker result");
        let marker = marker
            .as_object()
            .expect("marker must stay a JSON object, not a string");
        assert_eq!(marker["truncated"], json!(true));
        assert!(
            marker["next_action"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "marker carries agent guidance"
        );
        assert!(
            marker["preview"].as_str().is_some_and(|s| s.len() <= 1024),
            "preview is bounded"
        );
        assert_eq!(
            truncated.calls, calls,
            "structured calls[] metadata must survive result truncation"
        );
        assert!(
            truncated.result_shaping.is_none(),
            "stale shaping metadata must not describe the marker"
        );
    }

    #[test]
    fn large_log_set_cut_matches_probe_serialized_contract() {
        // Varied line lengths plus JSON-escaped and multibyte characters so the
        // arithmetic length model is exercised against real serde output.
        let logs: Vec<String> = (0..800)
            .map(|i| {
                format!(
                    "line {i}: \"quoted\" \\ back {} — ünïcode",
                    "x".repeat(i % 97)
                )
            })
            .collect();
        let response = response_with_logs(json!({"ok": true}), logs.clone());
        let (max_bytes, max_tokens, divisor) = (16 * 1024, 100_000, 4);

        let mut base = response.clone();
        base.logs = Vec::new();
        let expected = expected_drop_count(&logs, &base, max_bytes, max_tokens, divisor);
        assert!(
            expected > 0 && expected < logs.len(),
            "cut must land mid-range for this fixture, got {expected}"
        );

        let truncated = truncate_execution_response(response, max_bytes, max_tokens, divisor);

        // Logs-dominant response: the small result must survive untouched.
        assert_eq!(truncated.result, Some(json!({"ok": true})));
        let mut want = vec![format!(
            "[logs truncated to fit response budget — {expected} line(s) dropped]"
        )];
        want.extend_from_slice(&logs[expected..]);
        assert_eq!(truncated.logs, want);
    }

    #[test]
    fn token_budget_alone_can_force_the_cut() {
        let logs: Vec<String> = (0..300).map(|i| format!("log line number {i}")).collect();
        let response = response_with_logs(json!({"ok": true}), logs.clone());
        // Byte budget is generous; the estimated-token term must drive the cut.
        let (max_bytes, max_tokens, divisor) = (1024 * 1024, 512, 4);

        let mut base = response.clone();
        base.logs = Vec::new();
        let expected = expected_drop_count(&logs, &base, max_bytes, max_tokens, divisor);
        assert!(
            expected > 0 && expected < logs.len(),
            "cut must land mid-range for this fixture, got {expected}"
        );

        let truncated = truncate_execution_response(response, max_bytes, max_tokens, divisor);
        let mut want = vec![format!(
            "[logs truncated to fit response budget — {expected} line(s) dropped]"
        )];
        want.extend_from_slice(&logs[expected..]);
        assert_eq!(truncated.logs, want);
    }

    #[test]
    fn oversized_single_line_drops_everything() {
        let logs = vec!["z".repeat(64 * 1024)];
        let response = response_with_logs(json!({"ok": true}), logs);
        let truncated = truncate_execution_response(response, 4 * 1024, 100_000, 4);
        assert_eq!(
            truncated.logs,
            vec!["[logs truncated to fit response budget — 1 line(s) dropped]".to_string()]
        );
    }

    #[test]
    fn within_budget_response_is_returned_unchanged() {
        let logs: Vec<String> = (0..4).map(|i| format!("short {i}")).collect();
        let response = response_with_logs(json!({"ok": true}), logs.clone());
        let untouched = truncate_execution_response(response.clone(), 1024 * 1024, 100_000, 4);
        assert_eq!(untouched, response);
    }
}
