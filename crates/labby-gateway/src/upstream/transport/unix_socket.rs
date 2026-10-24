//! rmcp Unix-domain socket transport adapter.
//!
//! Socket I/O, HTTP framing, sessions, SSE, and raw-response preservation stay
//! owned by rmcp's `UnixSocketHttpClient`. This adapter only applies Labby's
//! established defensive SEP-2243 method/name headers at the final wire
//! boundary, matching the reqwest HTTP adapter.

use std::collections::HashMap;
use std::sync::Arc;

use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::ClientJsonRpcMessage;
use rmcp::transport::UnixSocketHttpClient;
use rmcp::transport::common::http_header::{HEADER_MCP_METHOD, HEADER_MCP_NAME};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};

use super::super::http_client;

#[derive(Clone)]
pub(crate) struct LabbyUnixSocketHttpClient {
    inner: UnixSocketHttpClient,
    scope_challenge_handler: Option<labby_auth::upstream::manager::OauthScopeChallengeHandler>,
}

pub(crate) fn request_uri(authority_uri: &str) -> anyhow::Result<String> {
    let parsed = url::Url::parse(authority_uri)?;
    let mut request_uri = parsed.path().to_string();
    if request_uri.is_empty() {
        request_uri.push('/');
    }
    if let Some(query) = parsed.query() {
        request_uri.push('?');
        request_uri.push_str(query);
    }
    Ok(request_uri)
}

impl LabbyUnixSocketHttpClient {
    pub(crate) fn new(socket_path: &str, authority_uri: &str, max_response_bytes: usize) -> Self {
        Self {
            inner: UnixSocketHttpClient::new(socket_path, authority_uri)
                .with_max_response_bytes(max_response_bytes),
            scope_challenge_handler: None,
        }
    }

    pub(crate) fn with_scope_challenge_handler(
        mut self,
        handler: labby_auth::upstream::manager::OauthScopeChallengeHandler,
    ) -> Self {
        self.scope_challenge_handler = Some(handler);
        self
    }

    fn observe_scope_challenge<T>(
        &self,
        result: &Result<
            T,
            StreamableHttpError<<UnixSocketHttpClient as StreamableHttpClient>::Error>,
        >,
    ) {
        if let (Some(handler), Err(StreamableHttpError::InsufficientScope(error))) =
            (&self.scope_challenge_handler, result)
        {
            if let Some(scope) = labby_auth::upstream::manager::required_scope_from_challenge(
                &error.www_authenticate_header,
            ) {
                handler(scope);
            }
        }
    }

    fn enrich_headers(
        message: &ClientJsonRpcMessage,
        mut headers: HashMap<HeaderName, HeaderValue>,
    ) -> HashMap<HeaderName, HeaderValue> {
        headers.retain(|name, _| {
            !name.as_str().eq_ignore_ascii_case(HEADER_MCP_METHOD)
                && !name.as_str().eq_ignore_ascii_case(HEADER_MCP_NAME)
        });
        if let Some(method) = http_client::jsonrpc_method_header(message) {
            headers.insert(HeaderName::from_static("mcp-method"), method);
        }
        if let Some(name) = http_client::jsonrpc_name_header(message) {
            headers.insert(HeaderName::from_static("mcp-name"), name);
        }
        headers
    }
}

impl StreamableHttpClient for LabbyUnixSocketHttpClient {
    type Error = <UnixSocketHttpClient as StreamableHttpClient>::Error;

    fn preserves_raw_responses() -> bool {
        UnixSocketHttpClient::preserves_raw_responses()
    }

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let custom_headers = Self::enrich_headers(&message, custom_headers);
        let result = self
            .inner
            .post_message(uri, message, session_id, auth_header, custom_headers)
            .await;
        self.observe_scope_challenge(&result);
        result
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let custom_headers = Self::enrich_headers(&message, custom_headers);
        let result = self
            .inner
            .post_message_with_max_sse_event_size(
                uri,
                message,
                session_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await;
        self.observe_scope_challenge(&result);
        result
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let result = self
            .inner
            .delete_session(uri, session_id, auth_header, custom_headers)
            .await;
        self.observe_scope_challenge(&result);
        result
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<sse_stream::Sse, sse_stream::Error>>,
        StreamableHttpError<Self::Error>,
    > {
        let result = self
            .inner
            .get_stream(uri, session_id, last_event_id, auth_header, custom_headers)
            .await;
        self.observe_scope_challenge(&result);
        result
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<sse_stream::Sse, sse_stream::Error>>,
        StreamableHttpError<Self::Error>,
    > {
        let result = self
            .inner
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await;
        self.observe_scope_challenge(&result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unix_scope_challenge_is_observed_without_replaying_the_request() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("scope.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
                assert!(request.len() < 16 * 1024);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            stream.write_all(b"HTTP/1.1 403 Forbidden\r\nWWW-Authenticate: Bearer error=\"insufficient_scope\", scope=\"files:write\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            stream.flush().await.unwrap();
        });
        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&observed);
        let client =
            LabbyUnixSocketHttpClient::new(socket.to_str().unwrap(), "http://local.test/mcp", 1024)
                .with_scope_challenge_handler(Arc::new(move |scope| {
                    recorded.lock().unwrap().push(scope)
                }));
        let request = serde_json::from_value(
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        )
        .unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.post_message("/mcp".into(), request, None, None, HashMap::new()),
        )
        .await
        .unwrap();
        assert!(matches!(
            result,
            Err(StreamableHttpError::InsufficientScope(_))
        ));
        assert_eq!(*observed.lock().unwrap(), vec!["files:write"]);
        tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn request_uri_preserves_path_and_query_without_authority() {
        assert_eq!(
            request_uri("http://cortex.internal/mcp?tenant=infra").expect("valid URL"),
            "/mcp?tenant=infra"
        );
        assert_eq!(
            request_uri("http://cortex.internal").expect("valid URL"),
            "/"
        );
    }

    #[test]
    fn defensive_headers_replace_stale_method_and_name_values() {
        let message: ClientJsonRpcMessage = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "unix_echo",
                "arguments": {}
            }
        }))
        .expect("valid MCP request");
        let headers = HashMap::from([
            (
                HeaderName::from_static("mcp-method"),
                HeaderValue::from_static("wrong/method"),
            ),
            (
                HeaderName::from_static("mcp-name"),
                HeaderValue::from_static("wrong-name"),
            ),
            (
                HeaderName::from_static("x-labby-test"),
                HeaderValue::from_static("present"),
            ),
        ]);

        let headers = LabbyUnixSocketHttpClient::enrich_headers(&message, headers);

        assert_eq!(
            headers
                .get(&HeaderName::from_static("mcp-method"))
                .and_then(|value| value.to_str().ok()),
            Some("tools/call")
        );
        assert_eq!(
            headers
                .get(&HeaderName::from_static("mcp-name"))
                .and_then(|value| value.to_str().ok()),
            Some("unix_echo")
        );
        assert_eq!(
            headers
                .get(&HeaderName::from_static("x-labby-test"))
                .and_then(|value| value.to_str().ok()),
            Some("present")
        );
    }

    #[test]
    fn unix_socket_adapter_preserves_raw_responses() {
        assert!(LabbyUnixSocketHttpClient::preserves_raw_responses());
    }

    #[tokio::test]
    async fn unix_socket_adapter_forwards_response_body_cap() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        use tokio::net::UnixListener;

        let tempdir = tempfile::Builder::new()
            .prefix("lb-uds-cap-")
            .tempdir_in("/tmp")
            .expect("short temporary directory");
        let socket_path = tempdir.path().join("mcp.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind Unix socket");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).await.expect("read request");
            assert!(read > 0, "client must send an HTTP request");
            let body = vec![b'x'; 1024];
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(head.as_bytes())
                .await
                .expect("write headers");
            stream.write_all(&body).await.expect("write body");
            stream.flush().await.expect("flush response");
        });

        let message: ClientJsonRpcMessage = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "server/discover",
            "params": {}
        }))
        .expect("valid request");
        let client = LabbyUnixSocketHttpClient::new(
            socket_path.to_string_lossy().as_ref(),
            "http://cap.internal/mcp",
            128,
        );
        let error = client
            .post_message(Arc::from("/mcp"), message, None, None, HashMap::new())
            .await
            .expect_err("oversized response must fail before JSON decoding");
        assert!(
            error.to_string().contains("response_too_large"),
            "unexpected error: {error}"
        );

        server.await.expect("Unix response-cap server task");
    }
}
