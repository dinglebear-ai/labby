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
use serde::Deserialize;
use serde_json::{Map, Number, Value};

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::error::ToolError;
use crate::dispatch::server_logs::ACTIONS;

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .route("/", post(handle))
        .route("/query", get(query))
}

#[derive(Debug, Deserialize)]
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
    require_server_logs_admin(request_id, auth.as_ref())?;
    dispatch_request(headers, auth, query_request(query)).await
}

async fn handle(
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    require_server_logs_admin(request_id, auth.as_ref())?;
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

fn require_server_logs_admin(
    request_id: Option<&str>,
    auth: Option<&Extension<AuthContext>>,
) -> Result<(), ToolError> {
    if auth.is_some_and(|ctx| ctx.0.scopes.iter().any(|scope| scope == "lab:admin")) {
        return Ok(());
    }
    tracing::warn!(
        surface = "api",
        service = "server_logs",
        action = "server_logs.query",
        request_id,
        kind = "forbidden",
        "server_logs query rejected: lab:admin scope required"
    );
    Err(ToolError::Sdk {
        sdk_kind: "forbidden".to_string(),
        message: "server_logs.query requires `lab:admin` scope".to_string(),
    })
}

impl ServerLogsQuery {
    fn into_params(self) -> Value {
        let mut map = Map::new();
        insert_number(&mut map, "limit", self.limit);
        insert_number(&mut map, "max_scan_bytes", self.max_scan_bytes);
        insert_string(&mut map, "level", self.level);
        insert_string(&mut map, "target", self.target);
        insert_string(&mut map, "service", self.service);
        insert_string(&mut map, "action", self.action);
        insert_string(&mut map, "kind", self.kind);
        insert_string(&mut map, "query", self.query);
        insert_string(&mut map, "file", self.file);
        Value::Object(map)
    }
}

fn insert_string(map: &mut Map<String, Value>, key: &str, value: Option<String>) {
    let Some(value) = value.map(|value| value.trim().to_string()) else {
        return;
    };
    if !value.is_empty() {
        map.insert(key.to_string(), Value::String(value));
    }
}

fn insert_number(map: &mut Map<String, Value>, key: &str, value: Option<u64>) {
    if let Some(value) = value {
        map.insert(key.to_string(), Value::Number(Number::from(value)));
    }
}
