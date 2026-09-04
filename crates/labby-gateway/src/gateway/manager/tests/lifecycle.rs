//! Reload / pool-lifecycle tests: lazy seeding, catalog diffing, runtime
//! handle swaps, and virtual-server quarantine migration.

use std::collections::BTreeSet;
use std::future::Future as _;
use std::task::{Context, Waker};

use crate::gateway::config::{load_gateway_config, write_gateway_config};
use crate::gateway::manager::pool_lifecycle::quarantine_unregistered_virtual_servers;
use crate::gateway::manager::{GatewayCatalogSnapshot, diff_catalogs};
use labby_runtime::gateway_config::{VirtualServerConfig, VirtualServerSurfacesConfig};

use super::*;

#[tokio::test]
async fn reload_seeds_lazy_upstreams_without_connecting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    write_gateway_config(
        &path,
        &GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream: vec![fixture_http_upstream("alpha")],
            ..GatewayConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("reload");

    let pool = manager.current_pool().await.expect("pool installed");
    assert!(pool.cached_upstream_summary("alpha").await.is_some());
    assert_eq!(pool.connection_count_for_tests().await, 0);
    assert!(pool.healthy_tools_for_upstream("alpha").await.is_empty());
}

#[tokio::test]
async fn reload_applies_configured_upstream_request_timeout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream_request_timeout_ms: Some(60_000),
            upstream: vec![fixture_http_upstream("alpha")],
            ..GatewayConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("reload");

    let pool = manager.current_pool().await.expect("pool installed");
    assert_eq!(pool.request_timeout(), Duration::from_mins(1));
}

#[tokio::test]
async fn gateway_test_does_not_schedule_background_reprobes() {
    UpstreamPool::reset_probe_task_schedule_count_for_tests("ephemeral-stdio");
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let upstream = fixture_stdio_upstream("ephemeral-stdio");

    let _runtime = manager
        .test(Ok::<&UpstreamConfig, &str>(&upstream))
        .await
        .expect("gateway test returns a runtime view");

    assert_eq!(
        UpstreamPool::probe_task_schedule_count_for_tests("ephemeral-stdio"),
        0
    );
}

#[test]
fn catalog_diff_detects_removed_tool_provider() {
    let before = GatewayCatalogSnapshot {
        tools: std::iter::once("fixture-http-echo".to_string()).collect(),
        resources: BTreeSet::new(),
        prompts: BTreeSet::new(),
    };
    let after = GatewayCatalogSnapshot::default();

    let diff = diff_catalogs(&before, &after);
    assert!(diff.tools_changed);
    assert!(!diff.resources_changed);
    assert!(!diff.prompts_changed);
}

// The HTTP dispatch surface wraps every request in a 30s TimeoutLayer that
// drops the handler future at the deadline. A reload driven directly by the
// request future is silently cancelled mid-rebuild and the pending config is
// never applied. The detached entry point must survive caller cancellation.
#[tokio::test]
async fn detached_reload_applies_config_after_caller_cancellation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![fixture_http_upstream("alpha")],
            ..GatewayConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("initial reload");

    let mut cfg = load_gateway_config(&path).expect("load config");
    cfg.upstream = vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ];
    write_gateway_config(&path, &cfg).expect("rewrite config");

    // Simulate the timeout middleware dropping the request future after a
    // single poll: the reload must keep running in its owned task.
    //
    // Poll exactly once by hand rather than racing `timeout(Duration::ZERO, ..)`
    // against the spawned task. That form `.await`s, which hands control to the
    // runtime and lets the reload run to completion before the deadline check —
    // whether it does is poll-ordering dependent, which is what made this test
    // flaky in CI (issue #261 B1).
    //
    // One manual poll is deterministic here. `reload_with_origin_detached` runs
    // synchronously up to its `tokio::spawn` and then awaits the `JoinHandle`,
    // and `#[tokio::test]` gives a **current-thread** runtime, on which a
    // spawned task cannot make progress until the caller yields. So the task is
    // guaranteed to be spawned-but-unstarted at this point: `Poll::Pending`,
    // asserted directly instead of inferred. (Converting this test to
    // `flavor = "multi_thread"` would reintroduce the race.)
    let mut detached =
        Box::pin(manager.reload_with_origin_detached(None, None, Duration::from_secs(30)));
    let first_poll = detached
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()));
    assert!(
        first_poll.is_pending(),
        "caller future completed within one poll; cancellation was not exercised"
    );
    // The real drop — `Box::pin` owns the future, so this is the caller
    // cancelling mid-reload rather than merely releasing a borrow.
    drop(detached);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(pool) = manager.current_pool().await
            && pool.cached_upstream_summary("beta").await.is_some()
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "detached reload never applied the pending config"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn detached_reload_returns_completed_diff_within_wait_budget() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![fixture_http_upstream("alpha")],
            ..GatewayConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());
    let outcome = manager
        .reload_with_origin_detached(None, None, Duration::from_secs(30))
        .await
        .expect("detached reload");

    assert!(outcome.completed);
    assert!(outcome.diff.is_some());
    assert!(manager.current_pool().await.is_some());
}

#[tokio::test]
async fn runtime_handle_starts_without_pool() {
    let handle = GatewayRuntimeHandle::default();
    assert!(handle.current_pool().await.is_none());
}

#[tokio::test]
async fn runtime_handle_swaps_pool_atomically() {
    let handle = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());

    handle.swap(Some(Arc::clone(&pool))).await;

    let current = handle.current_pool().await.expect("pool present");
    assert!(Arc::ptr_eq(&current, &pool));
}

// Re-fixtured post-gateway-pivot: `deploy` is a kept/registered service and must
// survive reload; `missing-service` is unregistered and must be quarantined.
#[tokio::test]
async fn reload_quarantines_virtual_servers_for_unregistered_services() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    write_gateway_config(
        &path,
        &GatewayConfig {
            virtual_servers: vec![
                VirtualServerConfig {
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
                    enabled: true,
                    surfaces: VirtualServerSurfacesConfig {
                        mcp: true,
                        ..VirtualServerSurfacesConfig::default()
                    },
                    mcp_policy: None,
                },
                VirtualServerConfig {
                    id: "stale-service".to_string(),
                    service: "missing-service".to_string(),
                    enabled: true,
                    surfaces: VirtualServerSurfacesConfig {
                        mcp: true,
                        ..VirtualServerSurfacesConfig::default()
                    },
                    mcp_policy: None,
                },
            ],
            ..GatewayConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default())
        .with_builtin_service_registry(deploy_known_registry());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("reload");

    let listed = manager.list().await.expect("list");
    assert!(listed.iter().any(|server| server.id == "deploy"));
    assert!(!listed.iter().any(|server| server.id == "stale-service"));

    let migrated = load_gateway_config(&path).expect("load migrated config");
    assert_eq!(migrated.virtual_servers.len(), 1);
    assert_eq!(migrated.virtual_servers[0].id, "deploy");
    assert_eq!(migrated.quarantined_virtual_servers.len(), 1);
    assert_eq!(migrated.quarantined_virtual_servers[0].id, "stale-service");
}

#[tokio::test]
async fn reload_does_not_duplicate_existing_quarantined_virtual_server() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let stale = VirtualServerConfig {
        id: "stale-service".to_string(),
        service: "missing-service".to_string(),
        enabled: true,
        surfaces: VirtualServerSurfacesConfig {
            mcp: true,
            ..VirtualServerSurfacesConfig::default()
        },
        mcp_policy: None,
    };
    write_gateway_config(
        &path,
        &GatewayConfig {
            virtual_servers: vec![stale.clone()],
            quarantined_virtual_servers: vec![stale],
            ..GatewayConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("reload");

    let migrated = load_gateway_config(&path).expect("load migrated config");
    assert!(migrated.virtual_servers.is_empty());
    assert_eq!(migrated.quarantined_virtual_servers.len(), 1);
    assert_eq!(migrated.quarantined_virtual_servers[0].id, "stale-service");
}

#[test]
fn quarantine_migration_is_noop_when_only_existing_quarantine_remains() {
    let stale = VirtualServerConfig {
        id: "stale-service".to_string(),
        service: "missing-service".to_string(),
        enabled: true,
        surfaces: VirtualServerSurfacesConfig::default(),
        mcp_policy: None,
    };

    // The default-registry builder lives in `lab`; this test only exercises the
    // already-quarantined branch (no active virtual servers), so an empty registry
    // is sufficient — nothing is looked up.
    let registry = crate::gateway::service_registry::EmptyServiceRegistry;
    let (migrated, migration) = quarantine_unregistered_virtual_servers(
        GatewayConfig {
            quarantined_virtual_servers: vec![stale],
            ..GatewayConfig::default()
        },
        &registry,
    );

    assert!(!migration.changed());
    assert!(migrated.virtual_servers.is_empty());
    assert_eq!(migrated.quarantined_virtual_servers.len(), 1);
}

// T7 — reload availability: unaffected upstream catalog entries and their live
// pool survive a single-upstream config change.
//
// The reconciliation property: after a reload that only adds one new upstream,
// the catalog entries for unchanged upstreams remain in the same live pool
// instead of forcing a full swap-and-drain.
//
// Why this captures the intent: `pool_lifecycle.rs` now evicts only changed
// upstream names, then lazy-seeds the updated config into the existing pool.
// If reconciliation regresses to dropping unchanged entries or rebuilding the
// whole pool, this test will fail.
#[tokio::test]
async fn reload_unaffected_upstream_catalog_entry_survives_single_upstream_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    // Start with two upstreams.
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
            ],
            ..GatewayConfig::default()
        },
    )
    .expect("write initial config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("initial reload");

    // Capture the pool pointer after the first reload.
    let pool_after_first = manager
        .current_pool()
        .await
        .expect("pool after first reload");
    assert!(
        pool_after_first
            .cached_upstream_summary("alpha")
            .await
            .is_some(),
        "alpha must be seeded after initial reload"
    );
    assert!(
        pool_after_first
            .cached_upstream_summary("bravo")
            .await
            .is_some(),
        "bravo must be seeded after initial reload"
    );

    // Write a new config that adds a third upstream (charlie) — alpha and bravo
    // are unchanged.
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
                fixture_http_upstream("charlie"),
            ],
            ..GatewayConfig::default()
        },
    )
    .expect("write updated config");

    manager
        .reload_with_origin(None, None)
        .await
        .expect("second reload");

    let pool_after_second = manager
        .current_pool()
        .await
        .expect("pool after second reload");

    // The reconciliation property: alpha and bravo are still present in the
    // preserved pool even though only charlie was added.
    assert!(
        pool_after_second
            .cached_upstream_summary("alpha")
            .await
            .is_some(),
        "alpha catalog entry must survive reload of unaffected upstream"
    );
    assert!(
        pool_after_second
            .cached_upstream_summary("bravo")
            .await
            .is_some(),
        "bravo catalog entry must survive reload of unaffected upstream"
    );
    assert!(
        pool_after_second
            .cached_upstream_summary("charlie")
            .await
            .is_some(),
        "charlie must be seeded in the new pool"
    );

    assert!(
        !Arc::ptr_eq(&pool_after_first, &pool_after_second),
        "single-upstream changes must publish a privately reconciled pool"
    );
}

#[tokio::test]
async fn gateway_add_reconciles_only_changed_upstream_in_live_pool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let initial = GatewayConfig {
        upstream: vec![
            fixture_http_upstream("alpha"),
            fixture_http_upstream("bravo"),
        ],
        ..GatewayConfig::default()
    };
    write_gateway_config(&path, &initial).expect("write initial config");

    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());
    manager.seed_config(initial.clone()).await;
    let pool_before = Arc::new(manager.new_base_pool(
        initial.upstream_request_timeout(),
        initial.upstream_relay_timeout(),
    ));
    pool_before.seed_lazy_upstreams(&initial.upstream).await;
    manager.runtime.swap(Some(Arc::clone(&pool_before))).await;
    assert!(
        pool_before
            .upstream_tool_last_error("alpha")
            .await
            .is_none()
    );
    assert!(
        pool_before
            .upstream_tool_last_error("bravo")
            .await
            .is_none()
    );

    manager
        .add(fixture_http_upstream("charlie"), None, Some("test"), None)
        .await
        .expect("transactional gateway add");

    let pool_after = manager
        .current_pool()
        .await
        .expect("pool after gateway add");
    assert!(
        Arc::ptr_eq(&pool_before, &pool_after),
        "transactional add must preserve the live pool and reconcile only the changed upstream"
    );
    for name in ["alpha", "bravo", "charlie"] {
        assert!(
            pool_after.cached_upstream_summary(name).await.is_some(),
            "{name} must remain seeded after selective transactional add"
        );
    }
    assert!(
        pool_after.upstream_tool_last_error("alpha").await.is_none(),
        "transactional add must not probe unrelated alpha"
    );
    assert!(
        pool_after.upstream_tool_last_error("bravo").await.is_none(),
        "transactional add must not probe unrelated bravo"
    );
}

#[tokio::test]
async fn transactional_selective_probe_does_not_hold_publication_barrier() -> Result<(), String> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let initial = GatewayConfig {
        upstream: vec![fixture_http_upstream("alpha")],
        ..GatewayConfig::default()
    };
    write_gateway_config(&path, &initial).expect("write initial config");

    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());
    manager.seed_config(initial.clone()).await;
    let pool = Arc::new(manager.new_base_pool(
        initial.upstream_request_timeout(),
        initial.upstream_relay_timeout(),
    ));
    pool.seed_lazy_upstreams(&initial.upstream).await;
    manager.runtime.swap(Some(Arc::clone(&pool))).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let mut charlie = fixture_http_upstream("charlie");
    charlie.url = Some(format!("http://{address}/mcp"));

    let mut adding = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.add(charlie, None, Some("test"), None).await })
    };
    // Admission includes durable config publication and Windows ACL helpers.
    // Those are fixture preparation, not the publication-reader latency being
    // tested below. Surface an early mutation failure instead of hiding it
    // behind a listener timeout; keep the actual reader deadline at one second.
    let socket = tokio::select! {
        accepted = tokio::time::timeout(Duration::from_secs(30), listener.accept()) => {
            match accepted {
                Ok(Ok((socket, _))) => socket,
                failure => {
                    adding.abort();
                    drop(adding.await);
                    return Err(format!("selective probe did not reach blocking server: {failure:?}"));
                }
            }
        }
        result = &mut adding => return Err(format!("selective add ended before its probe connected: {result:?}")),
    };

    let (published, published_pool) =
        tokio::time::timeout(Duration::from_secs(1), manager.published_config_and_pool())
            .await
            .expect("publication reader must not wait for changed-upstream network I/O");
    assert!(
        published
            .upstream
            .iter()
            .any(|upstream| upstream.name == "charlie"),
        "candidate config must already be published while the changed upstream probes"
    );
    assert!(
        published_pool
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &pool)),
        "transactional selective reconcile must preserve the live pool"
    );

    drop(socket);
    drop(listener);
    tokio::time::timeout(Duration::from_secs(5), adding)
        .await
        .expect("selective add completes after blocked peer closes")
        .expect("add task")
        .expect("probe failure is health state, not transaction failure");
    Ok(())
}

#[tokio::test]
async fn transactional_selective_runtime_state_failure_restores_live_pool_and_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let initial = GatewayConfig {
        upstream: vec![fixture_http_upstream("alpha")],
        ..GatewayConfig::default()
    };
    write_gateway_config(&path, &initial).expect("write initial config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager.seed_config(initial.clone()).await;
    let pool = Arc::new(manager.new_base_pool(
        initial.upstream_request_timeout(),
        initial.upstream_relay_timeout(),
    ));
    pool.seed_lazy_upstreams(&initial.upstream).await;
    manager.runtime.swap(Some(Arc::clone(&pool))).await;

    let runtime_state_path = path.with_file_name("config.runtime.json");
    std::fs::create_dir(&runtime_state_path).expect("block runtime-state file with directory");

    manager
        .add(fixture_http_upstream("charlie"), None, Some("test"), None)
        .await
        .expect_err("runtime-state failure must fail the transaction");

    let live = manager.current_config().await;
    assert_eq!(
        live.upstream
            .iter()
            .map(|upstream| upstream.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha"],
        "live config must return to the prior revision"
    );
    let current_pool = manager.current_pool().await.expect("live pool");
    assert!(Arc::ptr_eq(&pool, &current_pool));
    assert!(
        current_pool
            .cached_upstream_summary("alpha")
            .await
            .is_some(),
        "prior upstream must remain in the live pool"
    );
    assert!(
        current_pool
            .cached_upstream_summary("charlie")
            .await
            .is_none(),
        "failed candidate upstream must be removed from the live pool"
    );
    let persisted = load_gateway_config(&path).expect("rollback disk config");
    assert_eq!(persisted.upstream.len(), 1);
    assert_eq!(persisted.upstream[0].name, "alpha");
}

#[tokio::test]
async fn reload_changed_upstream_rebuilds_pool_instead_of_reusing_stale_runtime() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
            ],
            ..GatewayConfig::default()
        },
    )
    .expect("write initial config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("initial reload");
    let pool_before = manager
        .current_pool()
        .await
        .expect("pool after first reload");

    let mut changed_alpha = fixture_http_upstream("alpha");
    changed_alpha.url = Some("http://127.0.0.1:9100".to_string());
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![changed_alpha, fixture_http_upstream("bravo")],
            ..GatewayConfig::default()
        },
    )
    .expect("write updated config");

    manager
        .reload_with_origin(None, None)
        .await
        .expect("second reload");
    let pool_after = manager
        .current_pool()
        .await
        .expect("pool after second reload");

    assert!(
        !Arc::ptr_eq(&pool_before, &pool_after),
        "modified upstreams must rebuild the pool to avoid stale runtime state"
    );
}

#[tokio::test]
async fn cancelled_reload_keeps_old_pool_available_while_replacement_probe_is_blocked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![fixture_http_upstream("alpha")],
            ..GatewayConfig::default()
        },
    )
    .expect("write initial config");
    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("initial reload");
    let old_pool = manager.current_pool().await.expect("old pool");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let mut changed = fixture_http_upstream("alpha");
    changed.url = Some(format!("http://{address}/mcp"));
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![changed],
            ..GatewayConfig::default()
        },
    )
    .expect("write changed config");

    let reload_manager = manager.clone();
    let reload = tokio::spawn(async move { reload_manager.reload_with_origin(None, None).await });
    let (_socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("replacement probe reached blocking server")
        .expect("accepted");
    reload.abort();
    drop(reload.await);

    let still_live = manager
        .current_pool()
        .await
        .expect("runtime remains published");
    assert!(
        Arc::ptr_eq(&old_pool, &still_live),
        "cancellation before replacement readiness must keep the old pool serving"
    );
}

#[tokio::test]
async fn publication_barrier_hides_mixed_pool_and_config_revisions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    manager.seed_config(GatewayConfig::default()).await;

    let publication = manager.publication_barrier.write().await;
    manager
        .runtime
        .swap(Some(Arc::new(UpstreamPool::new())))
        .await;

    // Pause after the first component changes. A multi-component reader must
    // not observe this mixed revision while publication is in flight.
    let reading_manager = manager.clone();
    let reader = tokio::spawn(async move { reading_manager.published_config_and_pool().await });
    tokio::task::yield_now().await;
    assert!(
        !reader.is_finished(),
        "reader must wait at deterministic mid-publication pause"
    );

    manager.config.write().await.code_mode.enabled = true;
    manager.store.set_process_code_mode_enabled(true);
    drop(publication);

    let (config, pool) = reader.await.expect("reader joins");
    assert!(config.code_mode.enabled);
    assert!(pool.is_some());
}

#[tokio::test]
async fn reload_removed_upstream_rebuilds_pool_instead_of_reusing_stale_runtime() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
            ],
            ..GatewayConfig::default()
        },
    )
    .expect("write initial config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("initial reload");
    let pool_before = manager
        .current_pool()
        .await
        .expect("pool after first reload");

    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![fixture_http_upstream("bravo")],
            ..GatewayConfig::default()
        },
    )
    .expect("write updated config");

    manager
        .reload_with_origin(None, None)
        .await
        .expect("second reload");
    let pool_after = manager
        .current_pool()
        .await
        .expect("pool after second reload");

    assert!(
        !Arc::ptr_eq(&pool_before, &pool_after),
        "removed upstreams must rebuild the pool to avoid stale runtime state"
    );
}

// Perf C1 regression: a true no-op reload (the on-disk config is byte-identical
// to the live in-memory config) MUST preserve the live `Arc<UpstreamPool>`. The
// fingerprint-gated short-circuit in `pool_lifecycle.rs`
// (`upstream_runtime_fingerprint` + the `pool_inputs_unchanged` branch that logs
// `pool_rebuild_skipped=true`) is what keeps lazily-spawned stdio children alive
// across unrelated reloads. Without this, every reload would tear down and
// rebuild the pool, forcing a re-handshake on next use.
//
// This is the no-op counterpart to
// `reload_unaffected_upstream_catalog_entry_survives_single_upstream_change`.
// We have no log-capture infra wired in this tree, so the `Arc::ptr_eq`
// identity assertion is the core contract: same Arc means the rebuild was
// skipped.
#[tokio::test]
async fn reload_noop_preserves_live_pool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
            ],
            ..GatewayConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("initial reload");

    let pool_before = manager
        .current_pool()
        .await
        .expect("pool after first reload");

    // Reload again WITHOUT changing the config on disk — the upstream set, gateway
    // spawn prefs, and code-mode settings are byte-identical, so the fingerprint
    // matches and the live pool must be preserved.
    manager
        .reload_with_origin(None, None)
        .await
        .expect("no-op reload");

    let pool_after = manager
        .current_pool()
        .await
        .expect("pool after no-op reload");

    assert!(
        Arc::ptr_eq(&pool_before, &pool_after),
        "a no-op reload must preserve the SAME live pool (fingerprint unchanged ⇒ \
         pool_rebuild_skipped); a rebuilt pool here re-regresses Perf C1"
    );
}

// Perf C1 regression: a reload that changes ONLY fields the fingerprint
// deliberately EXCLUDES (here: `protected_mcp_routes`) must also preserve the live
// pool. `upstream_runtime_fingerprint` hashes only the upstream set, gateway spawn
// prefs, code-mode config, and request timeout — protected routes, virtual servers,
// tombstones, and public URLs are reconciled separately and must NOT force a pool
// rebuild. If a future edit folds protected routes into the fingerprint, this test
// fails (correctly flagging the C1 regression).
#[tokio::test]
async fn reload_protected_routes_only_change_preserves_live_pool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
            ],
            ..GatewayConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("initial reload");

    let pool_before = manager
        .current_pool()
        .await
        .expect("pool after first reload");

    // Rewrite the config adding ONLY a protected MCP route — every upstream and all
    // pool-shaping config is untouched, so the runtime fingerprint is unchanged.
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
            ],
            protected_mcp_routes: vec![fixture_protected_route("syslog")],
            ..GatewayConfig::default()
        },
    )
    .expect("write config with protected route");

    manager
        .reload_with_origin(None, None)
        .await
        .expect("protected-routes-only reload");

    let pool_after = manager
        .current_pool()
        .await
        .expect("pool after protected-routes reload");

    assert!(
        Arc::ptr_eq(&pool_before, &pool_after),
        "a reload that changes only fingerprint-excluded fields (protected routes) \
         must preserve the SAME live pool; a rebuilt pool here re-regresses Perf C1"
    );

    // The protected-routes reconciliation still happened — the in-memory config now
    // carries the new route even though the pool was preserved.
    let cfg = manager.current_config().await;
    assert_eq!(
        cfg.protected_mcp_routes.len(),
        1,
        "protected route must be applied even on the pool-preserving path"
    );
}
