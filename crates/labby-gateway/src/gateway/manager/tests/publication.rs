//! Published runtime Loadout snapshot and generation tests.

use crate::gateway::config::write_gateway_config;
use labby_runtime::gateway_config::GatewayLoadoutConfig;

use super::*;

fn loadout(name: &str, upstreams: &[&str]) -> GatewayLoadoutConfig {
    GatewayLoadoutConfig {
        name: name.to_string(),
        upstreams: upstreams.iter().map(|name| (*name).to_string()).collect(),
        ..GatewayLoadoutConfig::default()
    }
}

fn config_with_loadout(loadout: GatewayLoadoutConfig) -> GatewayConfig {
    let upstream = loadout
        .upstreams
        .iter()
        .map(|name| fixture_http_upstream(name))
        .collect();
    GatewayConfig {
        upstream,
        loadouts: vec![loadout],
        ..GatewayConfig::default()
    }
}

#[tokio::test]
async fn runtime_config_generations_are_shared_by_clones_and_distinct_between_managers() {
    let first_dir = tempfile::tempdir().expect("first tempdir");
    let second_dir = tempfile::tempdir().expect("second tempdir");
    let first = GatewayManager::new(
        first_dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let first_clone = first.clone();
    let second = GatewayManager::new(
        second_dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let first_generation = first
        .published_runtime_loadout_snapshot("project")
        .await
        .generation();
    assert_eq!(
        first_clone
            .published_runtime_loadout_snapshot("project")
            .await
            .generation(),
        first_generation
    );
    assert_ne!(
        second
            .published_runtime_loadout_snapshot("project")
            .await
            .generation(),
        first_generation
    );
}

#[tokio::test]
async fn published_runtime_loadout_snapshot_ignores_staged_desired_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = loadout("project", &["runtime"]);
    let desired = loadout("project", &["desired"]);
    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());

    manager
        .seed_config(config_with_loadout(runtime.clone()))
        .await;
    let before = manager.published_runtime_loadout_snapshot("project").await;
    write_gateway_config(&path, &config_with_loadout(desired)).expect("stage desired config");
    let after = manager.published_runtime_loadout_snapshot("project").await;

    assert_eq!(before.loadout(), Some(&runtime));
    assert_eq!(after.loadout(), Some(&runtime));
    assert_eq!(after.generation(), before.generation());
}

#[tokio::test]
async fn runtime_config_generation_distinguishes_loadout_aba() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let a = loadout("project", &["alpha"]);
    let b = loadout("project", &["bravo"]);

    manager.seed_config(config_with_loadout(a.clone())).await;
    let first_a = manager.published_runtime_loadout_snapshot("project").await;
    manager.seed_config(config_with_loadout(b.clone())).await;
    let published_b = manager.published_runtime_loadout_snapshot("project").await;
    manager.seed_config(config_with_loadout(a.clone())).await;
    let second_a = manager.published_runtime_loadout_snapshot("project").await;

    assert_eq!(first_a.loadout(), Some(&a));
    assert_eq!(published_b.loadout(), Some(&b));
    assert_eq!(second_a.loadout(), Some(&a));
    assert_ne!(first_a.generation(), published_b.generation());
    assert_ne!(published_b.generation(), second_a.generation());
    assert_ne!(first_a.generation(), second_a.generation());
}

#[tokio::test]
async fn identical_runtime_config_republication_advances_generation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let config = config_with_loadout(loadout("project", &["alpha"]));

    manager.seed_config(config.clone()).await;
    let first = manager.published_runtime_loadout_snapshot("project").await;
    manager.seed_config(config).await;
    let second = manager.published_runtime_loadout_snapshot("project").await;

    assert_eq!(first.loadout(), second.loadout());
    assert_ne!(first.generation(), second.generation());
}

#[tokio::test]
async fn missing_loadout_is_bound_to_the_observed_publication() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    manager.seed_config(GatewayConfig::default()).await;
    let missing = manager.published_runtime_loadout_snapshot("project").await;
    manager
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    let present = manager.published_runtime_loadout_snapshot("project").await;

    assert!(missing.loadout().is_none());
    assert!(present.loadout().is_some());
    assert_ne!(missing.generation(), present.generation());
}
