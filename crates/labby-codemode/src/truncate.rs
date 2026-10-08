//! Response-budget truncation for Code Mode execution responses and log caps.

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

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

    // Preserve the model-requested final result before optional debug detail.
    // High-fan-out executions can make redacted call params dominate an otherwise
    // compact response (for example, dozens of SSH command strings). Params are
    // explicitly optional trace metadata, so drop them first under envelope
    // pressure while preserving every call record and its timing/error fields.
    // This keeps a useful compact result from being replaced by a truncation
    // marker merely because tracing was enabled.
    if response.calls.iter().any(|call| call.params.is_some()) {
        for call in &mut response.calls {
            call.params = None;
        }
        if response_within_budget(
            &response,
            max_response_bytes,
            max_response_tokens,
            token_estimate_divisor,
        ) {
            return response;
        }
    }

    // Cap the FINAL result next, but only when doing so actually shrinks the
    // envelope. The marker has a ~1 KB preview, so markering an already-small
    // result (e.g. `{"ok":true}`) would *grow* it. In a logs-dominant response
    // the result therefore remains intact and the log trimming path below gets
    // the next opportunity to reclaim space.
    if let Some(marker) = result_marker(
        &mut response,
        max_response_bytes,
        max_response_tokens,
        token_estimate_divisor,
    ) {
        response.result = Some(marker);
        response.result_shaping = None;
    }

    // A logs-dominant response can still exceed budget after capping the result. Trim `logs`
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

/// Probe automatic artifact preservation without cloning the result tree or
/// optional trace payloads. Every temporarily removed field is restored.
pub(crate) fn result_would_be_truncated(
    response: &mut CodeModeExecutionResponse,
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> bool {
    if response_within_budget(
        response,
        max_response_bytes,
        max_response_tokens,
        token_estimate_divisor,
    ) {
        return false;
    }
    let params: Vec<_> = response
        .calls
        .iter_mut()
        .map(|call| call.params.take())
        .collect();
    let changed = !response_within_budget(
        response,
        max_response_bytes,
        max_response_tokens,
        token_estimate_divisor,
    ) && result_marker(
        response,
        max_response_bytes,
        max_response_tokens,
        token_estimate_divisor,
    )
    .is_some();
    for (call, params) in response.calls.iter_mut().zip(params) {
        call.params = params;
    }
    changed
}

fn result_marker(
    response: &mut CodeModeExecutionResponse,
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> Option<Value> {
    let result = response.result.take()?;
    // Reuse one serialization across preview sizes, and probe in place so
    // large logs, UI payloads, and call metadata are never cloned per marker.
    let serialized = serde_json::to_string(&result).unwrap_or_else(|_| "null".to_string());
    let preserved = preserved_result_receipt(&serialized, &response.artifacts);
    let original_len = serialized.len();
    let shaping = response.result_shaping.take();
    let mut selected = None;
    for (preview_bytes, compact) in [(1024, false), (512, false), (0, false), (0, true)] {
        let marker = truncation_marker_serialized(
            &serialized,
            token_estimate_divisor,
            &response.artifacts,
            preview_bytes,
            compact,
            preserved,
        );
        let marker_len = serde_json::to_vec(&marker).map_or(usize::MAX, |s| s.len());
        if marker_len >= original_len {
            continue;
        }
        response.result = Some(marker);
        if response_within_budget(
            response,
            max_response_bytes,
            max_response_tokens,
            token_estimate_divisor,
        ) || compact
        {
            selected = response.result.take();
            break;
        }
    }
    response.result = Some(result);
    response.result_shaping = shaping;
    selected
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

pub(crate) const TRUNCATION_RECOVERY: &str = "Only the returned output was truncated; execution has already run. Do not replay mutations to retrieve output. Use existing artifact receipts or a read-only query returning fewer fields. For a resource, use resource_read_example with the exact URI from codemode.listResources(upstream), then repeat with offset = next_offset until done. Offsets count JavaScript UTF-16 code units, not bytes. Each call re-reads the resource; keep its version stable. Reduce length if a chunk still exceeds your response budget. The omitted output is not cached by this marker.";

// Serializing the resource envelope preserves every content block, including
// binary metadata. A small slice leaves room for JSON escaping and call metadata.
pub(crate) const RESOURCE_READ_EXAMPLE: &str = r#"async () => {
  const uri = "REPLACE_WITH_DISCOVERED_RESOURCE_URI";
  const offset = 0, length = 1000;
  const serialized = JSON.stringify(await codemode.readResource(uri));
  let end = Math.min(offset + Math.max(2, length), serialized.length);
  const last = serialized.charCodeAt(end - 1);
  if (end < serialized.length && last >= 0xD800 && last <= 0xDBFF) end--;
  const chunk = serialized.slice(offset, end);
  const next_offset = offset + chunk.length;
  return {chunk, next_offset, total: serialized.length, done: next_offset >= serialized.length};
}"#;

#[cfg(test)]
fn truncation_marker(
    value: &Value,
    token_estimate_divisor: u32,
    artifacts: &[CodeModeArtifactReceipt],
    preview_bytes: usize,
    compact: bool,
) -> Value {
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    truncation_marker_serialized(
        &serialized,
        token_estimate_divisor,
        artifacts,
        preview_bytes,
        compact,
        preserved_result_receipt(&serialized, artifacts),
    )
}

fn preserved_result_receipt<'a>(
    serialized: &str,
    artifacts: &'a [CodeModeArtifactReceipt],
) -> Option<&'a CodeModeArtifactReceipt> {
    let mut candidates = artifacts.iter().filter(|receipt| {
        receipt.path == "automatic/final-result.json"
            && receipt.artifact_id.is_some()
            && receipt.bytes == serialized.len()
    });
    let first = candidates.next()?;
    // The path can be chosen by explicit writes. Verify broker-derived bytes
    // and digest before claiming any receipt holds the complete returned JSON.
    // Hash once across all preview probes, and only when a candidate exists.
    let digest = hex::encode(Sha256::digest(serialized.as_bytes()));
    std::iter::once(first)
        .chain(candidates)
        .find(|receipt| receipt.sha256 == digest)
}

fn truncation_marker_serialized(
    serialized: &str,
    token_estimate_divisor: u32,
    artifacts: &[CodeModeArtifactReceipt],
    preview_bytes: usize,
    compact: bool,
    preserved: Option<&CodeModeArtifactReceipt>,
) -> Value {
    let preview = utf8_prefix_by_bytes(serialized, preview_bytes).to_string();
    let mut marker = json!({
        "truncated": true,
        "original_size": serialized.len(),
        "original_tokens": estimated_tokens(serialized.len(), token_estimate_divisor),
        "preview": preview,
        "artifacts": artifacts,
        "next_action": TRUNCATION_RECOVERY,
        "resource_read_example": RESOURCE_READ_EXAMPLE
    });
    if compact {
        marker
            .as_object_mut()
            .unwrap()
            .remove("resource_read_example");
        marker["next_action"] = json!(
            "Output only; execution already ran. Do not replay mutations. Omitted output is not cached. Use artifact receipts or a read-only query. For a stable resource, read its exact discovered URI, JSON.stringify the envelope, and return small slices without splitting UTF-16 surrogate pairs. Advance the offset by the returned chunk length until the total length is reached; lower the chunk size if needed."
        );
    }
    if let Some(receipt) = preserved {
        marker["preserved_result_artifact_id"] = json!(receipt.artifact_id);
        marker["next_action"] = json!(
            "Complete returned JSON saved in preserved_result_artifact_id. Read it with codemode.readArtifact(id, {offset, length}); offsets are UTF-8 bytes. Do not replay mutations. Artifact retention limits apply."
        );
    }
    marker
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
mod tests;

#[cfg(test)]
#[path = "truncate/resource_recovery_tests.rs"]
mod resource_recovery_tests;
