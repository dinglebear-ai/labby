//! Assertions and performance evidence evaluated against the unshaped response.

use super::{SnippetFixture, SnippetTestBudgets};
use crate::types::CodeModeExecutionOutcome;
use crate::{CodeModeExecutionError, CodeModeExecutionResponse};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// Execution measurements; byte counts use serialized UTF-8, not JS string length.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SnippetTestMetrics {
    /// End-to-end duration including runner acquisition and catalog construction.
    pub elapsed_ms: u128,
    /// Attempted calls, including unsuccessful calls.
    pub tool_calls: usize,
    /// Calls reported unsuccessful by the broker.
    pub failed_calls: usize,
    /// Serialized result bytes before any result shaping.
    pub output_bytes: usize,
    /// Serialized full execution response bytes, including trace metadata.
    pub envelope_bytes: usize,
    /// Whether the display response was truncated.
    pub truncated: bool,
    /// Call counts grouped by exact upstream namespace.
    pub calls_by_upstream: BTreeMap<String, usize>,
}

/// One timing row, intentionally excluding parameters and upstream result bodies.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SnippetTestCall {
    /// Exact tool identifier.
    pub tool: String,
    /// Whether the call completed successfully.
    pub ok: bool,
    /// Offset from execution start in milliseconds.
    pub start_ms: Option<u128>,
    /// Duration in milliseconds.
    pub elapsed_ms: u128,
}

/// Bounded test report usable by CLI, MCP, HTTP, and CI.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SnippetTestReport {
    /// Snippet under test.
    pub name: String,
    /// Explicit execution mode: mock or live.
    pub mode: String,
    /// All assertions and limits were satisfied.
    pub passed: bool,
    /// Measurements taken before result shaping.
    pub metrics: SnippetTestMetrics,
    /// Violations, without embedding large/sensitive expected or actual values.
    pub violations: Vec<String>,
    /// Result included only when it fits the requested output budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Structured execution failure, if the sandbox did not complete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    /// Compact timing graph in dispatch order.
    pub calls: Vec<SnippetTestCall>,
}

fn serialized_bytes(value: &impl Serialize) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

fn truncated(response: &CodeModeExecutionResponse) -> bool {
    response
        .result_shaping
        .as_ref()
        .is_some_and(|metadata| metadata.truncated)
}

/// Measure one response and enforce budgets plus the expected failed-call count.
/// Assertions and snapshots remain additional checks in fixture_report.
pub fn assess_response(
    name: &str,
    mode: &str,
    response: &CodeModeExecutionResponse,
    display: &CodeModeExecutionResponse,
    elapsed_ms: u128,
    budgets: &SnippetTestBudgets,
    expected_failures: usize,
) -> SnippetTestReport {
    let output_bytes = response.result.as_ref().map_or(0, serialized_bytes);
    let mut calls_by_upstream = BTreeMap::new();
    for call in &response.calls {
        let upstream = call
            .id
            .split_once("::")
            .map_or(call.id.as_str(), |(ns, _)| ns);
        *calls_by_upstream.entry(upstream.to_owned()).or_insert(0) += 1;
    }
    let metrics = SnippetTestMetrics {
        elapsed_ms,
        tool_calls: response.calls.len(),
        failed_calls: response.calls.iter().filter(|c| !c.ok).count(),
        output_bytes,
        envelope_bytes: serialized_bytes(response),
        truncated: truncated(display),
        calls_by_upstream,
    };
    let mut violations = Vec::new();
    if elapsed_ms > u128::from(budgets.wall_clock_ms) {
        violations.push("wall_clock_ms budget exceeded".to_owned());
    }
    if metrics.tool_calls as u64 > budgets.tool_calls {
        violations.push("tool_calls budget exceeded".to_owned());
    }
    if output_bytes > budgets.output_bytes {
        violations.push("output_bytes budget exceeded".to_owned());
    }
    if metrics.failed_calls != expected_failures {
        violations.push(format!(
            "expected {expected_failures} failed calls, observed {}",
            metrics.failed_calls
        ));
    }
    if metrics.truncated {
        violations.push("execution response was truncated".to_owned());
    }
    if response.result.is_none() {
        violations.push("snippet returned undefined".to_owned());
    }
    let mut calls: Vec<_> = response
        .calls
        .iter()
        .map(|c| SnippetTestCall {
            tool: c.id.clone(),
            ok: c.ok,
            start_ms: c.start_ms,
            elapsed_ms: c.elapsed_ms,
        })
        .collect();
    calls.sort_by_key(|c| c.start_ms);
    SnippetTestReport {
        name: name.to_owned(),
        mode: mode.to_owned(),
        passed: violations.is_empty(),
        metrics,
        violations,
        result: if output_bytes <= budgets.output_bytes {
            response.result.clone()
        } else {
            None
        },
        error: None,
        calls,
    }
}

pub(super) fn fixture_report(
    name: &str,
    fixture: &SnippetFixture,
    outcome: Result<CodeModeExecutionOutcome, CodeModeExecutionError>,
    elapsed_ms: u128,
    remaining: Vec<String>,
) -> SnippetTestReport {
    let (raw, display, error) = match outcome {
        Ok(outcome) => (outcome.raw_response, outcome.display_response, None),
        Err(error) => {
            let response = CodeModeExecutionResponse {
                execution_id: None,
                result: None,
                result_shaping: None,
                ui: None,
                calls: error.calls().to_vec(),
                logs: Vec::new(),
                artifacts: Vec::new(),
            };
            let error = serde_json::to_value(error.into_contract_tool_error()).ok();
            (response.clone(), response, error)
        }
    };
    let mut report = assess_response(
        name,
        "mock",
        &raw,
        &display,
        elapsed_ms,
        &fixture.budgets,
        fixture.expected_failures,
    );
    report.error = error;
    if report.error.is_some() {
        report
            .violations
            .push("sandbox execution failed".to_owned());
    }
    report.violations.extend(remaining);
    for (pointer, expected) in &fixture.expect {
        if raw
            .result
            .as_ref()
            .and_then(|result| result.pointer(pointer))
            != Some(expected)
        {
            report
                .violations
                .push(format!("result assertion failed at {pointer}"));
        }
    }
    if let Some(expected) = &fixture.snapshot {
        let mut expected = expected.clone();
        let mut actual = raw.result.clone().unwrap_or(Value::Null);
        for pointer in &fixture.ignore_paths {
            match (actual.pointer_mut(pointer), expected.pointer_mut(pointer)) {
                (Some(actual), Some(expected)) => {
                    *actual = Value::Null;
                    *expected = Value::Null;
                }
                _ => report
                    .violations
                    .push(format!("snapshot ignore path missing: {pointer}")),
            }
        }
        if actual != expected {
            report
                .violations
                .push("result snapshot mismatch".to_owned());
        }
    }
    report.passed = report.violations.is_empty();
    report
}
