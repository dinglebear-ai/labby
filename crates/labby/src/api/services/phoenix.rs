//! Authenticated HTTP adapter for the container-local Phoenix assistant.

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
        RouteDescriptor::new("POST", "/", "handle", "phoenix", RouteAuth::V1)
            .when("mounted only when API authentication is configured"),
    ]
}

fn csrf_exempt(action: &str) -> bool {
    matches!(
        action,
        "help" | "schema" | "phoenix.status" | "phoenix.session.read"
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
    let owner = identity.safe_fingerprint().to_owned();
    let runtime = state.phoenix_runtime;
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
            runtime.dispatch(&owner, &action, params).await
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
