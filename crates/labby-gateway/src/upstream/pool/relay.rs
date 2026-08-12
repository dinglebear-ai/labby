//! MRTR-capable client handler for proxied upstream tool calls.
//!
//! The pool's normal upstream connections are served with the unit handler,
//! which advertises no client input capabilities. [`RelayClientHandler`]
//! mirrors the downstream client's input capabilities to the upstream. Tool
//! calls use rmcp's `call_tool_once`, so an upstream `input_required` result is
//! preserved for the downstream client instead of being fulfilled inside the
//! gateway.
//!
//! The handler deliberately does not implement the removed server-initiated
//! elicitation, sampling, or roots callbacks. Its dedicated connection is
//! cached per `(upstream, session_id, subject)` so capabilities and OAuth
//! identity cannot cross downstream sessions.
//!
//! ## Cache key — `(upstream, session_id, subject)`
//!
//! The cache key has three parts, each closing a distinct reuse hazard:
//! - **`upstream`** — different servers, different connections (obvious).
//! - **`session_id`** — minted once per `LabMcpServer` session, so a cached
//!   relay connection is bound to exactly one downstream agent peer and an
//!   upstream elicitation can never be misrouted to a different agent.
//! - **`subject`** (`Option<String>`) — the OAuth subject the dedicated
//!   connection authenticated as (`None` for the non-OAuth/raw proxy path).
//!   Without it, two OAuth identities sharing one session could reuse a
//!   connection authenticated as the wrong subject. The pooled subject path
//!   keys `subject_connections` by `(upstream, subject)` for the same reason;
//!   the relay adds `session_id` because it must also bind to the agent peer.
//!
//! ## Deadlines
//!
//! [`UpstreamPool::call_tool_relayed`] uses the pool's `relay_timeout`
//! (default 5 minutes, `upstream_relay_timeout_ms`) instead of the normal
//! upstream request timeout.
//!
//! ## Scope — `call_tool` only
//!
//! Only the proxied `call_tool` path is relay-handled. Resource reads
//! (`read_resource`) and prompt fetches (`get_prompt`) still go through the
//! pooled `()` connection, so an upstream that raises elicitation/sampling/roots
//! *during* one of those will have it declined by the unit handler. This is a
//! deliberate scope boundary: tool calls are where interactive upstreams elicit
//! in practice. Widening it means routing those paths through a relay handler
//! too.

use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Instant;

#[allow(deprecated)]
use rmcp::model::LoggingMessageNotificationParam;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResponse, CancelledNotificationParam,
    ClientCapabilities, ClientInfo, ClientRequest, CustomNotification, GetPromptRequest,
    GetPromptRequestParams, GetPromptResponse, ProgressNotificationParam, ProgressToken,
    ReadResourceRequest, ReadResourceRequestParams, ReadResourceResponse, RequestId,
    RequestMetaObject, ResourceUpdatedNotificationParam, ServerNotification, ServerResult,
    TaskStatusNotification, TaskStatusNotificationParams,
};
use rmcp::service::{NotificationContext, Peer, PeerRequestOptions, ServiceError};
use rmcp::{ClientHandler, RoleClient, RoleServer};
use tokio::sync::{Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;

use labby_runtime::gateway_config::UpstreamConfig;

use crate::MCP_RELAY_CANCELLATION_TOKEN_META_KEY;

use super::super::types::UpstreamCapability;
use super::capability_call::{bounded_service_error_text, service_error_affects_connection_health};
use super::connect::connect_upstream_with_handler;
use super::entries::{
    prompt_exposed, resolve_request_prompt_exposure_policy,
    resolve_request_resource_exposure_policy, resource_exposed,
};
use super::helpers::{
    SUBJECT_CONN_IDLE_TTL, SUBJECT_CONN_MAX_ENTRIES, bare_upstream_prompt_name,
    estimate_call_tool_response_size, estimate_resource_response_size, max_response_bytes,
    normalize_resource_result_uri, redact_resource_uri_for_logging, upstream_transport,
};
use super::http_cancellation::{HttpCancellationSender, build_http_cancellation_sender};
use super::logging::{
    UpstreamRequestLog, log_upstream_request_error, log_upstream_request_finish,
    log_upstream_request_start,
};
use super::notifications::UpstreamNotificationEvent;
use super::relay_cancellation::{
    PendingRelayRequestId, RelaySendOutcome, await_relay_send, dispatch_relay_cancellation,
    spawn_bounded_handle_cancellation,
};
use super::{UpstreamConnection, UpstreamPool};

const RELAY_ROUTE_CLEANUP_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
const PROGRESS_NOTIFICATION_DELIVERY_GRACE: std::time::Duration =
    std::time::Duration::from_millis(500);

#[derive(Clone)]
struct RelayRequestRoute {
    downstream_request_id: RequestId,
    upstream_progress_token: ProgressToken,
    downstream_progress_token: Option<ProgressToken>,
}

#[derive(Default)]
struct RelayTaskRouteState {
    mappings: HashMap<String, String>,
    pending: HashMap<String, Vec<TaskStatusNotificationParams>>,
}

#[derive(Default)]
pub(super) struct RelayRouteState {
    requests: RwLock<HashMap<RequestId, RelayRequestRoute>>,
    progress: RwLock<HashMap<ProgressToken, ProgressToken>>,
    tasks: Mutex<RelayTaskRouteState>,
    progress_notification_sequence: AtomicU64,
    progress_notification_notify: Notify,
    task_notification_sequence: AtomicU64,
    task_notification_notify: Notify,
}

impl RelayRouteState {
    async fn register_request(
        &self,
        upstream_request_id: RequestId,
        downstream_request_id: RequestId,
        upstream_progress_token: ProgressToken,
        downstream_progress_token: Option<ProgressToken>,
    ) {
        if let Some(downstream_progress_token) = downstream_progress_token.clone() {
            self.progress
                .write()
                .await
                .insert(upstream_progress_token.clone(), downstream_progress_token);
        }
        self.requests.write().await.insert(
            upstream_request_id,
            RelayRequestRoute {
                downstream_request_id,
                upstream_progress_token,
                downstream_progress_token,
            },
        );
    }

    async fn unregister_request(&self, upstream_request_id: &RequestId) {
        if let Some(route) = self.requests.write().await.remove(upstream_request_id)
            && route.downstream_progress_token.is_some()
        {
            self.progress
                .write()
                .await
                .remove(&route.upstream_progress_token);
        }
    }

    fn schedule_unregister_request(self: &Arc<Self>, upstream_request_id: RequestId) {
        let routes = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(RELAY_ROUTE_CLEANUP_GRACE).await;
            routes.unregister_request(&upstream_request_id).await;
        });
    }

    async fn downstream_request_id(&self, upstream_request_id: &RequestId) -> Option<RequestId> {
        self.requests
            .read()
            .await
            .get(upstream_request_id)
            .map(|route| route.downstream_request_id.clone())
    }

    async fn downstream_progress_token(
        &self,
        upstream_progress_token: &ProgressToken,
    ) -> Option<ProgressToken> {
        self.progress
            .read()
            .await
            .get(upstream_progress_token)
            .cloned()
    }

    fn progress_notification_sequence(&self) -> u64 {
        self.progress_notification_sequence.load(Ordering::Acquire)
    }

    fn record_progress_notification_delivery(&self) {
        self.progress_notification_sequence
            .fetch_add(1, Ordering::AcqRel);
        self.progress_notification_notify.notify_waiters();
    }

    async fn wait_for_progress_notification_after(
        &self,
        previous: u64,
        timeout: std::time::Duration,
    ) -> bool {
        if self.progress_notification_sequence() > previous {
            return true;
        }
        let notified = self.progress_notification_notify.notified();
        if self.progress_notification_sequence() > previous {
            return true;
        }
        tokio::time::timeout(timeout, notified).await.is_ok()
            && self.progress_notification_sequence() > previous
    }

    pub(super) async fn register_task_id(
        &self,
        native_task_id: &str,
        gateway_task_id: &str,
    ) -> Vec<TaskStatusNotificationParams> {
        let mut tasks = self.tasks.lock().await;
        tasks
            .mappings
            .insert(native_task_id.to_string(), gateway_task_id.to_string());
        let mut pending = tasks.pending.remove(native_task_id).unwrap_or_default();
        for params in &mut pending {
            params.task.task.task_id = gateway_task_id.to_string();
        }
        pending
    }

    async fn translate_or_queue_task_status(
        &self,
        mut params: TaskStatusNotificationParams,
    ) -> Option<TaskStatusNotificationParams> {
        let native_task_id = params.task.task.task_id.clone();
        let mut tasks = self.tasks.lock().await;
        if let Some(gateway_task_id) = tasks.mappings.get(&native_task_id) {
            params.task.task.task_id = gateway_task_id.clone();
            Some(params)
        } else {
            tasks
                .pending
                .entry(native_task_id)
                .or_default()
                .push(params);
            None
        }
    }

    async fn gateway_task_id(&self, native_task_id: &str) -> Option<String> {
        self.tasks
            .lock()
            .await
            .mappings
            .get(native_task_id)
            .cloned()
    }

    pub(super) fn task_notification_sequence(&self) -> u64 {
        self.task_notification_sequence.load(Ordering::Acquire)
    }

    fn record_task_notification_delivery(&self) {
        self.task_notification_sequence
            .fetch_add(1, Ordering::AcqRel);
        self.task_notification_notify.notify_waiters();
    }

    pub(super) async fn wait_for_task_notification_after(
        &self,
        previous: u64,
        timeout: std::time::Duration,
    ) -> bool {
        if self.task_notification_sequence() > previous {
            return true;
        }
        let notified = self.task_notification_notify.notified();
        if self.task_notification_sequence() > previous {
            return true;
        }
        tokio::time::timeout(timeout, notified).await.is_ok()
            && self.task_notification_sequence() > previous
    }
}

/// A client handler that mirrors the downstream agent's MRTR input
/// capabilities to one upstream connection.
///
/// Construct one per dedicated upstream connection with [`RelayClientHandler::new`].
#[derive(Clone)]
pub(crate) struct RelayClientHandler {
    /// The downstream agent connection to forward request-scoped notifications to.
    /// Task-owned relay connections rebind this peer for each stateless HTTP request.
    downstream: Arc<RwLock<Peer<RoleServer>>>,
    /// Name of the upstream this handler is attached to.
    upstream_name: Arc<str>,
    /// Exact client capabilities declared on the downstream request that
    /// opened this relay connection. The 2026 protocol forbids inferring these
    /// from a prior request or discovery exchange.
    capabilities: ClientCapabilities,
    /// Per-connection translation table for request, progress, and task IDs.
    routes: Arc<RelayRouteState>,
    /// Shared pool event bus for subscription-routed catalog/resource changes.
    notification_tx: tokio::sync::broadcast::Sender<UpstreamNotificationEvent>,
    /// Subject-scoped OAuth relays must not publish user-specific events globally.
    publish_catalog_events: bool,
}

impl RelayClientHandler {
    pub(crate) fn new(
        downstream: Peer<RoleServer>,
        upstream_name: Arc<str>,
        capabilities: ClientCapabilities,
    ) -> Self {
        let (notification_tx, _receiver) = tokio::sync::broadcast::channel(1);
        Self::new_with_routes(
            downstream,
            upstream_name,
            capabilities,
            Arc::new(RelayRouteState::default()),
            notification_tx,
            false,
        )
    }

    pub(super) fn new_with_routes(
        downstream: Peer<RoleServer>,
        upstream_name: Arc<str>,
        capabilities: ClientCapabilities,
        routes: Arc<RelayRouteState>,
        notification_tx: tokio::sync::broadcast::Sender<UpstreamNotificationEvent>,
        publish_catalog_events: bool,
    ) -> Self {
        Self {
            downstream: Arc::new(RwLock::new(downstream)),
            upstream_name,
            capabilities,
            routes,
            notification_tx,
            publish_catalog_events,
        }
    }

    async fn downstream(&self) -> Peer<RoleServer> {
        self.downstream.read().await.clone()
    }

    pub(super) async fn rebind_downstream(&self, downstream: Peer<RoleServer>) {
        *self.downstream.write().await = downstream;
    }

    pub(super) async fn forward_task_status(&self, params: TaskStatusNotificationParams) {
        let task_id = params.task.task.task_id.clone();
        let status = params.task.status();
        let downstream = self.downstream().await;
        match downstream
            .send_notification(ServerNotification::TaskStatusNotification(
                TaskStatusNotification::new(params),
            ))
            .await
        {
            Ok(()) => {
                self.routes.record_task_notification_delivery();
                tracing::debug!(
                    upstream = %self.upstream_name,
                    task_id,
                    ?status,
                    "forwarded translated task notification downstream"
                );
            }
            Err(error) => {
                tracing::warn!(
                    upstream = %self.upstream_name,
                    task_id,
                    ?status,
                    error = %error,
                    "failed to forward translated task notification downstream"
                );
            }
        }
    }
}

impl ClientHandler for RelayClientHandler {
    /// Advertise the exact capability snapshot from the current downstream
    /// request. Anything the caller did not declare for this request is not
    /// claimed on its behalf.
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        info.capabilities = self.capabilities.clone();
        info
    }

    async fn on_cancelled(
        &self,
        mut params: CancelledNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let Some(upstream_request_id) = params.request_id.clone() else {
            return;
        };
        let Some(downstream_request_id) = self
            .routes
            .downstream_request_id(&upstream_request_id)
            .await
        else {
            return;
        };
        params.request_id = Some(downstream_request_id);
        let downstream = self.downstream().await;
        drop(downstream.notify_cancelled(params).await);
        self.routes.unregister_request(&upstream_request_id).await;
    }

    async fn on_progress(
        &self,
        mut params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let Some(downstream_token) = self
            .routes
            .downstream_progress_token(&params.progress_token)
            .await
        else {
            return;
        };
        params.progress_token = downstream_token;
        let downstream = self.downstream().await;
        match downstream.notify_progress(params).await {
            Ok(()) => self.routes.record_progress_notification_delivery(),
            Err(error) => tracing::warn!(
                upstream = %self.upstream_name,
                error = %error,
                "failed to forward progress notification downstream"
            ),
        }
    }

    #[allow(deprecated)]
    async fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let downstream = self.downstream().await;
        drop(downstream.notify_logging_message(params).await);
    }

    async fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        if self.publish_catalog_events {
            let uri = UpstreamPool::gateway_resource_uri(&self.upstream_name, &params.uri);
            drop(
                self.notification_tx
                    .send(UpstreamNotificationEvent::ResourceUpdated {
                        upstream: self.upstream_name.to_string(),
                        uri,
                    }),
            );
        }
    }

    async fn on_resource_list_changed(&self, _context: NotificationContext<RoleClient>) {
        if self.publish_catalog_events {
            drop(
                self.notification_tx
                    .send(UpstreamNotificationEvent::ResourceListChanged {
                        upstream: self.upstream_name.to_string(),
                    }),
            );
        }
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        if self.publish_catalog_events {
            drop(
                self.notification_tx
                    .send(UpstreamNotificationEvent::ToolListChanged {
                        upstream: self.upstream_name.to_string(),
                    }),
            );
        }
    }

    async fn on_prompt_list_changed(&self, _context: NotificationContext<RoleClient>) {
        if self.publish_catalog_events {
            drop(
                self.notification_tx
                    .send(UpstreamNotificationEvent::PromptListChanged {
                        upstream: self.upstream_name.to_string(),
                    }),
            );
        }
    }

    async fn on_task_status(
        &self,
        params: TaskStatusNotificationParams,
        _context: NotificationContext<RoleClient>,
    ) {
        let native_task_id = params.task.task.task_id.clone();
        let status = params.task.status();
        tracing::debug!(
            upstream = %self.upstream_name,
            native_task_id,
            ?status,
            "received upstream task notification"
        );
        let Some(params) = self.routes.translate_or_queue_task_status(params).await else {
            tracing::debug!(
                upstream = %self.upstream_name,
                native_task_id,
                "queued task notification until gateway task registration"
            );
            return;
        };
        self.forward_task_status(params).await;
    }

    async fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _context: NotificationContext<RoleClient>,
    ) {
        let downstream = self.downstream().await;
        drop(
            downstream
                .send_notification(ServerNotification::CustomNotification(notification))
                .await,
        );
    }
}

async fn send_relay_request(
    peer: &Peer<RoleClient>,
    routes: &Arc<RelayRouteState>,
    cancellation_sender: Option<&HttpCancellationSender>,
    request: ClientRequest,
    mut request_meta: Option<RequestMetaObject>,
    downstream_request_id: RequestId,
    downstream_cancel: CancellationToken,
    timeout: std::time::Duration,
) -> Result<ServerResult, ServiceError> {
    let cancellation_token = uuid::Uuid::new_v4().to_string();
    request_meta
        .get_or_insert_with(RequestMetaObject::new)
        .0
        .0
        .insert(
            MCP_RELAY_CANCELLATION_TOKEN_META_KEY.to_string(),
            serde_json::Value::String(cancellation_token.clone()),
        );
    if downstream_cancel.is_cancelled() {
        return Err(ServiceError::Cancelled {
            reason: Some("downstream request was already cancelled".to_string()),
        });
    }
    let downstream_progress_token = request_meta
        .as_ref()
        .and_then(RequestMetaObject::get_progress_token);
    let expects_progress = downstream_progress_token.is_some();
    let progress_sequence = routes.progress_notification_sequence();
    let options = request_meta
        .map(|meta| PeerRequestOptions::no_options().with_meta(meta))
        .unwrap_or_else(PeerRequestOptions::no_options);
    let deadline = tokio::time::Instant::now() + timeout;
    let mut handle = match await_relay_send(
        peer.send_cancellable_request(request, options),
        &downstream_cancel,
        deadline,
    )
    .await
    {
        RelaySendOutcome::Sent(Ok(handle)) => handle,
        RelaySendOutcome::Sent(Err(error)) => return Err(error),
        RelaySendOutcome::Cancelled => {
            return Err(ServiceError::Cancelled {
                reason: Some("downstream request cancelled before relay send".to_string()),
            });
        }
        RelaySendOutcome::TimedOut => return Err(ServiceError::Timeout { timeout }),
    };
    let pending_request_id = Arc::new(PendingRelayRequestId::default());
    let cancellation_dispatched = Arc::new(AtomicBool::new(false));
    let relay_finished = CancellationToken::new();
    let upstream_request_id = handle.id.clone();
    pending_request_id.set(upstream_request_id.clone());
    {
        let peer = peer.clone();
        let cancellation_sender = cancellation_sender.cloned();
        let pending_request_id = Arc::clone(&pending_request_id);
        let cancellation_token = cancellation_token.clone();
        let downstream_cancel = downstream_cancel.clone();
        let relay_finished = relay_finished.clone();
        let cancellation_dispatched = Arc::clone(&cancellation_dispatched);
        tokio::spawn(async move {
            tokio::select! {
                () = downstream_cancel.cancelled() => {
                    dispatch_relay_cancellation(
                        &peer,
                        cancellation_sender.as_ref(),
                        &pending_request_id,
                        "downstream request cancelled",
                        &cancellation_token,
                        &cancellation_dispatched,
                    );
                }
                () = relay_finished.cancelled() => {
                    tokio::select! {
                        () = downstream_cancel.cancelled() => {
                            dispatch_relay_cancellation(
                                &peer,
                                cancellation_sender.as_ref(),
                                &pending_request_id,
                                "downstream request cancelled during relay teardown",
                                &cancellation_token,
                                &cancellation_dispatched,
                            );
                        }
                        () = tokio::time::sleep(RELAY_ROUTE_CLEANUP_GRACE) => {}
                    }
                }
                () = tokio::time::sleep_until(deadline) => {
                    dispatch_relay_cancellation(
                        &peer,
                        cancellation_sender.as_ref(),
                        &pending_request_id,
                        "relayed request timeout",
                        &cancellation_token,
                        &cancellation_dispatched,
                    );
                }
            }
        });
    }

    routes
        .register_request(
            upstream_request_id.clone(),
            downstream_request_id,
            handle.progress_token.clone(),
            downstream_progress_token,
        )
        .await;

    let result = tokio::select! {
        response = &mut handle.rx => {
            let response = response.map_err(|_| ServiceError::TransportClosed)?;
            if !matches!(
                &response,
                Err(ServiceError::Cancelled { .. } | ServiceError::TransportClosed)
            ) {
                relay_finished.cancel();
            }
            if expects_progress && response.is_ok() {
                routes
                    .wait_for_progress_notification_after(
                        progress_sequence,
                        PROGRESS_NOTIFICATION_DELIVERY_GRACE,
                    )
                    .await;
            }
            response
        }
        () = downstream_cancel.cancelled() => {
            routes.unregister_request(&upstream_request_id).await;
            let reason = "downstream request cancelled".to_string();
            dispatch_relay_cancellation(
                peer,
                cancellation_sender,
                &pending_request_id,
                &reason,
                &cancellation_token,
                &cancellation_dispatched,
            );
            relay_finished.cancel();
            drop(spawn_bounded_handle_cancellation(
                handle.cancel(Some(reason.clone())),
            ));
            return Err(ServiceError::Cancelled {
                reason: Some(reason),
            });
        }
        () = tokio::time::sleep_until(deadline) => {
            routes.unregister_request(&upstream_request_id).await;
            let reason = "relayed request timeout".to_string();
            dispatch_relay_cancellation(
                peer,
                cancellation_sender,
                &pending_request_id,
                &reason,
                &cancellation_token,
                &cancellation_dispatched,
            );
            relay_finished.cancel();
            drop(spawn_bounded_handle_cancellation(
                handle.cancel(Some(reason)),
            ));
            return Err(ServiceError::Timeout { timeout });
        }
    };

    routes.schedule_unregister_request(upstream_request_id);
    result
}

pub(super) fn capability_fingerprint(capabilities: &ClientCapabilities) -> String {
    serde_json::to_string(capabilities)
        .expect("MCP client capabilities must serialize to a JSON object")
}

/// A cached relay connection, keyed in the pool by
/// `(upstream, session_id, subject)`.
///
/// The `RelayClientHandler` inside `_connection` is bound to **one** downstream
/// agent peer (the session identified by the key) and authenticated as **one**
/// OAuth subject. Because the key includes the downstream session id, a cached
/// entry is only ever reused by the same agent — never shared across sessions.
/// Because it also includes the subject, a connection authenticated as one
/// identity is never reused for a call made as another.
pub(super) struct RelayCachedConnection {
    /// Keeps the relay-served running service (and any stdio child) alive.
    pub(super) _connection: UpstreamConnection<RelayClientHandler>,
    /// Pre-cloned upstream peer for the cache-hit fast path.
    pub(super) peer: Peer<RoleClient>,
    /// Capability snapshot used to initialize this connection.
    pub(super) capability_fingerprint: String,
    /// Request/progress/task identifier translations owned by this connection.
    pub(super) routes: Arc<RelayRouteState>,
    /// Explicit cancellation POST sender for HTTP and Unix-socket transports.
    pub(super) cancellation_sender: Option<HttpCancellationSender>,
    /// Wall-clock instant when this entry was last used.
    pub(super) last_used: Instant,
}
impl RelayCachedConnection {
    pub(super) async fn rebind_downstream(&self, downstream: Peer<RoleServer>) {
        self._connection
            ._client_service
            .service()
            .rebind_downstream(downstream)
            .await;
    }

    pub(super) async fn flush_task_status_notifications(
        &self,
        notifications: Vec<TaskStatusNotificationParams>,
    ) {
        let handler = self._connection._client_service.service();
        for params in notifications {
            handler.forward_task_status(params).await;
        }
    }
}

/// Evict least-recently-used relay connections until the map holds at most
/// `max_entries`, sparing the about-to-be-inserted `protect` key. Mirrors
/// `connection::evict_lru_over_cap` for the relay-typed cache.
fn evict_relay_lru_over_cap(
    cache: &mut HashMap<(String, u64, Option<String>), RelayCachedConnection>,
    max_entries: usize,
    protect: &(String, u64, Option<String>),
) -> Vec<(String, UpstreamConnection<RelayClientHandler>)> {
    let mut evicted = Vec::new();
    while cache.len() > max_entries {
        let lru_key = cache
            .iter()
            .filter(|(k, _)| *k != protect)
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(k, _)| k.clone());
        match lru_key {
            Some(key) => {
                if let Some(entry) = cache.remove(&key) {
                    evicted.push((key.0, entry._connection));
                }
            }
            None => break,
        }
    }
    evicted
}

impl UpstreamPool {
    /// Call a single tool on an upstream over a **relay-handled** connection
    /// that is cached per `(upstream, downstream-session, oauth-subject)`.
    ///
    /// Unlike [`UpstreamPool::call_tool`] (a pooled, multiplexed `()`
    /// connection), the connection here is served with a [`RelayClientHandler`]
    /// bound to `downstream`, so any server→client request the upstream raises
    /// mid-call (elicitation/sampling/roots) is forwarded to that one agent.
    ///
    /// `session_id` must uniquely identify the downstream agent connection (the
    /// gateway mints one per `LabMcpServer` session). Together with `subject`
    /// (the OAuth identity, `None` on the raw path) it forms the back of the
    /// cache key, which guarantees a cached relay connection is never reused by
    /// a *different* agent or *different* identity — making the upstream→agent
    /// mapping unambiguous even though the connection is reused across calls
    /// within the session.
    ///
    /// Reuses the generic `connect_upstream_with_handler` seam, so every
    /// transport (HTTP, WebSocket, stdio, OAuth-HTTP) and the stdio
    /// process-reaping guard work unchanged. `subject` is forwarded for
    /// OAuth-scoped upstreams (`None` for the common non-OAuth case).
    ///
    /// Returns `None` only when no connection could be established — mirroring
    /// `call_tool`'s "not connected" signal.
    pub async fn call_tool_relayed(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        params: CallToolRequestParams,
        downstream: Peer<RoleServer>,
        downstream_request_id: RequestId,
        downstream_cancel: CancellationToken,
        session_id: u64,
        capabilities: ClientCapabilities,
        caller_subject: Option<&str>,
        task_authorization: super::TaskRouteAuthorization,
    ) -> Option<Result<CallToolResponse, String>> {
        let started = Instant::now();
        let tool_name = params.name.to_string();
        // Subject-scoped relay is the second OAuth execution entry point (the
        // first being `subject_scoped_call_tool*`), so it carries the same
        // fail-closed `expose_tools` guard. The raw (`subject == None`) branch is
        // deliberately left alone: its owner resolution already went through the
        // catalog, whose entries are exposure-filtered.
        if subject.is_some()
            && !super::tools_call::subject_scoped_tool_is_exposed(config, &tool_name)
        {
            return Some(Err(super::tools_call::hidden_tool_error(
                config, &tool_name,
            )));
        }
        let relay_key = (config.name.clone(), session_id, subject.map(str::to_owned));
        let request_meta = params.meta.clone();
        let (peer, routes, cancellation_sender, generation) = self
            .acquire_or_connect_relay(config, subject, downstream, session_id, capabilities)
            .await?;

        // Mirror the pooled path's observability + circuit-breaker contract (see
        // `timed_capability_call`): emit `request.start`/`finish`/`error` and feed
        // success/failure into the breaker, so a wedged relayed upstream is
        // excluded just like a pooled one. This matters most for the
        // subject-scoped branch, whose MCP arm records nothing itself — without
        // this, a failing OAuth upstream reached over the relay would never trip
        // the breaker. (`acquire_or_connect_relay` already records connect
        // failures, so the raw MCP `None` arm skips its record when relaying.)
        let event = UpstreamRequestLog::tool(&config.name, &tool_name, subject.is_some())
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        let _stdio_inflight = super::stdio_transport::register_inflight(event, generation);

        // Relayed calls block on a human answering the forwarded elicitation,
        // so they use the longer `relay_timeout` (default 5 min) rather than the
        // 30s `request_timeout` the pooled path uses — otherwise a confirmation
        // dialog left open for a minute would abort the whole upstream call.
        let timeout = self.relay_timeout;
        let _permit =
            match tokio::time::timeout(timeout, self.acquire_upstream_call_permit(&config.name))
                .await
            {
                Ok(Ok(permit)) => permit,
                Ok(Err(error)) => return Some(Err(error)),
                Err(_) => {
                    log_upstream_request_error(
                        event,
                        started.elapsed().as_millis(),
                        "queue_saturated",
                        None,
                        None,
                        None,
                    );
                    return Some(Err(format!(
                        "upstream `{}` relay concurrency queue timed out",
                        config.name
                    )));
                }
            };
        let response = send_relay_request(
            &peer,
            &routes,
            cancellation_sender.as_ref(),
            ClientRequest::CallToolRequest(CallToolRequest::new(params)),
            request_meta,
            downstream_request_id,
            downstream_cancel,
            timeout,
        )
        .await;
        match response {
            Ok(result) => {
                let result = match result {
                    ServerResult::CallToolResult(result) => CallToolResponse::Complete(result),
                    ServerResult::InputRequiredResult(result) => {
                        CallToolResponse::InputRequired(result)
                    }
                    ServerResult::CreateTaskResult(result) => CallToolResponse::Task(result),
                    _ => return Some(Err(ServiceError::UnexpectedResponse.to_string())),
                };
                let response_size = estimate_call_tool_response_size(&result);
                let max_bytes = max_response_bytes();
                if response_size > max_bytes {
                    // The peer returned a complete response. The gateway's size
                    // policy rejected it, but the MCP connection remains healthy.
                    self.record_success_for(&config.name, UpstreamCapability::Tools)
                        .await;
                    log_upstream_request_error(
                        event,
                        started.elapsed().as_millis(),
                        "response_too_large",
                        None,
                        Some(response_size),
                        Some(max_bytes),
                    );
                    return Some(Err(format!(
                        "upstream response too large ({response_size} bytes, max {max_bytes})"
                    )));
                }
                self.record_success_for(&config.name, UpstreamCapability::Tools)
                    .await;
                log_upstream_request_finish(
                    event,
                    started.elapsed().as_millis(),
                    Some(response_size),
                );
                let result = self
                    .register_task_response(&relay_key, caller_subject, task_authorization, result)
                    .await;
                Some(result)
            }
            Err(error @ ServiceError::Cancelled { .. }) => {
                log_upstream_request_error(
                    event,
                    started.elapsed().as_millis(),
                    "cancelled",
                    Some(&error),
                    None,
                    None,
                );
                Some(Err(error.to_string()))
            }
            Err(error) => {
                let kind = if matches!(error, ServiceError::Timeout { .. }) {
                    "timeout"
                } else {
                    "upstream_error"
                };
                let message = format!(
                    "relayed upstream call failed: {}",
                    bounded_service_error_text(&error)
                );
                if service_error_affects_connection_health(&error) {
                    // Transport/protocol failures may mean the cached connection
                    // is dead, so trip the breaker and force a reconnect.
                    self.record_failure_for(
                        &config.name,
                        UpstreamCapability::Tools,
                        message.clone(),
                    )
                    .await;
                    self.evict_relay_connection(&config.name, session_id, subject)
                        .await;
                } else {
                    // A valid MCP error response proves the relay connection is alive.
                    self.record_success_for(&config.name, UpstreamCapability::Tools)
                        .await;
                }
                log_upstream_request_error(
                    event,
                    started.elapsed().as_millis(),
                    kind,
                    Some(&error),
                    None,
                    None,
                );
                Some(Err(message))
            }
        }
    }

    /// Fetch one prompt over a request-scoped relay connection, preserving an
    /// upstream `input_required` response for the downstream caller.
    pub async fn get_prompt_relayed(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        mut params: GetPromptRequestParams,
        downstream: Peer<RoleServer>,
        downstream_request_id: RequestId,
        downstream_cancel: CancellationToken,
        session_id: u64,
        capabilities: ClientCapabilities,
    ) -> Option<Result<GetPromptResponse, String>> {
        let started = Instant::now();
        params.name = bare_upstream_prompt_name(&config.name, &params.name).to_string();
        let prompt_name = params.name.to_string();
        // The relay path is a third way to fetch a prompt (selected whenever the
        // downstream advertises an MRTR input capability), so it needs the same
        // `expose_prompts` gate as `get_prompt` and `subject_scoped_get_prompt`.
        if !prompt_exposed(
            &resolve_request_prompt_exposure_policy(&config.name, config.expose_prompts.clone()),
            &config.name,
            &prompt_name,
        ) {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "prompt.get",
                capability = "prompts",
                upstream = %config.name,
                prompt = %prompt_name,
                relayed = true,
                kind = "prompt_not_exposed",
                "relayed upstream prompt get blocked by exposure policy"
            );
            return Some(Err(format!(
                "prompt `{prompt_name}` is not exposed by upstream `{}`",
                config.name
            )));
        }
        let request_meta = params.meta.clone();
        let (peer, routes, cancellation_sender, generation) = self
            .acquire_or_connect_relay(config, subject, downstream, session_id, capabilities)
            .await?;
        let event = UpstreamRequestLog::prompt(&config.name, &prompt_name, subject.is_some())
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        let _stdio_inflight = super::stdio_transport::register_inflight(event, generation);

        let timeout = self.relay_timeout;
        let _permit =
            match tokio::time::timeout(timeout, self.acquire_upstream_call_permit(&config.name))
                .await
            {
                Ok(Ok(permit)) => permit,
                Ok(Err(error)) => return Some(Err(error)),
                Err(_) => {
                    log_upstream_request_error(
                        event,
                        started.elapsed().as_millis(),
                        "queue_saturated",
                        None,
                        None,
                        None,
                    );
                    return Some(Err(format!(
                        "upstream `{}` relay concurrency queue timed out",
                        config.name
                    )));
                }
            };
        let response = send_relay_request(
            &peer,
            &routes,
            cancellation_sender.as_ref(),
            ClientRequest::GetPromptRequest(GetPromptRequest::new(params)),
            request_meta,
            downstream_request_id,
            downstream_cancel,
            timeout,
        )
        .await;
        match response {
            Ok(result) => {
                let result = match result {
                    ServerResult::GetPromptResult(result) => GetPromptResponse::Complete(result),
                    ServerResult::InputRequiredResult(result) => {
                        GetPromptResponse::InputRequired(result)
                    }
                    _ => return Some(Err(ServiceError::UnexpectedResponse.to_string())),
                };
                self.record_success_for(&config.name, UpstreamCapability::Prompts)
                    .await;
                log_upstream_request_finish(event, started.elapsed().as_millis(), Some(0));
                Some(Ok(result))
            }
            Err(error @ ServiceError::Cancelled { .. }) => {
                log_upstream_request_error(
                    event,
                    started.elapsed().as_millis(),
                    "cancelled",
                    Some(&error),
                    None,
                    None,
                );
                Some(Err(error.to_string()))
            }
            Err(error) => {
                let kind = if matches!(error, ServiceError::Timeout { .. }) {
                    "timeout"
                } else {
                    "upstream_error"
                };
                let message = format!(
                    "relayed upstream prompt get failed: {}",
                    bounded_service_error_text(&error)
                );
                self.record_failure_for(&config.name, UpstreamCapability::Prompts, message.clone())
                    .await;
                self.evict_relay_connection(&config.name, session_id, subject)
                    .await;
                log_upstream_request_error(
                    event,
                    started.elapsed().as_millis(),
                    kind,
                    Some(&error),
                    None,
                    None,
                );
                Some(Err(message))
            }
        }
    }

    /// Read one gateway-prefixed resource over a request-scoped relay
    /// connection, preserving MRTR fields and incomplete responses.
    pub async fn read_resource_relayed(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        mut params: ReadResourceRequestParams,
        downstream: Peer<RoleServer>,
        downstream_request_id: RequestId,
        downstream_cancel: CancellationToken,
        session_id: u64,
        capabilities: ClientCapabilities,
    ) -> Option<Result<ReadResourceResponse, String>> {
        let started = Instant::now();
        let gateway_uri = params.uri.clone();
        let prefix = format!("lab://upstream/{}/", config.name);
        let original_uri = match gateway_uri.strip_prefix(&prefix) {
            Some(uri) => uri.to_string(),
            None => {
                return Some(Err(format!(
                    "resource URI does not match upstream `{}`",
                    config.name
                )));
            }
        };
        params.uri = original_uri;
        // Same gate as `read_upstream_resource` / `subject_scoped_read_resource`
        // — the relay branch must not become the way around `expose_resources`.
        if !resource_exposed(
            &resolve_request_resource_exposure_policy(
                &config.name,
                config.expose_resources.clone(),
            ),
            &params.uri,
        ) {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "resource.read",
                capability = "resources",
                upstream = %config.name,
                resource_uri = %redact_resource_uri_for_logging(&gateway_uri),
                relayed = true,
                kind = "resource_not_exposed",
                "relayed upstream resource read blocked by exposure policy"
            );
            return Some(Err(format!(
                "resource is not exposed by upstream `{}`",
                config.name
            )));
        }
        let request_meta = params.meta.clone();
        let (peer, routes, cancellation_sender, generation) = self
            .acquire_or_connect_relay(config, subject, downstream, session_id, capabilities)
            .await?;
        let redacted_uri = redact_resource_uri_for_logging(&gateway_uri);
        let event = UpstreamRequestLog::resource(&config.name, redacted_uri, subject.is_some())
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        let _stdio_inflight = super::stdio_transport::register_inflight(event, generation);

        let timeout = self.relay_timeout;
        let _permit =
            match tokio::time::timeout(timeout, self.acquire_upstream_call_permit(&config.name))
                .await
            {
                Ok(Ok(permit)) => permit,
                Ok(Err(error)) => return Some(Err(error)),
                Err(_) => {
                    log_upstream_request_error(
                        event,
                        started.elapsed().as_millis(),
                        "queue_saturated",
                        None,
                        None,
                        None,
                    );
                    return Some(Err(format!(
                        "upstream `{}` relay concurrency queue timed out",
                        config.name
                    )));
                }
            };
        let response = send_relay_request(
            &peer,
            &routes,
            cancellation_sender.as_ref(),
            ClientRequest::ReadResourceRequest(ReadResourceRequest::new(params)),
            request_meta,
            downstream_request_id,
            downstream_cancel,
            timeout,
        )
        .await;
        match response {
            Ok(result) => {
                let result = match result {
                    ServerResult::ReadResourceResult(result) => {
                        ReadResourceResponse::Complete(result)
                    }
                    ServerResult::InputRequiredResult(result) => {
                        ReadResourceResponse::InputRequired(result)
                    }
                    _ => return Some(Err(ServiceError::UnexpectedResponse.to_string())),
                };
                let response_size = match &result {
                    ReadResourceResponse::Complete(complete) => {
                        estimate_resource_response_size(complete)
                    }
                    ReadResourceResponse::InputRequired(input_required) => {
                        serde_json::to_vec(input_required).map_or(0, |bytes| bytes.len())
                    }
                    _ => 0,
                };
                let max_bytes = max_response_bytes();
                if response_size > max_bytes {
                    let message = format!(
                        "upstream response too large ({response_size} bytes, max {max_bytes})"
                    );
                    self.record_failure_for(
                        &config.name,
                        UpstreamCapability::Resources,
                        message.clone(),
                    )
                    .await;
                    log_upstream_request_error(
                        event,
                        started.elapsed().as_millis(),
                        "response_too_large",
                        None,
                        Some(response_size),
                        Some(max_bytes),
                    );
                    return Some(Err(message));
                }
                let result = match result {
                    ReadResourceResponse::Complete(complete) => ReadResourceResponse::Complete(
                        normalize_resource_result_uri(complete, &gateway_uri),
                    ),
                    incomplete @ ReadResourceResponse::InputRequired(_) => incomplete,
                    other => other,
                };
                self.record_success_for(&config.name, UpstreamCapability::Resources)
                    .await;
                log_upstream_request_finish(
                    event,
                    started.elapsed().as_millis(),
                    Some(response_size),
                );
                Some(Ok(result))
            }
            Err(error @ ServiceError::Cancelled { .. }) => {
                log_upstream_request_error(
                    event,
                    started.elapsed().as_millis(),
                    "cancelled",
                    Some(&error),
                    None,
                    None,
                );
                Some(Err(error.to_string()))
            }
            Err(error) => {
                let kind = if matches!(error, ServiceError::Timeout { .. }) {
                    "timeout"
                } else {
                    "upstream_error"
                };
                let message = format!(
                    "relayed upstream resource read failed: {}",
                    bounded_service_error_text(&error)
                );
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Resources,
                    message.clone(),
                )
                .await;
                self.evict_relay_connection(&config.name, session_id, subject)
                    .await;
                log_upstream_request_error(
                    event,
                    started.elapsed().as_millis(),
                    kind,
                    Some(&error),
                    None,
                    None,
                );
                Some(Err(message))
            }
        }
    }

    /// Return a cached relay peer for `(upstream, session_id)`, or open and
    /// cache a new relay connection. Mirrors `acquire_or_connect_subject`:
    /// write-locked fast path with inline TTL + dead-transport eviction, then a
    /// per-key single-flight slow path.
    async fn acquire_or_connect_relay(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        downstream: Peer<RoleServer>,
        session_id: u64,
        capabilities: ClientCapabilities,
    ) -> Option<(
        Peer<RoleClient>,
        Arc<RelayRouteState>,
        Option<HttpCancellationSender>,
        Option<u64>,
    )> {
        // One shared reader/writer invariant covers subject, relay, task, and
        // OAuth-client publication. Credential invalidation is the sole writer.
        let _oauth_lifecycle = self.oauth_invalidation_barrier.read().await;
        // `subject` (the OAuth identity, `None` on the raw path) is part of the
        // cache key so a connection authenticated as one subject is never reused
        // for a call made as another — see the module-level "Cache key" note.
        let key = (config.name.clone(), session_id, subject.map(str::to_owned));
        let requested_capability_fingerprint = capability_fingerprint(&capabilities);

        // Fast path: fresh, live cached entry using the same per-request
        // capability snapshot. A changed capability set requires a new MCP
        // connection because rmcp fixes outbound client metadata at discovery.
        {
            let mut cache = self.relay_connections.write().await;
            if let Some(entry) = cache.get_mut(&key) {
                if entry.last_used.elapsed() < SUBJECT_CONN_IDLE_TTL
                    && !entry.peer.is_transport_closed()
                    && entry.capability_fingerprint == requested_capability_fingerprint
                {
                    entry.last_used = Instant::now();
                    return Some((
                        entry.peer.clone(),
                        Arc::clone(&entry.routes),
                        entry.cancellation_sender.clone(),
                        entry._connection.runtime.generation,
                    ));
                }
                cache.remove(&key);
            }
        }

        self.ensure_subject_sweep_task().await;

        // Slow path: per-key single-flight so concurrent first calls do not open
        // duplicate connections.
        let connect_lock: Arc<Mutex<()>> = {
            let mut locks = self.relay_connect_locks.write().await;
            locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = connect_lock.lock().await;

        // Re-check after acquiring the lock.
        {
            let mut cache = self.relay_connections.write().await;
            if let Some(entry) = cache.get_mut(&key) {
                if entry.last_used.elapsed() < SUBJECT_CONN_IDLE_TTL
                    && !entry.peer.is_transport_closed()
                    && entry.capability_fingerprint == requested_capability_fingerprint
                {
                    entry.last_used = Instant::now();
                    return Some((
                        entry.peer.clone(),
                        Arc::clone(&entry.routes),
                        entry.cancellation_sender.clone(),
                        entry._connection.runtime.generation,
                    ));
                }
                cache.remove(&key);
            }
        }

        let upstream_name: Arc<str> = Arc::from(config.name.as_str());
        let routes = Arc::new(RelayRouteState::default());
        let handler = RelayClientHandler::new_with_routes(
            downstream,
            Arc::clone(&upstream_name),
            capabilities,
            Arc::clone(&routes),
            self.notification_tx.clone(),
            subject.is_none(),
        );
        let (conn, _tools) = match connect_upstream_with_handler(
            config,
            subject,
            self.oauth_client_cache.as_ref(),
            self.runtime_origin.as_deref(),
            self.runtime_owner.as_ref(),
            Some(&self.shared_http_client),
            handler,
        )
        .await
        {
            Ok(pair) => pair,
            Err(error) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Tools,
                    format!("relayed upstream connect failed: {error}"),
                )
                .await;
                return None;
            }
        };

        let cancellation_sender = match build_http_cancellation_sender(
            config,
            subject,
            self.oauth_client_cache.as_ref(),
            Some(&self.shared_http_client),
        )
        .await
        {
            Ok(sender) => sender,
            Err(error) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Tools,
                    format!("relayed cancellation sender setup failed: {error}"),
                )
                .await;
                conn.shutdown(&config.name, "relay.cancellation_sender.error")
                    .await;
                return None;
            }
        };
        let peer = conn.peer.clone();
        let generation = conn.runtime.generation;
        // Enforce the LRU cap BEFORE inserting so a burst of unique sessions
        // cannot push the live-peer count past the bound; shut evicted peers
        // down off-lock.
        let evicted = {
            let mut cache = self.relay_connections.write().await;
            let evicted = evict_relay_lru_over_cap(&mut cache, SUBJECT_CONN_MAX_ENTRIES - 1, &key);
            cache.insert(
                key.clone(),
                RelayCachedConnection {
                    _connection: conn,
                    peer: peer.clone(),
                    capability_fingerprint: requested_capability_fingerprint,
                    routes: Arc::clone(&routes),
                    cancellation_sender: cancellation_sender.clone(),
                    last_used: Instant::now(),
                },
            );
            evicted
        };
        for (name, evicted_conn) in evicted {
            evicted_conn.shutdown(&name, "relay.cache.lru_evict").await;
        }

        Some((peer, routes, cancellation_sender, generation))
    }

    /// Evict and shut down the cached relay connection for one
    /// `(upstream, session, subject)` key (called on a failed/timed-out call).
    /// `subject` must match the one the connection was opened with (`None` on
    /// the raw path) or the wrong / no entry is removed.
    pub(super) async fn evict_relay_connection(
        &self,
        upstream_name: &str,
        session_id: u64,
        subject: Option<&str>,
    ) {
        let key = (
            upstream_name.to_string(),
            session_id,
            subject.map(str::to_owned),
        );
        let removed = self.relay_connections.write().await.remove(&key);
        if let Some(entry) = removed {
            entry
                ._connection
                .shutdown(upstream_name, "relay.cache.evict")
                .await;
        }
    }

    /// Evict every cached relay connection for one upstream.
    pub(super) async fn evict_relay_connections_for(&self, upstream_name: &str) {
        let drained: Vec<_> = {
            let mut cache = self.relay_connections.write().await;
            let keys = cache
                .keys()
                .filter(|(name, _, _)| name == upstream_name)
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| cache.remove(&key).map(|entry| (key, entry)))
                .collect()
        };
        for ((name, _session, _subject), entry) in drained {
            entry
                ._connection
                .shutdown(&name, "relay.cache.upstream_reconcile")
                .await;
        }
    }

    /// Evict all cached relay connections (called during pool drain).
    pub(super) async fn evict_all_relay_connections(&self) {
        let drained: Vec<_> = self.relay_connections.write().await.drain().collect();
        for ((name, _session, _subject), entry) in drained {
            entry._connection.shutdown(&name, "relay.cache.drain").await;
        }
    }

    /// Sweep the relay-connection cache: evict entries past the idle TTL or
    /// whose upstream transport has closed, shutting their peers down off-lock.
    /// Also prunes orphan single-flight locks. Returns
    /// `(connections_evicted, locks_pruned)`.
    pub(super) async fn sweep_relay_connections(&self) -> (usize, usize) {
        let expired = {
            let mut cache = self.relay_connections.write().await;
            let stale_keys: Vec<(String, u64, Option<String>)> = cache
                .iter()
                .filter(|(_, entry)| {
                    entry.last_used.elapsed() >= SUBJECT_CONN_IDLE_TTL
                        || entry.peer.is_transport_closed()
                })
                .map(|(key, _)| key.clone())
                .collect();
            stale_keys
                .into_iter()
                .filter_map(|key| cache.remove(&key).map(|entry| (key.0, entry._connection)))
                .collect::<Vec<_>>()
        };
        let connections_evicted = expired.len();
        for (name, conn) in expired {
            conn.shutdown(&name, "relay.cache.sweep").await;
        }

        let locks_pruned = {
            let cache = self.relay_connections.read().await;
            let mut locks = self.relay_connect_locks.write().await;
            let before = locks.len();
            locks.retain(|key, lock| cache.contains_key(key) || Arc::strong_count(lock) > 1);
            before - locks.len()
        };

        (connections_evicted, locks_pruned)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, CancelledNotificationParam,
        ClientCapabilities, ContentBlock, CreateTaskResult, DetailedTask, ElicitRequest,
        ElicitRequestParams, ElicitationSchema, ErrorData, GetPromptRequestParams,
        GetPromptResponse, Implementation, InputRequest, InputRequests, InputRequiredResult,
        PaginatedRequestParams, PrimitiveSchemaDefinition, ProgressNotificationParam,
        ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities,
        ServerInfo, ServerNotification, Task, TaskPayload, TaskStatus, TaskStatusNotification,
        TaskStatusNotificationParams,
    };
    use rmcp::service::ClientLifecycleMode;
    use rmcp::service::{NotificationContext, RequestContext, RunningService};
    use rmcp::{ClientHandler, ClientServiceExt, RoleServer, ServerHandler, ServiceExt};
    use std::time::Instant;

    use crate::upstream::types::UpstreamRuntimeMetadata;

    use super::super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
    use super::*;

    fn relay_test_capabilities() -> ClientCapabilities {
        ClientCapabilities::builder().enable_elicitation().build()
    }

    #[tokio::test]
    async fn relay_route_state_translates_request_progress_and_task_ids() {
        let routes = RelayRouteState::default();
        let upstream_request = RequestId::String("upstream-request".into());
        let downstream_request = RequestId::String("downstream-request".into());
        let upstream_progress = ProgressToken(rmcp::model::NumberOrString::String(
            "upstream-progress".into(),
        ));
        let downstream_progress = ProgressToken(rmcp::model::NumberOrString::String(
            "downstream-progress".into(),
        ));

        routes
            .register_request(
                upstream_request.clone(),
                downstream_request.clone(),
                upstream_progress.clone(),
                Some(downstream_progress.clone()),
            )
            .await;

        assert_eq!(
            routes.downstream_request_id(&upstream_request).await,
            Some(downstream_request)
        );
        assert_eq!(
            routes.downstream_progress_token(&upstream_progress).await,
            Some(downstream_progress)
        );

        assert!(
            routes
                .register_task_id("native-task", "gateway-task")
                .await
                .is_empty()
        );
        assert_eq!(
            routes.gateway_task_id("native-task").await.as_deref(),
            Some("gateway-task")
        );

        routes.unregister_request(&upstream_request).await;
        assert_eq!(routes.downstream_request_id(&upstream_request).await, None);
        assert_eq!(
            routes.downstream_progress_token(&upstream_progress).await,
            None
        );
    }

    #[tokio::test]
    async fn task_status_waits_for_gateway_task_registration() {
        let routes = RelayRouteState::default();
        let native_task = DetailedTask::new(
            Task::new(
                "native-early-task",
                TaskStatus::Working,
                "2026-08-01T00:00:00Z",
                "2026-08-01T00:00:00Z",
            ),
            TaskPayload::Working,
        );
        let params = TaskStatusNotificationParams::new(native_task);

        assert!(
            routes
                .translate_or_queue_task_status(params)
                .await
                .is_none()
        );
        let pending = routes
            .register_task_id("native-early-task", "gateway-early-task")
            .await;

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].task.task.task_id, "gateway-early-task");
    }

    /// A downstream client that advertises form elicitation for MRTR.
    #[derive(Clone)]
    struct CapableAgent;

    impl ClientHandler for CapableAgent {
        fn get_info(&self) -> ClientInfo {
            let mut info = ClientInfo::default();
            info.capabilities = relay_test_capabilities();
            info
        }
    }

    /// A trivial downstream-facing server: just enough to hand back a
    /// `Peer<RoleServer>` once the agent connects.
    #[derive(Clone)]
    struct TrivialServer;

    impl ServerHandler for TrivialServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::default()
        }
    }

    #[derive(Clone, Default)]
    struct RecordingAgent {
        progress: Arc<Mutex<Vec<ProgressNotificationParam>>>,
        cancellations: Arc<Mutex<Vec<CancelledNotificationParam>>>,
        task_ids: Arc<Mutex<Vec<String>>>,
    }

    impl ClientHandler for RecordingAgent {
        fn get_info(&self) -> ClientInfo {
            let mut info = ClientInfo::default();
            info.capabilities = ClientCapabilities::builder()
                .enable_elicitation()
                .enable_tasks()
                .build();
            info
        }

        async fn on_progress(
            &self,
            params: ProgressNotificationParam,
            _context: NotificationContext<RoleClient>,
        ) {
            self.progress.lock().await.push(params);
        }

        async fn on_cancelled(
            &self,
            params: CancelledNotificationParam,
            _context: NotificationContext<RoleClient>,
        ) {
            self.cancellations.lock().await.push(params);
        }

        async fn on_task_status(
            &self,
            params: TaskStatusNotificationParams,
            _context: NotificationContext<RoleClient>,
        ) {
            self.task_ids
                .lock()
                .await
                .push(params.task.task.task_id.clone());
        }
    }

    async fn cached_relay_pool<S, C>(
        upstream: S,
        agent: C,
        capabilities: ClientCapabilities,
    ) -> (
        UpstreamPool,
        UpstreamConfig,
        RunningService<RoleServer, TrivialServer>,
    )
    where
        S: ServerHandler,
        C: ClientHandler,
    {
        let pool = UpstreamPool::new();
        let config = super::super::testsupport::test_upstream_config();

        let (gateway_transport, agent_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        tokio::spawn(async move {
            let running = agent
                .serve_with_lifecycle(
                    agent_transport,
                    ClientLifecycleMode::Discover {
                        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                    },
                )
                .await
                .expect("recording agent connects");
            running.waiting().await.expect("recording agent runs");
        });
        let downstream_server = TrivialServer
            .serve(gateway_transport)
            .await
            .expect("downstream server connects");
        let downstream = downstream_server.peer().clone();

        let (upstream_transport, relay_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let upstream_task = tokio::spawn(async move {
            let running = upstream
                .serve(upstream_transport)
                .await
                .expect("test upstream connects");
            running.waiting().await.expect("test upstream runs");
        });
        let routes = Arc::new(RelayRouteState::default());
        let handler = RelayClientHandler::new_with_routes(
            downstream,
            Arc::from(config.name.as_str()),
            capabilities.clone(),
            Arc::clone(&routes),
            pool.notification_tx.clone(),
            true,
        );
        let client_service = handler
            .serve_with_lifecycle(
                relay_transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .expect("relay client connects");
        let peer = client_service.peer().clone();
        pool.relay_connections.write().await.insert(
            (config.name.clone(), 1, None),
            RelayCachedConnection {
                _connection: UpstreamConnection::new(
                    client_service,
                    Some(upstream_task),
                    peer.clone(),
                    UpstreamRuntimeMetadata::default(),
                ),
                peer,
                capability_fingerprint: capability_fingerprint(&capabilities),
                routes,
                cancellation_sender: None,
                last_used: Instant::now(),
            },
        );

        (pool, config, downstream_server)
    }

    async fn wait_for_recorded_count<T>(values: &Mutex<Vec<T>>, expected: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if values.lock().await.len() >= expected {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("notification should be relayed");
    }

    fn elicitation_input_required() -> InputRequiredResult {
        let schema = ElicitationSchema::builder()
            .required_property(
                "confirm",
                PrimitiveSchemaDefinition::Boolean(rmcp::model::BooleanSchema::default()),
            )
            .build()
            .expect("schema builds");
        let params = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "confirm the action?".to_string(),
            requested_schema: schema,
        };
        let requests = InputRequests::from([(
            "confirmation".to_string(),
            InputRequest::Elicitation(ElicitRequest::new(params)),
        )]);
        InputRequiredResult::from_input_requests(requests)
    }

    #[derive(Clone)]
    struct ProgressCancelUpstream;

    impl ServerHandler for ProgressCancelUpstream {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            let progress_token = context
                .meta
                .get_progress_token()
                .expect("relay request carries an upstream progress token");
            context
                .peer
                .notify_progress(
                    ProgressNotificationParam::new(progress_token, 0.5).with_message("halfway"),
                )
                .await
                .expect("upstream sends progress");
            context
                .peer
                .notify_cancelled(CancelledNotificationParam::new(
                    Some(context.id.clone()),
                    Some("upstream cancelled".to_string()),
                ))
                .await
                .expect("upstream sends cancellation");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            Ok(CallToolResult::success(Vec::new()).into())
        }
    }

    #[derive(Clone, Default)]
    struct CancellationAwareUpstream {
        started: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
    }

    impl ServerHandler for CancellationAwareUpstream {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            self.started.store(true, Ordering::SeqCst);
            context.ct.cancelled().await;
            self.cancelled.store(true, Ordering::SeqCst);
            Err(ErrorData::internal_error("cancelled by downstream", None))
        }
    }

    #[derive(Clone)]
    struct TaskNotificationUpstream;

    impl ServerHandler for TaskNotificationUpstream {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_tasks()
                    .build(),
            )
        }

        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            const NATIVE_TASK_ID: &str = "native-notification-task";
            let peer = context.peer.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let task = DetailedTask::new(
                    Task::new(
                        NATIVE_TASK_ID,
                        TaskStatus::Working,
                        "2026-08-01T00:00:00Z",
                        "2026-08-01T00:00:01Z",
                    ),
                    TaskPayload::Working,
                );
                drop(
                    peer.send_notification(ServerNotification::TaskStatusNotification(
                        TaskStatusNotification::new(TaskStatusNotificationParams::new(task)),
                    ))
                    .await,
                );
            });
            Ok(CreateTaskResult::new(Task::new(
                NATIVE_TASK_ID,
                TaskStatus::Working,
                "2026-08-01T00:00:00Z",
                "2026-08-01T00:00:00Z",
            ))
            .into())
        }
    }

    #[tokio::test]
    async fn relay_translates_progress_and_cancelled_notification_ids() {
        let agent = RecordingAgent::default();
        let capabilities = agent.get_info().capabilities;
        let (pool, config, downstream_server) =
            cached_relay_pool(ProgressCancelUpstream, agent.clone(), capabilities.clone()).await;
        let downstream_request_id = RequestId::String("downstream-request".into());
        let downstream_progress_token = ProgressToken(rmcp::model::NumberOrString::String(
            "downstream-progress".into(),
        ));
        let mut meta = RequestMetaObject::with_client_context(
            ProtocolVersion::V_2026_07_28,
            Implementation::new("recording-agent", "1.0.0"),
            capabilities.clone(),
        );
        meta.set_progress_token(downstream_progress_token.clone());
        let mut request = CallToolRequestParams::new("notify");
        request.meta = Some(meta);

        let result = pool
            .call_tool_relayed(
                &config,
                None,
                request,
                downstream_server.peer().clone(),
                downstream_request_id.clone(),
                CancellationToken::new(),
                1,
                capabilities,
                None,
                crate::upstream::pool::TaskRouteAuthorization::root(),
            )
            .await
            .expect("cached relay exists");
        assert!(
            matches!(result, Err(ref message) if message.contains("upstream cancelled")),
            "upstream cancellation should terminate the relayed request: {result:?}"
        );

        wait_for_recorded_count(&agent.progress, 1).await;
        wait_for_recorded_count(&agent.cancellations, 1).await;
        assert_eq!(
            agent.progress.lock().await[0].progress_token,
            downstream_progress_token
        );
        assert_eq!(
            agent.cancellations.lock().await[0].request_id,
            Some(downstream_request_id)
        );
    }

    #[tokio::test]
    async fn downstream_cancellation_cancels_the_upstream_request() {
        let upstream = CancellationAwareUpstream::default();
        let capabilities = relay_test_capabilities();
        let (pool, config, downstream_server) =
            cached_relay_pool(upstream.clone(), CapableAgent, capabilities.clone()).await;
        let cancellation = CancellationToken::new();
        let pool_for_call = pool.clone();
        let config_for_call = config.clone();
        let downstream = downstream_server.peer().clone();
        let cancellation_for_call = cancellation.clone();
        let call = tokio::spawn(async move {
            pool_for_call
                .call_tool_relayed(
                    &config_for_call,
                    None,
                    CallToolRequestParams::new("wait"),
                    downstream,
                    RequestId::String("downstream-cancel".into()),
                    cancellation_for_call,
                    1,
                    capabilities,
                    None,
                    crate::upstream::pool::TaskRouteAuthorization::root(),
                )
                .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !upstream.started.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("upstream request should start");
        cancellation.cancel();
        let result = call.await.expect("relay task joins").expect("relay exists");
        assert!(
            matches!(result, Err(ref message) if message.contains("downstream request cancelled"))
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !upstream.cancelled.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("upstream request context should be cancelled");
    }

    #[tokio::test]
    async fn precancelled_relay_request_never_reaches_the_upstream() {
        let upstream = CancellationAwareUpstream::default();
        let capabilities = relay_test_capabilities();
        let (pool, config, downstream_server) =
            cached_relay_pool(upstream.clone(), CapableAgent, capabilities.clone()).await;
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let result = pool
            .call_tool_relayed(
                &config,
                None,
                CallToolRequestParams::new("must-not-run"),
                downstream_server.peer().clone(),
                RequestId::String("pre-cancelled".into()),
                cancellation,
                1,
                capabilities,
                None,
                crate::upstream::pool::TaskRouteAuthorization::root(),
            )
            .await
            .expect("cached relay exists");

        assert!(
            matches!(result, Err(ref message) if message.contains("already cancelled")),
            "pre-cancelled request must fail locally: {result:?}"
        );
        tokio::task::yield_now().await;
        assert!(
            !upstream.started.load(Ordering::SeqCst),
            "a pre-cancelled destructive request must never execute upstream"
        );
    }

    #[tokio::test]
    async fn relay_cancellation_dispatch_does_not_wait_for_request_id() {
        let capabilities = relay_test_capabilities();
        let (pool, config, _downstream_server) = cached_relay_pool(
            CancellationAwareUpstream::default(),
            CapableAgent,
            capabilities,
        )
        .await;
        let peer = {
            let connections = pool.relay_connections.read().await;
            connections
                .get(&(config.name, 1, None))
                .expect("cached relay connection")
                .peer
                .clone()
        };
        let pending_request_id = Arc::new(PendingRelayRequestId::default());
        let dispatched = AtomicBool::new(false);

        let started = Instant::now();
        dispatch_relay_cancellation(
            &peer,
            None,
            &pending_request_id,
            "downstream request cancelled",
            "test-relay-token",
            &dispatched,
        );

        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "relay-token cancellation must be dispatched before waiting for the upstream request id"
        );
    }

    #[tokio::test]
    async fn task_status_notifications_use_the_gateway_task_id() {
        let agent = RecordingAgent::default();
        let capabilities = agent.get_info().capabilities;
        let (pool, config, downstream_server) = cached_relay_pool(
            TaskNotificationUpstream,
            agent.clone(),
            capabilities.clone(),
        )
        .await;
        let result = pool
            .call_tool_relayed(
                &config,
                None,
                CallToolRequestParams::new("task"),
                downstream_server.peer().clone(),
                RequestId::String("downstream-task".into()),
                CancellationToken::new(),
                1,
                capabilities,
                None,
                crate::upstream::pool::TaskRouteAuthorization::root(),
            )
            .await
            .expect("cached relay exists")
            .expect("task call succeeds");
        assert!(
            matches!(&result, CallToolResponse::Task(_)),
            "expected gateway task handle"
        );
        let CallToolResponse::Task(created) = result else {
            return;
        };
        let gateway_task_id = created.task.task_id;
        assert_ne!(gateway_task_id, "native-notification-task");

        wait_for_recorded_count(&agent.task_ids, 1).await;
        assert_eq!(agent.task_ids.lock().await[0], gateway_task_id);
    }

    /// A mock upstream server whose interactive primitives return MRTR input requests.
    #[derive(Clone)]
    struct ElicitingUpstream;

    impl ServerHandler for ElicitingUpstream {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_prompts()
                    .enable_resources()
                    .build(),
            )
        }

        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            Ok(elicitation_input_required().into())
        }

        async fn get_prompt(
            &self,
            _request: GetPromptRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetPromptResponse, ErrorData> {
            Ok(elicitation_input_required().into())
        }

        async fn read_resource(
            &self,
            _request: ReadResourceRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResponse, ErrorData> {
            Ok(elicitation_input_required().into())
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
            Ok(rmcp::model::ListToolsResult::with_all_items(vec![
                rmcp::model::Tool::new(
                    "echo".to_string(),
                    "echoes confirmation".to_string(),
                    Arc::new(serde_json::Map::new()),
                ),
            ]))
        }
    }

    /// End-to-end proof that the one-round path preserves an upstream MRTR
    /// result instead of invoking a server-initiated callback.
    #[tokio::test]
    async fn upstream_input_required_is_preserved_for_downstream() {
        let (gw_server_transport, agent_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _agent_task = tokio::spawn(async move {
            let running = CapableAgent
                .serve_with_lifecycle(
                    agent_transport,
                    ClientLifecycleMode::Discover {
                        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                    },
                )
                .await
                .expect("agent connects");
            running.waiting().await.expect("agent runs");
        });
        let gw_server = TrivialServer
            .serve(gw_server_transport)
            .await
            .expect("gateway server side connects");
        let downstream = gw_server.peer().clone();

        let (upstream_transport, gw_client_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _upstream_task = tokio::spawn(async move {
            let running = ElicitingUpstream
                .serve(upstream_transport)
                .await
                .expect("upstream connects");
            running.waiting().await.expect("upstream runs");
        });
        let relay_capabilities = relay_test_capabilities();
        let handler = RelayClientHandler::new(
            downstream,
            Arc::from("test-upstream"),
            relay_capabilities.clone(),
        );
        assert_eq!(handler.get_info().capabilities, relay_capabilities);
        let gw_client = handler
            .serve_with_lifecycle(
                gw_client_transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .expect("relayed upstream connection establishes");
        let upstream_peer = gw_client.peer().clone();

        let result = upstream_peer
            .call_tool_once(CallToolRequestParams::new("echo"))
            .await
            .expect("one-round tool call succeeds");
        assert!(
            matches!(result, CallToolResponse::InputRequired(_)),
            "the gateway-facing client must receive the input_required result"
        );
    }

    #[tokio::test]
    async fn upstream_prompt_and_resource_input_required_are_preserved_for_downstream() {
        let pool = UpstreamPool::new();
        let config = super::super::testsupport::test_upstream_config();
        let capabilities = relay_test_capabilities();
        let session_id = 41;
        let (entry, downstream_server) = live_relay_cached_connection(Instant::now()).await;
        let downstream = downstream_server.peer().clone();
        pool.relay_connections
            .write()
            .await
            .insert((config.name.clone(), session_id, None), entry);

        let prompt = pool
            .get_prompt_relayed(
                &config,
                None,
                GetPromptRequestParams::new(format!("{}/confirm", config.name)),
                downstream.clone(),
                RequestId::Number(1),
                CancellationToken::new(),
                session_id,
                capabilities.clone(),
            )
            .await
            .expect("cached relay prompt connection")
            .expect("relayed prompt request succeeds");
        assert!(matches!(prompt, GetPromptResponse::InputRequired(_)));

        let resource = pool
            .read_resource_relayed(
                &config,
                None,
                ReadResourceRequestParams::new(format!(
                    "lab://upstream/{}/file:///confirm",
                    config.name
                )),
                downstream,
                RequestId::Number(2),
                CancellationToken::new(),
                session_id,
                capabilities,
            )
            .await
            .expect("cached relay resource connection")
            .expect("relayed resource request succeeds");
        assert!(matches!(resource, ReadResourceResponse::InputRequired(_)));
    }

    /// `call_tool_relayed` returns `None` (the "not connected" signal, mirroring
    /// `call_tool`) when the dedicated connect fails — here because the config
    /// names neither a URL nor a command. Proves the orchestration's
    /// connect-failure path without needing a live transport.
    #[tokio::test]
    async fn call_tool_relayed_returns_none_when_connect_fails() {
        // A downstream agent peer is required by the signature; the connect
        // fails before it is ever used.
        let (gw_server_transport, agent_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _agent_task = tokio::spawn(async move {
            let running = ().serve(agent_transport).await.expect("agent connects");
            running.waiting().await.expect("agent runs");
        });
        let gw_server = TrivialServer
            .serve(gw_server_transport)
            .await
            .expect("gateway server side connects");
        let downstream = gw_server.peer().clone();

        let pool = UpstreamPool::new();
        // Neither `url` nor `command` set → connect_upstream_with_handler errors.
        let config = super::super::testsupport::test_upstream_config();

        let result = pool
            .call_tool_relayed(
                &config,
                None,
                CallToolRequestParams::new("anything"),
                downstream,
                RequestId::Number(1),
                CancellationToken::new(),
                1,
                relay_test_capabilities(),
                None,
                crate::upstream::pool::TaskRouteAuthorization::root(),
            )
            .await;

        assert!(
            result.is_none(),
            "a failed dedicated connect should surface as None"
        );
    }

    /// Build a live `RelayCachedConnection` over in-memory duplex transports for
    /// the cache-ops tests. Returns the entry plus the downstream-server running
    /// service, which the caller must keep alive (dropping it closes the agent
    /// peer the relay handler is bound to).
    async fn live_relay_cached_connection(
        last_used: Instant,
    ) -> (
        RelayCachedConnection,
        RunningService<RoleServer, TrivialServer>,
    ) {
        let (gw_server_transport, agent_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        tokio::spawn(async move {
            if let Ok(running) = ().serve(agent_transport).await {
                running.waiting().await.ok();
            }
        });
        let gw_server = TrivialServer
            .serve(gw_server_transport)
            .await
            .expect("downstream server connects");
        let downstream = gw_server.peer().clone();

        let (upstream_transport, gw_client_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        tokio::spawn(async move {
            if let Ok(running) = ElicitingUpstream.serve(upstream_transport).await {
                running.waiting().await.ok();
            }
        });
        let routes = Arc::new(RelayRouteState::default());
        let (notification_tx, _receiver) = tokio::sync::broadcast::channel(1);
        let service = RelayClientHandler::new_with_routes(
            downstream,
            Arc::from("up"),
            relay_test_capabilities(),
            Arc::clone(&routes),
            notification_tx,
            false,
        )
        .serve_with_lifecycle(
            gw_client_transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("relay client connects");
        let peer = service.peer().clone();
        let conn = UpstreamConnection {
            _client_service: service.into(),
            _server_task: None,
            peer: peer.clone(),
            runtime: UpstreamRuntimeMetadata::default(),
        };
        (
            RelayCachedConnection {
                _connection: conn,
                peer,
                capability_fingerprint: capability_fingerprint(&relay_test_capabilities()),
                routes,
                cancellation_sender: None,
                last_used,
            },
            gw_server,
        )
    }

    /// `evict_all_relay_connections` empties the cache (and shuts the cached
    /// connections down) — the drain path.
    #[tokio::test]
    async fn relay_cache_evict_all_clears_entries() {
        let pool = UpstreamPool::new();
        let (entry, _keepalive) = live_relay_cached_connection(Instant::now()).await;
        pool.relay_connections
            .write()
            .await
            .insert(("up".to_string(), 7, None), entry);
        assert_eq!(pool.relay_connections.read().await.len(), 1);

        pool.evict_all_relay_connections().await;
        assert!(pool.relay_connections.read().await.is_empty());
    }

    /// `evict_relay_connection` removes only the targeted
    /// `(upstream, session, subject)` entry, leaving a different session's entry
    /// intact.
    #[tokio::test]
    async fn relay_cache_evict_one_is_scoped_to_session() {
        let pool = UpstreamPool::new();
        let (a, _ka) = live_relay_cached_connection(Instant::now()).await;
        let (b, _kb) = live_relay_cached_connection(Instant::now()).await;
        {
            let mut cache = pool.relay_connections.write().await;
            cache.insert(("up".to_string(), 1, None), a);
            cache.insert(("up".to_string(), 2, None), b);
        }

        pool.evict_relay_connection("up", 1, None).await;

        let remaining: Vec<_> = pool
            .relay_connections
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        assert_eq!(remaining, vec![("up".to_string(), 2, None)]);
    }

    /// The cache key includes the OAuth subject, so two identities sharing one
    /// downstream session get **separate** cached connections — a relay
    /// connection authenticated as one subject is never reused for the other.
    /// Regression guard for the subject-isolation fix.
    #[tokio::test]
    async fn relay_cache_key_isolates_oauth_subjects() {
        let pool = UpstreamPool::new();
        let (alice, _ka) = live_relay_cached_connection(Instant::now()).await;
        let (bob, _kb) = live_relay_cached_connection(Instant::now()).await;
        // Same upstream AND same session id (1) — only the subject differs.
        {
            let mut cache = pool.relay_connections.write().await;
            cache.insert(("up".to_string(), 1, Some("alice".to_string())), alice);
            cache.insert(("up".to_string(), 1, Some("bob".to_string())), bob);
        }
        assert_eq!(
            pool.relay_connections.read().await.len(),
            2,
            "two subjects in one session must not collide on the same key"
        );

        // Evicting alice's connection leaves bob's intact.
        pool.evict_relay_connection("up", 1, Some("alice")).await;
        let remaining: Vec<_> = pool
            .relay_connections
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            remaining,
            vec![("up".to_string(), 1, Some("bob".to_string()))],
            "only the targeted subject's connection should be evicted"
        );
    }

    /// Revoking one OAuth subject must close every relay peer authenticated as
    /// that subject while preserving other subjects and unauthenticated peers.
    /// This catches an invalidation path that only evicts `OauthClientCache` or
    /// the ordinary subject-connection cache.
    #[tokio::test]
    async fn oauth_subject_invalidation_evicts_only_matching_relay_peers() {
        let pool = UpstreamPool::new();
        let (alice_a, _alice_a_keepalive) = live_relay_cached_connection(Instant::now()).await;
        let (alice_b, _alice_b_keepalive) = live_relay_cached_connection(Instant::now()).await;
        let (bob, _bob_keepalive) = live_relay_cached_connection(Instant::now()).await;
        let (anonymous, _anonymous_keepalive) = live_relay_cached_connection(Instant::now()).await;
        {
            let mut cache = pool.relay_connections.write().await;
            cache.insert(("up".to_string(), 1, Some("alice".to_string())), alice_a);
            cache.insert(("up".to_string(), 2, Some("alice".to_string())), alice_b);
            cache.insert(("up".to_string(), 3, Some("bob".to_string())), bob);
            cache.insert(("up".to_string(), 4, None), anonymous);
        }

        let invalidated = pool
            .invalidate_oauth_subject_sessions("up", "alice", "oauth.credentials.clear")
            .await;

        assert_eq!(invalidated.relay_connections, 2);
        let mut remaining = pool
            .relay_connections
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        remaining.sort();
        assert_eq!(
            remaining,
            vec![
                ("up".to_string(), 3, Some("bob".to_string())),
                ("up".to_string(), 4, None),
            ]
        );
    }

    /// Revoking a provider credential shared across upstream OAuth managers
    /// must close every authenticated relay peer, while leaving raw relay
    /// sessions alone.
    #[tokio::test]
    async fn shared_oauth_invalidation_preserves_independent_oauth_relay_peers() {
        let pool = UpstreamPool::new();
        let (alice, _alice_keepalive) = live_relay_cached_connection(Instant::now()).await;
        let (bob, _bob_keepalive) = live_relay_cached_connection(Instant::now()).await;
        let (anonymous, _anonymous_keepalive) = live_relay_cached_connection(Instant::now()).await;
        {
            let mut cache = pool.relay_connections.write().await;
            cache.insert(("first".to_string(), 1, Some("alice".to_string())), alice);
            cache.insert(("second".to_string(), 2, Some("bob".to_string())), bob);
            cache.insert(("raw".to_string(), 3, None), anonymous);
        }

        let invalidated = pool
            .invalidate_oauth_upstream_sessions(
                &["first".to_string()],
                "oauth.google_provider.revoke",
            )
            .await;

        assert_eq!(invalidated.relay_connections, 1);
        let mut remaining = pool
            .relay_connections
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        remaining.sort();
        assert_eq!(remaining.len(), 2);
        assert!(remaining.contains(&("second".to_string(), 2, Some("bob".to_string()))));
        assert!(remaining.contains(&("raw".to_string(), 3, None)));
    }

    #[tokio::test]
    async fn subject_and_shared_oauth_invalidation_cannot_deadlock() {
        let pool = UpstreamPool::new();
        let subject_pool = pool.clone();
        let shared_pool = pool.clone();
        let shared_upstreams = ["first".to_string()];

        tokio::time::timeout(std::time::Duration::from_secs(1), async move {
            tokio::join!(
                subject_pool.invalidate_oauth_subject_sessions(
                    "first",
                    "alice",
                    "oauth.credentials.clear",
                ),
                shared_pool.invalidate_oauth_upstream_sessions(
                    &shared_upstreams,
                    "oauth.google_provider.revoke",
                ),
            )
        })
        .await
        .expect("credential invalidations must share one deadlock-free barrier");
    }

    /// `sweep_relay_connections` evicts entries past the idle TTL while keeping
    /// fresh ones.
    #[tokio::test]
    async fn relay_cache_sweep_evicts_idle_entries() {
        use std::time::{Duration, Instant};

        let pool = UpstreamPool::new();
        let stale_used = Instant::now()
            .checked_sub(SUBJECT_CONN_IDLE_TTL + Duration::from_mins(1))
            .expect("instant in range");
        let (stale, _ks) = live_relay_cached_connection(stale_used).await;
        let (fresh, _kf) = live_relay_cached_connection(Instant::now()).await;
        {
            let mut cache = pool.relay_connections.write().await;
            cache.insert(("up".to_string(), 1, None), stale);
            cache.insert(("up".to_string(), 2, None), fresh);
        }

        let (evicted, _pruned) = pool.sweep_relay_connections().await;
        assert_eq!(evicted, 1, "only the idle-TTL-expired entry should evict");

        let remaining: Vec<_> = pool
            .relay_connections
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        assert_eq!(remaining, vec![("up".to_string(), 2, None)]);
    }

    /// The relay path uses its own `relay_timeout` (default 5 min), distinct
    /// from the 30s `request_timeout` — so a relayed call waiting on a human
    /// answering an elicitation is not aborted mid-dialog. Regression guard for
    /// the human-aware-deadline fix; `call_tool_relayed` reads `self.relay_timeout`.
    #[test]
    fn relay_timeout_defaults_to_five_minutes_and_is_configurable() {
        use std::time::Duration;
        let pool = UpstreamPool::new();
        assert_eq!(
            pool.relay_timeout,
            Duration::from_mins(5),
            "default relay timeout must be 5 min, NOT the 30s request timeout"
        );
        assert_ne!(
            pool.relay_timeout, pool.request_timeout,
            "relay and request timeouts must be independent"
        );

        let overridden = UpstreamPool::new().with_relay_timeout(Duration::from_secs(42));
        assert_eq!(overridden.relay_timeout, Duration::from_secs(42));
    }

    /// A valid MCP error response proves the relayed connection is alive.
    /// The error still reaches the caller and logs, but must not poison the
    /// upstream's connection health or evict the reusable relay peer.
    #[tokio::test]
    async fn relayed_mcp_error_keeps_connection_healthy() {
        use super::super::entries::healthy_in_process_entry;
        use std::collections::HashMap;

        /// Upstream whose tool call always errors.
        #[derive(Clone)]
        struct FailingUpstream;
        impl ServerHandler for FailingUpstream {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }
            async fn call_tool(
                &self,
                _request: CallToolRequestParams,
                _context: RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                Err(ErrorData::internal_error("boom".to_string(), None))
            }
            async fn list_tools(
                &self,
                _request: Option<PaginatedRequestParams>,
                _context: RequestContext<RoleServer>,
            ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
                Ok(rmcp::model::ListToolsResult::with_all_items(vec![
                    rmcp::model::Tool::new(
                        "echo".to_string(),
                        "echoes".to_string(),
                        Arc::new(serde_json::Map::new()),
                    ),
                ]))
            }
        }

        let pool = UpstreamPool::new();
        let config = super::super::testsupport::test_upstream_config(); // name = "test"

        // Seed the catalog so `record_failure_for` has an entry to mark unhealthy.
        let name_arc: Arc<str> = Arc::from(config.name.as_str());
        pool.catalog.write().await.insert(
            config.name.clone(),
            healthy_in_process_entry(Arc::clone(&name_arc), HashMap::new()),
        );

        // Downstream agent + a relay connection to the failing upstream, seeded
        // under (name, session=1, None) so `call_tool_relayed` takes the fast
        // path (the test config has no url/command, so a real connect would fail).
        let (gw_server_transport, agent_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        tokio::spawn(async move {
            if let Ok(running) = ().serve(agent_transport).await {
                running.waiting().await.ok();
            }
        });
        let gw_server = TrivialServer
            .serve(gw_server_transport)
            .await
            .expect("downstream server connects");
        let downstream = gw_server.peer().clone();

        let (upstream_transport, gw_client_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        tokio::spawn(async move {
            if let Ok(running) = FailingUpstream.serve(upstream_transport).await {
                running.waiting().await.ok();
            }
        });
        let service = RelayClientHandler::new(
            downstream.clone(),
            Arc::from(config.name.as_str()),
            relay_test_capabilities(),
        )
        .serve(gw_client_transport)
        .await
        .expect("relay client connects");
        let peer = service.peer().clone();
        let conn = UpstreamConnection {
            _client_service: service.into(),
            _server_task: None,
            peer: peer.clone(),
            runtime: UpstreamRuntimeMetadata::default(),
        };
        pool.relay_connections.write().await.insert(
            (config.name.clone(), 1, None),
            RelayCachedConnection {
                _connection: conn,
                peer,
                capability_fingerprint: capability_fingerprint(&relay_test_capabilities()),
                routes: Arc::new(RelayRouteState::default()),
                cancellation_sender: None,
                last_used: Instant::now(),
            },
        );

        let result = pool
            .call_tool_relayed(
                &config,
                None,
                CallToolRequestParams::new("echo"),
                downstream,
                RequestId::Number(1),
                CancellationToken::new(),
                1,
                relay_test_capabilities(),
                None,
                crate::upstream::pool::TaskRouteAuthorization::root(),
            )
            .await;
        assert!(
            matches!(result, Some(Err(_))),
            "a failing relayed upstream call should surface as Some(Err)"
        );

        let status = pool.upstream_status().await;
        let health = status
            .into_iter()
            .find(|(name, _)| name == &config.name)
            .map(|(_, health)| health)
            .expect("upstream present in status");
        assert!(
            health.is_routable(),
            "valid relayed MCP error must keep connection health routable, got {health:?}"
        );
        assert!(
            pool.relay_connections
                .read()
                .await
                .contains_key(&(config.name.clone(), 1, None)),
            "valid MCP error must not evict the relay connection"
        );

        // Hold the downstream server alive until the relayed call completed.
        drop(gw_server);
    }

    /// Relayed calls enforce the same response-size cap as the pooled path,
    /// while preserving connection health because the peer completed a valid response.
    #[tokio::test]
    async fn relayed_call_oversized_response_returns_cap_error() {
        use super::super::entries::healthy_in_process_entry;
        use std::collections::HashMap;

        #[derive(Clone)]
        struct OversizedUpstream;
        impl ServerHandler for OversizedUpstream {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }
            async fn call_tool(
                &self,
                _request: CallToolRequestParams,
                _context: RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                let payload = "x".repeat(12 * 1024 * 1024);
                Ok(CallToolResult::success(vec![ContentBlock::text(payload)]).into())
            }
            async fn list_tools(
                &self,
                _request: Option<PaginatedRequestParams>,
                _context: RequestContext<RoleServer>,
            ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
                Ok(rmcp::model::ListToolsResult::with_all_items(vec![
                    rmcp::model::Tool::new(
                        "big".to_string(),
                        "returns a large response".to_string(),
                        Arc::new(serde_json::Map::new()),
                    ),
                ]))
            }
        }

        let pool = UpstreamPool::new();
        let config = super::super::testsupport::test_upstream_config();
        let name_arc: Arc<str> = Arc::from(config.name.as_str());
        pool.catalog.write().await.insert(
            config.name.clone(),
            healthy_in_process_entry(Arc::clone(&name_arc), HashMap::new()),
        );

        let (gw_server_transport, agent_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        tokio::spawn(async move {
            if let Ok(running) = ().serve(agent_transport).await {
                running.waiting().await.ok();
            }
        });
        let gw_server = TrivialServer
            .serve(gw_server_transport)
            .await
            .expect("downstream server connects");
        let downstream = gw_server.peer().clone();

        let (upstream_transport, gw_client_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        tokio::spawn(async move {
            if let Ok(running) = OversizedUpstream.serve(upstream_transport).await {
                running.waiting().await.ok();
            }
        });
        let service = RelayClientHandler::new(
            downstream.clone(),
            Arc::from(config.name.as_str()),
            relay_test_capabilities(),
        )
        .serve(gw_client_transport)
        .await
        .expect("relay client connects");
        let peer = service.peer().clone();
        let conn = UpstreamConnection {
            _client_service: service.into(),
            _server_task: None,
            peer: peer.clone(),
            runtime: UpstreamRuntimeMetadata::default(),
        };
        pool.relay_connections.write().await.insert(
            (config.name.clone(), 1, None),
            RelayCachedConnection {
                _connection: conn,
                peer,
                capability_fingerprint: capability_fingerprint(&relay_test_capabilities()),
                routes: Arc::new(RelayRouteState::default()),
                cancellation_sender: None,
                last_used: Instant::now(),
            },
        );

        let result = pool
            .call_tool_relayed(
                &config,
                None,
                CallToolRequestParams::new("big"),
                downstream,
                RequestId::Number(1),
                CancellationToken::new(),
                1,
                relay_test_capabilities(),
                None,
                crate::upstream::pool::TaskRouteAuthorization::root(),
            )
            .await
            .expect("cached relay connection is present")
            .expect_err("oversized relayed response should be rejected");
        assert!(
            result.contains("too large"),
            "expected response cap error, got: {result}"
        );

        let health = pool
            .upstream_status()
            .await
            .into_iter()
            .find(|(name, _)| name == &config.name)
            .map(|(_, health)| health)
            .expect("upstream present in status");
        assert!(
            health.is_routable(),
            "oversized relayed response must preserve connection health, got {health:?}"
        );

        drop(gw_server);
    }

    /// `acquire_or_connect_relay` builds the cache key FROM the `subject` param,
    /// so a fast-path lookup hits only the matching subject. Seeding "alice" and
    /// asking for "alice" returns the cached peer (no connect); asking for "bob"
    /// (same upstream + session) misses and falls through to a connect that fails
    /// (the test config has no url/command). Guards the key *construction* at the
    /// live connect seam, complementing `relay_cache_key_isolates_oauth_subjects`
    /// which only proves the key *type* discriminates via direct map insertion.
    #[tokio::test]
    async fn acquire_or_connect_relay_keys_by_subject() {
        let pool = UpstreamPool::new();
        let config = super::super::testsupport::test_upstream_config(); // name "test", no url/command

        // A downstream agent peer is required by the signature; it is unused on
        // the fast path (cache hit) and on the bob miss (connect fails first).
        let (gw_server_transport, agent_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        tokio::spawn(async move {
            if let Ok(running) = ().serve(agent_transport).await {
                running.waiting().await.ok();
            }
        });
        let gw_server = TrivialServer
            .serve(gw_server_transport)
            .await
            .expect("downstream server connects");
        let downstream = gw_server.peer().clone();

        // Seed a live relay connection under (name, session=1, Some("alice")).
        let (entry, _keepalive) = live_relay_cached_connection(Instant::now()).await;
        pool.relay_connections
            .write()
            .await
            .insert((config.name.clone(), 1, Some("alice".to_string())), entry);

        // alice → fast-path cache hit (Some, no connect attempt).
        let alice = pool
            .acquire_or_connect_relay(
                &config,
                Some("alice"),
                downstream.clone(),
                1,
                relay_test_capabilities(),
            )
            .await;
        assert!(
            alice.is_some(),
            "alice's subject-keyed entry must be a cache hit"
        );

        // bob → distinct key → miss → connect attempt → fails (no url/command) →
        // None. If the key ignored the subject, bob would wrongly reuse alice's.
        let bob = pool
            .acquire_or_connect_relay(
                &config,
                Some("bob"),
                downstream,
                1,
                relay_test_capabilities(),
            )
            .await;
        assert!(
            bob.is_none(),
            "bob must NOT reuse alice's connection — the key includes the subject"
        );

        drop(gw_server);
    }
}
