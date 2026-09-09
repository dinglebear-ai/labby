//! Authenticated HTTP adapter for owner-scoped Agent actions.
//!
//! The handler only establishes the caller context (verified identity,
//! transport ceiling, session CSRF for mutations) and hands the request to
//! shared dispatch through the common API wrapper so dispatch logging,
//! request-id correlation, and surface policy apply uniformly.

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
use crate::dispatch::access_errors::map_runtime_error;
use crate::dispatch::agents::AgentDispatchContext;
use crate::dispatch::error::ToolError;

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    crate::api::route_registry::RouteGroup::empty().route(descriptors().remove(0), post(handle))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("POST", "/", "handle", "agents", RouteAuth::V1)
            .when("mounted only when API authentication is configured"),
    ]
}

/// Read-only actions a browser session may issue without a CSRF token.
fn csrf_exempt(action: &str) -> bool {
    matches!(
        action,
        "help" | "schema" | "agents.list" | "agents.get" | "agents.session.status"
    )
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
    let ceiling = crate::access::AuthorityCeiling::from_auth_context(&auth);
    let access_runtime = state.access_runtime;
    // The dispatch wrapper borrows the request headers and auth context for
    // logging; the CSRF check inside the closure needs its own copies.
    let request_headers = headers.clone();
    let request_auth = auth.clone();
    handle_action_with_meta(
        "agents",
        "api",
        dispatch_meta_from_headers(
            &headers,
            Some(&auth),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        request,
        crate::dispatch::agents::ACTIONS,
        move |action, params| async move {
            if !csrf_exempt(&action) {
                super::require_session_csrf(&action, &request_headers, Some(&request_auth))?;
            }
            let store = access_runtime
                .store()
                .await
                .map_err(|error| map_runtime_error("agents", error))?;
            crate::dispatch::agents::dispatch(
                AgentDispatchContext {
                    store,
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
        message: "Agent access requires host-established identity".into(),
        required_scopes: vec![],
    }
}
