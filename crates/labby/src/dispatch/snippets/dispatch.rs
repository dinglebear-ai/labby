use serde::Deserialize;
use serde_json::{Value, json};

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::code_mode::{
    CodeModeBroker, CodeModeCaller, CodeModeSourceLookup, CodeModeSurface, ToolScope,
};
use crate::dispatch::helpers::{action_schema, help_payload, lab_home, require_str, to_json};
use labby_codemode::snippet::harness::{
    SnippetFixture, SnippetTestBudgets, assess_response, test_fixture,
};
use labby_codemode::{CodeModeExecutionResponse, MAX_SOURCE_BYTES};

use super::catalog::ACTIONS;
use super::store::{
    builtin_snippet_dir, code_for_snippet, create_promoted_user_snippet, create_user_snippet,
    list_snippets, merge_snippet_input, remove_user_snippet, resolve_snippet,
    validate_snippet_body, validate_snippet_name,
};

#[derive(Debug, Deserialize)]
struct CreateParams {
    name: String,
    body: String,
    description: Option<String>,
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize)]
struct ExecParams {
    name: Option<String>,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    all: bool,
}

#[derive(Debug, Deserialize)]
struct TestParams {
    #[serde(flatten)]
    exec: ExecParams,
    #[serde(default)]
    live: bool,
    fixture: Option<Value>,
    #[serde(default)]
    budgets: SnippetTestBudgets,
}

#[derive(Debug, Deserialize)]
struct ValidateParams {
    name: Option<String>,
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PromoteParams {
    execution_id: String,
    name: String,
    description: Option<String>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    shadow_builtin: bool,
}

#[derive(Debug, Clone)]
pub struct SnippetDispatchContext {
    pub actor_key: Option<String>,
    pub is_admin: bool,
    pub route_scope: String,
    pub capability_filter_fingerprint: String,
    /// Caller/route authority inherited by saved snippet execution. A snippet's
    /// own declaration may only narrow this scope, never widen it.
    pub execution_scope: ToolScope,
    /// Preserve the initiating surface's identity/capabilities all the way into
    /// Code Mode. Treating remote snippet calls as trusted-local would erase
    /// runtime ownership and OAuth-subject semantics.
    pub execution_caller: CodeModeCaller,
    pub execution_surface: CodeModeSurface,
}

struct SnippetExecutionOutcome {
    raw_response: CodeModeExecutionResponse,
    display_response: CodeModeExecutionResponse,
}

impl SnippetDispatchContext {
    #[must_use]
    pub fn trusted_local() -> Self {
        Self {
            actor_key: None,
            is_admin: true,
            route_scope: "root".to_string(),
            capability_filter_fingerprint: ToolScope::default().fingerprint(),
            execution_scope: ToolScope::default(),
            execution_caller: CodeModeCaller::TrustedLocal,
            execution_surface: CodeModeSurface::Cli,
        }
    }
}

pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    let manager = crate::dispatch::gateway::current_gateway_manager();
    dispatch_inner(manager.as_deref(), action, params, None).await
}

pub async fn dispatch_with_manager_and_context(
    manager: &crate::dispatch::gateway::manager::GatewayManager,
    action: &str,
    params: Value,
    dispatch_context: Option<SnippetDispatchContext>,
) -> Result<Value, ToolError> {
    dispatch_inner(Some(manager), action, params, dispatch_context).await
}

async fn dispatch_inner(
    manager: Option<&crate::dispatch::gateway::manager::GatewayManager>,
    action: &str,
    params: Value,
    dispatch_context: Option<SnippetDispatchContext>,
) -> Result<Value, ToolError> {
    let execution_scope = dispatch_context
        .as_ref()
        .map(|context| context.execution_scope.clone())
        .unwrap_or_default();
    let execution_caller = dispatch_context
        .as_ref()
        .map(|context| context.execution_caller.clone())
        .unwrap_or(CodeModeCaller::TrustedLocal);
    let execution_surface = dispatch_context
        .as_ref()
        .map(|context| context.execution_surface)
        .unwrap_or(CodeModeSurface::Cli);
    match action {
        "help" => Ok(help_payload("snippets", ACTIONS)),
        "schema" => {
            let a = require_str(&params, "action")?;
            action_schema(ACTIONS, a)
        }
        "snippets.list" => {
            let snippets = list_snippets(&lab_home(), &builtin_snippet_dir())?;
            to_json(json!({ "snippets": snippets }))
        }
        "snippets.get" => {
            let name = require_str(&params, "name")?;
            to_json(resolve_snippet(&lab_home(), &builtin_snippet_dir(), &name)?)
        }
        "snippets.create" => {
            let params: CreateParams = parse_params(params)?;
            to_json(create_user_snippet(
                &lab_home(),
                &params.name,
                &params.body,
                params.description.as_deref(),
                params.force,
            )?)
        }
        "snippets.promote" => {
            let params: PromoteParams = parse_params(params)?;
            promote_snippet(manager, params, dispatch_context).await
        }
        "snippets.validate" => {
            let params: ValidateParams = parse_params(params)?;
            validate_snippet(params.name.as_deref(), params.body.as_deref())
        }
        "snippets.remove" => {
            let name = require_str(&params, "name")?;
            to_json(remove_user_snippet(
                &lab_home(),
                &builtin_snippet_dir(),
                &name,
            )?)
        }
        "snippets.exec" => {
            let params: ExecParams = parse_params(params)?;
            let Some(name) = params.name else {
                return Err(missing_param("missing required parameter `name`", "name"));
            };
            let outcome = execute_snippet_outcome(
                manager,
                &name,
                params.params,
                &execution_scope,
                &execution_caller,
                execution_surface,
            )
            .await?;
            to_json(outcome.display_response)
        }
        "snippets.test" => {
            let test: TestParams = parse_params(params)?;
            test.budgets.validate()?;
            let params = test.exec;
            if test.live == test.fixture.is_some() {
                return Err(ToolError::InvalidParam {
                    message: "choose exactly one of fixture (offline) or live: true".to_owned(),
                    param: "live".to_owned(),
                });
            }
            if params.all && (params.name.is_some() || test.fixture.is_some()) {
                return Err(ToolError::InvalidParam {
                    message: "all is only supported for explicit live tests without a name"
                        .to_owned(),
                    param: "all".to_owned(),
                });
            }
            if params.all && !params.params.is_null() && params.params != json!({}) {
                return Err(ToolError::InvalidParam {
                    message: "all uses each snippet's defaults; explicit params are not supported"
                        .to_owned(),
                    param: "params".to_owned(),
                });
            }
            if params.all {
                return test_all_snippets(
                    manager,
                    &execution_scope,
                    &execution_caller,
                    execution_surface,
                    &test.budgets,
                )
                .await;
            }
            let name = params
                .name
                .ok_or_else(|| missing_param("missing required parameter name", "name"))?;
            if let Some(fixture) = test.fixture {
                let fixture = SnippetFixture::from_value(fixture)?;
                let snippet = resolve_snippet(&lab_home(), &builtin_snippet_dir(), &name)?;
                return to_json(test_fixture(&snippet, params.params, fixture).await?);
            }
            let started = std::time::Instant::now();
            let outcome = execute_snippet_outcome(
                manager,
                &name,
                params.params,
                &execution_scope,
                &execution_caller,
                execution_surface,
            )
            .await?;
            snippet_test_result(name, outcome, started.elapsed().as_millis(), &test.budgets)
        }
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action `{unknown}` for service `snippets`"),
            valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        }),
    }
}

async fn promote_snippet(
    manager: Option<&crate::dispatch::gateway::manager::GatewayManager>,
    params: PromoteParams,
    dispatch_context: Option<SnippetDispatchContext>,
) -> Result<Value, ToolError> {
    validate_snippet_name(&params.name)?;
    let manager = manager.ok_or_else(|| ToolError::Sdk {
        sdk_kind: "gateway_unavailable".to_string(),
        message: "snippets.promote requires the live gateway manager source store".to_string(),
    })?;
    let context = dispatch_context.unwrap_or_else(SnippetDispatchContext::trusted_local);
    let source = manager
        .resolve_code_mode_source(
            &params.execution_id,
            &CodeModeSourceLookup {
                actor_key: context.actor_key,
                is_admin: context.is_admin,
                route_scope: context.route_scope,
                capability_filter_fingerprint: context.capability_filter_fingerprint,
            },
        )
        .await?;
    let info = create_promoted_user_snippet(
        &lab_home(),
        &builtin_snippet_dir(),
        &params.name,
        &source.code,
        params.description.as_deref(),
        params.force,
        params.shadow_builtin,
    )?;
    to_json(json!({
        "execution_id": source.execution_id,
        "source": {
            "created_at_ms": source.created_at_ms,
            "is_admin": source.is_admin,
            "surface": match source.surface {
                CodeModeSurface::Mcp => "mcp",
                CodeModeSurface::Cli => "cli",
                CodeModeSurface::Api => "api",
            },
            "route_scope": source.route_scope,
        },
        "snippet": info,
    }))
}

fn validate_snippet(name: Option<&str>, body: Option<&str>) -> Result<Value, ToolError> {
    if let Some(body) = body {
        let name = name.ok_or_else(|| {
            missing_param(
                "missing required parameter `name` when validating a body",
                "name",
            )
        })?;
        validate_snippet_name(name)?;
        validate_snippet_body(name, body)?;
        return to_json(json!({
            "valid": true,
            "name": name,
            "mode": "body",
        }));
    }

    let name =
        name.ok_or_else(|| missing_param("missing required parameter `name` or `body`", "name"))?;
    let snippet = resolve_snippet(&lab_home(), &builtin_snippet_dir(), name)?;
    let _code = code_for_snippet(&snippet)?;
    to_json(json!({
        "valid": true,
        "name": snippet.name,
        "mode": "existing",
        "source": snippet.source,
        "path": snippet.path,
    }))
}

async fn test_all_snippets(
    manager: Option<&crate::dispatch::gateway::manager::GatewayManager>,
    caller_scope: &ToolScope,
    caller: &CodeModeCaller,
    surface: CodeModeSurface,
    budgets: &SnippetTestBudgets,
) -> Result<Value, ToolError> {
    let snippets = list_snippets(&lab_home(), &builtin_snippet_dir())?;
    let mut results = Vec::with_capacity(snippets.len());
    for snippet in snippets {
        let started = std::time::Instant::now();
        match execute_snippet_outcome(
            manager,
            &snippet.name,
            Value::Object(Default::default()),
            caller_scope,
            caller,
            surface,
        )
        .await
        {
            Ok(outcome) => {
                results.push(snippet_test_result(
                    snippet.name,
                    outcome,
                    started.elapsed().as_millis(),
                    budgets,
                )?);
            }
            Err(error) => {
                results.push(json!({
                    "name": snippet.name,
                    "passed": false,
                    "error": error,
                }));
            }
        }
    }
    let passed = results.iter().all(|result| {
        result
            .get("passed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    });
    to_json(json!({
        "passed": passed,
        "results": results,
    }))
}

fn snippet_response_passed(response: &CodeModeExecutionResponse) -> bool {
    response.result.is_some()
        && response.calls.iter().all(|call| call.ok)
        && response
            .result
            .as_ref()
            .and_then(|result| result.get("ok"))
            .and_then(Value::as_bool)
            .unwrap_or(true)
}

fn snippet_test_result(
    name: String,
    outcome: SnippetExecutionOutcome,
    elapsed_ms: u128,
    budgets: &SnippetTestBudgets,
) -> Result<Value, ToolError> {
    let mut report = assess_response(
        &name,
        "live",
        &outcome.raw_response,
        &outcome.display_response,
        elapsed_ms,
        budgets,
        0,
    );
    if !snippet_response_passed(&outcome.raw_response) {
        report
            .violations
            .push("snippet or one of its calls reported failure".to_owned());
    }
    report.passed = report.violations.is_empty();
    to_json(report)
}

fn snippet_execution_scope(
    snippet: &super::store::ResolvedSnippet,
    caller_scope: &ToolScope,
) -> ToolScope {
    snippet
        .tools
        .as_ref()
        .map(|declared| declared.intersect(caller_scope))
        .unwrap_or_else(|| caller_scope.clone())
}

async fn execute_snippet_outcome(
    manager: Option<&crate::dispatch::gateway::manager::GatewayManager>,
    name: &str,
    input: Value,
    caller_scope: &ToolScope,
    caller: &CodeModeCaller,
    surface: CodeModeSurface,
) -> Result<SnippetExecutionOutcome, ToolError> {
    let owned_manager;
    let manager = if let Some(manager) = manager {
        manager
    } else {
        owned_manager = crate::dispatch::gateway::require_gateway_manager()?;
        owned_manager.as_ref()
    };
    let broker = CodeModeBroker::new(Some(manager));
    let config = manager.code_mode_config().await;
    let snippet = resolve_snippet(&lab_home(), &builtin_snippet_dir(), name)?;
    let code = code_for_snippet(&snippet)?;
    let input = merge_snippet_input(&snippet, input)?;
    let max_source_bytes = config.max_source_bytes.min(MAX_SOURCE_BYTES);
    let code = wrap_snippet_with_input_bounded(&code, &input, max_source_bytes)?;
    // Saved snippet source stays entirely on the execution plane. When the
    // snippet declares exact upstream dependencies, use that declaration to
    // narrow the caller/route authority instead of cold-probing every configured
    // upstream. Snippets without declarations inherit the caller scope exactly.
    let scope = snippet_execution_scope(&snippet, caller_scope);
    let outcome = broker
        .execute_with_raw_response(
            &code,
            caller.clone(),
            surface,
            config,
            scope,
            // Saved-snippet dispatch does not mint a durable execution id on
            // this path; `None` keeps `record_step` write-free here.
            None,
        )
        .await
        // Contract-preserving path: `into_tool_error` collapses to bare
        // kind + message, losing evidence/safety/original_kind and the refined
        // recovery metadata. `into_contract_tool_error` carries the full
        // CodeModeCallError contract through the dispatch ToolError so MCP,
        // HTTP, and CLI envelopes render the same fidelity as the direct Code
        // Mode MCP path (`code_mode_error_envelope`). The executed-calls trace
        // remains dispatch-internal and is intentionally not serialized here.
        .map_err(labby_codemode::CodeModeExecutionError::into_contract_tool_error)?;
    Ok(SnippetExecutionOutcome {
        raw_response: outcome.raw_response,
        display_response: outcome.display_response,
    })
}

fn wrap_snippet_with_input_bounded(
    code: &str,
    input: &Value,
    max_source_bytes: usize,
) -> Result<String, ToolError> {
    let input = serde_json::to_string(input).map_err(|e| ToolError::InvalidParam {
        message: format!("snippet params must be JSON-serializable: {e}"),
        param: "params".to_string(),
    })?;
    let input_literal = serde_json::to_string(&input).map_err(|e| ToolError::InvalidParam {
        message: format!("snippet params must be JSON-serializable: {e}"),
        param: "params".to_string(),
    })?;
    let wrapped = format!(
        "async () => {{\n  const __labSnippetInput = JSON.parse({input_literal});\n  return await ({code})(__labSnippetInput);\n}}"
    );
    if wrapped.len() > max_source_bytes {
        return Err(ToolError::InvalidParam {
            message: format!(
                "saved snippet invocation exceeds Code Mode source limit {max_source_bytes} bytes after serializing params ({} bytes)",
                wrapped.len()
            ),
            param: "params".to_string(),
        });
    }
    Ok(wrapped)
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ToolError> {
    serde_json::from_value(params).map_err(|e| ToolError::InvalidParam {
        message: format!("invalid snippets params: {e}"),
        param: "params".to_string(),
    })
}

fn missing_param(message: &str, param: &str) -> ToolError {
    ToolError::MissingParam {
        message: message.to_string(),
        param: param.to_string(),
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
