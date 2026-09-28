//! Shared fixtures for upstream notification behavior.
//!
//! This lives behind `testkit` rather than `cfg(test)` because the product's
//! notification consumer is in the `labby` crate. The regression this module
//! exists to support — one upstream's slow re-list must not stall another
//! upstream's events — can only be proven by driving that real consumer
//! against an upstream whose listings block on demand.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use rmcp::model::{
    ErrorData, ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    Resource, ServerCapabilities, ServerInfo, SubscriptionFilter,
};
use rmcp::service::{RequestContext, SubscriptionContext};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, RoleClient, RoleServer, ServerHandler, ServiceExt,
};

use super::super::types::UpstreamRuntimeMetadata;
use super::entries::healthy_in_process_entry;
use super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
use super::{UpstreamConnection, UpstreamPool};

pub const NATIVE_RESOURCE_URI: &str = "file:///tmp/subscription-resource";

/// An in-process upstream whose catalog listings can be blocked, failed, and
/// counted, and which can emit real `list_changed` notifications over a
/// `subscriptions/listen` stream.
#[derive(Clone)]
pub struct SubscriptionServer {
    /// Number of `subscriptions/listen` acceptance attempts observed.
    pub(super) attempts: Arc<AtomicUsize>,
    listening: Arc<AtomicBool>,
    failures_before_accept: usize,
    /// `listen` fails while `attempts` is at or below this.
    pub(super) listen_failures_before_stable: usize,
    acceptance_delay: Duration,
    tools: Arc<tokio::sync::RwLock<Vec<rmcp::model::Tool>>>,
    tool_change: Arc<tokio::sync::Notify>,
    resources: Arc<tokio::sync::RwLock<Vec<Resource>>>,
    resource_change: Arc<tokio::sync::Notify>,
    /// `resources/list` fails while this is set.
    pub fail_list_resources: Arc<AtomicBool>,
    /// `tools/list` fails while this is set.
    pub fail_list_tools: Arc<AtomicBool>,
    /// `resources/list` blocks while this is `false`, which lets a test hold
    /// one upstream's re-list open for as long as it needs.
    pub resource_list_gate: Arc<tokio::sync::watch::Sender<bool>>,
    /// Number of `resources/list` requests the fixture has begun serving.
    pub resource_list_calls: Arc<AtomicUsize>,
    /// Number of `tools/list` requests the fixture has begun serving.
    pub tool_list_calls: Arc<AtomicUsize>,
}

impl SubscriptionServer {
    #[must_use]
    pub fn accepting() -> Self {
        Self {
            attempts: Arc::new(AtomicUsize::new(0)),
            listening: Arc::new(AtomicBool::new(false)),
            failures_before_accept: 0,
            listen_failures_before_stable: 0,
            acceptance_delay: Duration::ZERO,
            tools: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            tool_change: Arc::new(tokio::sync::Notify::new()),
            resources: Arc::new(tokio::sync::RwLock::new(vec![Resource::new(
                NATIVE_RESOURCE_URI,
                "subscription-resource",
            )])),
            resource_change: Arc::new(tokio::sync::Notify::new()),
            fail_list_resources: Arc::new(AtomicBool::new(false)),
            fail_list_tools: Arc::new(AtomicBool::new(false)),
            resource_list_gate: Arc::new(tokio::sync::watch::Sender::new(true)),
            resource_list_calls: Arc::new(AtomicUsize::new(0)),
            tool_list_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[must_use]
    pub(super) fn fail_then_accept() -> Self {
        Self {
            listen_failures_before_stable: 1,
            ..Self::accepting()
        }
    }

    #[must_use]
    pub(super) fn delayed(delay: Duration) -> Self {
        Self {
            acceptance_delay: delay,
            ..Self::accepting()
        }
    }

    /// Close the `resources/list` gate so the next re-list blocks until
    /// [`Self::open_resource_list_gate`].
    pub fn close_resource_list_gate(&self) {
        self.resource_list_gate.send_replace(false);
    }

    pub fn open_resource_list_gate(&self) {
        self.resource_list_gate.send_replace(true);
    }

    pub async fn replace_tools_and_notify(&self, names: &[&str]) {
        *self.tools.write().await = names
            .iter()
            .map(|name| super::testsupport::test_tool(name))
            .collect();
        self.tool_change.notify_one();
    }

    pub async fn replace_resources_and_notify(&self, uris: &[&str]) {
        *self.resources.write().await = uris
            .iter()
            .map(|uri| Resource::new((*uri).to_string(), (*uri).to_string()))
            .collect();
        self.resource_change.notify_one();
    }

    /// Wait for the fixture's `subscriptions/listen` handler to be active. The
    /// handshake crosses a duplex transport, so a yield count is not a bound;
    /// a wall-clock deadline is.
    pub async fn wait_until_listening(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self.listening.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("subscription listener must be active before emitting list-changed");
    }
}

impl ServerHandler for SubscriptionServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .enable_resources()
                .enable_resources_list_changed()
                .enable_resources_subscribe()
                .build(),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.tool_list_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_list_tools.load(Ordering::SeqCst) {
            return Err(ErrorData::internal_error("fixture tool failure", None));
        }
        Ok(ListToolsResult::with_all_items(
            self.tools.read().await.clone(),
        ))
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.resource_list_calls.fetch_add(1, Ordering::SeqCst);
        let mut gate = self.resource_list_gate.subscribe();
        gate.wait_for(|open| *open)
            .await
            .map_err(|_| ErrorData::internal_error("fixture gate dropped", None))?;
        if self.fail_list_resources.load(Ordering::SeqCst) {
            return Err(ErrorData::internal_error("fixture listing failure", None));
        }
        Ok(ListResourcesResult::with_all_items(
            self.resources.read().await.clone(),
        ))
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if !self.acceptance_delay.is_zero() {
            std::thread::sleep(self.acceptance_delay);
        }
        if attempt < self.failures_before_accept {
            return None;
        }
        Some(requested.supported_by(&self.get_info().capabilities))
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        if self.attempts.load(Ordering::SeqCst) <= self.listen_failures_before_stable {
            return Err(ErrorData::internal_error(
                "temporary subscription stream failure",
                None,
            ));
        }
        self.listening.store(true, Ordering::SeqCst);
        loop {
            tokio::select! {
                () = context.cancelled() => return Ok(()),
                () = self.tool_change.notified() => {
                    context
                        .sink()
                        .notify_tool_list_changed()
                        .await
                        .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                }
                () = self.resource_change.notified() => {
                    context
                        .sink()
                        .notify_resource_list_changed()
                        .await
                        .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                }
            }
        }
    }
}

/// Bind `server` into `pool` as a healthy, connected upstream named `upstream`.
pub async fn add_subscription_server(
    pool: &UpstreamPool,
    upstream: &str,
    server: SubscriptionServer,
) {
    let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
    let server_task = tokio::spawn(async move {
        let running = server
            .serve(server_transport)
            .await
            .expect("subscription server starts");
        running.waiting().await.expect("subscription server runs");
    });
    let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
        .serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("subscription client starts");
    let peer = client_service.peer().clone();

    let mut entry = healthy_in_process_entry(Arc::from(upstream), HashMap::new());
    entry.resource_uris = vec![NATIVE_RESOURCE_URI.to_string()];
    assert!(
        pool.install_connection_catalog_entry(
            upstream.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
            entry,
        )
        .await
        .expect("bind subscription test connection")
        .is_none()
    );
    pool.resource_upstreams
        .write()
        .await
        .push(upstream.to_string());
}
