use serde::Deserialize;
use serde_json::{Value, json};

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::code_mode::{
    CodeModeCaller, CodeModeSourceLookup, CodeModeSurface, ToolScope,
};
use crate::dispatch::helpers::{action_schema, help_payload, lab_home, require_str, to_json};
use labby_codemode::CodeModeExecutionResponse;

use super::catalog::ACTIONS;
use super::execution::{execute_snippet_outcome, receipt_owner};
use super::store::{
    builtin_snippet_dir, code_for_snippet, create_promoted_user_snippet,
    create_user_snippet_checked, list_snippets_with_diagnostics, remove_user_snippet,
    resolve_snippet, validate_snippet_body, validate_snippet_name,
};

#[derive(Debug, Deserialize)]
struct CreateParams {
    name: String,
    body: String,
    description: Option<String>,
    #[serde(default)]
    force: bool,
    expected_digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExecParams {
    name: Option<String>,
    #[serde(default)]
    params: Value,
    expected_preview_fingerprint: Option<String>,
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

pub(super) struct SnippetExecutionOutcome {
    pub(super) raw_response: CodeModeExecutionResponse,
    pub(super) display_response: CodeModeExecutionResponse,
    pub(super) receipt_status: String,
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
    dispatch_inner(manager.as_deref(), action, params, None, None).await
}

/// Every operation that executes or accesses owner-scoped records must retain
/// the surface's actual caller, route and capability ceiling.
pub fn requires_execution_context(action: &str) -> bool {
    matches!(
        action,
        "snippets.exec"
            | "snippets.fixture"
            | "snippets.test"
            | "snippets.promote"
            | "snippets.preview"
            | "snippets.replay"
            | "snippets.receipt"
            | "snippets.history"
            | "snippets.artifact"
    )
}

/// CLI mock tests have already loaded configuration but intentionally have no
/// gateway manager or upstream connections. Carry its source ceiling explicitly.
pub async fn dispatch_with_source_limit(
    action: &str,
    params: Value,
    max_source_bytes: usize,
) -> Result<Value, ToolError> {
    let manager = crate::dispatch::gateway::current_gateway_manager();
    dispatch_inner(
        manager.as_deref(),
        action,
        params,
        None,
        Some(max_source_bytes),
    )
    .await
}

pub async fn dispatch_with_manager_and_context(
    manager: &crate::dispatch::gateway::manager::GatewayManager,
    action: &str,
    params: Value,
    dispatch_context: Option<SnippetDispatchContext>,
) -> Result<Value, ToolError> {
    dispatch_inner(Some(manager), action, params, dispatch_context, None).await
}

async fn dispatch_inner(
    manager: Option<&crate::dispatch::gateway::manager::GatewayManager>,
    action: &str,
    params: Value,
    dispatch_context: Option<SnippetDispatchContext>,
    source_limit_override: Option<usize>,
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
        "snippets.list" => to_json(list_snippets_with_diagnostics(
            &lab_home(),
            &builtin_snippet_dir(),
        )?),
        "snippets.fixture" => super::fixtures::generate(manager, params, dispatch_context).await,
        "snippets.history" | "snippets.artifact" => {
            super::history::dispatch(manager, action, params, dispatch_context).await
        }
        "snippets.preview" | "snippets.replay" => {
            super::preview::dispatch(manager, action, params, dispatch_context).await
        }
        "snippets.receipt" => {
            let id = require_str(&params, "execution_id")?;
            let manager = manager.ok_or_else(|| {
                ToolError::internal_message("snippet receipt lookup requires a gateway")
            })?;
            let context = dispatch_context.unwrap_or_else(SnippetDispatchContext::trusted_local);
            if !context.is_admin || !context.execution_caller.is_admin() {
                return Err(ToolError::Forbidden {
                    message: "snippet receipt lookup requires lab:admin".into(),
                    required_scopes: vec!["lab:admin".into()],
                });
            }
            to_json(
                manager
                    .snippet_receipt(&id, receipt_owner(&context))
                    .await?,
            )
        }
        "snippets.get" => {
            let name = require_str(&params, "name")?;
            to_json(resolve_snippet(&lab_home(), &builtin_snippet_dir(), &name)?)
        }
        "snippets.create" => {
            let params: CreateParams = parse_params(params)?;
            to_json(create_user_snippet_checked(
                &lab_home(),
                &params.name,
                &params.body,
                params.description.as_deref(),
                params.force,
                params.expected_digest.as_deref(),
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
            if let Some(expected) = params.expected_preview_fingerprint {
                return super::preview::guarded_exec(
                    manager,
                    &name,
                    params.params,
                    dispatch_context,
                    &expected,
                )
                .await;
            }
            let outcome = execute_snippet_outcome(
                manager,
                &name,
                params.params,
                &execution_scope,
                &execution_caller,
                execution_surface,
                dispatch_context.as_ref(),
            )
            .await?;
            let mut response = to_json(outcome.display_response)?;
            response["receipt_status"] = json!(outcome.receipt_status);
            Ok(response)
        }
        "snippets.test" => {
            super::testing::test(
                manager,
                params,
                &execution_scope,
                &execution_caller,
                execution_surface,
                source_limit_override,
                dispatch_context.as_ref(),
            )
            .await
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

fn snippet_response_passed(response: &CodeModeExecutionResponse) -> bool {
    response.calls.iter().all(|call| call.ok)
        && response
            .result
            .as_ref()
            .and_then(|result| result.get("ok"))
            .and_then(Value::as_bool)
            .unwrap_or(true)
}

pub(super) fn snippet_test_result(
    name: String,
    outcome: SnippetExecutionOutcome,
) -> Result<Value, ToolError> {
    let passed = snippet_response_passed(&outcome.raw_response);
    to_json(json!({
        "name": name,
        "passed": passed,
        "response": outcome.display_response,
        "receipt_status": outcome.receipt_status,
    }))
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
#[path = "dispatch_tests.rs"]
mod tests;
