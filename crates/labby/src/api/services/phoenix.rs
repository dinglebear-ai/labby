//! Authenticated HTTP adapter for the container-local Phoenix assistant.

use std::collections::BTreeSet;
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

fn collect_mcp_app_uris(value: &Value, depth: usize, uris: &mut BTreeSet<String>) {
    if depth > 12 || uris.len() >= 8 {
        return;
    }
    match value {
        Value::Array(values) => {
            for value in values {
                collect_mcp_app_uris(value, depth + 1, uris);
            }
        }
        Value::Object(object) => {
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
                uris.insert(uri.to_owned());
            }
            for value in object.values() {
                collect_mcp_app_uris(value, depth + 1, uris);
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
    manager: Option<&labby_gateway::gateway::manager::GatewayManager>,
) -> Value {
    let Some(manager) = manager else {
        return payload;
    };
    let Some(events) = payload.get_mut("events").and_then(Value::as_array_mut) else {
        return payload;
    };
    for event in events {
        if !is_mcp_tool_event(event) {
            continue;
        }
        let mut uris = BTreeSet::new();
        collect_mcp_app_uris(event, 0, &mut uris);
        if uris.is_empty() {
            continue;
        }
        let mut apps = Vec::with_capacity(uris.len());
        for uri in uris {
            let mut app = json!({ "resourceUri": uri });
            match manager.read_published_ui_resource(&uri).await {
                Ok(resource) => match serde_json::to_value(resource) {
                    Ok(resource) => app["resource"] = resource,
                    Err(_) => app["errorKind"] = json!("invalid_resource"),
                },
                Err(error) => app["errorKind"] = json!(mcp_app_read_error_kind(&error)),
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
                    return Ok(hydrate_mcp_apps(payload, gateway_manager.as_deref()).await);
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
                    "type": "mcpToolCall",
                    "result": {
                        "_meta": {"ui": {"resourceUri": "ui://connexin/echo.html"}}
                    }
                }
            }
        });
        let mut uris = BTreeSet::new();
        collect_mcp_app_uris(&event, 0, &mut uris);
        assert!(is_mcp_tool_event(&event));
        assert_eq!(
            uris.into_iter().collect::<Vec<_>>(),
            vec!["ui://connexin/echo.html"]
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
        let mut uris = BTreeSet::new();
        collect_mcp_app_uris(&event, 0, &mut uris);
        assert_eq!(
            uris.into_iter().collect::<Vec<_>>(),
            vec!["ui://connexin/countdown.html"]
        );
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
        let mut uris = BTreeSet::new();
        collect_mcp_app_uris(&event, 0, &mut uris);
        assert!(uris.is_empty());
    }
}
