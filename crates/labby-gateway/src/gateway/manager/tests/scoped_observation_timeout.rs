use super::*;
use std::mem::size_of_val;

#[tokio::test(start_paused = true)]
async fn outer_discovery_timeout_remains_failed_until_explicit_refresh() {
    let config = fixture_oauth_upstream("alpha", "https://fixture.invalid/mcp");
    let (manager, pool) = code_mode_manager_with_pool(config.clone()).await;
    let server = catalog();
    pool.install_test_subject_server_for_upstream(&config, "alice", server.clone())
        .await;
    manager
        .oauth_status_discovery("alpha", "alice", config.clone())
        .await;
    manager.oauth_status_discovery_cache.lock().await.clear();
    server.blocked.store(true, Ordering::SeqCst);
    let calls = server.requests.load(Ordering::SeqCst);
    let gate = pool
        .hold_subject_connect_gate_for_tests("alpha", "alice")
        .await;
    let discovery = {
        let manager = manager.clone();
        let config = config.clone();
        tokio::spawn(async move {
            manager
                .oauth_status_discovery("alpha", "alice", config)
                .await
        })
    };
    // Delay entry into tools/list so its inner pagination deadline falls after
    // the outer status deadline. This exercises cancellation, not an RPC error.
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    drop(gate);
    while server.requests.load(Ordering::SeqCst) == calls {
        tokio::task::yield_now().await;
    }
    let timeout = crate::upstream::pool::upstream_discovery_timeout(
        &config,
        manager.config.read().await.upstream_request_timeout(),
    );
    tokio::time::advance(timeout.checked_sub(Duration::from_secs(1)).unwrap()).await;
    let failed = discovery.await.unwrap();
    assert_eq!(failed.observation.tools.state, State::Failed);
    assert!(
        failed
            .tool_error
            .as_deref()
            .unwrap()
            .contains("subject tool discovery timed out")
    );
    assert_eq!(failed.observation.tools.discovered, Some(91));
    let scope = GatewayEnrichmentScope {
        oauth_subject: Some("alice".into()),
        ..Default::default()
    };
    let list = manager.list_scoped(&scope).await.unwrap();
    let single = manager.get_server_scoped("alpha", &scope).await.unwrap();
    let detail = manager.get_scoped("alpha", &scope).await.unwrap();
    let runtime = manager.status_scoped(Some("alpha"), &scope).await.unwrap();
    for observation in [
        list[0].capability_observation.as_ref(),
        single.capability_observation.as_ref(),
        detail.runtime.capability_observation.as_ref(),
        runtime[0].capability_observation.as_ref(),
    ] {
        let tools = &observation.unwrap().tools;
        assert_eq!(tools.state, State::Failed);
        assert_eq!(tools.discovered, Some(91));
        assert!(tools.error.is_some());
    }
    let calls = server.requests.load(Ordering::SeqCst);
    for elapsed in [0, 31, 260] {
        tokio::time::advance(Duration::from_secs(elapsed)).await;
        let cached = manager
            .oauth_status_discovery("alpha", "alice", config.clone())
            .await;
        assert_eq!(cached.observation.tools.state, State::Failed);
        assert_eq!(cached.observation.tools.discovered, Some(91));
        assert!(cached.tool_error.is_some());
        assert_eq!(
            server.requests.load(Ordering::SeqCst),
            calls,
            "five-minute failure cooldown must not retry"
        );
    }
    server.blocked.store(false, Ordering::SeqCst);
    server.gate.notify_waiters();
    pool.reprobe_tools_for_upstream_as(&config, Some("alice"), None)
        .await
        .unwrap();
    let recovered = manager
        .oauth_status_discovery("alpha", "alice", config)
        .await;
    assert_eq!(recovered.observation.tools.state, State::Known);
    assert_eq!(recovered.observation.tools.discovered, Some(91));
    assert!(recovered.tool_error.is_none());
}

#[tokio::test]
async fn oauth_ephemeral_test_labels_credential_scope() {
    let dir = tempfile::tempdir().unwrap();
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let mut ordinary = fixture_stdio_upstream("ordinary");
    ordinary.command = Some("false".into());
    let observed = manager.test(Ok(&ordinary)).await.unwrap();
    assert_eq!(
        observed.capability_observation.unwrap().scope,
        crate::gateway::view_models::CapabilityObservationScope::Global
    );
    let oauth = fixture_oauth_upstream("oauth", "https://fixture.invalid/mcp");
    let observed = manager.test(Ok(&oauth)).await.unwrap();
    assert_eq!(
        observed.capability_observation.unwrap().scope,
        crate::gateway::view_models::CapabilityObservationScope::Credential
    );
}

#[tokio::test(start_paused = true)]
async fn cold_outer_discovery_timeout_is_visible_without_a_catalog() {
    let config = fixture_oauth_upstream("alpha", "https://fixture.invalid/mcp");
    let (manager, pool) = code_mode_manager_with_pool(config.clone()).await;
    pool.seed_lazy_upstreams(std::slice::from_ref(&config))
        .await;
    let gate = pool
        .hold_subject_connect_gate_for_tests("alpha", "alice")
        .await;
    let discovery = {
        let manager = manager.clone();
        let config = config.clone();
        tokio::spawn(async move {
            manager
                .oauth_status_discovery("alpha", "alice", config)
                .await
        })
    };
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    let timeout = crate::upstream::pool::upstream_discovery_timeout(
        &config,
        manager.config.read().await.upstream_request_timeout(),
    );
    tokio::time::advance(timeout).await;
    let failed = discovery.await.unwrap();
    drop(gate);
    assert_eq!(failed.observation.tools.state, State::Failed);
    assert_eq!(failed.observation.tools.discovered, None);
    let scope = GatewayEnrichmentScope {
        oauth_subject: Some("alice".into()),
        ..Default::default()
    };
    let view = manager.get_scoped("alpha", &scope).await.unwrap();
    assert_eq!(
        view.runtime.capability_observation.unwrap().tools.state,
        State::Failed
    );
    let cached = manager
        .oauth_status_discovery("alpha", "alice", config)
        .await;
    assert_eq!(cached.observation.tools.state, State::Failed);
    assert!(cached.tool_error.is_some());
}

#[tokio::test]
async fn ephemeral_test_keeps_its_parent_future_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let mut upstream = fixture_stdio_upstream("frame-size");
    upstream.command = Some("false".into());
    let test = manager.test(Ok(&upstream));
    let projection = crate::gateway::projection::runtime_view(None, "frame-size", None);
    let pool = UpstreamPool::new();
    let observation = pool.cached_global_observation("frame-size");
    eprintln!(
        "future bytes: test={}, projection={}, observation={}",
        size_of_val(&test),
        size_of_val(&projection),
        size_of_val(&observation)
    );
    assert!(
        size_of_val(&test) < 4_096,
        "gateway.test must not embed the discovery/projection state machines in its caller"
    );
}
