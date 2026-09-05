//! HTTP client wrapper that enforces a maximum response body size at the
//! [`StreamableHttpClient`] trait layer, BEFORE deserialization.
//!
//! Background — the gateway proxies upstream MCP servers over HTTP via
//! rmcp's `StreamableHttpClientTransport`. Without a body cap, a hostile or
//! buggy upstream can return a multi-GB response that OOMs the gateway
//! before the post-hoc size checks in `pool/tools_call.rs`,
//! `pool/resources_read.rs`, and `pool/prompts_get.rs` ever fire. This
//! wrapper inserts the cap at the transport layer.
//!
//! Cap semantics:
//! - `post_message` → `Json(_, _)`: cumulative cap on the buffered body.
//! - `post_message` → `Sse(_, _)`: PER-EVENT cap (not cumulative), so
//!   long-lived legitimate SSE subscriptions are not disconnected.
//! - `get_stream`: PER-EVENT cap (not cumulative).
//! - Session headers and deletion are forwarded for legacy upstream lifecycle
//!   compatibility. Labby's downstream MCP endpoint remains stateless.
//!
//! The cap applies to DECODED bytes — reqwest auto-decodes
//! `Content-Encoding: gzip|br|zstd` by default, and `bytes_stream()` yields
//! decoded chunks. A 1 KB gzip-bomb expanding to 50 MB therefore trips the
//! cap correctly.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use futures::StreamExt;
use futures::stream::BoxStream;
use reqwest::header::{ACCEPT, HeaderName, HeaderValue, WWW_AUTHENTICATE};
use rmcp::model::{ClientJsonRpcMessage, JsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::common::http_header::{
    BASE64_HEADER_PREFIX, BASE64_HEADER_SUFFIX, EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID,
    HEADER_MCP_METHOD, HEADER_MCP_NAME, HEADER_SESSION_ID, JSON_MIME_TYPE,
};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, InsufficientScopeError, SseError, StreamableHttpClient, StreamableHttpError,
    StreamableHttpPostResponse,
};
use sse_stream::{Sse, SseStream};

// Re-implement the SDK's small reserved-header and OAuth-scope helpers locally
// so the body-capped adapter can stay transport-compatible without forking rmcp.
const RESERVED_HEADERS: &[&str] = &[
    "accept",
    HEADER_SESSION_ID,
    HEADER_MCP_METHOD, // allowed through; the adapter overwrites it from the body
    HEADER_MCP_NAME,   // allowed through; the adapter overwrites it from the body
    "MCP-Protocol-Version", // allowed through; worker injects post-init
    HEADER_LAST_EVENT_ID,
];

fn validate_custom_header(name: &HeaderName) -> Result<(), String> {
    if RESERVED_HEADERS
        .iter()
        .any(|&r| name.as_str().eq_ignore_ascii_case(r))
    {
        if name.as_str().eq_ignore_ascii_case("MCP-Protocol-Version")
            || name.as_str().eq_ignore_ascii_case(HEADER_MCP_METHOD)
            || name.as_str().eq_ignore_ascii_case(HEADER_MCP_NAME)
        {
            return Ok(());
        }
        return Err(name.to_string());
    }
    Ok(())
}

fn extract_scope_from_header(header: &str) -> Option<String> {
    let (scheme, parameters) = header.trim().split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    for parameter in parameters.split(',') {
        let Some((name, raw_value)) = parameter.trim().split_once('=') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("scope") {
            continue;
        }
        let value = raw_value.trim();
        if let Some(quoted) = value.strip_prefix('"') {
            return quoted.find('"').map(|end| quoted[..end].to_string());
        }
        let end = value
            .find(|character: char| character == ';' || character.is_whitespace())
            .unwrap_or(value.len());
        if end != 0 {
            return Some(value[..end].to_string());
        }
    }
    None
}

/// Wraps a [`reqwest::Client`] and enforces a per-response decoded-body
/// size cap at the [`StreamableHttpClient`] trait layer.
#[derive(Clone)]
pub struct BodyCappedHttpClient {
    inner: reqwest::Client,
    max_bytes: usize,
    response_budget: Arc<tokio::sync::Semaphore>,
    response_weight: u32,
}

const RESPONSE_BUDGET_QUANTUM: usize = 1024;
const AGGREGATE_RESPONSE_BUDGET_BYTES: usize = 80 * 1024 * 1024;
const AGGREGATE_RESPONSE_BUDGET_PERMITS: usize =
    AGGREGATE_RESPONSE_BUDGET_BYTES / RESPONSE_BUDGET_QUANTUM;
static GLOBAL_RESPONSE_BUDGET: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
const RESPONSE_BUDGET_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

fn effective_response_limit(max_bytes: usize) -> usize {
    max_bytes.min(AGGREGATE_RESPONSE_BUDGET_BYTES)
}

impl BodyCappedHttpClient {
    #[must_use]
    pub fn new(inner: reqwest::Client, max_bytes: usize) -> Self {
        let max_bytes = effective_response_limit(max_bytes);
        let response_weight = max_bytes
            .div_ceil(RESPONSE_BUDGET_QUANTUM)
            .max(1)
            .min(AGGREGATE_RESPONSE_BUDGET_PERMITS);
        let response_budget = Arc::clone(GLOBAL_RESPONSE_BUDGET.get_or_init(|| {
            Arc::new(tokio::sync::Semaphore::new(
                AGGREGATE_RESPONSE_BUDGET_PERMITS,
            ))
        }));
        Self {
            inner,
            max_bytes,
            response_budget,
            response_weight: u32::try_from(response_weight).unwrap_or(u32::MAX),
        }
    }

    #[must_use]
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    async fn acquire_response_budget(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, StreamableHttpError<reqwest::Error>> {
        acquire_response_permit(Arc::clone(&self.response_budget), self.response_weight)
            .await
            .map_err(|error| {
                StreamableHttpError::UnexpectedServerResponse(Cow::Borrowed(match error {
                    CappedResponseBodyError::BudgetExhausted => "response_budget_exhausted",
                    _ => "response_budget_closed",
                }))
            })
    }

    #[cfg(test)]
    fn with_response_budget(
        inner: reqwest::Client,
        max_bytes: usize,
        response_budget: Arc<tokio::sync::Semaphore>,
    ) -> Self {
        Self {
            inner,
            max_bytes,
            response_budget,
            response_weight: u32::try_from(max_bytes.div_ceil(RESPONSE_BUDGET_QUANTUM).max(1))
                .unwrap(),
        }
    }
}

/// Apply `custom_headers` after validating them. Mirrors the helper in
/// rmcp's reqwest impl since `validate_custom_header` is public.
fn apply_custom_headers(
    mut builder: reqwest::RequestBuilder,
    custom_headers: HashMap<HeaderName, HeaderValue>,
) -> Result<reqwest::RequestBuilder, StreamableHttpError<reqwest::Error>> {
    for (name, value) in custom_headers {
        validate_custom_header(&name).map_err(StreamableHttpError::ReservedHeaderConflict)?;
        builder = builder.header(name, value);
    }
    Ok(builder)
}

fn parse_json_rpc_error(body: &str) -> Option<ServerJsonRpcMessage> {
    match serde_json::from_str::<ServerJsonRpcMessage>(body) {
        Ok(message @ JsonRpcMessage::Error(_)) => Some(message),
        _ => None,
    }
}

/// Return the method header required by SEP-2243 from the JSON-RPC body.
///
/// The rmcp worker normally supplies this header for modern protocol
/// versions. Keep the adapter defensive because it is the final wire boundary
/// for both OAuth and non-OAuth upstream clients, and strict peers reject a
/// body/header mismatch before dispatching the request.
fn jsonrpc_method_header(message: &ClientJsonRpcMessage) -> Option<HeaderValue> {
    let value = serde_json::to_value(message).ok()?;
    let method = value.get("method").and_then(serde_json::Value::as_str)?;
    HeaderValue::from_str(method).ok()
}

/// Return the SEP-2243 name header derived from the JSON-RPC body.
///
/// `Mcp-Name` is required for named methods such as `tools/call`. Deriving it
/// here, at the final wire boundary, keeps strict upstreams working even when
/// the SDK did not populate its negotiated custom-header map. Values use the
/// SEP-2243 Base64 sentinel when they cannot be represented safely as a plain
/// HTTP field value.
fn jsonrpc_name_header(message: &ClientJsonRpcMessage) -> Option<HeaderValue> {
    let value = serde_json::to_value(message).ok()?;
    let method = value.get("method").and_then(serde_json::Value::as_str)?;
    let params = value.get("params")?;
    let key = match method {
        "tools/call" | "prompts/get" => "name",
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" => "uri",
        "tasks/get" | "tasks/update" | "tasks/cancel" => "taskId",
        _ => return None,
    };
    let raw = params.get(key).and_then(serde_json::Value::as_str)?;
    let requires_base64 = !raw.is_empty()
        && (matches!(raw.as_bytes().first(), Some(b' ' | b'\t'))
            || matches!(raw.as_bytes().last(), Some(b' ' | b'\t'))
            || raw
                .chars()
                .any(|ch| (ch as u32) < 0x20 || (ch as u32) > 0x7e)
            || (raw.starts_with(BASE64_HEADER_PREFIX) && raw.ends_with(BASE64_HEADER_SUFFIX)));
    let encoded = if requires_base64 {
        format!(
            "{BASE64_HEADER_PREFIX}{}{BASE64_HEADER_SUFFIX}",
            BASE64_STANDARD.encode(raw)
        )
    } else {
        raw.to_owned()
    };
    HeaderValue::from_str(&encoded).ok()
}

/// Read a reqwest response body fully into a `Vec<u8>` while enforcing
/// `max_bytes`. Checks `Content-Length` first for fast rejection, then
/// counts bytes as `bytes_stream()` yields chunks. Aborts the read the
/// moment the cumulative count exceeds `max_bytes`.
///
/// Returns `StreamableHttpError::UnexpectedServerResponse` with the
/// stable `response_too_large` prefix when the cap is exceeded.
async fn read_body_capped(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, StreamableHttpError<reqwest::Error>> {
    let max_u64 = max_bytes as u64;
    // Pre-check Content-Length when present (fast reject for hostile upstreams
    // that declare oversized bodies up front).
    let declared = response.content_length();
    if let Some(cl) = declared
        && cl > max_u64
    {
        return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
            format!("response_too_large: declared {cl} bytes, max {max_bytes}"),
        )));
    }
    // Preallocate when Content-Length is honest and under cap. Saves
    // ~log2(N) reallocs on the hot path for every legitimate response.
    let initial_cap = declared.map(|cl| cl.min(max_u64) as usize).unwrap_or(0);
    let mut buf: Vec<u8> = Vec::with_capacity(initial_cap);
    let mut stream = response.bytes_stream();
    let mut count: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(StreamableHttpError::Client)?;
        count = count.saturating_add(chunk.len() as u64);
        if count > max_u64 {
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("response_too_large: streamed {count} bytes, max {max_bytes}"),
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

#[derive(Debug, thiserror::Error)]
pub enum CappedResponseBodyError {
    #[error("response_too_large: streamed {observed_bytes} bytes, max {max_bytes}")]
    TooLarge {
        observed_bytes: u64,
        max_bytes: usize,
    },
    #[error("upstream response read failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("response_budget_closed")]
    BudgetClosed,
    #[error("response_budget_exhausted")]
    BudgetExhausted,
    #[error("response decode failed: {0}")]
    Decode(String),
}

async fn acquire_response_permit(
    budget: Arc<tokio::sync::Semaphore>,
    weight: u32,
) -> Result<tokio::sync::OwnedSemaphorePermit, CappedResponseBodyError> {
    // SSE streams can retain their permits indefinitely. A response waiting
    // behind those streams must terminate even when no caller deadline exists.
    tokio::time::timeout(RESPONSE_BUDGET_WAIT, budget.acquire_many_owned(weight))
        .await
        .map_err(|_| CappedResponseBodyError::BudgetExhausted)?
        .map_err(|_| CappedResponseBodyError::BudgetClosed)
}

/// Read a non-MCP HTTP response with the same pre-materialization and global
/// aggregate memory bounds used by upstream MCP transports.
pub async fn read_response_body_capped(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, CappedResponseBodyError> {
    let max_bytes = effective_response_limit(max_bytes);
    let weight = max_bytes
        .div_ceil(RESPONSE_BUDGET_QUANTUM)
        .max(1)
        .min(AGGREGATE_RESPONSE_BUDGET_PERMITS);
    let budget = Arc::clone(GLOBAL_RESPONSE_BUDGET.get_or_init(|| {
        Arc::new(tokio::sync::Semaphore::new(
            AGGREGATE_RESPONSE_BUDGET_PERMITS,
        ))
    }));
    let _permit =
        acquire_response_permit(budget, u32::try_from(weight).unwrap_or(u32::MAX)).await?;
    if let Some(declared) = response.content_length()
        && declared > max_bytes as u64
    {
        return Err(CappedResponseBodyError::TooLarge {
            observed_bytes: declared,
            max_bytes,
        });
    }
    let mut bytes =
        Vec::with_capacity(response.content_length().unwrap_or(0).min(max_bytes as u64) as usize);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(CappedResponseBodyError::TooLarge {
                observed_bytes: bytes.len().saturating_add(chunk.len()) as u64,
                max_bytes,
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Stream-error type for the per-event SSE body cap. `SseStream::from_byte_stream`
/// is generic over any `E: std::error::Error`, so we don't need to synthesize
/// a `reqwest::Error` — a dedicated enum that wraps reqwest errors AND our cap
/// breach is cleaner and surfaces the `response_too_large:` token via Display.
#[derive(Debug)]
pub enum CappedStreamError {
    Reqwest(reqwest::Error),
    TooLarge { event_bytes: u64, max_bytes: usize },
}

impl std::fmt::Display for CappedStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Keep the "upstream stream error:" prefix so log lines surface
            // that the failure came from inside the body-cap wrapper and
            // not bare reqwest. `source()` still chains to the inner error
            // for `{:#}` formatters.
            Self::Reqwest(e) => write!(f, "upstream stream error: {e}"),
            Self::TooLarge {
                event_bytes,
                max_bytes,
            } => write!(
                f,
                "response_too_large: single SSE event reached {event_bytes} bytes, max {max_bytes}"
            ),
        }
    }
}

impl std::error::Error for CappedStreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Reqwest(e) => Some(e),
            Self::TooLarge { .. } => None,
        }
    }
}

/// Wrap an SSE byte stream so any SINGLE event exceeding `max_bytes`
/// produces a stream error. Bytes are counted per-event: the counter
/// resets to 0 immediately after each `"\n\n"` delimiter, and bytes
/// after the delimiter (within the same chunk) count toward the next
/// event. Cumulative bytes across many events are unconstrained —
/// legitimate long-lived subscriptions keep working.
///
/// Cross-chunk delimiters (chunk N ends `\n`, chunk N+1 starts `\n`)
/// are detected via the `prev_ended_with_lf` state.
fn per_event_capped_byte_stream(
    inner: impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send + 'static,
    max_bytes: usize,
) -> BoxStream<'static, Result<bytes::Bytes, CappedStreamError>> {
    use bytes::Bytes;
    let max_u64 = max_bytes as u64;
    // State: (running event-byte count, line-ending state across chunks).
    let stream = inner.scan(
        (0u64, EventBoundaryState::default()),
        move |state, chunk_res| {
            let res = match chunk_res {
                Ok(chunk) => match account_event_bytes(&chunk, state.0, state.1, max_u64) {
                    Ok((new_count, new_boundary_state)) => {
                        *state = (new_count, new_boundary_state);
                        Ok::<Bytes, _>(chunk)
                    }
                    Err(event_bytes) => {
                        *state = (0, EventBoundaryState::default());
                        Err(CappedStreamError::TooLarge {
                            event_bytes,
                            max_bytes,
                        })
                    }
                },
                Err(e) => Err(CappedStreamError::Reqwest(e)),
            };
            futures::future::ready(Some(res))
        },
    );
    stream.boxed()
}

fn hold_response_budget(
    stream: BoxStream<'static, Result<bytes::Bytes, CappedStreamError>>,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> BoxStream<'static, Result<bytes::Bytes, CappedStreamError>> {
    stream
        .scan(permit, |_permit, item| futures::future::ready(Some(item)))
        .boxed()
}

/// Account the bytes of `chunk` against the per-event counter, resetting
/// the counter at each `"\n\n"` delimiter (which may span this chunk and
/// the previous one).
///
/// On success, returns `(new_count, prev_chunk_ended_with_lf)`. On cap
/// breach, returns `Err(event_byte_count_that_exceeded)` — caller maps to
/// `CappedStreamError::TooLarge`.
///
/// Counts bytes after the final `\n\n` in this chunk toward the next event
/// (rather than discarding them as the naive "add full chunk, then reset"
/// would). Detects boundaries that span chunks (prev ends '\n', this
/// starts '\n').
#[derive(Clone, Copy, Debug, Default)]
struct EventBoundaryState {
    previous_line_ended: bool,
    pending_cr: bool,
}

fn account_event_bytes(
    chunk: &[u8],
    mut count: u64,
    mut state: EventBoundaryState,
    max_bytes: u64,
) -> Result<(u64, EventBoundaryState), u64> {
    for &byte in chunk {
        count = count.saturating_add(1);
        if count > max_bytes {
            return Err(count);
        }

        if state.pending_cr {
            state.pending_cr = false;
            if byte == b'\n' {
                if state.previous_line_ended {
                    count = 0;
                    state.previous_line_ended = false;
                } else {
                    state.previous_line_ended = true;
                }
                continue;
            }
            if state.previous_line_ended {
                count = 0;
                state.previous_line_ended = false;
            } else {
                state.previous_line_ended = true;
            }
        }

        match byte {
            b'\r' => state.pending_cr = true,
            b'\n' if state.previous_line_ended => {
                count = 0;
                state.previous_line_ended = false;
            }
            b'\n' => state.previous_line_ended = true,
            _ => state.previous_line_ended = false,
        }
    }
    Ok((count, state))
}

/// Legacy helper kept for the chunk_contains_event_boundary tests in
/// docs and review evidence. The new `account_event_bytes` function
/// supersedes it for the streaming path.
#[cfg(test)]
fn chunk_contains_event_boundary(chunk: &[u8], prev_ended_with_lf: bool) -> bool {
    if prev_ended_with_lf && chunk.first() == Some(&b'\n') {
        return true;
    }
    chunk.windows(2).any(|w| w == b"\n\n")
}

impl StreamableHttpClient for BodyCappedHttpClient {
    type Error = reqwest::Error;

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let mut request_builder = self
            .inner
            .get(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "));
        if let Some(session_id) = session_id {
            request_builder = request_builder.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        if let Some(last_event_id) = last_event_id {
            request_builder = request_builder.header(HEADER_LAST_EVENT_ID, last_event_id);
        }
        if let Some(auth_header) = auth_token {
            request_builder = request_builder.bearer_auth(auth_header);
        }
        request_builder = apply_custom_headers(request_builder, custom_headers)?;
        let response = request_builder
            .send()
            .await
            .map_err(StreamableHttpError::Client)?;
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        let response = response
            .error_for_status()
            .map_err(StreamableHttpError::Client)?;
        match response.headers().get(reqwest::header::CONTENT_TYPE) {
            Some(ct) => {
                if !ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes())
                    && !ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes())
                {
                    return Err(StreamableHttpError::UnexpectedContentType(Some(
                        String::from_utf8_lossy(ct.as_bytes()).to_string(),
                    )));
                }
            }
            None => {
                return Err(StreamableHttpError::UnexpectedContentType(None));
            }
        }
        let permit = self.acquire_response_budget().await?;
        let capped = hold_response_budget(
            per_event_capped_byte_stream(response.bytes_stream(), self.max_bytes),
            permit,
        );
        Ok(SseStream::from_bytes_stream(capped).boxed())
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let mut request = self
            .inner
            .delete(uri.as_ref())
            .header(HEADER_SESSION_ID, session.as_ref());
        if let Some(auth_header) = auth_token {
            request = request.bearer_auth(auth_header);
        }
        request = apply_custom_headers(request, custom_headers)?;
        let response = request.send().await.map_err(StreamableHttpError::Client)?;
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        response
            .error_for_status()
            .map_err(StreamableHttpError::Client)?;
        Ok(())
    }

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        mut custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let mut request = self
            .inner
            .post(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "));
        if let Some(auth_header) = auth_token {
            request = request.bearer_auth(auth_header);
        }
        // rmcp 3.x already adds Mcp-Method to the per-request custom-header
        // map for negotiated modern peers. Remove that copy before applying
        // headers because reqwest's `header` appends repeated values; the
        // strict SEP-2243 contract requires exactly one body-derived value.
        custom_headers.retain(|name, _| {
            !name.as_str().eq_ignore_ascii_case(HEADER_MCP_METHOD)
                && !name.as_str().eq_ignore_ascii_case(HEADER_MCP_NAME)
        });
        request = apply_custom_headers(request, custom_headers)?;
        if let Some(method) = jsonrpc_method_header(&message) {
            request = request.header(HEADER_MCP_METHOD, method);
        }
        if let Some(name) = jsonrpc_name_header(&message) {
            request = request.header(HEADER_MCP_NAME, name);
        }
        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        let response = request
            .json(&message)
            .send()
            .await
            .map_err(StreamableHttpError::Client)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && let Some(header) = response.headers().get(WWW_AUTHENTICATE)
        {
            let header = header
                .to_str()
                .map_err(|_| {
                    StreamableHttpError::UnexpectedServerResponse(Cow::from(
                        "invalid www-authenticate header value",
                    ))
                })?
                .to_string();
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                header,
            )));
        }
        if response.status() == reqwest::StatusCode::FORBIDDEN
            && let Some(header) = response.headers().get(WWW_AUTHENTICATE)
        {
            let header_str = header.to_str().map_err(|_| {
                StreamableHttpError::UnexpectedServerResponse(Cow::from(
                    "invalid www-authenticate header value",
                ))
            })?;
            let scope = extract_scope_from_header(header_str);
            return Err(StreamableHttpError::InsufficientScope(
                InsufficientScopeError::new(header_str.to_string(), scope),
            ));
        }
        let status = response.status();
        if matches!(
            status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == reqwest::StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .map(|ct| String::from_utf8_lossy(ct.as_bytes()).to_string());
        let content_length = response.content_length();
        let response_session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        if status.is_success()
            && content_length == Some(0)
            && matches!(
                message,
                ClientJsonRpcMessage::Notification(_)
                    | ClientJsonRpcMessage::Response(_)
                    | ClientJsonRpcMessage::Error(_)
            )
        {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        // Non-success: read body with cap so a hostile error response can't OOM.
        if !status.is_success() {
            let _permit = self.acquire_response_budget().await?;
            let body_bytes = read_body_capped(response, self.max_bytes).await?;
            let body = String::from_utf8_lossy(&body_bytes).to_string();
            if content_type
                .as_deref()
                .is_some_and(|ct| ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()))
                && let Some(message) = parse_json_rpc_error(&body)
            {
                return Ok(StreamableHttpPostResponse::Json(
                    message,
                    response_session_id,
                ));
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {status}: {body}"),
            )));
        }
        match content_type.as_deref() {
            Some(ct) if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => {
                let permit = self.acquire_response_budget().await?;
                let capped = hold_response_budget(
                    per_event_capped_byte_stream(response.bytes_stream(), self.max_bytes),
                    permit,
                );
                Ok(StreamableHttpPostResponse::Sse(
                    SseStream::from_bytes_stream(capped).boxed(),
                    response_session_id,
                ))
            }
            Some(ct) if ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()) => {
                let _permit = self.acquire_response_budget().await?;
                let body_bytes = read_body_capped(response, self.max_bytes).await?;
                match serde_json::from_slice::<ServerJsonRpcMessage>(&body_bytes) {
                    Ok(message) => Ok(StreamableHttpPostResponse::Json(
                        message,
                        response_session_id,
                    )),
                    Err(e) => {
                        tracing::warn!(
                            "could not parse JSON response as ServerJsonRpcMessage, treating as accepted: {e}"
                        );
                        Ok(StreamableHttpPostResponse::Accepted)
                    }
                }
            }
            _ => {
                tracing::error!("unexpected content type: {:?}", content_type);
                Err(StreamableHttpError::UnexpectedContentType(content_type))
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use serde_json::Value;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn build(max_bytes: usize) -> BodyCappedHttpClient {
        // See upstream/pool.rs::UpstreamPool::new for why this call is
        // needed under "rustls-no-provider" -- idempotent, safe to ignore Err.
        drop(rustls::crypto::ring::default_provider().install_default());
        let inner = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("client");
        BodyCappedHttpClient::new(inner, max_bytes)
    }

    #[test]
    fn caller_limit_cannot_exceed_the_aggregate_budget() {
        let client = build(AGGREGATE_RESPONSE_BUDGET_BYTES + 1);
        assert_eq!(client.max_bytes(), AGGREGATE_RESPONSE_BUDGET_BYTES);
        assert_eq!(build(1024).max_bytes(), 1024);
    }

    #[tokio::test]
    async fn retained_stream_budget_fails_with_bounded_admission_and_recovers() {
        let budget = Arc::new(tokio::sync::Semaphore::new(1));
        let retained = Arc::clone(&budget).acquire_owned().await.unwrap();
        let result = tokio::time::timeout(
            RESPONSE_BUDGET_WAIT * 3,
            acquire_response_permit(Arc::clone(&budget), 1),
        )
        .await
        .expect("admission must not wait for a long-lived SSE stream to close");
        assert!(matches!(
            result,
            Err(CappedResponseBodyError::BudgetExhausted)
        ));
        drop(retained);
        drop(
            acquire_response_permit(Arc::clone(&budget), 1)
                .await
                .unwrap(),
        );
        budget.close();
        assert!(matches!(
            acquire_response_permit(budget, 1).await,
            Err(CappedResponseBodyError::BudgetClosed)
        ));
    }

    #[tokio::test]
    async fn aggregate_budget_bounds_near_cap_responses_at_fleet_scale() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        drop(rustls::crypto::ring::default_provider().install_default());
        for request_count in [10_usize, 100, 1000] {
            let per_response = 10 * 1024 * 1024;
            let permits_per_response = per_response / RESPONSE_BUDGET_QUANTUM;
            let budget = Arc::new(tokio::sync::Semaphore::new(permits_per_response * 8));
            let client = BodyCappedHttpClient::with_response_budget(
                reqwest::Client::new(),
                per_response,
                budget,
            );
            let active = Arc::new(AtomicUsize::new(0));
            let peak = Arc::new(AtomicUsize::new(0));
            let mut tasks = tokio::task::JoinSet::new();
            for _ in 0..request_count {
                let client = client.clone();
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                tasks.spawn(async move {
                    let _permit = client.acquire_response_budget().await.unwrap();
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
            while tasks.join_next().await.is_some() {}
            assert!(peak.load(Ordering::SeqCst) <= 8, "scale={request_count}");
            assert_eq!(active.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn shared_capped_reader_rejects_before_materializing_oversized_body() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/oversized"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 4096]))
            .mount(&server)
            .await;
        let response = reqwest::Client::new()
            .get(format!("{}/oversized", server.uri()))
            .send()
            .await
            .unwrap();
        let error = read_response_body_capped(response, 1024).await.unwrap_err();
        assert!(matches!(error, CappedResponseBodyError::TooLarge { .. }));
    }

    fn jsonrpc_request() -> ClientJsonRpcMessage {
        jsonrpc_request_with_method("tools/list")
    }

    fn jsonrpc_request_with_method(method: &str) -> ClientJsonRpcMessage {
        serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": {}
        }))
        .expect("valid jsonrpc")
    }

    #[test]
    fn bearer_scope_parser_rejects_parameter_name_substrings() {
        assert_eq!(
            extract_scope_from_header(
                r#"Bearer error="insufficient_scope", fooscope="admin", scope="mcp:write""#,
            ),
            Some("mcp:write".to_string())
        );
        assert_eq!(
            extract_scope_from_header(r#"Bearer error="insufficient_scope", fooscope="admin""#),
            None
        );
    }

    #[tokio::test]
    async fn post_message_injects_mcp_method_from_jsonrpc_body() {
        let server = MockServer::start().await;
        let strict_contract_matched = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let strict_contract_matched_for_response = Arc::clone(&strict_contract_matched);
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(move |request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).expect("valid JSON-RPC");
                let expected = body
                    .get("method")
                    .and_then(Value::as_str)
                    .expect("JSON-RPC method");
                let actual = request
                    .headers
                    .get(HEADER_MCP_METHOD)
                    .and_then(|value| value.to_str().ok());
                let value_count = request.headers.get_all(HEADER_MCP_METHOD).iter().count();

                if actual != Some(expected) || value_count != 1 {
                    return ResponseTemplate::new(400).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "error": {
                            "code": rmcp::model::ErrorCode::HEADER_MISMATCH.0,
                            "message": "the request headers and body disagree"
                        }
                    }));
                }
                strict_contract_matched_for_response
                    .store(true, std::sync::atomic::Ordering::SeqCst);

                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "error": {"code": -32601, "message": "test response"}
                }))
            })
            .mount(&server)
            .await;

        let client = build(1024 * 1024);
        let uri: Arc<str> = format!("{}/mcp", server.uri()).into();
        let mut custom_headers = HashMap::new();
        custom_headers.insert(
            HeaderName::from_static("mcp-method"),
            HeaderValue::from_static("incorrect-method"),
        );
        let result = client
            .post_message(
                uri,
                jsonrpc_request_with_method("server/discover"),
                None,
                None,
                custom_headers,
            )
            .await;

        assert!(
            result.is_ok(),
            "strict MCP endpoint should accept request: {result:?}"
        );
        assert!(
            strict_contract_matched.load(std::sync::atomic::Ordering::SeqCst),
            "request must carry exactly one body-derived Mcp-Method header"
        );
    }

    #[tokio::test]
    async fn post_message_injects_exactly_one_mcp_name_from_jsonrpc_body() {
        let server = MockServer::start().await;
        let strict_contract_matched = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let strict_contract_matched_for_response = Arc::clone(&strict_contract_matched);
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(move |request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).expect("valid JSON-RPC");
                let expected = body
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .expect("tools/call params.name");
                let actual = request
                    .headers
                    .get(HEADER_MCP_NAME)
                    .and_then(|value| value.to_str().ok());
                let value_count = request.headers.get_all(HEADER_MCP_NAME).iter().count();

                if actual != Some(expected) || value_count != 1 {
                    return ResponseTemplate::new(400).set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "error": {
                            "code": rmcp::model::ErrorCode::HEADER_MISMATCH.0,
                            "message": "the request headers and body disagree"
                        }
                    }));
                }
                strict_contract_matched_for_response
                    .store(true, std::sync::atomic::Ordering::SeqCst);

                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "error": {"code": -32601, "message": "test response"}
                }))
            })
            .mount(&server)
            .await;

        let message: ClientJsonRpcMessage = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "get_issue",
                "arguments": {"id": "U8-477"}
            }
        }))
        .expect("valid tools/call request");
        let mut custom_headers = HashMap::new();
        custom_headers.insert(
            HeaderName::from_static("mcp-name"),
            HeaderValue::from_static("incorrect-name"),
        );
        let result = build(1024 * 1024)
            .post_message(
                format!("{}/mcp", server.uri()).into(),
                message,
                None,
                None,
                custom_headers,
            )
            .await;

        assert!(
            result.is_ok(),
            "strict MCP endpoint should accept request: {result:?}"
        );
        assert!(
            strict_contract_matched.load(std::sync::atomic::Ordering::SeqCst),
            "request must carry exactly one body-derived Mcp-Name header"
        );
    }

    #[tokio::test]
    async fn post_message_preserves_mcp_param_headers() {
        let server = MockServer::start().await;
        let param_header_matched = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let param_header_matched_for_response = Arc::clone(&param_header_matched);
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(move |request: &wiremock::Request| {
                let actual = request
                    .headers
                    .get("mcp-param-owner")
                    .and_then(|value| value.to_str().ok());
                if actual == Some("dinglebear-ai") {
                    param_header_matched_for_response
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "error": {"code": -32601, "message": "test response"}
                }))
            })
            .mount(&server)
            .await;

        let message: ClientJsonRpcMessage = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "pull_request_read",
                "arguments": {"owner": "dinglebear-ai", "repo": "labby"}
            }
        }))
        .expect("valid tools/call request");
        let mut custom_headers = HashMap::new();
        custom_headers.insert(
            HeaderName::from_static("mcp-param-owner"),
            HeaderValue::from_static("dinglebear-ai"),
        );

        build(1024 * 1024)
            .post_message(
                format!("{}/mcp", server.uri()).into(),
                message,
                None,
                None,
                custom_headers,
            )
            .await
            .expect("request with rmcp parameter mirror succeeds");

        assert!(
            param_header_matched.load(std::sync::atomic::Ordering::SeqCst),
            "BodyCappedHttpClient must not strip rmcp's Mcp-Param-* headers"
        );
    }

    #[tokio::test]
    async fn allows_response_under_cap() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#.as_bytes().to_vec(),
                "application/json",
            ))
            .mount(&server)
            .await;

        let client = build(10 * 1024 * 1024);
        let uri: Arc<str> = format!("{}/mcp", server.uri()).into();
        let result = client
            .post_message(uri, jsonrpc_request(), None, None, HashMap::new())
            .await;
        assert!(result.is_ok(), "small response should succeed: {result:?}");
    }

    #[tokio::test]
    async fn rejects_oversized_response_body() {
        let server = MockServer::start().await;
        let big = "x".repeat(5 * 1024 * 1024);
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(r#"{{"jsonrpc":"2.0","id":1,"result":"{big}"}}"#).into_bytes(),
                "application/json",
            ))
            .mount(&server)
            .await;

        let client = build(1024 * 1024); // 1 MB cap
        let uri: Arc<str> = format!("{}/mcp", server.uri()).into();
        let result = client
            .post_message(uri, jsonrpc_request(), None, None, HashMap::new())
            .await;
        let err = result.expect_err("must reject oversized body");
        let s = format!("{err:?}");
        assert!(
            s.contains("response_too_large"),
            "expected response_too_large, got: {s}"
        );
    }

    #[test]
    fn capped_stream_error_display_contains_token() {
        let e = CappedStreamError::TooLarge {
            event_bytes: 12345,
            max_bytes: 1024,
        };
        let msg = format!("{e}");
        assert!(msg.contains("response_too_large"), "got: {msg}");
        assert!(msg.contains("12345"));
        assert!(msg.contains("1024"));
    }

    #[test]
    fn chunk_contains_event_boundary_intra_chunk() {
        // "\n\n" entirely within one chunk
        assert!(chunk_contains_event_boundary(b"abc\n\ndef", false));
        assert!(!chunk_contains_event_boundary(b"abc\ndef", false));
        assert!(!chunk_contains_event_boundary(b"", false));
    }

    #[test]
    fn account_event_bytes_single_event_under_cap() {
        // 6-byte event in one chunk, no delimiter inside.
        let (c, state) =
            account_event_bytes(b"abcdef", 0, EventBoundaryState::default(), 100).unwrap();
        assert_eq!(c, 6);
        assert!(!state.previous_line_ended);
    }

    #[test]
    fn account_event_bytes_intra_chunk_boundary_resets() {
        // First event "abc\n\n" (5 bytes accounted), then "def" starts next event.
        let (c, state) =
            account_event_bytes(b"abc\n\ndef", 0, EventBoundaryState::default(), 100).unwrap();
        // After the "\n\n" the counter resets, then 3 bytes of next event.
        assert_eq!(c, 3, "counter must track bytes AFTER the \\n\\n");
        assert!(!state.previous_line_ended);
    }

    #[test]
    fn account_event_bytes_cross_chunk_boundary_resets() {
        // Previous chunk ended with '\n' and we already saw 4 bytes of an
        // event; this chunk starts with '\n', closing the event. Then
        // "next_event" accumulates from scratch.
        let (c, state) = account_event_bytes(
            b"\nnext",
            4,
            EventBoundaryState {
                previous_line_ended: true,
                pending_cr: false,
            },
            100,
        )
        .unwrap();
        // After the cross-chunk "\n\n" the counter resets, then 4 bytes
        // of "next" accumulate.
        assert_eq!(c, 4);
        assert!(!state.previous_line_ended);
    }

    #[test]
    fn account_event_bytes_caps_oversized_event() {
        // Cap = 5 bytes. Chunk = "abcdefg" with no delimiter — should error.
        let err = account_event_bytes(b"abcdefg", 0, EventBoundaryState::default(), 5).unwrap_err();
        assert!(err > 5, "error must include exceeded byte count: got {err}");
    }

    #[test]
    fn account_event_bytes_no_false_positive_on_multi_event_chunk() {
        // Three small events in one chunk; cap larger than any single
        // event but smaller than total. Naive "add chunk.len() then reset"
        // would falsely flag. account_event_bytes resets per-event so the
        // chunk passes cleanly.
        let chunk = b"event1\n\nevent2\n\nevent3";
        // Cap = 10 bytes — each event is 6, total chunk is 22.
        let (c, state) = account_event_bytes(chunk, 0, EventBoundaryState::default(), 10).unwrap();
        // After the trailing "event3" (no closing "\n\n"), counter = 6.
        assert_eq!(c, 6);
        assert!(!state.previous_line_ended);
    }

    #[test]
    fn account_event_bytes_tracks_trailing_lf() {
        // Chunk ends with '\n' — next chunk must be told to look for cross
        // boundary.
        let (_c, state) =
            account_event_bytes(b"abc\n", 0, EventBoundaryState::default(), 100).unwrap();
        assert!(
            state.previous_line_ended,
            "must track a trailing line ending across chunks"
        );
    }

    #[test]
    fn account_event_bytes_resets_at_crlf_boundaries_across_chunks() {
        let (count, boundary_state) =
            account_event_bytes(b"data: 1234\r\n\r", 0, EventBoundaryState::default(), 20).unwrap();
        let (count, _) =
            account_event_bytes(b"\ndata: 5678\r\n\r\n", count, boundary_state, 20).unwrap();

        assert_eq!(count, 0);
    }

    #[test]
    fn account_event_bytes_counts_multiline_crlf_event_across_chunks() {
        let (count, boundary_state) =
            account_event_bytes(b"data: 1234\r\n", 0, EventBoundaryState::default(), 20).unwrap();
        let error = account_event_bytes(b"data: 5678\r\n\r\n", count, boundary_state, 20)
            .expect_err("one multiline SSE event must use one cumulative byte budget");

        assert!(error > 20);
    }

    /// SSE happy path through the full pipeline: server returns an
    /// `text/event-stream` response with multiple small events under the
    /// per-event cap. `post_message` must return `Sse(stream, _)` and the
    /// stream must yield at least one event without erroring.
    ///
    /// This guards against regressions in the per_event_capped_byte_stream
    /// state machine (scan + chunk_contains_event_boundary) when refactored.
    #[tokio::test]
    async fn sse_happy_path_yields_events_under_cap() {
        use futures::StreamExt;
        use rmcp::transport::streamable_http_client::StreamableHttpPostResponse as Resp;

        let server = MockServer::start().await;
        // 3 small SSE events well under the cap.
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":1}\n\n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":2}\n\n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":3}\n\n";
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(body.as_bytes().to_vec(), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let client = build(10 * 1024 * 1024);
        let uri: Arc<str> = format!("{}/mcp", server.uri()).into();
        let result = client
            .post_message(uri, jsonrpc_request(), None, None, HashMap::new())
            .await
            .expect("sse post_message must succeed");

        let mut stream = match result {
            Resp::Sse(s, _) => s,
            other => panic!("expected Sse variant, got: {other:?}"),
        };
        let mut event_count = 0usize;
        while let Some(item) = stream.next().await {
            let _sse = item.expect("each SSE event must parse cleanly under cap");
            event_count += 1;
            if event_count >= 3 {
                break;
            }
        }
        assert!(event_count >= 1, "must yield at least one SSE event");
    }

    #[tokio::test]
    async fn adapter_forwards_legacy_session_ids_in_both_directions() {
        use rmcp::transport::streamable_http_client::StreamableHttpPostResponse as Resp;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(|request: &wiremock::Request| {
                assert_eq!(
                    request
                        .headers
                        .get("mcp-session-id")
                        .and_then(|value| value.to_str().ok()),
                    Some("legacy-session"),
                    "legacy upstream requests must forward Mcp-Session-Id"
                );
                ResponseTemplate::new(200)
                    .insert_header("Mcp-Session-Id", "legacy-session")
                    .set_body_raw(
                        br#"{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","tools":[],"ttlMs":0,"cacheScope":"private"}}"#
                            .to_vec(),
                        "application/json",
                    )
            })
            .mount(&server)
            .await;

        let client = build(1024 * 1024);
        let uri: Arc<str> = format!("{}/mcp", server.uri()).into();
        let response = client
            .post_message(
                uri,
                jsonrpc_request(),
                Some(Arc::from("legacy-session")),
                None,
                HashMap::new(),
            )
            .await
            .expect("session-aware post succeeds");

        let Resp::Json(_, session_id) = response else {
            panic!("expected JSON response");
        };
        assert_eq!(session_id.as_deref(), Some("legacy-session"));
    }

    #[test]
    fn chunk_contains_event_boundary_cross_chunk() {
        // Previous chunk ended with '\n' and this chunk starts with '\n'.
        // Without the prev-state flag the windowed scan would miss this.
        assert!(chunk_contains_event_boundary(b"\nrest", true));
        // Prev '\n' but next chunk doesn't start with '\n': no boundary.
        assert!(!chunk_contains_event_boundary(b"rest", true));
        // No prev '\n', chunk starts with '\n' but no in-chunk "\n\n": OK.
        assert!(!chunk_contains_event_boundary(b"\nrest", false));
    }
}
