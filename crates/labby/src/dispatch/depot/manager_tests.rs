use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::Instant;

use super::health::{Failure, HealthState, Provenance};
use super::manager::{Manager, SecretSnapshot};
use super::network::NetworkPolicy;
use super::network_tests::{tls_fixture, tls_fixture_delayed};
use super::provider::{Identity, ProviderError, ProviderRuntime};
use super::scheduler::Scheduler;
use crate::config::depot::DepotPreferences;

fn preferences(name: &str, path: &str) -> DepotPreferences {
    toml::from_str(&format!(
        r#"
public_enabled = false
[[providers]]
id = "team"
name = "{name}"
endpoint = "https://example.com/{path}"
enabled = true
auth_mode = "anonymous"
"#
    ))
    .unwrap()
}

fn identity() -> Value {
    json!({"contractVersion":"depot.discovery/v1", "deploymentId":"deployment",
        "deploymentEpoch":"boot", "authorityEpoch":"tenant-visibility", "listingEpoch":"catalog",
        "snapshotContinuations":true, "maxPageSize":200})
}

fn response(value: Value) -> String {
    let body = value.to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

#[test]
fn names_reuse_runtime_but_a_b_a_and_stale_candidates_do_not() {
    let secrets = SecretSnapshot::default();
    let policy = NetworkPolicy::default();
    let manager = Manager::new(&preferences("Team", "a"), secrets.clone(), policy.clone());
    let original = manager.snapshot();
    let runtime = original.providers["team"].runtime.clone();
    let rename = manager.prepare(
        &preferences("New name", "a"),
        secrets.clone(),
        policy.clone(),
    );
    let stale = manager.prepare(
        &preferences("Stale name", "a"),
        secrets.clone(),
        policy.clone(),
    );
    manager.publish(rename).unwrap();
    let renamed = manager.snapshot();
    assert_eq!(original.membership_epoch, renamed.membership_epoch);
    assert_eq!(original.providers["team"].view.name, "Team");
    assert_eq!(renamed.providers["team"].view.name, "New name");
    assert!(Arc::ptr_eq(&runtime, &renamed.providers["team"].runtime));
    assert!(manager.publish(stale).is_err());
    manager
        .publish(manager.prepare(&preferences("Team", "b"), secrets.clone(), policy.clone()))
        .unwrap();
    assert!(runtime.cancelled());
    manager
        .publish(manager.prepare(&preferences("Team", "a"), secrets, policy))
        .unwrap();
    assert!(!manager.is_current("team", runtime.incarnation()));
    assert_ne!(
        runtime.incarnation(),
        manager.snapshot().providers["team"].runtime.incarnation()
    );
}

#[test]
fn membership_changes_on_disable_and_secrets_rotate_only_their_provider() {
    let mut config = preferences("Team", "a");
    config.providers[0]["auth_mode"] = "bearer".into();
    config.providers[0]
        .as_table_mut()
        .unwrap()
        .insert("bearer_token_env".into(), "LABBY_DEPOT_TEAM_TOKEN".into());
    let values = |token: &str| {
        SecretSnapshot::from_values(BTreeMap::from([(
            "LABBY_DEPOT_TEAM_TOKEN".into(),
            token.into(),
        )]))
    };
    let manager = Manager::new(&config, values("first"), NetworkPolicy::default());
    let first = manager.snapshot();
    manager
        .publish(manager.prepare(&config, values("second"), NetworkPolicy::default()))
        .unwrap();
    let second = manager.snapshot();
    assert!(first.providers["team"].runtime.cancelled());
    assert!(Arc::ptr_eq(
        &first.providers["public"].runtime,
        &second.providers["public"].runtime
    ));
    assert_eq!(first.membership_epoch, second.membership_epoch);
    config.providers[0]["enabled"] = false.into();
    manager
        .publish(manager.prepare(&config, values("second"), NetworkPolicy::default()))
        .unwrap();
    assert_ne!(second.membership_epoch, manager.snapshot().membership_epoch);
    assert!(!format!("{:?}", values("secret-value")).contains("secret-value"));
}

#[test]
fn composition_uses_effective_toml_and_one_shared_manager_without_environment_reads() {
    let config = crate::config::LabConfig {
        depot: preferences("Configured", "prefix"),
        ..Default::default()
    };
    let state = crate::api::state::AppState::new().with_config(config);
    let clone = state.clone();
    assert!(Arc::ptr_eq(&state.depot_manager, &clone.depot_manager));
    let snapshot = state.depot_manager.snapshot();
    assert!(!snapshot.providers["public"].view.enabled);
    assert_eq!(snapshot.providers["team"].view.name, "Configured");
    assert!(
        state
            .depot_manager
            .status()
            .iter()
            .all(|row| row.health.state == HealthState::Unknown)
    );
}

#[test]
fn qualification_requires_exact_supported_contract_and_bounded_opaque_identity() {
    assert!(Identity::parse(identity()).is_ok());
    for (key, value) in [
        ("contractVersion", json!("future/v2")),
        ("snapshotContinuations", json!(false)),
        ("maxPageSize", json!(201)),
        ("deploymentId", json!("")),
        ("authorityEpoch", json!(4)),
        ("listingEpoch", json!("a".repeat(129))),
    ] {
        let mut invalid = identity();
        invalid[key] = value;
        assert!(Identity::parse(invalid).is_err());
    }
}

#[tokio::test]
async fn auth_failure_requires_manual_probe_and_successful_qualification_is_cached() {
    let (client, mut received) = tls_fixture(response(identity())).await;
    let runtime = ProviderRuntime::from_test_client(client);
    runtime
        .health
        .record(Err(Failure::Unauthorized), Provenance::List);
    let scheduler = Scheduler::default();
    let admission = scheduler.admit("actor", Instant::now()).await.unwrap();
    assert_eq!(
        runtime.qualify(&admission, false).await,
        Err(ProviderError::Failed(Failure::Unauthorized))
    );
    assert!(matches!(
        received.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(runtime.qualify(&admission, true).await.is_ok());
    received.await.unwrap();
    assert_eq!(runtime.health.view().state, HealthState::Healthy);
    assert!(runtime.qualify(&admission, false).await.is_ok());
}

#[tokio::test]
async fn cancellation_drops_inflight_qualification_and_cannot_publish_health() {
    let (client, received) =
        tls_fixture_delayed(response(identity()), Duration::from_millis(250)).await;
    let runtime = Arc::new(ProviderRuntime::from_test_client(client));
    let worker = runtime.clone();
    let task = tokio::spawn(async move {
        let scheduler = Scheduler::default();
        let admission = scheduler.admit("actor", Instant::now()).await.unwrap();
        worker.qualify(&admission, false).await
    });
    received.await.unwrap();
    runtime.cancel();
    assert_eq!(task.await.unwrap(), Err(ProviderError::Stale));
    assert!(!runtime.retains_test_client());
    assert_eq!(runtime.health.view().state, HealthState::Unknown);
}

#[test]
fn transient_cooldown_is_bounded_and_schema_failure_never_automatically_retries() {
    let health = super::health::Health::default();
    health.record(Err(Failure::Transient), Provenance::Get);
    let view = health.view();
    assert!(view.retry_not_before.unwrap() - view.observed_at.unwrap() <= 30);
    assert_eq!(health.admit(false), Err(ProviderError::Pending));
    assert!(health.admit(true).is_ok());
    health.record(Err(Failure::Incompatible), Provenance::Qualification);
    assert_eq!(
        health.admit(false),
        Err(ProviderError::Failed(Failure::Incompatible))
    );
    assert!(health.admit(true).is_ok());
}

#[test]
fn late_automatic_success_cannot_clear_sticky_auth_or_schema_failure() {
    for failure in [Failure::Unauthorized, Failure::Incompatible] {
        let health = super::health::Health::default();
        health.record(Err(failure), Provenance::List);
        health.record(Ok(()), Provenance::List);
        assert_eq!(health.admit(false), Err(ProviderError::Failed(failure)));
        health.record(Ok(()), Provenance::Probe);
        assert!(health.admit(false).is_ok());
    }
}

#[tokio::test]
async fn missing_contract_route_is_incompatible_not_a_healthy_missing_artifact() {
    let (client, received) =
        tls_fixture("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".into()).await;
    let runtime = ProviderRuntime::from_test_client(client);
    let scheduler = Scheduler::default();
    let admission = scheduler.admit("actor", Instant::now()).await.unwrap();
    assert_eq!(
        runtime.qualify(&admission, false).await,
        Err(ProviderError::Failed(Failure::Incompatible))
    );
    received.await.unwrap();
    assert_eq!(runtime.health.view().state, HealthState::Incompatible);
}
