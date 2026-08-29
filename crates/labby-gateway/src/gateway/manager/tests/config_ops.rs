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
                proxy_skills: false,
                expose_skills: None,
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
async fn independent_managers_serialize_read_modify_write_across_process_boundary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let first = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    let second = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    let mut alpha = fixture_stdio_upstream("alpha");
    alpha.enabled = false;
    let mut bravo = fixture_stdio_upstream("bravo");
    bravo.enabled = false;

    let (first_result, second_result) = tokio::join!(
        first.add(alpha, None, None, None),
        second.add(bravo, None, None, None),
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

#[test]
fn gateway_mutation_child_process() {
    let Ok(path) = std::env::var("LABBY_TEST_GATEWAY_MUTATION_PATH") else {
        return;
    };
    let name = std::env::var("LABBY_TEST_GATEWAY_MUTATION_NAME").expect("child mutation name");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    runtime.block_on(async move {
        let manager = GatewayManager::new(PathBuf::from(path), GatewayRuntimeHandle::default());
        let mut upstream = fixture_stdio_upstream(&name);
        upstream.enabled = false;
        manager
            .add(upstream, None, None, None)
            .await
            .expect("child add");
    });
}

#[test]
fn separate_processes_preserve_both_gateway_mutations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let executable = std::env::current_exe().expect("current test executable");
    let spawn = |name: &str| {
        std::process::Command::new(&executable)
            .args([
                "--exact",
                "gateway::manager::tests::config_ops::gateway_mutation_child_process",
                "--nocapture",
            ])
            .env("LABBY_TEST_GATEWAY_MUTATION_PATH", &path)
            .env("LABBY_TEST_GATEWAY_MUTATION_NAME", name)
            .spawn()
            .expect("spawn child mutation")
    };
    let mut alpha = spawn("alpha");
    let mut bravo = spawn("bravo");
    assert!(alpha.wait().expect("wait alpha").success());
    assert!(bravo.wait().expect("wait bravo").success());

    let persisted = load_gateway_config(&path).expect("load persisted config");
    let names = persisted
        .upstream
        .iter()
        .map(|upstream| upstream.name.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(names, BTreeSet::from(["alpha", "bravo"]));
}

#[tokio::test]
async fn failed_reload_rolls_back_disk_live_state_and_restart_truth() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let initial = GatewayConfig::default();
    crate::gateway::config::write_gateway_config(&path, &initial).expect("seed disk");
    let store = Arc::new(FaultAfterPersistStore::new(path.clone()));
    let manager =
        GatewayManager::with_store(path.clone(), GatewayRuntimeHandle::default(), store.clone());
    manager
        .seed_config_unchecked_for_tests(initial.clone())
        .await;
    store.fail_next_reload();

    manager
        .add(fixture_stdio_upstream("must-rollback"), None, None, None)
        .await
        .expect_err("reload failure must fail the mutation");

    let persisted = load_gateway_config(&path).expect("rollback restores parseable disk config");
    assert!(
        persisted.upstream.is_empty(),
        "disk must return to the previous revision"
    );
    assert!(
        manager.current_config().await.upstream.is_empty(),
        "live config must remain on the previous revision"
    );
    assert!(
        path.with_file_name("config.toml.bak").exists(),
        "the pre-commit durable revision must be backed up"
    );
    let restarted = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    restarted
        .reload_with_origin(None, None)
        .await
        .expect("restart reload");
    assert!(restarted.current_config().await.upstream.is_empty());
}

#[tokio::test]
async fn failed_code_mode_reconcile_preserves_process_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    crate::gateway::config::write_gateway_config(&path, &GatewayConfig::default())
        .expect("seed disk");
    let store = Arc::new(FaultAfterPersistStore::new(path.clone()));
    let manager = GatewayManager::with_store(path, GatewayRuntimeHandle::default(), store.clone());
    store.fail_next_reload();
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
        .expect_err("candidate reconcile fails");
    assert!(
        !store.process_code_mode_enabled(),
        "rejected candidate must not publish the process-wide flag"
    );
}

#[tokio::test]
async fn abort_after_persist_does_not_cancel_the_owned_config_transaction() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    crate::gateway::config::write_gateway_config(&path, &GatewayConfig::default())
        .expect("seed disk");
    let persisted = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let store = Arc::new(PauseAfterPersistStore {
        path: path.clone(),
        persisted: Arc::clone(&persisted),
        release: Arc::clone(&release),
    });
    let manager = GatewayManager::with_store(path.clone(), GatewayRuntimeHandle::default(), store);
    let worker = manager.clone();
    let mut candidate = fixture_stdio_upstream("survives-abort");
    candidate.enabled = false;
    let request = tokio::spawn(async move { worker.add(candidate, None, None, None).await });

    tokio::task::spawn_blocking({
        let persisted = Arc::clone(&persisted);
        move || {
            let (lock, cv) = &*persisted;
            let mut ready = lock.lock().expect("persist lock");
            while !*ready {
                ready = cv.wait(ready).expect("persist wait");
            }
        }
    })
    .await
    .expect("persist waiter");
    request.abort();
    let (release_lock, release_cv) = &*release;
    *release_lock.lock().expect("release lock") = true;
    release_cv.notify_all();

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if manager
                .current_config()
                .await
                .upstream
                .iter()
                .any(|upstream| upstream.name == "survives-abort")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("detached transaction completes");
    assert!(
        load_gateway_config(&path)
            .expect("durable config")
            .upstream
            .iter()
            .any(|upstream| upstream.name == "survives-abort")
    );
}

#[tokio::test]
async fn abort_direct_persist_keeps_durable_and_live_config_coherent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    crate::gateway::config::write_gateway_config(&path, &GatewayConfig::default())
        .expect("seed disk");
    let persisted = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let manager = GatewayManager::with_store(
        path.clone(),
        GatewayRuntimeHandle::default(),
        Arc::new(PauseAfterPersistStore {
            path: path.clone(),
            persisted: Arc::clone(&persisted),
            release: Arc::clone(&release),
        }),
    );
    let worker = manager.clone();
    let request = tokio::spawn(async move {
        let guard = worker.acquire_config_mutation().await?;
        let mut candidate = worker.load_config_for_mutation().await?;
        candidate.code_mode.mcp_ui_enabled = true;
        worker.persist_config_owned(guard, candidate).await
    });
    tokio::task::spawn_blocking({
        let persisted = Arc::clone(&persisted);
        move || {
            let (lock, cv) = &*persisted;
            let mut ready = lock.lock().expect("persist lock");
            while !*ready {
                ready = cv.wait(ready).expect("persist wait");
            }
        }
    })
    .await
    .expect("persist waiter");
    request.abort();
    let (release_lock, release_cv) = &*release;
    *release_lock.lock().expect("release lock") = true;
    release_cv.notify_all();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !manager.current_config().await.code_mode.mcp_ui_enabled {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned direct persist completes");
    assert!(load_gateway_config(&path).unwrap().code_mode.mcp_ui_enabled);
}

#[tokio::test]
async fn abort_staged_persist_finishes_durable_write_without_publishing_live_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    crate::gateway::config::write_gateway_config(&path, &GatewayConfig::default())
        .expect("seed disk");
    let persisted = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let manager = GatewayManager::with_store(
        path.clone(),
        GatewayRuntimeHandle::default(),
        Arc::new(PauseAfterPersistStore {
            path: path.clone(),
            persisted: Arc::clone(&persisted),
            release: Arc::clone(&release),
        }),
    );
    let live_before = manager.current_config().await.code_mode.mcp_ui_enabled;
    let desired_value = !live_before;
    let worker = manager.clone();
    let request = tokio::spawn(async move {
        let guard = worker.acquire_config_mutation().await?;
        let mut candidate = worker.load_config_for_mutation().await?;
        candidate.code_mode.mcp_ui_enabled = desired_value;
        worker.persist_desired_config_owned(guard, candidate).await
    });
    tokio::task::spawn_blocking({
        let persisted = Arc::clone(&persisted);
        move || {
            let (lock, cv) = &*persisted;
            let mut ready = lock.lock().expect("persist lock");
            while !*ready {
                ready = cv.wait(ready).expect("persist wait");
            }
        }
    })
    .await
    .expect("persist waiter");
    request.abort();
    let (release_lock, release_cv) = &*release;
    *release_lock.lock().expect("release lock") = true;
    release_cv.notify_all();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if load_gateway_config(&path)
                .expect("durable config")
                .code_mode
                .mcp_ui_enabled
                == desired_value
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned staged persist completes");
    assert_eq!(
        load_gateway_config(&path).unwrap().code_mode.mcp_ui_enabled,
        desired_value
    );
    assert_eq!(
        manager.current_config().await.code_mode.mcp_ui_enabled,
        live_before
    );
}

#[tokio::test]
async fn reload_rollback_covers_batch_update_remove_and_code_mode_mutations() {
    // batch-add
    let batch_dir = tempfile::tempdir().expect("batch tempdir");
    let batch_path = batch_dir.path().join("config.toml");
    crate::gateway::config::write_gateway_config(&batch_path, &GatewayConfig::default())
        .expect("seed batch disk");
    let batch_store = Arc::new(FaultAfterPersistStore::new(batch_path.clone()));
    let batch = GatewayManager::with_store(
        batch_path.clone(),
        GatewayRuntimeHandle::default(),
        batch_store.clone(),
    );
    batch_store.fail_next_reload();
    batch
        .batch_add(
            vec![
                fixture_stdio_upstream("alpha"),
                fixture_stdio_upstream("bravo"),
            ],
            None,
            None,
        )
        .await
        .expect_err("batch reload failure");
    assert!(
        load_gateway_config(&batch_path)
            .unwrap()
            .upstream
            .is_empty()
    );

    // update and remove each start from one disabled durable upstream, avoiding
    // any real transport connection while exercising the full reload path.
    for operation in ["update", "remove"] {
        let dir = tempfile::tempdir().expect("mutation tempdir");
        let path = dir.path().join("config.toml");
        let mut initial = GatewayConfig::default();
        let mut upstream = fixture_stdio_upstream("alpha");
        upstream.enabled = false;
        initial.upstream.push(upstream);
        crate::gateway::config::write_gateway_config(&path, &initial).expect("seed disk");
        let store = Arc::new(FaultAfterPersistStore::new(path.clone()));
        let manager = GatewayManager::with_store(
            path.clone(),
            GatewayRuntimeHandle::default(),
            store.clone(),
        );
        manager.seed_config_unchecked_for_tests(initial).await;
        store.fail_next_reload();
        let result = if operation == "update" {
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
                .map(|_| ())
        } else {
            manager.remove("alpha", None, None).await.map(|_| ())
        };
        result.expect_err("reload failure");
        let persisted = load_gateway_config(&path).expect("rollback disk");
        assert_eq!(persisted.upstream.len(), 1);
        assert!(!persisted.upstream[0].enabled);
        let live = manager.current_config().await;
        assert_eq!(live.upstream.len(), 1);
        assert!(!live.upstream[0].enabled);
    }

    // Code Mode runtime settings also require a pool reconcile and therefore
    // share the same rollback contract.
    let mode_dir = tempfile::tempdir().expect("mode tempdir");
    let mode_path = mode_dir.path().join("config.toml");
    crate::gateway::config::write_gateway_config(&mode_path, &GatewayConfig::default())
        .expect("seed mode disk");
    let mode_store = Arc::new(FaultAfterPersistStore::new(mode_path.clone()));
    let mode = GatewayManager::with_store(
        mode_path.clone(),
        GatewayRuntimeHandle::default(),
        mode_store.clone(),
    );
    mode_store.fail_next_reload();
    mode.set_code_mode_config(
        CodeModeConfig {
            enabled: true,
            ..CodeModeConfig::default()
        },
        None,
        None,
    )
    .await
    .expect_err("code mode reload failure");
    assert!(!load_gateway_config(&mode_path).unwrap().code_mode.enabled);
    assert!(!mode.current_config().await.code_mode.enabled);
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
        .add(fixture_http_upstream("alpha"), None, None, None)
        .await
        .expect("add alpha to the live pool");
    let after_add = runtime.current_pool().await.expect("pool after add");
    assert!(
        Arc::ptr_eq(&initial_pool, &after_add),
        "transactional add should selectively reconcile the existing live pool"
    );
    assert_eq!(after_add.upstream_count().await, 1);
    assert_eq!(
        initial_pool.upstream_count().await,
        1,
        "the existing live pool should contain the selectively reconciled upstream"
    );

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
        Arc::ptr_eq(&after_add, &after_update),
        "transactional update should selectively reconcile the existing live pool"
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
        Arc::ptr_eq(&before_remove, &after_remove),
        "transactional remove should selectively reconcile the existing live pool"
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
                mcp_ui_enabled: true,
                ..CodeModeConfig::default()
            },
            Some(labby_runtime::catalog_notify::SOURCE_MCP_CALL_MCP_APP),
            None,
        )
        .await
        .expect("persist Code Mode MCP UI setting");

    assert!(updated.mcp_ui_enabled);
    assert!(manager.code_mode_app_state().is_enabled());
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
    assert!(persisted.code_mode.mcp_ui_enabled);

    let restarted = GatewayManager::new(path, GatewayRuntimeHandle::default());
    restarted.seed_config_unchecked_for_tests(persisted).await;
    assert!(restarted.code_mode_app_state().is_enabled());
}

#[tokio::test]
async fn mcp_app_visibility_setting_persists_notifies_and_skips_pool_rebuild() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let mut manager = GatewayManager::new(path.clone(), runtime.clone());
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();
    manager.set_notifier(crate::gateway::types::CatalogChangeNotifier::new(notify_tx));
    let mut initial = GatewayConfig::default();
    initial.code_mode.mcp_ui_enabled = true;
    initial.mcp_apps.manager = true;
    initial.mcp_apps.gateway_status = true;
    initial.mcp_apps.server_logs = true;
    initial.mcp_apps.add_server = true;
    initial.mcp_apps.settings = true;
    manager.seed_config_unchecked_for_tests(initial).await;
    assert!(runtime.current_pool().await.is_none());

    let updated = manager
        .set_mcp_app_visibility(
            "all",
            false,
            Some(labby_runtime::catalog_notify::SOURCE_MCP_CALL_MCP_APP),
        )
        .await
        .expect("persist MCP App visibility");

    assert!(!updated.mcp_apps.manager);
    assert!(!updated.code_mode.mcp_ui_enabled);
    assert!(!updated.mcp_apps.gateway_status);
    assert!(!updated.mcp_apps.server_logs);
    assert!(!updated.mcp_apps.add_server);
    assert!(!updated.mcp_apps.settings);
    assert!(!manager.code_mode_app_state().is_enabled());
    assert!(
        runtime.current_pool().await.is_none(),
        "app-only settings must not create or rebuild the upstream pool"
    );

    let event = tokio::time::timeout(Duration::from_secs(1), notify_rx.recv())
        .await
        .expect("MCP App catalog notification timed out")
        .expect("MCP App catalog notification channel closed");
    assert!(event.diff.tools_changed);
    assert!(event.diff.resources_changed);
    assert!(!event.diff.prompts_changed);
    assert_eq!(
        event.source,
        labby_runtime::catalog_notify::SOURCE_MCP_CALL_MCP_APP
    );
    assert!(
        notify_rx.try_recv().is_err(),
        "a bulk visibility change should emit exactly one catalog event"
    );

    let persisted = load_gateway_config(&path).expect("load persisted config");
    assert!(!persisted.mcp_apps.manager);
    assert!(!persisted.code_mode.mcp_ui_enabled);
    assert!(!persisted.mcp_apps.gateway_status);
    assert!(!persisted.mcp_apps.server_logs);
    assert!(!persisted.mcp_apps.add_server);
    assert!(!persisted.mcp_apps.settings);

    let restarted = GatewayManager::new(path, GatewayRuntimeHandle::default());
    restarted.seed_config_unchecked_for_tests(persisted).await;
    assert!(!restarted.code_mode_app_state().is_enabled());
    let restarted_apps = restarted.mcp_apps_config().await;
    assert!(!restarted_apps.manager);
    assert!(!restarted_apps.gateway_status);
    assert!(!restarted_apps.server_logs);
    assert!(!restarted_apps.add_server);
    assert!(!restarted_apps.settings);
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
// out of `labby-gateway` into `lab`'s `LabConfigStore`, so the manager no longer
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

#[tokio::test]
async fn queued_reload_rechecks_expected_revision_inside_mutation_lock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    let mut initial = GatewayConfig::default();
    initial
        .upstream
        .push(fixture_stdio_upstream("gateway-alpha"));
    crate::gateway::config::write_gateway_config(&path, &initial).unwrap();
    let expected = crate::gateway::manager::views::upstream_revision(&initial.upstream[0]);

    let guard = manager.acquire_config_mutation().await.unwrap();
    let queued = {
        let manager = manager.clone();
        tokio::spawn(async move {
            manager
                .reload_checked(Some("gateway-alpha"), Some(&expected), None, None)
                .await
        })
    };
    tokio::task::yield_now().await;
    let mut changed = initial;
    changed.upstream[0].enabled = false;
    crate::gateway::config::write_gateway_config(&path, &changed).unwrap();
    drop(guard);

    let error = queued
        .await
        .unwrap()
        .expect_err("queued reload must observe the newer durable revision");
    assert_eq!(error.kind(), "stale_state");
}
