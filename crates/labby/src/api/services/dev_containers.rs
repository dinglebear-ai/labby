//! Authenticated HTTP adapter for Dev Container actions.
//!
//! The handler only establishes the caller context (session CSRF, verified
//! identity, authority ceiling) and hands the request to shared dispatch
//! through the common API wrapper. Parameter validation, owner-scope
//! resolution, and authorization decisions live in
//! `crate::dispatch::dev_containers`.

use std::net::SocketAddr;

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    routing::post,
};
use labby_auth::{AuthContext, VerifiedIdentity};
use serde_json::Value;

use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, error::ApiError, state::AppState};
use crate::dispatch::error::ToolError;

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    crate::api::route_registry::RouteGroup::empty().route(descriptors().remove(0), post(handle))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("POST", "/", "handle", "dev_containers", RouteAuth::V1)
            .when("mounted only when API authentication is configured"),
    ]
}

/// Read-only actions a browser session may issue without a CSRF token.
fn csrf_exempt(action: &str) -> bool {
    matches!(action, "help" | "schema" | "dev_containers.list")
}

async fn handle(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    Json(request): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let auth = auth.ok_or_else(denied)?;
    let identity = identity.ok_or_else(denied)?.0;
    if !csrf_exempt(&request.action) {
        super::require_session_csrf(&request.action, &headers, Some(&auth.0))?;
    }
    let ceiling = crate::access::AuthorityCeiling::from_auth_context(&auth.0);
    let access_runtime = state.access_runtime;
    handle_action_with_meta(
        "dev_containers",
        "api",
        dispatch_meta_from_headers(
            &headers,
            Some(&auth.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        request,
        crate::dispatch::dev_containers::ACTIONS,
        move |action, params| async move {
            crate::dispatch::dev_containers::dispatch(
                crate::dispatch::dev_containers::DevContainerDispatchContext {
                    access_runtime,
                    identity,
                    ceiling,
                },
                &action,
                params,
            )
            .await
        },
    )
    .await
}

fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "Dev Container operation is not authorized".into(),
        required_scopes: Vec::new(),
    }
}
