use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use std::time::Instant;

use crate::authorize::{
    authorize, browser_login, callback, native_callback, native_poll, register_client,
};
use crate::error::AuthErrorKind;
use crate::metadata::{authorization_server_metadata, jwks, protected_resource_metadata};
use crate::state::AuthState;
use crate::token::{revoke, token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRouteId {
    AuthorizationServerMetadata,
    AuthorizationServerMetadataPath,
    ProtectedResourceMetadata,
    Jwks,
    Register,
    Authorize,
    BrowserLogin,
    ProviderCallback,
    NativeCallback,
    NativePoll,
    Token,
    Revoke,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthRouteSpec {
    pub id: AuthRouteId,
    pub method: &'static str,
    pub path: &'static str,
    pub browser_only: bool,
}

/// Canonical provider-aware OAuth route inventory consumed by auth routers,
/// product mounting, generated route metadata, and proxy projection.
#[must_use]
pub fn auth_route_specs(provider: crate::config::InboundProviderKind) -> Vec<AuthRouteSpec> {
    let callback = match provider {
        crate::config::InboundProviderKind::Google => "/auth/google/callback",
        crate::config::InboundProviderKind::Authelia => "/auth/oidc/callback",
    };
    vec![
        AuthRouteSpec {
            id: AuthRouteId::AuthorizationServerMetadata,
            method: "GET",
            path: "/.well-known/oauth-authorization-server",
            browser_only: false,
        },
        AuthRouteSpec {
            id: AuthRouteId::AuthorizationServerMetadataPath,
            method: "GET",
            path: "/.well-known/oauth-authorization-server/{*route}",
            browser_only: false,
        },
        AuthRouteSpec {
            id: AuthRouteId::ProtectedResourceMetadata,
            method: "GET",
            path: "/.well-known/oauth-protected-resource",
            browser_only: false,
        },
        AuthRouteSpec {
            id: AuthRouteId::Jwks,
            method: "GET",
            path: "/jwks",
            browser_only: false,
        },
        AuthRouteSpec {
            id: AuthRouteId::Register,
            method: "POST",
            path: "/register",
            browser_only: false,
        },
        AuthRouteSpec {
            id: AuthRouteId::Authorize,
            method: "GET",
            path: "/authorize",
            browser_only: false,
        },
        AuthRouteSpec {
            id: AuthRouteId::BrowserLogin,
            method: "GET",
            path: "/auth/login",
            browser_only: true,
        },
        AuthRouteSpec {
            id: AuthRouteId::ProviderCallback,
            method: "GET",
            path: callback,
            browser_only: false,
        },
        AuthRouteSpec {
            id: AuthRouteId::NativeCallback,
            method: "GET",
            path: "/native/callback",
            browser_only: true,
        },
        AuthRouteSpec {
            id: AuthRouteId::NativePoll,
            method: "POST",
            path: "/native/poll",
            browser_only: true,
        },
        AuthRouteSpec {
            id: AuthRouteId::Token,
            method: "POST",
            path: "/token",
            browser_only: false,
        },
        AuthRouteSpec {
            id: AuthRouteId::Revoke,
            method: "POST",
            path: "/revoke",
            browser_only: false,
        },
    ]
}

pub fn router(state: AuthState) -> Router {
    build_protocol_router(&state, true)
        .with_state(state)
        .layer(middleware::from_fn(auth_dispatch_observability))
}

/// Bearer-only OAuth subset router for headless consumers (e.g. syslog-mcp).
///
/// Mounts only the endpoints a non-browser MCP client needs to discover and
/// exchange tokens — `/.well-known/*`, `/jwks`, `/authorize`,
/// `/auth/google/callback`, and `/token`. Excludes:
///
/// - `/auth/login` (browser HTML — no UI on a headless service).
/// - `/register` unless dynamic registration is explicitly enabled.
/// - Any session-cookie endpoints.
///
/// Use [`router`] for the full surface (lab itself).
pub fn bearer_only_router(state: AuthState) -> Router {
    build_protocol_router(&state, false)
        .with_state(state)
        .layer(middleware::from_fn(auth_dispatch_observability))
}

fn build_protocol_router(state: &AuthState, include_browser: bool) -> Router<AuthState> {
    let mut app = Router::new();
    for spec in auth_route_specs(state.inbound_provider.kind()) {
        if (!include_browser && spec.browser_only)
            || (spec.id == AuthRouteId::Register && !state.config.enable_dynamic_registration)
        {
            continue;
        }
        app = match spec.id {
            AuthRouteId::AuthorizationServerMetadata
            | AuthRouteId::AuthorizationServerMetadataPath => {
                app.route(spec.path, get(authorization_server_metadata))
            }
            AuthRouteId::ProtectedResourceMetadata => {
                app.route(spec.path, get(protected_resource_metadata))
            }
            AuthRouteId::Jwks => app.route(spec.path, get(jwks)),
            AuthRouteId::Register => app.route(spec.path, post(register_client)),
            AuthRouteId::Authorize => app.route(spec.path, get(authorize)),
            AuthRouteId::BrowserLogin => app.route(spec.path, get(browser_login)),
            AuthRouteId::ProviderCallback => app.route(spec.path, get(callback)),
            AuthRouteId::NativeCallback => app.route(spec.path, get(native_callback)),
            AuthRouteId::NativePoll => app.route(spec.path, post(native_poll)),
            AuthRouteId::Token => app.route(spec.path, post(token)),
            AuthRouteId::Revoke => app.route(spec.path, post(revoke)),
        };
    }
    app
}

/// Provider-aware snapshot of the routes mounted by [`bearer_only_router`].
///
/// If you add or remove an endpoint in `bearer_only_router`, update this
/// list AND consider whether the change is intentional — silently
/// drifting the headless subset is the bug this snapshot exists to catch
/// (REVIEW-APPLIED #9).
#[must_use]
pub fn bearer_only_router_paths(
    provider: crate::config::InboundProviderKind,
) -> Vec<(&'static str, &'static str)> {
    auth_route_specs(provider)
        .into_iter()
        .filter(|spec| !spec.browser_only && spec.id != AuthRouteId::Register)
        .map(|spec| {
            let path = if spec.id == AuthRouteId::AuthorizationServerMetadataPath {
                "/.well-known/oauth-authorization-server/mcp"
            } else {
                spec.path
            };
            (spec.method, path)
        })
        .collect()
}

/// Paths that must NOT be mounted by [`bearer_only_router`] — verified
/// by the snapshot test. Headless MCP clients have no browser to complete a
/// native-app OAuth flow with, so `/native/callback`/`/native/poll` belong
/// here alongside the browser-only/DCR-only endpoints.
pub const BEARER_ONLY_ROUTER_FORBIDDEN_PATHS: &[(&str, &str)] = &[
    ("GET", "/auth/login"),
    ("POST", "/register"),
    ("GET", "/native/callback"),
    ("POST", "/native/poll"),
];

/// Emit the canonical API dispatch event for an inbound OAuth endpoint.
///
/// Product binaries that mount the auth handlers through adapter functions
/// must apply this middleware to their auth route group as well.
pub async fn auth_dispatch_observability(request: Request, next: Next) -> Response {
    let action = auth_dispatch_action(request.uri().path());
    let request_id = request_id(request.headers()).map(ToOwned::to_owned);
    let start = Instant::now();
    let mut response = next.run(request).await;
    let elapsed_ms = start.elapsed().as_millis();
    let status = response.status();
    let kind = response
        .extensions()
        .get::<AuthErrorKind>()
        .map(|kind| kind.0)
        .or_else(|| status_error_kind(status));

    if status.is_server_error() || status.is_client_error() {
        tracing::warn!(
            surface = "api",
            service = "auth",
            action,
            request_id = request_id.as_deref(),
            elapsed_ms,
            kind,
            status = status.as_u16(),
            "dispatch.error"
        );
    } else {
        tracing::info!(
            surface = "api",
            service = "auth",
            action,
            request_id = request_id.as_deref(),
            elapsed_ms,
            status = status.as_u16(),
            "dispatch.finish"
        );
    }

    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if matches!(action, "oauth.callback" | "oauth.native_callback") {
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
        );
    }
    response
}

fn request_id(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
}

fn status_error_kind(status: StatusCode) -> Option<&'static str> {
    if status.is_client_error() {
        Some("request_failed")
    } else if status.is_server_error() {
        Some("internal_error")
    } else {
        None
    }
}

fn auth_dispatch_action(path: &str) -> &'static str {
    for provider in [
        crate::config::InboundProviderKind::Google,
        crate::config::InboundProviderKind::Authelia,
    ] {
        for spec in auth_route_specs(provider) {
            let matched = path == spec.path
                || (spec.id == AuthRouteId::AuthorizationServerMetadataPath
                    && path.starts_with("/.well-known/oauth-authorization-server/"));
            if matched {
                return match spec.id {
                    AuthRouteId::AuthorizationServerMetadata
                    | AuthRouteId::AuthorizationServerMetadataPath => {
                        "oauth.metadata.authorization_server"
                    }
                    AuthRouteId::ProtectedResourceMetadata => "oauth.metadata.protected_resource",
                    AuthRouteId::Jwks => "oauth.jwks",
                    AuthRouteId::Register => "oauth.register",
                    AuthRouteId::Authorize => "oauth.authorize",
                    AuthRouteId::BrowserLogin => "oauth.browser_login",
                    AuthRouteId::ProviderCallback => "oauth.callback",
                    AuthRouteId::NativeCallback => "oauth.native_callback",
                    AuthRouteId::NativePoll => "oauth.native_poll",
                    AuthRouteId::Token => "oauth.token",
                    AuthRouteId::Revoke => "oauth.revoke",
                };
            }
        }
    }
    "oauth.unknown"
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use tower::util::ServiceExt;

    use axum::extract::connect_info::MockConnectInfo;
    use std::net::SocketAddr;

    use super::*;
    use crate::authorize::tests::{test_auth_config, test_auth_state_with_config};

    #[test]
    fn auth_dispatch_action_names_are_stable() {
        assert_eq!(
            auth_dispatch_action("/.well-known/oauth-authorization-server"),
            "oauth.metadata.authorization_server"
        );
        assert_eq!(auth_dispatch_action("/register"), "oauth.register");
        assert_eq!(auth_dispatch_action("/authorize"), "oauth.authorize");
        assert_eq!(auth_dispatch_action("/token"), "oauth.token");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn auth_dispatch_logs_request_id_action_elapsed_and_failure_kind() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK.lock().await;
        let buf = crate::test_support::global_tracing_buffer();

        // Build a state with dynamic registration enabled so /register is mounted.
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        // `oneshot` skips the live ConnectInfo layer the rate-limit extractor needs.
        let app = router(test_auth_state_with_config(config).await)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9001))));
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/register")
                    .header("content-type", "application/json")
                    .header("x-request-id", "req-auth-1")
                    .body(Body::from(r#"{"redirect_uris":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let logs = crate::test_support::captured_logs(&buf);
        for expected in [
            "\"surface\":\"api\"",
            "\"service\":\"auth\"",
            "\"action\":\"oauth.register\"",
            "\"request_id\":\"req-auth-1\"",
            "\"kind\":\"validation_failed\"",
            "\"status\":422",
            "\"dispatch.error\"",
        ] {
            assert!(
                logs.contains(expected),
                "missing auth dispatch log field `{expected}` in:\n{logs}"
            );
        }
        assert!(
            logs.contains("\"elapsed_ms\":"),
            "missing elapsed_ms in:\n{logs}"
        );
    }

    /// Snapshot test for [`bearer_only_router`] — sends a probe
    /// request to each path from [`bearer_only_router_paths`] and asserts
    /// the response is NOT 404 (i.e. the route is mounted), then probes
    /// each path in [`BEARER_ONLY_ROUTER_FORBIDDEN_PATHS`] and asserts
    /// IT IS 404 (i.e. the route is NOT mounted).
    ///
    /// Catches future drift where labby-auth contributors add endpoints to
    /// [`router`] but forget to keep the headless subset in lock-step.
    #[tokio::test(flavor = "current_thread")]
    async fn bearer_only_router_route_list_matches_pinned_snapshot() {
        let mut config = test_auth_config();
        config.enable_dynamic_registration = false;
        let state = test_auth_state_with_config(config).await;
        let app = bearer_only_router(state);

        for (method, path) in bearer_only_router_paths(crate::config::InboundProviderKind::Google) {
            let response = app
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "expected `{method} {path}` to be mounted on bearer_only_router \
                 but got 404 — did the route get removed without updating \
                 bearer_only_router_paths()?"
            );
        }

        for (method, path) in BEARER_ONLY_ROUTER_FORBIDDEN_PATHS {
            let response = app
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .method(*method)
                        .uri(*path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "expected `{method} {path}` to be ABSENT from bearer_only_router \
                 but got status {} — Locked Decision: bearer_only_router \
                 must NOT mount /auth/login or /register",
                response.status()
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bearer_only_router_mounts_only_the_selected_provider_callback() {
        let base = test_auth_state_with_config(test_auth_config()).await;
        let (provider, _server) =
            crate::authelia::tests::mock_provider_for_nonce("route-test").await;
        let generation = base
            .store
            .activate_inbound_provider(
                "authelia",
                provider.issuer(),
                "route-test",
                crate::util::now_unix(),
            )
            .await
            .unwrap()
            .generation;
        let state = AuthState::for_tests_with_provider(
            (*base.config).clone(),
            base.store.clone(),
            (*base.signing_keys).clone(),
            crate::oauth_provider::InboundProviderRuntime::Authelia(Box::new(provider)),
            generation,
        );
        let full = router(state.clone());
        for path in ["/auth/login", "/native/callback", "/auth/oidc/callback"] {
            let response = full
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "full router omitted {path}"
            );
        }
        let app = bearer_only_router(state);
        for (path, expected) in [
            ("/auth/oidc/callback", true),
            ("/auth/google/callback", false),
        ] {
            let response = app
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status() != StatusCode::NOT_FOUND,
                expected,
                "unexpected mount state for {path}"
            );
        }
        let wrong_method = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/auth/oidc/callback")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            wrong_method.status(),
            StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_FOUND
        ));
        let encoded = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/auth/oidc%2Fcallback")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(encoded.status(), StatusCode::NOT_FOUND);
    }
}
