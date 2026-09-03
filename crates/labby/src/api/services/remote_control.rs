//! Thin HTTP adapters for remote Artifact control-plane services.

use std::net::SocketAddr;

use axum::{
    Extension, Json,
    body::Bytes,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, header},
    routing::{post, put},
};
use serde::Deserialize;
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};

pub fn routes(service: &'static str, _state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    let handler = match service {
        "sources" => post(handle_sources),
        "jobs" => post(handle_jobs),
        "uploads" => post(handle_uploads),
        "bundles" => post(handle_bundles),
        _ => return RouteGroup::empty(),
    };
    let mut route_descriptors = descriptors(service).into_iter();
    let group = RouteGroup::empty().route(route_descriptors.next().unwrap(), handler);
    if service == "uploads" {
        group.route(
            route_descriptors.next().unwrap(),
            put(upload_bytes).layer(axum::extract::DefaultBodyLimit::max(50_000_000)),
        )
    } else {
        group
    }
}

pub(crate) fn descriptors(
    service: &'static str,
) -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    let mut descriptors = vec![
        RouteDescriptor::new("POST", "/", "handle", service, RouteAuth::V1)
            .feature("skills")
            .when("mounted only when API authentication is configured")
            .host_validated(),
    ];
    if service == "uploads" {
        descriptors.push(
            RouteDescriptor::new("PUT", "/{id}", "upload_bytes", service, RouteAuth::V1)
                .feature("skills")
                .when("mounted only when API authentication is configured")
                .host_validated()
                .private_no_store()
                .side_effects("stores bounded bytes in a principal-bound remote upload slot"),
        );
    }
    descriptors
}

#[derive(Debug, Deserialize)]
struct UploadQuery {
    connection_id: Option<String>,
}

async fn upload_bytes(
    auth: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
    Query(query): Query<UploadQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let is_admin = auth
        .as_ref()
        .is_some_and(|Extension(auth)| auth.scopes.iter().any(|scope| scope == "lab:admin"));
    if !is_admin {
        return Err(crate::dispatch::error::ToolError::Forbidden {
            message: "Artifact uploads require lab:admin scope".to_owned(),
            required_scopes: vec!["lab:admin".to_owned()],
        }
        .into());
    }
    if body.len() > 50_000_000 {
        return Err(crate::dispatch::error::ToolError::InvalidParam {
            message: "Artifact upload exceeds 50000000 bytes".to_owned(),
            param: "body".to_owned(),
        }
        .into());
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream");
    let controls = crate::dispatch::skill_library::process_controls().ok_or_else(|| {
        crate::dispatch::error::ToolError::Sdk {
            sdk_kind: "source_unavailable".to_owned(),
            message: "Remote Artifact control plane is unavailable".to_owned(),
        }
    })?;
    controls
        .upload(
            query.connection_id.as_deref(),
            &id,
            body.to_vec(),
            content_type,
        )
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn handle_sources(
    state: State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    handle("sources", state, peer, headers, auth, body).await
}
async fn handle_jobs(
    state: State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    handle("jobs", state, peer, headers, auth, body).await
}
async fn handle_uploads(
    state: State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    handle("uploads", state, peer, headers, auth, body).await
}
async fn handle_bundles(
    state: State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    body: Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    handle("bundles", state, peer, headers, auth, body).await
}

async fn handle(
    service: &'static str,
    State(_state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    handle_action_with_meta(
        service,
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        crate::dispatch::remote_control::actions(service),
        move |action, params| async move {
            crate::dispatch::remote_control::dispatch(service, &action, params).await
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use axum::body::Bytes;
    use axum::response::IntoResponse;

    use super::*;

    fn auth(scopes: &[&str]) -> Option<Extension<AuthContext>> {
        Some(Extension(AuthContext {
            sub: "operator".to_owned(),
            actor_key: None,
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            issuer: "test".to_owned(),
            via_session: false,
            csrf_token: None,
            email: None,
        }))
    }

    #[tokio::test]
    async fn raw_upload_requires_admin_before_remote_dispatch() {
        let error = upload_bytes(
            None,
            Path("upload-1".to_owned()),
            Query(UploadQuery {
                connection_id: None,
            }),
            HeaderMap::new(),
            Bytes::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.into_response().status(),
            axum::http::StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn raw_upload_rejects_non_admin_execution_scope() {
        let error = upload_bytes(
            auth(&["lab"]),
            Path("upload-1".to_owned()),
            Query(UploadQuery {
                connection_id: None,
            }),
            HeaderMap::new(),
            Bytes::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.into_response().status(),
            axum::http::StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn raw_upload_enforces_bound_before_remote_dispatch() {
        let error = upload_bytes(
            auth(&["lab:admin"]),
            Path("upload-1".to_owned()),
            Query(UploadQuery {
                connection_id: None,
            }),
            HeaderMap::new(),
            Bytes::from(vec![0; 50_000_001]),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.into_response().status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}
