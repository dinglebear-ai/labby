//! Native HTTP regression coverage for cold resource discovery and recovery.

use super::*;
use rmcp::model::{ClientInfo, ProtocolVersion};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RunningService};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

mod waiting;

const DOCUMENTS: [(&str, &str); 2] = [
    (
        "task-contract",
        "# QA VM task contract\nUse managed commands.\n",
    ),
    ("skill", "# QA VM skill\nReserve, test, release.\n"),
];

#[derive(Default)]
struct ProbeCounts {
    active: AtomicUsize,
    peak: AtomicUsize,
}

struct ActiveProbe(Arc<ProbeCounts>);
impl Drop for ActiveProbe {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Default)]
struct QaUpstream {
    probes: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
    gate: Option<Arc<tokio::sync::Semaphore>>,
    entered: Arc<tokio::sync::Notify>,
    counts: Arc<ProbeCounts>,
}

impl ServerHandler for QaUpstream {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2025_11_25)
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        let active = self.counts.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.counts.peak.fetch_max(active, Ordering::SeqCst);
        let _probe = ActiveProbe(Arc::clone(&self.counts));
        self.entered.notify_one();
        if let Some(gate) = &self.gate {
            gate.acquire().await.expect("open fixture gate").forget();
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(ErrorData::internal_error(
                "deliberate connection failure",
                None,
            ));
        }
        Ok(rmcp::model::ListToolsResult::with_all_items(vec![]))
    }

    async fn list_resources(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(
            DOCUMENTS
                .iter()
                .map(|(name, _)| Resource::new(format!("qa-vm-service://{name}"), *name))
                .collect(),
        ))
    }

    async fn list_resource_templates(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new("qa-vm-service://{name}", "document"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let (_, text) = DOCUMENTS
            .iter()
            .find(|(name, _)| request.uri == format!("qa-vm-service://{name}"))
            .expect("listed document requested");
        // Deliberately emulate the old provider's missing discriminator.
        let legacy: ReadResourceResult = serde_json::from_value(json!({"contents": [{
            "uri": request.uri, "mimeType": "text/markdown", "text": text
        }]}))
        .unwrap();
        Ok(legacy.into())
    }
}

struct HttpFixture {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve_http<S: ServerHandler + Send + Sync + 'static>(
    factory: impl Fn() -> S + Send + Sync + 'static,
    reader: bool,
) -> HttpFixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let service = StreamableHttpService::new(
        move || Ok(factory()),
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_allowed_hosts(vec![address.to_string()])
            .with_json_response(true),
    );
    let mut router = axum::Router::new().nest_service("/mcp", service);
    if reader {
        // Inject only a read-scoped identity at the HTTP boundary, as auth middleware does.
        router = router.layer(axum::Extension(labby_auth::auth_context::AuthContext {
            sub: "resource-regression-reader".into(),
            actor_key: None,
            scopes: vec!["lab:read".into()],
            issuer: "https://fixture.invalid".into(),
            via_session: true,
            csrf_token: None,
            email: None,
        }));
    }
    HttpFixture {
        url: format!("http://{address}/mcp"),
        task: tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }),
    }
}

async fn serve_upstream(server: QaUpstream) -> HttpFixture {
    serve_http(move || server.clone(), false).await
}

async fn serve_gateway(server: LabMcpServer) -> HttpFixture {
    // Match production's per-request handlers and isolated relay identities.
    serve_http(
        move || LabMcpServer {
            installation_id: server.installation_id.clone(),
            registry: Arc::clone(&server.registry),
            access_runtime: Arc::clone(&server.access_runtime),
            file_stash_runtime: Arc::clone(&server.file_stash_runtime),
            gateway_manager: server.gateway_manager.clone(),
            peers: Arc::clone(&server.peers),
            code_mode_app_state: server.code_mode_app_state.clone(),
            last_listed_tool_contract: Default::default(),
            route_runtime: Arc::clone(&server.route_runtime),
            client_registry: server.client_registry.clone(),
            transport_label: "http",
            logging_level: Arc::clone(&server.logging_level),
            route_scope: server.route_scope.clone(),
            relay_session_id: crate::mcp::server::next_relay_session_id(),
            code_mode_widget_callbacks_enabled_for_test: false,
        },
        true,
    )
    .await
}

async fn gateway(
    configs: Vec<crate::config::UpstreamConfig>,
    budget_ms: u64,
    concurrency: usize,
) -> (LabMcpServer, Arc<UpstreamPool>) {
    let pool = Arc::new(UpstreamPool::new());
    pool.seed_lazy_upstreams(&configs).await;
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager("config.toml".into(), runtime),
    );
    let mut config = crate::config::LabConfig::default();
    config.gateway.mcp_list_warm_timeout_ms = Some(budget_ms);
    config.gateway.upstream_discovery_concurrency = Some(concurrency);
    config.upstream = configs;
    manager
        .seed_config_unchecked_for_tests(config.to_gateway_config())
        .await;
    let mut server = code_mode_server().await;
    server.gateway_manager = Some(manager);
    (server, pool)
}

fn upstream(name: &str, http: &HttpFixture) -> crate::config::UpstreamConfig {
    serde_json::from_value(json!({"name": name, "url": http.url, "proxy_prompts": false})).unwrap()
}

#[derive(Clone)]
struct ResourceClient(ProtocolVersion);
impl rmcp::ClientHandler for ResourceClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default().with_protocol_version(self.0.clone())
    }
}

async fn client(http: &HttpFixture, modern: bool) -> RunningService<RoleClient, ResourceClient> {
    let version = if modern {
        ProtocolVersion::V_2026_07_28
    } else {
        ProtocolVersion::V_2025_11_25
    };
    let lifecycle = if modern {
        ClientLifecycleMode::Discover {
            preferred_versions: vec![version.clone()],
        }
    } else {
        ClientLifecycleMode::Initialize
    };
    let worker = StreamableHttpClientWorker::new(
        reqwest::Client::new(),
        StreamableHttpClientTransportConfig::with_uri(http.url.clone()),
    );
    tokio::time::timeout(
        Duration::from_secs(5),
        ResourceClient(version).serve_with_lifecycle(worker, lifecycle),
    )
    .await
    .unwrap()
    .unwrap()
}

async fn assert_documents(client: &RunningService<RoleClient, ResourceClient>, modern: bool) {
    let listed = client.peer().list_resources(None).await.unwrap();
    let mut uris: Vec<_> = listed
        .resources
        .iter()
        .filter(|r| r.uri.starts_with("lab://upstream/qa/"))
        .map(|r| r.uri.as_str())
        .collect();
    uris.sort_unstable();
    assert_eq!(
        uris,
        [
            "lab://upstream/qa/qa-vm-service://skill",
            "lab://upstream/qa/qa-vm-service://task-contract"
        ]
    );
    for (name, text) in DOCUMENTS {
        let read = client
            .peer()
            .read_resource(ReadResourceRequestParams::new(format!(
                "lab://upstream/qa/qa-vm-service://{name}"
            )))
            .await
            .expect("native SDK must accept resource response");
        let wire = serde_json::to_value(read).unwrap();
        assert_eq!(wire["contents"].as_array().unwrap().len(), 1);
        assert_eq!(wire["contents"][0]["text"], text);
        assert_eq!(wire["contents"][0]["mimeType"], "text/markdown");
        assert_eq!(
            wire.get("resultType").and_then(Value::as_str),
            modern.then_some("complete")
        );
    }
}

#[tokio::test]
async fn native_http_cold_list_then_read_modern_and_legacy() {
    for modern in [true, false] {
        let fixture = QaUpstream::default();
        let provider = serve_upstream(fixture.clone()).await;
        let (server, pool) = gateway(vec![upstream("qa", &provider)], 2000, 2).await;
        let http = serve_gateway(server).await;
        let client = client(&http, modern).await;
        assert_eq!(
            pool.connection_count_for_tests().await,
            0,
            "handshake must leave discovery cold"
        );
        assert_documents(&client, modern).await;
        assert_eq!(
            fixture.probes.load(Ordering::SeqCst),
            3,
            "one discovery plus two isolated read relays"
        );
        client.cancel().await.unwrap();
    }
}

#[tokio::test]
async fn native_http_concurrent_lists_templates_and_reads_reuse_regular_connection() {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let fixture = QaUpstream {
        gate: Some(Arc::clone(&gate)),
        ..Default::default()
    };
    let provider = serve_upstream(fixture.clone()).await;
    let (server, pool) = gateway(vec![upstream("qa", &provider)], 5000, 2).await;
    let http = serve_gateway(server).await;
    let clients = futures::future::join_all((0..4).map(|_| client(&http, true))).await;
    {
        let calls = async {
            futures::future::join_all(clients.iter().map(|client| async {
                let ((), templates) = tokio::join!(
                    assert_documents(client, true),
                    client.peer().list_resource_templates(None)
                );
                assert!(
                    templates
                        .unwrap()
                        .resource_templates
                        .iter()
                        .any(|t| t.uri_template == "lab://upstream/qa/qa-vm-service://{name}")
                );
            }))
            .await;
        };
        tokio::pin!(calls);
        tokio::select! {
            () = &mut calls => panic!("requests must wait for cold upstream"),
            () = fixture.entered.notified() => {}
        }
        gate.add_permits(16);
        tokio::time::timeout(Duration::from_secs(5), calls)
            .await
            .unwrap();
    }
    // One regular discovery plus eight request-isolated read relays.
    assert_eq!(fixture.probes.load(Ordering::SeqCst), 9);
    assert_eq!(pool.connection_count_for_tests().await, 1);
    for client in clients {
        client.cancel().await.unwrap();
    }
}

#[tokio::test]
async fn mixed_upstreams_share_one_budget_and_preserve_healthy_resources() {
    let counts = Arc::new(ProbeCounts::default());
    let healthy = QaUpstream {
        counts: Arc::clone(&counts),
        ..Default::default()
    };
    let failing = QaUpstream {
        counts: Arc::clone(&counts),
        ..Default::default()
    };
    failing.fail.store(true, Ordering::SeqCst);
    let stalled = QaUpstream {
        counts: Arc::clone(&counts),
        gate: Some(Arc::new(tokio::sync::Semaphore::new(0))),
        ..Default::default()
    };
    let good_http = serve_upstream(healthy.clone()).await;
    let bad_http = serve_upstream(failing.clone()).await;
    let slow_http = serve_upstream(stalled.clone()).await;
    let configs = vec![
        upstream("good", &good_http),
        upstream("bad", &bad_http),
        upstream("slow-a", &slow_http),
        upstream("slow-b", &slow_http),
        upstream("queued", &slow_http),
    ];
    let (server, pool) = gateway(configs, 200, 2).await;
    let started = tokio::time::Instant::now();
    tokio::time::timeout(
        Duration::from_millis(600),
        server.ensure_resource_upstreams_ready(&pool),
    )
    .await
    .expect("mixed discovery must finish within one overall budget");
    assert!(
        started.elapsed() < Duration::from_millis(600),
        "one budget, not one per batch"
    );
    assert_eq!(healthy.probes.load(Ordering::SeqCst), 1);
    assert_eq!(failing.probes.load(Ordering::SeqCst), 1);
    assert_eq!(
        stalled.probes.load(Ordering::SeqCst),
        2,
        "queued upstream must not start after deadline"
    );
    assert_eq!(counts.peak.load(Ordering::SeqCst), 2);
    let resources = pool.list_upstream_resources().await;
    assert_eq!(resources.len(), 2);
    assert!(
        resources
            .iter()
            .all(|r| r.uri.starts_with("lab://upstream/good/"))
    );
    assert!(matches!(
        pool.upstream_tool_health("good").await,
        Some(crate::dispatch::upstream::types::UpstreamHealth::Healthy)
    ));
}

#[tokio::test]
async fn cooldown_expiry_allows_fresh_native_listing_to_recover() {
    let fixture = QaUpstream::default();
    fixture.fail.store(true, Ordering::SeqCst);
    let provider = serve_upstream(fixture.clone()).await;
    let (server, pool) = gateway(vec![upstream("qa", &provider)], 2000, 2).await;
    for _ in 0..3 {
        server.ensure_resource_upstreams_ready(&pool).await;
    }
    fixture.fail.store(false, Ordering::SeqCst);
    server.ensure_resource_upstreams_ready(&pool).await;
    assert_eq!(
        fixture.probes.load(Ordering::SeqCst),
        3,
        "cooldown must suppress retries"
    );
    assert!(!pool.should_reprobe("qa").await);
    assert!(pool.expire_tool_cooldown_for_tests("qa").await);
    assert!(pool.should_reprobe("qa").await);
    let http = serve_gateway(server).await;
    let client = client(&http, true).await;
    assert_documents(&client, true).await;
    assert_eq!(fixture.probes.load(Ordering::SeqCst), 6);
    assert!(matches!(
        pool.upstream_tool_health("qa").await,
        Some(crate::dispatch::upstream::types::UpstreamHealth::Healthy)
    ));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn cancelled_discovery_releases_lock_and_next_native_listing_recovers() {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let fixture = QaUpstream {
        gate: Some(Arc::clone(&gate)),
        ..Default::default()
    };
    let provider = serve_upstream(fixture.clone()).await;
    let (server, pool) = gateway(vec![upstream("qa", &provider)], 2000, 2).await;
    {
        let discovery = server.ensure_resource_upstreams_ready(&pool);
        tokio::pin!(discovery);
        tokio::select! {
            () = &mut discovery => panic!("fixture has not released discovery"),
            () = fixture.entered.notified() => {}
        }
        // Dropping this polled future models caller cancellation mid-connect.
    }
    assert_eq!(pool.connection_count_for_tests().await, 0);
    gate.add_permits(4);
    let http = serve_gateway(server).await;
    let client = client(&http, true).await;
    tokio::time::timeout(Duration::from_secs(5), assert_documents(&client, true))
        .await
        .unwrap();
    assert_eq!(fixture.probes.load(Ordering::SeqCst), 4);
    assert!(matches!(
        pool.upstream_tool_health("qa").await,
        Some(crate::dispatch::upstream::types::UpstreamHealth::Healthy)
    ));
    client.cancel().await.unwrap();
}
