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

#[cfg(test)]
mod tests {
    use std::sync::{LazyLock, Mutex};

    use axum::{
        Extension, Router,
        body::Body,
        http::{Request, StatusCode, header},
    };
    use serde_json::json;
    use tower::ServiceExt;

    use crate::api::{oauth::AuthContext, state::AppState};

    static CONFIG_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn auth(scopes: &[&str]) -> AuthContext {
        AuthContext {
            sub: "server-logs-test".to_string(),
            actor_key: None,
            scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: Some("server-logs@example.com".to_string()),
        }
    }

    fn app_with_auth(auth: AuthContext) -> Router {
        let state = AppState::from_registry(crate::registry::build_default_registry());
        Router::new()
            .merge(super::routes(state.clone()))
            .merge(super::data_routes(state.clone()))
            .layer(Extension(auth))
            .with_state(state)
    }

    async fn request(
        app: Router,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> axum::response::Response {
        let builder = Request::builder().method(method).uri(uri);
        let builder = if body.is_some() {
            builder.header(header::CONTENT_TYPE, "application/json")
        } else {
            builder
        };
        let request = builder
            .body(Body::from(
                body.map_or_else(String::new, |body| body.to_string()),
            ))
            .expect("request");
        app.oneshot(request).await.expect("response")
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn query_get_returns_logs_for_admin_scope() {
        let _guard = CONFIG_TEST_LOCK.lock().expect("config test lock");
        let temp = tempfile::tempdir().expect("tempdir");
        let log_dir = temp.path().join("logs");
        std::fs::create_dir_all(&log_dir).expect("log dir");
        let log_path = log_dir.join("lab.test.log");
        std::fs::write(
            &log_path,
            r#"{"timestamp":"2026-07-12T00:00:01Z","level":"INFO","fields":{"message":"route ok","service":"gateway"}}"#,
        )
        .expect("write log");
        let config_path = temp.path().join("config.toml");
        let log_dir_toml =
            serde_json::to_string(&log_dir.display().to_string()).expect("serialize log dir");
        std::fs::write(&config_path, format!("[log]\ndir = {log_dir_toml}\n"))
            .expect("write config");
        crate::config::set_test_config_toml_path(Some(config_path));

        let response = request(
            app_with_auth(auth(&["lab:read", "lab:admin"])),
            "GET",
            "/query?service=gateway&limit=5",
            None,
        )
        .await;

        crate::config::set_test_config_toml_path(None);
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["kind"], "server_logs");
        assert_eq!(json["filters"]["service"], "gateway");
        assert_eq!(json["entries"][0]["message"], "route ok");
    }

    #[tokio::test]
    async fn query_routes_reject_read_only_scope() {
        let app = app_with_auth(auth(&["lab:read"]));

        let get = request(app.clone(), "GET", "/query", None).await;
        assert_eq!(get.status(), StatusCode::FORBIDDEN);

        let post = request(
            app,
            "POST",
            "/",
            Some(json!({"action": "server_logs.query", "params": {}})),
        )
        .await;
        assert_eq!(post.status(), StatusCode::FORBIDDEN);
    }
}
