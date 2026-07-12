//! HTTP route group for Labby's own server process log viewer.
//!
//! This is intentionally narrow: it exposes the same `server_logs.query`
//! dispatch action used by the MCP app-backed tool. It does not reintroduce the
//! removed syslog/fleet log-ingestion service.

use axum::{
    Extension, Json, Router,
    extract::Query,
    http::HeaderMap,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::error::ToolError;
use crate::dispatch::server_logs::ACTIONS;

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new().route("/", post(handle))
}

pub fn data_routes(_state: AppState) -> Router<AppState> {
    Router::new().route("/query", get(query))
}

#[derive(Debug, Deserialize, Serialize)]
struct ServerLogsQuery {
    limit: Option<u64>,
    level: Option<String>,
    target: Option<String>,
    service: Option<String>,
    action: Option<String>,
    kind: Option<String>,
    query: Option<String>,
    file: Option<String>,
    max_scan_bytes: Option<u64>,
}

async fn query(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Query(query): Query<ServerLogsQuery>,
) -> Result<Json<Value>, ApiError> {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    require_server_logs_admin("server_logs.query", request_id, auth.as_ref())?;
    dispatch_request(headers, auth, query_request(query)).await
}

async fn handle(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    require_server_logs_admin(&req.action, request_id, auth.as_ref())?;
    dispatch_request(headers, auth, req).await
}

async fn dispatch_request(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    req: ActionRequest,
) -> Result<Json<Value>, ApiError> {
    handle_action_with_meta(
        "server_logs",
        "api",
        dispatch_meta_from_headers(&headers, auth.as_ref().map(|value| &value.0), None),
        req,
        ACTIONS,
        |action, params| async move {
            crate::dispatch::server_logs::dispatch(&action, params).await
        },
    )
    .await
}

fn query_request(query: ServerLogsQuery) -> ActionRequest {
    ActionRequest {
        action: "server_logs.query".to_string(),
        params: query.into_params(),
    }
}

fn server_logs_action_requires_admin(action: &str) -> bool {
    let bare = action.strip_prefix("server_logs.").unwrap_or(action);
    if bare == "help" || bare == "schema" {
        return false;
    }
    ACTIONS
        .iter()
        .find(|spec| spec.name == action)
        .map(|spec| spec.requires_admin)
        .unwrap_or(true)
}

fn has_admin_scope(auth: Option<&Extension<AuthContext>>) -> bool {
    auth.is_some_and(|ctx| ctx.0.scopes.iter().any(|scope| scope == "lab:admin"))
}

fn require_server_logs_admin(
    action: &str,
    request_id: Option<&str>,
    auth: Option<&Extension<AuthContext>>,
) -> Result<(), ToolError> {
    if !server_logs_action_requires_admin(action) || has_admin_scope(auth) {
        return Ok(());
    }
    tracing::warn!(
        surface = "api",
        service = "server_logs",
        action,
        request_id,
        kind = "forbidden",
        "server_logs action rejected: lab:admin scope required"
    );
    Err(ToolError::Sdk {
        sdk_kind: "forbidden".to_string(),
        message: format!("action `{action}` requires `lab:admin` scope"),
    })
}

impl ServerLogsQuery {
    fn into_params(self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
    }
}
