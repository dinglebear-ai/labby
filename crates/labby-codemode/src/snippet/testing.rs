//! Deterministic snippet tests on the production Code Mode sandbox.
//!
//! This module is independent of gateway configuration. Its fixture host owns
//! finite JSON responses; scoped read-only execution denies local providers,
//! artifacts, nested snippets and any tool absent from the fixture catalog.
mod assertions;
mod host;
mod model;
#[cfg(test)]
mod tests;

use super::store::{ResolvedSnippet, code_for_snippet, merge_snippet_input};
use crate::error::ToolError;
use crate::{CodeModeBroker, CodeModeCaller, CodeModeExecutedCall, CodeModeSurface};
use host::FixtureHost;
pub use model::{MockCall, MockError, SnippetTestCase, TestBudgets};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Instant;

/// Bounded diagnostic report, with no raw tool parameters or response fixtures.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SnippetTestReport {
    /// Canonical snippet name.
    pub name: String,
    /// True only when execution, fixtures, assertions and all budgets pass.
    pub passed: bool,
    /// Execution mode; offline tests always report mock.
    pub mode: String,
    /// Measurements from the actual runtime and its unshaped result.
    pub metrics: TestMetrics,
    /// At most 32 diagnostics. Values are not echoed in assertion messages.
    pub failures: Vec<String>,
    /// Additional diagnostics omitted from this bounded report.
    pub omitted_failures: usize,
    /// Result is omitted when it violates the output budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}
/// Runtime measurements; estimated tokens are an approximation, not billing.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TestMetrics {
    /// Total elapsed wall time in milliseconds.
    pub elapsed_ms: u128,
    /// Number of attempted calls in the sandbox trace.
    pub tool_calls: usize,
    /// Failed attempted calls, including deliberately simulated failures.
    pub failed_calls: usize,
    /// UTF-8 bytes in the raw result.
    pub output_bytes: usize,
    /// Approximate estimate: UTF-8 bytes divided by four, not tokenizer output.
    pub estimated_tokens: usize,
    /// Counts keyed by exact upstream::tool identifiers.
    pub calls_by_tool: BTreeMap<String, usize>,
    /// Whether runtime display shaping changed the raw result.
    pub result_changed: bool,
}

/// Run one fixture test without reading gateway configuration or calling upstreams.
pub async fn run_mock(
    snippet: &ResolvedSnippet,
    case: &SnippetTestCase,
) -> Result<SnippetTestReport, ToolError> {
    case.validate()?;
    let started = Instant::now();
    let code = code_for_snippet(snippet)?;
    let input = merge_snippet_input(snippet, case.input.clone())?;
    let input = serde_json::to_string(&input)
        .map_err(|_| model::invalid("cannot serialize snippet input"))?;
    let wrapped = format!("async () => {{ return await (\n{code}\n)({input}); }}");
    let host = FixtureHost::new(case)?;
    let scope = snippet
        .tools
        .as_ref()
        .map_or_else(|| host.scope.clone(), |t| t.intersect(&host.scope));
    let outcome = CodeModeBroker::new(Some(&host))
        .execute_with_raw_response(
            &wrapped,
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            host.config.clone(),
            scope,
            None,
        )
        .await;
    let elapsed_ms = started.elapsed().as_millis();
    let (result, calls, changed, mut failures) = match outcome {
        Ok(outcome) => {
            let changed = outcome.raw_response.result != outcome.display_response.result;
            (
                outcome.raw_response.result,
                outcome.raw_response.calls,
                changed,
                Vec::new(),
            )
        }
        Err(error) => (
            None,
            error.calls().to_vec(),
            false,
            vec![format!("execution failed: {}", error.kind())],
        ),
    };
    host.shutdown().await;
    let metrics = measure(elapsed_ms, &calls, result.as_ref(), changed);
    failures.extend(host.failures(metrics.failed_calls));
    failures.extend(assertions::check_result(case, result.as_ref()));
    if metrics.elapsed_ms > u128::from(case.budgets.wall_clock_ms) {
        failures.push("wall-clock budget exceeded".to_string());
    }
    if metrics.tool_calls > case.budgets.tool_calls {
        failures.push("tool-call budget exceeded".to_string());
    }
    if metrics.output_bytes > case.budgets.output_bytes {
        failures.push("output-byte budget exceeded".to_string());
    }
    if changed {
        failures.push("runtime changed or truncated the result".to_string());
    }
    let passed = failures.is_empty();
    let omitted_failures = failures.len().saturating_sub(32);
    failures.truncate(32);
    let result = if metrics.output_bytes > case.budgets.output_bytes {
        None
    } else {
        result
    };
    Ok(SnippetTestReport {
        name: snippet.name.clone(),
        passed,
        mode: "mock".to_string(),
        metrics,
        failures,
        omitted_failures,
        result,
    })
}
fn measure(
    elapsed_ms: u128,
    calls: &[CodeModeExecutedCall],
    result: Option<&Value>,
    result_changed: bool,
) -> TestMetrics {
    let output_bytes = result.map_or(0, |v| v.to_string().len());
    let mut calls_by_tool = BTreeMap::new();
    for call in calls {
        *calls_by_tool.entry(call.id.clone()).or_insert(0) += 1;
    }
    TestMetrics {
        elapsed_ms,
        tool_calls: calls.len(),
        failed_calls: calls.iter().filter(|c| !c.ok).count(),
        output_bytes,
        estimated_tokens: output_bytes.div_ceil(4),
        calls_by_tool,
        result_changed,
    }
}
