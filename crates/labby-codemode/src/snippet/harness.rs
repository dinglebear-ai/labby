//! Deterministic snippet tests in the production parser and isolated runner.
use super::store::{ResolvedSnippet, code_for_snippet, merge_snippet_input};
use crate::error::ToolError;
use crate::{CodeModeBroker, CodeModeCaller, CodeModeConfig, CodeModeSurface, ToolScope};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Instant;

mod offline;
#[cfg(test)]
mod tests;
const WRAPPER: &str = include_str!("harness.js");
/// Maximum serialized fixture size, checked before runner invocation.
pub const MAX_FIXTURE_BYTES: usize = 512 * 1024;

/// A synthetic tool rejection.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixtureError {
    /// Stable synthetic error kind.
    pub kind: String,
    /// Synthetic message; never include credentials.
    pub message: String,
}

/// A bounded synthetic response rule, matched in declaration order.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixtureCall {
    /// Exact upstream::tool identifier.
    pub tool: String,
    /// Optional top-level parameter subset; nested values compare exactly.
    #[serde(default)]
    pub r#match: Option<Value>,
    /// Synthetic response, including null.
    #[serde(default)]
    pub result: Value,
    /// Synthetic rejection instead of a result.
    #[serde(default)]
    pub error: Option<FixtureError>,
    /// Required consumption count, default one.
    #[serde(default = "one")]
    pub times: usize,
}
const fn one() -> usize {
    1
}

/// Resource budgets enforced against raw execution results.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct FixtureBudgets {
    /// Wall-clock deadline, maximum 30 seconds.
    pub wall_clock_ms: u64,
    /// Maximum attempted synthetic calls, at most 512.
    pub tool_calls: usize,
    /// Maximum raw output bytes, at most 16,000.
    pub output_bytes: usize,
}
impl Default for FixtureBudgets {
    fn default() -> Self {
        Self {
            wall_clock_ms: 20_000,
            tool_calls: 40,
            output_bytes: 16_000,
        }
    }
}

/// Portable JSON fixture. Every response rule must be fully consumed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnippetFixture {
    /// Synthetic tool responses.
    #[serde(default)]
    pub calls: Vec<FixtureCall>,
    /// Equality assertions keyed by JSON Pointer.
    #[serde(default)]
    pub expect: BTreeMap<String, Value>,
    /// JSON Pointers that must be absent, not merely null.
    #[serde(default)]
    pub absent: Vec<String>,
    /// Optional complete normalized output snapshot.
    #[serde(default)]
    pub snapshot: Option<Value>,
    /// JSON Pointers replaced by null on both sides of snapshot comparison.
    #[serde(default)]
    pub ignore_paths: Vec<String>,
    /// Resource limits.
    #[serde(default)]
    pub budgets: FixtureBudgets,
}

/// Synthetic trace metadata; no parameters or response payloads.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FixtureTrace {
    /// Exact tool identifier.
    pub tool: String,
    /// Whether the synthetic response resolved rather than rejected.
    pub ok: bool,
    /// Matching rule index, null for unexpected calls.
    pub fixture_index: Option<usize>,
    /// Synthetic elapsed time, not a live upstream measurement.
    pub elapsed_ms: u64,
}

/// Measured fixture execution data. Synthetic latency is not a live benchmark.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FixtureMetrics {
    /// Wall-clock elapsed time.
    pub wall_clock_ms: u64,
    /// Attempted synthetic calls.
    pub tool_calls: usize,
    /// Serialized output bytes before shaping.
    pub output_bytes: usize,
    /// Bytes/4 estimate, not a tokenizer measurement.
    pub estimated_tokens: usize,
    /// Peak overlapping synthetic calls.
    pub max_in_flight: usize,
}

/// Shared CLI/API/MCP fixture test report.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SnippetFixtureReport {
    /// Canonical snippet name.
    pub name: String,
    /// Explicit mock-execution marker.
    pub mode: String,
    /// All assertions, rule consumption and budgets passed.
    pub passed: bool,
    /// Compact diagnostics without full fixture values.
    pub failures: Vec<String>,
    /// Actual runner measurements.
    pub metrics: FixtureMetrics,
    /// First 32 synthetic calls, in invocation order.
    pub calls: Vec<FixtureTrace>,
    /// Whether trace entries were omitted from this report.
    pub trace_truncated: bool,
    /// Output, omitted when it exceeds the output budget.
    pub result: Option<Value>,
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: "fixture".into(),
    }
}

impl SnippetFixture {
    /// Validate a fixture without executing JavaScript or contacting tools.
    pub fn validate(&self) -> Result<(), ToolError> {
        let size = serde_json::to_vec(self)
            .map_err(|e| invalid(e.to_string()))?
            .len();
        if size > MAX_FIXTURE_BYTES {
            return Err(invalid("fixture exceeds 512 KiB"));
        }
        if self.calls.len() > 512
            || self.expect.len() > 64
            || self.absent.len() > 64
            || self.ignore_paths.len() > 64
        {
            return Err(invalid("fixture exceeds rule or assertion limit"));
        }
        if !(1..=30_000).contains(&self.budgets.wall_clock_ms)
            || self.budgets.tool_calls > 512
            || !(1..=16_000).contains(&self.budgets.output_bytes)
        {
            return Err(invalid(
                "invalid fixture budgets: wall_clock_ms 1..30000, tool_calls 0..512, output_bytes 1..16000",
            ));
        }
        let mut count = 0usize;
        for rule in &self.calls {
            let Some((namespace, tool)) = rule.tool.split_once("::") else {
                return Err(invalid(
                    "fixture tools must use exact upstream::tool identifiers",
                ));
            };
            if namespace.is_empty()
                || tool.is_empty()
                || namespace.len() > 128
                || tool.len() > 128
                || namespace.starts_with("__")
                || ["lab", "state", "git", "openapi"].contains(&namespace)
                || rule.tool.contains([' ', '*', '\n'])
                || tool.contains("::")
            {
                return Err(invalid("invalid or reserved fixture tool identifier"));
            }
            if rule.r#match.as_ref().is_some_and(|v| !v.is_object()) {
                return Err(invalid("fixture match must be an object"));
            }
            if rule.times == 0 || rule.times > 512 {
                return Err(invalid("fixture times must be 1..512"));
            }
            count += rule.times;
            if count > 512 {
                return Err(invalid("fixture consumes more than 512 calls"));
            }
            if rule.error.is_some() && !rule.result.is_null() {
                return Err(invalid(
                    "fixture rule cannot return both a result and an error",
                ));
            }
        }
        for pointer in self
            .expect
            .keys()
            .chain(self.absent.iter())
            .chain(self.ignore_paths.iter())
        {
            if pointer.len() > 256 || (!pointer.is_empty() && !pointer.starts_with('/')) {
                return Err(invalid(
                    "assertions and ignore_paths must use JSON Pointers",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct RawReport {
    result: Value,
    exception: Option<String>,
    calls: Vec<FixtureTrace>,
    attempted: usize,
    unexpected: usize,
    max_in_flight: usize,
    unused: Vec<Value>,
}

/// Execute synthetic calls in the production QuickJS subprocess, without a gateway.
/// Only callTool and codemode.batch are mocked. The host scope denies global
/// bridge escapes, local providers, resources and artifact writes.
pub async fn run_fixture(
    snippet: &ResolvedSnippet,
    input: Value,
    fixture: &SnippetFixture,
) -> Result<SnippetFixtureReport, ToolError> {
    fixture.validate()?;
    let code = code_for_snippet(snippet)?;
    let input = merge_snippet_input(snippet, input)?;
    let encode = |v: &Value| serde_json::to_string(v).map_err(|e| invalid(e.to_string()));
    let code_json = encode(&Value::String(code))?;
    let fixture_json = encode(&serde_json::to_value(fixture).map_err(|e| invalid(e.to_string()))?)?;
    let input_json = encode(&input)?;
    let wrapped = format!(
        "async () => {{ return await ({WRAPPER})({code_json}, {fixture_json}, {input_json}, codemode.batch); }}"
    );
    let config = CodeModeConfig {
        timeout_ms: fixture.budgets.wall_clock_ms,
        max_source_bytes: crate::MAX_SOURCE_BYTES,
        trace_params: false,
        ..CodeModeConfig::default()
    };
    let host = offline::OfflineHost::new(config.clone())?;
    let started = Instant::now();
    let execution = CodeModeBroker::new(Some(&host))
        .execute_with_raw_response(
            &wrapped,
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            config,
            ToolScope::scoped_namespaces(Vec::new(), Vec::new()).read_only(),
            None,
        )
        .await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    host.shutdown().await;
    let response = execution
        .map_err(crate::CodeModeExecutionError::into_contract_tool_error)?
        .raw_response;
    let escaped = !response.calls.is_empty() || !response.artifacts.is_empty();
    let raw: RawReport = serde_json::from_value(
        response
            .result
            .ok_or_else(|| invalid("fixture runner returned no result"))?,
    )
    .map_err(|e| invalid(format!("invalid fixture runner response: {e}")))?;
    evaluate(&snippet.name, raw, fixture, elapsed_ms, escaped)
}

fn evaluate(
    name: &str,
    mut raw: RawReport,
    fixture: &SnippetFixture,
    elapsed: u64,
    escaped: bool,
) -> Result<SnippetFixtureReport, ToolError> {
    let mut failures = Vec::new();
    if let Some(exception) = raw.exception {
        failures.push(format!("snippet exception: {exception}"));
    }
    if escaped {
        failures.push("snippet attempted to use the real host bridge".into());
    }
    if raw.unexpected > 0 {
        failures.push(format!(
            "{} unexpected or over-budget calls",
            raw.unexpected
        ));
    }
    if !raw.unused.is_empty() {
        failures.push(format!(
            "{} fixture rules were not fully consumed",
            raw.unused.len()
        ));
    }
    if raw.result.get("ok").and_then(Value::as_bool) == Some(false)
        && fixture.expect.get("/ok") != Some(&Value::Bool(false))
    {
        failures.push("snippet returned ok: false".into());
    }
    let bytes = serde_json::to_vec(&raw.result)
        .map_err(|e| invalid(e.to_string()))?
        .len();
    if bytes > fixture.budgets.output_bytes {
        failures.push("output_bytes budget exceeded".into());
    }
    if elapsed > fixture.budgets.wall_clock_ms {
        failures.push("wall_clock_ms budget exceeded".into());
    }
    if raw.attempted > fixture.budgets.tool_calls {
        failures.push("tool_calls budget exceeded".into());
    }
    for (pointer, expected) in &fixture.expect {
        if raw.result.pointer(pointer) != Some(expected) {
            failures.push(format!("assertion failed at {pointer}"));
        }
    }
    for pointer in &fixture.absent {
        if raw.result.pointer(pointer).is_some() {
            failures.push(format!("expected absent path at {pointer}"));
        }
    }
    if let Some(snapshot) = &fixture.snapshot {
        let mut expected = snapshot.clone();
        let mut actual = raw.result.clone();
        for pointer in &fixture.ignore_paths {
            if let Some(v) = expected.pointer_mut(pointer) {
                *v = Value::Null;
            }
            if let Some(v) = actual.pointer_mut(pointer) {
                *v = Value::Null;
            }
        }
        if actual != expected {
            failures.push("normalized snapshot mismatch".into());
        }
    }
    let trace_truncated = raw.calls.len() > 32;
    raw.calls.truncate(32);
    Ok(SnippetFixtureReport {
        name: name.into(),
        mode: "mock".into(),
        passed: failures.is_empty(),
        failures,
        metrics: FixtureMetrics {
            wall_clock_ms: elapsed,
            tool_calls: raw.attempted,
            output_bytes: bytes,
            estimated_tokens: bytes.div_ceil(4),
            max_in_flight: raw.max_in_flight,
        },
        calls: raw.calls,
        trace_truncated,
        result: (bytes <= fixture.budgets.output_bytes).then_some(raw.result),
    })
}
