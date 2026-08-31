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
//! upstream request timeout. One absolute deadline spans bulkhead queueing and
//! the upstream send/response phase; downstream cancellation also interrupts
//! queueing before any upstream request is issued.
//!
//! ## Scope
//!
//! Proxied tool calls, prompt fetches, and resource reads can all use the
//! dedicated relay connection when the downstream request requires request-
//! scoped client capabilities. Keeping those capability paths on the same
//! relay machinery preserves downstream metadata, progress/cancellation
//! routing, OAuth subject isolation, and input-required responses consistently
//! across the gateway-facing MCP surface.

use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;

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
#[cfg(test)]
use super::UpstreamConnection;
use super::UpstreamPool;
use super::capability_call::{bounded_service_error_text, service_error_affects_connection_health};
use super::connect::{
    OrderedRelayNotification, RelayNotificationInterceptor,
    connect_upstream_with_handler_and_notifications,
};
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
    UpstreamRequestLog, log_upstream_request_cancelled, log_upstream_request_error,
    log_upstream_request_finish, log_upstream_request_start,
};
use super::notifications::UpstreamNotificationEvent;
use super::relay_cache::{
    RelayCacheKey, RelayCachedConnection, capability_fingerprint, evict_relay_lru_over_cap,
};
use super::relay_cancellation::{
    PendingRelayRequestId, RelayPermitOutcome, RelaySendOutcome, await_relay_permit,
    await_relay_send, dispatch_relay_cancellation, spawn_bounded_handle_cancellation,
};
use super::tools_call::{
    is_tool_header_mismatch, record_header_mismatch, record_header_retry, refresh_tool_header_cache,
};

const RELAY_ROUTE_CLEANUP_GRACE: Duration = Duration::from_secs(2);
const CANCELLATION_NOTIFICATION_DELIVERY_GRACE: Duration = Duration::from_millis(500);
const MAX_PENDING_PROGRESS_ROUTES: usize = 256;
const MAX_PENDING_PROGRESS_PER_ROUTE: usize = 16;
const MAX_PENDING_REQUEST_CANCELLATIONS: usize = 256;

#[derive(Clone)]
struct RelayRequestRoute {
    downstream_request_id: RequestId,
    upstream_progress_token: ProgressToken,
}

#[derive(Default)]
struct RelayCancellationActivity {
    started: u64,
    in_flight: usize,
}

#[derive(Default)]
struct RelayRequestRouteState {
    mappings: HashMap<RequestId, RelayRequestRoute>,
    pending_cancellations: HashMap<RequestId, CancelledNotificationParam>,
    cancellation_activity: HashMap<RequestId, RelayCancellationActivity>,
}

struct RelayRegistrationPending {
    cancellation: Option<CancelledNotificationParam>,
}

#[derive(Clone)]
enum RelayProgressRoute {
    Disabled,
    Activating(ProgressToken),
    Active(ProgressToken),
}

#[derive(Default)]
struct RelayProgressRouteState {
    mappings: HashMap<ProgressToken, RelayProgressRoute>,
    pending: HashMap<ProgressToken, Vec<ProgressNotificationParam>>,
}

#[derive(Default)]
struct RelayTaskRouteState {
    mappings: HashMap<String, String>,
    pending: HashMap<String, Vec<TaskStatusNotificationParams>>,
}

#[derive(Default)]
pub(super) struct RelayRouteState {
    requests: Mutex<RelayRequestRouteState>,
    progress: Mutex<RelayProgressRouteState>,
    tasks: Mutex<RelayTaskRouteState>,
    cancellation_notification_notify: Notify,
    task_notification_sequence: AtomicU64,
    task_notification_notify: Notify,
    #[cfg(test)]
    relay_watcher_finish_sequence: AtomicU64,
    #[cfg(test)]
    relay_watcher_finish_notify: Notify,
}

impl RelayRouteState {
    async fn register_request(
        &self,
        upstream_request_id: RequestId,
        downstream_request_id: RequestId,
        upstream_progress_token: ProgressToken,
        downstream_progress_token: Option<ProgressToken>,
    ) -> RelayRegistrationPending {
        {
            let mut progress = self.progress.lock().await;
            let route = downstream_progress_token
                .map_or(RelayProgressRoute::Disabled, RelayProgressRoute::Activating);
            progress
                .mappings
                .insert(upstream_progress_token.clone(), route);
            if matches!(
                progress.mappings.get(&upstream_progress_token),
                Some(RelayProgressRoute::Disabled)
            ) {
                progress.pending.remove(&upstream_progress_token);
            }
        }
        let pending_cancellation = {
            let mut requests = self.requests.lock().await;
            requests.mappings.insert(
                upstream_request_id.clone(),
                RelayRequestRoute {
                    downstream_request_id: downstream_request_id.clone(),
                    upstream_progress_token,
                },
            );
            requests
                .cancellation_activity
                .entry(upstream_request_id.clone())
                .or_default();
            requests
                .pending_cancellations
                .remove(&upstream_request_id)
                .map(|mut params| {
                    params.request_id = Some(downstream_request_id);
                    params
                })
        };
        RelayRegistrationPending {
            cancellation: pending_cancellation,
        }
    }

    async fn unregister_request(&self, upstream_request_id: &RequestId) {
        let route = {
            let mut requests = self.requests.lock().await;
            requests.pending_cancellations.remove(upstream_request_id);
            requests.cancellation_activity.remove(upstream_request_id);
            requests.mappings.remove(upstream_request_id)
        };
        if let Some(route) = route {
            let mut progress = self.progress.lock().await;
            progress.mappings.remove(&route.upstream_progress_token);
            progress.pending.remove(&route.upstream_progress_token);
        }
    }

    fn schedule_unregister_request(self: &Arc<Self>, upstream_request_id: RequestId) {
        let routes = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(RELAY_ROUTE_CLEANUP_GRACE).await;
            routes.unregister_request(&upstream_request_id).await;
        });
    }

    #[cfg(test)]
    async fn downstream_request_id(&self, upstream_request_id: &RequestId) -> Option<RequestId> {
        self.requests
            .lock()
            .await
            .mappings
            .get(upstream_request_id)
            .map(|route| route.downstream_request_id.clone())
    }

    async fn translate_or_queue_cancellation(
        &self,
        mut params: CancelledNotificationParam,
    ) -> Option<(RequestId, CancelledNotificationParam)> {
        let upstream_request_id = params.request_id.clone()?;
        let mut requests = self.requests.lock().await;
        if let Some(downstream_request_id) = requests
            .mappings
            .get(&upstream_request_id)
            .map(|route| route.downstream_request_id.clone())
        {
            params.request_id = Some(downstream_request_id);
            let activity = requests
                .cancellation_activity
                .entry(upstream_request_id.clone())
                .or_default();
            activity.started = activity.started.saturating_add(1);
            activity.in_flight = activity.in_flight.saturating_add(1);
            drop(requests);
            self.cancellation_notification_notify.notify_waiters();
            return Some((upstream_request_id, params));
        }
        if requests.pending_cancellations.len() < MAX_PENDING_REQUEST_CANCELLATIONS {
            requests
                .pending_cancellations
                .entry(upstream_request_id)
                .or_insert(params);
        }
        None
    }

    async fn begin_cancellation_forward(&self, upstream_request_id: &RequestId) {
        let mut requests = self.requests.lock().await;
        let activity = requests
            .cancellation_activity
            .entry(upstream_request_id.clone())
            .or_default();
        activity.started = activity.started.saturating_add(1);
        activity.in_flight = activity.in_flight.saturating_add(1);
        drop(requests);
        self.cancellation_notification_notify.notify_waiters();
    }

    async fn finish_cancellation_forward(&self, upstream_request_id: &RequestId) {
        let mut requests = self.requests.lock().await;
        if let Some(activity) = requests.cancellation_activity.get_mut(upstream_request_id) {
            activity.in_flight = activity.in_flight.saturating_sub(1);
        }
        drop(requests);
        self.cancellation_notification_notify.notify_waiters();
    }

    async fn cancellation_started(&self, upstream_request_id: &RequestId) -> u64 {
        self.requests
            .lock()
            .await
            .cancellation_activity
            .get(upstream_request_id)
            .map_or(0, |activity| activity.started)
    }

    async fn wait_for_cancellation_handlers_after(
        &self,
        upstream_request_id: &RequestId,
        previous_started: u64,
        timeout: Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.cancellation_notification_notify.notified();
            let (started, in_flight) = {
                let requests = self.requests.lock().await;
                requests
                    .cancellation_activity
                    .get(upstream_request_id)
                    .map_or((0, 0), |activity| (activity.started, activity.in_flight))
            };
            if started > previous_started && in_flight == 0 {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return false;
            }
        }
    }

    #[cfg(test)]
    async fn downstream_progress_token(
        &self,
        upstream_progress_token: &ProgressToken,
    ) -> Option<ProgressToken> {
        match self
            .progress
            .lock()
            .await
            .mappings
            .get(upstream_progress_token)
            .cloned()
        {
            Some(RelayProgressRoute::Activating(token) | RelayProgressRoute::Active(token)) => {
                Some(token)
            }
            Some(RelayProgressRoute::Disabled) | None => None,
        }
    }

    async fn translate_or_queue_progress(
        &self,
        mut params: ProgressNotificationParam,
    ) -> Option<ProgressNotificationParam> {
        let upstream_progress_token = params.progress_token.clone();
        let mut progress = self.progress.lock().await;
        match progress.mappings.get(&upstream_progress_token).cloned() {
            Some(RelayProgressRoute::Active(downstream_progress_token)) => {
                params.progress_token = downstream_progress_token;
                Some(params)
            }
            Some(RelayProgressRoute::Disabled) => None,
            Some(RelayProgressRoute::Activating(_)) | None => {
                if let Some(pending) = progress.pending.get_mut(&upstream_progress_token) {
                    if pending.len() < MAX_PENDING_PROGRESS_PER_ROUTE {
                        pending.push(params);
                    }
                } else if progress.pending.len() < MAX_PENDING_PROGRESS_ROUTES {
                    progress
                        .pending
                        .insert(upstream_progress_token, vec![params]);
                }
                None
            }
        }
    }

    async fn take_pending_progress_batch_or_activate(
        &self,
        upstream_progress_token: &ProgressToken,
    ) -> Option<Vec<ProgressNotificationParam>> {
        let mut progress = self.progress.lock().await;
        let downstream_progress_token =
            match progress.mappings.get(upstream_progress_token).cloned() {
                Some(RelayProgressRoute::Activating(token)) => token,
                Some(RelayProgressRoute::Disabled | RelayProgressRoute::Active(_)) | None => {
                    return None;
                }
            };
        let pending = progress
            .pending
            .remove(upstream_progress_token)
            .unwrap_or_default();
        if pending.is_empty() {
            progress.mappings.insert(
                upstream_progress_token.clone(),
                RelayProgressRoute::Active(downstream_progress_token),
            );
            return None;
        }
        Some(
            pending
                .into_iter()
                .map(|mut params| {
                    params.progress_token = downstream_progress_token.clone();
                    params
                })
                .collect(),
        )
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
        timeout: Duration,
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

    #[cfg(test)]
    async fn wait_for_relay_watcher_finish_after(&self, previous: u64) {
        if self.relay_watcher_finish_sequence.load(Ordering::Acquire) > previous {
            return;
        }
        let notified = self.relay_watcher_finish_notify.notified();
        if self.relay_watcher_finish_sequence.load(Ordering::Acquire) > previous {
            return;
        }
        notified.await;
    }
}

#[cfg(test)]
struct RelayWatcherFinishGuard(Arc<RelayRouteState>);

#[cfg(test)]
impl Drop for RelayWatcherFinishGuard {
    fn drop(&mut self) {
        self.0
            .relay_watcher_finish_sequence
            .fetch_add(1, Ordering::AcqRel);
        self.0.relay_watcher_finish_notify.notify_waiters();
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

    async fn forward_progress(&self, params: ProgressNotificationParam) {
        let upstream_progress_token = params.progress_token.clone();
        let Some(params) = self.routes.translate_or_queue_progress(params).await else {
            tracing::debug!(
                upstream = %self.upstream_name,
                ?upstream_progress_token,
                "queued or dropped progress notification without an active downstream progress route"
            );
            return;
        };
        let downstream = self.downstream().await;
        if let Err(error) = downstream.notify_progress(params).await {
            tracing::warn!(
                upstream = %self.upstream_name,
                error = %error,
                "failed to forward progress notification downstream"
            );
        }
    }

    fn notification_interceptor(&self) -> RelayNotificationInterceptor {
        let handler = self.clone();
        Arc::new(move |notification| {
            let handler = handler.clone();
            Box::pin(async move {
                match notification {
                    OrderedRelayNotification::Progress(params) => {
                        handler.forward_progress(params).await;
                    }
                    OrderedRelayNotification::TaskStatus(params) => {
                        handler.handle_task_status(params).await;
                    }
                }
            })
        })
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

    async fn handle_task_status(&self, params: TaskStatusNotificationParams) {
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
        params: CancelledNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let upstream_request_id = params.request_id.clone();
        let Some((upstream_request_id, params)) =
            self.routes.translate_or_queue_cancellation(params).await
        else {
            tracing::debug!(
                upstream = %self.upstream_name,
                ?upstream_request_id,
                "queued or dropped cancellation notification without an active downstream request route"
            );
            return;
        };
        let downstream = self.downstream().await;
        if let Err(error) = downstream.notify_cancelled(params).await {
            tracing::warn!(
                upstream = %self.upstream_name,
                error = %error,
                "failed to forward cancellation notification downstream"
            );
        }
        self.routes
            .finish_cancellation_forward(&upstream_request_id)
            .await;
        self.routes.schedule_unregister_request(upstream_request_id);
    }

    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.forward_progress(params).await;
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
        self.handle_task_status(params).await;
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
    downstream: &Peer<RoleServer>,
    upstream_name: &str,
    cancellation_sender: Option<&HttpCancellationSender>,
    request: ClientRequest,
    mut request_meta: Option<RequestMetaObject>,
    downstream_request_id: RequestId,
    downstream_cancel: CancellationToken,
    timeout: Duration,
    deadline: tokio::time::Instant,
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
    let options = request_meta
        .map(|meta| PeerRequestOptions::no_options().with_meta(meta))
        .unwrap_or_else(PeerRequestOptions::no_options);
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
        #[cfg(test)]
        let routes = Arc::clone(routes);
        tokio::spawn(async move {
            #[cfg(test)]
            let _finish_guard = RelayWatcherFinishGuard(routes);
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

    let cancellation_started = routes.cancellation_started(&upstream_request_id).await;
    let pending_notifications = routes
        .register_request(
            upstream_request_id.clone(),
            downstream_request_id,
            handle.progress_token.clone(),
            downstream_progress_token,
        )
        .await;
    while let Some(pending_progress) = routes
        .take_pending_progress_batch_or_activate(&handle.progress_token)
        .await
    {
        for params in pending_progress {
            if let Err(error) = downstream.notify_progress(params).await {
                tracing::warn!(
                    upstream = upstream_name,
                    error = %error,
                    "failed to forward queued progress notification downstream"
                );
            }
        }
    }
    if let Some(params) = pending_notifications.cancellation {
        routes
            .begin_cancellation_forward(&upstream_request_id)
            .await;
        if let Err(error) = downstream.notify_cancelled(params).await {
            tracing::warn!(
                upstream = upstream_name,
                error = %error,
                "failed to forward queued cancellation notification downstream"
            );
        }
        routes
            .finish_cancellation_forward(&upstream_request_id)
            .await;
        routes.schedule_unregister_request(upstream_request_id.clone());
    }

    let result = tokio::select! {
        response = &mut handle.rx => {
            // Always retire the detached watcher when the response channel
            // resolves, including upstream cancellation and transport close.
            // Its bounded grace still preserves the late downstream-cancel race.
            relay_finished.cancel();
            let response = response.map_err(|_| ServiceError::TransportClosed)?;
            if matches!(&response, Err(ServiceError::Cancelled { .. })) {
                routes
                    .wait_for_cancellation_handlers_after(
                        &upstream_request_id,
                        cancellation_started,
                        CANCELLATION_NOTIFICATION_DELIVERY_GRACE,
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

type RelayToolRequestFuture<'a> = BoxFuture<'a, Result<ServerResult, ServiceError>>;

fn send_relay_tool_request_with_header_recovery<'a>(
    pool: &'a UpstreamPool,
    peer: &'a Peer<RoleClient>,
    routes: &'a Arc<RelayRouteState>,
    downstream: &'a Peer<RoleServer>,
    upstream_name: &'a str,
    cancellation_sender: Option<&'a HttpCancellationSender>,
    params: CallToolRequestParams,
    request_meta: Option<RequestMetaObject>,
    downstream_request_id: RequestId,
    downstream_cancel: CancellationToken,
    timeout: Duration,
    deadline: tokio::time::Instant,
) -> RelayToolRequestFuture<'a> {
    Box::pin(async move {
        let retry_params = params.clone();
        let retry_request_meta = request_meta.clone();
        let retry_downstream_request_id = downstream_request_id.clone();
        let retry_downstream_cancel = downstream_cancel.clone();
        let response = send_relay_request(
            peer,
            routes,
            downstream,
            upstream_name,
            cancellation_sender,
            ClientRequest::CallToolRequest(CallToolRequest::new(params)),
            request_meta,
            downstream_request_id,
            downstream_cancel,
            timeout,
            deadline,
        )
        .await;
        if !response
            .as_ref()
            .is_err_and(|error| is_tool_header_mismatch(error))
        {
            return response;
        }

        record_header_mismatch(pool, upstream_name);
        tokio::select! {
            refreshed = refresh_tool_header_cache(pool, peer, upstream_name) => refreshed?,
            () = retry_downstream_cancel.cancelled() => {
                return Err(ServiceError::Cancelled {
                    reason: Some("downstream request cancelled during header-schema refresh".to_string()),
                });
            }
            () = tokio::time::sleep_until(deadline) => {
                return Err(ServiceError::Timeout { timeout });
            }
        }

        let result = send_relay_request(
            peer,
            routes,
            downstream,
            upstream_name,
            cancellation_sender,
            ClientRequest::CallToolRequest(CallToolRequest::new(retry_params)),
            retry_request_meta,
            retry_downstream_request_id,
            retry_downstream_cancel,
            timeout,
            deadline,
        )
        .await;
        record_header_retry(pool, upstream_name, &result);
        result
    })
}

fn downstream_cancelled(reason: &str) -> String {
    ServiceError::Cancelled {
        reason: Some(reason.to_string()),
    }
    .to_string()
}

impl UpstreamPool {
    /// Call a single tool on an upstream over a **relay-handled** connection
    /// that is cached per `(upstream, downstream-session, oauth-subject)`.
    ///
    /// Unlike [`UpstreamPool::call_tool`] (a pooled, multiplexed `()`
    /// connection), the connection here is served with a `RelayClientHandler`
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
    /// Reuses the generic relay connection seam, including its sequential,
    /// cancellation-safe progress/task-status notification transport, so every
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
    ) -> Option<Result<CallToolResponse, super::CapabilityCallError>> {
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
            return Some(Err(super::CapabilityCallError::Other {
                message: super::tools_call::hidden_tool_error(config, &tool_name),
            }));
        }
        if downstream_cancel.is_cancelled() {
            return Some(Err(super::CapabilityCallError::Cancelled {
                message: downstream_cancelled("downstream request was already cancelled"),
            }));
        }
        let oauth_epoch = if subject.is_some() {
            self.oauth_lifecycle_epoch()
        } else {
            None
        };
        let relay_key = (
            config.name.clone(),
            session_id,
            subject.map(str::to_owned),
            capability_fingerprint(&capabilities),
        );
        let request_meta = params.meta.clone();
        let timeout = self.relay_timeout;
        let deadline = tokio::time::Instant::now() + timeout;
        let event = UpstreamRequestLog::tool(&config.name, &tool_name, subject.is_some())
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        let downstream_for_request = downstream.clone();
        let connection = tokio::select! {
            biased;
            () = downstream_cancel.cancelled() => {
                log_upstream_request_cancelled(event, started.elapsed().as_millis(), "connect_cancelled");
                super::usage_record::record_usage_call(self, event, caller_subject, "connect_cancelled", started.elapsed().as_millis());
                return Some(Err(super::CapabilityCallError::Cancelled {
                    message: downstream_cancelled("downstream request cancelled while connecting"),
                }));
            }
            () = tokio::time::sleep_until(deadline) => {
                let message = format!("upstream `{}` relay connection timed out", config.name);
                log_upstream_request_error(event, started.elapsed().as_millis(), "connect_timeout", None, None, None);
                super::usage_record::record_usage_call(self, event, caller_subject, "connect_timeout", started.elapsed().as_millis());
                self.record_failure_for(&config.name, UpstreamCapability::Tools, message.clone()).await;
                return Some(Err(super::CapabilityCallError::Timeout {
                    message,
                }));
            }
            connection = self.acquire_or_connect_relay_guarded(
                config, subject, downstream, session_id, capabilities
            ) => connection,
        };
        let Some(connection) = connection else {
            log_upstream_request_error(
                event,
                started.elapsed().as_millis(),
                "connect_error",
                None,
                None,
                None,
            );
            super::usage_record::record_usage_call(
                self,
                event,
                caller_subject,
                "connect_error",
                started.elapsed().as_millis(),
            );
            return None;
        };
        let (peer, routes, cancellation_sender, generation) = connection;

        // Mirror the pooled path's observability + circuit-breaker contract (see
        // `timed_capability_call`): emit `request.start`/`finish`/`error` and feed
        // success/failure into the breaker, so a wedged relayed upstream is
        // excluded just like a pooled one. This matters most for the
        // subject-scoped branch, whose MCP arm records nothing itself — without
        // this, a failing OAuth upstream reached over the relay would never trip
        // the breaker. (`acquire_or_connect_relay` already records connect
        // failures, so the raw MCP `None` arm skips its record when relaying.)
        let _stdio_inflight = super::stdio_transport::register_inflight(event, generation);

        // Relayed calls block on a human answering the forwarded elicitation,
        // so they use the longer `relay_timeout` (default 5 min) rather than the
        // 30s `request_timeout` the pooled path uses — otherwise a confirmation
        // dialog left open for a minute would abort the whole upstream call.
        let _permit = match await_relay_permit(
            self.acquire_upstream_call_permit(&config.name),
            &downstream_cancel,
            deadline,
        )
        .await
        {
            RelayPermitOutcome::Acquired(Ok(permit)) => permit,
            RelayPermitOutcome::Acquired(Err(error)) => {
                log_upstream_request_error(
                    event,
                    started.elapsed().as_millis(),
                    "queue_error",
                    None,
                    None,
                    None,
                );
                super::usage_record::record_usage_call(
                    self,
                    event,
                    caller_subject,
                    "queue_error",
                    started.elapsed().as_millis(),
                );
                return Some(Err(super::CapabilityCallError::Other { message: error }));
            }
            RelayPermitOutcome::Cancelled => {
                log_upstream_request_cancelled(event, started.elapsed().as_millis(), "cancelled");
                super::usage_record::record_usage_call(
                    self,
                    event,
                    caller_subject,
                    "cancelled",
                    started.elapsed().as_millis(),
                );
                return Some(Err(super::CapabilityCallError::Cancelled {
                    message: downstream_cancelled("downstream request cancelled while queued"),
                }));
            }
            RelayPermitOutcome::TimedOut => {
                log_upstream_request_error(
                    event,
                    started.elapsed().as_millis(),
                    "queue_saturated",
                    None,
                    None,
                    None,
                );
                let message = format!(
                    "upstream `{}` relay concurrency queue timed out",
                    config.name
                );
                super::usage_record::record_usage_call(
                    self,
                    event,
                    caller_subject,
                    "queue_saturated",
                    started.elapsed().as_millis(),
                );
                return Some(Err(super::CapabilityCallError::QueueSaturated { message }));
            }
        };
        // Keep the HeaderMismatch refresh/replay state out of this already-large
        // relay future. Multi-hop relays nest this future recursively; carrying
        // the retry branch inline pushed Tokio worker stacks over the edge even
        // when HeaderMismatch recovery was never exercised. Boxing the focused
        // request future keeps the normal relay connection path stack-bounded.
        let response = send_relay_tool_request_with_header_recovery(
            self,
            &peer,
            &routes,
            &downstream_for_request,
            &config.name,
            cancellation_sender.as_ref(),
            params,
            request_meta,
            downstream_request_id,
            downstream_cancel,
            timeout,
            deadline,
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
                    _ => {
                        let error = ServiceError::UnexpectedResponse;
                        let message =
                            "relayed upstream returned an unexpected response".to_string();
                        self.record_failure_for(
                            &config.name,
                            UpstreamCapability::Tools,
                            message.clone(),
                        )
                        .await;
                        self.evict_relay_connection(&relay_key).await;
                        log_upstream_request_error(
                            event,
                            started.elapsed().as_millis(),
                            "protocol_error",
                            Some(&error),
                            None,
                            None,
                        );
                        super::usage_record::record_usage_call(
                            self,
                            event,
                            caller_subject,
                            "protocol_error",
                            started.elapsed().as_millis(),
                        );
                        return Some(Err(super::CapabilityCallError::Protocol { message }));
                    }
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
                    let message = format!(
                        "upstream response too large ({response_size} bytes, max {max_bytes})"
                    );
                    super::usage_record::record_usage_call(
                        self,
                        event,
                        caller_subject,
                        "response_too_large",
                        started.elapsed().as_millis(),
                    );
                    return Some(Err(super::CapabilityCallError::ResponseTooLarge {
                        message,
                    }));
                }
                let _oauth_publication = match self.oauth_publication_guard(oauth_epoch).await {
                    Ok(guard) => guard,
                    Err(error) => {
                        return Some(Err(super::CapabilityCallError::Other {
                            message: error.to_string(),
                        }));
                    }
                };
                let result = self
                    .register_task_response(&relay_key, caller_subject, task_authorization, result)
                    .await;
                match result {
                    Ok(result) => {
                        self.record_success_for(&config.name, UpstreamCapability::Tools)
                            .await;
                        log_upstream_request_finish(
                            event,
                            started.elapsed().as_millis(),
                            Some(response_size),
                        );
                        super::usage_record::record_usage_call(
                            self,
                            event,
                            caller_subject,
                            "ok",
                            started.elapsed().as_millis(),
                        );
                        Some(Ok(result))
                    }
                    Err(message) => {
                        let error = ServiceError::UnexpectedResponse;
                        self.record_failure_for(
                            &config.name,
                            UpstreamCapability::Tools,
                            message.clone(),
                        )
                        .await;
                        log_upstream_request_error(
                            event,
                            started.elapsed().as_millis(),
                            "protocol_error",
                            Some(&error),
                            None,
                            None,
                        );
                        super::usage_record::record_usage_call(
                            self,
                            event,
                            caller_subject,
                            "protocol_error",
                            started.elapsed().as_millis(),
                        );
                        Some(Err(super::CapabilityCallError::Protocol { message }))
                    }
                }
            }
            Err(error @ ServiceError::Cancelled { .. }) => {
                log_upstream_request_cancelled(event, started.elapsed().as_millis(), "cancelled");
                super::usage_record::record_usage_call(
                    self,
                    event,
                    caller_subject,
                    "cancelled",
                    started.elapsed().as_millis(),
                );
                Some(Err(super::CapabilityCallError::Cancelled {
                    message: error.to_string(),
                }))
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
                    self.evict_relay_connection(&relay_key).await;
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
                super::usage_record::record_usage_call(
                    self,
                    event,
                    caller_subject,
                    kind,
                    started.elapsed().as_millis(),
                );
                Some(Err(super::CapabilityCallError::from_service_error(
                    error, message,
                )))
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
        if downstream_cancel.is_cancelled() {
            return Some(Err(downstream_cancelled(
                "downstream request was already cancelled",
            )));
        }
        let request_meta = params.meta.clone();
        let timeout = self.relay_timeout;
        let deadline = tokio::time::Instant::now() + timeout;
        let relay_key = (
            config.name.clone(),
            session_id,
            subject.map(str::to_owned),
            capability_fingerprint(&capabilities),
        );
        let event = UpstreamRequestLog::prompt(&config.name, &prompt_name, subject.is_some())
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        let downstream_for_request = downstream.clone();
        let connection = tokio::select! {
            biased;
            () = downstream_cancel.cancelled() => {
                log_upstream_request_cancelled(event, started.elapsed().as_millis(), "connect_cancelled");
                super::usage_record::record_usage_call(self, event, subject, "connect_cancelled", started.elapsed().as_millis());
                return Some(Err(downstream_cancelled(
                    "downstream request cancelled while connecting",
                )));
            }
            () = tokio::time::sleep_until(deadline) => {
                let message = format!("upstream `{}` relay connection timed out", config.name);
                log_upstream_request_error(event, started.elapsed().as_millis(), "connect_timeout", None, None, None);
                super::usage_record::record_usage_call(self, event, subject, "connect_timeout", started.elapsed().as_millis());
                self.record_failure_for(&config.name, UpstreamCapability::Prompts, message.clone()).await;
                return Some(Err(message));
            }
            connection = self.acquire_or_connect_relay(
                config, subject, downstream, session_id, capabilities
            ) => connection,
        };
        let Some(connection) = connection else {
            log_upstream_request_error(
                event,
                started.elapsed().as_millis(),
                "connect_error",
                None,
                None,
                None,
            );
            super::usage_record::record_usage_call(
                self,
                event,
                subject,
                "connect_error",
                started.elapsed().as_millis(),
            );
            return None;
        };
        let (peer, routes, cancellation_sender, generation) = connection;
        let _stdio_inflight = super::stdio_transport::register_inflight(event, generation);

        let _permit = match await_relay_permit(
            self.acquire_upstream_call_permit(&config.name),
            &downstream_cancel,
            deadline,
        )
        .await
        {
            RelayPermitOutcome::Acquired(Ok(permit)) => permit,
            RelayPermitOutcome::Acquired(Err(error)) => {
                log_upstream_request_error(
                    event,
                    started.elapsed().as_millis(),
                    "queue_error",
                    None,
                    None,
                    None,
                );
                return Some(Err(error));
            }
            RelayPermitOutcome::Cancelled => {
                log_upstream_request_cancelled(event, started.elapsed().as_millis(), "cancelled");
                return Some(Err(downstream_cancelled(
                    "downstream request cancelled while queued",
                )));
            }
            RelayPermitOutcome::TimedOut => {
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
            &downstream_for_request,
            &config.name,
            cancellation_sender.as_ref(),
            ClientRequest::GetPromptRequest(GetPromptRequest::new(params)),
            request_meta,
            downstream_request_id,
            downstream_cancel,
            timeout,
            deadline,
        )
        .await;
        match response {
            Ok(result) => {
                let result = match result {
                    ServerResult::GetPromptResult(result) => GetPromptResponse::Complete(result),
                    ServerResult::InputRequiredResult(result) => {
                        GetPromptResponse::InputRequired(result)
                    }
                    _ => {
                        let error = ServiceError::UnexpectedResponse;
                        let message =
                            "relayed upstream prompt returned an unexpected response".to_string();
                        self.record_failure_for(
                            &config.name,
                            UpstreamCapability::Prompts,
                            message.clone(),
                        )
                        .await;
                        self.evict_relay_connection(&relay_key).await;
                        log_upstream_request_error(
                            event,
                            started.elapsed().as_millis(),
                            "protocol_error",
                            Some(&error),
                            None,
                            None,
                        );
                        return Some(Err(message));
                    }
                };
                self.record_success_for(&config.name, UpstreamCapability::Prompts)
                    .await;
                log_upstream_request_finish(event, started.elapsed().as_millis(), Some(0));
                Some(Ok(result))
            }
            Err(error @ ServiceError::Cancelled { .. }) => {
                log_upstream_request_cancelled(event, started.elapsed().as_millis(), "cancelled");
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
                if service_error_affects_connection_health(&error) {
                    self.record_failure_for(
                        &config.name,
                        UpstreamCapability::Prompts,
                        message.clone(),
                    )
                    .await;
                    self.evict_relay_connection(&relay_key).await;
                } else {
                    self.record_success_for(&config.name, UpstreamCapability::Prompts)
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

    /// Read one gateway-prefixed or native MCP App `ui://` resource over a
    /// request-scoped relay connection, preserving MRTR fields and incomplete
    /// responses.
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
        let original_uri = if let Some(uri) = gateway_uri.strip_prefix(&prefix) {
            uri.to_string()
        } else if gateway_uri.starts_with("ui://") {
            gateway_uri.clone()
        } else {
            return Some(Err(format!(
                "resource URI does not match upstream `{}`",
                config.name
            )));
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
        if downstream_cancel.is_cancelled() {
            return Some(Err(downstream_cancelled(
                "downstream request was already cancelled",
            )));
        }
        let request_meta = params.meta.clone();
        let timeout = self.relay_timeout;
        let deadline = tokio::time::Instant::now() + timeout;
        let relay_key = (
            config.name.clone(),
            session_id,
            subject.map(str::to_owned),
            capability_fingerprint(&capabilities),
        );
        let redacted_uri = redact_resource_uri_for_logging(&gateway_uri);
        let event = UpstreamRequestLog::resource(&config.name, redacted_uri, subject.is_some())
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        let downstream_for_request = downstream.clone();
        let connection = tokio::select! {
            biased;
            () = downstream_cancel.cancelled() => {
                log_upstream_request_cancelled(event, started.elapsed().as_millis(), "connect_cancelled");
                super::usage_record::record_usage_call(self, event, subject, "connect_cancelled", started.elapsed().as_millis());
                return Some(Err(downstream_cancelled(
                    "downstream request cancelled while connecting",
                )));
            }
            () = tokio::time::sleep_until(deadline) => {
                let message = format!("upstream `{}` relay connection timed out", config.name);
                log_upstream_request_error(event, started.elapsed().as_millis(), "connect_timeout", None, None, None);
                super::usage_record::record_usage_call(self, event, subject, "connect_timeout", started.elapsed().as_millis());
                self.record_failure_for(&config.name, UpstreamCapability::Resources, message.clone()).await;
                return Some(Err(message));
            }
            connection = self.acquire_or_connect_relay(
                config, subject, downstream, session_id, capabilities
            ) => connection,
        };
        let Some(connection) = connection else {
            log_upstream_request_error(
                event,
                started.elapsed().as_millis(),
                "connect_error",
                None,
                None,
                None,
            );
            super::usage_record::record_usage_call(
                self,
                event,
                subject,
                "connect_error",
                started.elapsed().as_millis(),
            );
            return None;
        };
        let (peer, routes, cancellation_sender, generation) = connection;
        let _stdio_inflight = super::stdio_transport::register_inflight(event, generation);

        let _permit = match await_relay_permit(
            self.acquire_upstream_call_permit(&config.name),
            &downstream_cancel,
            deadline,
        )
        .await
        {
            RelayPermitOutcome::Acquired(Ok(permit)) => permit,
            RelayPermitOutcome::Acquired(Err(error)) => {
                log_upstream_request_error(
                    event,
                    started.elapsed().as_millis(),
                    "queue_error",
                    None,
                    None,
                    None,
                );
                return Some(Err(error));
            }
            RelayPermitOutcome::Cancelled => {
                log_upstream_request_cancelled(event, started.elapsed().as_millis(), "cancelled");
                return Some(Err(downstream_cancelled(
                    "downstream request cancelled while queued",
                )));
            }
            RelayPermitOutcome::TimedOut => {
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
            &downstream_for_request,
            &config.name,
            cancellation_sender.as_ref(),
            ClientRequest::ReadResourceRequest(ReadResourceRequest::new(params)),
            request_meta,
            downstream_request_id,
            downstream_cancel,
            timeout,
            deadline,
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
                    _ => {
                        let error = ServiceError::UnexpectedResponse;
                        let message =
                            "relayed upstream resource returned an unexpected response".to_string();
                        self.record_failure_for(
                            &config.name,
                            UpstreamCapability::Resources,
                            message.clone(),
                        )
                        .await;
                        self.evict_relay_connection(&relay_key).await;
                        log_upstream_request_error(
                            event,
                            started.elapsed().as_millis(),
                            "protocol_error",
                            Some(&error),
                            None,
                            None,
                        );
                        return Some(Err(message));
                    }
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
                    self.record_success_for(&config.name, UpstreamCapability::Resources)
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
                log_upstream_request_cancelled(event, started.elapsed().as_millis(), "cancelled");
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
                if service_error_affects_connection_health(&error) {
                    self.record_failure_for(
                        &config.name,
                        UpstreamCapability::Resources,
                        message.clone(),
                    )
                    .await;
                    self.evict_relay_connection(&relay_key).await;
                } else {
                    self.record_success_for(&config.name, UpstreamCapability::Resources)
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

    /// Return a cached relay peer for one full [`RelayCacheKey`], or open and
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
        self.acquire_or_connect_relay_guarded(config, subject, downstream, session_id, capabilities)
            .await
    }

    /// Relay acquisition when the caller already owns the OAuth lifecycle
    /// reader for a larger atomic operation.
    async fn acquire_or_connect_relay_guarded(
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
        let lifecycle_epoch = subject.and_then(|_| self.oauth_lifecycle_epoch());
        // `subject` (the OAuth identity, `None` on the raw path) is part of the
        // cache key so a connection authenticated as one subject is never reused
        // for a call made as another — see the module-level "Cache key" note.
        let requested_capability_fingerprint = capability_fingerprint(&capabilities);
        let key = (
            config.name.clone(),
            session_id,
            subject.map(str::to_owned),
            requested_capability_fingerprint.clone(),
        );

        // Fast path: fresh, live cached entry using the same per-request
        // capability snapshot. A changed capability set requires a new MCP
        // connection because rmcp fixes outbound client metadata at discovery.
        {
            let mut cache = self.relay_connections.write().await;
            if let Some(entry) = cache.get_mut(&key) {
                if entry.last_used.elapsed() < SUBJECT_CONN_IDLE_TTL
                    && !entry.peer.is_transport_closed()
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
        let notification_interceptor = handler.notification_interceptor();
        let (conn, _tools) = match connect_upstream_with_handler_and_notifications(
            config,
            subject,
            self.oauth_client_cache.as_ref(),
            self.runtime_origin.as_deref(),
            self.runtime_owner.as_ref(),
            Some(&self.shared_http_client),
            handler,
            Some(notification_interceptor),
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
        let _oauth_publication = match self.oauth_publication_guard(lifecycle_epoch).await {
            Ok(guard) => guard,
            Err(_) => {
                conn.shutdown(&config.name, "relay.oauth_epoch.changed")
                    .await;
                return None;
            }
        };
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
    pub(super) async fn evict_relay_connection(&self, key: &RelayCacheKey) {
        let removed = self.relay_connections.write().await.remove(key);
        if let Some(entry) = removed {
            entry
                ._connection
                .shutdown(&key.0, "relay.cache.evict")
                .await;
        }
    }

    /// Evict every cached relay connection for one upstream.
    pub(super) async fn evict_relay_connections_for(&self, upstream_name: &str) {
        let drained: Vec<_> = {
            let mut cache = self.relay_connections.write().await;
            let keys = cache
                .keys()
                .filter(|(name, _, _, _)| name == upstream_name)
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| cache.remove(&key).map(|entry| (key, entry)))
                .collect()
        };
        for ((name, _session, _subject, _fingerprint), entry) in drained {
            entry
                ._connection
                .shutdown(&name, "relay.cache.upstream_reconcile")
                .await;
        }
    }

    /// Evict all cached relay connections (called during pool drain).
    pub(super) async fn evict_all_relay_connections(&self) {
        let drained: Vec<_> = self.relay_connections.write().await.drain().collect();
        for ((name, _session, _subject, _fingerprint), entry) in drained {
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
            let stale_keys: Vec<RelayCacheKey> = cache
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
#[allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
mod tests {
    use std::mem::size_of;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, CancelledNotificationParam,
        ClientCapabilities, ContentBlock, CreateTaskResult, DetailedTask, ElicitRequest,
        ElicitRequestParams, ElicitationSchema, ErrorCode, ErrorData, GetPromptRequestParams,
        GetPromptResponse, Implementation, InputRequest, InputRequests, InputRequiredResult,
        PaginatedRequestParams, PrimitiveSchemaDefinition, ProgressNotificationParam,
        ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities,
        ServerInfo, ServerNotification, Task, TaskPayload, TaskStatus, TaskStatusNotification,
        TaskStatusNotificationParams,
    };
    use rmcp::service::ClientLifecycleMode;
    use rmcp::service::{NotificationContext, RequestContext, RunningService};
    use rmcp::{ClientHandler, ClientServiceExt, RoleServer, ServerHandler, ServiceExt};
    use std::time::{Duration, Instant};

    use crate::upstream::types::UpstreamRuntimeMetadata;

    use super::super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
    use super::*;

    fn relay_test_capabilities() -> ClientCapabilities {
        ClientCapabilities::builder().enable_elicitation().build()
    }

    fn relay_cache_key(name: &str, session: u64, subject: Option<&str>) -> RelayCacheKey {
        relay_cache_key_for_capabilities(name, session, subject, &relay_test_capabilities())
    }

    fn relay_cache_key_for_capabilities(
        name: &str,
        session: u64,
        subject: Option<&str>,
        capabilities: &ClientCapabilities,
    ) -> RelayCacheKey {
        (
            name.to_string(),
            session,
            subject.map(str::to_owned),
            capability_fingerprint(capabilities),
        )
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

        let pending = routes
            .register_request(
                upstream_request.clone(),
                downstream_request.clone(),
                upstream_progress.clone(),
                Some(downstream_progress.clone()),
            )
            .await;
        assert!(pending.cancellation.is_none());
        assert!(
            routes
                .take_pending_progress_batch_or_activate(&upstream_progress)
                .await
                .is_none()
        );

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
    async fn progress_waits_for_request_route_registration() {
        let routes = RelayRouteState::default();
        let upstream_request = RequestId::String("upstream-early-request".into());
        let downstream_request = RequestId::String("downstream-early-request".into());
        let upstream_progress = ProgressToken(rmcp::model::NumberOrString::String(
            "upstream-early-progress".into(),
        ));
        let downstream_progress = ProgressToken(rmcp::model::NumberOrString::String(
            "downstream-early-progress".into(),
        ));
        let params =
            ProgressNotificationParam::new(upstream_progress.clone(), 0.25).with_message("early");

        assert!(routes.translate_or_queue_progress(params).await.is_none());
        let pending = routes
            .register_request(
                upstream_request,
                downstream_request,
                upstream_progress.clone(),
                Some(downstream_progress.clone()),
            )
            .await;
        assert!(pending.cancellation.is_none());

        let first = routes
            .take_pending_progress_batch_or_activate(&upstream_progress)
            .await
            .expect("first queued progress batch");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].progress_token, downstream_progress);
        assert_eq!(first[0].message.as_deref(), Some("early"));

        let later =
            ProgressNotificationParam::new(upstream_progress.clone(), 0.75).with_message("later");
        assert!(routes.translate_or_queue_progress(later).await.is_none());
        let second = routes
            .take_pending_progress_batch_or_activate(&upstream_progress)
            .await
            .expect("second queued progress batch");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].progress_token, downstream_progress);
        assert_eq!(second[0].message.as_deref(), Some("later"));
        assert!(
            routes
                .take_pending_progress_batch_or_activate(&upstream_progress)
                .await
                .is_none()
        );

        let live =
            ProgressNotificationParam::new(upstream_progress.clone(), 1.0).with_message("live");
        let translated = routes
            .translate_or_queue_progress(live)
            .await
            .expect("active progress route");
        assert_eq!(translated.progress_token, downstream_progress);
        assert_eq!(translated.message.as_deref(), Some("live"));
    }

    #[tokio::test]
    async fn cancellation_waits_for_request_route_registration() {
        let routes = RelayRouteState::default();
        let upstream_request = RequestId::String("upstream-early-cancel".into());
        let downstream_request = RequestId::String("downstream-early-cancel".into());
        let upstream_progress = ProgressToken(rmcp::model::NumberOrString::String(
            "upstream-cancel-progress".into(),
        ));
        let params = CancelledNotificationParam::new(
            Some(upstream_request.clone()),
            Some("early cancel".to_string()),
        );

        assert!(
            routes
                .translate_or_queue_cancellation(params)
                .await
                .is_none()
        );
        let pending = routes
            .register_request(
                upstream_request,
                downstream_request.clone(),
                upstream_progress,
                None,
            )
            .await;

        let cancellation = pending.cancellation.expect("queued cancellation");
        assert_eq!(cancellation.request_id, Some(downstream_request));
        assert_eq!(cancellation.reason.as_deref(), Some("early cancel"));
    }

    #[tokio::test]
    async fn cancellation_handler_wait_tracks_delayed_inflight_forwarding() {
        let routes = Arc::new(RelayRouteState::default());
        let upstream_request = RequestId::String("upstream-cancel-wait".into());
        let downstream_request = RequestId::String("downstream-cancel-wait".into());
        let upstream_progress = ProgressToken(rmcp::model::NumberOrString::String(
            "upstream-cancel-wait-progress".into(),
        ));
        let previous = routes.cancellation_started(&upstream_request).await;
        routes
            .register_request(
                upstream_request.clone(),
                downstream_request,
                upstream_progress,
                None,
            )
            .await;

        let waiting_routes = Arc::clone(&routes);
        let waiting_request = upstream_request.clone();
        let waiter = tokio::spawn(async move {
            waiting_routes
                .wait_for_cancellation_handlers_after(
                    &waiting_request,
                    previous,
                    Duration::from_secs(1),
                )
                .await
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        let translated = routes
            .translate_or_queue_cancellation(CancelledNotificationParam::new(
                Some(upstream_request.clone()),
                Some("cancelled upstream".to_string()),
            ))
            .await
            .expect("active cancellation route");
        assert_eq!(translated.0, upstream_request);
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        routes.finish_cancellation_forward(&translated.0).await;
        assert!(waiter.await.expect("cancellation waiter"));
    }

    #[tokio::test]
    async fn pending_progress_queue_is_bounded_before_route_registration() {
        let routes = RelayRouteState::default();
        let upstream_progress = ProgressToken(rmcp::model::NumberOrString::String(
            "bounded-progress".into(),
        ));

        for _ in 0..(MAX_PENDING_PROGRESS_PER_ROUTE + 2) {
            let params = ProgressNotificationParam::new(upstream_progress.clone(), 0.5);
            assert!(routes.translate_or_queue_progress(params).await.is_none());
        }
        assert_eq!(
            routes
                .progress
                .lock()
                .await
                .pending
                .get(&upstream_progress)
                .map(Vec::len),
            Some(MAX_PENDING_PROGRESS_PER_ROUTE)
        );

        let distinct_routes = RelayRouteState::default();
        for index in 0..(MAX_PENDING_PROGRESS_ROUTES + 2) {
            let token = ProgressToken(rmcp::model::NumberOrString::String(
                format!("bounded-route-{index}").into(),
            ));
            let params = ProgressNotificationParam::new(token, 0.5);
            assert!(
                distinct_routes
                    .translate_or_queue_progress(params)
                    .await
                    .is_none()
            );
        }
        assert_eq!(
            distinct_routes.progress.lock().await.pending.len(),
            MAX_PENDING_PROGRESS_ROUTES
        );
    }

    #[tokio::test]
    async fn pending_cancellation_queue_is_bounded_before_route_registration() {
        let routes = RelayRouteState::default();
        for index in 0..(MAX_PENDING_REQUEST_CANCELLATIONS + 2) {
            let request_id = RequestId::String(format!("bounded-cancel-{index}").into());
            let params =
                CancelledNotificationParam::new(Some(request_id), Some("early cancel".to_string()));
            assert!(
                routes
                    .translate_or_queue_cancellation(params)
                    .await
                    .is_none()
            );
        }
        assert_eq!(
            routes.requests.lock().await.pending_cancellations.len(),
            MAX_PENDING_REQUEST_CANCELLATIONS
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
            relay_cache_key_for_capabilities(&config.name, 1, None, &capabilities),
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

    #[test]
    fn relay_header_recovery_future_stays_structurally_boxed() {
        let bytes = size_of::<RelayToolRequestFuture<'static>>();
        let max_boxed_future_bytes = 2 * size_of::<usize>();
        assert!(
            bytes <= max_boxed_future_bytes,
            "relay HeaderMismatch recovery future must remain a boxed trait-object pointer; got {bytes} bytes (expected at most {max_boxed_future_bytes})"
        );
    }

    #[tokio::test]
    async fn relayed_header_mismatch_refreshes_schema_and_retries_once() {
        #[derive(Clone)]
        struct HeaderMismatchUpstream {
            list_calls: Arc<AtomicUsize>,
            tool_calls: Arc<AtomicUsize>,
        }

        impl ServerHandler for HeaderMismatchUpstream {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }

            async fn list_tools(
                &self,
                _: Option<PaginatedRequestParams>,
                _: RequestContext<RoleServer>,
            ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
                self.list_calls.fetch_add(1, Ordering::SeqCst);
                Ok(rmcp::model::ListToolsResult::with_all_items(vec![
                    rmcp::model::Tool::new(
                        "header.tool",
                        "relay header recovery fixture",
                        Arc::new(serde_json::Map::new()),
                    ),
                ]))
            }

            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                let attempt = self.tool_calls.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    return Err(ErrorData::new(
                        ErrorCode::HEADER_MISMATCH,
                        "header mismatch: missing Mcp-Param-owner header for parameter \"owner\"",
                        None,
                    ));
                }
                Ok(CallToolResult::success(vec![ContentBlock::text("recovered")]).into())
            }
        }

        let list_calls = Arc::new(AtomicUsize::new(0));
        let tool_calls = Arc::new(AtomicUsize::new(0));
        let upstream = HeaderMismatchUpstream {
            list_calls: Arc::clone(&list_calls),
            tool_calls: Arc::clone(&tool_calls),
        };
        let capabilities = relay_test_capabilities();
        let (pool, config, downstream_server) =
            cached_relay_pool(upstream, CapableAgent, capabilities.clone()).await;

        let result = pool
            .call_tool_relayed(
                &config,
                None,
                CallToolRequestParams::new("header.tool"),
                downstream_server.peer().clone(),
                RequestId::String("header-recovery".into()),
                CancellationToken::new(),
                1,
                capabilities,
                None,
                crate::upstream::pool::TaskRouteAuthorization::root(),
            )
            .await
            .expect("cached relay exists")
            .expect("relay HeaderMismatch should self-heal");

        assert!(matches!(result, CallToolResponse::Complete(_)));
        assert_eq!(list_calls.load(Ordering::SeqCst), 1);
        assert_eq!(tool_calls.load(Ordering::SeqCst), 2);
        let metrics = pool.header_recovery_metrics(&config.name);
        assert_eq!(metrics.mismatch_detected, 1);
        assert_eq!(metrics.schema_refreshes, 1);
        assert_eq!(metrics.retry_successes, 1);
        assert_eq!(metrics.retry_failures, 0);
    }

    #[tokio::test]
    async fn relayed_header_refresh_never_extends_original_deadline() {
        #[derive(Clone)]
        struct SlowHeaderRefreshUpstream {
            list_calls: Arc<AtomicUsize>,
            tool_calls: Arc<AtomicUsize>,
        }

        impl ServerHandler for SlowHeaderRefreshUpstream {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }

            async fn list_tools(
                &self,
                _: Option<PaginatedRequestParams>,
                _: RequestContext<RoleServer>,
            ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
                self.list_calls.fetch_add(1, Ordering::SeqCst);
                // Far longer than the relay budget, so the ceiling below can be
                // generous without ever colliding with this fixture.
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(rmcp::model::ListToolsResult::with_all_items(vec![]))
            }

            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                self.tool_calls.fetch_add(1, Ordering::SeqCst);
                Err(ErrorData::new(
                    ErrorCode::HEADER_MISMATCH,
                    "header mismatch: missing Mcp-Param-owner header for parameter \"owner\"",
                    None,
                ))
            }
        }

        let list_calls = Arc::new(AtomicUsize::new(0));
        let tool_calls = Arc::new(AtomicUsize::new(0));
        let upstream = SlowHeaderRefreshUpstream {
            list_calls: Arc::clone(&list_calls),
            tool_calls: Arc::clone(&tool_calls),
        };
        let capabilities = relay_test_capabilities();
        let (pool, config, downstream_server) =
            cached_relay_pool(upstream, CapableAgent, capabilities.clone()).await;
        let pool = pool.with_relay_timeout(Duration::from_millis(80));
        let started = Instant::now();

        let result = pool
            .call_tool_relayed(
                &config,
                None,
                CallToolRequestParams::new("header.tool"),
                downstream_server.peer().clone(),
                RequestId::String("header-refresh-deadline".into()),
                CancellationToken::new(),
                1,
                capabilities,
                None,
                crate::upstream::pool::TaskRouteAuthorization::root(),
            )
            .await
            .expect("cached relay exists")
            .expect_err("slow schema refresh must obey the original relay deadline");

        assert!(matches!(
            result,
            super::super::CapabilityCallError::Timeout { .. }
        ));
        // The refresh fixture stalls for 30s, so this only has to prove the 80ms
        // relay deadline ended the call. A 500ms ceiling proved the same thing
        // but tripped on scheduler jitter under parallel test load.
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the original relay deadline must bound the call: {:?}",
            started.elapsed()
        );
        assert_eq!(list_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            tool_calls.load(Ordering::SeqCst),
            1,
            "deadline expiry must prevent a second tools/call"
        );
        let metrics = pool.header_recovery_metrics(&config.name);
        assert_eq!(metrics.mismatch_detected, 1);
        assert_eq!(metrics.schema_refreshes, 1);
        assert_eq!(metrics.retry_successes, 0);
        assert_eq!(metrics.retry_failures, 0);
    }

    async fn wait_for_recorded_count<T>(values: &Mutex<Vec<T>>, expected: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if values.lock().await.len() >= expected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
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
            tokio::time::sleep(Duration::from_millis(20)).await;
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
                tokio::time::sleep(Duration::from_millis(50)).await;
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
        let routes = {
            let connections = pool.relay_connections.read().await;
            let entry = connections
                .get(&relay_cache_key_for_capabilities(
                    &config.name,
                    1,
                    None,
                    &capabilities,
                ))
                .expect("cached relay connection");
            assert!(!entry.peer.is_transport_closed());
            Arc::clone(&entry.routes)
        };
        let watcher_sequence = routes.relay_watcher_finish_sequence.load(Ordering::Acquire);
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
            matches!(result, Err(ref error) if error.to_string().contains("upstream cancelled")),
            "upstream cancellation should terminate the relayed request: {result:?}"
        );
        tokio::time::timeout(
            RELAY_ROUTE_CLEANUP_GRACE + Duration::from_secs(1),
            routes.wait_for_relay_watcher_finish_after(watcher_sequence),
        )
        .await
        .expect("cancelled response retires its detached relay watcher after bounded grace");

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

        tokio::time::timeout(Duration::from_secs(2), async {
            while !upstream.started.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("upstream request should start");
        cancellation.cancel();
        let result = call.await.expect("relay task joins").expect("relay exists");
        assert!(
            matches!(result, Err(ref error) if error.to_string().contains("downstream request cancelled"))
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while !upstream.cancelled.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
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

        let message = result.expect_err("pre-cancelled relay must fail locally");
        assert_eq!(
            message.to_string(),
            downstream_cancelled("downstream request was already cancelled"),
            "pre-cancelled request must preserve the canonical cancellation error"
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
                .get(&relay_cache_key(&config.name, 1, None))
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

        // Dispatch is synchronous, so this should be near-instant; the ceiling
        // only distinguishes "dispatched now" from "waited for the upstream
        // request id", and a wide one does that without measuring the scheduler.
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "relay-token cancellation must be dispatched before waiting for the upstream request id: {:?}",
            started.elapsed()
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
            .insert(relay_cache_key(&config.name, session_id, None), entry);

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
            incarnation: None,
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
            .insert(relay_cache_key("up", 7, None), entry);
        assert_eq!(pool.relay_connections.read().await.len(), 1);

        pool.evict_all_relay_connections().await;
        assert!(pool.relay_connections.read().await.is_empty());
    }

    /// A capability change creates a distinct relay generation. If establishing
    /// that generation fails, the prior generation must remain alive because it
    /// may still be serving a long-running call or elicitation.
    #[tokio::test]
    async fn capability_change_does_not_drop_active_relay_generation() {
        let pool = UpstreamPool::new();
        let config = super::super::testsupport::test_upstream_config();
        let session_id = 7;
        let (entry, keepalive) = live_relay_cached_connection(Instant::now()).await;
        let original_key = relay_cache_key(&config.name, session_id, None);
        let original_peer = entry.peer.clone();
        pool.relay_connections
            .write()
            .await
            .insert(original_key.clone(), entry);

        let changed_capabilities = ClientCapabilities::builder().enable_tasks().build();
        let replacement = pool
            .acquire_or_connect_relay(
                &config,
                None,
                keepalive.peer().clone(),
                session_id,
                changed_capabilities,
            )
            .await;
        assert!(
            replacement.is_none(),
            "fixture config cannot open a replacement"
        );

        let cache = pool.relay_connections.read().await;
        assert!(
            cache.contains_key(&original_key),
            "a failed capability-generation replacement must not evict the active generation"
        );
        assert!(
            !original_peer.is_transport_closed(),
            "the original relay peer must remain usable by its in-flight call"
        );
    }

    async fn relay_test_downstream() -> RunningService<RoleServer, TrivialServer> {
        let (server_transport, agent_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        tokio::spawn(async move {
            if let Ok(running) = ().serve(agent_transport).await {
                running.waiting().await.ok();
            }
        });
        TrivialServer
            .serve(server_transport)
            .await
            .expect("downstream server connects")
    }

    async fn wait_for_usage_outcome(
        store: &crate::usage::UsageStore,
        outcome: &str,
        expected_count: i64,
    ) -> i64 {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let expected = outcome.to_string();
            let count = store
                .with_conn(move |conn| {
                    conn.query_row(
                        "SELECT COUNT(*) FROM upstream_calls WHERE outcome = ?1",
                        [expected],
                        |row| row.get(0),
                    )
                    .map_err(crate::usage::store::sqlite_error)
                })
                .await
                .expect("usage query succeeds");
            if count >= expected_count || Instant::now() >= deadline {
                return count;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn usage_items_for_outcome(
        store: &crate::usage::UsageStore,
        outcome: &str,
    ) -> Vec<String> {
        let expected = outcome.to_string();
        store
            .with_conn(move |conn| {
                let mut statement = conn
                    .prepare(
                        "SELECT tool_name FROM upstream_calls WHERE outcome = ?1 ORDER BY tool_name",
                    )
                    .map_err(crate::usage::store::sqlite_error)?;
                statement
                    .query_map([expected], |row| row.get(0))
                    .map_err(crate::usage::store::sqlite_error)?
                    .collect::<Result<Vec<String>, _>>()
                    .map_err(crate::usage::store::sqlite_error)
            })
            .await
            .expect("usage item query succeeds")
    }

    /// Cold relay establishment is part of the request's cancellation scope.
    /// Holding the per-key single-flight lock models a stalled concurrent
    /// connector without relying on a real network timeout.
    #[tokio::test]
    async fn cold_relay_connect_stops_on_downstream_cancellation() {
        let dir = tempfile::tempdir().expect("usage tempdir");
        let store = Arc::new(
            crate::usage::UsageStore::open(dir.path().join("usage.db"))
                .await
                .expect("usage store opens"),
        );
        let pool = UpstreamPool::new()
            .with_usage_store(Some(Arc::clone(&store)))
            .with_relay_timeout(Duration::from_secs(30));
        let config = super::super::testsupport::test_upstream_config();
        let capabilities = relay_test_capabilities();
        let key = relay_cache_key(&config.name, 41, None);
        let connect_lock = Arc::new(Mutex::new(()));
        pool.relay_connect_locks
            .write()
            .await
            .insert(key, Arc::clone(&connect_lock));
        let _held = connect_lock.lock().await;
        let downstream = relay_test_downstream().await;
        let cancellation = CancellationToken::new();
        let cancel = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            cancel.cancel();
        });

        let started = Instant::now();
        let result = pool
            .call_tool_relayed(
                &config,
                None,
                CallToolRequestParams::new("blocked-connect"),
                downstream.peer().clone(),
                RequestId::Number(1),
                cancellation,
                41,
                capabilities,
                None,
                crate::upstream::pool::TaskRouteAuthorization::root(),
            )
            .await
            .expect("cancellation produces a classified result")
            .expect_err("cold connect must cancel");
        assert!(matches!(
            result,
            super::super::capability_call::CapabilityCallError::Cancelled { .. }
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            wait_for_usage_outcome(&store, "connect_cancelled", 1).await,
            1
        );
    }

    /// The same absolute relay deadline covers cold connection establishment;
    /// a blocked connector does not receive an extra timeout before queue/RPC.
    #[test]
    fn cold_relay_connect_obeys_absolute_deadline() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            let timeout = Duration::from_millis(40);
            let dir = tempfile::tempdir().expect("usage tempdir");
            let store = Arc::new(
                crate::usage::UsageStore::open(dir.path().join("usage.db"))
                    .await
                    .expect("usage store opens"),
            );
            let pool = UpstreamPool::new()
                .with_usage_store(Some(Arc::clone(&store)))
                .with_relay_timeout(timeout);
            let config = super::super::testsupport::test_upstream_config();
            let capabilities = relay_test_capabilities();
            let key = relay_cache_key(&config.name, 42, None);
            let connect_lock = Arc::new(Mutex::new(()));
            pool.relay_connect_locks
                .write()
                .await
                .insert(key, Arc::clone(&connect_lock));
            let _held = connect_lock.lock().await;
            let downstream = relay_test_downstream().await;

            let started = Instant::now();
            let result = pool
                .call_tool_relayed(
                    &config,
                    None,
                    CallToolRequestParams::new("blocked-connect"),
                    downstream.peer().clone(),
                    RequestId::Number(2),
                    CancellationToken::new(),
                    42,
                    capabilities,
                    None,
                    crate::upstream::pool::TaskRouteAuthorization::root(),
                )
                .await
                .expect("deadline produces a classified result")
                .expect_err("cold connect must time out");
            assert!(matches!(
                result,
                super::super::capability_call::CapabilityCallError::Timeout { .. }
            ));
            assert!(
                started.elapsed() < timeout + Duration::from_secs(5),
                "cold connect exceeded the absolute relay budget"
            );
            assert_eq!(
                wait_for_usage_outcome(&store, "connect_timeout", 1).await,
                1
            );
        });
    }

    #[derive(Clone, Copy)]
    enum RelayTestCapability {
        Tools,
        Prompts,
        Resources,
    }

    async fn invoke_cold_relay_path(
        pool: &UpstreamPool,
        config: &UpstreamConfig,
        downstream: &Peer<RoleServer>,
        session_id: u64,
        cancellation: CancellationToken,
        capability: RelayTestCapability,
    ) {
        let request_id = RequestId::Number(session_id as i64);
        let capabilities = relay_test_capabilities();
        match capability {
            RelayTestCapability::Tools => {
                let _result = pool
                    .call_tool_relayed(
                        config,
                        None,
                        CallToolRequestParams::new("cold-tool"),
                        downstream.clone(),
                        request_id,
                        cancellation,
                        session_id,
                        capabilities,
                        None,
                        crate::upstream::pool::TaskRouteAuthorization::root(),
                    )
                    .await;
            }
            RelayTestCapability::Prompts => {
                let _result = pool
                    .get_prompt_relayed(
                        config,
                        None,
                        GetPromptRequestParams::new(format!("{}/cold-prompt", config.name)),
                        downstream.clone(),
                        request_id,
                        cancellation,
                        session_id,
                        capabilities,
                    )
                    .await;
            }
            RelayTestCapability::Resources => {
                let _result = pool
                    .read_resource_relayed(
                        config,
                        None,
                        ReadResourceRequestParams::new(format!(
                            "lab://upstream/{}/file:///cold-resource",
                            config.name
                        )),
                        downstream.clone(),
                        request_id,
                        cancellation,
                        session_id,
                        capabilities,
                    )
                    .await;
            }
        }
    }

    /// Exercise the real tool, prompt, and resource adapters instead of
    /// inspecting their source text. Each cold-connect exit must reach the
    /// shared request telemetry boundary with its classified outcome.
    #[tokio::test]
    async fn every_cold_relay_adapter_records_cancel_timeout_and_connect_error() {
        let dir = tempfile::tempdir().expect("usage tempdir");
        let store = Arc::new(
            crate::usage::UsageStore::open(dir.path().join("usage.db"))
                .await
                .expect("usage store opens"),
        );
        let config = super::super::testsupport::test_upstream_config();
        let downstream = relay_test_downstream().await;

        let capabilities = [
            RelayTestCapability::Tools,
            RelayTestCapability::Prompts,
            RelayTestCapability::Resources,
        ];
        for (index, capability) in capabilities.into_iter().enumerate() {
            let session_id = 100 + index as u64;
            let pool = UpstreamPool::new()
                .with_usage_store(Some(Arc::clone(&store)))
                .with_relay_timeout(Duration::from_secs(30));
            let key = relay_cache_key(&config.name, session_id, None);
            let connect_lock = Arc::new(Mutex::new(()));
            pool.relay_connect_locks
                .write()
                .await
                .insert(key, Arc::clone(&connect_lock));
            let _held = connect_lock.lock().await;
            let cancellation = CancellationToken::new();
            let cancel = cancellation.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                cancel.cancel();
            });
            invoke_cold_relay_path(
                &pool,
                &config,
                downstream.peer(),
                session_id,
                cancellation,
                capability,
            )
            .await;
        }
        assert_eq!(
            wait_for_usage_outcome(&store, "connect_cancelled", 3).await,
            3
        );
        let expected_items = vec![
            "cold-prompt".to_string(),
            "cold-tool".to_string(),
            "lab://upstream/test/file:///cold-resource".to_string(),
        ];
        assert_eq!(
            usage_items_for_outcome(&store, "connect_cancelled").await,
            expected_items
        );

        for (index, capability) in capabilities.into_iter().enumerate() {
            let session_id = 200 + index as u64;
            let pool = UpstreamPool::new()
                .with_usage_store(Some(Arc::clone(&store)))
                .with_relay_timeout(Duration::from_millis(20));
            let key = relay_cache_key(&config.name, session_id, None);
            let connect_lock = Arc::new(Mutex::new(()));
            pool.relay_connect_locks
                .write()
                .await
                .insert(key, Arc::clone(&connect_lock));
            let _held = connect_lock.lock().await;
            invoke_cold_relay_path(
                &pool,
                &config,
                downstream.peer(),
                session_id,
                CancellationToken::new(),
                capability,
            )
            .await;
        }
        assert_eq!(
            wait_for_usage_outcome(&store, "connect_timeout", 3).await,
            3
        );
        assert_eq!(
            usage_items_for_outcome(&store, "connect_timeout").await,
            expected_items
        );

        let pool = UpstreamPool::new().with_usage_store(Some(Arc::clone(&store)));
        for (index, capability) in capabilities.into_iter().enumerate() {
            invoke_cold_relay_path(
                &pool,
                &config,
                downstream.peer(),
                300 + index as u64,
                CancellationToken::new(),
                capability,
            )
            .await;
        }
        assert_eq!(wait_for_usage_outcome(&store, "connect_error", 3).await, 3);
        assert_eq!(
            usage_items_for_outcome(&store, "connect_error").await,
            expected_items
        );
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
            cache.insert(relay_cache_key("up", 1, None), a);
            cache.insert(relay_cache_key("up", 2, None), b);
        }

        pool.evict_relay_connection(&relay_cache_key("up", 1, None))
            .await;

        let remaining: Vec<_> = pool
            .relay_connections
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        assert_eq!(remaining, vec![relay_cache_key("up", 2, None)]);
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
            cache.insert(relay_cache_key("up", 1, Some("alice")), alice);
            cache.insert(relay_cache_key("up", 1, Some("bob")), bob);
        }
        assert_eq!(
            pool.relay_connections.read().await.len(),
            2,
            "two subjects in one session must not collide on the same key"
        );

        // Evicting alice's connection leaves bob's intact.
        pool.evict_relay_connection(&relay_cache_key("up", 1, Some("alice")))
            .await;
        let remaining: Vec<_> = pool
            .relay_connections
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            remaining,
            vec![relay_cache_key("up", 1, Some("bob"))],
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
            cache.insert(relay_cache_key("up", 1, Some("alice")), alice_a);
            cache.insert(relay_cache_key("up", 2, Some("alice")), alice_b);
            cache.insert(relay_cache_key("up", 3, Some("bob")), bob);
            cache.insert(relay_cache_key("up", 4, None), anonymous);
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
                relay_cache_key("up", 3, Some("bob")),
                relay_cache_key("up", 4, None),
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
            cache.insert(relay_cache_key("first", 1, Some("alice")), alice);
            cache.insert(relay_cache_key("second", 2, Some("bob")), bob);
            cache.insert(relay_cache_key("raw", 3, None), anonymous);
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
        assert!(remaining.contains(&relay_cache_key("second", 2, Some("bob"))));
        assert!(remaining.contains(&relay_cache_key("raw", 3, None)));
    }

    #[tokio::test]
    async fn subject_and_shared_oauth_invalidation_cannot_deadlock() {
        let pool = UpstreamPool::new();
        let subject_pool = pool.clone();
        let shared_pool = pool.clone();
        let shared_upstreams = ["first".to_string()];

        tokio::time::timeout(Duration::from_secs(1), async move {
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
            cache.insert(relay_cache_key("up", 1, None), stale);
            cache.insert(relay_cache_key("up", 2, None), fresh);
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
        assert_eq!(remaining, vec![relay_cache_key("up", 2, None)]);
    }

    /// The relay path uses its own `relay_timeout` (default 5 min), distinct
    /// from the 30s `request_timeout` — so a relayed call waiting on a human
    /// answering an elicitation is not aborted mid-dialog. Regression guard for
    /// the human-aware-deadline fix; `call_tool_relayed` reads `self.relay_timeout`.
    #[test]
    fn relay_timeout_defaults_to_five_minutes_and_is_configurable() {
        use Duration;
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
        pool.catalog_write().await.insert(
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
            incarnation: None,
        };
        pool.relay_connections.write().await.insert(
            relay_cache_key(&config.name, 1, None),
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
            matches!(
                result,
                Some(Err(
                    super::super::capability_call::CapabilityCallError::Mcp { .. }
                ))
            ),
            "a valid MCP rejection must preserve its typed application-error class"
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
                .contains_key(&relay_cache_key(&config.name, 1, None)),
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
        pool.catalog_write().await.insert(
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
            incarnation: None,
        };
        pool.relay_connections.write().await.insert(
            relay_cache_key(&config.name, 1, None),
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
            result.to_string().contains("too large"),
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
            .insert(relay_cache_key(&config.name, 1, Some("alice")), entry);

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
