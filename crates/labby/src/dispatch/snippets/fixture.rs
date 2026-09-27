//! Offline adapter: no gateway initialization, no upstream authority.

use super::store::{builtin_snippet_dir, resolve_snippet};
use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::code_mode::{
    CodeModeBroker, CodeModeCaller, CodeModeSurface, ToolScope,
};
use labby_codemode::snippet::harness::{self, SnippetFixture};
use serde_json::{Value, json};

pub(super) async fn run(
    name: &str,
    fixture: &SnippetFixture,
    input: Value,
    caller: CodeModeCaller,
    surface: CodeModeSurface,
) -> Result<Value, ToolError> {
    let snippet = resolve_snippet(
        &crate::dispatch::helpers::lab_home(),
        &builtin_snippet_dir(),
        name,
    )?;
    let code = harness::prepare(
        &snippet,
        fixture,
        if input.is_null() { json!({}) } else { input },
    )?;
    let config = labby_runtime::CodeModeConfig {
        timeout_ms: fixture.budgets.wall_clock_ms,
        ..Default::default()
    };
    let broker = CodeModeBroker::<crate::dispatch::gateway::manager::GatewayManager>::new(None);
    // This is the containment boundary. The lexical JS mock cannot grant
    // access to live tools, local providers, artifact writes, or resources.
    let scope = ToolScope::scoped_namespaces(Vec::new(), Vec::new()).read_only();
    let start = std::time::Instant::now();
    let outcome = broker
        .execute_with_raw_response(&code, caller, surface, config, scope, None)
        .await
        .map_err(labby_codemode::CodeModeExecutionError::into_contract_tool_error)?;
    let response = outcome.raw_response;
    let external_calls = response.calls.len() + response.artifacts.len();
    let mut report = harness::report(
        fixture,
        response.result.as_ref().unwrap_or(&Value::Null),
        u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        external_calls,
    );
    report["name"] = json!(name);
    Ok(report)
}
