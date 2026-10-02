//! Fixture-first snippet testing shared by CLI, MCP and HTTP.
use super::dispatch::{SnippetDispatchContext, snippet_test_result};
use super::execution::execute_snippet_outcome;
use super::store::{builtin_snippet_dir, list_snippets, resolve_snippet};
use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::dispatch::helpers::lab_home;
use labby_codemode::snippet::harness::{
    MAX_FIXTURE_BYTES, SnippetFixture, run_fixture_with_source_limit,
};
use labby_codemode::{CodeModeCaller, CodeModeSurface, ToolScope};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::time::Instant;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestParams {
    name: Option<String>,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    live: bool,
    fixture: Option<SnippetFixture>,
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: "params".into(),
    }
}

impl TestParams {
    fn validate(&self) -> Result<(), ToolError> {
        if self.all == self.name.is_some() {
            return Err(invalid("provide a snippet name or all: true, not both"));
        }
        if self.fixture.is_some() && (self.live || self.all) {
            return Err(invalid("fixture cannot be combined with live or all"));
        }
        if let Some(fixture) = &self.fixture {
            fixture.validate()?;
        }
        Ok(())
    }
}

/// Test saved snippets without opening upstream connections unless live is explicit.
pub(super) async fn test(
    manager: Option<&GatewayManager>,
    params: Value,
    scope: &ToolScope,
    caller: &CodeModeCaller,
    surface: CodeModeSurface,
    source_limit_override: Option<usize>,
    context: Option<&SnippetDispatchContext>,
) -> Result<Value, ToolError> {
    let params: TestParams = serde_json::from_value(params).map_err(|e| invalid(e.to_string()))?;
    params.validate()?;
    if let Some(name) = &params.name {
        return test_one(
            manager,
            name,
            &params,
            scope,
            caller,
            surface,
            source_limit_override,
            context,
        )
        .await;
    }
    let names: BTreeSet<_> = list_snippets(&lab_home(), &builtin_snippet_dir())?
        .into_iter()
        .map(|snippet| snippet.name)
        .collect();
    if names.len() > 100 {
        return Err(invalid(
            "test --all is bounded to 100 snippets; test named subsets instead",
        ));
    }
    let mut results = Vec::new();
    for name in names {
        let mut report = match test_one(
            manager,
            &name,
            &params,
            scope,
            caller,
            surface,
            source_limit_override,
            context,
        )
        .await
        {
            Ok(report) => report,
            Err(error) => json!({"name": name, "passed": false, "error": error}),
        };
        // Bulk reports retain diagnostics/metrics, never multiply result payloads.
        if let Some(object) = report.as_object_mut() {
            object.remove("result");
            object.remove("response");
            object.remove("calls");
        }
        results.push(report);
    }
    let passed = !results.is_empty() && results.iter().all(|r| r["passed"] == true);
    Ok(json!({"passed":passed,"results":results}))
}

async fn test_one(
    manager: Option<&GatewayManager>,
    name: &str,
    params: &TestParams,
    scope: &ToolScope,
    caller: &CodeModeCaller,
    surface: CodeModeSurface,
    source_limit_override: Option<usize>,
    context: Option<&SnippetDispatchContext>,
) -> Result<Value, ToolError> {
    if !params.live {
        let snippet = resolve_snippet(&lab_home(), &builtin_snippet_dir(), name)?;
        let fixture = match &params.fixture {
            Some(fixture) => fixture.clone(),
            None => {
                let path = snippet.path.with_extension("test.json");
                let file = std::fs::File::open(&path).map_err(|error| invalid(format!(
                    "cannot read fixture {}: {error}; supply a fixture object or explicitly set live: true", path.display())))?;
                let mut bytes = Vec::new();
                file.take((MAX_FIXTURE_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|error| invalid(format!("cannot read fixture: {error}")))?;
                if bytes.len() > MAX_FIXTURE_BYTES {
                    return Err(invalid("fixture exceeds 512 KiB"));
                }
                serde_json::from_slice(&bytes)
                    .map_err(|error| invalid(format!("invalid fixture JSON: {error}")))?
            }
        };
        let max_source_bytes = match manager {
            Some(manager) => manager.code_mode_config().await.max_source_bytes,
            None => source_limit_override.unwrap_or(labby_codemode::MAX_SOURCE_BYTES),
        };
        let report = run_fixture_with_source_limit(
            &snippet,
            params.params.clone(),
            &fixture,
            max_source_bytes,
        )
        .await?;
        return serde_json::to_value(report).map_err(|error| invalid(error.to_string()));
    }
    let started = Instant::now();
    let outcome = execute_snippet_outcome(
        manager,
        name,
        params.params.clone(),
        scope,
        caller,
        surface,
        context,
    )
    .await?;
    let elapsed = started.elapsed().as_millis();
    let bytes = serde_json::to_vec(&outcome.raw_response.result)
        .map_err(|error| invalid(error.to_string()))?
        .len();
    let mut by_tool = BTreeMap::<String, usize>::new();
    for call in &outcome.raw_response.calls {
        *by_tool.entry(call.id.clone()).or_default() += 1;
    }
    let failed_calls = outcome
        .raw_response
        .calls
        .iter()
        .filter(|call| !call.ok)
        .count();
    let truncated = outcome.raw_response.result != outcome.display_response.result;
    let metrics = json!({"wall_clock_ms":elapsed,"tool_calls":outcome.raw_response.calls.len(),
        "calls_by_tool":by_tool,"failed_calls":failed_calls,"output_bytes":bytes,
        "estimated_tokens":bytes.div_ceil(4),"truncated":truncated});
    let has_result = outcome.raw_response.result.is_some();
    let mut report = snippet_test_result(name.to_owned(), outcome)?;
    let passed = report["passed"] == true && has_result && failed_calls == 0 && !truncated;
    report["passed"] = Value::Bool(passed);
    report["mode"] = json!("live");
    report["metrics"] = metrics;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_execution_requires_an_explicit_switch() {
        let params: TestParams = serde_json::from_value(json!({"name":"demo"})).unwrap();
        params.validate().unwrap();
        assert!(!params.live);
    }
    #[test]
    fn rejects_ambiguous_execution_modes_and_unknown_parameters() {
        for value in [
            json!({}),
            json!({"name":"demo","all":true}),
            json!({"name":"demo","live":true,"fixture":{}}),
            json!({"all":true,"fixture":{}}),
        ] {
            assert!(
                serde_json::from_value::<TestParams>(value)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        assert!(serde_json::from_value::<TestParams>(json!({"name":"demo","lve":true})).is_err());
    }
}
