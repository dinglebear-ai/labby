//! Embedded app manifest and static asset route handlers.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderName, HeaderValue, header};
use axum::response::{Html, IntoResponse, Response};

use super::error::ApiError;
use super::state::AppState;
use crate::dispatch::error::ToolError;

pub(super) async fn apps_manifest(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manifest =
        crate::app_manifest::manifest_for_registry(&state.registry).map_err(|error| {
            ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!(
                    "app `{}` references missing action `{}/{}`",
                    error.app_slug, error.service, error.action
                ),
            }
        })?;
    let value = serde_json::to_value(manifest).map_err(|error| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to serialize app manifest: {error}"),
    })?;
    Ok(Json(value))
}

pub(super) async fn apps_launcher_page() -> Response {
    no_store_html(crate::app_assets::APPS_LAUNCHER_HTML)
}

pub(super) async fn server_logs_app_page() -> Response {
    no_store_html(crate::app_assets::SERVER_LOGS_APP_HTML)
}

fn no_store_html(body: &'static str) -> Response {
    let mut response = Html(body).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

pub(super) async fn labby_app_host_js() -> Response {
    let mut response = (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        crate::app_assets::LABBY_APP_HOST_JS,
    )
        .into_response();
    response.headers_mut().insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    response
}
