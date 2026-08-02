//! Connection establishment for HTTP and WebSocket upstreams.
//!
//! `connect_upstream` is the transport-dispatching entry point; it delegates to
//! `connect_http_upstream`, `connect_websocket_upstream`, or (in
//! `connect_stdio.rs`) the stdio/in-process connectors. These free functions are
//! `pub(super)` so the pool module and the sibling `connect_stdio` module can
//! call them across the module boundary.

use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStringExt as _;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use reqwest::header::{AUTHORIZATION, HeaderName, HeaderValue};

use rmcp::ClientHandler;
use rmcp::model::{
    CancelledNotification, CancelledNotificationParam, ClientJsonRpcMessage, ClientNotification,
    NotificationMetaObject, ProtocolVersion, RequestId, RequestMetaObject,
};
use rmcp::service::ClientServiceExt;
use rmcp::transport::AuthClient;
use rmcp::transport::common::http_header::HEADER_MCP_PROTOCOL_VERSION;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};

use labby_auth::upstream::cache::OauthClientCache;
use labby_runtime::gateway_config::{UpstreamConfig, UpstreamTransport};

use super::super::auth::{configured_bearer_token, websocket_authorization_header};
use super::super::http_client;
use super::super::transport::websocket::{
    WebSocketTransportConfig, connect as connect_websocket_transport, parse_ws_url,
};
use super::super::types::{UpstreamRuntimeMetadata, UpstreamRuntimeOwner};
use super::connect_stdio::connect_stdio_upstream;
use super::helpers::{
    DEFAULT_REQUEST_TIMEOUT, max_response_bytes, upstream_target_redacted, upstream_transport,
};
use super::legacy_client::VersionedClientHandler;
use super::lifecycle_compat::{
    LifecycleAttempt, compatibility_retry, legacy_protocol_version, log_fallback,
};
use super::{UpstreamClientService, UpstreamConnection};
use crate::MCP_RELAY_CANCELLATION_TOKEN_META_KEY;

/// Connect to an upstream MCP server, optionally reusing a caller-supplied
/// `reqwest::Client` for HTTP connections (P-M10).
///
/// When `shared_client` is `Some`, that client is used as the base HTTP
/// transport (non-OAuth) or as the inner client for the OAuth path.  When
/// `None` the function falls back to building a fresh client, preserving the
/// pre-P-M10 behaviour.
pub(super) async fn connect_upstream_with_client(
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
    shared_client: Option<&reqwest::Client>,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    connect_upstream_with_handler(
        config,
        subject,
        oauth_client_cache,
        runtime_origin,
        runtime_owner,
        shared_client,
        (),
    )
    .await
}

/// Connect to an upstream MCP server, serving the client side with `handler`.
///
/// This is the generic seam behind `connect_upstream_with_client` (which passes
/// the unit handler `()`). The relay path passes a `RelayClientHandler` so the
/// dedicated connection forwards server→client requests to the downstream agent.
/// `handler` is moved into whichever transport branch matches the config.
pub(super) async fn connect_upstream_with_handler<H: ClientHandler + Clone>(
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
    shared_client: Option<&reqwest::Client>,
    handler: H,
) -> anyhow::Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>)> {
    let started = Instant::now();
    tracing::debug!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.connect",
        event = "attempt",
        operation = "connection.acquire",
        upstream = %config.name,
        transport = upstream_transport(config),
        target = %upstream_target_redacted(config),
        subject_scoped = subject.is_some(),
        "upstream connection acquire attempt"
    );
    let result = match config.effective_transport() {
        Some(UpstreamTransport::Http) => {
            let url = config.url.as_deref().ok_or_else(|| {
                anyhow::anyhow!("upstream {} HTTP transport has no url", config.name)
            })?;
            connect_http_upstream(
                url,
                config,
                subject,
                oauth_client_cache,
                shared_client,
                handler,
            )
            .await
        }
        Some(UpstreamTransport::Websocket) => {
            let url = config.url.as_deref().ok_or_else(|| {
                anyhow::anyhow!("upstream {} WebSocket transport has no url", config.name)
            })?;
            connect_websocket_upstream(url, config, handler).await
        }
        Some(UpstreamTransport::Stdio) => {
            let command = config.command.as_deref().ok_or_else(|| {
                anyhow::anyhow!("upstream {} stdio transport has no command", config.name)
            })?;
            connect_stdio_upstream(
                command,
                &config.args,
                config,
                runtime_origin,
                runtime_owner,
                handler,
            )
            .await
        }
        Some(UpstreamTransport::UnixSocket) => {
            let url = config.url.as_deref().ok_or_else(|| {
                anyhow::anyhow!("upstream {} Unix socket transport has no url", config.name)
            })?;
            connect_unix_socket_upstream(url, config, subject, oauth_client_cache, handler).await
        }
        None => Err(anyhow::anyhow!(
            "upstream {} has neither url nor command",
            config.name
        )),
    };
    match &result {
        Ok((_, tools)) => tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.connect",
            event = "finish",
            operation = "connection.acquire",
            upstream = %config.name,
            transport = upstream_transport(config),
            target = %upstream_target_redacted(config),
            subject_scoped = subject.is_some(),
            tool_count = tools.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "upstream connection acquire finish"
        ),
        Err(error) => tracing::warn!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.connect",
            event = "error",
            operation = "connection.acquire",
            upstream = %config.name,
            transport = upstream_transport(config),
            target = %upstream_target_redacted(config),
            subject_scoped = subject.is_some(),
            kind = "upstream_connect_error",
            error = %error,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream connection acquire error"
        ),
    }
    result
}

pub(super) async fn connect_upstream(
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    connect_upstream_with_client(
        config,
        subject,
        oauth_client_cache,
        runtime_origin,
        runtime_owner,
        None,
    )
    .await
}

#[cfg(unix)]
fn unix_socket_connect_path(path: &str) -> PathBuf {
    #[cfg(target_os = "linux")]
    if let Some(name) = path
        .as_bytes()
        .strip_prefix(b"@")
        .filter(|name| !name.is_empty())
    {
        let mut address = Vec::with_capacity(name.len() + 1);
        address.push(0);
        address.extend_from_slice(name);
        return PathBuf::from(OsString::from_vec(address));
    }
    PathBuf::from(path)
}

#[cfg(unix)]
async fn connect_unix_socket_upstream<H: ClientHandler + Clone>(
    url: &str,
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    handler: H,
) -> anyhow::Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>)> {
    let socket_path = config.socket_path.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "upstream {} Unix socket transport has no socket_path",
            config.name
        )
    })?;

    // Keep the existing BodyCappedHttpClient and rmcp worker path intact. The
    // only change is reqwest's connector, so OAuth, bearer headers, lifecycle
    // fallback, JSON body limits, and per-event SSE limits stay identical to
    // HTTP/TCP upstreams.
    drop(rustls::crypto::ring::default_provider().install_default());
    let client = reqwest::Client::builder()
        .timeout(DEFAULT_REQUEST_TIMEOUT)
        .http1_only()
        .unix_socket(unix_socket_connect_path(socket_path))
        .build()
        .map_err(|error| {
            anyhow::anyhow!(
                "failed to build Unix socket client for upstream {}: {error}",
                config.name
            )
        })?;

    connect_http_upstream(
        url,
        config,
        subject,
        oauth_client_cache,
        Some(&client),
        handler,
    )
    .await
}

#[cfg(not(unix))]
async fn connect_unix_socket_upstream<H: ClientHandler + Clone>(
    _url: &str,
    config: &UpstreamConfig,
    _subject: Option<&str>,
    _oauth_client_cache: Option<&OauthClientCache>,
    _handler: H,
) -> anyhow::Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>)> {
    anyhow::bail!(
        "upstream {} uses unix_socket, which is unsupported on this platform",
        config.name
    )
}

pub(super) async fn connect_websocket_upstream<H: ClientHandler + Clone>(
    url: &str,
    config: &UpstreamConfig,
    handler: H,
) -> anyhow::Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>)> {
    match connect_websocket_upstream_once(url, config, handler.clone(), LifecycleAttempt::Modern)
        .await
    {
        Ok(connection) => Ok(connection),
        Err(error) => {
            let Some(attempt) = compatibility_retry(&error) else {
                return Err(error);
            };
            log_fallback(&config.name, "websocket", attempt, &error);
            connect_websocket_upstream_once(url, config, handler, attempt).await
        }
    }
}

async fn connect_websocket_upstream_once<H: ClientHandler>(
    url: &str,
    config: &UpstreamConfig,
    handler: H,
    lifecycle: LifecycleAttempt,
) -> anyhow::Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>)> {
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "websocket",
        action = "upstream.connect.start", target = %upstream_target_redacted(config),
        "upstream connect start",
    );
    if config.oauth.is_some() {
        anyhow::bail!(
            "upstream {} declares oauth, but websocket upstream oauth is not yet supported",
            config.name
        );
    }

    let parsed = parse_ws_url(url).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let authorization = websocket_authorization_header(config);
    let transport = connect_websocket_transport(
        WebSocketTransportConfig::new(parsed.to_string()).with_authorization(authorization),
    );
    let service = match lifecycle {
        LifecycleAttempt::Modern => UpstreamClientService::Direct(
            handler
                .serve_with_lifecycle(transport, lifecycle.mode())
                .await?,
        ),
        LifecycleAttempt::LegacyInitialize => UpstreamClientService::Versioned(
            VersionedClientHandler::new(handler, legacy_protocol_version())
                .serve_with_lifecycle(transport, lifecycle.mode())
                .await?,
        ),
    };
    let peer = service.peer().clone();
    let tools = peer.list_all_tools().await?;
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "websocket",
        action = "upstream.connect.finish", tool_count = tools.len(),
        "upstream connect finish",
    );
    Ok((
        UpstreamConnection {
            _client_service: service,
            _server_task: None,
            peer,
            runtime: UpstreamRuntimeMetadata::default(),
        },
        tools,
    ))
}

pub(super) fn stable_jitter_seed(name: &str, attempt: u32) -> u64 {
    let mut hash = 1_469_598_103_934_665_603_u64;
    for byte in name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash ^ u64::from(attempt)
}

/// Connect to an HTTP upstream MCP server.
///
/// `shared_client` is an optional caller-supplied `reqwest::Client` to reuse
/// for connection-pooling and TLS session reuse (P-M10).  When `None` a fresh
/// client is built.  Both the OAuth and non-OAuth paths wrap the base client in
/// `BodyCappedHttpClient` so the response-size cap (P-H4) is always applied.
pub(super) async fn connect_http_upstream<H: ClientHandler + Clone>(
    url: &str,
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    shared_client: Option<&reqwest::Client>,
    handler: H,
) -> anyhow::Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>)> {
    match connect_http_upstream_once(
        url,
        config,
        subject,
        oauth_client_cache,
        shared_client,
        handler.clone(),
        LifecycleAttempt::Modern,
    )
    .await
    {
        Ok(connection) => Ok(connection),
        Err(error) => {
            let Some(attempt) = compatibility_retry(&error) else {
                return Err(error);
            };
            log_fallback(&config.name, upstream_transport(config), attempt, &error);
            connect_http_upstream_once(
                url,
                config,
                subject,
                oauth_client_cache,
                shared_client,
                handler,
                attempt,
            )
            .await
        }
    }
}

fn configured_custom_headers(
    config: &UpstreamConfig,
) -> anyhow::Result<HashMap<HeaderName, HeaderValue>> {
    let mut headers = HashMap::with_capacity(config.headers.len());
    for (raw_name, raw_value) in &config.headers {
        let name = HeaderName::from_bytes(raw_name.trim().as_bytes()).map_err(|error| {
            anyhow::anyhow!(
                "upstream {} has invalid custom header name {:?}: {error}",
                config.name,
                raw_name
            )
        })?;
        if name == AUTHORIZATION {
            anyhow::bail!(
                "upstream {} must use bearer_token_env or OAuth for Authorization",
                config.name
            );
        }
        let value = HeaderValue::from_str(raw_value).map_err(|error| {
            anyhow::anyhow!(
                "upstream {} has invalid value for custom header {}: {error}",
                config.name,
                name
            )
        })?;
        headers.insert(name, value);
    }
    Ok(headers)
}

#[derive(Clone)]
enum HttpCancellationClient {
    Plain(http_client::BodyCappedHttpClient),
    Oauth(AuthClient<http_client::BodyCappedHttpClient>),
}

/// Sends an explicit notifications/cancelled POST for HTTP transports.
///
/// rmcp 3.1 closes a modern HTTP request's local response stream when a
/// RequestHandle is cancelled, but does not transmit the cancellation
/// notification to the server. Relay calls retain this sender so Labby can
/// deliver the wire notification before asking rmcp to close its local stream.
#[derive(Clone)]
pub(super) struct HttpCancellationSender {
    uri: Arc<str>,
    client: HttpCancellationClient,
    auth_token: Option<String>,
    custom_headers: HashMap<HeaderName, HeaderValue>,
}

fn cancellation_message(
    request_id: RequestId,
    reason: Option<String>,
    token: &str,
) -> ClientJsonRpcMessage {
    let mut params = CancelledNotificationParam::new(Some(request_id), reason);
    let mut meta = NotificationMetaObject::new();
    meta.0.0.insert(
        MCP_RELAY_CANCELLATION_TOKEN_META_KEY.to_string(),
        serde_json::Value::String(token.to_string()),
    );
    params.meta = Some(meta.clone());
    let mut cancelled = CancelledNotification::new(params);
    cancelled.extensions.insert(meta);
    ClientJsonRpcMessage::notification(ClientNotification::CancelledNotification(cancelled))
}

impl HttpCancellationSender {
    pub(super) fn new_request_token(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }

    pub(super) fn attach_request_token(&self, meta: &mut RequestMetaObject, token: &str) {
        meta.0.0.insert(
            MCP_RELAY_CANCELLATION_TOKEN_META_KEY.to_string(),
            serde_json::Value::String(token.to_string()),
        );
    }

    async fn post_message(&self, message: ClientJsonRpcMessage) -> anyhow::Result<()> {
        let result = match &self.client {
            HttpCancellationClient::Plain(client) => {
                client
                    .post_message(
                        Arc::clone(&self.uri),
                        message,
                        None,
                        self.auth_token.clone(),
                        self.custom_headers.clone(),
                    )
                    .await
            }
            HttpCancellationClient::Oauth(client) => {
                client
                    .post_message(
                        Arc::clone(&self.uri),
                        message,
                        None,
                        None,
                        self.custom_headers.clone(),
                    )
                    .await
            }
        };
        result
            .map(|_| ())
            .map_err(|error| anyhow::anyhow!("explicit HTTP cancellation failed: {error}"))
    }

    pub(super) async fn send(
        &self,
        request_id: RequestId,
        reason: Option<String>,
        token: &str,
    ) -> anyhow::Result<()> {
        self.post_message(cancellation_message(request_id, reason, token))
            .await
    }
}

/// Build the side-channel used to deliver cancellation notifications for HTTP
/// relay connections. Non-HTTP transports return None because their rmcp
/// worker already forwards notifications/cancelled on the underlying stream.
pub(super) async fn build_http_cancellation_sender(
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    shared_client: Option<&reqwest::Client>,
) -> anyhow::Result<Option<HttpCancellationSender>> {
    let transport = config.effective_transport();
    if !matches!(
        transport,
        Some(UpstreamTransport::Http | UpstreamTransport::UnixSocket)
    ) {
        return Ok(None);
    }

    let url = config.url.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "upstream {} HTTP cancellation sender has no url",
            config.name
        )
    })?;
    let mut custom_headers = configured_custom_headers(config)?;
    custom_headers.insert(
        HeaderName::from_bytes(HEADER_MCP_PROTOCOL_VERSION.as_bytes())?,
        HeaderValue::from_str(&ProtocolVersion::V_2026_07_28.to_string())?,
    );

    let base_client = match transport {
        Some(UpstreamTransport::Http) => {
            if let Some(client) = shared_client {
                client.clone()
            } else {
                drop(rustls::crypto::ring::default_provider().install_default());
                reqwest::Client::builder()
                    .timeout(DEFAULT_REQUEST_TIMEOUT)
                    .build()?
            }
        }
        Some(UpstreamTransport::UnixSocket) => {
            #[cfg(unix)]
            {
                let socket_path = config.socket_path.as_deref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "upstream {} Unix socket cancellation sender has no socket_path",
                        config.name
                    )
                })?;
                drop(rustls::crypto::ring::default_provider().install_default());
                reqwest::Client::builder()
                    .timeout(DEFAULT_REQUEST_TIMEOUT)
                    .http1_only()
                    .unix_socket(unix_socket_connect_path(socket_path))
                    .build()?
            }
            #[cfg(not(unix))]
            {
                return Ok(None);
            }
        }
        _ => return Ok(None),
    };
    let capped = http_client::BodyCappedHttpClient::new(base_client, max_response_bytes());

    let (client, auth_token) = if config.oauth.is_some() {
        let subject = subject.ok_or_else(|| {
            anyhow::anyhow!(
                "upstream {} requires an authenticated subject for cancellation",
                config.name
            )
        })?;
        let cache = oauth_client_cache.ok_or_else(|| {
            anyhow::anyhow!(
                "upstream {} requires OAuth but no auth client cache is registered",
                config.name
            )
        })?;
        let auth_client = cache
            .get_or_build_capped(config, subject, capped)
            .await
            .map_err(|error| anyhow::anyhow!("oauth_required: {error}"))?;
        (HttpCancellationClient::Oauth(auth_client), None)
    } else {
        let auth_token = config
            .bearer_token_env
            .as_deref()
            .and_then(configured_bearer_token);
        (HttpCancellationClient::Plain(capped), auth_token)
    };

    Ok(Some(HttpCancellationSender {
        uri: Arc::from(url),
        client,
        auth_token,
        custom_headers,
    }))
}

async fn connect_http_upstream_once<H: ClientHandler>(
    url: &str,
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    shared_client: Option<&reqwest::Client>,
    handler: H,
    lifecycle: LifecycleAttempt,
) -> anyhow::Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>)> {
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = upstream_transport(config),
        action = "upstream.connect.start", target = %upstream_target_redacted(config),
        "upstream connect start",
    );
    let mut transport_config = StreamableHttpClientTransportConfig::with_uri(url);
    transport_config.custom_headers = configured_custom_headers(config)?;

    // Resolve base HTTP client: reuse the pool-level shared client when
    // available, otherwise build a fresh one (backward-compatible fallback).
    let base_client = if let Some(c) = shared_client {
        c.clone()
    } else {
        // See upstream/pool.rs::UpstreamPool::new for why this call is
        // needed under "rustls-no-provider" -- idempotent, safe to ignore Err.
        drop(rustls::crypto::ring::default_provider().install_default());
        reqwest::Client::builder()
            .timeout(DEFAULT_REQUEST_TIMEOUT)
            .build()?
    };

    // Wrap in BodyCappedHttpClient so both the OAuth and non-OAuth paths
    // enforce the streaming response-size cap (P-H4).
    let capped = http_client::BodyCappedHttpClient::new(base_client, max_response_bytes());

    // OAuth path: when the upstream declares oauth config, build an AuthClient.
    if config.oauth.is_some() {
        let subject = subject.ok_or_else(|| {
            anyhow::anyhow!(
                "upstream {} requires an authenticated subject; discovery must be request-scoped",
                config.name
            )
        })?;
        let cache = oauth_client_cache.ok_or_else(|| {
            anyhow::anyhow!(
                "upstream {} requires OAuth but no auth client cache is registered",
                config.name
            )
        })?;

        let auth_client = cache
            .get_or_build_capped(config, subject, capped)
            .await
            .map_err(|e| anyhow::anyhow!("oauth_required: {e}"))?;

        let worker = StreamableHttpClientWorker::new(auth_client, transport_config);
        let service = match lifecycle {
            LifecycleAttempt::Modern => UpstreamClientService::Direct(
                handler
                    .serve_with_lifecycle(worker, lifecycle.mode())
                    .await?,
            ),
            LifecycleAttempt::LegacyInitialize => UpstreamClientService::Versioned(
                VersionedClientHandler::new(handler, legacy_protocol_version())
                    .serve_with_lifecycle(worker, lifecycle.mode())
                    .await?,
            ),
        };
        let peer = service.peer().clone();
        let tools = peer.list_all_tools().await?;
        return Ok((
            UpstreamConnection {
                _client_service: service,
                _server_task: None,
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
            },
            tools,
        ));
    }

    // Non-OAuth path: optionally inject a static bearer token from env.
    if let Some(ref env_name) = config.bearer_token_env {
        if let Some(token) = configured_bearer_token(env_name) {
            transport_config.auth_header = Some(token);
        } else {
            tracing::warn!(
                upstream = %config.name,
                env_var = %env_name,
                "bearer_token_env configured but env var not set"
            );
        }
    }

    // `capped` is already built above with the shared/fresh base client.
    let worker = StreamableHttpClientWorker::new(capped, transport_config);
    let service = match lifecycle {
        LifecycleAttempt::Modern => UpstreamClientService::Direct(
            handler
                .serve_with_lifecycle(worker, lifecycle.mode())
                .await?,
        ),
        LifecycleAttempt::LegacyInitialize => UpstreamClientService::Versioned(
            VersionedClientHandler::new(handler, legacy_protocol_version())
                .serve_with_lifecycle(worker, lifecycle.mode())
                .await?,
        ),
    };
    let peer = service.peer().clone();
    let tools = peer.list_all_tools().await?;
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = upstream_transport(config),
        action = "upstream.connect.finish", tool_count = tools.len(),
        "upstream connect finish",
    );

    Ok((
        UpstreamConnection {
            _client_service: service,
            _server_task: None,
            peer,
            runtime: UpstreamRuntimeMetadata::default(),
        },
        tools,
    ))
}

pub(super) fn runtime_origin_label(
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
) -> Option<String> {
    if let Some(raw) = runtime_owner
        .and_then(|owner| owner.raw.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(raw.to_string());
    }

    if let Some(origin) = runtime_origin
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(origin.to_string());
    }

    for (prefix, session_key) in [
        ("claude-code", "CLAUDE_SESSION_ID"),
        ("codex", "CODEX_SESSION_ID"),
    ] {
        if let Ok(session) = std::env::var(session_key) {
            let trimmed = session.trim();
            if !trimmed.is_empty() {
                return Some(format!("{prefix}:{trimmed}"));
            }
        }
    }

    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        let trimmed = term_program.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    Some("gateway-managed".to_string())
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;

    #[test]
    fn cancellation_token_survives_json_round_trip() {
        let token = "test-cancellation-token";
        let message = cancellation_message(
            RequestId::Number(13),
            Some("downstream request cancelled".to_string()),
            token,
        );
        let wire = serde_json::to_value(&message).expect("serialize cancellation notification");
        assert_eq!(
            wire.pointer("/params/_meta/ai.dinglebear.labby~1relayCancellationToken")
                .and_then(serde_json::Value::as_str),
            Some(token)
        );

        let decoded: ClientJsonRpcMessage =
            serde_json::from_value(wire).expect("deserialize cancellation notification");
        assert!(
            matches!(&decoded, ClientJsonRpcMessage::Notification(_)),
            "expected notification"
        );
        let ClientJsonRpcMessage::Notification(notification) = decoded else {
            return;
        };
        assert!(
            matches!(
                &notification.notification,
                ClientNotification::CancelledNotification(_)
            ),
            "expected cancelled notification"
        );
        let ClientNotification::CancelledNotification(cancelled) = notification.notification else {
            return;
        };
        let typed_token = cancelled.params.meta.as_ref().and_then(|meta| {
            meta.0
                .0
                .get(MCP_RELAY_CANCELLATION_TOKEN_META_KEY)
                .and_then(serde_json::Value::as_str)
        });
        let extension_token = cancelled
            .extensions
            .get::<NotificationMetaObject>()
            .and_then(|meta| {
                meta.0
                    .0
                    .get(MCP_RELAY_CANCELLATION_TOKEN_META_KEY)
                    .and_then(serde_json::Value::as_str)
            });
        assert_eq!(typed_token.or(extension_token), Some(token));
    }
}
