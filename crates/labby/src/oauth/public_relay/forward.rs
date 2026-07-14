use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode};
use bytes::BytesMut;
use std::error::Error as _;

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
        let target_label = request.target.redacted_label();
        let mut url = request.target.url().clone();
        let base_path = url.path().trim_end_matches('/').to_string();
        if !request.suffix_path.is_empty() {
            let mut path = base_path.clone();
            if !path.ends_with('/') {
                path.push('/');
            }
            path.push_str(request.suffix_path.trim_matches('/'));
            url.set_path(&path);
        }
        if !path_is_under_base(url.path(), &base_path) {
            return Err(PublicRelayError::InvalidSuffix(
                "normalized path escapes machine callback route".into(),
            ));
        }
        url.set_query(request.query.as_deref().filter(|query| !query.is_empty()));

        let mut builder = self.client.request(request.method, url);
        for (name, value) in &filter_public_request_headers(&request.headers) {
            builder = builder.header(name, value);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }

        let mut response = builder
            .send()
            .await
            .map_err(|error| map_forward_error("send", &target_label, error))?;
        let status = StatusCode::from_u16(response.status().as_u16())
            .map_err(|_| PublicRelayError::UpstreamError)?;
        let headers = filter_public_response_headers(response.headers());
        let mut out = BytesMut::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| map_forward_error("read_body", &target_label, error))?
        {
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

fn path_is_under_base(path: &str, base: &str) -> bool {
    path == base
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn map_forward_error(
    stage: &'static str,
    target_label: &str,
    error: reqwest::Error,
) -> PublicRelayError {
    let mapped = if error.is_timeout() {
        PublicRelayError::UpstreamTimeout
    } else {
        PublicRelayError::UpstreamError
    };
    tracing::warn!(
        surface = "api",
        service = "oauth_relay",
        action = "callback.forward",
        stage,
        target = target_label,
        kind = mapped.kind(),
        source = %reqwest_source_without_url(&error),
        "public oauth callback relay upstream failure"
    );
    mapped
}

fn reqwest_source_without_url(error: &reqwest::Error) -> String {
    let mut parts = Vec::new();
    let mut source = error.source();
    while let Some(cause) = source {
        parts.push(cause.to_string());
        source = cause.source();
    }
    if parts.is_empty() {
        if error.is_timeout() {
            "timeout".to_string()
        } else if error.is_connect() {
            "connect error".to_string()
        } else if error.is_body() {
            "body error".to_string()
        } else {
            "request error".to_string()
        }
    } else {
        parts.join(": ")
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
        seen_method: Arc<Mutex<Option<Method>>>,
        seen_uri: Arc<Mutex<Option<Uri>>>,
        seen_body: Arc<Mutex<Bytes>>,
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
    async fn public_forwarder_preserves_suffix_query_and_body() {
        let upstream =
            spawn_upstream(StatusCode::OK, HeaderMap::new(), Bytes::from_static(b"ok")).await;

        forward_to_with_parts(
            &upstream,
            "extra/path".into(),
            Some("code=abc&state=xyz".into()),
            HeaderMap::new(),
            Bytes::from_static(b"callback-body"),
        )
        .await
        .unwrap();

        assert_eq!(*upstream.seen_method.lock().unwrap(), Some(Method::POST));
        assert_eq!(
            upstream
                .seen_uri
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .to_string(),
            "/callback/dookie/extra/path?code=abc&state=xyz"
        );
        assert_eq!(
            upstream.seen_body.lock().unwrap().as_ref(),
            b"callback-body"
        );
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

    #[tokio::test]
    async fn public_forwarder_rejects_normalized_suffix_escape() {
        let upstream =
            spawn_upstream(StatusCode::OK, HeaderMap::new(), Bytes::from_static(b"ok")).await;
        let forwarder = PublicRelayForwarder::new().unwrap();
        let machine = MachineId::parse("dookie").unwrap();
        let mut url = url::Url::parse("http://100.88.16.79:38935/callback/dookie").unwrap();
        url.set_host(Some(&upstream.addr.ip().to_string())).unwrap();
        url.set_port(Some(upstream.addr.port())).unwrap();
        let target = RelayTarget::from_validated_parts_for_tests(machine, url);

        let error = forwarder
            .forward(ForwardRequest {
                method: Method::GET,
                target,
                suffix_path: "%2e%2e/%2e%2e/secret".into(),
                query: None,
                headers: HeaderMap::new(),
                body: Bytes::new(),
            })
            .await
            .expect_err("normalized suffix should not escape callback base");

        assert!(matches!(error, PublicRelayError::InvalidSuffix(_)));
        upstream.handle.abort();
    }

    async fn forward_to(
        upstream: &UpstreamHandle,
        headers: HeaderMap,
        body: Bytes,
    ) -> Result<ForwardResponse, PublicRelayError> {
        forward_to_with_parts(upstream, String::new(), None, headers, body).await
    }

    async fn forward_to_with_parts(
        upstream: &UpstreamHandle,
        suffix_path: String,
        query: Option<String>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Result<ForwardResponse, PublicRelayError> {
        let forwarder = PublicRelayForwarder::new().unwrap();
        let machine = MachineId::parse("dookie").unwrap();
        let target =
            RelayTarget::parse(machine.clone(), "http://100.88.16.79:38935/callback/dookie")
                .unwrap();
        let mut url = target.url().clone();
        url.set_host(Some(&upstream.addr.ip().to_string())).unwrap();
        url.set_port(Some(upstream.addr.port())).unwrap();
        let target = RelayTarget::from_validated_parts_for_tests(machine, url);
        forwarder
            .forward(ForwardRequest {
                method: Method::POST,
                target,
                suffix_path,
                query,
                headers,
                body,
            })
            .await
    }

    async fn spawn_upstream(status: StatusCode, headers: HeaderMap, body: Bytes) -> UpstreamHandle {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen_headers = Arc::new(Mutex::new(HeaderMap::new()));
        let seen_method = Arc::new(Mutex::new(None));
        let seen_uri = Arc::new(Mutex::new(None));
        let seen_body = Arc::new(Mutex::new(Bytes::new()));
        let state = UpstreamState {
            seen_headers: seen_headers.clone(),
            seen_method: seen_method.clone(),
            seen_uri: seen_uri.clone(),
            seen_body: seen_body.clone(),
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
            seen_method,
            seen_uri,
            seen_body,
        }
    }

    async fn upstream_handler(
        State(state): State<UpstreamState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> impl IntoResponse {
        *state.seen_method.lock().unwrap() = Some(method);
        *state.seen_uri.lock().unwrap() = Some(uri);
        *state.seen_headers.lock().unwrap() = headers;
        *state.seen_body.lock().unwrap() = body;
        (state.status, state.headers, state.body)
    }

    struct UpstreamHandle {
        addr: SocketAddr,
        handle: JoinHandle<()>,
        seen_headers: Arc<Mutex<HeaderMap>>,
        seen_method: Arc<Mutex<Option<Method>>>,
        seen_uri: Arc<Mutex<Option<Uri>>>,
        seen_body: Arc<Mutex<Bytes>>,
    }
}
