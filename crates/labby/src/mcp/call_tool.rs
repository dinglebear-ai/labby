//! `call_tool` dispatch entry: arg parse + service lookup, the gateway
//! meta-tool routing, the post-meta-tool gates
//! (visibility / action-allowed / code_mode-hidden / admin-scope /
//! destructive elicitation), the builtin dispatch branch, and the
//! fall-through to the upstream proxy tail.
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.5`) as an inherent
//! `impl LabMcpServer` method. The `ServerHandler` trait impl in
//! `server.rs` keeps a one-line delegator.
//!
//! Preserves the exact early-return ordering (codemode → visibility → action →
//! code_mode-hidden → admin-scope → elicitation → builtin → upstream tail). The
//! codemode and upstream branches live in
//! `call_tool_codemode.rs` / `call_tool_upstream.rs`. No behavior change.

use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use serde_json::Value;

use crate::dispatch::error::ToolError;
#[cfg(feature = "gateway")]
use crate::dispatch::gateway::manager::CallbackToolLookup;
#[cfg(feature = "gateway")]
use crate::dispatch::upstream::types::UpstreamTool;
#[cfg(feature = "gateway")]
use crate::mcp::call_tool_upstream::PreResolvedUpstreamTool;
use crate::mcp::catalog::SERVER_LOGS_TOOL_NAME;
#[cfg(feature = "gateway")]
use crate::mcp::catalog::{
    ADD_SERVER_TOOL_NAME, CODE_MODE_TOOL_NAME, CODE_MODE_UI_TOOL_NAME, GATEWAY_STATUS_TOOL_NAME,
    MCP_APP_TOOL_NAME,
};
#[cfg(feature = "gateway")]
use crate::mcp::catalog_coalesce::schedule_catalog_notification;
#[cfg(feature = "gateway")]
use crate::mcp::catalog_notifications::CatalogNotificationChanges;
use crate::mcp::context::{
    auth_context_from_extensions, tool_execute_builtin_action_allowed, tool_execute_scope_allowed,
};
use crate::mcp::envelope::{build_error, build_error_extra};
use crate::mcp::error::DispatchError;
#[cfg(feature = "gateway")]
use crate::mcp::handlers_resources::admin_app_resources_visible;
use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel, spawn_dispatch_notification};
#[cfg(feature = "gateway")]
use crate::mcp::permanent_tools::PermanentToolId;
use crate::mcp::result_format::{
    error_result_from_envelope, estimate_tokens_args, format_dispatch_result, tool_error_envelope,
};
use crate::mcp::server::LabMcpServer;

#[cfg(feature = "gateway")]
enum WidgetCallbackGate {
    Allowed {
        resolved: Box<PreResolvedUpstreamTool>,
        /// True when the callback target is a tool that Code Mode keeps hidden
        /// from `list_tools` (an MCP App sibling, or any exposed tool surfaced
        /// only through the legacy `LABBY_CODE_MODE_WIDGET_CALLBACKS` bypass).
        /// Calling such a hidden tool via the bypass requires the `lab`/
        /// `lab:admin` scope check below. It is `false` only for `DirectMcpApp`
        /// candidates, which are already advertised in `list_tools`.
        requires_scope_check: bool,
    },
    Destructive {
        resolved: Box<PreResolvedUpstreamTool>,
        requires_scope_check: bool,
    },
    Ambiguous {
        valid: Vec<String>,
    },
}

fn route_scope_denied_result(service: &str, action: &str, message: String) -> CallToolResult {
    let envelope = build_error(service, action, "route_scope_denied", &message);
    error_result_from_envelope(envelope)
}

#[cfg(feature = "gateway")]
fn retain_route_visible_gateway_status_rows(
    value: &mut Value,
    route_scope: &crate::mcp::route_scope::McpRouteScope,
) {
    let Value::Array(rows) = value else {
        return;
    };
    rows.retain(|row| {
        let id = row.get("id").and_then(Value::as_str).unwrap_or_default();
        let name = row.get("name").and_then(Value::as_str).unwrap_or_default();
        match row.get("source").and_then(Value::as_str) {
            Some("custom_gateway") => route_scope.allows_upstream(id),
            Some("in_process") => {
                route_scope.allows_service(id) || route_scope.allows_service(name)
            }
            _ => false,
        }
    });
}

/// Attach the authenticated MCP subject to gateway mutations without replacing caller values.
fn inject_gateway_origin_param(params: Value, subject: Option<&str>) -> Value {
    let raw = subject
        .map(|value| format!("mcp:{value}"))
        .unwrap_or_else(|| "mcp:anonymous".to_string());
    let Some(mut object) = params.as_object().cloned() else {
        return params;
    };
    object.entry("owner".to_string()).or_insert_with(|| {
        serde_json::json!({
            "surface": "mcp",
            "subject": subject,
            "raw": raw,
        })
    });
    object
        .entry("origin".to_string())
        .or_insert_with(|| Value::String(raw));
    Value::Object(object)
}

impl LabMcpServer {
    #[cfg(feature = "gateway")]
    /// Record one structured failure event for a handled Add Server callback.
    async fn log_add_server_failure(
        &self,
        context: &RequestContext<RoleServer>,
        action: &str,
        kind: &'static str,
        message: &str,
        elapsed_ms: u128,
    ) {
        let subject = self.request_subject_log_tag(context);
        if kind == "internal_error" {
            tracing::error!(
                surface = "mcp",
                service = ADD_SERVER_TOOL_NAME,
                action,
                subject,
                elapsed_ms,
                kind,
                error = %message,
                "Add Server dispatch error"
            );
        } else {
            tracing::warn!(
                surface = "mcp",
                service = ADD_SERVER_TOOL_NAME,
                action,
                subject,
                elapsed_ms,
                kind,
                error = %message,
                "Add Server dispatch error"
            );
        }
        self.emit_dispatch_notification(
            context,
            ADD_SERVER_TOOL_NAME,
            action,
            elapsed_ms,
            DispatchLogOutcome::Failure {
                level: if kind == "internal_error" {
                    LoggingLevel::Error
                } else {
                    LoggingLevel::Warning
                },
                kind,
            },
        )
        .await;
    }

    fn log_route_scope_denial(
        &self,
        context: &RequestContext<RoleServer>,
        service: &str,
        action: &str,
        message: &str,
        elapsed_ms: u128,
    ) {
        let subject = self.request_subject_log_tag(context);
        tracing::warn!(
            surface = "mcp",
            service,
            action,
            subject,
            route_scope = %self.route_scope.label(),
            elapsed_ms,
            kind = "route_scope_denied",
            error = %message,
            "MCP call denied by protected route scope"
        );
        if !self.should_emit_logging_notification(LoggingLevel::Warning) {
            return;
        }

        let actor_key = crate::mcp::context::actor_key_from_extensions(&context.extensions)
            .map(ToOwned::to_owned);
        spawn_dispatch_notification(
            context.peer.clone(),
            actor_key,
            service.to_string(),
            action.to_string(),
            elapsed_ms,
            DispatchLogOutcome::Failure {
                level: LoggingLevel::Warning,
                kind: "route_scope_denied",
            },
        );
    }

    pub(crate) async fn call_tool_response_impl(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if self.tool_request_is_destructive(&request, &context).await {
            let service = request.name.as_ref();
            let action = request
                .arguments
                .as_ref()
                .and_then(|arguments| arguments.get("action"))
                .and_then(Value::as_str)
                .unwrap_or("call_tool");
            match crate::mcp::elicitation::destructive_confirmation(&request, service, action) {
                crate::mcp::elicitation::DestructiveConfirmation::Proceed => {}
                crate::mcp::elicitation::DestructiveConfirmation::InputRequired(result) => {
                    return Ok(CallToolResponse::InputRequired(result));
                }
                crate::mcp::elicitation::DestructiveConfirmation::Refused => {
                    let envelope = build_error(
                        service,
                        action,
                        "confirmation_required",
                        &format!("action `{action}` is destructive — confirm to proceed"),
                    );
                    return Ok(error_result_from_envelope(envelope).into());
                }
            }
        }
        let start = Instant::now();
        // Marks the caller's turn as open for the whole dispatch, including
        // every early return below. A catalog notification emitted while this
        // is held invalidates a binding the caller is actively using, so the
        // fanout reports it as `during_tool_call` — the signal that separates
        // harmless catalog movement from the flapping clients actually feel.
        let _in_flight = crate::mcp::catalog_churn::InFlightToolCall::enter();
        let service = request.name.as_ref().to_string();
        let upstream_request = request.clone();
        let args = request.arguments.unwrap_or_default();
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let params = args.get("params").cloned().unwrap_or(Value::Null);
        let instance = params
            .get("instance")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let param_key_count = params.as_object().map_or(0, serde_json::Map::len);

        let svc = self.registry.services().iter().find(|s| s.name == service);

        #[cfg(feature = "gateway")]
        {
            // ── Text-only MCP App control surface. This is intentionally separate
            // from `codemode_ui` so disabling the app never removes the tool needed
            // to restore it.
            if service == MCP_APP_TOOL_NAME {
                if !self.route_scope.exposes_code_mode() {
                    let elapsed_ms = start.elapsed().as_millis();
                    self.log_route_scope_denial(
                        &context,
                        &service,
                        "call_tool",
                        "Code Mode is not exposed on this MCP route",
                        elapsed_ms,
                    );
                    return Ok(route_scope_denied_result(
                        &service,
                        "call_tool",
                        "Code Mode is not exposed on this MCP route".to_string(),
                    )
                    .into());
                }

                let auth = auth_context_from_extensions(&context.extensions);
                let synthetic_action = if action.is_empty() {
                    "status"
                } else {
                    action.as_str()
                };
                if !tool_execute_scope_allowed(auth) {
                    let envelope = build_error_extra(
                        &service,
                        synthetic_action,
                        "forbidden",
                        "mcp_app requires one of scopes: lab, lab:admin",
                        &serde_json::json!({ "required_scopes": ["lab", "lab:admin"] }),
                    );
                    return Ok(error_result_from_envelope(envelope).into());
                }

                let target = args
                    .get("target")
                    .and_then(Value::as_str)
                    .unwrap_or("codemode");
                if target != "codemode" {
                    let envelope = build_error_extra(
                        &service,
                        synthetic_action,
                        "invalid_param",
                        &format!("unsupported MCP App target `{target}`"),
                        &serde_json::json!({ "valid": ["codemode"] }),
                    );
                    return Ok(error_result_from_envelope(envelope).into());
                }

                let desired = match synthetic_action {
                    "status" => None,
                    "enable" => Some(true),
                    "disable" => Some(false),
                    _ => {
                        let envelope = build_error_extra(
                            &service,
                            synthetic_action,
                            "unknown_action",
                            &format!("unknown MCP App action `{synthetic_action}`"),
                            &serde_json::json!({ "valid": ["status", "enable", "disable"] }),
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    }
                };

                if desired.is_some() && !admin_app_resources_visible(auth) {
                    let envelope = build_error_extra(
                        &service,
                        synthetic_action,
                        "forbidden",
                        "changing MCP App state requires lab:admin scope",
                        &serde_json::json!({ "required_scopes": ["lab:admin"] }),
                    );
                    return Ok(error_result_from_envelope(envelope).into());
                }

                let previous = self.code_mode_app_state.is_enabled();
                let enabled = if let Some(desired) = desired {
                    if let Some(manager) = self.gateway_manager.as_ref() {
                        let mut next = manager.code_mode_config().await;
                        next.mcp_ui_enabled = desired;
                        match manager
                            .set_code_mode_config(
                                next,
                                Some(labby_runtime::catalog_notify::SOURCE_MCP_CALL_MCP_APP),
                                None,
                            )
                            .await
                        {
                            Ok(current) => current.mcp_ui_enabled,
                            Err(error) => {
                                let envelope =
                                    tool_error_envelope(&service, synthetic_action, &error);
                                return Ok(error_result_from_envelope(envelope).into());
                            }
                        }
                    } else {
                        self.code_mode_app_state.set_enabled(desired);
                        desired
                    }
                } else {
                    previous
                };
                let changed = desired.is_some() && previous != enabled;
                if changed && self.gateway_manager.is_none() {
                    schedule_catalog_notification(
                        &self.peers,
                        CatalogNotificationChanges::new(true, true, false),
                        labby_runtime::catalog_notify::SOURCE_MCP_CALL_MCP_APP,
                    );
                }

                let notification_scheduled = changed;
                tracing::info!(
                    surface = "mcp",
                    service = MCP_APP_TOOL_NAME,
                    action = synthetic_action,
                    subject = self.request_subject_log_tag(&context),
                    target,
                    enabled,
                    changed,
                    notification_scheduled,
                    elapsed_ms = start.elapsed().as_millis(),
                    "Code Mode MCP App state evaluated"
                );
                let payload = serde_json::json!({
                    "kind": "mcp_app_control",
                    "target": "codemode",
                    "enabled": enabled,
                    "changed": changed,
                    "scope": "gateway",
                    "text_tool": CODE_MODE_TOOL_NAME,
                    "ui_tool": CODE_MODE_UI_TOOL_NAME,
                    "notification_scheduled": notification_scheduled,
                });
                let mut result =
                    CallToolResult::success(vec![ContentBlock::text(payload.to_string())]);
                result.structured_content = Some(payload);
                return Ok(result.into());
            }

            // ── Gateway Code Mode execution. Both public names share one backend;
            // only `codemode_ui` is advertised with MCP App metadata. The
            // text-only name resolves through the permanent tool registry so its
            // identity survives upstream churn.
            if matches!(
                self.registry.permanent_tools().resolve(&service),
                Some(PermanentToolId::CodeMode)
            ) || service == CODE_MODE_UI_TOOL_NAME
            {
                if !self.route_scope.exposes_code_mode() {
                    let elapsed_ms = start.elapsed().as_millis();
                    self.log_route_scope_denial(
                        &context,
                        &service,
                        "call_tool",
                        "Code Mode is not exposed on this MCP route",
                        elapsed_ms,
                    );
                    return Ok(route_scope_denied_result(
                        &service,
                        "call_tool",
                        "Code Mode is not exposed on this MCP route".to_string(),
                    )
                    .into());
                }
                if service == CODE_MODE_UI_TOOL_NAME && !self.code_mode_app_state.is_enabled() {
                    let envelope = build_error_extra(
                        &service,
                        "call_tool",
                        "app_disabled",
                        "the Code Mode MCP App is disabled; use codemode for text-only execution or mcp_app to re-enable it",
                        &serde_json::json!({
                            "text_tool": CODE_MODE_TOOL_NAME,
                            "control_tool": MCP_APP_TOOL_NAME,
                        }),
                    );
                    return Ok(error_result_from_envelope(envelope).into());
                }
                return self
                    .call_tool_codemode_impl(&service, &args, &context)
                    .await
                    .map(Into::into);
            }

            let handles_add_server = service == ADD_SERVER_TOOL_NAME
                && admin_app_resources_visible(auth_context_from_extensions(&context.extensions))
                && self.add_server_app_available_on_mcp().await;
            if handles_add_server {
                let synthetic_action = if action.is_empty() {
                    "open"
                } else {
                    action.as_str()
                };
                let auth = auth_context_from_extensions(&context.extensions);
                let result = match synthetic_action {
                    "open" => Ok(serde_json::json!({
                        "kind": "add_server",
                        "status": "ready",
                    })),
                    "test" | "create" => {
                        let Some(manager) = &self.gateway_manager else {
                            let message = "gateway manager not wired";
                            self.log_add_server_failure(
                                &context,
                                synthetic_action,
                                "internal_error",
                                message,
                                start.elapsed().as_millis(),
                            )
                            .await;
                            let envelope =
                                build_error(&service, synthetic_action, "internal_error", message);
                            return Ok(error_result_from_envelope(envelope).into());
                        };
                        let gateway_action = if synthetic_action == "test" {
                            "gateway.test"
                        } else {
                            "gateway.add"
                        };
                        if !self.action_allowed_on_mcp("gateway", gateway_action).await {
                            let message = format!(
                                "action `{gateway_action}` is not exposed for service `gateway`"
                            );
                            self.log_add_server_failure(
                                &context,
                                synthetic_action,
                                "unknown_action",
                                &message,
                                start.elapsed().as_millis(),
                            )
                            .await;
                            let envelope = build_error_extra(
                                &service,
                                synthetic_action,
                                "unknown_action",
                                &message,
                                &serde_json::json!({
                                    "canonical_action": gateway_action,
                                    "valid": self.allowed_mcp_actions("gateway").await,
                                }),
                            );
                            return Ok(error_result_from_envelope(envelope).into());
                        }
                        let gateway_entry = self
                            .registry
                            .services()
                            .iter()
                            .find(|entry| entry.name == "gateway");
                        let Some(gateway_entry) = gateway_entry else {
                            let message = "gateway registry entry not wired";
                            self.log_add_server_failure(
                                &context,
                                synthetic_action,
                                "internal_error",
                                message,
                                start.elapsed().as_millis(),
                            )
                            .await;
                            let envelope =
                                build_error(&service, synthetic_action, "internal_error", message);
                            return Ok(error_result_from_envelope(envelope).into());
                        };
                        if !tool_execute_builtin_action_allowed(gateway_entry, gateway_action, auth)
                        {
                            let message =
                                format!("action `{gateway_action}` requires `lab:admin` scope");
                            self.log_add_server_failure(
                                &context,
                                synthetic_action,
                                "forbidden",
                                &message,
                                start.elapsed().as_millis(),
                            )
                            .await;
                            let envelope = build_error_extra(
                                &service,
                                synthetic_action,
                                "forbidden",
                                &message,
                                &serde_json::json!({ "required_scopes": ["lab:admin"] }),
                            );
                            return Ok(error_result_from_envelope(envelope).into());
                        }
                        let params =
                            inject_gateway_origin_param(params, self.request_subject(&context));
                        let enrichment_scope = crate::dispatch::gateway::GatewayEnrichmentScope {
                            route_visible_upstreams: self.route_scope.allowed_upstreams().cloned(),
                        };
                        crate::dispatch::gateway::dispatch_with_manager_scoped(
                            manager,
                            gateway_action,
                            params,
                            enrichment_scope,
                        )
                        .await
                    }
                    _ => Err(ToolError::UnknownAction {
                        message: format!("unknown Add Server action `{synthetic_action}`"),
                        valid: vec!["open".to_string(), "test".to_string(), "create".to_string()],
                        hint: None,
                    }),
                };
                let result =
                    result.map_err(|error| anyhow::Error::from(DispatchError::from(error)));
                let elapsed_ms = start.elapsed().as_millis();
                let input_tokens = estimate_tokens_args(&args);
                let (result, outcome) = format_dispatch_result(
                    result,
                    &service,
                    synthetic_action,
                    elapsed_ms,
                    &self.request_subject_log_tag(&context),
                    self.request_actor_key(&context),
                    input_tokens,
                );
                self.emit_dispatch_notification(
                    &context,
                    &service,
                    synthetic_action,
                    elapsed_ms,
                    outcome,
                )
                .await;
                return Ok(result.into());
            }

            let handles_gateway_status = service == GATEWAY_STATUS_TOOL_NAME
                && admin_app_resources_visible(auth_context_from_extensions(&context.extensions))
                && self.gateway_status_app_available_on_mcp().await;
            if handles_gateway_status {
                let synthetic_action = if action.is_empty() {
                    "open"
                } else {
                    action.as_str()
                };
                let result = match synthetic_action {
                    "open" | "refresh" => {
                        let manager = self
                            .gateway_manager
                            .as_ref()
                            .expect("availability requires a gateway manager");
                        let enrichment_scope = crate::dispatch::gateway::GatewayEnrichmentScope {
                            route_visible_upstreams: self.route_scope.allowed_upstreams().cloned(),
                        };
                        crate::dispatch::gateway::dispatch_with_manager_scoped(
                            manager,
                            "gateway.list",
                            serde_json::json!({}),
                            enrichment_scope,
                        )
                        .await
                        .map(|mut value| {
                            retain_route_visible_gateway_status_rows(&mut value, &self.route_scope);
                            value
                        })
                    }
                    _ => Err(ToolError::UnknownAction {
                        message: format!("unknown Gateway Status action `{synthetic_action}`"),
                        valid: vec!["open".to_string(), "refresh".to_string()],
                        hint: None,
                    }),
                };
                let result =
                    result.map_err(|error| anyhow::Error::from(DispatchError::from(error)));
                let elapsed_ms = start.elapsed().as_millis();
                let input_tokens = estimate_tokens_args(&args);
                let (result, outcome) = format_dispatch_result(
                    result,
                    &service,
                    synthetic_action,
                    elapsed_ms,
                    &self.request_subject_log_tag(&context),
                    self.request_actor_key(&context),
                    input_tokens,
                );
                self.emit_dispatch_notification(
                    &context,
                    &service,
                    synthetic_action,
                    elapsed_ms,
                    outcome,
                )
                .await;
                return Ok(result.into());
            }
        }

        if svc.is_some() && !self.route_scope.allows_service(&service) {
            let elapsed_ms = start.elapsed().as_millis();
            let message = format!("service `{service}` is not exposed on this MCP route");
            self.log_route_scope_denial(&context, &service, &action, &message, elapsed_ms);
            return Ok(route_scope_denied_result(&service, &action, message).into());
        }

        if svc.is_some() && !self.service_visible_on_mcp(&service).await {
            let envelope = build_error(
                &service,
                &action,
                "not_found",
                &format!("service `{service}` is not enabled on the mcp surface"),
            );
            return Ok(error_result_from_envelope(envelope).into());
        }

        if svc.is_some() && !self.action_allowed_on_mcp(&service, &action).await {
            let mut extra = serde_json::Map::new();
            if let Some(valid) = self.allowed_mcp_actions(&service).await {
                extra.insert(
                    "valid".to_string(),
                    serde_json::to_value(valid).unwrap_or(Value::Array(Vec::new())),
                );
            }
            let envelope = build_error_extra(
                &service,
                &action,
                "unknown_action",
                &format!("action `{action}` is not exposed for service `{service}`"),
                &Value::Object(extra),
            );
            return Ok(error_result_from_envelope(envelope).into());
        }

        // Upstream widget-callback resolution is a gateway-only concern (it
        // proxies to upstream MCP tools). Without the gateway feature there are
        // no upstream tools, so this resolution and the upstream tail below are
        // both compiled out.
        #[cfg(feature = "gateway")]
        let mut resolved_upstream_tool = None;
        #[cfg(feature = "gateway")]
        if self.code_mode_visibility().await.hides_raw_tools() && service != SERVER_LOGS_TOOL_NAME {
            let widget_callback = if svc.is_none() {
                match self.resolve_widget_callback_gate(&service, &context).await {
                    Ok(gate) => gate,
                    Err(err) => {
                        let envelope = tool_error_envelope(&service, "call_tool", &err);
                        return Ok(error_result_from_envelope(envelope).into());
                    }
                }
            } else {
                None
            };
            match widget_callback {
                Some(WidgetCallbackGate::Destructive {
                    resolved,
                    requires_scope_check,
                }) => {
                    if requires_scope_check
                        && !tool_execute_scope_allowed(auth_context_from_extensions(
                            &context.extensions,
                        ))
                    {
                        let envelope = build_error_extra(
                            &service,
                            &action,
                            "forbidden",
                            "hidden-tool widget callbacks require one of scopes: lab, lab:admin",
                            &serde_json::json!({
                                "required_scopes": ["lab", "lab:admin"],
                            }),
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    }
                    resolved_upstream_tool = Some(*resolved);
                }
                Some(WidgetCallbackGate::Ambiguous { valid }) => {
                    let envelope = build_error_extra(
                        &service,
                        &action,
                        "ambiguous_tool",
                        &format!("tool `{service}` matched multiple MCP App sibling tools"),
                        &serde_json::json!({ "valid": valid }),
                    );
                    return Ok(error_result_from_envelope(envelope).into());
                }
                Some(WidgetCallbackGate::Allowed {
                    resolved,
                    requires_scope_check,
                }) => {
                    if requires_scope_check
                        && !tool_execute_scope_allowed(auth_context_from_extensions(
                            &context.extensions,
                        ))
                    {
                        let envelope = build_error_extra(
                            &service,
                            &action,
                            "forbidden",
                            "hidden-tool widget callbacks require one of scopes: lab, lab:admin",
                            &serde_json::json!({
                                "required_scopes": ["lab", "lab:admin"],
                            }),
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    }
                    tracing::info!(
                        surface = "mcp",
                        service = %service,
                        action = %action,
                        upstream = %resolved.upstream_name,
                        route = resolved.route,
                        "code_mode raw-tool gate bypassed for MCP App widget callback"
                    );
                    resolved_upstream_tool = Some(*resolved);
                }
                None => {
                    let envelope = build_error(
                        &service,
                        &action,
                        "not_found",
                        &format!("tool `{service}` is hidden while code_mode mode is enabled"),
                    );
                    return Ok(error_result_from_envelope(envelope).into());
                }
            }
        }

        if let Some(entry) = svc
            && !tool_execute_builtin_action_allowed(
                entry,
                &action,
                auth_context_from_extensions(&context.extensions),
            )
        {
            let envelope = build_error_extra(
                &service,
                &action,
                "forbidden",
                &format!("action `{action}` for service `{service}` requires `lab:admin` scope"),
                &serde_json::json!({ "required_scopes": ["lab:admin"] }),
            );
            return Ok(error_result_from_envelope(envelope).into());
        }

        let subject = self.request_subject_log_tag(&context);
        let actor_key = self.request_actor_key(&context);
        let dispatch_action = if svc.is_some() {
            action.as_str()
        } else {
            "call_tool"
        };
        tracing::info!(
            surface = "mcp",
            service,
            action = dispatch_action,
            subject,
            actor_key,
            tool = %service,
            instance = instance.as_deref(),
            param_key_count,
            "dispatch start"
        );

        // Try built-in dispatch first.
        if let Some(entry) = svc {
            tracing::info!(
                surface = "mcp",
                service,
                action = action.as_str(),
                tool = %service,
                route = "builtin",
                "dispatch route selected"
            );
            #[cfg(feature = "gateway")]
            if service == "snippets" && action == "snippets.promote" {
                return self
                    .call_snippets_promote_impl(
                        &action, params, &args, start, &subject, actor_key, &context,
                    )
                    .await
                    .map(Into::into);
            }
            let result = if service == "gateway" {
                #[cfg(feature = "gateway")]
                {
                    let Some(manager) = &self.gateway_manager else {
                        let envelope = build_error(
                            &service,
                            &action,
                            "internal_error",
                            "gateway manager not wired",
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    };
                    let params =
                        inject_gateway_origin_param(params, self.request_subject(&context));
                    let enrichment_scope = crate::dispatch::gateway::GatewayEnrichmentScope {
                        route_visible_upstreams: self.route_scope.allowed_upstreams().cloned(),
                    };
                    crate::dispatch::gateway::dispatch_with_manager_scoped(
                        manager,
                        &action,
                        params,
                        enrichment_scope,
                    )
                    .await
                }
                #[cfg(not(feature = "gateway"))]
                {
                    (entry.dispatch)(action.clone(), params).await
                }
            } else {
                (entry.dispatch)(action.clone(), params).await
            };
            let result = result.map_err(|te| anyhow::Error::from(DispatchError::from(te)));
            let elapsed_ms = start.elapsed().as_millis();
            let input_tokens = estimate_tokens_args(&args);
            let (result, outcome) = format_dispatch_result(
                result,
                &service,
                &action,
                elapsed_ms,
                &subject,
                actor_key,
                input_tokens,
            );
            self.emit_dispatch_notification(&context, &service, &action, elapsed_ms, outcome)
                .await;
            return Ok(result.into());
        }

        // Fall through to upstream proxy dispatch (raw + subject-scoped +
        // no-dispatcher-wired fallback). The helper returns unconditionally.
        // The upstream proxy only exists with the gateway feature; without it an
        // unresolved service name is simply not found.
        #[cfg(feature = "gateway")]
        {
            self.call_tool_upstream_impl(
                &service,
                &action,
                upstream_request,
                resolved_upstream_tool,
                start,
                &subject,
                actor_key,
                &context,
            )
            .await
        }
        #[cfg(not(feature = "gateway"))]
        {
            let _ = (upstream_request, actor_key, start);
            let envelope = build_error(
                &service,
                &action,
                "not_found",
                &format!("service `{service}` not found"),
            );
            Ok(error_result_from_envelope(envelope).into())
        }
    }

    /// Complete-only test/internal adapter. Protocol callers use
    /// [`Self::call_tool_response_impl`] so MRTR and task result variants are
    /// preserved on the wire.
    #[cfg(test)]
    pub(crate) async fn call_tool_impl(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        match self.call_tool_response_impl(request, context).await? {
            CallToolResponse::Complete(result) => Ok(result),
            CallToolResponse::InputRequired(_) => Err(ErrorData::internal_error(
                "complete-only call adapter received input_required",
                None,
            )),
            CallToolResponse::Task(_) => Err(ErrorData::internal_error(
                "complete-only call adapter received task result",
                None,
            )),
            _ => Err(ErrorData::internal_error(
                "complete-only call adapter received unknown result type",
                None,
            )),
        }
    }
}

#[cfg(not(feature = "gateway"))]
impl LabMcpServer {
    /// Resolve whether a built-in tool call needs RC-native MRTR elicitation.
    ///
    /// Gateway-only synthetic and upstream tools are unavailable in this
    /// feature slice, so the registry is the complete classification source.
    pub(crate) async fn tool_request_is_destructive(
        &self,
        request: &CallToolRequestParams,
        _context: &RequestContext<RoleServer>,
    ) -> bool {
        let service = request.name.as_ref();
        let action = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("action"))
            .and_then(Value::as_str)
            .unwrap_or("");

        self.registry
            .services()
            .iter()
            .find(|entry| entry.name == service)
            .is_some_and(|entry| {
                entry
                    .actions
                    .iter()
                    .any(|candidate| candidate.name == action && candidate.destructive)
            })
    }
}

#[cfg(feature = "gateway")]
impl LabMcpServer {
    /// Resolve whether a tool call needs RC-native MRTR elicitation.
    ///
    /// This is deliberately classification-only. The protocol handler returns
    /// `input_required`; the normal dispatcher never starts an in-flight
    /// server-to-client elicitation RPC.
    pub(crate) async fn tool_request_is_destructive(
        &self,
        request: &CallToolRequestParams,
        context: &RequestContext<RoleServer>,
    ) -> bool {
        let service = request.name.as_ref();
        let action = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("action"))
            .and_then(Value::as_str)
            .unwrap_or("");

        if let Some(entry) = self
            .registry
            .services()
            .iter()
            .find(|entry| entry.name == service)
        {
            return entry
                .actions
                .iter()
                .any(|candidate| candidate.name == action && candidate.destructive);
        }

        #[cfg(feature = "gateway")]
        {
            if service == ADD_SERVER_TOOL_NAME {
                let gateway_action = match action {
                    "test" => Some("gateway.test"),
                    "create" => Some("gateway.add"),
                    _ => None,
                };
                return gateway_action.is_some_and(|gateway_action| {
                    self.registry
                        .services()
                        .iter()
                        .find(|entry| entry.name == "gateway")
                        .is_some_and(|entry| {
                            entry.actions.iter().any(|candidate| {
                                candidate.name == gateway_action && candidate.destructive
                            })
                        })
                });
            }

            if self.code_mode_visibility().await.hides_raw_tools()
                && service != SERVER_LOGS_TOOL_NAME
            {
                return matches!(
                    self.resolve_widget_callback_gate(service, context).await,
                    Ok(Some(WidgetCallbackGate::Destructive { .. }))
                );
            }

            let Some(manager) = &self.gateway_manager else {
                return false;
            };
            let owner = self.request_runtime_owner(context);
            let oauth_subject = crate::mcp::context::oauth_upstream_subject_for_request(
                auth_context_from_extensions(&context.extensions),
                self.request_subject(context),
            );
            return manager
                .resolve_raw_upstream_tool_scoped(
                    service,
                    self.route_scope.allowed_upstreams(),
                    Some(&owner),
                    oauth_subject.as_deref(),
                )
                .await
                .is_ok_and(|(_, tool)| tool.destructive);
        }

        #[cfg(not(feature = "gateway"))]
        false
    }

    async fn resolve_widget_callback_gate(
        &self,
        service: &str,
        context: &RequestContext<RoleServer>,
    ) -> Result<Option<WidgetCallbackGate>, ToolError> {
        let Some(manager) = &self.gateway_manager else {
            return Ok(None);
        };
        let owner = self.request_runtime_owner(context);
        let oauth_subject = crate::mcp::context::oauth_upstream_subject_for_request(
            auth_context_from_extensions(&context.extensions),
            self.request_subject(context),
        );
        let allowed = self.route_scope.allowed_upstreams();

        if self.code_mode_widget_callbacks_enabled() {
            let candidates = manager
                .resolve_widget_callback_tool_candidates_scoped(
                    service,
                    allowed,
                    Some(&owner),
                    oauth_subject.as_deref(),
                    CallbackToolLookup::LegacyAnyExposed,
                )
                .await?;
            // Legacy mode surfaces ANY exposed non-destructive upstream tool,
            // including ones with no MCP App UI resource that are therefore NOT
            // advertised in `list_tools`. Calling such a hidden tool through the
            // bypass must require the `lab`/`lab:admin` scope check, so this path
            // sets `requires_scope_check = true` (matching the sibling path),
            // rather than the `false` that is only correct for advertised
            // `DirectMcpApp` candidates.
            return Ok(classify_widget_callback_candidates(
                "upstream_widget_callback_legacy",
                true,
                candidates,
            ));
        }

        let direct_candidates = manager
            .resolve_widget_callback_tool_candidates_scoped(
                service,
                allowed,
                Some(&owner),
                oauth_subject.as_deref(),
                CallbackToolLookup::DirectMcpApp,
            )
            .await?;
        if !direct_candidates.is_empty() {
            return Ok(classify_widget_callback_candidates(
                "upstream_widget_callback",
                false,
                direct_candidates,
            ));
        }

        let sibling_candidates = manager
            .resolve_widget_callback_tool_candidates_scoped(
                service,
                allowed,
                Some(&owner),
                oauth_subject.as_deref(),
                CallbackToolLookup::SiblingOfMcpApp,
            )
            .await?;
        Ok(classify_widget_callback_candidates(
            "upstream_widget_sibling_callback",
            true,
            sibling_candidates,
        ))
    }

    fn code_mode_widget_callbacks_enabled(&self) -> bool {
        #[cfg(test)]
        if self.code_mode_widget_callbacks_enabled_for_test {
            return true;
        }

        crate::config::code_mode_widget_callbacks_enabled()
    }
}

#[cfg(feature = "gateway")]
fn classify_widget_callback_candidates(
    route: &'static str,
    requires_scope_check: bool,
    candidates: Vec<(String, UpstreamTool)>,
) -> Option<WidgetCallbackGate> {
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() > 1 {
        let valid = candidates
            .iter()
            .map(|(upstream, tool)| format!("{upstream}::{}", tool.tool.name))
            .collect();
        return Some(WidgetCallbackGate::Ambiguous { valid });
    }
    let (upstream_name, tool) = candidates.into_iter().next().expect("checked len");
    let resolved: Box<PreResolvedUpstreamTool> = PreResolvedUpstreamTool {
        upstream_name,
        tool,
        route,
    }
    .into();
    if resolved.tool.destructive {
        return Some(WidgetCallbackGate::Destructive {
            resolved,
            requires_scope_check,
        });
    }

    Some(WidgetCallbackGate::Allowed {
        resolved,
        requires_scope_check,
    })
}
