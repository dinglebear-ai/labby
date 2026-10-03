//! Native listener ownership and authorization retirement.
use axum::{Router, body::Body};
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use std::net::SocketAddr;
use tokio::task::JoinSet;
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
pub(crate) fn serve_owned<L: axum::serve::Listener + Send + 'static>(
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
    use super::*;
    use axum::routing::get;
    use std::time::Duration;
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
