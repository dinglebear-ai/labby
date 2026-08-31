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
use std::future::Future;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStringExt as _;
#[cfg(unix)]
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

use reqwest::header::{AUTHORIZATION, HeaderName, HeaderValue};

use rmcp::model::{
    JsonRpcMessage, ProgressNotificationParam, ServerNotification, ServerRequest,
    TaskStatusNotificationParams,
};
use rmcp::service::{ClientServiceExt, RawRxJsonRpcMessage, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};
use rmcp::transport::{Transport, TransportAdapterIdentity, WorkerTransport};
use rmcp::{ClientHandler, RoleClient};

use labby_auth::upstream::cache::OauthClientCache;
use labby_runtime::gateway_config::{UpstreamConfig, UpstreamTransport};

use super::super::auth::{configured_bearer_token, websocket_authorization_header};
use super::super::http_client;
use super::super::transport::websocket::{
    WebSocketTransportConfig, connect as connect_websocket_transport, parse_ws_url,
};
use super::super::types::{UpstreamRuntimeMetadata, UpstreamRuntimeOwner};
use super::catalog_pagination;
use super::connect_stdio::connect_stdio_upstream;
use super::helpers::{
    DEFAULT_REQUEST_TIMEOUT, DISCOVERY_TIMEOUT, max_response_bytes, upstream_target_redacted,
    upstream_transport,
};
use super::legacy_client::VersionedClientHandler;
use super::lifecycle_compat::{
    LifecycleAttempt, compatibility_retry, legacy_protocol_version, log_fallback,
};
use super::tools::MAX_UPSTREAM_TOOLS;
use super::{UpstreamClientService, UpstreamConnection};

#[derive(Clone)]
pub(super) enum OrderedRelayNotification {
    Progress(ProgressNotificationParam),
    TaskStatus(TaskStatusNotificationParams),
}

pub(super) type RelayNotificationInterceptor =
    Arc<dyn Fn(OrderedRelayNotification) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

const ORDERED_RELAY_NOTIFICATION_BUFFER: usize = 64;

type RelayNotificationWork = (u64, OrderedRelayNotification);

#[derive(Default)]
struct RelayNotificationDeliveryState {
    completed: AtomicU64,
    notify: tokio::sync::Notify,
}

impl RelayNotificationDeliveryState {
    fn mark_completed(&self, sequence: u64) {
        self.completed.store(sequence, Ordering::Release);
        self.notify.notify_waiters();
    }

    fn completed(&self) -> u64 {
        self.completed.load(Ordering::Acquire)
    }

    async fn wait_through(&self, sequence: u64) {
        while self.completed() < sequence {
            let notified = self.notify.notified();
            if self.completed() >= sequence {
                return;
            }
            notified.await;
        }
    }
}

pub(super) struct OrderedRelayNotificationTransport<T> {
    inner: T,
    notification_tx: Option<tokio::sync::mpsc::Sender<RelayNotificationWork>>,
    notification_delivery: Option<Arc<RelayNotificationDeliveryState>>,
    next_notification_sequence: u64,
    pending_message: Option<RxJsonRpcMessage<RoleClient>>,
    pending_raw_message: Option<RawRxJsonRpcMessage<RoleClient>>,
    pending_notification_sequence: u64,
}

impl<T> OrderedRelayNotificationTransport<T> {
    pub(super) fn new(
        inner: T,
        notification_interceptor: Option<RelayNotificationInterceptor>,
    ) -> Self {
        let (notification_tx, notification_delivery) =
            if let Some(interceptor) = notification_interceptor {
                let (tx, mut rx) = tokio::sync::mpsc::channel::<RelayNotificationWork>(
                    ORDERED_RELAY_NOTIFICATION_BUFFER,
                );
                let delivery = Arc::new(RelayNotificationDeliveryState::default());
                let worker_delivery = Arc::clone(&delivery);
                tokio::spawn(async move {
                    while let Some((sequence, notification)) = rx.recv().await {
                        interceptor(notification).await;
                        worker_delivery.mark_completed(sequence);
                    }
                });
                (Some(tx), Some(delivery))
            } else {
                (None, None)
            };
        Self {
            inner,
            notification_tx,
            notification_delivery,
            next_notification_sequence: 0,
            pending_message: None,
            pending_raw_message: None,
            pending_notification_sequence: 0,
        }
    }

    fn intercepted_notification<Resp>(
        message: &JsonRpcMessage<ServerRequest, Resp, ServerNotification>,
    ) -> Option<OrderedRelayNotification> {
        let JsonRpcMessage::Notification(notification) = message else {
            return None;
        };
        match &notification.notification {
            ServerNotification::ProgressNotification(progress) => {
                Some(OrderedRelayNotification::Progress(progress.params.clone()))
            }
            ServerNotification::TaskStatusNotification(task_status) => Some(
                OrderedRelayNotification::TaskStatus(task_status.params.clone()),
            ),
            _ => None,
        }
    }

    async fn wait_for_notifications_through(
        delivery: Option<Arc<RelayNotificationDeliveryState>>,
        sequence: u64,
    ) {
        if sequence == 0 {
            return;
        }
        if let Some(delivery) = delivery {
            delivery.wait_through(sequence).await;
        }
    }

    fn notifications_completed(&self) -> u64 {
        self.notification_delivery
            .as_ref()
            .map_or(self.next_notification_sequence, |delivery| {
                delivery.completed()
            })
    }
}

impl<T> Transport<RoleClient> for OrderedRelayNotificationTransport<T>
where
    T: Transport<RoleClient>,
{
    type Error = T::Error;

    fn preserves_raw_responses() -> bool {
        T::preserves_raw_responses()
    }

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        loop {
            if self.pending_message.is_some() {
                Self::wait_for_notifications_through(
                    self.notification_delivery.clone(),
                    self.pending_notification_sequence,
                )
                .await;
                self.pending_notification_sequence = 0;
                return self.pending_message.take();
            }

            let mut permit = match self.notification_tx.clone() {
                Some(sender) => match sender.reserve_owned().await {
                    Ok(permit) => Some(permit),
                    Err(_) => {
                        self.notification_tx = None;
                        self.notification_delivery = None;
                        None
                    }
                },
                None => None,
            };

            let Some(message) = self.inner.receive().await else {
                drop(permit);
                Self::wait_for_notifications_through(
                    self.notification_delivery.clone(),
                    self.next_notification_sequence,
                )
                .await;
                return None;
            };

            if let Some(notification) = Self::intercepted_notification(&message)
                && let Some(permit) = permit.take()
            {
                self.next_notification_sequence = self.next_notification_sequence.saturating_add(1);
                permit.send((self.next_notification_sequence, notification));
                continue;
            }

            drop(permit);
            if self.notifications_completed() < self.next_notification_sequence {
                self.pending_notification_sequence = self.next_notification_sequence;
                self.pending_message = Some(message);
                continue;
            }
            return Some(message);
        }
    }

    async fn receive_raw(&mut self) -> Option<RawRxJsonRpcMessage<RoleClient>> {
        loop {
            if self.pending_raw_message.is_some() {
                Self::wait_for_notifications_through(
                    self.notification_delivery.clone(),
                    self.pending_notification_sequence,
                )
                .await;
                self.pending_notification_sequence = 0;
                return self.pending_raw_message.take();
            }

            let mut permit = match self.notification_tx.clone() {
                Some(sender) => match sender.reserve_owned().await {
                    Ok(permit) => Some(permit),
                    Err(_) => {
                        self.notification_tx = None;
                        self.notification_delivery = None;
                        None
                    }
                },
                None => None,
            };
            let Some(message) = self.inner.receive_raw().await else {
                drop(permit);
                Self::wait_for_notifications_through(
                    self.notification_delivery.clone(),
                    self.next_notification_sequence,
                )
                .await;
                return None;
            };
            if let Some(notification) = Self::intercepted_notification(&message)
                && let Some(permit) = permit.take()
            {
                self.next_notification_sequence = self.next_notification_sequence.saturating_add(1);
                permit.send((self.next_notification_sequence, notification));
                continue;
            }
            drop(permit);
            if self.notifications_completed() < self.next_notification_sequence {
                self.pending_notification_sequence = self.next_notification_sequence;
                self.pending_raw_message = Some(message);
                continue;
            }
            return Some(message);
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

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
    connect_upstream_with_handler_and_notifications(
        config,
        subject,
        oauth_client_cache,
        runtime_origin,
        runtime_owner,
        shared_client,
        handler,
        None,
    )
    .await
}

pub(super) async fn connect_upstream_with_handler_and_notifications<H: ClientHandler + Clone>(
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
    shared_client: Option<&reqwest::Client>,
    handler: H,
    notification_interceptor: Option<RelayNotificationInterceptor>,
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
            connect_http_upstream_with_notifications(
                url,
                config,
                subject,
                oauth_client_cache,
                shared_client,
                handler,
                notification_interceptor.clone(),
            )
            .await
        }
        Some(UpstreamTransport::Websocket) => {
            let url = config.url.as_deref().ok_or_else(|| {
                anyhow::anyhow!("upstream {} WebSocket transport has no url", config.name)
            })?;
            connect_websocket_upstream(url, config, handler, notification_interceptor.clone()).await
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
                notification_interceptor.clone(),
            )
            .await
        }
        Some(UpstreamTransport::UnixSocket) => {
            let url = config.url.as_deref().ok_or_else(|| {
                anyhow::anyhow!("upstream {} Unix socket transport has no url", config.name)
            })?;
            connect_unix_socket_upstream(
                url,
                config,
                subject,
                oauth_client_cache,
                handler,
                notification_interceptor.clone(),
            )
            .await
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
pub(super) fn unix_socket_connect_path(path: &str) -> PathBuf {
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
    notification_interceptor: Option<RelayNotificationInterceptor>,
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

    connect_http_upstream_with_notifications(
        url,
        config,
        subject,
        oauth_client_cache,
        Some(&client),
        handler,
        notification_interceptor,
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
    _notification_interceptor: Option<RelayNotificationInterceptor>,
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
    notification_interceptor: Option<RelayNotificationInterceptor>,
) -> anyhow::Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>)> {
    match connect_websocket_upstream_once(
        url,
        config,
        handler.clone(),
        LifecycleAttempt::Modern,
        notification_interceptor.clone(),
    )
    .await
    {
        Ok(connection) => Ok(connection),
        Err(error) => {
            let Some(attempt) = compatibility_retry(&error) else {
                return Err(error);
            };
            log_fallback(&config.name, "websocket", attempt, &error);
            connect_websocket_upstream_once(url, config, handler, attempt, notification_interceptor)
                .await
        }
    }
}

async fn connect_websocket_upstream_once<H: ClientHandler>(
    url: &str,
    config: &UpstreamConfig,
    handler: H,
    lifecycle: LifecycleAttempt,
    notification_interceptor: Option<RelayNotificationInterceptor>,
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
    let transport = OrderedRelayNotificationTransport::new(transport, notification_interceptor);
    let service = match lifecycle {
        LifecycleAttempt::Modern => UpstreamClientService::Direct(
            handler
                .serve_with_lifecycle::<_, _, TransportAdapterIdentity>(transport, lifecycle.mode())
                .await?,
        ),
        LifecycleAttempt::LegacyInitialize => UpstreamClientService::Versioned(
            VersionedClientHandler::new(handler, legacy_protocol_version())
                .serve_with_lifecycle::<_, _, TransportAdapterIdentity>(transport, lifecycle.mode())
                .await?,
        ),
    };
    let peer = service.peer().clone();
    let tools = catalog_pagination::list_tools(&peer, DISCOVERY_TIMEOUT, MAX_UPSTREAM_TOOLS)
        .await
        .map_err(|error| anyhow::anyhow!(error.bounded_text()))?;
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
            incarnation: None,
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
    connect_http_upstream_with_notifications(
        url,
        config,
        subject,
        oauth_client_cache,
        shared_client,
        handler,
        None,
    )
    .await
}

async fn connect_http_upstream_with_notifications<H: ClientHandler + Clone>(
    url: &str,
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    shared_client: Option<&reqwest::Client>,
    handler: H,
    notification_interceptor: Option<RelayNotificationInterceptor>,
) -> anyhow::Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>)> {
    match connect_http_upstream_once(
        url,
        config,
        subject,
        oauth_client_cache,
        shared_client,
        handler.clone(),
        LifecycleAttempt::Modern,
        notification_interceptor.clone(),
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
                notification_interceptor,
            )
            .await
        }
    }
}

pub(super) fn configured_custom_headers(
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

async fn connect_http_upstream_once<H: ClientHandler>(
    url: &str,
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    shared_client: Option<&reqwest::Client>,
    handler: H,
    lifecycle: LifecycleAttempt,
    notification_interceptor: Option<RelayNotificationInterceptor>,
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
        let worker = WorkerTransport::spawn(worker);
        let worker =
            OrderedRelayNotificationTransport::new(worker, notification_interceptor.clone());
        let service = match lifecycle {
            LifecycleAttempt::Modern => UpstreamClientService::Direct(
                handler
                    .serve_with_lifecycle::<_, _, TransportAdapterIdentity>(
                        worker,
                        lifecycle.mode(),
                    )
                    .await?,
            ),
            LifecycleAttempt::LegacyInitialize => UpstreamClientService::Versioned(
                VersionedClientHandler::new(handler, legacy_protocol_version())
                    .serve_with_lifecycle::<_, _, TransportAdapterIdentity>(
                        worker,
                        lifecycle.mode(),
                    )
                    .await?,
            ),
        };
        let peer = service.peer().clone();
        let tools = catalog_pagination::list_tools(&peer, DISCOVERY_TIMEOUT, MAX_UPSTREAM_TOOLS)
            .await
            .map_err(|error| anyhow::anyhow!(error.bounded_text()))?;
        return Ok((
            UpstreamConnection {
                _client_service: service,
                _server_task: None,
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
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
    let worker = WorkerTransport::spawn(worker);
    let worker = OrderedRelayNotificationTransport::new(worker, notification_interceptor);
    let service = match lifecycle {
        LifecycleAttempt::Modern => UpstreamClientService::Direct(
            handler
                .serve_with_lifecycle::<_, _, TransportAdapterIdentity>(worker, lifecycle.mode())
                .await?,
        ),
        LifecycleAttempt::LegacyInitialize => UpstreamClientService::Versioned(
            VersionedClientHandler::new(handler, legacy_protocol_version())
                .serve_with_lifecycle::<_, _, TransportAdapterIdentity>(worker, lifecycle.mode())
                .await?,
        ),
    };
    let peer = service.peer().clone();
    let tools = catalog_pagination::list_tools(&peer, DISCOVERY_TIMEOUT, MAX_UPSTREAM_TOOLS)
        .await
        .map_err(|error| anyhow::anyhow!(error.bounded_text()))?;
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
            incarnation: None,
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
