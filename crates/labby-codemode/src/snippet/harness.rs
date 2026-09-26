//! Offline snippet tests using the production QuickJS runner and a fixture-only host.
//! No gateway is constructed and unmatched calls never fall back to live tools.

mod fixture_host;
mod report;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod triage_tests;

use std::collections::BTreeMap;
use std::time::Instant;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::store::{ResolvedSnippet, code_for_snippet, merge_snippet_input};
use crate::error::ToolError;
use crate::{CodeModeBroker, CodeModeCaller, CodeModeSurface};
use fixture_host::FixtureHost;
use labby_runtime::CodeModeConfig;
pub use report::{SnippetTestMetrics, SnippetTestReport, assess_response};

/// Maximum inline fixture size accepted before deserializing or running it.
pub const MAX_FIXTURE_BYTES: usize = 1024 * 1024;

/// Explicit performance ceilings, measured before model-facing result shaping.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SnippetTestBudgets {
    /// Maximum elapsed execution time in milliseconds (1 through 30,000).
    pub wall_clock_ms: u64,
    /// Maximum attempted tool calls (1 through 512).
    pub tool_calls: u64,
    /// Maximum UTF-8 bytes of the serialized result (1 through 1 MiB).
    pub output_bytes: usize,
}

impl Default for SnippetTestBudgets {
    fn default() -> Self {
        Self {
            wall_clock_ms: 20_000,
            tool_calls: 40,
            output_bytes: 16_000,
        }
    }
}

impl SnippetTestBudgets {
    /// Reject invalid limits instead of silently changing the requested test.
    pub fn validate(&self) -> Result<(), ToolError> {
        if !(1..=30_000).contains(&self.wall_clock_ms)
            || !(1..=512).contains(&self.tool_calls)
            || !(1..=MAX_FIXTURE_BYTES).contains(&self.output_bytes)
        {
            return Err(invalid(
                "budgets require wall_clock_ms=1..30000, tool_calls=1..512, output_bytes=1..1048576",
            ));
        }
        Ok(())
    }
}

/// One deterministic tool response, either JSON (including null) or an error.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FixtureResponse {
    /// JSON returned by a successful fake call.
    Returns(Value),
    /// Structured failure injected at the normal callTool boundary.
    Error(FixtureError),
}

/// A deliberately injected upstream failure.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixtureError {
    /// Stable error kind, for example upstream_timeout.
    pub kind: String,
    /// Synthetic diagnostic; do not include credentials or personal data.
    pub message: String,
}

/// Match a call by exact tool id and recursive parameter subset.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixtureCall {
    /// Exact namespace::tool identifier.
    pub tool: String,
    /// Expected parameter subset; arrays and scalars match exactly.
    #[serde(default = "empty_object")]
    pub params: Value,
    /// Deterministic response for each matching call.
    pub response: FixtureResponse,
    /// Exact required use count, bounded to 1 through 512.
    #[serde(default = "one")]
    pub times: usize,
}

/// Portable inline JSON fixture. All expected calls must be consumed.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnippetFixture {
    /// Fake tool rules, in match precedence order.
    pub calls: Vec<FixtureCall>,
    /// Expected result values addressed by RFC 6901 JSON pointers.
    #[serde(default)]
    pub expect: BTreeMap<String, Value>,
    /// Optional exact snapshot of the complete result.
    #[serde(default)]
    pub snapshot: Option<Value>,
    /// Explicit JSON pointers replaced with null on both snapshot sides.
    #[serde(default)]
    pub ignore_paths: Vec<String>,
    /// Number of deliberately expected failed calls; default zero.
    #[serde(default)]
    pub expected_failures: usize,
    /// Performance ceilings for this case.
    #[serde(default)]
    pub budgets: SnippetTestBudgets,
}

fn one() -> usize {
    1
}
fn empty_object() -> Value {
    serde_json::json!({})
}

pub(super) fn invalid(message: &str) -> ToolError {
    ToolError::InvalidParam {
        message: message.to_owned(),
        param: "fixture".to_owned(),
    }
}

impl SnippetFixture {
    /// Parse a bounded fixture without reading paths or contacting any service.
    pub fn from_value(value: Value) -> Result<Self, ToolError> {
        let bytes = serde_json::to_vec(&value).map_err(|_| invalid("fixture must be JSON"))?;
        if bytes.len() > MAX_FIXTURE_BYTES {
            return Err(invalid("fixture exceeds 1 MiB"));
        }
        let fixture: Self =
            serde_json::from_value(value).map_err(|e| invalid(&format!("invalid fixture: {e}")))?;
        fixture.validate()?;
        Ok(fixture)
    }

    /// Check rules and budgets before creating a runner.
    pub fn validate(&self) -> Result<(), ToolError> {
        self.budgets.validate()?;
        if self.calls.len() > 512 || self.expected_failures > 512 {
            return Err(invalid("fixture supports at most 512 rules and failures"));
        }
        if self.expect.is_empty() && self.snapshot.is_none() {
            return Err(invalid("fixture requires result assertions or a snapshot"));
        }
        for path in self.expect.keys().chain(self.ignore_paths.iter()) {
            if !path.is_empty() && !path.starts_with('/') {
                return Err(invalid("assertion and ignore paths must be JSON pointers"));
            }
        }
        if !self.ignore_paths.is_empty() && self.snapshot.is_none() {
            return Err(invalid("ignore_paths requires a snapshot"));
        }
        let required_calls = self
            .calls
            .iter()
            .fold(0usize, |total, call| total.saturating_add(call.times));
        if required_calls > 512 {
            return Err(invalid("fixture requires more than 512 calls"));
        }
        let injected_errors: usize = self
            .calls
            .iter()
            .filter(|call| matches!(call.response, FixtureResponse::Error(_)))
            .map(|call| call.times)
            .sum();
        if injected_errors != self.expected_failures {
            return Err(invalid(
                "expected_failures must equal the number of deliberately injected error responses",
            ));
        }
        for call in &self.calls {
            let Some((namespace, tool)) = crate::types::split_namespaced_id(&call.tool) else {
                return Err(invalid("fixture tools must use namespace::tool"));
            };
            if matches!(
                namespace,
                "lab" | "__lab_internal" | "state" | "git" | "openapi" | "snippet"
            ) || namespace.trim() != namespace
                || tool.trim() != tool
                || call.tool != format!("{namespace}::{tool}")
            {
                return Err(invalid(
                    "fixture tools cannot use reserved namespaces or whitespace",
                ));
            }
            if !call.params.is_object() || !(1..=512).contains(&call.times) {
                return Err(invalid(
                    "fixture params must be objects and times must be 1..512",
                ));
            }
        }
        Ok(())
    }
}

/// Run a resolved snippet with fake upstreams and the real bounded sandbox.
///
/// Fixtures never carry gateway credentials. An explicit read-only tool scope
/// blocks artifact writes and runner-local providers, including for empty cases.
pub async fn test_fixture(
    snippet: &ResolvedSnippet,
    input: Value,
    fixture: SnippetFixture,
) -> Result<SnippetTestReport, ToolError> {
    fixture.validate()?;
    let source = code_for_snippet(snippet)?;
    let input = merge_snippet_input(snippet, input)?;
    // Parse JSON rather than inserting an object literal: "__proto__" must
    // remain an ordinary own property, not change the argument's prototype.
    let input_json = serde_json::to_string(&input.to_string())
        .map_err(|_| invalid("unable to serialize snippet input"))?;
    let source =
        format!("async () => {{ return await (\n{source}\n)(JSON.parse({input_json})); }}");
    let config = CodeModeConfig {
        timeout_ms: fixture.budgets.wall_clock_ms,
        trace_params: false,
        ..CodeModeConfig::default()
    };
    let host = FixtureHost::new(fixture.calls.clone(), config.clone())?;
    let scope = host.scope();
    let scope = snippet
        .tools
        .as_ref()
        .map_or_else(|| scope.clone(), |tools| tools.intersect(&scope))
        .read_only();
    let broker = CodeModeBroker::new(Some(&host));
    let started = Instant::now();
    let outcome = broker
        .execute_with_raw_response(
            &source,
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            config,
            scope,
            None,
        )
        .await;
    let elapsed_ms = started.elapsed().as_millis();
    Ok(report::fixture_report(
        &snippet.name,
        &fixture,
        outcome,
        elapsed_ms,
        host.remaining()?,
    ))
}
