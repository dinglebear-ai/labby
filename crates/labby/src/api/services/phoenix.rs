//! Authenticated HTTP adapter for the container-local Phoenix assistant.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    routing::post,
};
use labby_auth::{AuthContext, VerifiedIdentity};
use serde_json::{Value, json};

use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, error::ApiError, state::AppState};
use crate::dispatch::error::ToolError;

const MAX_MCP_APP_RESOURCES_PER_RESPONSE: usize = 8;

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    crate::api::route_registry::RouteGroup::empty().route(descriptors().remove(0), post(handle))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("POST", "/", "handle", "phoenix", RouteAuth::V1)
            .when("mounted only when API authentication is configured"),
    ]
}

fn csrf_exempt(action: &str) -> bool {
    matches!(
        action,
        "help" | "schema" | "phoenix.status" | "phoenix.session.list" | "phoenix.session.read"
    )
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct McpAppBinding {
    call_id: String,
    resource_uri: String,
    tool_input: Option<Value>,
}

fn object_call_id(object: &serde_json::Map<String, Value>, fallback: &str) -> String {
    ["id", "callId", "call_id", "toolCallId", "tool_call_id"]
        .into_iter()
        .find_map(|key| object.get(key).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

fn collect_mcp_app_bindings(
    value: &Value,
    fallback_call_id: &str,
    depth: usize,
    bindings: &mut Vec<McpAppBinding>,
) {
    if depth > 12 || bindings.len() >= MAX_MCP_APP_RESOURCES_PER_RESPONSE {
        return;
    }
    match value {
        Value::Array(values) => {
            for value in values {
                collect_mcp_app_bindings(value, fallback_call_id, depth + 1, bindings);
            }
        }
        Value::Object(object) => {
            let call_id = object_call_id(object, fallback_call_id);
            let standard = object
                .get("ui")
                .and_then(Value::as_object)
                .and_then(|ui| ui.get("resourceUri"))
                .and_then(Value::as_str);
            let openai = object.get("openai/outputTemplate").and_then(Value::as_str);
            if let Some(uri) = standard.or(openai)
                && uri.starts_with("ui://")
                && uri.len() <= 4096
            {
                let tool_input = object
                    .get("arguments")
                    .or_else(|| object.get("params"))
                    .or_else(|| object.get("input"))
                    .cloned();
                bindings.push(McpAppBinding {
                    call_id: call_id.clone(),
                    resource_uri: uri.to_owned(),
                    tool_input,
                });
            }
            for child in object.values() {
                collect_mcp_app_bindings(child, &call_id, depth + 1, bindings);
            }
        }
        _ => {}
    }
}

fn is_mcp_tool_event(event: &Value) -> bool {
    let method = event
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if method != "item/completed" && method != "item/mcpToolCall/progress" {
        return false;
    }
    event
        .pointer("/params/item/type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.eq_ignore_ascii_case("mcpToolCall"))
        || method == "item/mcpToolCall/progress"
}

fn event_tool_input(event: &Value) -> Value {
    let item = event.pointer("/params/item").unwrap_or(&Value::Null);
    item.get("arguments")
        .or_else(|| item.get("params"))
        .or_else(|| item.get("input"))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

fn event_tool_result(event: &Value) -> Value {
    let item = event.pointer("/params/item").unwrap_or(&Value::Null);
    item.get("result")
        .or_else(|| item.get("output"))
        .cloned()
        .unwrap_or_else(|| item.clone())
}

fn event_call_id(event: &Value) -> String {
    let item = event.pointer("/params/item").and_then(Value::as_object);
    item.map_or_else(
        || "mcp-tool".to_owned(),
        |object| object_call_id(object, "mcp-tool"),
    )
}

fn app_identity(sequence: u64, call_id: &str, resource_uri: &str) -> String {
    format!("{sequence:020}:{call_id}:{resource_uri}")
}

#[cfg(feature = "gateway")]
fn mcp_app_read_error_kind(
    error: &labby_gateway::gateway::manager::PublishedResourceReadError,
) -> &'static str {
    use labby_gateway::gateway::manager::PublishedResourceReadError;
    match error {
        PublishedResourceReadError::Unavailable => "unavailable",
        PublishedResourceReadError::QueueUnavailable => "queue_unavailable",
        PublishedResourceReadError::Upstream => "upstream_error",
        PublishedResourceReadError::Timeout => "timeout",
        PublishedResourceReadError::Cancelled => "cancelled",
        PublishedResourceReadError::TooLarge => "response_too_large",
    }
}

#[cfg(feature = "gateway")]
async fn hydrate_mcp_apps(
    mut payload: Value,
    runtime: &crate::dispatch::phoenix::PhoenixRuntime,
    owner: &str,
    manager: Option<&labby_gateway::gateway::manager::GatewayManager>,
) -> Value {
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if session_id.is_empty() {
        return payload;
    }
    let Some(events) = payload.get_mut("events").and_then(Value::as_array_mut) else {
        return payload;
    };
    let mut resource_cache = BTreeMap::<String, Value>::new();
    let mut error_cache = BTreeMap::<String, String>::new();
    for event in events {
        if !is_mcp_tool_event(event) {
            continue;
        }
        let sequence = event.get("sequence").and_then(Value::as_u64).unwrap_or(0);
        let fallback_call_id = event_call_id(event);
        let fallback_tool_input = event_tool_input(event);
        let tool_result = event_tool_result(event);
        let source = event
            .pointer("/params/item/result")
            .or_else(|| event.pointer("/params/item/output"))
            .unwrap_or(event);
        let mut bindings = Vec::new();
        collect_mcp_app_bindings(source, &fallback_call_id, 0, &mut bindings);
        bindings.sort();
        bindings.dedup();
        if bindings.is_empty() {
            continue;
        }
        let mut apps = Vec::with_capacity(bindings.len());
        for binding in bindings {
            let app_id = app_identity(sequence, &binding.call_id, &binding.resource_uri);
            if let Some(cached) = runtime.cached_mcp_app(owner, &session_id, &app_id).await {
                apps.push(cached);
                continue;
            }
            let mut app = json!({
                "id": app_id.clone(),
                "sequence": sequence,
                "callId": binding.call_id.clone(),
                "resourceUri": binding.resource_uri.clone(),
                "toolInput": binding.tool_input.clone().unwrap_or_else(|| fallback_tool_input.clone()),
                "toolResult": tool_result.clone(),
            });
            if let Some(resource) = resource_cache.get(&binding.resource_uri) {
                app["resource"] = resource.clone();
            } else if let Some(error) = error_cache.get(&binding.resource_uri) {
                app["errorKind"] = json!(error);
            } else if resource_cache.len() + error_cache.len() >= MAX_MCP_APP_RESOURCES_PER_RESPONSE
            {
                app["errorKind"] = json!("resource_budget_exceeded");
            } else if let Some(manager) = manager {
                match manager
                    .read_published_ui_resource(&binding.resource_uri)
                    .await
                {
                    Ok(resource) => match serde_json::to_value(resource) {
                        Ok(resource) => {
                            resource_cache.insert(binding.resource_uri.clone(), resource.clone());
                            app["resource"] = resource;
                        }
                        Err(_) => {
                            error_cache.insert(
                                binding.resource_uri.clone(),
                                "invalid_resource".to_owned(),
                            );
                            app["errorKind"] = json!("invalid_resource");
                        }
                    },
                    Err(error) => {
                        let kind = mcp_app_read_error_kind(&error).to_owned();
                        error_cache.insert(binding.resource_uri.clone(), kind.clone());
                        app["errorKind"] = json!(kind);
                    }
                }
            } else {
                app["errorKind"] = json!("unavailable");
            }
            if app.get("resource").is_some() {
                let _cache_result = runtime
                    .cache_mcp_app(owner, &session_id, app_id, app.clone())
                    .await;
            }
            apps.push(app);
        }
        if let Some(object) = event.as_object_mut() {
            object.insert("mcp_apps".to_owned(), Value::Array(apps));
        }
    }
    payload
}

async fn handle(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    Json(request): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let auth = auth.ok_or_else(denied)?.0;
    let identity = identity.ok_or_else(denied)?.0;
    let authority = state
        .access_runtime
        .session_authority(identity.clone())
        .await
        .map_err(|_| denied())?;
    require_platform_administrator(authority.platform_administrator)?;
    let owner = identity.safe_fingerprint().to_owned();
    let runtime = state.phoenix_runtime;
    #[cfg(feature = "gateway")]
    let gateway_manager = state.gateway_manager.clone();
    let request_headers = headers.clone();
    let request_auth = auth.clone();
    handle_action_with_meta(
        "phoenix",
        "api",
        dispatch_meta_from_headers(
            &headers,
            Some(&auth),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        request,
        crate::dispatch::phoenix::ACTIONS,
        move |action, params| async move {
            if !csrf_exempt(&action) {
                super::require_session_csrf(&action, &request_headers, Some(&request_auth))?;
            }
            let payload = runtime.dispatch(&owner, &action, params).await?;
            #[cfg(feature = "gateway")]
            {
                if matches!(
                    action.as_str(),
                    "phoenix.session.read" | "phoenix.turn.send" | "phoenix.turn.steer"
                ) {
                    return Ok(hydrate_mcp_apps(
                        payload,
                        runtime.as_ref(),
                        &owner,
                        gateway_manager.as_deref(),
                    )
                    .await);
                }
            }
            Ok(payload)
        },
    )
    .await
}

fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "Phoenix requires host-established identity".into(),
        required_scopes: vec![],
    }
}

fn require_platform_administrator(platform_administrator: bool) -> Result<(), ToolError> {
    if platform_administrator {
        Ok(())
    } else {
        Err(ToolError::Forbidden {
            message: "Phoenix requires platform administrator authority".into(),
            required_scopes: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_session_actions_are_csrf_exempt() {
        assert!(csrf_exempt("phoenix.session.list"));
        assert!(csrf_exempt("phoenix.session.read"));
        assert!(!csrf_exempt("phoenix.session.start"));
        assert!(!csrf_exempt("phoenix.session.rename"));
        assert!(!csrf_exempt("phoenix.session.close"));
    }

    #[test]
    fn lower_privilege_identity_cannot_delegate_the_owner_mcp_credential() {
        assert_eq!(
            require_platform_administrator(false).unwrap_err().kind(),
            "forbidden"
        );
        assert!(require_platform_administrator(true).is_ok());
    }

    #[test]
    fn detects_direct_upstream_mcp_app_metadata() {
        let event = json!({
            "method": "item/completed",
            "params": {
                "item": {
                    "id": "direct-call",
                    "type": "mcpToolCall",
                    "result": {
                        "_meta": {"ui": {"resourceUri": "ui://connexin/echo.html"}}
                    }
                }
            }
        });
        let mut bindings = Vec::new();
        collect_mcp_app_bindings(
            event.pointer("/params/item/result").unwrap(),
            &event_call_id(&event),
            0,
            &mut bindings,
        );
        assert!(is_mcp_tool_event(&event));
        assert_eq!(
            bindings,
            vec![McpAppBinding {
                call_id: "direct-call".to_owned(),
                resource_uri: "ui://connexin/echo.html".to_owned(),
            }]
        );
    }

    #[test]
    fn detects_nested_code_mode_mcp_app_metadata() {
        let event = json!({
            "method": "item/completed",
            "params": {
                "item": {
                    "type": "mcpToolCall",
                    "result": {
                        "structuredContent": {
                            "kind": "code_mode_execute_trace",
                            "calls": [{
                                "id": "connexin::countdown",
                                "ui": {"resourceUri": "ui://connexin/countdown.html"}
                            }]
                        }
                    }
                }
            }
        });
        let mut bindings = Vec::new();
        collect_mcp_app_bindings(
            event.pointer("/params/item/result").unwrap(),
            &event_call_id(&event),
            0,
            &mut bindings,
        );
        assert_eq!(
            bindings,
            vec![McpAppBinding {
                call_id: "connexin::countdown".to_owned(),
                resource_uri: "ui://connexin/countdown.html".to_owned(),
            }]
        );
    }

    #[test]
    fn same_resource_uri_keeps_distinct_call_identities() {
        let result = json!({
            "calls": [
                {"id": "call-a", "ui": {"resourceUri": "ui://connexin/echo.html"}},
                {"id": "call-b", "ui": {"resourceUri": "ui://connexin/echo.html"}}
            ]
        });
        let mut bindings = Vec::new();
        collect_mcp_app_bindings(&result, "outer", 0, &mut bindings);
        bindings.sort();
        assert_eq!(bindings.len(), 2);
        assert_ne!(
            app_identity(42, &bindings[0].call_id, &bindings[0].resource_uri),
            app_identity(42, &bindings[1].call_id, &bindings[1].resource_uri)
        );
    }

    #[test]
    fn app_identity_is_stable_across_session_reload() {
        let first = app_identity(17, "call-17", "ui://connexin/countdown.html");
        let reloaded = app_identity(17, "call-17", "ui://connexin/countdown.html");
        assert_eq!(first, reloaded);
    }

    #[test]
    fn ignores_malformed_and_non_ui_app_metadata() {
        let event = json!({
            "method": "item/completed",
            "params": {
                "item": {
                    "type": "mcpToolCall",
                    "result": {
                        "a": {"resourceUri": "ui://not-metadata/widget.html"},
                        "b": {"resourceUri": 42},
                        "c": {"ui": {"resourceUri": "https://example.invalid/widget"}}
                    }
                }
            }
        });
        let mut bindings = Vec::new();
        collect_mcp_app_bindings(&event, &event_call_id(&event), 0, &mut bindings);
        assert!(bindings.is_empty());
    }
}
