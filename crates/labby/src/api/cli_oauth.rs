//! Public metadata for the product's native operator client.

use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use super::{
    AppState,
    route_registry::{RouteAuth, RouteDescriptor},
};

pub(crate) fn descriptor() -> RouteDescriptor {
    RouteDescriptor::new(
        "GET",
        crate::oauth::cli_session::CLIENT_METADATA_PATH,
        "cli_client_metadata",
        "discovery",
        RouteAuth::Public,
    )
    .when("returns metadata only when an HTTPS public URL is configured")
}

pub(crate) async fn metadata(State(state): State<AppState>) -> Response {
    let Some(server) = state
        .auth_config
        .as_ref()
        .and_then(|config| config.public_url.as_ref())
        .filter(|url| url.scheme() == "https")
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match crate::oauth::cli_session::client_metadata(server) {
        Ok(document) => (
            [(header::CACHE_CONTROL, "public, max-age=300")],
            Json(document),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
