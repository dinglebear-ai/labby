//! Published runtime Loadout snapshot and generation tests.

use crate::gateway::config::write_gateway_config;
use crate::gateway::manager::{
    LoadoutMcpCatalogPublicationError, LoadoutPromptCatalogPublicationError,
    LoadoutResourceCatalogPublicationError, LoadoutResourceTemplateCatalogPublicationError,
    LoadoutToolCatalogPublicationError,
};
use labby_runtime::gateway_config::{
    GatewayLoadoutConfig, ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig,
    ProtectedMcpRouteTarget, VirtualServerConfig, VirtualServerMcpPolicyConfig,
    VirtualServerSurfacesConfig,
};
use rmcp::model::{Prompt, Resource, ResourceTemplate};

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct PublicationRegistry {
    services: Vec<(
        &'static str,
        Vec<crate::gateway::service_registry::ServiceActionInfo>,
    )>,
    reads: Arc<AtomicUsize>,
}

#[tokio::test]
async fn loadout_prompt_catalog_filters_orders_and_preserves_exact_generations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_prompt_routes_for_tests(
        "zeta",
        vec![
            Prompt::new("beta", Some("exact metadata"), None),
            Prompt::new("alpha", Some(""), None),
        ],
    )
    .await;
    pool.insert_prompt_routes_for_tests(
        "alpha",
        vec![Prompt::new("remote", Some("remote metadata"), None)],
    )
    .await;
    pool.insert_prompt_routes_for_tests("excluded", vec![Prompt::new("hidden", Some(""), None)])
        .await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("prompts.toml"), runtime.clone());
    let mut hidden = loadout("hidden", &["alpha"]);
    hidden.expose_prompts = false;
    let mut config = config_with_loadout(loadout("project", &["zeta", "alpha"]));
    config.loadouts.push(hidden);
    manager.seed_config(config).await;

    let snapshot = manager
        .published_loadout_prompt_catalog_snapshot("project")
        .await
        .expect("prompt snapshot");
    assert_eq!(
        snapshot.runtime_config_generation(),
        manager
            .published_runtime_loadout_snapshot("project")
            .await
            .generation()
    );
    assert_eq!(
        snapshot.pool_publication_generation(),
        runtime.published_pool_snapshot().generation()
    );
    assert_eq!(
        snapshot.prompt_catalog_generation(),
        pool.published_prompt_catalog()
            .await
            .expect("pool prompt publication")
            .generation()
    );
    assert_eq!(
        snapshot
            .routes()
            .iter()
            .map(|route| (route.upstream_name.as_ref(), route.native_name.as_ref()))
            .collect::<Vec<_>>(),
        vec![("alpha", "remote"), ("zeta", "alpha"), ("zeta", "beta")]
    );
    assert_eq!(
        snapshot.routes()[0].prompt.description.as_deref(),
        Some("remote metadata")
    );
    assert!(
        manager
            .published_loadout_prompt_catalog_snapshot("hidden")
            .await
            .expect("hidden")
            .routes()
            .is_empty()
    );
}

#[tokio::test]
async fn loadout_prompt_catalog_redacts_errors_and_bounds_churn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let no_pool = GatewayManager::new(
        dir.path().join("none.toml"),
        GatewayRuntimeHandle::default(),
    );
    no_pool
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    assert_eq!(
        no_pool
            .published_loadout_prompt_catalog_snapshot("project")
            .await
            .err(),
        Some(LoadoutPromptCatalogPublicationError::MissingPool)
    );

    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_prompt_routes_for_tests("alpha", vec![Prompt::new("row", Some(""), None)])
        .await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("churn.toml"), runtime.clone());
    manager
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    assert_eq!(
        manager
            .published_loadout_prompt_catalog_snapshot("missing")
            .await
            .err(),
        Some(LoadoutPromptCatalogPublicationError::MissingLoadout)
    );
    pool.insert_prompt_routes_for_tests(
        "alpha",
        vec![
            Prompt::new("duplicate", Some("one"), None),
            Prompt::new("duplicate", Some("two"), None),
        ],
    )
    .await;
    assert_eq!(
        manager
            .published_loadout_prompt_catalog_snapshot("project")
            .await
            .err(),
        Some(LoadoutPromptCatalogPublicationError::CatalogUnavailable)
    );
    pool.insert_prompt_routes_for_tests("alpha", vec![Prompt::new("row", Some(""), None)])
        .await;

    let churning_pool = Arc::clone(&pool);
    let result = manager
        .compose_loadout_prompt_catalog("project", move |attempt| {
            let pool = Arc::clone(&churning_pool);
            async move {
                pool.insert_prompt_routes_for_tests(
                    "alpha",
                    vec![Prompt::new(format!("churn-{attempt}"), Some(""), None)],
                )
                .await;
            }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(LoadoutPromptCatalogPublicationError::Unstable)
    );

    let changing = manager.clone();
    let result = manager
        .compose_loadout_prompt_catalog("project", move |_| {
            let manager = changing.clone();
            async move {
                manager
                    .seed_config(config_with_loadout(loadout("project", &["alpha"])))
                    .await;
            }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(LoadoutPromptCatalogPublicationError::Unstable)
    );

    let transient = Arc::new(UpstreamPool::new());
    transient
        .insert_prompt_routes_for_tests("alpha", vec![Prompt::new("other", Some(""), None)])
        .await;
    let changing_runtime = runtime.clone();
    let result = manager
        .compose_loadout_prompt_catalog("project", move |attempt| {
            let runtime = changing_runtime.clone();
            let next = if attempt % 2 == 0 {
                Arc::clone(&transient)
            } else {
                Arc::clone(&pool)
            };
            async move { runtime.swap(Some(next)).await }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(LoadoutPromptCatalogPublicationError::Unstable)
    );
}

#[tokio::test]
async fn loadout_prompt_catalog_retries_prompt_and_pool_aba_independently() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let original = Arc::new(UpstreamPool::new());
    original
        .insert_prompt_routes_for_tests("alpha", vec![Prompt::new("a", Some("A"), None)])
        .await;
    runtime.swap(Some(Arc::clone(&original))).await;
    let manager = GatewayManager::new(dir.path().join("prompt-aba.toml"), runtime.clone());
    manager
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    let initial_prompt = original
        .published_prompt_catalog()
        .await
        .expect("initial")
        .generation();
    let initial_pool = runtime.published_pool_snapshot().generation();
    let transient = Arc::new(UpstreamPool::new());
    transient
        .insert_prompt_routes_for_tests("alpha", vec![Prompt::new("transient", Some(""), None)])
        .await;
    let hook_pool = Arc::clone(&original);
    let hook_runtime = runtime.clone();
    let hook_transient = Arc::clone(&transient);
    let hook_original = Arc::clone(&original);
    let snapshot = manager
        .compose_loadout_prompt_catalog("project", move |attempt| {
            let pool = Arc::clone(&hook_pool);
            let runtime = hook_runtime.clone();
            let transient = Arc::clone(&hook_transient);
            let original = Arc::clone(&hook_original);
            async move {
                if attempt == 0 {
                    pool.insert_prompt_routes_for_tests(
                        "alpha",
                        vec![Prompt::new("b", Some("B"), None)],
                    )
                    .await;
                    pool.insert_prompt_routes_for_tests(
                        "alpha",
                        vec![Prompt::new("a", Some("A"), None)],
                    )
                    .await;
                } else if attempt == 1 {
                    runtime.swap(Some(transient)).await;
                    runtime.swap(Some(original)).await;
                }
            }
        })
        .await
        .expect("ABA retries");
    assert_eq!(snapshot.routes()[0].native_name.as_ref(), "a");
    assert_ne!(snapshot.prompt_catalog_generation(), initial_prompt);
    assert_ne!(snapshot.pool_publication_generation(), initial_pool);
    assert_eq!(
        snapshot.prompt_catalog_generation(),
        original
            .published_prompt_catalog()
            .await
            .expect("final")
            .generation()
    );
    assert_eq!(
        snapshot.pool_publication_generation(),
        runtime.published_pool_snapshot().generation()
    );
}

#[tokio::test]
async fn loadout_prompt_catalog_retries_config_aba_independently() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_prompt_routes_for_tests("alpha", vec![Prompt::new("a", Some(""), None)])
        .await;
    runtime.swap(Some(pool)).await;
    let manager = GatewayManager::new(dir.path().join("prompt-config-aba.toml"), runtime);
    manager
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    let initial = manager
        .published_runtime_loadout_snapshot("project")
        .await
        .generation();
    let changing = manager.clone();
    let snapshot = manager
        .compose_loadout_prompt_catalog("project", move |attempt| {
            let manager = changing.clone();
            async move {
                if attempt == 0 {
                    manager
                        .seed_config(config_with_loadout(loadout("project", &["bravo"])))
                        .await;
                    manager
                        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
                        .await;
                }
            }
        })
        .await
        .expect("config ABA retries");
    let current = manager
        .published_runtime_loadout_snapshot("project")
        .await
        .generation();
    assert_ne!(initial, current);
    assert_eq!(snapshot.runtime_config_generation(), current);
    assert_eq!(snapshot.routes().len(), 1);
}

#[tokio::test]
async fn loadout_resource_template_catalog_filters_upstreams_and_expose_resources() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_resource_template_routes_for_tests(
        "alpha",
        vec![ResourceTemplate::new("file:///alpha/{id}", "alpha").with_description("metadata")],
    )
    .await;
    pool.insert_resource_template_routes_for_tests(
        "bravo",
        vec![ResourceTemplate::new("file:///bravo/{id}", "bravo")],
    )
    .await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("templates.toml"), runtime.clone());
    let mut hidden = loadout("hidden", &["alpha"]);
    hidden.expose_resources = false;
    hidden.expose_skills = false;
    let mut config = config_with_loadout(loadout("project", &["bravo", "alpha"]));
    config.loadouts.push(hidden);
    manager.seed_config(config).await;
    let snapshot = manager
        .published_loadout_resource_template_catalog_snapshot("project")
        .await
        .expect("template snapshot");
    assert_eq!(
        snapshot.runtime_config_generation(),
        manager
            .published_runtime_loadout_snapshot("project")
            .await
            .generation()
    );
    assert_eq!(
        snapshot.pool_publication_generation(),
        runtime.published_pool_snapshot().generation()
    );
    assert_eq!(
        snapshot.resource_template_catalog_generation(),
        pool.published_resource_template_catalog()
            .await
            .expect("pool template publication")
            .generation()
    );
    assert_eq!(snapshot.routes().len(), 2);
    assert_eq!(snapshot.routes()[0].upstream_name.as_ref(), "alpha");
    assert_eq!(
        snapshot.routes()[0].native_uri_template.as_ref(),
        "file:///alpha/{id}"
    );
    assert_eq!(
        snapshot.routes()[0].template.description.as_deref(),
        Some("metadata")
    );
    assert_eq!(snapshot.routes()[1].upstream_name.as_ref(), "bravo");
    assert_eq!(
        snapshot.routes()[1].native_uri_template.as_ref(),
        "file:///bravo/{id}"
    );
    assert!(
        manager
            .published_loadout_resource_template_catalog_snapshot("hidden")
            .await
            .expect("hidden")
            .routes()
            .is_empty()
    );
}

#[tokio::test]
async fn loadout_resource_template_catalog_redacts_errors_and_bounds_config_churn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let no_pool = GatewayManager::new(
        dir.path().join("none.toml"),
        GatewayRuntimeHandle::default(),
    );
    no_pool
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    assert_eq!(
        no_pool
            .published_loadout_resource_template_catalog_snapshot("project")
            .await
            .err(),
        Some(LoadoutResourceTemplateCatalogPublicationError::MissingPool)
    );
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_resource_template_routes_for_tests(
        "alpha",
        vec![ResourceTemplate::new("file:///{id}", "row")],
    )
    .await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("churn.toml"), runtime);
    manager
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    assert_eq!(
        manager
            .published_loadout_resource_template_catalog_snapshot("missing")
            .await
            .err(),
        Some(LoadoutResourceTemplateCatalogPublicationError::MissingLoadout)
    );
    let changing = manager.clone();
    let result = manager
        .compose_loadout_resource_template_catalog("project", move |_| {
            let manager = changing.clone();
            async move {
                manager
                    .seed_config(config_with_loadout(loadout("project", &["alpha"])))
                    .await;
            }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(LoadoutResourceTemplateCatalogPublicationError::Unstable)
    );
    pool.insert_resource_template_routes_for_tests(
        "alpha",
        vec![
            ResourceTemplate::new("file:///dup/{id}", "one"),
            ResourceTemplate::new("file:///dup/{id}", "two"),
        ],
    )
    .await;
    assert_eq!(
        manager
            .published_loadout_resource_template_catalog_snapshot("project")
            .await
            .err(),
        Some(LoadoutResourceTemplateCatalogPublicationError::CatalogUnavailable)
    );
    let churning_pool = Arc::clone(&pool);
    let result = manager
        .compose_loadout_resource_template_catalog("project", move |attempt| {
            let pool = Arc::clone(&churning_pool);
            async move {
                pool.insert_resource_template_routes_for_tests(
                    "alpha",
                    vec![ResourceTemplate::new(
                        format!("file:///churn/{attempt}/{{id}}"),
                        "churn",
                    )],
                )
                .await;
            }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(LoadoutResourceTemplateCatalogPublicationError::Unstable)
    );
}

#[tokio::test]
async fn loadout_resource_template_catalog_retries_template_and_pool_aba() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let original = Arc::new(UpstreamPool::new());
    original
        .insert_resource_template_routes_for_tests(
            "alpha",
            vec![ResourceTemplate::new("file:///a/{id}", "A")],
        )
        .await;
    runtime.swap(Some(Arc::clone(&original))).await;
    let manager = GatewayManager::new(dir.path().join("template-aba.toml"), runtime.clone());
    manager
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    let initial_template = original
        .published_resource_template_catalog()
        .await
        .expect("initial")
        .generation();
    let initial_pool = runtime.published_pool_snapshot().generation();
    let transient = Arc::new(UpstreamPool::new());
    transient
        .insert_resource_template_routes_for_tests(
            "alpha",
            vec![ResourceTemplate::new("file:///transient/{id}", "transient")],
        )
        .await;
    let hook_pool = Arc::clone(&original);
    let hook_runtime = runtime.clone();
    let hook_transient = Arc::clone(&transient);
    let hook_original = Arc::clone(&original);
    let snapshot = manager
        .compose_loadout_resource_template_catalog("project", move |attempt| {
            let pool = Arc::clone(&hook_pool);
            let runtime = hook_runtime.clone();
            let transient = Arc::clone(&hook_transient);
            let original = Arc::clone(&hook_original);
            async move {
                if attempt == 0 {
                    pool.insert_resource_template_routes_for_tests(
                        "alpha",
                        vec![ResourceTemplate::new("file:///b/{id}", "B")],
                    )
                    .await;
                    pool.insert_resource_template_routes_for_tests(
                        "alpha",
                        vec![ResourceTemplate::new("file:///a/{id}", "A")],
                    )
                    .await;
                } else if attempt == 1 {
                    runtime.swap(Some(transient)).await;
                    runtime.swap(Some(original)).await;
                }
            }
        })
        .await
        .expect("ABA retries");
    assert_eq!(
        snapshot.routes()[0].native_uri_template.as_ref(),
        "file:///a/{id}"
    );
    assert_ne!(
        snapshot.resource_template_catalog_generation(),
        initial_template
    );
    assert_ne!(snapshot.pool_publication_generation(), initial_pool);
    assert_eq!(
        snapshot.resource_template_catalog_generation(),
        original
            .published_resource_template_catalog()
            .await
            .expect("final")
            .generation()
    );
    assert_eq!(
        snapshot.pool_publication_generation(),
        runtime.published_pool_snapshot().generation()
    );
    assert_eq!(
        snapshot.runtime_config_generation(),
        manager
            .published_runtime_loadout_snapshot("project")
            .await
            .generation()
    );
}

#[tokio::test]
async fn loadout_resource_template_catalog_retries_config_aba() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_resource_template_routes_for_tests(
        "alpha",
        vec![ResourceTemplate::new("file:///a/{id}", "A")],
    )
    .await;
    runtime.swap(Some(pool)).await;
    let manager = GatewayManager::new(dir.path().join("template-config-aba.toml"), runtime);
    manager
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    let initial = manager
        .published_runtime_loadout_snapshot("project")
        .await
        .generation();
    let changing = manager.clone();
    let snapshot = manager
        .compose_loadout_resource_template_catalog("project", move |attempt| {
            let manager = changing.clone();
            async move {
                if attempt == 0 {
                    manager
                        .seed_config(config_with_loadout(loadout("project", &["bravo"])))
                        .await;
                    manager
                        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
                        .await;
                }
            }
        })
        .await
        .expect("config ABA retries");
    let current = manager
        .published_runtime_loadout_snapshot("project")
        .await
        .generation();
    assert_ne!(initial, current);
    assert_eq!(snapshot.runtime_config_generation(), current);
    assert_eq!(snapshot.routes().len(), 1);
}

#[tokio::test]
async fn loadout_resource_catalog_filters_exact_upstreams_and_preserves_metadata() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_resource_routes_for_tests(
        "alpha",
        vec![Resource::new("file:///alpha", "alpha").with_description("metadata")],
    )
    .await;
    pool.insert_resource_routes_for_tests("bravo", vec![Resource::new("file:///bravo", "bravo")])
        .await;
    runtime.swap(Some(pool)).await;
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime);
    let mut hidden = loadout("hidden", &["alpha"]);
    hidden.expose_resources = false;
    hidden.expose_skills = false;
    let mut config = config_with_loadout(loadout("project", &["alpha"]));
    config.loadouts.push(hidden);
    manager.seed_config(config).await;
    let snapshot = manager
        .published_loadout_resource_catalog_snapshot("project")
        .await
        .expect("resource snapshot");
    assert_eq!(snapshot.routes().len(), 1);
    assert_eq!(snapshot.routes()[0].upstream_name.as_ref(), "alpha");
    assert_eq!(snapshot.routes()[0].native_uri.as_ref(), "file:///alpha");
    assert_eq!(
        snapshot.routes()[0].resource.description.as_deref(),
        Some("metadata")
    );
    assert!(
        manager
            .published_loadout_resource_catalog_snapshot("hidden")
            .await
            .expect("hidden")
            .routes()
            .is_empty()
    );
}

#[tokio::test]
async fn loadout_resource_catalog_redacts_missing_states_and_bounds_churn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let no_pool = GatewayManager::new(
        dir.path().join("none.toml"),
        GatewayRuntimeHandle::default(),
    );
    no_pool
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    assert_eq!(
        no_pool
            .published_loadout_resource_catalog_snapshot("project")
            .await
            .err(),
        Some(LoadoutResourceCatalogPublicationError::MissingPool)
    );

    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_resource_routes_for_tests("alpha", vec![Resource::new("file:///alpha", "alpha")])
        .await;
    runtime.swap(Some(pool)).await;
    let manager = GatewayManager::new(dir.path().join("churn.toml"), runtime);
    manager
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    assert_eq!(
        manager
            .published_loadout_resource_catalog_snapshot("missing")
            .await
            .err(),
        Some(LoadoutResourceCatalogPublicationError::MissingLoadout)
    );
    let changing = manager.clone();
    let result = manager
        .compose_loadout_resource_catalog("project", move |_| {
            let manager = changing.clone();
            async move {
                manager
                    .seed_config(config_with_loadout(loadout("project", &["alpha"])))
                    .await;
            }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(LoadoutResourceCatalogPublicationError::Unstable)
    );

    let invalid_runtime = GatewayRuntimeHandle::default();
    let invalid_pool = Arc::new(UpstreamPool::new());
    invalid_pool
        .insert_resource_routes_for_tests(
            "alpha",
            vec![
                Resource::new("file:///dup", "one"),
                Resource::new("file:///dup", "two"),
            ],
        )
        .await;
    invalid_runtime.swap(Some(invalid_pool)).await;
    let invalid = GatewayManager::new(dir.path().join("invalid.toml"), invalid_runtime);
    invalid
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    assert_eq!(
        invalid
            .published_loadout_resource_catalog_snapshot("project")
            .await
            .err(),
        Some(LoadoutResourceCatalogPublicationError::CatalogUnavailable)
    );
}

#[tokio::test]
async fn loadout_resource_catalog_retries_resource_and_pool_aba() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let original = Arc::new(UpstreamPool::new());
    original
        .insert_resource_routes_for_tests("alpha", vec![Resource::new("file:///a", "A")])
        .await;
    runtime.swap(Some(Arc::clone(&original))).await;
    let manager = GatewayManager::new(dir.path().join("aba.toml"), runtime.clone());
    manager
        .seed_config(config_with_loadout(loadout("project", &["alpha"])))
        .await;
    let initial_resource = original
        .published_resource_catalog()
        .await
        .expect("initial")
        .generation();
    let initial_pool = runtime.published_pool_snapshot().generation();
    let transient = Arc::new(UpstreamPool::new());
    transient
        .insert_resource_routes_for_tests(
            "alpha",
            vec![Resource::new("file:///transient", "transient")],
        )
        .await;
    let hook_pool = Arc::clone(&original);
    let hook_runtime = runtime.clone();
    let hook_transient = Arc::clone(&transient);
    let hook_original = Arc::clone(&original);
    let snapshot = manager
        .compose_loadout_resource_catalog("project", move |attempt| {
            let pool = Arc::clone(&hook_pool);
            let runtime = hook_runtime.clone();
            let transient = Arc::clone(&hook_transient);
            let original = Arc::clone(&hook_original);
            async move {
                if attempt == 0 {
                    pool.insert_resource_routes_for_tests(
                        "alpha",
                        vec![Resource::new("file:///b", "B")],
                    )
                    .await;
                    pool.insert_resource_routes_for_tests(
                        "alpha",
                        vec![Resource::new("file:///a", "A")],
                    )
                    .await;
                } else if attempt == 1 {
                    runtime.swap(Some(transient)).await;
                    runtime.swap(Some(original)).await;
                }
            }
        })
        .await
        .expect("ABA retries");
    assert_eq!(snapshot.routes()[0].native_uri.as_ref(), "file:///a");
    assert_ne!(snapshot.resource_catalog_generation(), initial_resource);
    assert_ne!(snapshot.pool_publication_generation(), initial_pool);
    assert_eq!(
        snapshot.resource_catalog_generation(),
        original
            .published_resource_catalog()
            .await
            .expect("final")
            .generation()
    );
    assert_eq!(
        snapshot.pool_publication_generation(),
        runtime.published_pool_snapshot().generation()
    );
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

fn ordered_publication_registry() -> Arc<PublicationRegistry> {
    Arc::new(PublicationRegistry::new(vec![
        ("zulu", vec![]),
        ("alpha", vec![]),
    ]))
}

fn mcp_virtual_server(id: &str, service: &str, allowed: &[&str]) -> VirtualServerConfig {
    VirtualServerConfig {
        id: id.to_string(),
        service: service.to_string(),
        enabled: true,
        surfaces: VirtualServerSurfacesConfig {
            mcp: true,
            ..Default::default()
        },
        mcp_policy: (!allowed.is_empty()).then(|| VirtualServerMcpPolicyConfig {
            allowed_actions: allowed.iter().map(|action| (*action).to_string()).collect(),
        }),
    }
}

fn project_route_config(project: &str, loadout_ref: Option<&str>) -> GatewayConfig {
    let named = loadout_ref.is_some();
    GatewayConfig {
        loadouts: vec![GatewayLoadoutConfig {
            name: "production".into(),
            upstreams: vec!["alpha".into(), "bravo".into()],
            services: vec!["setup".into(), "gateway".into()],
            expose_code_mode: true,
            ..Default::default()
        }],
        virtual_servers: vec![
            mcp_virtual_server("setup", "setup", &[]),
            mcp_virtual_server("gateway", "gateway", &[]),
        ],
        protected_mcp_routes: vec![ProtectedMcpRouteConfig {
            name: "project-route".into(),
            enabled: true,
            public_host: "MCP.Example.com.".into(),
            public_path: "/project".into(),
            upstream: None,
            backend_url: String::new(),
            backend_mcp_path: "/mcp".into(),
            scopes: vec![],
            health_path: None,
            target: Some(ProtectedMcpRouteTarget::GatewaySubset(
                ProtectedGatewaySubsetTarget {
                    project_id: Some(project.into()),
                    loadout: loadout_ref.map(str::to_string),
                    upstreams: if named {
                        Vec::new()
                    } else {
                        vec!["alpha".into()]
                    },
                    services: if named {
                        Vec::new()
                    } else {
                        vec!["setup".into()]
                    },
                    expose_code_mode: false,
                },
            )),
        }],
        ..Default::default()
    }
}

#[tokio::test]
async fn project_route_publication_binds_canonical_identity_and_narrows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    manager
        .seed_config_unchecked_for_tests(project_route_config("project-a", None))
        .await;
    let snapshot = manager
        .published_project_route_snapshot("project-route", "project-a", "production")
        .await
        .expect("route");
    assert_eq!(snapshot.resource(), "https://mcp.example.com/project");
    assert_eq!(snapshot.project_id(), "project-a");
    assert_eq!(snapshot.effective_loadout().upstreams, vec!["alpha"]);
    assert_eq!(snapshot.effective_loadout().services, vec!["setup"]);
    assert_eq!(
        snapshot
            .effective_service_names()
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<&str>>(),
        vec!["setup"]
    );
    assert!(!snapshot.effective_loadout().expose_code_mode);
    assert!(snapshot.effective_loadout().expose_tools);
}

#[tokio::test]
async fn project_route_publication_canonicalizes_service_alias_and_rejects_collision() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let mut config = project_route_config("project-a", Some("production"));
    config.loadouts[0].services = vec!["setup-primary".into()];
    config.virtual_servers = vec![mcp_virtual_server("setup-primary", "setup", &[])];
    manager
        .seed_config_unchecked_for_tests(config.clone())
        .await;
    let snapshot = manager
        .published_project_route_snapshot("project-route", "project-a", "production")
        .await
        .expect("aliased route");
    assert_eq!(snapshot.effective_service_names()[0].as_ref(), "setup");

    config.loadouts[0].services.push("setup".into());
    manager.seed_config_unchecked_for_tests(config).await;
    assert_eq!(
        manager
            .published_project_route_snapshot("project-route", "project-a", "production")
            .await
            .err(),
        Some(crate::gateway::manager::ProjectRoutePublicationError::Unavailable)
    );
}

#[tokio::test]
async fn project_route_publication_redacts_failures_and_retries_aba() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    manager
        .seed_config_unchecked_for_tests(project_route_config("project-a", Some("production")))
        .await;
    assert_eq!(
        manager
            .published_project_route_snapshot("project-route", "project-b", "production")
            .await
            .err(),
        Some(crate::gateway::manager::ProjectRoutePublicationError::Unavailable)
    );
    assert_eq!(
        manager
            .published_project_route_snapshot("project-route", "project-a", "wrong")
            .await
            .err(),
        Some(crate::gateway::manager::ProjectRoutePublicationError::Unavailable)
    );
    let changing = manager.clone();
    let snapshot = manager
        .compose_project_route_snapshot(
            "project-route",
            "project-a",
            "production",
            move |attempt| {
                let manager = changing.clone();
                async move {
                    if attempt == 0 {
                        manager
                            .seed_config_unchecked_for_tests(project_route_config(
                                "project-b",
                                Some("production"),
                            ))
                            .await;
                        manager
                            .seed_config_unchecked_for_tests(project_route_config(
                                "project-a",
                                Some("production"),
                            ))
                            .await;
                    }
                }
            },
        )
        .await
        .expect("ABA retry");
    assert_eq!(snapshot.project_id(), "project-a");
}

#[tokio::test]
async fn project_route_publication_rejects_unavailable_and_bounds_churn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let mut disabled = project_route_config("project-a", None);
    disabled.protected_mcp_routes[0].enabled = false;
    manager.seed_config_unchecked_for_tests(disabled).await;
    assert_eq!(
        manager
            .published_project_route_snapshot("project-route", "project-a", "production")
            .await
            .err(),
        Some(crate::gateway::manager::ProjectRoutePublicationError::Unavailable)
    );

    let mut duplicate = project_route_config("project-a", None);
    let mut alias = duplicate.protected_mcp_routes[0].clone();
    alias.name = "alias".into();
    alias.public_host = "mcp.example.com:443".into();
    duplicate.protected_mcp_routes.push(alias);
    manager.seed_config_unchecked_for_tests(duplicate).await;
    assert_eq!(
        manager
            .published_project_route_snapshot("project-route", "project-a", "production")
            .await
            .err(),
        Some(crate::gateway::manager::ProjectRoutePublicationError::Unavailable)
    );

    manager
        .seed_config_unchecked_for_tests(project_route_config("project-a", None))
        .await;
    let changing = manager.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let result = manager
        .compose_project_route_snapshot("project-route", "project-a", "production", move |_| {
            let manager = changing.clone();
            let observed = Arc::clone(&observed);
            async move {
                observed.fetch_add(1, Ordering::SeqCst);
                manager
                    .seed_config_unchecked_for_tests(project_route_config("project-a", None))
                    .await;
            }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(crate::gateway::manager::ProjectRoutePublicationError::Unstable)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn project_route_publication_failures_are_non_enumerating() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let mut cases = Vec::new();

    let mut missing_project = project_route_config("project-a", None);
    let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) =
        missing_project.protected_mcp_routes[0].target.as_mut()
    else {
        unreachable!()
    };
    target.project_id = None;
    cases.push(missing_project);

    let mut non_gateway = project_route_config("project-a", None);
    non_gateway.protected_mcp_routes[0].target = None;
    cases.push(non_gateway);

    let mut duplicate_name = project_route_config("project-a", None);
    let mut duplicate = duplicate_name.protected_mcp_routes[0].clone();
    duplicate.public_path = "/other".into();
    duplicate_name.protected_mcp_routes.push(duplicate);
    cases.push(duplicate_name);

    let mut duplicate_loadout = project_route_config("project-a", None);
    duplicate_loadout
        .loadouts
        .push(duplicate_loadout.loadouts[0].clone());
    cases.push(duplicate_loadout);

    for config in cases {
        manager.seed_config_unchecked_for_tests(config).await;
        let error = manager
            .published_project_route_snapshot("project-route", "project-a", "production")
            .await
            .err()
            .expect("semantic failure");
        assert_eq!(
            error,
            crate::gateway::manager::ProjectRoutePublicationError::Unavailable
        );
        assert_eq!(
            error.to_string(),
            "project route publication is unavailable"
        );
    }

    manager
        .seed_config_unchecked_for_tests(project_route_config("project-a", None))
        .await;
    let missing = manager
        .published_project_route_snapshot("missing", "project-a", "production")
        .await
        .err()
        .expect("missing route");
    assert_eq!(
        missing.to_string(),
        "project route publication is unavailable"
    );
}

#[tokio::test]
async fn project_route_publication_does_not_mutate_old_snapshots() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    manager
        .seed_config_unchecked_for_tests(project_route_config("project-a", None))
        .await;
    let old = manager
        .published_project_route_snapshot("project-route", "project-a", "production")
        .await
        .expect("old snapshot");
    let mut changed = project_route_config("project-a", None);
    changed.loadouts[0].upstreams = vec!["charlie".into()];
    manager.seed_config_unchecked_for_tests(changed).await;
    assert_eq!(old.effective_loadout().upstreams, vec!["alpha"]);
}

fn unified_config(upstream: &str) -> GatewayConfig {
    let mut selected = loadout("project", &[upstream]);
    selected.services = vec!["deploy".to_string()];
    GatewayConfig {
        loadouts: vec![selected],
        virtual_servers: vec![mcp_virtual_server("deploy", "deploy", &["a.action"])],
        ..Default::default()
    }
}

#[tokio::test]
async fn unified_loadout_mcp_catalog_binds_exact_common_interval() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "echo"))
        .await;
    pool.insert_resource_routes_for_tests(
        "alpha",
        vec![Resource::new("file:///alpha", "alpha").with_description("metadata")],
    )
    .await;
    pool.insert_resource_template_routes_for_tests(
        "alpha",
        vec![
            ResourceTemplate::new("file:///alpha/{id}", "template")
                .with_description("template metadata"),
        ],
    )
    .await;
    pool.insert_prompt_routes_for_tests(
        "alpha",
        vec![Prompt::new("deploy", Some("prompt metadata"), None)],
    )
    .await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime.clone())
        .with_builtin_service_registry(publication_registry("deploy"));
    manager
        .seed_config_unchecked_for_tests(unified_config("alpha"))
        .await;
    let snapshot = manager
        .published_loadout_mcp_catalog_snapshot("project")
        .await
        .expect("unified");
    assert_eq!(snapshot.tools().routes()[0].tool_name.as_ref(), "echo");
    assert_eq!(snapshot.services().services()[0].name(), "deploy");
    assert_eq!(
        snapshot.resources().routes()[0]
            .resource
            .description
            .as_deref(),
        Some("metadata")
    );
    assert_eq!(
        snapshot.resource_templates().routes()[0]
            .template
            .description
            .as_deref(),
        Some("template metadata")
    );
    assert_eq!(
        snapshot.prompts().routes()[0].native_name.as_ref(),
        "deploy"
    );
    assert_eq!(
        snapshot.prompts().routes()[0].prompt.description.as_deref(),
        Some("prompt metadata")
    );
    assert_eq!(
        snapshot.tools().runtime_config_generation(),
        snapshot.services().runtime_config_generation()
    );
    assert_eq!(
        snapshot.tools().pool_publication_generation(),
        runtime.published_pool_snapshot().generation()
    );
    assert_eq!(
        snapshot.resources().pool_publication_generation(),
        snapshot.tools().pool_publication_generation()
    );
    assert_eq!(
        snapshot.resources().runtime_config_generation(),
        snapshot.tools().runtime_config_generation()
    );
    assert_eq!(
        snapshot.resource_templates().runtime_config_generation(),
        snapshot.tools().runtime_config_generation()
    );
    assert_eq!(
        snapshot.resource_templates().pool_publication_generation(),
        snapshot.tools().pool_publication_generation()
    );
    assert_eq!(
        snapshot.prompts().runtime_config_generation(),
        snapshot.tools().runtime_config_generation()
    );
    assert_eq!(
        snapshot.prompts().pool_publication_generation(),
        snapshot.tools().pool_publication_generation()
    );
    assert_eq!(
        snapshot.resources().resource_catalog_generation(),
        pool.published_resource_catalog()
            .await
            .expect("resources")
            .generation()
    );
    assert_eq!(
        snapshot
            .resource_templates()
            .resource_template_catalog_generation(),
        pool.published_resource_template_catalog()
            .await
            .expect("resource templates")
            .generation()
    );
    assert_eq!(
        snapshot.prompts().prompt_catalog_generation(),
        pool.published_prompt_catalog()
            .await
            .expect("prompts")
            .generation()
    );
    assert_eq!(
        snapshot.tools().tool_catalog_generation(),
        pool.published_tool_catalog()
            .await
            .expect("catalog")
            .generation()
    );
    assert_eq!(
        snapshot.services().service_registry_generation(),
        manager
            .published_service_registry_snapshot()
            .expect("services")
            .generation()
    );
}

#[tokio::test]
async fn unified_loadout_mcp_catalog_retries_all_family_aba() {
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
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime.clone())
        .with_builtin_service_registry(publication_registry("deploy"));
    manager
        .seed_config_unchecked_for_tests(unified_config("alpha"))
        .await;
    let initial_runtime = manager
        .published_runtime_loadout_snapshot("project")
        .await
        .generation();
    let initial_pool = runtime.published_pool_snapshot().generation();
    let changing = manager.clone();
    let swapping = runtime.clone();
    let original_for_hook = Arc::clone(&original);
    let snapshot = manager
        .compose_loadout_mcp_catalog("project", move |attempt| {
            let manager = changing.clone();
            let runtime = swapping.clone();
            let transient = Arc::clone(&transient);
            let original = Arc::clone(&original_for_hook);
            async move {
                if attempt == 0 {
                    manager
                        .seed_config_unchecked_for_tests(unified_config("bravo"))
                        .await;
                    manager
                        .seed_config_unchecked_for_tests(unified_config("alpha"))
                        .await;
                    runtime.swap(Some(transient)).await;
                    runtime.swap(Some(Arc::clone(&original))).await;
                }
            }
        })
        .await
        .expect("ABA retry");
    assert_eq!(snapshot.tools().routes()[0].tool_name.as_ref(), "original");
    assert_eq!(snapshot.services().services()[0].name(), "deploy");
    assert_ne!(
        snapshot.tools().runtime_config_generation(),
        initial_runtime
    );
    assert_ne!(snapshot.tools().pool_publication_generation(), initial_pool);
    assert_eq!(
        snapshot.tools().runtime_config_generation(),
        snapshot.services().runtime_config_generation()
    );
}

#[tokio::test]
async fn unified_loadout_mcp_catalog_retries_tool_and_registry_aba_independently() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "original"))
        .await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime)
        .with_builtin_service_registry(publication_registry("deploy"));
    manager
        .seed_config_unchecked_for_tests(unified_config("alpha"))
        .await;
    let initial_tools = pool
        .published_tool_catalog()
        .await
        .expect("tools")
        .generation();
    let initial_services = manager
        .published_service_registry_snapshot()
        .expect("services")
        .generation();
    let changing = manager.clone();
    let changing_pool = Arc::clone(&pool);
    let snapshot = manager
        .compose_loadout_mcp_catalog("project", move |attempt| {
            let manager = changing.clone();
            let pool = Arc::clone(&changing_pool);
            async move {
                if attempt == 0 {
                    pool.insert_entry_for_tests(
                        "alpha",
                        healthy_entry_with_tool("alpha", "changed"),
                    )
                    .await;
                    pool.insert_entry_for_tests(
                        "alpha",
                        healthy_entry_with_tool("alpha", "original"),
                    )
                    .await;
                } else if attempt == 1 {
                    manager.set_builtin_service_registry(publication_registry("transient"));
                    manager.set_builtin_service_registry(publication_registry("deploy"));
                }
            }
        })
        .await
        .expect("catalog ABA retries");
    assert_ne!(snapshot.tools().tool_catalog_generation(), initial_tools);
    assert_ne!(
        snapshot.services().service_registry_generation(),
        initial_services
    );
    assert_eq!(snapshot.tools().routes()[0].tool_name.as_ref(), "original");
    assert_eq!(snapshot.services().services()[0].name(), "deploy");
}

#[tokio::test]
async fn unified_loadout_mcp_catalog_retries_resource_aba_independently() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "echo"))
        .await;
    pool.insert_resource_routes_for_tests(
        "alpha",
        vec![Resource::new("file:///original", "original")],
    )
    .await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("resource-aba.toml"), runtime)
        .with_builtin_service_registry(publication_registry("deploy"));
    manager
        .seed_config_unchecked_for_tests(unified_config("alpha"))
        .await;
    let before = manager
        .published_loadout_mcp_catalog_snapshot("project")
        .await
        .expect("before resource change");
    let initial_tools = before.tools().tool_catalog_generation();
    let initial = pool
        .published_resource_catalog()
        .await
        .expect("initial")
        .generation();
    let changing_pool = Arc::clone(&pool);
    let snapshot = manager
        .compose_loadout_mcp_catalog("project", move |attempt| {
            let pool = Arc::clone(&changing_pool);
            async move {
                if attempt == 0 {
                    pool.insert_resource_routes_for_tests(
                        "alpha",
                        vec![Resource::new("file:///transient", "transient")],
                    )
                    .await;
                    pool.insert_resource_routes_for_tests(
                        "alpha",
                        vec![Resource::new("file:///original", "original")],
                    )
                    .await;
                }
            }
        })
        .await
        .expect("resource ABA retry");
    assert_eq!(
        snapshot.resources().routes()[0].native_uri.as_ref(),
        "file:///original"
    );
    assert_ne!(snapshot.resources().resource_catalog_generation(), initial);
    assert_eq!(
        snapshot.resources().resource_catalog_generation(),
        pool.published_resource_catalog()
            .await
            .expect("final")
            .generation()
    );
    assert_eq!(snapshot.tools().tool_catalog_generation(), initial_tools);
    assert_eq!(
        snapshot.tools().runtime_config_generation(),
        before.tools().runtime_config_generation()
    );
    assert_eq!(
        snapshot.tools().pool_publication_generation(),
        before.tools().pool_publication_generation()
    );
    assert_eq!(
        snapshot.services().service_registry_generation(),
        before.services().service_registry_generation()
    );
    assert_eq!(
        snapshot
            .resource_templates()
            .resource_template_catalog_generation(),
        before
            .resource_templates()
            .resource_template_catalog_generation()
    );
    assert_eq!(
        snapshot.prompts().prompt_catalog_generation(),
        before.prompts().prompt_catalog_generation()
    );
    assert_ne!(
        snapshot.resources().resource_catalog_generation(),
        before.resources().resource_catalog_generation()
    );
    assert!(!before.same_publication_as(&snapshot));
}

#[tokio::test]
async fn unified_loadout_mcp_catalog_bounds_sustained_resource_churn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "echo"))
        .await;
    pool.insert_resource_routes_for_tests(
        "alpha",
        vec![Resource::new("file:///initial", "initial")],
    )
    .await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("resource-churn.toml"), runtime)
        .with_builtin_service_registry(publication_registry("deploy"));
    manager
        .seed_config_unchecked_for_tests(unified_config("alpha"))
        .await;
    let changing_pool = Arc::clone(&pool);
    let result = manager
        .compose_loadout_mcp_catalog("project", move |attempt| {
            let pool = Arc::clone(&changing_pool);
            async move {
                pool.insert_resource_routes_for_tests(
                    "alpha",
                    vec![Resource::new(format!("file:///{attempt}"), "changed")],
                )
                .await;
            }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(LoadoutMcpCatalogPublicationError::Unstable)
    );
}

#[tokio::test]
async fn unified_loadout_mcp_catalog_tracks_only_resource_template_generation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "echo"))
        .await;
    pool.insert_resource_routes_for_tests(
        "alpha",
        vec![Resource::new("file:///resource", "resource")],
    )
    .await;
    pool.insert_resource_template_routes_for_tests(
        "alpha",
        vec![ResourceTemplate::new("file:///before/{id}", "before")],
    )
    .await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("template-only.toml"), runtime)
        .with_builtin_service_registry(publication_registry("deploy"));
    manager
        .seed_config_unchecked_for_tests(unified_config("alpha"))
        .await;
    let before = manager
        .published_loadout_mcp_catalog_snapshot("project")
        .await
        .expect("before");
    let changing = Arc::clone(&pool);
    let after = manager
        .compose_loadout_mcp_catalog("project", move |attempt| {
            let pool = Arc::clone(&changing);
            async move {
                if attempt == 0 {
                    pool.insert_resource_template_routes_for_tests(
                        "alpha",
                        vec![ResourceTemplate::new("file:///transient/{id}", "transient")],
                    )
                    .await;
                    pool.insert_resource_template_routes_for_tests(
                        "alpha",
                        vec![ResourceTemplate::new("file:///before/{id}", "before")],
                    )
                    .await;
                }
            }
        })
        .await
        .expect("template ABA retries");
    assert_eq!(
        before.tools().runtime_config_generation(),
        after.tools().runtime_config_generation()
    );
    assert_eq!(
        before.tools().pool_publication_generation(),
        after.tools().pool_publication_generation()
    );
    assert_eq!(
        before.tools().tool_catalog_generation(),
        after.tools().tool_catalog_generation()
    );
    assert_eq!(
        before.resources().resource_catalog_generation(),
        after.resources().resource_catalog_generation()
    );
    assert_eq!(
        before.prompts().prompt_catalog_generation(),
        after.prompts().prompt_catalog_generation()
    );
    assert_eq!(
        before.services().service_registry_generation(),
        after.services().service_registry_generation()
    );
    assert_ne!(
        before
            .resource_templates()
            .resource_template_catalog_generation(),
        after
            .resource_templates()
            .resource_template_catalog_generation()
    );
    assert!(!before.same_publication_as(&after));
}

#[tokio::test]
async fn unified_loadout_mcp_catalog_bounds_sustained_resource_template_churn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "echo"))
        .await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("template-churn.toml"), runtime)
        .with_builtin_service_registry(publication_registry("deploy"));
    manager
        .seed_config_unchecked_for_tests(unified_config("alpha"))
        .await;
    let changing = Arc::clone(&pool);
    let result = manager
        .compose_loadout_mcp_catalog("project", move |attempt| {
            let pool = Arc::clone(&changing);
            async move {
                pool.insert_resource_template_routes_for_tests(
                    "alpha",
                    vec![ResourceTemplate::new(
                        format!("file:///{attempt}/{{id}}"),
                        "changed",
                    )],
                )
                .await;
            }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(LoadoutMcpCatalogPublicationError::Unstable)
    );
    pool.insert_resource_template_routes_for_tests(
        "alpha",
        vec![
            ResourceTemplate::new("file:///dup/{id}", "one"),
            ResourceTemplate::new("file:///dup/{id}", "two"),
        ],
    )
    .await;
    assert_eq!(
        manager
            .published_loadout_mcp_catalog_snapshot("project")
            .await
            .err(),
        Some(LoadoutMcpCatalogPublicationError::CatalogUnavailable)
    );
}

#[tokio::test]
async fn unified_loadout_mcp_catalog_tracks_only_prompt_generation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "echo"))
        .await;
    pool.insert_resource_routes_for_tests(
        "alpha",
        vec![Resource::new("file:///resource", "resource")],
    )
    .await;
    pool.insert_resource_template_routes_for_tests(
        "alpha",
        vec![ResourceTemplate::new("file:///template/{id}", "template")],
    )
    .await;
    pool.insert_prompt_routes_for_tests(
        "alpha",
        vec![Prompt::new("before", Some("metadata"), None)],
    )
    .await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("prompt-only.toml"), runtime)
        .with_builtin_service_registry(publication_registry("deploy"));
    manager
        .seed_config_unchecked_for_tests(unified_config("alpha"))
        .await;
    let before = manager
        .published_loadout_mcp_catalog_snapshot("project")
        .await
        .expect("before");
    let changing = Arc::clone(&pool);
    let after = manager
        .compose_loadout_mcp_catalog("project", move |attempt| {
            let pool = Arc::clone(&changing);
            async move {
                if attempt == 0 {
                    pool.insert_prompt_routes_for_tests(
                        "alpha",
                        vec![Prompt::new("transient", Some("changed"), None)],
                    )
                    .await;
                    pool.insert_prompt_routes_for_tests(
                        "alpha",
                        vec![Prompt::new("before", Some("metadata"), None)],
                    )
                    .await;
                }
            }
        })
        .await
        .expect("prompt ABA retries");

    assert_eq!(after.prompts().routes()[0].native_name.as_ref(), "before");
    assert_eq!(
        before.tools().runtime_config_generation(),
        after.tools().runtime_config_generation()
    );
    assert_eq!(
        before.tools().pool_publication_generation(),
        after.tools().pool_publication_generation()
    );
    assert_eq!(
        before.tools().tool_catalog_generation(),
        after.tools().tool_catalog_generation()
    );
    assert_eq!(
        before.resources().resource_catalog_generation(),
        after.resources().resource_catalog_generation()
    );
    assert_eq!(
        before
            .resource_templates()
            .resource_template_catalog_generation(),
        after
            .resource_templates()
            .resource_template_catalog_generation()
    );
    assert_eq!(
        before.services().service_registry_generation(),
        after.services().service_registry_generation()
    );
    assert_ne!(
        before.prompts().prompt_catalog_generation(),
        after.prompts().prompt_catalog_generation()
    );
    assert_eq!(
        after.prompts().prompt_catalog_generation(),
        pool.published_prompt_catalog()
            .await
            .expect("prompts")
            .generation()
    );
    assert!(!before.same_publication_as(&after));
}

#[tokio::test]
async fn unified_loadout_mcp_catalog_bounds_prompt_churn_and_maps_stable_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "echo"))
        .await;
    pool.insert_prompt_routes_for_tests(
        "alpha",
        vec![Prompt::new("initial", None::<String>, None)],
    )
    .await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("prompt-churn.toml"), runtime)
        .with_builtin_service_registry(publication_registry("deploy"));
    manager
        .seed_config_unchecked_for_tests(unified_config("alpha"))
        .await;
    let changing = Arc::clone(&pool);
    let result = manager
        .compose_loadout_mcp_catalog("project", move |attempt| {
            let pool = Arc::clone(&changing);
            async move {
                pool.insert_prompt_routes_for_tests(
                    "alpha",
                    vec![Prompt::new(
                        format!("changed-{attempt}"),
                        None::<String>,
                        None,
                    )],
                )
                .await;
            }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(LoadoutMcpCatalogPublicationError::Unstable)
    );

    pool.insert_prompt_routes_for_tests(
        "alpha",
        vec![
            Prompt::new("duplicate", None::<String>, None),
            Prompt::new("duplicate", None::<String>, None),
        ],
    )
    .await;
    assert_eq!(
        manager
            .published_loadout_mcp_catalog_snapshot("project")
            .await
            .err(),
        Some(LoadoutMcpCatalogPublicationError::CatalogUnavailable)
    );
}

#[tokio::test]
async fn unified_loadout_mcp_catalog_maps_stable_missing_states() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = GatewayManager::new(
        dir.path().join("missing.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_builtin_service_registry(publication_registry("deploy"));
    assert_eq!(
        missing
            .published_loadout_mcp_catalog_snapshot("project")
            .await
            .err(),
        Some(LoadoutMcpCatalogPublicationError::MissingLoadout)
    );

    let no_pool = GatewayManager::new(
        dir.path().join("no-pool.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_builtin_service_registry(publication_registry("deploy"));
    no_pool
        .seed_config_unchecked_for_tests(unified_config("alpha"))
        .await;
    assert_eq!(
        no_pool
            .published_loadout_mcp_catalog_snapshot("project")
            .await
            .err(),
        Some(LoadoutMcpCatalogPublicationError::MissingPool)
    );
}

#[tokio::test]
async fn unified_loadout_mcp_catalog_fails_closed_under_sustained_churn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "echo"))
        .await;
    runtime.swap(Some(pool)).await;
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime)
        .with_builtin_service_registry(publication_registry("deploy"));
    manager
        .seed_config_unchecked_for_tests(unified_config("alpha"))
        .await;
    let changing = manager.clone();
    let result = manager
        .compose_loadout_mcp_catalog("project", move |attempt| {
            let manager = changing.clone();
            async move {
                manager
                    .seed_config_unchecked_for_tests(unified_config(if attempt % 2 == 0 {
                        "bravo"
                    } else {
                        "alpha"
                    }))
                    .await;
            }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(LoadoutMcpCatalogPublicationError::Unstable)
    );
}

#[tokio::test]
async fn loadout_service_catalog_applies_alias_visibility_policy_and_metadata() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(PublicationRegistry::new(vec![(
        "deploy",
        vec![
            service_action("help", false, false),
            service_action("schema", false, false),
            service_action("deploy.plan", false, true),
            service_action("deploy.destroy", true, true),
        ],
    )]));
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_builtin_service_registry(registry);
    let mut selected = loadout("project", &[]);
    selected.services = vec!["deploy-primary".to_string()];
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            loadouts: vec![selected],
            virtual_servers: vec![mcp_virtual_server(
                "deploy-primary",
                "deploy",
                &["deploy.plan"],
            )],
            ..Default::default()
        })
        .await;

    let snapshot = manager
        .published_loadout_service_catalog_snapshot("project")
        .await
        .expect("snapshot");
    assert_eq!(snapshot.services().len(), 1);
    let service = &snapshot.services()[0];
    assert_eq!(service.name(), "deploy");
    assert!(service.allows_implicit_help_and_schema());
    assert_eq!(
        service
            .actions()
            .iter()
            .map(|action| action.name())
            .collect::<Vec<_>>(),
        vec!["deploy.plan", "help", "schema"]
    );
    assert!(service.actions()[0].requires_admin());
    assert!(!service.actions()[0].destructive());
}

#[tokio::test]
async fn loadout_service_catalog_publishes_direct_services_and_hides_disabled_aliases() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_builtin_service_registry(publication_registry("deploy"));
    let mut hidden = loadout("hidden", &[]);
    hidden.services = vec!["deploy".to_string()];
    hidden.expose_tools = false;
    let mut absent = loadout("absent", &[]);
    absent.services = vec!["deploy".to_string()];
    let mut disabled = loadout("disabled", &[]);
    disabled.services = vec!["deploy-off".to_string()];
    let mut surface_off = loadout("surface-off", &[]);
    surface_off.services = vec!["deploy-no-mcp".to_string()];
    let mut disabled_server = mcp_virtual_server("deploy-off", "deploy", &[]);
    disabled_server.enabled = false;
    let mut surface_off_server = mcp_virtual_server("deploy-no-mcp", "deploy", &[]);
    surface_off_server.surfaces.mcp = false;
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            loadouts: vec![hidden, absent, disabled, surface_off],
            virtual_servers: vec![disabled_server, surface_off_server],
            ..Default::default()
        })
        .await;
    assert!(
        manager
            .published_loadout_service_catalog_snapshot("hidden")
            .await
            .expect("hidden")
            .services()
            .is_empty()
    );
    assert!(
        manager
            .published_loadout_service_catalog_snapshot("disabled")
            .await
            .expect("disabled")
            .services()
            .is_empty()
    );
    assert!(
        manager
            .published_loadout_service_catalog_snapshot("surface-off")
            .await
            .expect("surface off")
            .services()
            .is_empty()
    );
    let direct = manager
        .published_loadout_service_catalog_snapshot("absent")
        .await
        .expect("direct built-in service");
    assert_eq!(direct.services().len(), 1);
    assert_eq!(direct.services()[0].name(), "deploy");
    assert_eq!(
        direct.services()[0]
            .actions()
            .iter()
            .map(|action| action.name())
            .collect::<Vec<_>>(),
        vec!["a.action", "z.action"]
    );
    assert_eq!(
        manager
            .published_loadout_service_catalog_snapshot("missing")
            .await
            .err(),
        Some(crate::gateway::manager::LoadoutServiceCatalogPublicationError::MissingLoadout)
    );
}

#[tokio::test]
async fn loadout_service_catalog_fails_closed_on_alias_collision() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_builtin_service_registry(publication_registry("deploy"));
    let mut selected = loadout("project", &[]);
    selected.services = vec!["deploy".to_string(), "deploy-primary".to_string()];
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            loadouts: vec![selected],
            virtual_servers: vec![mcp_virtual_server(
                "deploy-primary",
                "deploy",
                &["a.action"],
            )],
            ..Default::default()
        })
        .await;
    assert_eq!(
        manager
            .published_loadout_service_catalog_snapshot("project")
            .await
            .err(),
        Some(crate::gateway::manager::LoadoutServiceCatalogPublicationError::CatalogUnavailable)
    );
}

#[tokio::test]
async fn loadout_service_catalog_retries_registry_swap_and_rejects_sustained_churn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_builtin_service_registry(publication_registry("old"));
    let mut selected = loadout("project", &[]);
    selected.services = vec!["new".to_string()];
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            loadouts: vec![selected],
            virtual_servers: vec![mcp_virtual_server("new", "new", &[])],
            ..Default::default()
        })
        .await;
    let changing = manager.clone();
    let snapshot = manager
        .compose_loadout_service_catalog("project", move |attempt| {
            let manager = changing.clone();
            async move {
                if attempt == 0 {
                    manager.set_builtin_service_registry(publication_registry("new"));
                }
            }
        })
        .await
        .expect("retry");
    assert_eq!(snapshot.services()[0].name(), "new");

    let changing = manager.clone();
    let result = manager
        .compose_loadout_service_catalog("project", move |_| {
            let manager = changing.clone();
            async move {
                manager.set_builtin_service_registry(publication_registry("new"));
            }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(crate::gateway::manager::LoadoutServiceCatalogPublicationError::Unstable)
    );
}

#[tokio::test]
async fn loadout_service_catalog_binds_registry_aba_and_sorts_services() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_builtin_service_registry(ordered_publication_registry());
    let initial = manager
        .published_service_registry_snapshot()
        .expect("initial")
        .generation();
    let mut selected = loadout("project", &[]);
    selected.services = vec!["zulu".to_string(), "alpha".to_string()];
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            loadouts: vec![selected],
            virtual_servers: vec![
                mcp_virtual_server("zulu", "zulu", &[]),
                mcp_virtual_server("alpha", "alpha", &[]),
            ],
            ..Default::default()
        })
        .await;
    let changing = manager.clone();
    let snapshot = manager
        .compose_loadout_service_catalog("project", move |attempt| {
            let manager = changing.clone();
            async move {
                if attempt == 0 {
                    manager.set_builtin_service_registry(publication_registry("transient"));
                    manager.set_builtin_service_registry(ordered_publication_registry());
                }
            }
        })
        .await
        .expect("ABA retry");
    assert_ne!(snapshot.service_registry_generation(), initial);
    assert_eq!(
        snapshot
            .services()
            .iter()
            .map(|service| service.name())
            .collect::<Vec<_>>(),
        vec!["alpha", "zulu"]
    );
}

#[tokio::test]
async fn loadout_service_catalog_retries_and_rejects_config_churn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_builtin_service_registry(publication_registry("deploy"));
    fn service_config(action: &'static str) -> GatewayConfig {
        let mut selected = loadout("project", &[]);
        selected.services = vec!["deploy".to_string()];
        GatewayConfig {
            loadouts: vec![selected],
            virtual_servers: vec![mcp_virtual_server("deploy", "deploy", &[action])],
            ..Default::default()
        }
    }
    manager
        .seed_config_unchecked_for_tests(service_config("a.action"))
        .await;
    let changing = manager.clone();
    let snapshot = manager
        .compose_loadout_service_catalog("project", move |attempt| {
            let manager = changing.clone();
            async move {
                if attempt == 0 {
                    manager
                        .seed_config_unchecked_for_tests(service_config("z.action"))
                        .await;
                }
            }
        })
        .await
        .expect("config retry");
    assert_eq!(snapshot.services()[0].actions()[0].name(), "z.action");

    let changing = manager.clone();
    let result = manager
        .compose_loadout_service_catalog("project", move |attempt| {
            let manager = changing.clone();
            async move {
                manager
                    .seed_config_unchecked_for_tests(service_config(if attempt % 2 == 0 {
                        "a.action"
                    } else {
                        "z.action"
                    }))
                    .await;
            }
        })
        .await;
    assert_eq!(
        result.err(),
        Some(crate::gateway::manager::LoadoutServiceCatalogPublicationError::Unstable)
    );
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
async fn bootstrap_policy_lease_binds_route_and_blocks_publication() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("bootstrap-policy.toml"),
        GatewayRuntimeHandle::default(),
    );
    let mut config = config_with_loadout(loadout("production", &[]));
    config.protected_mcp_routes.push(ProtectedMcpRouteConfig {
        name: "operator".into(),
        enabled: true,
        public_host: "mcp.example".into(),
        public_path: "/operator".into(),
        upstream: None,
        backend_url: String::new(),
        backend_mcp_path: "/mcp".into(),
        scopes: vec!["lab:read".into(), "lab:admin".into()],
        health_path: None,
        target: Some(ProtectedMcpRouteTarget::GatewaySubset(
            ProtectedGatewaySubsetTarget {
                loadout: Some("production".into()),
                ..Default::default()
            },
        )),
    });
    manager.seed_config_unchecked_for_tests(config).await;

    let lease = manager
        .acquire_published_bootstrap_policy_lease("production", "operator")
        .await
        .expect("published bootstrap policy");
    assert_eq!(lease.resource(), "https://mcp.example/operator");
    assert_eq!(lease.audience(), lease.resource());
    assert_eq!(lease.scopes(), &["lab:admin", "lab:read"]);
    let leased_fingerprint = lease.policy_fingerprint();
    let publishing_manager = manager.clone();
    let publication = tokio::spawn(async move {
        let _publication = publishing_manager.publication_barrier.write().await;
    });
    // Product authority performs its awaited durable reconciliation here. A
    // concurrently queued publisher must remain blocked for the full async
    // critical section, and the lease must still expose the same snapshot.
    tokio::task::yield_now().await;
    assert!(
        !publication.is_finished(),
        "publication interleaved the lease"
    );
    assert_eq!(lease.policy_fingerprint(), leased_fingerprint);
    drop(lease);
    tokio::time::timeout(Duration::from_millis(100), publication)
        .await
        .expect("publication resumes after lease release")
        .expect("publication task completes");
}

#[tokio::test]
async fn bootstrap_policy_lease_rejects_configured_but_unbound_loadout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("bootstrap-policy.toml"),
        GatewayRuntimeHandle::default(),
    );
    manager
        .seed_config_unchecked_for_tests(config_with_loadout(loadout("production", &[])))
        .await;
    assert_eq!(
        manager
            .acquire_published_bootstrap_policy_lease("production", "missing")
            .await
            .err(),
        Some(crate::gateway::manager::BootstrapPolicyLeaseError::Unavailable)
    );
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
