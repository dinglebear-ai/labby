//! Focused regression tests for upstream notification subscription lifecycles.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rmcp::model::{
    ErrorCode, ErrorData, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProtocolVersion, Resource, ServerCapabilities, ServerInfo, SubscriptionFilter,
};
use rmcp::service::{RequestContext, ServiceError, SubscriptionContext};
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
    listen_failures_before_stable: usize,
    acceptance_delay: Duration,
    tools: Arc<tokio::sync::RwLock<Vec<rmcp::model::Tool>>>,
    tool_change: Arc<tokio::sync::Notify>,
}

impl SubscriptionServer {
    fn accepting() -> Self {
        Self {
            attempts: Arc::new(AtomicUsize::new(0)),
            failures_before_accept: 0,
            listen_failures_before_stable: 0,
            acceptance_delay: Duration::ZERO,
            tools: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            tool_change: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn fail_then_accept() -> Self {
        Self {
            listen_failures_before_stable: 1,
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

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![Resource::new(
            NATIVE_RESOURCE_URI,
            "subscription-resource",
        )]))
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

    // Serial execution would take 1200ms, so any ceiling below that proves the
    // two acknowledgements overlapped. Sitting just under it leaves the most
    // room for scheduler jitter while keeping the proof exact.
    assert!(
        started.elapsed() < Duration::from_millis(1_150),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resource_listing_does_not_wait_for_subscription_rehandshake() {
    let pool = UpstreamPool::new();
    add_subscription_server(
        &pool,
        "leaf",
        SubscriptionServer::delayed(Duration::from_millis(600)),
    )
    .await;

    let started = Instant::now();
    let resources = pool.list_upstream_resources().await;

    assert_eq!(
        resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>(),
        vec!["lab://upstream/leaf/file:///tmp/subscription-resource"]
    );
    // The handshake this must not wait for takes 600ms, so a ceiling below that
    // is the whole proof; 200ms was near enough to trip on scheduler jitter.
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "resources/list waited for a subscription handshake: {:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_background_subscription_refreshes_are_coalesced() {
    let pool = UpstreamPool::new();
    let server = SubscriptionServer::delayed(Duration::from_millis(100));
    let attempts = Arc::clone(&server.attempts);
    add_subscription_server(&pool, "leaf", server).await;

    pool.schedule_upstream_subscription_refreshes(vec!["leaf".to_string()])
        .await;
    pool.schedule_upstream_subscription_refreshes(vec!["leaf".to_string()])
        .await;

    tokio::time::timeout(Duration::from_secs(1), async {
        while attempts.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("subscription refresh starts");
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn initial_failure_is_retried_and_eventually_published() {
    let pool = UpstreamPool::new();
    let server = SubscriptionServer::fail_then_accept();
    let attempts = Arc::clone(&server.attempts);
    add_subscription_server(&pool, "leaf", server).await;

    pool.refresh_upstream_subscription("leaf").await;

    tokio::time::timeout(Duration::from_secs(5), async {
        while attempts.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("temporary stream failure is retried before the deadline");
    let expected = UpstreamPool::gateway_resource_uri("leaf", NATIVE_RESOURCE_URI);
    assert!(
        pool.subscribable_resource_uris_snapshot()
            .contains(&expected)
    );
}

#[test]
fn subscription_listen_is_only_attempted_for_modern_protocol() {
    assert!(
        super::notifications::subscription_listen_supported_protocol(
            &ProtocolVersion::V_2026_07_28
        )
    );
    assert!(
        !super::notifications::subscription_listen_supported_protocol(
            &ProtocolVersion::V_2025_11_25
        )
    );
}

#[test]
fn terminal_subscription_errors_stop_unchanged_retries() {
    let method_not_found = ServiceError::McpError(ErrorData::new(
        ErrorCode::METHOD_NOT_FOUND,
        "Method not found",
        None,
    ));
    assert!(super::notifications::terminal_subscription_listen_error(
        &method_not_found,
        0
    ));

    let invalid_params = ServiceError::McpError(ErrorData::new(
        ErrorCode::INVALID_PARAMS,
        "Invalid request parameters",
        None,
    ));
    assert!(!super::notifications::terminal_subscription_listen_error(
        &invalid_params,
        0
    ));
    assert!(super::notifications::terminal_subscription_listen_error(
        &invalid_params,
        1
    ));

    let subscription_limit = ServiceError::McpError(ErrorData::new(
        ErrorCode::INTERNAL_ERROR,
        "Subscription limit reached",
        None,
    ));
    assert!(super::notifications::terminal_subscription_listen_error(
        &subscription_limit,
        0
    ));

    assert!(!super::notifications::terminal_subscription_listen_error(
        &ServiceError::TransportClosed,
        9
    ));
}

#[test]
fn subscription_retry_delay_is_bounded_exponential_and_dephased() {
    let alpha = super::notifications::subscription_retry_delay("alpha", 0);
    let bravo = super::notifications::subscription_retry_delay("bravo", 0);
    assert_ne!(alpha, bravo, "upstream identity must dephase retries");
    assert!(super::notifications::subscription_retry_delay("alpha", 8) <= Duration::from_secs(72));
    assert!(super::notifications::subscription_retry_delay("alpha", 8) >= Duration::from_secs(48));
    assert!(super::notifications::subscription_retry_delay("alpha", 2) > alpha);
}

#[test]
fn subscription_backoff_resets_only_after_stable_interval() {
    assert_eq!(
        super::notifications::next_subscription_retry_attempt(4, Duration::from_secs(29)),
        5
    );
    assert_eq!(
        super::notifications::next_subscription_retry_attempt(4, Duration::from_secs(30)),
        0
    );
}

#[tokio::test(start_paused = true)]
async fn failed_subscription_reconnect_is_cancelled_during_backoff() {
    let pool = UpstreamPool::new();
    let mut server = SubscriptionServer::accepting();
    server.listen_failures_before_stable = usize::MAX;
    let attempts = Arc::clone(&server.attempts);
    add_subscription_server(&pool, "leaf", server).await;

    pool.refresh_upstream_subscription("leaf").await;
    tokio::task::yield_now().await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    pool.cancel_all_upstream_subscriptions().await;
    tokio::time::advance(Duration::from_mins(2)).await;
    tokio::task::yield_now().await;

    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn failed_subscription_reconnects_after_its_jittered_deadline() {
    let pool = UpstreamPool::new();
    let server = SubscriptionServer::fail_then_accept();
    let attempts = Arc::clone(&server.attempts);
    add_subscription_server(&pool, "leaf", server).await;

    pool.refresh_upstream_subscription("leaf").await;
    tokio::task::yield_now().await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    let delay = super::notifications::subscription_retry_delay("leaf", 1);
    tokio::time::advance(delay.saturating_sub(Duration::from_millis(1))).await;
    tokio::task::yield_now().await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);

    tokio::time::advance(Duration::from_millis(1)).await;
    for _ in 0..20 {
        if attempts.load(Ordering::SeqCst) >= 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let expected = UpstreamPool::gateway_resource_uri("leaf", NATIVE_RESOURCE_URI);
    assert!(
        pool.subscribable_resource_uris_snapshot()
            .contains(&expected)
    );
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
