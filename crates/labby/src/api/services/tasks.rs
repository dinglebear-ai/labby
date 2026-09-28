//! Authenticated HTTP adapter for Agent Task actions.
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
use crate::dispatch::error::ToolError;
use crate::dispatch::tasks::TaskDispatchContext;

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    crate::api::route_registry::RouteGroup::empty().route(descriptors().remove(0), post(handle))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("POST", "/", "handle", "tasks", RouteAuth::V1)
            .when("mounted only when API authentication is configured"),
    ]
}

/// Read-only actions a browser session may issue without a CSRF token.
fn csrf_exempt(action: &str) -> bool {
    matches!(
        action,
        "help" | "schema" | "tasks.list" | "tasks.get" | "tasks.result"
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
        "tasks",
        "api",
        dispatch_meta_from_headers(
            &headers,
            Some(&auth),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        request,
        crate::dispatch::tasks::ACTIONS,
        move |action, params| async move {
            if !csrf_exempt(&action) {
                super::require_session_csrf(&action, &request_headers, Some(&request_auth))?;
            }
            let store = access_runtime
                .store()
                .await
                .map_err(|error| map_runtime_error("tasks", error))?;
            crate::dispatch::tasks::dispatch(
                TaskDispatchContext {
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
        message: "Agent Task access requires host-established identity".into(),
        required_scopes: vec![],
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        Extension, Router,
        body::Body,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt as _;

    /// Every authenticated service shares the access setup gate, not just the
    /// gateway: the same uninitialized store answers HTTP 409 here too.
    #[tokio::test]
    async fn tasks_report_an_uninitialized_access_store_as_a_setup_gate() {
        let state = super::super::uninitialized_access_state().await;
        let identity = labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "static-bearer:primary",
        )
        .expect("static bearer identity");
        let auth = labby_auth::AuthContext {
            sub: "static-bearer:primary".to_string(),
            actor_key: None,
            scopes: vec!["lab:admin".to_string()],
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: None,
        };
        let app: Router = super::routes(state.clone())
            .router
            .layer(Extension(auth))
            .layer(Extension(identity))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "action": "tasks.list", "params": {} }).to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(payload["kind"], "access_setup_required", "{payload}");
        assert_eq!(payload["recovery"]["same_arguments"], "never", "{payload}");
    }
}
