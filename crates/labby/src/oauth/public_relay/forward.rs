use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode};
use bytes::BytesMut;

use super::policy::{
    PUBLIC_CONNECT_TIMEOUT, PUBLIC_READ_TIMEOUT, PUBLIC_RESPONSE_LIMIT_BYTES, PUBLIC_TOTAL_TIMEOUT,
    filter_public_request_headers, filter_public_response_headers,
};
use super::types::{PublicRelayError, RelayTarget};

pub struct PublicRelayForwarder {
    client: reqwest::Client,
}

pub struct ForwardRequest {
    pub method: Method,
    pub target: RelayTarget,
    pub suffix_path: String,
    pub query: Option<String>,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Debug)]
pub struct ForwardResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl PublicRelayForwarder {
    pub fn new() -> Result<Self, PublicRelayError> {
        drop(rustls::crypto::ring::default_provider().install_default());
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(PUBLIC_CONNECT_TIMEOUT)
            .read_timeout(PUBLIC_READ_TIMEOUT)
            .timeout(PUBLIC_TOTAL_TIMEOUT)
            .no_gzip()
            .build()
            .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
        Ok(Self { client })
    }

    pub async fn forward(
        &self,
        request: ForwardRequest,
    ) -> Result<ForwardResponse, PublicRelayError> {
        let mut url = request.target.url.clone();
        if !request.suffix_path.is_empty() {
            let mut path = url.path().trim_end_matches('/').to_string();
            if !path.ends_with('/') {
                path.push('/');
            }
            path.push_str(request.suffix_path.trim_matches('/'));
            url.set_path(&path);
        }
        url.set_query(request.query.as_deref().filter(|query| !query.is_empty()));

        let mut builder = self.client.request(request.method, url);
        for (name, value) in &filter_public_request_headers(&request.headers) {
            builder = builder.header(name, value);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }

        let mut response = builder.send().await.map_err(|error| {
            if error.is_timeout() {
                PublicRelayError::UpstreamTimeout
            } else {
                PublicRelayError::UpstreamError
            }
        })?;
        let status = StatusCode::from_u16(response.status().as_u16())
            .map_err(|_| PublicRelayError::UpstreamError)?;
        let headers = filter_public_response_headers(response.headers());
        let mut out = BytesMut::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            if error.is_timeout() {
                PublicRelayError::UpstreamTimeout
            } else {
                PublicRelayError::UpstreamError
            }
        })? {
            if out.len() + chunk.len() > PUBLIC_RESPONSE_LIMIT_BYTES {
                return Err(PublicRelayError::ResponseTooLarge);
            }
            out.extend_from_slice(&chunk);
        }
        Ok(ForwardResponse {
            status,
            headers,
            body: out.freeze(),
        })
    }
}

impl Default for PublicRelayForwarder {
    fn default() -> Self {
        Self::new().expect("public relay forwarder configuration is valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    use axum::{
        Router,
        body::Bytes,
        extract::State,
        http::{HeaderValue, Uri, header},
        response::IntoResponse,
        routing::any,
    };
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use crate::oauth::public_relay::{MachineId, PUBLIC_RESPONSE_LIMIT_BYTES};

    #[derive(Clone)]
    struct UpstreamState {
        seen_headers: Arc<Mutex<HeaderMap>>,
        status: StatusCode,
        headers: HeaderMap,
        body: Bytes,
    }

    #[tokio::test]
    async fn public_forwarder_does_not_follow_redirects() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::LOCATION,
            HeaderValue::from_static("https://example.com"),
        );
        let upstream =
            spawn_upstream(StatusCode::FOUND, headers, Bytes::from_static(b"redirect")).await;
        let response = forward_to(&upstream, HeaderMap::new(), Bytes::new())
            .await
            .unwrap();

        assert_eq!(response.status, StatusCode::FOUND);
        upstream.handle.abort();
    }

    #[tokio::test]
    async fn public_forwarder_strips_location_and_set_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::LOCATION,
            HeaderValue::from_static("https://example.com"),
        );
        headers.insert(header::SET_COOKIE, HeaderValue::from_static("secret=1"));
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        let upstream = spawn_upstream(StatusCode::OK, headers, Bytes::from_static(b"ok")).await;
        let response = forward_to(&upstream, HeaderMap::new(), Bytes::new())
            .await
            .unwrap();

        assert!(response.headers.get(header::LOCATION).is_none());
        assert!(response.headers.get(header::SET_COOKIE).is_none());
        assert!(response.headers.get(header::CONTENT_TYPE).is_some());
        upstream.handle.abort();
    }

    #[tokio::test]
    async fn public_forwarder_rejects_large_response() {
        let upstream = spawn_upstream(
            StatusCode::OK,
            HeaderMap::new(),
            Bytes::from(vec![b'a'; PUBLIC_RESPONSE_LIMIT_BYTES + 1]),
        )
        .await;

        let error = forward_to(&upstream, HeaderMap::new(), Bytes::new())
            .await
            .expect_err("large response should fail");

        assert!(matches!(error, PublicRelayError::ResponseTooLarge));
        upstream.handle.abort();
    }

    #[tokio::test]
    async fn public_forwarder_drops_auth_and_cookie_request_headers() {
        let upstream =
            spawn_upstream(StatusCode::OK, HeaderMap::new(), Bytes::from_static(b"ok")).await;
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer secret"),
        );
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("lab_session=secret"),
        );
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));

        forward_to(&upstream, headers, Bytes::new()).await.unwrap();

        let seen = upstream.seen_headers.lock().unwrap().clone();
        assert!(seen.get(header::AUTHORIZATION).is_none());
        assert!(seen.get(header::COOKIE).is_none());
        assert!(seen.get(header::CONTENT_TYPE).is_some());
        upstream.handle.abort();
    }

    async fn forward_to(
        upstream: &UpstreamHandle,
        headers: HeaderMap,
        body: Bytes,
    ) -> Result<ForwardResponse, PublicRelayError> {
        let forwarder = PublicRelayForwarder::new().unwrap();
        let machine = MachineId::parse("dookie").unwrap();
        let target = RelayTarget::parse(
            machine,
            &format!("http://{}:38935/callback/dookie", upstream.addr.ip()),
        )
        .unwrap_or_else(|_| {
            RelayTarget::parse(
                MachineId::parse("dookie").unwrap(),
                "http://100.88.16.79:38935/callback/dookie",
            )
            .unwrap()
        });
        let mut target = target;
        target
            .url
            .set_host(Some(&upstream.addr.ip().to_string()))
            .unwrap();
        target.url.set_port(Some(upstream.addr.port())).unwrap();
        forwarder
            .forward(ForwardRequest {
                method: Method::POST,
                target,
                suffix_path: String::new(),
                query: None,
                headers,
                body,
            })
            .await
    }

    async fn spawn_upstream(status: StatusCode, headers: HeaderMap, body: Bytes) -> UpstreamHandle {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen_headers = Arc::new(Mutex::new(HeaderMap::new()));
        let state = UpstreamState {
            seen_headers: seen_headers.clone(),
            status,
            headers,
            body,
        };
        let app = Router::new()
            .fallback(any(upstream_handler))
            .with_state(state);
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        UpstreamHandle {
            addr,
            handle,
            seen_headers,
        }
    }

    async fn upstream_handler(
        State(state): State<UpstreamState>,
        _method: Method,
        _uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> impl IntoResponse {
        drop(body);
        *state.seen_headers.lock().unwrap() = headers;
        (state.status, state.headers, state.body)
    }

    struct UpstreamHandle {
        addr: SocketAddr,
        handle: JoinHandle<()>,
        seen_headers: Arc<Mutex<HeaderMap>>,
    }
}
