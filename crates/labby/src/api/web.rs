use super::state::AppState;
use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};

/// Whether the embedded SPA bundle (`index.html`) shipped in this binary.
///
/// Delegates to `labby-web`, which owns the build-time embedded asset
/// table. Used by the serve path and router tests to decide whether the
/// embedded fallback is meaningful. Always `false` when the `web-ui`
/// feature (pulled in by `gateway`) is disabled, since no asset bundle is
/// compiled in for that build.
#[cfg(feature = "web-ui")]
pub fn embedded_web_assets_available() -> bool {
    labby_web::embedded_assets_available()
}

#[cfg(not(feature = "web-ui"))]
pub fn embedded_web_assets_available() -> bool {
    false
}

/// Turn a resolved asset into an axum response, honoring `HEAD` (headers only).
#[cfg(feature = "web-ui")]
fn web_asset_response(asset: labby_web::AssetResponse, method: &Method) -> Response {
    use axum::body::Body;
    use axum::http::header;

    let labby_web::AssetResponse {
        bytes,
        content_type,
        cache_control,
    } = asset;
    let mut response = Response::new(if *method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(bytes)
    });
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(cache_control),
    );
    response
}

#[cfg(feature = "web-ui")]
pub async fn serve_web_request(State(state): State<AppState>, request: Request) -> Response {
    use labby_web::{AssetSource, serve_asset};

    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return StatusCode::NOT_FOUND.into_response();
    }

    // The web router is the product's final fallback, so an extensionless miss
    // is not automatically a browser navigation. Keep the asset crate's SPA
    // resolution reusable, but only opt product-owned UI routes into its
    // index.html fallback. Otherwise absent API/protocol routes (including a
    // configured protected-MCP host whose route is currently absent) would be
    // converted into successful HTML responses.
    let path = request.uri().path();
    if !is_web_navigation(path) && std::path::Path::new(path).extension().is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Source precedence is product policy and stays here: a configured directory
    // always wins; the embedded bundle is the no-config fallback. When neither
    // is available there is nothing to serve. Asset lookup, symlink-escape
    // rejection, and header derivation live in `labby-web`.
    let source = if let Some(base_dir) = state.web_assets_dir.as_deref() {
        AssetSource::Directory(base_dir.to_path_buf())
    } else if state.embedded_web_assets {
        AssetSource::Embedded
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match serve_asset(request.uri().path(), &source).await {
        Ok(asset) => web_asset_response(asset, request.method()),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(feature = "web-ui")]
fn is_web_navigation(path: &str) -> bool {
    const EXACT: &[&str] = &[
        "/",
        "/404",
        "/agents",
        "/administration",
        "/create",
        "/design-system",
        "/depot",
        "/dev-containers",
        "/docs",
        "/gateway",
        "/gateways",
        "/loadouts",
        "/library",
        "/logs",
        "/mcp/code-mode",
        "/settings",
        "/skills",
        "/snippets",
        "/stash",
        "/tasks",
        "/tools",
        "/traces",
        "/usage",
    ];

    // Preserve the root route: stripping its only slash would turn it into an
    // empty string and incorrectly disable the embedded index page.
    let normalized = if path == "/" {
        path
    } else {
        path.strip_suffix('/').unwrap_or(path)
    };
    EXACT.contains(&normalized) || normalized.starts_with("/settings/")
}

/// No asset bundle is compiled in without `web-ui` — the SPA fallback route
/// is never mounted in that build (see `router.rs`'s `web_assets_enabled()`
/// gate), but this stub keeps the handler path referenceable from shared,
/// always-compiled code.
#[cfg(not(feature = "web-ui"))]
pub async fn serve_web_request(State(_state): State<AppState>, _request: Request) -> Response {
    StatusCode::NOT_FOUND.into_response()
}
