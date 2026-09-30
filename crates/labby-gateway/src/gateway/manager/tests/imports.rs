//! Auto-import partition + tombstone matching tests.

use crate::gateway::manager::partition_discovered_for_import;
use crate::gateway::types::ImportSkipReason;
use labby_runtime::gateway_config::UpstreamImportTombstone;

use super::*;

#[test]
fn auto_import_partition_skips_name_tombstoned_discovered_server() {
    let cfg = GatewayConfig {
        upstream_import_tombstones: vec![UpstreamImportTombstone::now(
            "removed-server",
            fixture_import_source("removed-server"),
        )],
        ..GatewayConfig::default()
    };

    let (result, specs_to_add) =
        partition_discovered_for_import(&cfg, vec![fixture_discovered_http("removed-server")]);

    assert!(specs_to_add.is_empty());
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].name, "removed-server");
    assert_eq!(result.skipped[0].reason, ImportSkipReason::Tombstoned);
}

#[test]
fn auto_import_partition_skips_source_tombstone_after_lab_rename() {
    let cfg = GatewayConfig {
        upstream_import_tombstones: vec![UpstreamImportTombstone::now(
            "renamed-in-lab",
            fixture_import_source("original-config-name"),
        )],
        ..GatewayConfig::default()
    };

    let (result, specs_to_add) = partition_discovered_for_import(
        &cfg,
        vec![fixture_discovered_http("original-config-name")],
    );

    assert!(specs_to_add.is_empty());
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].name, "original-config-name");
    assert_eq!(result.skipped[0].reason, ImportSkipReason::Tombstoned);
}

#[test]
fn auto_import_partition_does_not_source_match_legacy_tombstone_without_server_name() {
    let cfg = GatewayConfig {
        upstream_import_tombstones: vec![UpstreamImportTombstone::now(
            "old-removed-server",
            ImportSource::new(
                "codex",
                "/home/alice/.codex/config.toml",
                "2026-05-15T00:00:00Z",
            ),
        )],
        ..GatewayConfig::default()
    };

    let (result, specs_to_add) = partition_discovered_for_import(
        &cfg,
        vec![fixture_discovered_http("different-server-same-file")],
    );

    assert!(result.skipped.is_empty());
    assert_eq!(specs_to_add.len(), 1);
    assert_eq!(specs_to_add[0].name, "different-server-same-file");
}

#[test]
fn auto_import_partition_does_not_tombstone_same_name_from_different_source() {
    let cfg = GatewayConfig {
        upstream_import_tombstones: vec![UpstreamImportTombstone::now(
            "shared-name",
            fixture_import_source("shared-name"),
        )],
        ..GatewayConfig::default()
    };
    let mut discovered = fixture_discovered_http("shared-name");
    discovered.source_client = "claude-code".to_string();
    discovered.source_path = "/home/alice/.claude/settings.json".to_string();

    let (result, specs_to_add) = partition_discovered_for_import(&cfg, vec![discovered]);

    assert!(result.skipped.is_empty());
    assert_eq!(specs_to_add.len(), 1);
    assert_eq!(specs_to_add[0].name, "shared-name");
}

#[test]
fn auto_import_partition_does_not_tombstone_same_source_when_fingerprint_changes() {
    let source =
        fixture_import_source("fingerprinted").with_transport_fingerprint("old-fingerprint");
    let cfg = GatewayConfig {
        upstream_import_tombstones: vec![UpstreamImportTombstone::now("fingerprinted", source)],
        ..GatewayConfig::default()
    };
    let mut discovered = fixture_discovered_http("fingerprinted");
    discovered.spec.imported_from =
        Some(fixture_import_source("fingerprinted").with_transport_fingerprint("new-fingerprint"));

    let (result, specs_to_add) = partition_discovered_for_import(&cfg, vec![discovered]);

    assert!(result.skipped.is_empty());
    assert_eq!(specs_to_add.len(), 1);
    assert_eq!(specs_to_add[0].name, "fingerprinted");
}

#[tokio::test]
async fn approve_pending_import_persists_through_injected_store() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let store = Arc::new(SlowPersistStore {
        calls: Arc::clone(&calls),
        delay: Duration::from_millis(0),
    });
    let manager = GatewayManager::with_store(
        std::fs::canonicalize(dir.path())
            .unwrap()
            .join("config.toml"),
        GatewayRuntimeHandle::default(),
        store,
    );
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            upstream_pending: vec![fixture_http_upstream("pending")],
            ..GatewayConfig::default()
        })
        .await;

    manager
        .approve_pending_import("pending")
        .await
        .expect("approve pending import");

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let cfg = manager.current_config().await;
    assert!(
        cfg.upstream
            .iter()
            .any(|upstream| upstream.name == "pending")
    );
    assert!(cfg.upstream_pending.is_empty());
}

#[tokio::test]
async fn reject_pending_import_persists_through_injected_store() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let store = Arc::new(SlowPersistStore {
        calls: Arc::clone(&calls),
        delay: Duration::from_millis(0),
    });
    let manager = GatewayManager::with_store(
        std::fs::canonicalize(dir.path())
            .unwrap()
            .join("config.toml"),
        GatewayRuntimeHandle::default(),
        store,
    );
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            upstream_pending: vec![fixture_discovered_http("pending").spec],
            ..GatewayConfig::default()
        })
        .await;

    manager
        .reject_pending_import("pending")
        .await
        .expect("reject pending import");

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let cfg = manager.current_config().await;
    assert!(cfg.upstream_pending.is_empty());
    assert_eq!(cfg.upstream_import_tombstones.len(), 1);
}

async fn stale_discovered_import_preserves_completed_rejection(origin: &str) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let server = fixture_discovered_http("rejected");
    let initial = GatewayConfig {
        upstream_pending: vec![server.spec.clone()],
        ..Default::default()
    };
    crate::gateway::config::write_gateway_config(&path, &initial).unwrap();
    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager.seed_config(initial).await;
    let (_, eligible_before_rejection) =
        partition_discovered_for_import(&manager.current_config().await, vec![server]);
    assert_eq!(eligible_before_rejection.len(), 1);
    manager.reject_pending_import("rejected").await.unwrap();
    let result = manager
        .batch_add(eligible_before_rejection, Some(origin), None)
        .await
        .unwrap();
    let durable = crate::gateway::config::load_gateway_config(&path).unwrap();
    assert!(
        durable.upstream.is_empty(),
        "stale discovery must not restore a rejected entry"
    );
    assert_eq!(durable.upstream_import_tombstones.len(), 1);
    assert!(result.views.is_empty());
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].1.kind(), "import_tombstoned");
}
#[tokio::test]
async fn automatic_import_rechecks_tombstone_after_rejection() {
    stale_discovered_import_preserves_completed_rejection("gateway.auto_import").await;
}
#[tokio::test]
async fn manual_import_rechecks_tombstone_after_rejection() {
    stale_discovered_import_preserves_completed_rejection("gateway.import").await;
}

#[tokio::test]
async fn explicit_add_can_restore_a_rejected_discovered_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let server = fixture_discovered_http("restore-explicit");
    let mut spec = server.spec;
    spec.enabled = false;
    let initial = GatewayConfig {
        upstream_import_tombstones: vec![UpstreamImportTombstone::now(
            &spec.name,
            fixture_import_source("restore-explicit"),
        )],
        ..Default::default()
    };
    crate::gateway::config::write_gateway_config(&path, &initial).unwrap();
    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager.seed_config(initial).await;
    manager
        .add(spec, None, Some("gateway.add"), None)
        .await
        .unwrap();
    let durable = crate::gateway::config::load_gateway_config(&path).unwrap();
    assert_eq!(durable.upstream.len(), 1);
    assert!(durable.upstream_import_tombstones.is_empty());
}
