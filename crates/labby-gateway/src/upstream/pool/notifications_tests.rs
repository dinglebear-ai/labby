//! Focused regression tests for upstream notification subscription lifecycles.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rmcp::model::{
    ErrorData, ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities,
    ServerInfo, SubscriptionFilter,
};
use rmcp::service::{RequestContext, SubscriptionContext};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, RoleClient, RoleServer, ServerHandler, ServiceExt,
};

use super::super::types::UpstreamRuntimeMetadata;
use super::entries::healthy_in_process_entry;
use super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
use super::{UpstreamConnection, UpstreamPool};

const NATIVE_RESOURCE_URI: &str = "file:///tmp/subscription-resource";

#[derive(Clone)]
struct SubscriptionServer {
    attempts: Arc<AtomicUsize>,
    failures_before_accept: usize,
    acceptance_delay: Duration,
    tools: Arc<tokio::sync::RwLock<Vec<rmcp::model::Tool>>>,
    tool_change: Arc<tokio::sync::Notify>,
}

impl SubscriptionServer {
    fn accepting() -> Self {
        Self {
            attempts: Arc::new(AtomicUsize::new(0)),
            failures_before_accept: 0,
            acceptance_delay: Duration::ZERO,
            tools: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            tool_change: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn fail_then_accept() -> Self {
        Self {
            failures_before_accept: 1,
            ..Self::accepting()
        }
    }

    fn delayed(delay: Duration) -> Self {
        Self {
            acceptance_delay: delay,
            ..Self::accepting()
        }
    }

    async fn replace_tools_and_notify(&self, names: &[&str]) {
        *self.tools.write().await = names
            .iter()
            .map(|name| super::testsupport::test_tool(name))
            .collect();
        self.tool_change.notify_one();
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
        Ok(ListToolsResult::with_all_items(
            self.tools.read().await.clone(),
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
        (attempt >= self.failures_before_accept)
            .then(|| requested.supported_by(&self.get_info().capabilities))
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
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
            }
        }
    }
}

async fn add_subscription_server(pool: &UpstreamPool, upstream: &str, server: SubscriptionServer) {
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
    pool.catalog
        .write()
        .await
        .insert(upstream.to_string(), entry);
    pool.connections.write().await.insert(
        upstream.to_string(),
        UpstreamConnection {
            _client_service: client_service.into(),
            _server_task: Some(server_task),
            peer,
            runtime: UpstreamRuntimeMetadata::default(),
        },
    );
}

#[tokio::test]
async fn refresh_returns_after_acknowledgement_is_visible() {
    let pool = UpstreamPool::new();
    add_subscription_server(&pool, "leaf", SubscriptionServer::accepting()).await;

    pool.refresh_upstream_subscription("leaf").await;

    let expected = UpstreamPool::gateway_resource_uri("leaf", NATIVE_RESOURCE_URI);
    assert!(
        pool.subscribable_resource_uris_snapshot()
            .contains(&expected)
    );
}

#[tokio::test]
async fn older_generation_cannot_publish_or_clear_newer_acknowledgement() {
    let pool = UpstreamPool::new();
    let older = pool.begin_subscription_generation("leaf").await;
    let newer = pool.begin_subscription_generation("leaf").await;
    let accepted = SubscriptionFilter::builder()
        .resource_subscriptions(vec![NATIVE_RESOURCE_URI.to_string()])
        .build();

    assert!(
        !pool
            .record_subscription_resources_if_current("leaf", &older, &accepted)
            .await
    );
    assert!(
        pool.record_subscription_resources_if_current("leaf", &newer, &accepted)
            .await
    );
    assert!(
        !pool
            .clear_subscription_resources_if_current("leaf", &older)
            .await
    );

    let expected = UpstreamPool::gateway_resource_uri("leaf", NATIVE_RESOURCE_URI);
    assert!(
        pool.subscribable_resource_uris_snapshot()
            .contains(&expected)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batch_refresh_waits_for_acknowledgements_concurrently() {
    let pool = UpstreamPool::new();
    let delay = Duration::from_millis(600);
    add_subscription_server(&pool, "alpha", SubscriptionServer::delayed(delay)).await;
    add_subscription_server(&pool, "bravo", SubscriptionServer::delayed(delay)).await;

    let started = Instant::now();
    pool.refresh_upstream_subscriptions_concurrently(vec!["alpha".into(), "bravo".into()])
        .await;

    assert!(
        started.elapsed() < Duration::from_millis(1_050),
        "two 600 ms acknowledgements should overlap, elapsed {:?}",
        started.elapsed()
    );
    let snapshot = pool.subscribable_resource_uris_snapshot();
    assert!(snapshot.contains(&UpstreamPool::gateway_resource_uri(
        "alpha",
        NATIVE_RESOURCE_URI
    )));
    assert!(snapshot.contains(&UpstreamPool::gateway_resource_uri(
        "bravo",
        NATIVE_RESOURCE_URI
    )));
}

#[tokio::test]
async fn initial_failure_is_retried_and_eventually_published() {
    let pool = UpstreamPool::new();
    let server = SubscriptionServer::fail_then_accept();
    let attempts = Arc::clone(&server.attempts);
    add_subscription_server(&pool, "leaf", server).await;

    pool.refresh_upstream_subscription("leaf").await;

    let expected = UpstreamPool::gateway_resource_uri("leaf", NATIVE_RESOURCE_URI);
    tokio::time::timeout(Duration::from_secs(4), async {
        while !pool
            .subscribable_resource_uris_snapshot()
            .contains(&expected)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("retry publishes the accepted resource before the deadline");
    assert!(attempts.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn tool_change_consumer_refreshes_the_exact_named_catalog() {
    let pool = UpstreamPool::new();
    let server = SubscriptionServer::accepting();
    add_subscription_server(&pool, "leaf", server.clone()).await;
    pool.refresh_upstream_subscription("leaf").await;
    let mut notifications = pool.subscribe_notifications();

    server
        .replace_tools_and_notify(&["added_after_list_changed"])
        .await;

    let event = tokio::time::timeout(Duration::from_secs(2), notifications.recv())
        .await
        .expect("tool-list event arrives")
        .expect("notification channel stays open");
    assert!(
        matches!(
            &event,
            super::UpstreamNotificationEvent::ToolListChanged { .. }
        ),
        "expected a tool-list event, got {event:?}"
    );
    let super::UpstreamNotificationEvent::ToolListChanged { upstream } = event else {
        return;
    };
    assert!(pool.refresh_tools_after_list_changed(&upstream).await);
    let tool_names = pool
        .healthy_tools_for_upstream("leaf")
        .await
        .into_iter()
        .map(|tool| tool.tool.name.to_string())
        .collect::<Vec<_>>();
    assert_eq!(tool_names, ["added_after_list_changed"]);
}
