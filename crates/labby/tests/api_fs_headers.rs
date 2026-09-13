#![allow(
    clippy::bool_assert_comparison,
    clippy::err_expect,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::len_zero,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::single_char_pattern,
    clippy::unnested_or_patterns
)]
//! Integration tests for lab-f1t2.12: security headers on /v1/fs error responses.
//!
//! The `/v1/fs` subrouter must set the security headers
//! (`X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`,
//! `Content-Security-Policy: default-src 'none'; sandbox`) on BOTH 200 OK
//! responses AND error responses produced by `ToolError::into_response()`.
//!
//! The earlier implementation set the headers inline on the 200 path only,
//! so error responses (404/403/422/500 JSON) served without them, permitting
//! browser MIME-sniffing on error bodies. The fix mounts a
//! `SetResponseHeaderLayer` on the subrouter.

#![cfg(feature = "fs")]

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use tower::ServiceExt;

/// Build a router that exposes /v1/fs without bearer auth. The fs subrouter
/// dispatches through the shared `api::services::fs::routes()` builder so the
/// security-header layer is exercised exactly as in production.
fn fs_router() -> Router {
    // AppState::new() leaves `workspace_root` unset, so every fs handler
    // short-circuits on `not_configured_error()` — a ToolError which flows
    // through `IntoResponse` unchanged. That is exactly the error path we
    // want to assert headers on.
    let state = labby::api::state::AppState::new();
    let router = Router::new().nest(
        "/fs",
        labby::api::services::fs::routes(state.clone()).router,
    );
    Router::new().nest("/v1", router).with_state(state)
}

fn assert_security_headers(response: &axum::response::Response) {
    let headers = response.headers();
    assert_eq!(
        headers
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff"),
        "X-Content-Type-Options must be nosniff on error responses"
    );
    assert_eq!(
        headers.get("x-frame-options").and_then(|v| v.to_str().ok()),
        Some("DENY"),
        "X-Frame-Options must be DENY on error responses"
    );
    assert_eq!(
        headers
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok()),
        Some("default-src 'none'; sandbox"),
        "Content-Security-Policy must be the sandboxed null-src CSP"
    );
}

#[tokio::test]
async fn fs_preview_error_response_carries_security_headers() {
    let app = fs_router();
    // workspace_root is unset, so this returns a ToolError / non-200.
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/fs/preview?path=anything.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(
        response.status(),
        StatusCode::OK,
        "test precondition: response must be an error (workspace_root unset)"
    );
    assert_security_headers(&response);
}

#[tokio::test]
async fn fs_preview_missing_required_query_param_carries_security_headers() {
    // Missing the required `path` query param → axum Query rejection, which
    // is produced without the handler ever running. The subrouter-level
    // layer must still cover this path.
    let app = fs_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/fs/preview")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(response.status(), StatusCode::OK);
    assert_security_headers(&response);
}

#[tokio::test]
async fn fs_list_error_response_carries_security_headers() {
    let app = fs_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/fs/list")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(response.status(), StatusCode::OK);
    assert_security_headers(&response);
}

#[tokio::test]
async fn workspace_routes_deny_project_readers_before_accessing_files() {
    for scopes in [
        vec![],
        vec!["lab:read".to_string()],
        vec!["lab".to_string()],
    ] {
        let auth = labby::api::oauth::AuthContext {
            sub: "reader".into(),
            actor_key: None,
            scopes,
            issuer: "test".into(),
            via_session: false,
            csrf_token: None,
            email: None,
        };
        let app = fs_router().layer(axum::Extension(auth));
        for path in ["/v1/fs/list", "/v1/fs/preview?path=anything.txt"] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_security_headers(&response);
        }
    }
}

#[tokio::test]
async fn installation_admin_can_preview_the_configured_workspace() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("note.txt"), "workspace text").unwrap();
    let state =
        labby::api::state::AppState::new().with_workspace_root(root.path().canonicalize().unwrap());
    let auth = labby::api::oauth::AuthContext {
        sub: "admin".into(),
        actor_key: None,
        scopes: vec!["lab:admin".into()],
        issuer: "test".into(),
        via_session: true,
        csrf_token: None,
        email: None,
    };
    let app = labby::api::services::fs::routes(state.clone())
        .router
        .layer(axum::Extension(auth))
        .with_state(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/preview?path=note.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_security_headers(&response);
    use http_body_util::BodyExt;
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "workspace text"
    );
}
