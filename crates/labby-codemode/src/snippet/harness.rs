//! Deterministic snippet tests in the production parser and isolated runner.
use super::store::{
    ResolvedSnippet, code_for_snippet, merge_snippet_input, wrap_snippet_with_input_bounded,
};
use crate::error::ToolError;
use crate::{CodeModeBroker, CodeModeCaller, CodeModeConfig, CodeModeSurface, ToolScope};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Instant;

mod evaluate;
mod offline;
use evaluate::evaluate;
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

/// Synthetic nested snippet invocation; never resolves a real snippet.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixtureSnippet {
    /// Exact snippet name.
    pub name: String,
    /// Top-level input subset to match.
    #[serde(default)]
    pub r#match: Option<Value>,
    /// Synthetic nested output.
    #[serde(default)]
    pub result: Value,
    /// Synthetic rejection.
    #[serde(default)]
    pub error: Option<FixtureError>,
    /// Required consumption count.
    #[serde(default = "one")]
    pub times: usize,
}

/// Expected artifact write; data remains in memory and no file is created.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixtureArtifact {
    /// Exact relative artifact path.
    pub path: String,
    /// Expected content type, when supplied.
    #[serde(default)]
    pub content_type: Option<String>,
    /// Required literal content fragments.
    #[serde(default)]
    pub contains: Vec<String>,
    /// Required consumption count.
    #[serde(default = "one")]
    pub times: usize,
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
    /// Optional saved tool schemas for offline argument and response checks.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub schemas: BTreeMap<String, super::schemas::FixtureSchemas>,
    /// Fixture input defaults; explicit caller parameters take precedence.
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
    /// Synthetic nested snippet results, never real resolution.
    #[serde(default)]
    pub snippets: Vec<FixtureSnippet>,
    /// Expected writes with synthetic receipts and no filesystem access.
    #[serde(default)]
    pub artifacts: Vec<FixtureArtifact>,
    /// Synthetic tool responses.
    #[serde(default)]
    pub calls: Vec<FixtureCall>,
    /// Equality assertions keyed by JSON Pointer.
    #[serde(default)]
    pub expect: BTreeMap<String, Value>,
    /// JSON Pointers that must be absent, not merely null.
    #[serde(default)]
    pub absent: Vec<String>,
    /// Optional complete normalized output snapshot, including explicit JSON null.
    #[serde(
        default,
        deserialize_with = "present_snapshot",
        skip_serializing_if = "Option::is_none"
    )]
    pub snapshot: Option<Value>,
    /// JSON Pointers replaced by null on both sides of snapshot comparison.
    #[serde(default)]
    pub ignore_paths: Vec<String>,
    /// Resource limits.
    #[serde(default)]
    pub budgets: FixtureBudgets,
}

fn present_snapshot<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
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
    /// Output, serialized as null when it exceeds the output budget.
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
        if self.calls.len() + self.snippets.len() + self.artifacts.len() > 512
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
        if self.schemas.len() > 128 {
            return Err(invalid("fixture exceeds schema contract limit"));
        }
        for (tool, contract) in &self.schemas {
            if contract
                .fingerprint
                .as_ref()
                .is_some_and(|saved| *saved != contract.contract_fingerprint())
            {
                return Err(invalid("saved schema fingerprint does not match contract"));
            }
            if !self.calls.iter().any(|rule| &rule.tool == tool) {
                return Err(invalid("fixture schema has no corresponding tool rule"));
            }
            for schema in [&contract.input_schema, &contract.output_schema]
                .into_iter()
                .flatten()
            {
                super::schemas::check_schema(schema)?;
            }
        }
        let mut schema_budget = super::schemas::validation_budget();
        for rule in &self.calls {
            if rule.error.is_none()
                && let Some(schema) = self
                    .schemas
                    .get(&rule.tool)
                    .and_then(|s| s.output_schema.as_ref())
            {
                super::schemas::validate_value_with_budget(
                    &rule.result,
                    schema,
                    &mut schema_budget,
                )
                .map_err(|_| {
                    if schema_budget.is_exhausted() {
                        invalid("fixture response schema validation work budget exceeded")
                    } else {
                        invalid(format!(
                            "fixture response violates output schema for {}",
                            rule.tool
                        ))
                    }
                })?;
            }
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
                || ["lab", "snippet", "state", "git", "openapi"].contains(&namespace)
                || rule
                    .tool
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == '*')
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
        for rule in &self.snippets {
            super::store::validate_snippet_name(&rule.name)?;
            if rule.r#match.as_ref().is_some_and(|v| !v.is_object())
                || rule.times == 0
                || rule.times > 512
                || (rule.error.is_some() && !rule.result.is_null())
            {
                return Err(invalid("invalid synthetic snippet rule"));
            }
            count += rule.times;
        }
        for rule in &self.artifacts {
            if rule.path.is_empty()
                || rule.path.len() > 1024
                || rule.path.starts_with('/')
                || rule.path.contains('\\')
                || rule
                    .path
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
                || rule.contains.len() > 64
                || rule.contains.iter().any(|part| part.len() > 16_000)
                || rule.times == 0
                || rule.times > 512
                || rule
                    .content_type
                    .as_ref()
                    .is_some_and(|value| value.len() > 256)
            {
                return Err(invalid("invalid synthetic artifact rule"));
            }
            count += rule.times;
        }
        if count > 512 || self.params.len() > 64 {
            return Err(invalid("fixture exceeds invocation or input limit"));
        }
        for pointer in self
            .expect
            .keys()
            .chain(self.absent.iter())
            .chain(self.ignore_paths.iter())
        {
            if pointer.len() > 256 || !valid_json_pointer(pointer) {
                return Err(invalid(
                    "assertions and ignore_paths must use JSON Pointers",
                ));
            }
        }
        Ok(())
    }
}

fn valid_json_pointer(pointer: &str) -> bool {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return false;
    }
    let mut chars = pointer.chars();
    while let Some(character) = chars.next() {
        if character == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}

#[derive(Deserialize)]
struct RawReport {
    contract_calls: Vec<ContractCall>,
    result: Value,
    exception: Option<String>,
    calls: Vec<FixtureTrace>,
    attempted: usize,
    unexpected: usize,
    max_in_flight: usize,
    unused: Vec<Value>,
}

#[derive(Deserialize)]
struct ContractCall {
    tool: String,
    params: Value,
}

/// Execute synthetic calls in the production QuickJS subprocess, without a gateway.
/// Tool calls, nested snippets, and artifact writes use explicit synthetic rules. The host scope denies global
/// bridge escapes, local providers, resources and artifact writes.
pub async fn run_fixture(
    snippet: &ResolvedSnippet,
    input: Value,
    fixture: &SnippetFixture,
) -> Result<SnippetFixtureReport, ToolError> {
    run_fixture_with_source_limit(snippet, input, fixture, crate::MAX_SOURCE_BYTES).await
}

/// Run a fixture while applying the same configured source ceiling as live
/// saved-snippet execution. Fixture data itself uses a separate bounded wrapper.
pub async fn run_fixture_with_source_limit(
    snippet: &ResolvedSnippet,
    input: Value,
    fixture: &SnippetFixture,
    max_source_bytes: usize,
) -> Result<SnippetFixtureReport, ToolError> {
    let started = Instant::now();
    fixture.validate()?;
    if let Some(declared) = &snippet.tools {
        for rule in &fixture.calls {
            if !declared.as_slice().contains(&rule.tool) {
                return Err(invalid(
                    "fixture tool is outside the snippet tool declaration",
                ));
            }
        }
    }
    let code = code_for_snippet(snippet)?;
    let mut fixture_input = serde_json::Map::from_iter(fixture.params.clone());
    match input {
        Value::Null => {}
        Value::Object(caller_input) => fixture_input.extend(caller_input),
        _ => return Err(invalid("snippet input must be an object")),
    }
    let input = merge_snippet_input(snippet, Value::Object(fixture_input))?;
    // Match live invocation admission before fixture data is embedded in the
    // larger isolated test wrapper.
    wrap_snippet_with_input_bounded(&code, &input, max_source_bytes.min(crate::MAX_SOURCE_BYTES))?;
    let encode = |v: &Value| serde_json::to_string(v).map_err(|e| invalid(e.to_string()));
    let code_json = encode(&Value::String(code))?;
    let fixture_json = encode(&serde_json::to_value(fixture).map_err(|e| invalid(e.to_string()))?)?;
    let input_json = encode(&input)?;
    let wrapped = format!(
        "async () => {{ return await ({WRAPPER})({code_json}, {fixture_json}, {input_json}, codemode.batch); }}"
    );
    let preparation_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let remaining_ms = fixture
        .budgets
        .wall_clock_ms
        .checked_sub(preparation_ms)
        .filter(|remaining| *remaining > 0)
        .ok_or_else(|| invalid("fixture validation deadline exceeded before execution"))?;
    let config = CodeModeConfig {
        timeout_ms: remaining_ms,
        max_source_bytes: 10 * crate::MAX_SOURCE_BYTES,
        trace_params: false,
        ..CodeModeConfig::default()
    };
    let host = offline::OfflineHost::new(config.clone())?;
    let execution = CodeModeBroker::new(Some(&host))
        .execute_fixture_with_raw_response(
            &wrapped,
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            config,
            ToolScope::scoped_namespaces(Vec::new(), Vec::new()).read_only(),
        )
        .await;
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
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    evaluate(&snippet.name, raw, fixture, elapsed_ms, escaped)
}
