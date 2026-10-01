use super::*;
use crate::upstream::pool::testsupport::*;

#[tokio::test]
async fn optional_observation_preserves_failure_empty_and_age() {
    use crate::gateway::view_models::CapabilityObservationState as State;
    let pool = static_catalog_pool("alpha").await;
    let config = UpstreamConfig {
        name: "alpha".into(),
        proxy_resources: true,
        proxy_prompts: true,
        ..test_upstream_config()
    };
    pool.register_upstream_config_for_tests(&config);
    move_connection_to_subject_cache_with_tools(&pool, "alpha", "alice", vec![]).await;
    let peer = pool
        .subject_connections
        .read()
        .await
        .get(&("alpha".into(), "alice".into()))
        .unwrap()
        .peer
        .clone();
    let known = pool
        .cached_subject_summary(&config, Some("alice"))
        .await
        .observation();
    assert_eq!(known.tools.state, State::Known);
    assert_eq!(known.tools.discovered, Some(0));
    pool.record_subject_optional_failure("alpha", "alice", &peer, true, "resources/list timeout")
        .await;
    let failed = pool
        .cached_subject_summary(&config, Some("alice"))
        .await
        .observation();
    assert_eq!(failed.resources.state, State::Failed);
    assert!(failed.resources.error.is_some());
    assert_eq!(failed.tools.state, State::Known);
    pool.record_subject_optional_catalog("alpha", "alice", &peer, Some(vec![]), Some(vec![]))
        .await;
    let empty = pool
        .cached_subject_summary(&config, Some("alice"))
        .await
        .observation();
    assert_eq!(empty.resources.state, State::Known);
    assert_eq!(empty.resources.discovered, Some(0));
    assert_eq!(empty.resources.error, None);
    pool.age_subject_resource_catalog_for_tests("alpha", "alice", RESOURCE_SNAPSHOT_MAX_AGE)
        .await;
    assert_eq!(
        pool.cached_subject_summary(&config, Some("alice"))
            .await
            .observation()
            .resources
            .state,
        State::Stale
    );
    pool.subject_connections
        .write()
        .await
        .get_mut(&("alpha".into(), "alice".into()))
        .unwrap()
        .last_used = Instant::now()
        .checked_sub(SUBJECT_CONN_IDLE_TTL)
        .expect("test TTL fits the monotonic clock");
    assert_eq!(
        pool.cached_subject_summary(&config, Some("alice"))
            .await
            .observation()
            .tools
            .state,
        State::Stale
    );
    let mut replacement = config.clone();
    replacement.url = Some("http://replacement.invalid".into());
    pool.register_upstream_config_for_tests(&replacement);
    assert_eq!(
        pool.cached_subject_summary(&config, Some("alice"))
            .await
            .observation()
            .tools
            .state,
        State::Unknown
    );
}

#[tokio::test]
async fn observation_distinguishes_unknown_empty_and_private_catalogs() {
    let pool = static_catalog_pool("alpha").await;
    let config = UpstreamConfig {
        name: "alpha".into(),
        proxy_resources: true,
        proxy_prompts: true,
        ..test_upstream_config()
    };
    pool.register_upstream_config_for_tests(&config);
    let missing = pool
        .cached_subject_summary(&config, Some("alice"))
        .await
        .observation();
    assert_eq!(
        serde_json::to_value(&missing).unwrap()["tools"]["state"],
        "unknown"
    );
    assert_eq!(missing.tools.discovered, None);
    move_connection_to_subject_cache_with_tools(
        &pool,
        "alpha",
        "alice",
        (0..91).map(|i| test_tool(&format!("tool_{i}"))).collect(),
    )
    .await;
    let observed = pool
        .cached_subject_summary(&config, Some("alice"))
        .await
        .observation();
    assert_eq!(observed.tools.discovered, Some(91));
    assert_eq!(observed.tools.exposed, Some(91));
    assert_eq!(observed.skills.discovered, None);
    assert_eq!(
        pool.cached_subject_summary(&config, Some("bob"))
            .await
            .observation()
            .tools
            .discovered,
        None
    );
    assert_eq!(
        pool.cached_subject_inventory(&config, Some("alice"))
            .await
            .0
            .len(),
        91
    );
    assert!(
        pool.cached_subject_inventory(&config, Some("bob"))
            .await
            .0
            .is_empty()
    );
}

#[tokio::test]
async fn summaries_keep_subject_catalogs_isolated_and_empty_known() {
    let pool = static_catalog_pool("alpha").await;
    move_connection_to_subject_cache_with_tools(&pool, "alpha", "alice", vec![test_tool("search")])
        .await;
    let other = static_catalog_pool("alpha").await;
    let connection = other.connections.write().await.remove("alpha").unwrap();
    pool.connections
        .write()
        .await
        .insert("alpha".into(), connection);
    move_connection_to_subject_cache_with_tools(&pool, "alpha", "bob", vec![]).await;
    let config = UpstreamConfig {
        name: "alpha".into(),
        proxy_resources: true,
        proxy_prompts: true,
        ..test_upstream_config()
    };
    pool.register_upstream_config_for_tests(&config);
    let alice = pool.cached_subject_summary(&config, Some("alice")).await;
    let bob = pool.cached_subject_summary(&config, Some("bob")).await;
    assert!(alice.connected && bob.connected);
    assert_eq!(alice.summary.discovered_tool_count, 1);
    assert_eq!(bob.summary.discovered_tool_count, 0);
    assert!(bob.tools_known);
    assert!(!bob.resources_known);
    let bob_peer = pool
        .subject_connections
        .read()
        .await
        .get(&("alpha".into(), "bob".into()))
        .unwrap()
        .peer
        .clone();
    pool.record_subject_optional_catalog("alpha", "bob", &bob_peer, Some(vec![]), Some(vec![]))
        .await;
    let bob = pool.cached_subject_summary(&config, Some("bob")).await;
    assert!(bob.resources_known && bob.prompts_known);
    assert!(
        !pool
            .cached_subject_summary(&config, Some("alice"))
            .await
            .resources_known
    );
    let missing = pool.cached_subject_summary(&config, Some("unknown")).await;
    assert!(!missing.connected && !missing.tools_known);
    assert_eq!(missing.summary.discovered_tool_count, 0);
    let absent = pool.cached_subject_summary(&config, None).await;
    assert!(!absent.connected && !absent.tools_known);
}
