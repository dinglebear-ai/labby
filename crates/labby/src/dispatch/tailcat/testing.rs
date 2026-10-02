//! Real access-store fixture for native Tailcat authority tests.
use super::{ApprovedPairing, PairingRequest, PendingPairing};
use crate::access::{
    AccessCredentialAdapter, AccessRuntime, AccessStore, ActivateProofInput, ConsumeBootstrapInput,
};
use crate::config::{GatewayLoadoutConfig, LabConfig, ProtectedMcpRouteConfig};
use crate::dispatch::access_bootstrap::GatewayBootstrapPolicyAuthority;
use base64::Engine as _;
use labby_auth::ProductAccessGrantResolver as _;
use labby_primitives::product_credential::{ProductCredential, ProductCredentialVerifier as _};
use sha2::{Digest as _, Sha256};
use std::sync::Arc;

pub(crate) async fn fixture() -> (
    tempfile::TempDir,
    Arc<AccessRuntime>,
    Arc<AccessCredentialAdapter>,
    ApprovedPairing,
) {
    let (directory, runtime, adapter, approved, _manager) = fixture_with_manager().await;
    (directory, runtime, adapter, approved)
}

pub(super) async fn fixture_with_manager() -> (
    tempfile::TempDir,
    Arc<AccessRuntime>,
    Arc<AccessCredentialAdapter>,
    ApprovedPairing,
    labby_gateway::gateway::manager::GatewayManager,
) {
    fixture_with_upstream(None).await
}

pub(super) async fn fixture_with_upstream(
    upstream: Option<crate::config::UpstreamConfig>,
) -> (
    tempfile::TempDir,
    Arc<AccessRuntime>,
    Arc<AccessCredentialAdapter>,
    ApprovedPairing,
    labby_gateway::gateway::manager::GatewayManager,
) {
    let directory = tempfile::Builder::new()
        .prefix("tailcat-authority-")
        .tempdir_in(if cfg!(target_os = "macos") {
            std::path::PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir().canonicalize().unwrap()
        })
        .unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let route: ProtectedMcpRouteConfig = toml::from_str(
        r#"name = "sandbox"
public_host = "labby.example"
public_path = "/sandbox"
scopes = ["lab"]
[target]
kind = "gateway_subset"
project_id = "bootstrap-default"
loadout = "sandbox"
"#,
    )
    .unwrap();
    let loadout = GatewayLoadoutConfig {
        name: "sandbox".into(),
        upstreams: vec!["msb".into()],
        expose_resources: false,
        expose_prompts: false,
        expose_skills: false,
        ..Default::default()
    };
    let config = LabConfig {
        upstream: upstream.into_iter().collect(),
        loadouts: vec![loadout.clone()],
        protected_mcp_routes: vec![route.clone()],
        ..Default::default()
    };
    // Live acceptance reloads through the durable config owner; memory-only
    // seeding is sufficient for authority tests, but cannot survive reload.
    if !config.upstream.is_empty() {
        std::fs::write(
            directory.path().join("gateway.toml"),
            toml::to_string(&config.to_gateway_config()).unwrap(),
        )
        .unwrap();
    }
    let manager = crate::dispatch::gateway::config_store::test_gateway_manager(
        directory.path().join("gateway.toml"),
        Default::default(),
    );
    manager
        .seed_config_unchecked_for_tests(config.to_gateway_config())
        .await;
    let lease = manager
        .acquire_published_bootstrap_policy_lease("sandbox", "sandbox")
        .await
        .unwrap();
    let now = labby_auth::util::now_unix();
    let wire = format!(
        "lby_pc_v1_source_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
    );
    let store = AccessStore::open(directory.path().join("access.db"))
        .await
        .unwrap();
    store
        .activate_bootstrap_proof(ActivateProofInput {
            proof_id: "proof".into(),
            prepare_id: "prepare".into(),
            installation_id: "machine".into(),
            installation_generation: 1,
            proof_digest: [1; 32],
            manifest_digest: [2; 32],
            request_digest: [3; 32],
            idempotency_digest: [4; 32],
            credential_id: "source".into(),
            credential_digest: Sha256::digest(wire.as_bytes()).into(),
            proof_generation: 1,
            created_at: now,
            expires_at: now + 600,
        })
        .await
        .unwrap();
    let identity = labby_auth::VerifiedIdentity::external(
        labby_auth::Authenticator::BrowserSession,
        "https://accounts.google.com",
        "test-operator",
    )
    .unwrap();
    store
        .consume_bootstrap_proof(ConsumeBootstrapInput {
            proof_id: "proof".into(),
            proof_digest: [1; 32],
            request_digest: [3; 32],
            idempotency_digest: [4; 32],
            organization_name: "Local".into(),
            project_name: "Default".into(),
            canonical_issuer: "https://accounts.google.com".into(),
            subject: "test-operator".into(),
            identity_fingerprint: identity.safe_fingerprint(),
            loadout_id: "sandbox".into(),
            loadout_generation: 1,
            catalog_generation: 1,
            loadout_policy_fingerprint: lease.policy_fingerprint(),
            route_id: "sandbox".into(),
            route_generation: 1,
            resource: lease.resource().into(),
            audience: lease.audience().into(),
            scopes_json: "[\"lab\"]".into(),
            now,
            credential_expires_at: now + 3600,
        })
        .await
        .unwrap();
    drop(store);
    drop(lease);
    let runtime = Arc::new(AccessRuntime::initialize(directory.path().join("access.db")).await);
    let adapter = runtime.credential_adapter(Arc::new(GatewayBootstrapPolicyAuthority::new(
        manager.clone(),
        runtime.as_ref().clone(),
    )));
    let credential = ProductCredential::parse(&wire).unwrap();
    let grant = adapter.verify(&credential).await.unwrap();
    let bound = adapter.resolve(&grant).await.unwrap();
    let request = || PairingRequest {
        origin: "https://depot.example".into(),
        peer: format!("nodekey:{}", "1".repeat(64)),
        upstream: "msb".into(),
    };
    let mut pending = PendingPairing::prepare(
        request(),
        bound.clone(),
        "machine",
        &route,
        &loadout,
        now as u64,
    )
    .unwrap();
    let approved = pending
        .approve(&pending.nonce().unwrap(), &request(), &bound, now as u64)
        .unwrap();
    (directory, runtime, adapter, approved, manager)
}
