//! Winning bearer authority, rather than reconnect intent, owns durable handles.
#[path = "task_bearer_transaction_tests.rs"]
mod transaction;

use super::*;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Mutex;

const TOKEN_ENV: &str = "LABBY_TASK_BEARER_RELOAD_FIXTURE";
const FILE_TOKEN: &str = "file-original-fixture-token";
const PROCESS_TOKEN: &str = "process-winning-fixture-token";

#[derive(Clone)]
struct BearerTasks {
    tasks: HttpTasks,
    expected: Arc<Mutex<String>>,
}

impl Respond for BearerTasks {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        assert_eq!(
            request
                .headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap(),
            format!("Bearer {}", self.expected.lock().unwrap()),
            "each actual transport must use the winning credential"
        );
        self.tasks.respond(request)
    }
}

async fn fixture(
    root: &Path,
    label: &str,
    server: &MockServer,
) -> (GatewayConfig, GatewayManager, Arc<TaskRouteStore>) {
    let config_path = root.join(format!("{label}.toml"));
    let mut config = GatewayConfig::default();
    let mut upstream = super::super::super::super::testsupport::test_upstream_config();
    upstream.name = "bearer".into();
    upstream.url = Some(format!("{}/mcp", server.uri()));
    upstream.bearer_token_env = Some(TOKEN_ENV.into());
    config.upstream.push(upstream);
    crate::gateway::config::write_gateway_config(&config_path, &config).unwrap();
    let routes = Arc::new(
        TaskRouteStore::open(root.join(format!("{label}.db")))
            .await
            .unwrap(),
    );
    let manager = GatewayManager::new(config_path, GatewayRuntimeHandle::default())
        .with_task_route_store(Arc::clone(&routes));
    manager
        .seed_config_unchecked_for_tests(config.clone())
        .await;
    manager.reload_with_origin(None, None).await.unwrap();
    (config, manager, routes)
}

async fn register(
    pool: &super::super::super::super::UpstreamPool,
    upstream: &labby_runtime::gateway_config::UpstreamConfig,
    downstream: rmcp::service::Peer<rmcp::RoleServer>,
) -> String {
    let capabilities = ClientCapabilities::builder().enable_tasks().build();
    let key = (
        upstream.name.clone(),
        7,
        None,
        super::super::super::super::relay_cache::capability_fingerprint(&capabilities),
    );
    pool.acquire_relay_for_key(upstream, None, downstream, capabilities, key.clone())
        .await
        .unwrap();
    let response = pool
        .register_task_response(
            &key,
            &crate::gateway::code_mode::catalog_cache::fingerprint(upstream),
            Some("alice"),
            TaskRouteAuthorization::root(),
            create_task_response(),
        )
        .await
        .unwrap();
    let CallToolResponse::Task(created) = response else {
        unreachable!("fixture produces a task")
    };
    created.task.task_id
}

async fn assert_routable(
    pool: &super::super::super::super::UpstreamPool,
    id: &str,
    downstream: rmcp::service::Peer<rmcp::RoleServer>,
) {
    let authorization = TaskRouteAuthorization::root();
    pool.get_task_routed(
        GetTaskParams::new(id),
        Some("alice"),
        &authorization,
        downstream.clone(),
    )
    .await
    .unwrap();
    pool.update_task_routed(
        UpdateTaskParams::new(id, InputResponses::new()),
        Some("alice"),
        &authorization,
        id,
        downstream.clone(),
    )
    .await
    .unwrap();
    pool.cancel_task_routed(
        CancelTaskParams::new(id),
        Some("alice"),
        &authorization,
        id,
        downstream,
    )
    .await
    .unwrap();
}

async fn assert_revoked(
    pool: &super::super::super::super::UpstreamPool,
    id: &str,
    downstream: rmcp::service::Peer<rmcp::RoleServer>,
) {
    let authorization = TaskRouteAuthorization::root();
    assert_eq!(
        pool.get_task_routed(
            GetTaskParams::new(id),
            Some("alice"),
            &authorization,
            downstream.clone()
        )
        .await
        .unwrap_err(),
        "task not found"
    );
    assert_eq!(
        pool.update_task_routed(
            UpdateTaskParams::new(id, InputResponses::new()),
            Some("alice"),
            &authorization,
            id,
            downstream.clone()
        )
        .await
        .unwrap_err(),
        "task not found"
    );
    assert_eq!(
        pool.cancel_task_routed(
            CancelTaskParams::new(id),
            Some("alice"),
            &authorization,
            id,
            downstream
        )
        .await
        .unwrap_err(),
        "task not found"
    );
}

async fn preservation(process_override: bool) {
    let root = std::env::var_os("LABBY_HOME")
        .map(std::path::PathBuf::from)
        .unwrap();
    let dotenv = root.join(".env");
    std::fs::write(&dotenv, format!("{TOKEN_ENV}={FILE_TOKEN}\n")).unwrap();
    crate::upstream::auth::register_external_environment_keys(if process_override {
        BTreeSet::from([TOKEN_ENV.into()])
    } else {
        BTreeSet::new()
    });
    let http = MockServer::start().await;
    let responder = BearerTasks {
        tasks: HttpTasks::default(),
        expected: Arc::new(Mutex::new(
            if process_override {
                PROCESS_TOKEN
            } else {
                FILE_TOKEN
            }
            .into(),
        )),
    };
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(responder.clone())
        .mount(&http)
        .await;
    let (config, manager, routes) = fixture(&root, "preserved", &http).await;
    let (fixture_pool, _server, downstream, _) = task_pool().await;
    let initial = manager.current_pool_sync().unwrap();
    let id = register(&initial, &config.upstream[0], downstream.peer().clone()).await;
    for index in 0..3 {
        if index == 1 {
            let value = if process_override {
                "shadowed-file-replacement".into()
            } else {
                format!("Bearer {FILE_TOKEN}")
            };
            std::fs::write(&dotenv, format!("{TOKEN_ENV}='{value}'\n")).unwrap();
        } else if index == 2 && process_override {
            std::fs::remove_file(&dotenv).unwrap();
        }
        let old_pool = manager.current_pool_sync().unwrap();
        let before_discovery = http.received_requests().await.unwrap().len();
        manager.reload_with_origin(None, None).await.unwrap();
        let current = manager.current_pool_sync().unwrap();
        assert!(
            !Arc::ptr_eq(&old_pool, &current),
            "bearer peers still reconnect on every manual reload"
        );
        assert!(http.received_requests().await.unwrap().len() > before_discovery);
        assert!(
            routes.get(&id).await.unwrap().is_some(),
            "unchanged winning bearer authority must preserve its durable handle"
        );
        let before = responder.tasks.requests.load(Ordering::SeqCst);
        assert_routable(&current, &id, downstream.peer().clone()).await;
        assert_eq!(responder.tasks.requests.load(Ordering::SeqCst) - before, 3);
    }
    let reopened = Arc::new(
        TaskRouteStore::open(root.join("preserved.db"))
            .await
            .unwrap(),
    );
    let restarted =
        GatewayManager::new(root.join("preserved.toml"), GatewayRuntimeHandle::default())
            .with_task_route_store(Arc::clone(&reopened));
    restarted.seed_config_unchecked_for_tests(config).await;
    restarted.reload_with_origin(None, None).await.unwrap();
    assert!(reopened.get(&id).await.unwrap().is_some());
    assert_routable(
        &restarted.current_pool_sync().unwrap(),
        &id,
        downstream.peer().clone(),
    )
    .await;
    restarted
        .current_pool_sync()
        .unwrap()
        .drain_for_swap("bearer.fixture.cleanup")
        .await;
    manager
        .current_pool_sync()
        .unwrap()
        .drain_for_swap("bearer.fixture.cleanup")
        .await;
    fixture_pool.drain_for_swap("bearer.fixture.cleanup").await;
    downstream.cancel().await.unwrap();
}

#[tokio::test]
async fn manual_bearer_reload_preserves_file_managed_tasks() {
    if crate::upstream::test_isolation::run("upstream::pool::task_route_store::tests::reload::bearer::manual_bearer_reload_preserves_file_managed_tasks", &[]).await {
        return;
    }
    preservation(false).await;
}

#[tokio::test]
async fn manual_bearer_reload_preserves_process_override_tasks() {
    if crate::upstream::test_isolation::run("upstream::pool::task_route_store::tests::reload::bearer::manual_bearer_reload_preserves_process_override_tasks", &[(TOKEN_ENV, PROCESS_TOKEN)]).await {
        return;
    }
    preservation(true).await;
}

#[tokio::test]
async fn manual_bearer_reload_revokes_rotated_and_removed_file_authority() {
    if crate::upstream::test_isolation::run("upstream::pool::task_route_store::tests::reload::bearer::manual_bearer_reload_revokes_rotated_and_removed_file_authority", &[]).await {
        return;
    }
    crate::upstream::auth::register_external_environment_keys(BTreeSet::new());
    let root = std::env::var_os("LABBY_HOME")
        .map(std::path::PathBuf::from)
        .unwrap();
    let http = MockServer::start().await;
    let responder = BearerTasks {
        tasks: HttpTasks::default(),
        expected: Arc::new(Mutex::new(FILE_TOKEN.into())),
    };
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(responder.clone())
        .mount(&http)
        .await;
    let (fixture_pool, _server, downstream, _) = task_pool().await;
    for change in ["rotate", "remove"] {
        *responder.expected.lock().unwrap() = FILE_TOKEN.into();
        std::fs::write(root.join(".env"), format!("{TOKEN_ENV}={FILE_TOKEN}\n")).unwrap();
        let (config, manager, routes) = fixture(&root, change, &http).await;
        let id = register(
            &manager.current_pool_sync().unwrap(),
            &config.upstream[0],
            downstream.peer().clone(),
        )
        .await;
        if change == "rotate" {
            *responder.expected.lock().unwrap() = "rotated-file-fixture-token".into();
            std::fs::write(
                root.join(".env"),
                format!("{TOKEN_ENV}=rotated-file-fixture-token\n"),
            )
            .unwrap();
        } else {
            std::fs::remove_file(root.join(".env")).unwrap();
        }
        manager.reload_with_origin(None, None).await.unwrap();
        assert!(routes.get(&id).await.unwrap().is_none());
        let before = responder.tasks.requests.load(Ordering::SeqCst);
        assert_revoked(
            &manager.current_pool_sync().unwrap(),
            &id,
            downstream.peer().clone(),
        )
        .await;
        *responder.expected.lock().unwrap() = FILE_TOKEN.into();
        std::fs::write(root.join(".env"), format!("{TOKEN_ENV}={FILE_TOKEN}\n")).unwrap();
        manager.reload_with_origin(None, None).await.unwrap();
        assert_revoked(
            &manager.current_pool_sync().unwrap(),
            &id,
            downstream.peer().clone(),
        )
        .await;
        let reopened = Arc::new(
            TaskRouteStore::open(root.join(format!("{change}.db")))
                .await
                .unwrap(),
        );
        let restarted = GatewayManager::new(
            root.join(format!("{change}.toml")),
            GatewayRuntimeHandle::default(),
        )
        .with_task_route_store(Arc::clone(&reopened));
        restarted.seed_config_unchecked_for_tests(config).await;
        restarted.reload_with_origin(None, None).await.unwrap();
        assert!(reopened.get(&id).await.unwrap().is_none());
        assert_revoked(
            &restarted.current_pool_sync().unwrap(),
            &id,
            downstream.peer().clone(),
        )
        .await;
        assert_eq!(
            responder.tasks.requests.load(Ordering::SeqCst),
            before,
            "revoked handles never dispatch after reload/restore/restart"
        );
        restarted
            .current_pool_sync()
            .unwrap()
            .drain_for_swap("bearer.fixture.cleanup")
            .await;
        manager
            .current_pool_sync()
            .unwrap()
            .drain_for_swap("bearer.fixture.cleanup")
            .await;
    }
    fixture_pool.drain_for_swap("bearer.fixture.cleanup").await;
    downstream.cancel().await.unwrap();
}
