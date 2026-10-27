//! File updates revoke only if they replace the credential the transport uses.
use super::*;

#[tokio::test]
async fn bearer_file_update_preserves_unchanged_process_authority() {
    if crate::upstream::test_isolation::run(
        "upstream::pool::task_route_store::tests::reload::bearer::transaction::bearer_file_update_preserves_unchanged_process_authority",
        &[(TOKEN_ENV, PROCESS_TOKEN)],
    )
    .await
    {
        return;
    }
    crate::upstream::auth::register_external_environment_keys(BTreeSet::from([TOKEN_ENV.into()]));
    let root = std::env::var_os("LABBY_HOME")
        .map(std::path::PathBuf::from)
        .unwrap();
    std::fs::write(root.join(".env"), format!("{TOKEN_ENV}={FILE_TOKEN}\n")).unwrap();
    let http = MockServer::start().await;
    let responder = BearerTasks {
        tasks: HttpTasks::default(),
        expected: Arc::new(Mutex::new(PROCESS_TOKEN.into())),
    };
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(responder.clone())
        .mount(&http)
        .await;
    let (config, manager, routes) = fixture(&root, "process-update", &http).await;
    let (fixture_pool, _server, downstream, _) = task_pool().await;
    let id = register(
        &manager.current_pool_sync().unwrap(),
        &config.upstream[0],
        downstream.peer().clone(),
    )
    .await;
    let before = http.received_requests().await.unwrap().len();
    manager
        .update(
            "bearer",
            Default::default(),
            Some("shadowed-file-update".into()),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(
        http.received_requests().await.unwrap().len() > before,
        "credential update must still reconnect the transport"
    );
    assert!(
        routes.get(&id).await.unwrap().is_some(),
        "writing a shadowed file does not replace process authority"
    );
    assert_routable(
        &manager.current_pool_sync().unwrap(),
        &id,
        downstream.peer().clone(),
    )
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
async fn bearer_file_update_revokes_only_winning_replacements() {
    if crate::upstream::test_isolation::run(
        "upstream::pool::task_route_store::tests::reload::bearer::transaction::bearer_file_update_revokes_only_winning_replacements",
        &[],
    )
    .await
    {
        return;
    }
    crate::upstream::auth::register_external_environment_keys(BTreeSet::new());
    let root = std::env::var_os("LABBY_HOME")
        .map(std::path::PathBuf::from)
        .unwrap();
    std::fs::write(root.join(".env"), format!("{TOKEN_ENV}={FILE_TOKEN}\n")).unwrap();
    let http = MockServer::start().await;
    let responder = BearerTasks {
        tasks: HttpTasks::default(),
        expected: Arc::new(Mutex::new(FILE_TOKEN.into())),
    };
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(responder.clone())
        .mount(&http)
        .await;
    let (config, manager, routes) = fixture(&root, "file-update", &http).await;
    let (fixture_pool, _server, downstream, _) = task_pool().await;
    let id = register(
        &manager.current_pool_sync().unwrap(),
        &config.upstream[0],
        downstream.peer().clone(),
    )
    .await;
    for value in [FILE_TOKEN.to_string(), format!("Bearer {FILE_TOKEN}")] {
        manager
            .update("bearer", Default::default(), Some(value), None, None)
            .await
            .unwrap();
        assert!(
            routes.get(&id).await.unwrap().is_some(),
            "equivalent winning credential must preserve a durable handle"
        );
        assert_routable(
            &manager.current_pool_sync().unwrap(),
            &id,
            downstream.peer().clone(),
        )
        .await;
    }
    *responder.expected.lock().unwrap() = "actually-replaced-file-token".into();
    manager
        .update(
            "bearer",
            Default::default(),
            Some("actually-replaced-file-token".into()),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(routes.get(&id).await.unwrap().is_none());
    let before = responder.tasks.requests.load(Ordering::SeqCst);
    assert_revoked(
        &manager.current_pool_sync().unwrap(),
        &id,
        downstream.peer().clone(),
    )
    .await;
    assert_eq!(responder.tasks.requests.load(Ordering::SeqCst), before);
    manager
        .current_pool_sync()
        .unwrap()
        .drain_for_swap("bearer.fixture.cleanup")
        .await;
    fixture_pool.drain_for_swap("bearer.fixture.cleanup").await;
    downstream.cancel().await.unwrap();
}
