use super::*;

#[tokio::test]
async fn waiting_callers_exhaust_budget_without_poisoning_connection_owner() {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let fixture = QaUpstream {
        gate: Some(Arc::clone(&gate)),
        ..Default::default()
    };
    let provider = serve_upstream(fixture.clone()).await;
    let config = upstream("qa", &provider);
    let (server, pool) = gateway(vec![config.clone()], 40, 2).await;
    let owner = pool.ensure_connection_for_upstream(&config, None, None);
    tokio::pin!(owner);
    tokio::select! {
        result = &mut owner => panic!("owner must wait for fixture release: {result:?}"),
        () = fixture.entered.notified() => {}
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(
            server.ensure_resource_upstreams_ready(&pool),
            server.ensure_resource_upstreams_ready(&pool),
            server.ensure_resource_upstreams_ready(&pool)
        );
    })
    .await
    .expect("waiting callers must observe their own deadline");
    assert_eq!(
        fixture.probes.load(Ordering::SeqCst),
        1,
        "waiters must not open connections"
    );
    assert!(matches!(
        pool.upstream_tool_health("qa").await,
        Some(crate::dispatch::upstream::types::UpstreamHealth::Healthy)
    ));
    gate.add_permits(1);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), owner)
            .await
            .unwrap()
            .unwrap()
    );
    server.ensure_resource_upstreams_ready(&pool).await;
    assert_eq!(fixture.probes.load(Ordering::SeqCst), 1);
    assert_eq!(pool.list_upstream_resources().await.len(), 2);
}
