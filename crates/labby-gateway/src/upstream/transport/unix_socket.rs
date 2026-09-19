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
        self.inner
            .post_message(uri, message, session_id, auth_header, custom_headers)
            .await
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
        self.inner
            .post_message_with_max_sse_event_size(
                uri,
                message,
                session_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        self.inner
            .delete_session(uri, session_id, auth_header, custom_headers)
            .await
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
        self.inner
            .get_stream(uri, session_id, last_event_id, auth_header, custom_headers)
            .await
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
        self.inner
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
    }
}
