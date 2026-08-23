//! Published runtime Loadout snapshot and generation tests.

use crate::gateway::config::write_gateway_config;
use crate::gateway::manager::LoadoutToolCatalogPublicationError;
use labby_runtime::gateway_config::GatewayLoadoutConfig;

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct PublicationRegistry {
    services: Vec<(
        &'static str,
        Vec<crate::gateway::service_registry::ServiceActionInfo>,
    )>,
    reads: Arc<AtomicUsize>,
}

impl PublicationRegistry {
    fn new(
        services: Vec<(
            &'static str,
            Vec<crate::gateway::service_registry::ServiceActionInfo>,
        )>,
    ) -> Self {
        Self {
            services,
            reads: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl crate::registry::InProcessServiceRegistry for PublicationRegistry {
    fn in_process_services(&self) -> Vec<Box<dyn crate::registry::InProcessService>> {
        Vec::new()
    }
}

impl crate::gateway::service_registry::GatewayServiceRegistry for PublicationRegistry {
    fn service_names(&self) -> Vec<&'static str> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.services.iter().map(|(name, _)| *name).collect()
    }
    fn contains_service(&self, name: &str) -> bool {
        self.services
            .iter()
            .any(|(candidate, _)| *candidate == name)
    }
    fn service_actions(
        &self,
        name: &str,
    ) -> Option<Vec<crate::gateway::service_registry::ServiceActionInfo>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.services
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, actions)| actions.clone())
    }
    fn service_meta(&self, _name: &str) -> Option<&'static labby_primitives::plugin::PluginMeta> {
        None
    }
}

fn service_action(
    name: &'static str,
    destructive: bool,
    requires_admin: bool,
) -> crate::gateway::service_registry::ServiceActionInfo {
    crate::gateway::service_registry::ServiceActionInfo {
        name,
        description: if destructive { "danger" } else { "safe" },
        destructive,
        requires_admin,
    }
}

fn publication_registry(name: &'static str) -> Arc<PublicationRegistry> {
    Arc::new(PublicationRegistry::new(vec![(
        name,
        vec![
            service_action("z.action", true, true),
            service_action("a.action", false, false),
        ],
    )]))
}

#[test]
fn service_registry_publication_is_exact_sorted_and_materialized_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(PublicationRegistry::new(vec![
        (
            "bravo",
            vec![
                service_action("z.action", true, true),
                service_action("a.action", false, false),
            ],
        ),
        ("alpha", vec![]),
    ]));
    let reads = Arc::clone(&registry.reads);
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_builtin_service_registry(registry);
    let reads_after_publish = reads.load(Ordering::Relaxed);

    let snapshot = manager
        .published_service_registry_snapshot()
        .expect("valid catalog");
    assert_eq!(reads.load(Ordering::Relaxed), reads_after_publish);
    assert_eq!(snapshot.services()[0].name(), "alpha");
    assert_eq!(snapshot.services()[1].name(), "bravo");
    let actions = snapshot.services()[1].actions();
    assert_eq!(actions[0].name(), "a.action");
    assert_eq!(actions[0].description(), "safe");
    assert!(!actions[0].destructive());
    assert!(!actions[0].requires_admin());
    assert_eq!(actions[1].name(), "z.action");
    assert!(actions[1].destructive());
    assert!(actions[1].requires_admin());
}

#[test]
fn empty_service_registry_publishes_an_empty_catalog() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    assert!(
        manager
            .published_service_registry_snapshot()
            .expect("empty catalog")
            .services()
            .is_empty()
    );
}

#[test]
fn service_registry_generations_cover_identical_aba_clones_and_distinct_managers() {
    let first_dir = tempfile::tempdir().expect("first tempdir");
    let second_dir = tempfile::tempdir().expect("second tempdir");
    let manager = GatewayManager::new(
        first_dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_builtin_service_registry(publication_registry("alpha"));
    let clone = manager.clone();
    let first_a = manager
        .published_service_registry_snapshot()
        .expect("first a")
        .generation();
    manager.set_builtin_service_registry(publication_registry("alpha"));
    let identical_a = clone
        .published_service_registry_snapshot()
        .expect("identical a")
        .generation();
    manager.set_builtin_service_registry(publication_registry("bravo"));
    let b = manager
        .published_service_registry_snapshot()
        .expect("b")
        .generation();
    manager.set_builtin_service_registry(publication_registry("alpha"));
    let second_a = manager
        .published_service_registry_snapshot()
        .expect("second a")
        .generation();
    let distinct = GatewayManager::new(
        second_dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .published_service_registry_snapshot()
    .expect("distinct")
    .generation();
    assert_ne!(first_a, identical_a);
    assert_ne!(identical_a, b);
    assert_ne!(b, second_a);
    assert_ne!(first_a, second_a);
    assert_ne!(second_a, distinct);
}

#[test]
fn old_service_registry_snapshot_is_immutable_after_replacement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_builtin_service_registry(publication_registry("alpha"));
    let old = manager.published_service_registry_snapshot().expect("old");
    manager.set_builtin_service_registry(publication_registry("bravo"));
    let new = manager.published_service_registry_snapshot().expect("new");
    assert_eq!(old.services()[0].name(), "alpha");
    assert_eq!(new.services()[0].name(), "bravo");
    assert_ne!(old.generation(), new.generation());
}

#[test]
fn ambiguous_service_registry_publications_fail_closed() {
    use crate::gateway::service_registry::ServiceRegistryPublicationError;
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    manager.set_builtin_service_registry(Arc::new(PublicationRegistry::new(vec![
        ("dup", vec![]),
        ("dup", vec![]),
    ])));
    assert_eq!(
        manager.published_service_registry_snapshot().err(),
        Some(ServiceRegistryPublicationError::DuplicateService)
    );
    manager.set_builtin_service_registry(Arc::new(PublicationRegistry::new(vec![(
        "svc",
        vec![
            service_action("dup", false, false),
            service_action("dup", true, true),
        ],
    )])));
    assert_eq!(
        manager.published_service_registry_snapshot().err(),
        Some(ServiceRegistryPublicationError::DuplicateAction)
    );
}

struct InconsistentPublicationRegistry;
impl crate::registry::InProcessServiceRegistry for InconsistentPublicationRegistry {
    fn in_process_services(&self) -> Vec<Box<dyn crate::registry::InProcessService>> {
        Vec::new()
    }
}
impl crate::gateway::service_registry::GatewayServiceRegistry for InconsistentPublicationRegistry {
    fn service_names(&self) -> Vec<&'static str> {
        vec!["missing"]
    }
    fn contains_service(&self, _name: &str) -> bool {
        false
    }
    fn service_actions(
        &self,
        _name: &str,
    ) -> Option<Vec<crate::gateway::service_registry::ServiceActionInfo>> {
        None
    }
    fn service_meta(&self, _name: &str) -> Option<&'static labby_primitives::plugin::PluginMeta> {
        None
    }
}

struct OversizedPublicationRegistry;
impl crate::registry::InProcessServiceRegistry for OversizedPublicationRegistry {
    fn in_process_services(&self) -> Vec<Box<dyn crate::registry::InProcessService>> {
        Vec::new()
    }
}

struct OversizedActionsPublicationRegistry;
impl crate::registry::InProcessServiceRegistry for OversizedActionsPublicationRegistry {
    fn in_process_services(&self) -> Vec<Box<dyn crate::registry::InProcessService>> {
        Vec::new()
    }
}
impl crate::gateway::service_registry::GatewayServiceRegistry
    for OversizedActionsPublicationRegistry
{
    fn service_names(&self) -> Vec<&'static str> {
        vec!["service"]
    }
    fn contains_service(&self, _name: &str) -> bool {
        true
    }
    fn service_actions(
        &self,
        _name: &str,
    ) -> Option<Vec<crate::gateway::service_registry::ServiceActionInfo>> {
        Some(vec![service_action("action", false, false); 4097])
    }
    fn service_meta(&self, _name: &str) -> Option<&'static labby_primitives::plugin::PluginMeta> {
        None
    }
}
impl crate::gateway::service_registry::GatewayServiceRegistry for OversizedPublicationRegistry {
    fn service_names(&self) -> Vec<&'static str> {
        vec!["service"; 257]
    }
    fn contains_service(&self, _name: &str) -> bool {
        true
    }
    fn service_actions(
        &self,
        _name: &str,
    ) -> Option<Vec<crate::gateway::service_registry::ServiceActionInfo>> {
        Some(Vec::new())
    }
    fn service_meta(&self, _name: &str) -> Option<&'static labby_primitives::plugin::PluginMeta> {
        None
    }
}

#[test]
fn inconsistent_and_oversized_service_registries_fail_closed() {
    use crate::gateway::service_registry::ServiceRegistryPublicationError;
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    manager.set_builtin_service_registry(Arc::new(InconsistentPublicationRegistry));
    assert_eq!(
        manager.published_service_registry_snapshot().err(),
        Some(ServiceRegistryPublicationError::InvalidRegistry)
    );
    manager.set_builtin_service_registry(Arc::new(OversizedPublicationRegistry));
    assert_eq!(
        manager.published_service_registry_snapshot().err(),
        Some(ServiceRegistryPublicationError::TooLarge)
    );
    manager.set_builtin_service_registry(Arc::new(OversizedActionsPublicationRegistry));
    assert_eq!(
        manager.published_service_registry_snapshot().err(),
        Some(ServiceRegistryPublicationError::TooLarge)
    );
}

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

#[tokio::test]
async fn loadout_tool_catalog_is_exact_filtered_and_deterministic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_tests("bravo", healthy_entry_with_tool("bravo", "zulu"))
        .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "echo"))
        .await;
    pool.insert_entry_for_tests("excluded", healthy_entry_with_tool("excluded", "hidden"))
        .await;
    runtime.swap(Some(pool)).await;
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime);
    manager
        .seed_config(config_with_loadout(loadout(
            "project",
            &["bravo", "alpha", "ALPHA"],
        )))
        .await;

    let snapshot = manager
        .published_loadout_tool_catalog_snapshot("project")
        .await
        .expect("coherent catalog");
    let routes = snapshot
        .routes()
        .iter()
        .map(|route| (route.upstream_name.as_ref(), route.tool_name.as_ref()))
        .collect::<Vec<_>>();
    assert_eq!(routes, [("alpha", "echo"), ("bravo", "zulu")]);
    let runtime_generation = snapshot.runtime_config_generation();
    let pool_generation = snapshot.pool_publication_generation();
    let tool_generation = snapshot.tool_catalog_generation();
    assert_eq!(
        manager
            .published_loadout_tool_catalog_snapshot("project")
            .await
            .expect("stable catalog")
            .runtime_config_generation(),
        runtime_generation
    );
    assert_eq!(
        manager
            .published_loadout_tool_catalog_snapshot("project")
            .await
            .expect("stable catalog")
            .tool_catalog_generation(),
        tool_generation
    );
    assert_eq!(
        manager
            .published_loadout_tool_catalog_snapshot("project")
            .await
            .expect("stable catalog")
            .pool_publication_generation(),
        pool_generation
    );
}

#[tokio::test]
async fn loadout_tool_catalog_honors_false_empty_and_stable_missing_states() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "echo"))
        .await;
    runtime.swap(Some(pool)).await;
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime);
    let mut hidden = loadout("hidden", &["alpha"]);
    hidden.expose_tools = false;
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            loadouts: vec![hidden, loadout("empty", &[])],
            ..GatewayConfig::default()
        })
        .await;

    assert!(
        manager
            .published_loadout_tool_catalog_snapshot("hidden")
            .await
            .expect("hidden projection")
            .routes()
            .is_empty()
    );
    assert!(
        manager
            .published_loadout_tool_catalog_snapshot("empty")
            .await
            .expect("empty projection")
            .routes()
            .is_empty()
    );
    assert_eq!(
        manager
            .published_loadout_tool_catalog_snapshot("missing")
            .await
            .err(),
        Some(LoadoutToolCatalogPublicationError::MissingLoadout)
    );

    let no_pool = GatewayManager::new(
        dir.path().join("no-pool.toml"),
        GatewayRuntimeHandle::default(),
    );
    no_pool
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    assert_eq!(
        no_pool
            .published_loadout_tool_catalog_snapshot("project")
            .await
            .err(),
        Some(LoadoutToolCatalogPublicationError::MissingPool)
    );

    let invalid_runtime = GatewayRuntimeHandle::default();
    let invalid_pool = Arc::new(UpstreamPool::new());
    invalid_pool
        .insert_entry_for_tests("alpha", healthy_entry_with_tool("bravo", "echo"))
        .await;
    invalid_runtime.swap(Some(invalid_pool)).await;
    let invalid = GatewayManager::new(dir.path().join("invalid.toml"), invalid_runtime);
    invalid
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    assert_eq!(
        invalid
            .published_loadout_tool_catalog_snapshot("project")
            .await
            .err(),
        Some(LoadoutToolCatalogPublicationError::CatalogUnavailable)
    );
}

#[tokio::test]
async fn loadout_tool_catalog_retries_catalog_and_manager_publication_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "old"))
        .await;
    pool.insert_entry_for_tests("bravo", healthy_entry_with_tool("bravo", "new"))
        .await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime);
    manager
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;

    let changing_pool = Arc::clone(&pool);
    let after_catalog_change = manager
        .compose_loadout_tool_catalog("project", move |attempt| {
            let pool = Arc::clone(&changing_pool);
            async move {
                if attempt == 0 {
                    pool.insert_entry_for_tests(
                        "alpha",
                        healthy_entry_with_tool("alpha", "replacement"),
                    )
                    .await;
                }
            }
        })
        .await
        .expect("catalog change should retry");
    assert_eq!(
        after_catalog_change.routes()[0].tool_name.as_ref(),
        "replacement"
    );

    let changing_manager = manager.clone();
    let after_manager_change = manager
        .compose_loadout_tool_catalog("project", move |attempt| {
            let manager = changing_manager.clone();
            async move {
                if attempt == 0 {
                    manager
                        .seed_config(config_with_loadout(loadout("project", &["bravo"])))
                        .await;
                }
            }
        })
        .await
        .expect("manager change should retry");
    assert_eq!(
        after_manager_change.routes()[0].upstream_name.as_ref(),
        "bravo"
    );
}

#[tokio::test]
async fn loadout_tool_catalog_fails_closed_under_sustained_manager_churn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "a"))
        .await;
    pool.insert_entry_for_tests("bravo", healthy_entry_with_tool("bravo", "b"))
        .await;
    runtime.swap(Some(pool)).await;
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime);
    manager
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;

    let changing_manager = manager.clone();
    let result = manager
        .compose_loadout_tool_catalog("project", move |attempt| {
            let manager = changing_manager.clone();
            async move {
                let upstream = if attempt % 2 == 0 { "bravo" } else { "alpha" };
                manager
                    .seed_config(config_with_loadout(loadout("project", &[upstream])))
                    .await;
            }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(LoadoutToolCatalogPublicationError::Unstable)
    );
}

#[tokio::test]
async fn loadout_tool_catalog_retries_pool_aba_and_binds_final_publication() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let original = Arc::new(UpstreamPool::new());
    original
        .insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "original"))
        .await;
    let transient = Arc::new(UpstreamPool::new());
    transient
        .insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "transient"))
        .await;
    runtime.swap(Some(Arc::clone(&original))).await;
    let initial_generation = runtime.published_pool_snapshot().generation();
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime.clone());
    manager
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;

    let swap_runtime = runtime.clone();
    let republished_original = Arc::clone(&original);
    let result = manager
        .compose_loadout_tool_catalog("project", move |attempt| {
            let runtime = swap_runtime.clone();
            let transient = Arc::clone(&transient);
            let original = Arc::clone(&republished_original);
            async move {
                if attempt == 0 {
                    runtime.swap(Some(transient)).await;
                    runtime.swap(Some(original)).await;
                }
            }
        })
        .await
        .expect("pool ABA should retry to a coherent publication");

    assert_eq!(result.routes()[0].tool_name.as_ref(), "original");
    assert_ne!(result.pool_publication_generation(), initial_generation);
    assert_eq!(
        result.pool_publication_generation(),
        runtime.published_pool_snapshot().generation()
    );
}
