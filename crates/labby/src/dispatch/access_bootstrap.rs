//! Surface-neutral first-owner bootstrap orchestration.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::access::{
    AccessRuntime, AccessRuntimeError, AccessStoreError, ConsumeBootstrapInput, MutationOutcome,
};
#[cfg(feature = "gateway")]
use crate::access::{
    LiveAuthority, LiveAuthorityError, LiveAuthorityFuture, LiveAuthoritySnapshot, StoredBinding,
};
use crate::dispatch::setup::AccessBootstrapManifest;

pub(crate) struct ConsumePreparedBootstrap {
    pub(crate) proof_id: String,
    pub(crate) proof_digest: [u8; 32],
    pub(crate) request_digest: [u8; 32],
    pub(crate) idempotency_digest: [u8; 32],
    pub(crate) manifest: AccessBootstrapManifest,
    pub(crate) now: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum BootstrapConsumeError {
    #[error("bootstrap request is invalid")]
    Invalid,
    #[error("bootstrap proof is not authorized")]
    Unauthorized,
    #[error("published Loadout policy is unavailable")]
    LoadoutUnavailable,
    #[error("bootstrap operation conflicts with current state")]
    Conflict,
    #[error("bootstrap service is busy")]
    Busy,
    #[error("bootstrap service is unavailable")]
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProofServiceError {
    Denied,
    Unavailable,
}

#[derive(Clone, serde::Serialize)]
pub(crate) struct ProofMetadata {
    pub(crate) status: String,
    pub(crate) prepare_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) credential_id: Option<String>,
}

pub(crate) type ProofServiceFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProofMetadata, ProofServiceError>> + Send + 'a>>;

pub(crate) trait AccessBootstrapProofService: Send + Sync {
    fn consume<'a>(
        &'a self,
        proof: &'a str,
        manifest: AccessBootstrapManifest,
    ) -> ProofServiceFuture<'a>;
    fn status<'a>(&'a self, proof: &'a str, prepare_id: &'a str) -> ProofServiceFuture<'a>;
    fn cleanup<'a>(&'a self, proof: &'a str, prepare_id: &'a str) -> ProofServiceFuture<'a>;
}

#[derive(Clone)]
pub(crate) struct DaemonAccessBootstrapProofService {
    runtime: AccessRuntime,
    policy: Arc<dyn BootstrapPolicyAuthority>,
}

impl DaemonAccessBootstrapProofService {
    pub(crate) fn new(runtime: AccessRuntime, policy: Arc<dyn BootstrapPolicyAuthority>) -> Self {
        Self { runtime, policy }
    }
}

impl AccessBootstrapProofService for DaemonAccessBootstrapProofService {
    fn consume<'a>(
        &'a self,
        proof: &'a str,
        manifest: AccessBootstrapManifest,
    ) -> ProofServiceFuture<'a> {
        Box::pin(async move {
            let journal = crate::dispatch::setup::access_bootstrap::authenticate_daemon_prepare(
                proof,
                Some(&manifest),
                None,
            )
            .map_err(|_| ProofServiceError::Denied)?;
            let proof_id = journal.proof_id.clone();
            let proof_digest = digest32(&journal.proof_digest_hex)?;
            let now = unix_seconds()?;
            let outcome = consume_prepared_bootstrap(
                &self.runtime,
                self.policy.as_ref(),
                ConsumePreparedBootstrap {
                    proof_id: proof_id.clone(),
                    proof_digest,
                    request_digest: digest32(&journal.request_digest_hex)?,
                    idempotency_digest: digest32(&journal.idempotency_digest_hex)?,
                    manifest,
                    now,
                },
            )
            .await;
            if matches!(
                outcome,
                Err(BootstrapConsumeError::Invalid
                    | BootstrapConsumeError::Conflict
                    | BootstrapConsumeError::LoadoutUnavailable)
            ) {
                let _ = self
                    .runtime
                    .record_bootstrap_semantic_failure(proof_id, proof_digest, now)
                    .await;
                let _ = self
                    .runtime
                    .record_security_event(
                        "proof".into(),
                        "deny".into(),
                        "semantic_failure".into(),
                        proof_digest,
                        None,
                        now,
                    )
                    .await;
            }
            let outcome = outcome.map_err(|error| match error {
                BootstrapConsumeError::Unauthorized
                | BootstrapConsumeError::Invalid
                | BootstrapConsumeError::Conflict
                | BootstrapConsumeError::LoadoutUnavailable => ProofServiceError::Denied,
                BootstrapConsumeError::Busy | BootstrapConsumeError::Unavailable => {
                    ProofServiceError::Unavailable
                }
            })?;
            let journal =
                crate::dispatch::setup::access_bootstrap::advance_daemon_prepare_consumed(journal)
                    .map_err(|_| ProofServiceError::Unavailable)?;
            Ok(metadata(&journal, outcome))
        })
    }

    fn status<'a>(&'a self, proof: &'a str, prepare_id: &'a str) -> ProofServiceFuture<'a> {
        Box::pin(async move {
            let journal = crate::dispatch::setup::access_bootstrap::authenticate_daemon_prepare(
                proof,
                None,
                Some(prepare_id),
            )
            .map_err(|_| ProofServiceError::Denied)?;
            Ok(metadata(&journal, MutationOutcome::AlreadyApplied))
        })
    }

    fn cleanup<'a>(&'a self, proof: &'a str, prepare_id: &'a str) -> ProofServiceFuture<'a> {
        Box::pin(async move {
            let journal = crate::dispatch::setup::access_bootstrap::authenticate_daemon_prepare(
                proof,
                None,
                Some(prepare_id),
            )
            .map_err(|_| ProofServiceError::Denied)?;
            let _writer = self
                .runtime
                .acquire_bootstrap_writer()
                .await
                .map_err(|_| ProofServiceError::Unavailable)?;
            let journal = crate::dispatch::setup::access_bootstrap::cleanup_daemon_prepare(journal)
                .await
                .map_err(|_| ProofServiceError::Unavailable)?;
            Ok(metadata(&journal, MutationOutcome::AlreadyApplied))
        })
    }
}

fn metadata(
    journal: &crate::dispatch::setup::PrepareJournal,
    _outcome: MutationOutcome,
) -> ProofMetadata {
    ProofMetadata {
        status: serde_json::to_value(journal.state)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unavailable".to_owned()),
        prepare_id: journal.prepare_id.clone(),
        credential_id: matches!(
            journal.state,
            crate::dispatch::setup::PrepareJournalState::Consumed
        )
        .then(|| journal.credential_id.clone()),
    }
}

fn digest32(value: &str) -> Result<[u8; 32], ProofServiceError> {
    hex::decode(value)
        .map_err(|_| ProofServiceError::Denied)?
        .try_into()
        .map_err(|_| ProofServiceError::Denied)
}

fn unix_seconds() -> Result<i64, ProofServiceError> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ProofServiceError::Unavailable)?
        .as_secs();
    i64::try_from(seconds).map_err(|_| ProofServiceError::Unavailable)
}

pub(crate) trait BootstrapPolicyLease: Send {
    fn loadout_id(&self) -> &str;
    fn loadout_generation(&self) -> u64;
    fn catalog_generation(&self) -> u64;
    fn policy_fingerprint(&self) -> [u8; 32];
    fn route_id(&self) -> &str;
    fn route_generation(&self) -> u64;
    fn resource(&self) -> &str;
    fn audience(&self) -> &str;
    fn scopes(&self) -> &[String];
}

type BootstrapPolicyFuture<'a> = Pin<
    Box<
        dyn Future<Output = Result<Box<dyn BootstrapPolicyLease>, BootstrapConsumeError>>
            + Send
            + 'a,
    >,
>;

pub(crate) trait BootstrapPolicyAuthority: Send + Sync {
    fn acquire<'a>(&'a self, loadout_id: &'a str, route_id: &'a str) -> BootstrapPolicyFuture<'a>;
}

/// Lock order is policy lease -> runtime writer -> AccessStore's SQLite
/// transaction. The L4 store closure performs no await, I/O, or callback while
/// SQLite is held; both outer guards remain alive through its commit.
pub(crate) async fn consume_prepared_bootstrap(
    runtime: &AccessRuntime,
    policy: &dyn BootstrapPolicyAuthority,
    request: ConsumePreparedBootstrap,
) -> Result<MutationOutcome, BootstrapConsumeError> {
    validate_request(&request)?;
    let lease = policy
        .acquire(&request.manifest.loadout_id, &request.manifest.route_id)
        .await?;
    validate_policy(lease.as_ref(), &request.manifest)?;
    let _writer = runtime
        .acquire_bootstrap_writer()
        .await
        .map_err(map_runtime_error)?;
    let expires_at = request
        .now
        .checked_add(
            i64::try_from(request.manifest.ttl_seconds)
                .map_err(|_| BootstrapConsumeError::Invalid)?,
        )
        .ok_or(BootstrapConsumeError::Invalid)?;
    let identity_fingerprint = labby_auth::PrincipalLink::External {
        issuer: request.manifest.canonical_issuer.clone(),
        subject: request.manifest.subject.clone(),
    }
    .safe_fingerprint();
    let input = ConsumeBootstrapInput {
        proof_id: request.proof_id,
        proof_digest: request.proof_digest,
        request_digest: request.request_digest,
        idempotency_digest: request.idempotency_digest,
        organization_name: request.manifest.organization_name,
        project_name: request.manifest.project_name,
        canonical_issuer: request.manifest.canonical_issuer,
        subject: request.manifest.subject,
        identity_fingerprint,
        loadout_id: lease.loadout_id().to_owned(),
        loadout_generation: to_i64(lease.loadout_generation())?,
        catalog_generation: to_i64(lease.catalog_generation())?,
        loadout_policy_fingerprint: lease.policy_fingerprint(),
        route_id: lease.route_id().to_owned(),
        route_generation: to_i64(lease.route_generation())?,
        resource: lease.resource().to_owned(),
        audience: lease.audience().to_owned(),
        scopes_json: serde_json::to_string(lease.scopes())
            .map_err(|_| BootstrapConsumeError::Invalid)?,
        now: request.now,
        credential_expires_at: expires_at,
    };
    runtime
        .consume_prepared_bootstrap(input)
        .await
        .map_err(|error| {
            tracing::warn!(
                subsystem = "access_bootstrap",
                error_kind = store_error_kind(&error),
                "prepared bootstrap consume failed"
            );
            map_store_error(error)
        })
}

fn store_error_kind(error: &AccessStoreError) -> &'static str {
    match error {
        AccessStoreError::NotAuthorized => "not_authorized",
        AccessStoreError::BootstrapConflict => "conflict",
        AccessStoreError::InvalidBootstrapInput | AccessStoreError::MalformedVocabulary => {
            "invalid"
        }
        AccessStoreError::Locked => "locked",
        _ => "unavailable",
    }
}

fn validate_request(request: &ConsumePreparedBootstrap) -> Result<(), BootstrapConsumeError> {
    if request.manifest.version != 1
        || request.manifest.ttl_seconds == 0
        || request.manifest.ttl_seconds > 600
        || request.now < 0
        || request.manifest.scopes.is_empty()
    {
        return Err(BootstrapConsumeError::Invalid);
    }
    let mut scopes = request.manifest.scopes.clone();
    scopes.sort();
    scopes.dedup();
    if scopes != request.manifest.scopes {
        return Err(BootstrapConsumeError::Invalid);
    }
    Ok(())
}

fn validate_policy(
    lease: &dyn BootstrapPolicyLease,
    manifest: &AccessBootstrapManifest,
) -> Result<(), BootstrapConsumeError> {
    if lease.loadout_id() != manifest.loadout_id
        || lease.route_id() != manifest.route_id
        || lease.resource() != manifest.resource
        || lease.audience() != manifest.resource
        || lease.scopes() != manifest.scopes
    {
        return Err(BootstrapConsumeError::LoadoutUnavailable);
    }
    Ok(())
}

fn to_i64(value: u64) -> Result<i64, BootstrapConsumeError> {
    i64::try_from(value).map_err(|_| BootstrapConsumeError::Unavailable)
}

fn map_runtime_error(error: AccessRuntimeError) -> BootstrapConsumeError {
    match error {
        AccessRuntimeError::LifecycleUnavailable | AccessRuntimeError::Blocked(_) => {
            BootstrapConsumeError::Busy
        }
        AccessRuntimeError::SetupRequired(_)
        | AccessRuntimeError::BootstrapConflict
        | AccessRuntimeError::InvalidBootstrapInput => BootstrapConsumeError::Unavailable,
    }
}

fn map_store_error(error: AccessStoreError) -> BootstrapConsumeError {
    match error {
        AccessStoreError::NotAuthorized => BootstrapConsumeError::Unauthorized,
        AccessStoreError::BootstrapConflict => BootstrapConsumeError::Conflict,
        AccessStoreError::InvalidBootstrapInput | AccessStoreError::MalformedVocabulary => {
            BootstrapConsumeError::Invalid
        }
        AccessStoreError::Locked => BootstrapConsumeError::Busy,
        _ => BootstrapConsumeError::Unavailable,
    }
}

#[cfg(feature = "gateway")]
#[derive(Clone)]
pub(crate) struct GatewayBootstrapPolicyAuthority {
    manager: labby_gateway::gateway::manager::GatewayManager,
    access_runtime: AccessRuntime,
}

#[cfg(feature = "gateway")]
impl GatewayBootstrapPolicyAuthority {
    pub(crate) fn new(
        manager: labby_gateway::gateway::manager::GatewayManager,
        access_runtime: AccessRuntime,
    ) -> Self {
        Self {
            manager,
            access_runtime,
        }
    }
}

#[cfg(feature = "gateway")]
struct GatewayPolicyLease(labby_gateway::gateway::manager::PublishedBootstrapPolicyLease);

#[cfg(feature = "gateway")]
impl BootstrapPolicyLease for GatewayPolicyLease {
    fn loadout_id(&self) -> &str {
        self.0.loadout_id()
    }
    fn loadout_generation(&self) -> u64 {
        self.0.loadout_generation()
    }
    fn catalog_generation(&self) -> u64 {
        self.0.catalog_generation()
    }
    fn policy_fingerprint(&self) -> [u8; 32] {
        self.0.policy_fingerprint()
    }
    fn route_id(&self) -> &str {
        self.0.route_id()
    }
    fn route_generation(&self) -> u64 {
        self.0.route_generation()
    }
    fn resource(&self) -> &str {
        self.0.resource()
    }
    fn audience(&self) -> &str {
        self.0.audience()
    }
    fn scopes(&self) -> &[String] {
        self.0.scopes()
    }
}

#[cfg(feature = "gateway")]
impl BootstrapPolicyAuthority for GatewayBootstrapPolicyAuthority {
    fn acquire<'a>(&'a self, loadout_id: &'a str, route_id: &'a str) -> BootstrapPolicyFuture<'a> {
        Box::pin(async move {
            let lease = self
                .manager
                .acquire_published_bootstrap_policy_lease(loadout_id, route_id)
                .await
                .map_err(|_| BootstrapConsumeError::LoadoutUnavailable)?;
            let lease: Box<dyn BootstrapPolicyLease> = Box::new(GatewayPolicyLease(lease));
            Ok(lease)
        })
    }
}

/// Production uncached verifier source over the same published policy lease
/// used during bootstrap admission.
#[cfg(feature = "gateway")]
impl LiveAuthority for GatewayBootstrapPolicyAuthority {
    fn resolve<'a>(&'a self, binding: &'a StoredBinding) -> LiveAuthorityFuture<'a> {
        Box::pin(async move {
            let lease = self
                .manager
                .acquire_published_bootstrap_policy_lease(&binding.loadout_id, &binding.route_id)
                .await
                .map_err(|_| LiveAuthorityError::Unavailable)?;
            // The lease remains alive while the access runtime takes its
            // bounded writer permit and commits fingerprint reconciliation.
            // A publisher therefore cannot interleave a new policy between
            // the observed snapshot and its durable epoch assignment.
            let durable_epoch = self
                .access_runtime
                .reconcile_project_policy(binding.project_id.clone(), lease.policy_fingerprint())
                .await
                .map_err(|_| LiveAuthorityError::Unavailable)?;
            Ok(LiveAuthoritySnapshot {
                loadout_id: lease.loadout_id().to_owned(),
                loadout_generation: durable_epoch,
                assignment_generation: binding.assignment_generation,
                catalog_generation: durable_epoch,
                route_id: lease.route_id().to_owned(),
                route_generation: durable_epoch,
                resource: lease.resource().to_owned(),
                audience: lease.audience().to_owned(),
                scopes: lease.scopes().to_vec(),
                requires_admin: lease.scopes().iter().any(|scope| scope == "lab:admin"),
                destructive: false,
                policy_fingerprint: lease.policy_fingerprint(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lease {
        scopes: Vec<String>,
    }

    impl BootstrapPolicyLease for Lease {
        fn loadout_id(&self) -> &'static str {
            "production"
        }
        fn loadout_generation(&self) -> u64 {
            2
        }
        fn catalog_generation(&self) -> u64 {
            3
        }
        fn policy_fingerprint(&self) -> [u8; 32] {
            [4; 32]
        }
        fn route_id(&self) -> &'static str {
            "operator"
        }
        fn route_generation(&self) -> u64 {
            5
        }
        fn resource(&self) -> &'static str {
            "https://mcp.example/operator"
        }
        fn audience(&self) -> &'static str {
            "https://mcp.example/operator"
        }
        fn scopes(&self) -> &[String] {
            &self.scopes
        }
    }

    fn manifest() -> AccessBootstrapManifest {
        AccessBootstrapManifest {
            version: 1,
            installation_id: "install".into(),
            canonical_issuer: "urn:labby:local-operator:install".into(),
            organization_name: "Org".into(),
            project_name: "Project".into(),
            subject: "owner".into(),
            loadout_id: "production".into(),
            route_id: "operator".into(),
            resource: "https://mcp.example/operator".into(),
            scopes: vec!["lab:read".into()],
            ttl_seconds: 60,
            credential_id: "credential".into(),
            idempotency_key: "idempotency".into(),
        }
    }

    #[test]
    fn published_policy_must_match_every_bound_manifest_field() {
        let lease = Lease {
            scopes: vec!["lab:read".into()],
        };
        assert_eq!(validate_policy(&lease, &manifest()), Ok(()));
        let mut wrong = manifest();
        wrong.resource.push_str("/other");
        assert_eq!(
            validate_policy(&lease, &wrong),
            Err(BootstrapConsumeError::LoadoutUnavailable)
        );
    }

    #[test]
    fn request_requires_canonical_scope_order() {
        let mut manifest = manifest();
        manifest.scopes = vec!["lab:write".into(), "lab:read".into()];
        assert_eq!(
            validate_request(&ConsumePreparedBootstrap {
                proof_id: "proof".into(),
                proof_digest: [1; 32],
                request_digest: [2; 32],
                idempotency_digest: [3; 32],
                manifest,
                now: 1,
            }),
            Err(BootstrapConsumeError::Invalid)
        );
    }
}
