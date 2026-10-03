//! Dedicated loopback HTTP lifecycle. No operator router is mounted here.

use axum::{Router, body::Body};
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use std::net::SocketAddr;
use tokio::{
    net::TcpListener,
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
#[cfg(all(feature = "tailcat", unix))]
mod control;
#[cfg(all(feature = "tailcat", unix))]
pub(crate) use control::ControlListener;

fn serve_owned<L: axum::serve::Listener + Send + 'static>(
    mut listener: L,
    router: Router,
    stopping: CancellationToken,
    body_limit: usize,
) -> JoinHandle<()> {
    let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
    let router = router.layer(tower_http::limit::RequestBodyLimitLayer::new(body_limit));
    tokio::spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                () = stopping.cancelled() => break,
                _ = connections.join_next(), if !connections.is_empty() => {},
                (stream, _) = listener.accept() => {
                    let Ok(permit) = capacity.clone().try_acquire_owned() else { continue };
                    let router = router.clone();
                    connections.spawn(async move {
                        let _permit = permit;
                        let service = service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                            router.clone().oneshot(request.map(Body::new))
                        });
                        let _connection_result = hyper::server::conn::http1::Builder::new()
                            .timer(TokioTimer::new())
                            .header_read_timeout(std::time::Duration::from_secs(5))
                            .max_headers(32).max_buf_size(16 * 1024).keep_alive(false)
                            .serve_connection(TokioIo::new(stream), service).await;
                    });
                }
            }
        }
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    })
}

/// Construct only the selected protected MCP projection. Publication is owned
/// by the native session manager; this never mounts the operator API router.
pub(crate) fn restricted_router(
    state: super::AppState,
    route: crate::config::ProtectedMcpRouteConfig,
    authority: std::sync::Arc<crate::dispatch::tailcat::RequestAuthority>,
) -> Result<Router, crate::dispatch::tailcat::PairingError> {
    use axum::{
        http::{HeaderValue, StatusCode, header},
        response::IntoResponse as _,
    };
    use base64::Engine as _;
    let source = authority.approved.source();
    if state.web_ui_auth_disabled
        || !state
            .oauth_state
            .as_ref()
            .is_some_and(|auth| auth.config.mode == labby_auth::config::AuthMode::OAuth)
        || !state
            .access_credential_adapter
            .as_ref()
            .is_some_and(|adapter| std::sync::Arc::ptr_eq(adapter, &authority.adapter))
        || !route.enabled
        || route.name != source.route_id
        || route.public_resource() != source.resource
        || state.installation_id.as_deref() != Some(source.installation_id.as_str())
    {
        return Err(crate::dispatch::tailcat::PairingError);
    }
    let path: axum::http::Uri = route
        .public_path
        .parse()
        .map_err(|_| crate::dispatch::tailcat::PairingError)?;
    let host = HeaderValue::from_str(&route.public_host)
        .map_err(|_| crate::dispatch::tailcat::PairingError)?;
    let cleanup = std::sync::Arc::new(crate::dispatch::tailcat::cleanup::CleanupSession::new(
        authority.approved.upstream(),
    ));
    Ok(Router::new().route(
        "/mcp",
        axum::routing::any(move |mut request: axum::http::Request<Body>| {
            let state = state.clone();
            let route = route.clone();
            let authority = authority.clone();
            let cleanup = cleanup.clone();
            let path = path.clone();
            let host = host.clone();
            async move {
                fn single<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
                    let mut values = headers.get_all(name).iter();
                    let value = values.next()?.to_str().ok()?;
                    if values.next().is_some() {
                        return None;
                    }
                    Some(value)
                }
                let headers = request.headers();
                let context = single(headers, "authorization")
                    .and_then(|value| value.strip_prefix("Bearer "))
                    .zip(single(headers, "origin"))
                    .zip(single(headers, "labby-tailcat-generation"));
                let Some(((envelope, origin), generation)) = context else {
                    return StatusCode::UNAUTHORIZED.into_response();
                };
                let Ok(credential) = authority.authorize(envelope, origin, generation).await else {
                    return StatusCode::UNAUTHORIZED.into_response();
                };
                let wire = format!(
                    "Bearer {}{}_{}",
                    labby_primitives::product_credential::PRODUCT_CREDENTIAL_PREFIX,
                    credential.credential_id(),
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(credential.secret())
                );
                let Ok(mut bearer) = HeaderValue::from_str(&wire) else {
                    return StatusCode::UNAUTHORIZED.into_response();
                };
                bearer.set_sensitive(true);
                request.headers_mut().insert(header::AUTHORIZATION, bearer);
                request.headers_mut().insert(header::HOST, host);
                request.headers_mut().remove(header::COOKIE);
                // The exact browser origin was authenticated above. This inner
                // hop is native, like a desktop MCP client; its public HTTP
                // origin allowlist must not reinterpret the approved portal.
                request.headers_mut().remove(header::ORIGIN);
                request.extensions_mut().insert(cleanup);
                *request.uri_mut() = path;
                super::router::protected_mcp_route_entry(state, request, route).await
            }
        }),
    ))
}

pub(crate) struct RestrictedListener {
    address: SocketAddr,
    cancel: CancellationToken,
    task: JoinHandle<()>,
    authority_watch: Option<JoinHandle<()>>,
}
impl RestrictedListener {
    pub(crate) async fn start(router: Router) -> std::io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let cancel = CancellationToken::new();
        let task = serve_owned(listener, router, cancel.clone(), 1024 * 1024);
        Ok(Self {
            address,
            cancel,
            task,
            authority_watch: None,
        })
    }
    /// Revalidation also owns open SSE streams: expiry, revocation, or an
    /// unavailable native authority closes every connection without a new call.
    pub(crate) async fn start_authorized(
        router: Router,
        authority: std::sync::Arc<crate::dispatch::tailcat::RequestAuthority>,
        envelope: String,
    ) -> Result<Self, crate::dispatch::tailcat::PairingError> {
        authority
            .authorize(
                &envelope,
                authority.approved.origin(),
                authority.approved.generation(),
            )
            .await?;
        // Successful browser requests renew liveness; native authority polling does not.
        let activity = std::sync::Arc::new(std::sync::Mutex::new(None));
        let observed = activity.clone();
        let router = router.layer(axum::middleware::from_fn(
            move |request: axum::http::Request<Body>, next: axum::middleware::Next| {
                let observed = observed.clone();
                async move {
                    let response = next.run(request).await;
                    if response.status().is_success() {
                        if let Ok(mut last) = observed.lock() {
                            *last = Some(tokio::time::Instant::now());
                        }
                    }
                    response
                }
            },
        ));
        let mut listener = Self::start(router)
            .await
            .map_err(|_| crate::dispatch::tailcat::PairingError)?;
        let cancel = listener.cancel.clone();
        listener.authority_watch = Some(tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                }
                let idle = activity.lock().map_or(true, |last| {
                    browser_idle_expired(*last, tokio::time::Instant::now())
                });
                if idle {
                    cancel.cancel();
                    break;
                }
                let checked = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    authority.authorize(
                        &envelope,
                        authority.approved.origin(),
                        authority.approved.generation(),
                    ),
                )
                .await;
                if !matches!(checked, Ok(Ok(_))) {
                    cancel.cancel();
                    break;
                }
            }
        }));
        Ok(listener)
    }
    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }
    #[cfg(feature = "tailcat")]
    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }
}
impl Drop for RestrictedListener {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(watch) = &self.authority_watch {
            watch.abort();
        }
        self.task.abort();
    }
}

fn browser_idle_expired(last: Option<tokio::time::Instant>, now: tokio::time::Instant) -> bool {
    last.is_some_and(|last| {
        now.saturating_duration_since(last) >= std::time::Duration::from_secs(90)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn browser_liveness_has_bounded_retirement_after_first_request() {
        let now = tokio::time::Instant::now();
        assert!(!browser_idle_expired(None, now));
        assert!(!browser_idle_expired(
            Some(now),
            now + Duration::from_secs(89)
        ));
        assert!(browser_idle_expired(
            Some(now),
            now + Duration::from_secs(90)
        ));
        assert!(!browser_idle_expired(
            Some(now + Duration::from_mins(1)),
            now + Duration::from_secs(90)
        ));
    }
    use super::*;
    use axum::routing::get;
    use std::time::Duration;
    #[cfg(unix)]
    #[tokio::test]
    async fn restricted_projection_refuses_unconfigured_or_disabled_oauth() {
        use base64::Engine as _;
        let (directory, _runtime, adapter, approved) =
            crate::dispatch::tailcat::testing::fixture().await;
        let authority = std::sync::Arc::new(crate::dispatch::tailcat::RequestAuthority {
            adapter,
            source: labby_primitives::product_credential::ProductCredential::parse(&format!(
                "lby_pc_v1_source_{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
            ))
            .unwrap(),
            approved,
            key: std::sync::Arc::new(
                labby_auth::at_rest::TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap(),
            ),
        });
        let route = toml::from_str::<crate::config::ProtectedMcpRouteConfig>(
            r#"
name = "sandbox"
public_host = "labby.example"
public_path = "/sandbox"
scopes = ["lab"]
[target]
kind = "gateway_subset"
project_id = "bootstrap-default"
loadout = "sandbox"
"#,
        )
        .unwrap();
        let mut state = super::super::AppState::new();
        state.installation_id = Some("machine".into());
        state.access_credential_adapter = Some(authority.adapter.clone());
        assert!(restricted_router(state.clone(), route.clone(), authority.clone()).is_err());
        let auth_config = labby_auth::config::AuthConfig {
            mode: labby_auth::config::AuthMode::OAuth,
            public_url: Some(url::Url::parse("https://labby.example").unwrap()),
            sqlite_path: directory.path().join("auth.db"),
            key_path: directory.path().join("auth.pem"),
            google: labby_auth::config::GoogleConfig {
                client_id: "fixture-client".into(),
                client_secret: "fixture-secret".into(),
                ..Default::default()
            },
            token_encryption_key: Some(
                labby_auth::at_rest::TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap(),
            ),
            ..Default::default()
        };
        state.oauth_state = Some(std::sync::Arc::new(
            labby_auth::state::AuthState::new(auth_config)
                .await
                .unwrap(),
        ));
        let router = restricted_router(state.clone(), route.clone(), authority.clone()).unwrap();
        for (path, expected) in [
            ("/mcp", axum::http::StatusCode::UNAUTHORIZED),
            ("/v1/gateway", axum::http::StatusCode::NOT_FOUND),
            ("/health", axum::http::StatusCode::NOT_FOUND),
        ] {
            let response = router
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        state.web_ui_auth_disabled = true;
        assert!(restricted_router(state, route, authority).is_err());
    }
    #[tokio::test]
    async fn loopback_listener_drop_closes_port_and_active_streams() {
        let router = Router::new().route(
            "/mcp",
            get(|| async {
                Body::from_stream(futures::stream::pending::<
                    Result<bytes::Bytes, std::io::Error>,
                >())
            }),
        );
        let listener = RestrictedListener::start(router).await.unwrap();
        let address = listener.address();
        assert!(address.ip().is_loopback());
        let mut connection = tokio::net::TcpStream::connect(address).await.unwrap();
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        connection
            .write_all(b"GET /mcp HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut headers = [0; 1024];
        let count = tokio::time::timeout(Duration::from_secs(2), connection.read(&mut headers))
            .await
            .unwrap()
            .unwrap();
        assert!(
            std::str::from_utf8(&headers[..count])
                .unwrap()
                .starts_with("HTTP/1.1 200")
        );
        drop(listener);
        let count = tokio::time::timeout(Duration::from_secs(2), connection.read(&mut headers))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count, 0);
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
    }
}
