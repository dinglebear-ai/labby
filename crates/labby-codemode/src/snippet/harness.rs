//! Fixture-backed snippet execution and assertions, independent of product surfaces.
//!
//! The generated program must run in the production Code Mode sandbox with no
//! host, a deny-all tool scope, and read-only access. Lexical mocks are a testing
//! convenience, not the security boundary. Unexpected calls never reach MCP.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::store::{ResolvedSnippet, code_for_snippet, merge_snippet_input, validate_snippet_body};
use super::tool_declarations::SnippetToolDeclarations;
use crate::error::ToolError;

#[cfg(test)]
#[path = "harness_tests.rs"]
mod tests;

fn object() -> Value {
    json!({})
}
fn once() -> usize {
    1
}

/// Explicit, bounded assertions for one offline snippet run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnippetFixture {
    /// Default input, overridden by explicit invocation input.
    #[serde(default = "object")]
    pub input: Value,
    /// Synthetic call results matched by exact tool and optional parameter subset.
    #[serde(default)]
    pub calls: Vec<FixtureCall>,
    /// JSON Pointer assertions; the empty pointer is a whole-result snapshot.
    #[serde(default)]
    pub expect: BTreeMap<String, Value>,
    /// JSON Pointers that must be absent from the result.
    #[serde(default)]
    pub absent: Vec<String>,
    /// Replace explicitly named volatile values before snapshot comparisons.
    #[serde(default)]
    pub normalize: BTreeMap<String, Value>,
    /// Minimum simultaneous pending mock calls, useful for batch regressions.
    #[serde(default)]
    pub min_parallel_calls: usize,
    /// Wall-clock, tool-count, and UTF-8 result-size ceilings.
    #[serde(default)]
    pub budgets: FixtureBudgets,
}

/// One expected mock invocation, optionally repeated a fixed number of times.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureCall {
    /// Exact upstream::tool ID. Reserved local capabilities are prohibited.
    pub tool: String,
    /// Top-level parameter subset, with deeply equal values; null matches any.
    #[serde(default, rename = "match")]
    pub params: Option<Value>,
    /// JSON result; omitted means explicit null.
    #[serde(default)]
    pub result: Value,
    /// Expected tool rejection instead of a result.
    #[serde(default)]
    pub error: Option<FixtureError>,
    /// Exact number of expected invocations of this rule.
    #[serde(default = "once")]
    pub times: usize,
}

/// A synthetic tool failure with no live upstream involved.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureError {
    /// Stable error kind presented to snippet error handling.
    pub kind: String,
    /// Synthetic, nonsensitive error message.
    pub message: String,
}

/// Per-fixture performance limits; larger than hard sandbox limits is rejected.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FixtureBudgets {
    /// Maximum execution time in milliseconds, at most 30 seconds.
    pub wall_clock_ms: u64,
    /// Maximum mock call attempts, including failures, at most 512.
    pub tool_calls: usize,
    /// Maximum serialized snippet result, not trace, measured in UTF-8 bytes.
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

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: "fixture".to_string(),
    }
}

fn valid_pointer(pointer: &str) -> bool {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return false;
    }
    let mut chars = pointer.chars();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}

impl SnippetFixture {
    /// Reject malformed, oversized, ambiguous, or unbounded fixtures before execution.
    pub fn validate(&self) -> Result<(), ToolError> {
        if !self.input.is_object() {
            return Err(invalid("fixture input must be an object"));
        }
        if !(1..=30_000).contains(&self.budgets.wall_clock_ms)
            || self.budgets.tool_calls > 512
            || !(1..=24_000).contains(&self.budgets.output_bytes)
            || self.calls.len() > 512
            || self.min_parallel_calls > self.budgets.tool_calls
        {
            return Err(invalid("fixture exceeds hard execution budgets"));
        }
        let unique: std::collections::BTreeSet<_> =
            self.calls.iter().map(|c| c.tool.clone()).collect();
        SnippetToolDeclarations::try_from(unique.into_iter().collect::<Vec<_>>())?;
        let mut count = 0usize;
        for call in &self.calls {
            if !(1..=512).contains(&call.times)
                || call.params.as_ref().is_some_and(|v| !v.is_object())
                || call.error.as_ref().is_some_and(|e| {
                    e.kind.len() > 128 || e.message.len() > 1024 || !call.result.is_null()
                })
            {
                return Err(invalid(
                    "invalid mock call count, match object, or error/result pair",
                ));
            }
            count = count
                .checked_add(call.times)
                .ok_or_else(|| invalid("fixture call count overflow"))?;
        }
        if count > self.budgets.tool_calls {
            return Err(invalid("expected calls exceed the fixture tool budget"));
        }
        for pointer in self
            .expect
            .keys()
            .chain(self.absent.iter())
            .chain(self.normalize.keys())
        {
            if !valid_pointer(pointer) {
                return Err(invalid("assertions must use valid JSON Pointers"));
            }
        }
        if self.normalize.contains_key("") {
            return Err(invalid("normalization cannot replace the entire result"));
        }
        Ok(())
    }
}

/// Compile a fixture wrapper using the existing snippet parser and input merger.
///
/// Execute only with no gateway host and a deny-all read-only scope. Literal
/// callTool calls and codemode.batch are supported; generated helpers are not.
pub fn prepare(
    snippet: &ResolvedSnippet,
    fixture: &SnippetFixture,
    overrides: Value,
) -> Result<String, ToolError> {
    fixture.validate()?;
    validate_snippet_body(&snippet.name, &snippet.body)?;
    let source = code_for_snippet(snippet)?;
    if let Some(declared) = &snippet.tools {
        if fixture
            .calls
            .iter()
            .any(|call| !declared.as_slice().contains(&call.tool))
        {
            return Err(invalid("fixture tool is not declared by the snippet"));
        }
    }
    let mut input = fixture.input.clone();
    let overrides = overrides
        .as_object()
        .ok_or_else(|| invalid("input overrides must be an object"))?;
    input
        .as_object_mut()
        .expect("validated object")
        .extend(overrides.clone());
    let input = merge_snippet_input(snippet, input)?;
    let fixture = serde_json::to_string(fixture).map_err(|e| invalid(e.to_string()))?;
    let source = serde_json::to_string(&source).map_err(|e| invalid(e.to_string()))?;
    let input = serde_json::to_string(&input).map_err(|e| invalid(e.to_string()))?;
    // JSON.parse preserves data keys such as __proto__; object literal embedding does not.
    let fixture = serde_json::to_string(&fixture).map_err(|e| invalid(e.to_string()))?;
    let input = serde_json::to_string(&input).map_err(|e| invalid(e.to_string()))?;
    let harness = include_str!("harness.js");
    let batch = include_str!("../preamble/batch.js");
    let code = format!(
        "async () => {{ const codemode = {{}}; {batch} const run = ({harness}); return await run({source}, JSON.parse({fixture}), JSON.parse({input}), codemode.batch); }}"
    );
    if code.len() > crate::MAX_SOURCE_BYTES {
        return Err(invalid("fixture wrapper exceeds Code Mode source budget"));
    }
    Ok(code)
}

/// Evaluate raw sandbox evidence, including caught unexpected calls and budgets.
///
/// Trace rows omit parameters and return payloads. Fixtures must use synthetic
/// data: this function is not an automatic PII or secret scrubber.
pub fn report(
    fixture: &SnippetFixture,
    evidence: &Value,
    elapsed_ms: u64,
    external_calls: usize,
) -> Value {
    let mut failures = Vec::<String>::new();
    let result = evidence.get("result").cloned().unwrap_or(Value::Null);
    let mut normalized = result.clone();
    for (pointer, replacement) in &fixture.normalize {
        if let Some(target) = normalized.pointer_mut(pointer) {
            *target = replacement.clone();
        } else {
            failures.push(format!("normalization pointer missing: {pointer}"));
        }
    }
    if !evidence.get("exception").is_some_and(Value::is_null) {
        failures.push("snippet raised an exception or harness evidence is missing".into());
    }
    let attempted = evidence
        .get("attempted")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    let unexpected = evidence
        .get("unexpected")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX);
    let parallel = evidence
        .get("max_in_flight")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if unexpected != 0 {
        failures.push("unexpected or over-budget mock calls".into());
    }
    if !evidence
        .get("unused")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        failures.push("expected mock calls were not consumed".into());
    }
    if external_calls != 0 {
        failures.push("snippet attempted a non-fixture capability".into());
    }
    if elapsed_ms > fixture.budgets.wall_clock_ms {
        failures.push("wall-clock budget exceeded".into());
    }
    if attempted > fixture.budgets.tool_calls as u64 {
        failures.push("tool-call budget exceeded".into());
    }
    if parallel < fixture.min_parallel_calls as u64 {
        failures.push("parallel-call assertion failed".into());
    }
    let output_bytes = serde_json::to_vec(&result).map_or(usize::MAX, |v| v.len());
    if output_bytes > fixture.budgets.output_bytes {
        failures.push("UTF-8 output budget exceeded".into());
    }
    for (pointer, expected) in &fixture.expect {
        if normalized.pointer(pointer) != Some(expected) {
            failures.push(format!("result assertion failed: {pointer}"));
        }
    }
    for pointer in &fixture.absent {
        if normalized.pointer(pointer).is_some() {
            failures.push(format!("unexpected result field: {pointer}"));
        }
    }
    json!({
        "passed": failures.is_empty(), "mode": "mock", "failures": failures,
        "metrics": {"elapsed_ms": elapsed_ms, "tool_calls": attempted, "max_parallel_calls": parallel,
            "output_bytes": output_bytes, "estimated_tokens": output_bytes.div_ceil(4), "external_calls": external_calls},
        "result": if output_bytes <= fixture.budgets.output_bytes { result } else { Value::Null },
        "exception": evidence.get("exception"),
        "calls": evidence.get("calls"), "unused": evidence.get("unused")
    })
}
