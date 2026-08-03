//! Service config + upstream add/update persistence tests.

use std::collections::BTreeSet;

use crate::gateway::config::load_gateway_config;
use crate::gateway::config_mutation::read_env_values;
use labby_runtime::gateway_config::{VirtualServerConfig, VirtualServerSurfacesConfig};

use super::*;

// CWE-532 guard for targeted secret redaction in service configuration views.
// The local fixture declares one public URL and one secret token.
#[test]
fn service_config_get_redacts_secret_values() {
    let mut values = HashMap::new();
    values.insert(
        "FIXTURE_URL".to_string(),
        "http://127.0.0.1:9999".to_string(),
    );
    values.insert("FIXTURE_TOKEN".to_string(), "super-secret".to_string());

    let config = crate::gateway::projection::service_config_view(&FIXTURE_SERVICE_META, &values);

    let secret = config
        .fields
        .iter()
        .find(|field| field.name == "FIXTURE_TOKEN")
        .expect("secret field");
    assert!(secret.present);
    assert!(secret.secret);
    assert_eq!(
        secret.value_preview, None,
        "secret values must never be echoed back in a config read (CWE-532)"
    );

    // The non-secret companion field IS previewed — redaction is targeted, not a
    // blanket suppression of every field value.
    let non_secret = config
        .fields
        .iter()
        .find(|field| field.name == "FIXTURE_URL")
        .expect("non-secret field");
    assert!(non_secret.present);
    assert!(!non_secret.secret);
    assert_eq!(
        non_secret.value_preview.as_deref(),
        Some("http://127.0.0.1:9999")
    );
}

// Re-fixtured post-gateway-pivot via `service_config_view` directly against the
// kept `acp` `PluginMeta` (see `service_config_get_redacts_secret_values` for why
// the manager end-to-end path isn't usable). An empty value for a declared field
// must be reported as not-present and never previewed.
#[test]
fn service_config_get_treats_empty_values_as_not_present() {
    let mut values = HashMap::new();
    values.insert("FIXTURE_TOKEN".to_string(), "token".to_string());
    values.insert("FIXTURE_URL".to_string(), String::new());

    let config = crate::gateway::projection::service_config_view(&FIXTURE_SERVICE_META, &values);

    let db = config
        .fields
        .iter()
        .find(|field| field.name == "FIXTURE_URL")
        .expect("db field");
    assert!(!db.present);
    assert_eq!(db.value_preview, None);
}

// CANNOT be re-fixtured without production-code changes (out of test-only scope).
// This test asserts `configured == false` when a *required* field is missing, but
// post-gateway-pivot NO surviving/kept service declares any `required_env` (acp,
// stash, deploy, setup, doctor, marketplace all have `required_env: &[]`). With no
// required fields, `service_config_view` reports `configured: true` unconditionally,
// so the assertion can never hold. Re-enabling this requires either a kept service
// that declares a required env var, or a synthetic `PluginMeta` reachable through
// `registered_service_meta` (which resolves via the static `service_meta` table) —
// both are production-code changes. Leaving ignored per the restoration spec.
#[tokio::test]
async fn service_config_get_marks_service_unconfigured_when_required_fields_are_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default())
        .with_builtin_service_registry(deploy_known_registry());

    let mut values = BTreeMap::new();
    values.insert("FIXTURE_TOKEN".to_string(), "token".to_string());

    let config = manager
        .set_service_config("fixture-service", &values)
        .await
        .expect("set service config");

    assert!(
        !config.configured,
        "gateway_alpha should remain unconfigured until every required field is present"
    );
}

// Re-fixtured post-gateway-pivot via `service_config_view` directly against the
// kept `acp` `PluginMeta`. acp declares no required env (only optional), so the
// all-required-present predicate holds and the service reports `configured == true`
// once its fields are populated. Exercises the `configured == true` branch of
// `service_config_view` for a real registered service.
#[test]
fn service_config_get_marks_service_configured_when_required_fields_are_present() {
    let mut values = HashMap::new();
    values.insert(
        "FIXTURE_URL".to_string(),
        "http://127.0.0.1:9999".to_string(),
    );
    values.insert("FIXTURE_TOKEN".to_string(), "token".to_string());

    let config = crate::gateway::projection::service_config_view(&FIXTURE_SERVICE_META, &values);

    assert!(config.configured);
}

#[tokio::test]
async fn add_with_bearer_token_value_writes_env_and_references_generated_env_var() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let env_path = dir.path().join(".env");
    let manager =
        GatewayManager::new(path, GatewayRuntimeHandle::default()).with_env_path(env_path);

    let gateway = manager
        .add(
            UpstreamConfig {
                enabled: true,
                name: "github".to_string(),
                url: Some("https://api.githubcopilot.com/mcp/".to_string()),
                transport: None,
                socket_path: None,
                headers: Default::default(),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            },
            Some("ghp_secret".to_string()),
            None,
            None,
        )
        .await
        .expect("add gateway");

    assert_eq!(
        gateway.config.bearer_token_env.as_deref(),
        Some("LABBY_GW_GITHUB_AUTH_HEADER")
    );

    let values = read_env_values(&dir.path().join(".env")).expect("read env");
    assert_eq!(
        values
            .get("LABBY_GW_GITHUB_AUTH_HEADER")
            .map(String::as_str),
        Some("Bearer ghp_secret")
    );
}

#[tokio::test]
async fn concurrent_gateway_adds_persist_both_gateways() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());

    let first = manager.clone();
    let second = manager.clone();
    let (first_result, second_result) = tokio::join!(
        first.add(fixture_stdio_upstream("alpha"), None, None, None),
        second.add(fixture_stdio_upstream("bravo"), None, None, None),
    );

    first_result.expect("add alpha");
    second_result.expect("add bravo");

    let persisted = load_gateway_config(&path).expect("load persisted config");
    let names = persisted
        .upstream
        .iter()
        .map(|upstream| upstream.name.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(names, BTreeSet::from(["alpha", "bravo"]));
}

#[tokio::test]
async fn add_update_and_remove_reconcile_against_the_previous_live_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let manager = GatewayManager::new(path, runtime.clone());
    manager
        .seed_config_unchecked_for_tests(GatewayConfig::default())
        .await;
    manager
        .set_code_mode_config(
            CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            None,
            None,
        )
        .await
        .expect("seed an initial live pool");

    let initial_pool = runtime.current_pool().await.expect("initial pool");
    assert_eq!(initial_pool.upstream_count().await, 0);

    manager
        .add(fixture_stdio_upstream("alpha"), None, None, None)
        .await
        .expect("add alpha to the live pool");
    let after_add = runtime.current_pool().await.expect("pool after add");
    assert!(
        Arc::ptr_eq(&initial_pool, &after_add),
        "add-only reconciliation should preserve the live pool"
    );
    assert_eq!(after_add.upstream_count().await, 1);

    manager
        .update(
            "alpha",
            crate::gateway::params::GatewayUpdatePatch {
                enabled: Some(false),
                ..Default::default()
            },
            None,
            None,
            None,
        )
        .await
        .expect("disable alpha");
    let after_update = runtime.current_pool().await.expect("pool after update");
    assert!(
        !Arc::ptr_eq(&after_add, &after_update),
        "updating an existing upstream should reconcile a fresh pool"
    );
    assert_eq!(after_update.upstream_count().await, 0);

    manager
        .update(
            "alpha",
            crate::gateway::params::GatewayUpdatePatch {
                enabled: Some(true),
                ..Default::default()
            },
            None,
            None,
            None,
        )
        .await
        .expect("re-enable alpha");
    let before_remove = runtime.current_pool().await.expect("pool before remove");
    assert_eq!(before_remove.upstream_count().await, 1);

    manager
        .remove("alpha", None, None)
        .await
        .expect("remove alpha");
    let after_remove = runtime.current_pool().await.expect("pool after remove");
    assert!(
        !Arc::ptr_eq(&before_remove, &after_remove),
        "removing an existing upstream should reconcile a fresh pool"
    );
    assert_eq!(after_remove.upstream_count().await, 0);
}

#[tokio::test]
async fn batch_add_returns_successful_views_and_preserves_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    let outcome = manager
        .batch_add(
            vec![
                fixture_stdio_upstream("alpha"),
                fixture_stdio_upstream("bravo"),
                fixture_stdio_upstream("bad name"),
            ],
            None,
            None,
        )
        .await
        .expect("partial batch succeeds");

    let imported = outcome
        .views
        .iter()
        .map(|view| view.config.name.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(imported, BTreeSet::from(["alpha", "bravo"]));
    assert_eq!(outcome.errors.len(), 1);
    assert_eq!(outcome.errors[0].0, "bad name");
}

// Re-fixtured post-gateway-pivot: the virtual server is backed by the kept
// `deploy` service rather than retired service env fixtures. Asserts a concurrent root
// config mutation and a virtual-server surface mutation both persist.
#[tokio::test]
async fn concurrent_root_and_virtual_server_mutations_both_persist() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default())
        .with_builtin_service_registry(deploy_known_registry());
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            virtual_servers: vec![VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig {
                    cli: false,
                    api: false,
                    mcp: false,
                    webui: false,
                },
                mcp_policy: None,
            }],
            ..GatewayConfig::default()
        })
        .await;

    let root = manager.clone();
    let virtual_server = manager.clone();
    let (root_result, virtual_result) = tokio::join!(
        root.set_code_mode_config(
            CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            None,
            None,
        ),
        virtual_server.set_virtual_server_surface("deploy", "mcp", true),
    );

    root_result.expect("set root code mode config");
    virtual_result.expect("set virtual server surface");

    let persisted = load_gateway_config(&path).expect("load persisted config");
    assert!(persisted.code_mode.enabled);
    let gateway_alpha = persisted
        .virtual_servers
        .iter()
        .find(|server| server.id == "deploy")
        .expect("gateway_alpha virtual server persisted");
    assert!(gateway_alpha.surfaces.mcp);
}

#[tokio::test]
async fn code_mode_mcp_ui_setting_persists_notifies_and_skips_pool_rebuild() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let mut manager = GatewayManager::new(path.clone(), runtime.clone());
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();
    manager.set_notifier(crate::gateway::types::CatalogChangeNotifier::new(notify_tx));
    manager
        .seed_config_unchecked_for_tests(GatewayConfig::default())
        .await;
    assert!(runtime.current_pool().await.is_none());

    let updated = manager
        .set_code_mode_config(
            CodeModeConfig {
                mcp_ui_enabled: false,
                ..CodeModeConfig::default()
            },
            Some(labby_runtime::catalog_notify::SOURCE_MCP_CALL_MCP_APP),
            None,
        )
        .await
        .expect("persist Code Mode MCP UI setting");

    assert!(!updated.mcp_ui_enabled);
    assert!(!manager.code_mode_app_state().is_enabled());
    assert!(
        runtime.current_pool().await.is_none(),
        "a UI-only setting must not create or rebuild the upstream pool"
    );
    let event = tokio::time::timeout(Duration::from_secs(1), notify_rx.recv())
        .await
        .expect("MCP UI catalog notification timed out")
        .expect("MCP UI catalog notification channel closed");
    assert!(event.diff.tools_changed);
    assert!(event.diff.resources_changed);
    assert!(!event.diff.prompts_changed);
    assert_eq!(
        event.source,
        labby_runtime::catalog_notify::SOURCE_MCP_CALL_MCP_APP
    );
    assert!(
        notify_rx.try_recv().is_err(),
        "a UI-only toggle should emit exactly one catalog event"
    );

    let persisted = load_gateway_config(&path).expect("load persisted config");
    assert!(!persisted.code_mode.mcp_ui_enabled);

    let restarted = GatewayManager::new(path, GatewayRuntimeHandle::default());
    restarted.seed_config_unchecked_for_tests(persisted).await;
    assert!(!restarted.code_mode_app_state().is_enabled());
}

#[tokio::test]
async fn code_mode_runtime_change_notifies_from_the_previous_regime() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let mut manager = GatewayManager::new(path, runtime.clone());
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();
    manager.set_notifier(crate::gateway::types::CatalogChangeNotifier::new(notify_tx));
    manager
        .seed_config_unchecked_for_tests(GatewayConfig::default())
        .await;

    let updated = manager
        .set_code_mode_config(
            CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            None,
            None,
        )
        .await
        .expect("enable Code Mode");

    assert!(updated.enabled);
    assert!(runtime.current_pool().await.is_some());
    let event = tokio::time::timeout(Duration::from_secs(1), notify_rx.recv())
        .await
        .expect("Code Mode regime catalog notification timed out")
        .expect("Code Mode regime catalog notification channel closed");
    assert!(event.diff.tools_changed);
    assert_eq!(
        event.source,
        labby_runtime::catalog_notify::SOURCE_GATEWAY_RELOAD_FULL
    );
}

// Store-seam env persistence guard (rewritten in the gateway extraction).
//
// The host-owned service-client cache + `refresh_count()` instrumentation moved
// out of `lab-gateway` into `lab`'s `LabConfigStore`, so the manager no longer
// exposes `with_service_clients`. The credential-write half of that contract is
// now owned by the `GatewayConfigStore` seam: env vars are persisted through
// `store.persist_*`, exercised here against the default `FsGatewayConfigStore`
// (injected via `with_env_path`). This asserts a real env-credential write lands
// in the backing `.env` file through the store seam.
#[tokio::test]
async fn bearer_token_credential_write_persists_through_store_seam() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let env_path = dir.path().join(".env");
    let manager =
        GatewayManager::new(path, GatewayRuntimeHandle::default()).with_env_path(env_path.clone());

    manager
        .add(
            fixture_stdio_upstream("gateway-alpha"),
            Some("gateway_alpha-token".to_string()),
            None,
            None,
        )
        .await
        .expect("add gateway with bearer token");

    let values = read_env_values(&env_path).expect("read env values written via store seam");
    assert_eq!(
        values
            .get("LABBY_GW_GATEWAY_ALPHA_AUTH_HEADER")
            .map(String::as_str),
        Some("Bearer gateway_alpha-token"),
        "bearer credential must be persisted to the .env file through the store seam"
    );
}
