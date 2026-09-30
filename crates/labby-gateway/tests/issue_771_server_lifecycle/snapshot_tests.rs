use super::snapshot::{ObservationContext, ProtocolEra, ServerSnapshot, SnapshotError};
use rmcp::model::{
    CacheScope, DiscoverResult, Implementation, InitializeResult, ProtocolVersion,
    ServerCapabilities, ServerPeerInfo,
};
use serde_json::json;

pub(super) fn context() -> ObservationContext {
    ObservationContext {
        upstream_name: "configured-alpha".into(),
        config_fingerprint: "safe-definition-reference".into(),
        observation_scope: "opaque-trusted-owner-route".into(),
        catalog_fingerprint: Some("existing-bounded-catalog-fingerprint".into()),
        observed_at_unix_ms: 1_000,
        publication_generation: 7,
    }
}

pub(super) fn modern() -> DiscoverResult {
    let capabilities = serde_json::from_value(json!({
        "tools": {"listChanged": true},
        "extensions": {
            "io.modelcontextprotocol/tasks": {},
            "org.example/vendor": {"nested": {"kept": true}}
        }
    }))
    .unwrap();
    let mut result = DiscoverResult::new(
        vec![ProtocolVersion::V_2026_07_28, ProtocolVersion::V_2025_11_25],
        capabilities,
    )
    .with_ttl_ms(250)
    .with_cache_scope(CacheScope::Public);
    result.instructions = Some("Untrusted upstream instructions".into());
    result.meta =
        Some(serde_json::from_value(json!({"org.example/metadata": {"kept": true}})).unwrap());
    result
}

fn snapshot(result: DiscoverResult) -> ServerSnapshot {
    ServerSnapshot::from_discovery(context(), result, ProtocolVersion::V_2026_07_28).unwrap()
}

#[test]
fn discovery_capture_retains_versions_and_cache_hints() {
    let captured = snapshot(modern());
    let document = serde_json::to_value(captured.raw_discovery().unwrap()).unwrap();
    assert_eq!(
        document["supportedVersions"],
        json!(["2026-07-28", "2025-11-25"])
    );
    assert_eq!(document["ttlMs"], 250);
    assert_eq!(document["cacheScope"], "public");
    assert_eq!(captured.supported_versions().unwrap().len(), 2);
    assert_eq!(captured.effective_version(), &ProtocolVersion::V_2026_07_28);
    assert_eq!(captured.era(), ProtocolEra::Modern20260728);
}

#[test]
fn peer_info_loss_is_characterized_not_silently_fabricated() {
    let info = ServerPeerInfo::from_discover_result(ProtocolVersion::V_2026_07_28, modern());
    let document = serde_json::to_value(info).unwrap();
    for field in ["supportedVersions", "ttlMs", "cacheScope"] {
        assert!(
            document.get(field).is_none(),
            "re-audit capture seam if SDK changes"
        );
    }
}

#[test]
fn extensions_and_unknown_metadata_are_preserved_once() {
    let source = modern();
    let captured = snapshot(source.clone());
    assert_eq!(captured.capabilities(), &source.capabilities);
    assert_eq!(captured.meta(), source.meta.as_ref());
    assert_eq!(captured.instructions(), source.instructions.as_deref());
    let capabilities = serde_json::to_value(captured.capabilities()).unwrap();
    assert_eq!(
        capabilities["extensions"]["org.example/vendor"]["nested"]["kept"],
        true
    );
    assert!(
        capabilities["extensions"]
            .get("io.modelcontextprotocol/tasks")
            .is_some()
    );
}

#[test]
fn spoofed_server_info_cannot_replace_configured_observation_binding() {
    let info: Implementation = serde_json::from_value(json!({
        "name": "admin-other-upstream", "version": "authoritative-looking-version"
    }))
    .unwrap();
    let captured = snapshot(modern().with_server_info(info.clone()));
    assert_eq!(captured.server_info(), Some(info));
    assert_eq!(captured.context().upstream_name, "configured-alpha");
    assert_eq!(
        captured.context().config_fingerprint,
        "safe-definition-reference"
    );
    assert_eq!(
        captured.context().observation_scope,
        "opaque-trusted-owner-route"
    );
}

#[test]
fn missing_identity_does_not_invent_server_authority() {
    assert!(snapshot(modern()).server_info().is_none());
}

#[test]
fn public_cache_hint_never_erases_observation_scope() {
    let captured = snapshot(modern());
    assert_eq!(captured.cache_hints(), Some((250, &CacheScope::Public)));
    assert_eq!(
        captured.context().observation_scope,
        context().observation_scope
    );
}

#[test]
fn freshness_boundary_is_not_task_retention() {
    let captured = snapshot(modern());
    assert_eq!(captured.freshness_deadline_unix_ms(), Some(1_250));
    assert!(!captured.is_fresh_at(999));
    assert!(captured.is_fresh_at(1_000));
    assert!(captured.is_fresh_at(1_249));
    assert!(!captured.is_fresh_at(1_250));
    // Expired observations remain available as stale display data, not authority.
    assert_eq!(captured.capabilities(), &modern().capabilities);
}

#[test]
fn zero_ttl_is_never_a_default_idle_ttl() {
    let captured = snapshot(modern().with_ttl_ms(0));
    assert_eq!(captured.freshness_deadline_unix_ms(), Some(1_000));
    assert!(!captured.is_fresh_at(1_000));
}

#[test]
fn freshness_overflow_fails_closed() {
    let mut scope = context();
    scope.observed_at_unix_ms = u64::MAX;
    assert_eq!(
        ServerSnapshot::from_discovery(scope, modern(), ProtocolVersion::V_2026_07_28).err(),
        Some(SnapshotError::FreshnessOverflow)
    );
}

#[test]
fn unadvertised_effective_version_is_rejected() {
    let mut result = modern();
    result.supported_versions = vec![ProtocolVersion::V_2025_11_25];
    assert_eq!(
        ServerSnapshot::from_discovery(context(), result, ProtocolVersion::V_2026_07_28).err(),
        Some(SnapshotError::EffectiveVersionNotAdvertised)
    );
}

#[test]
fn noncomplete_discovery_cannot_be_cached_as_a_server_snapshot() {
    let mut document = serde_json::to_value(modern()).unwrap();
    document["resultType"] = json!("task");
    let result = serde_json::from_value(document).unwrap();
    assert_eq!(
        ServerSnapshot::from_discovery(context(), result, ProtocolVersion::V_2026_07_28).err(),
        Some(SnapshotError::IncompleteDiscovery)
    );
}

#[test]
fn legacy_is_explicit_and_does_not_invent_discovery_hints() {
    let result = InitializeResult::new(ServerCapabilities::default())
        .with_protocol_version(ProtocolVersion::V_2025_11_25)
        .with_instructions("legacy instructions");
    let captured = ServerSnapshot::from_legacy(context(), result.clone()).unwrap();
    assert_eq!(captured.era(), ProtocolEra::ExplicitLegacy);
    assert_eq!(captured.effective_version(), &ProtocolVersion::V_2025_11_25);
    assert_eq!(captured.capabilities(), &result.capabilities);
    assert_eq!(captured.server_info(), Some(result.server_info));
    assert_eq!(captured.instructions(), Some("legacy instructions"));
    assert!(captured.supported_versions().is_none());
    assert!(captured.cache_hints().is_none());
    assert!(captured.raw_discovery().is_none());
    assert!(!captured.is_fresh_at(1_000));
}

#[test]
fn modern_initialize_and_unknown_versions_do_not_create_legacy_sessions() {
    for version in [
        ProtocolVersion::V_2026_07_28,
        serde_json::from_value(json!("2099-01-01")).unwrap(),
    ] {
        let result = InitializeResult::new(ServerCapabilities::default())
            .with_protocol_version(version.clone());
        assert_eq!(
            ServerSnapshot::from_legacy(context(), result).err(),
            Some(SnapshotError::UnsupportedEra)
        );
        if version != ProtocolVersion::V_2026_07_28 {
            assert_eq!(
                ServerSnapshot::from_discovery(context(), modern(), version).err(),
                Some(SnapshotError::UnsupportedEra)
            );
        }
    }
    assert_eq!(
        ServerSnapshot::from_discovery(context(), modern(), ProtocolVersion::V_2025_11_25).err(),
        Some(SnapshotError::UnsupportedEra)
    );
}

#[test]
fn detached_snapshot_retains_catalog_observation_without_a_peer() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ServerSnapshot>();
    let retained = {
        let original = snapshot(modern());
        original.clone()
    };
    assert_eq!(
        retained.context().catalog_fingerprint,
        context().catalog_fingerprint
    );
    assert_eq!(retained.context().publication_generation, 7);
    assert_eq!(retained.raw_discovery().unwrap(), &modern());
}
