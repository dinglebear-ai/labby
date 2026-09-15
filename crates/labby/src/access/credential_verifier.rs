//! Bounded, uncached product-credential verification against live authority.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use labby_auth::{
    ProductAccessGrantResolutionFuture, ProductAccessGrantResolver, ProjectSessionBinding,
    ProjectSessionRevalidationError, ProjectSessionRevalidationFuture, ProjectSessionRevalidator,
};
use labby_primitives::product_credential::{
    BoundAccessGrant, PRODUCT_CREDENTIAL_PREFIX, ProductCredential, ProductCredentialGrant,
    ProductCredentialVerificationError, ProductCredentialVerificationFuture,
    ProductCredentialVerifier,
};
use rusqlite::OptionalExtension as _;
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use tokio::sync::Semaphore;

use super::error::AccessStoreError;
use super::runtime::AccessRuntime;
use super::store::AccessStore;

const READ_CONNECTIONS: usize = 4;
const MAX_OUTSTANDING_READS: usize = 32;
// Credential verification performs several serialized access-store operations
// (admission, read, and audit). Allow normal concurrent requests to wait for
// that short critical section without turning transient contention into a 502.
const QUEUE_DEADLINE: Duration = Duration::from_secs(1);

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct LiveAuthoritySnapshot {
    pub(crate) loadout_id: String,
    pub(crate) loadout_generation: u64,
    pub(crate) assignment_generation: u64,
    pub(crate) catalog_generation: u64,
    pub(crate) policy_fingerprint: [u8; 32],
    pub(crate) route_id: String,
    pub(crate) route_generation: u64,
    pub(crate) resource: String,
    pub(crate) audience: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) requires_admin: bool,
    pub(crate) destructive: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LiveAuthorityError {
    Denied,
    Unavailable,
}

pub(crate) type LiveAuthorityFuture<'a> =
    Pin<Box<dyn Future<Output = Result<LiveAuthoritySnapshot, LiveAuthorityError>> + Send + 'a>>;

/// Gateway-owned live policy seam. Implementations must read the currently
/// published immutable snapshot; this layer deliberately performs no caching.
pub(crate) trait LiveAuthority: Send + Sync {
    fn resolve<'a>(&'a self, binding: &'a StoredBinding) -> LiveAuthorityFuture<'a>;
}

impl<F> LiveAuthority for F
where
    F: Send + Sync + for<'a> Fn(&'a StoredBinding) -> LiveAuthorityFuture<'a>,
{
    fn resolve<'a>(&'a self, binding: &'a StoredBinding) -> LiveAuthorityFuture<'a> {
        self(binding)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct StoredBinding {
    pub(crate) installation_id: String,
    pub(crate) issuer: String,
    pub(crate) subject: String,
    pub(crate) principal_id: String,
    pub(crate) organization_id: String,
    pub(crate) project_id: String,
    pub(crate) loadout_id: String,
    pub(crate) loadout_generation: u64,
    pub(crate) assignment_generation: u64,
    pub(crate) catalog_generation: u64,
    pub(crate) policy_fingerprint: [u8; 32],
    pub(crate) route_id: String,
    pub(crate) route_generation: u64,
    pub(crate) membership_epoch: u64,
    pub(crate) organization_policy_epoch: u64,
    pub(crate) project_policy_epoch: u64,
    pub(crate) credential_id: String,
    pub(crate) credential_generation: u64,
    pub(crate) scopes: Vec<String>,
    pub(crate) resource: String,
    pub(crate) audience: String,
    pub(crate) expires_at: u64,
}

#[derive(Clone)]
pub(super) struct CredentialReadPool {
    stores: Arc<[AccessStore]>,
    outstanding: Arc<Semaphore>,
    executions: Arc<Semaphore>,
    cursor: Arc<AtomicUsize>,
}

impl CredentialReadPool {
    pub(super) fn from_store(store: AccessStore) -> Self {
        Self {
            stores: (0..READ_CONNECTIONS)
                .map(|_| store.clone())
                .collect::<Vec<_>>()
                .into(),
            outstanding: Arc::new(Semaphore::new(MAX_OUTSTANDING_READS)),
            executions: Arc::new(Semaphore::new(READ_CONNECTIONS)),
            cursor: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(super) async fn open(path: &Path) -> Result<Self, AccessStoreError> {
        let store = AccessStore::open_existing_current(path.to_path_buf()).await?;
        Ok(Self::from_store(store))
    }

    async fn read(
        &self,
        credential_id: String,
        digest: Option<[u8; 32]>,
    ) -> Result<StoredBinding, ProductCredentialVerificationError> {
        let admission = tokio::time::timeout(
            QUEUE_DEADLINE,
            Arc::clone(&self.outstanding).acquire_owned(),
        )
        .await
        .map_err(|_| ProductCredentialVerificationError::Unavailable)?
        .map_err(|_| ProductCredentialVerificationError::Unavailable)?;
        let execution =
            tokio::time::timeout(QUEUE_DEADLINE, Arc::clone(&self.executions).acquire_owned())
                .await
                .map_err(|_| ProductCredentialVerificationError::Unavailable)?
                .map_err(|_| ProductCredentialVerificationError::Unavailable)?;
        let index = self.cursor.fetch_add(1, Ordering::Relaxed) % self.stores.len();
        let result = read_current_binding(self.stores[index].clone(), credential_id, digest).await;
        drop(execution);
        drop(admission);
        result
    }
}

#[derive(Clone)]
pub(crate) struct AccessCredentialAdapter {
    runtime: AccessRuntime,
    live: Arc<dyn LiveAuthority>,
    /// Raw-credential attempts between the read-only budget check and the
    /// failure charge. Holding a permit across check, guess, and charge
    /// bounds check-then-charge overshoot to `MAX_UNVERIFIED_ATTEMPTS`.
    unverified_attempts: Arc<Semaphore>,
    #[cfg(test)]
    guesses_evaluated: Arc<AtomicUsize>,
}

pub(crate) struct ProtectedCredentialRequirements<'a> {
    pub(crate) route_id: &'a str,
    pub(crate) resource: &'a str,
    pub(crate) project_id: Option<&'a str>,
    pub(crate) loadout_id: Option<&'a str>,
    pub(crate) scopes: &'a [String],
}

pub(crate) struct VerifiedProductBinding {
    pub(crate) source: ProductCredentialGrant,
    pub(crate) bound: BoundAccessGrant,
}

impl AccessCredentialAdapter {
    pub(crate) fn new(runtime: AccessRuntime, live: Arc<dyn LiveAuthority>) -> Self {
        Self {
            runtime,
            live,
            unverified_attempts: Arc::new(Semaphore::new(MAX_UNVERIFIED_ATTEMPTS)),
            #[cfg(test)]
            guesses_evaluated: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Verify a product credential and bind it to one exact protected route.
    /// Transport adapters select the credential and map the typed denial; all
    /// authority tuple comparisons remain here beside live grant resolution.
    pub(crate) async fn bind_protected_route(
        &self,
        credential: &ProductCredential,
        required: ProtectedCredentialRequirements<'_>,
    ) -> Result<VerifiedProductBinding, ProductCredentialVerificationError> {
        let source = self.verify(credential).await?;
        let bound = self.resolve(&source).await?;
        let target_matches = required
            .project_id
            .is_none_or(|project| project == bound.project_id)
            && required
                .loadout_id
                .is_none_or(|loadout| loadout == bound.loadout_id);
        let source_matches = source.issuer == bound.issuer
            && source.subject == bound.subject
            && source.credential_id == bound.credential_id
            && source.credential_generation == bound.credential_generation
            && source.scopes == bound.scopes
            && source.resource == bound.resource
            && source.audience == bound.audience
            && source.expires_at == bound.expires_at;
        let scopes_match = scopes_within(required.scopes, &bound.scopes);
        if !target_matches
            || !source_matches
            || bound.route_id != required.route_id
            || bound.resource != required.resource
            || !scopes_match
        {
            return Err(ProductCredentialVerificationError::Denied);
        }
        Ok(VerifiedProductBinding { source, bound })
    }

    /// Refuse a verification attempt once the credential (or the whole
    /// installation) has exhausted its failed-attempt budget. This is a
    /// read-only check: successful verifications never consume the budget,
    /// so a legitimate client is not throttled by its own traffic. Failures
    /// are charged by [`Self::charge_failed_credential_attempt`].
    async fn admit_credential_attempt(
        &self,
        credential_id: &str,
    ) -> Result<(), ProductCredentialVerificationError> {
        let now = unix_now_for_verification()?;
        let target = credential_bucket(credential_id);
        let global_exhausted = self
            .runtime
            .security_operation_exhausted(
                "credential_global".into(),
                credential_global_bucket(),
                now,
                CREDENTIAL_FAILURE_WINDOW_SECONDS,
                CREDENTIAL_GLOBAL_FAILURE_LIMIT,
            )
            .await
            .map_err(|_| ProductCredentialVerificationError::Unavailable)?;
        let target_exhausted = self
            .runtime
            .security_operation_exhausted(
                "credential_peer".into(),
                target,
                now,
                CREDENTIAL_FAILURE_WINDOW_SECONDS,
                CREDENTIAL_PEER_FAILURE_LIMIT,
            )
            .await
            .map_err(|_| ProductCredentialVerificationError::Unavailable)?;
        if !global_exhausted && !target_exhausted {
            return Ok(());
        }
        self.runtime
            .record_security_event_or_warn(
                "credential_verify",
                "deny",
                "rate_limited",
                target,
                None,
                now,
            )
            .await;
        Err(ProductCredentialVerificationError::Denied)
    }

    /// Charge one failed verification against the per-credential and
    /// installation-wide budgets. Only denials are charged; an unavailable
    /// store is an outage, not a guess.
    async fn charge_failed_credential_attempt(&self, credential_id: &str) {
        let Ok(now) = unix_now_for_verification() else {
            return;
        };
        let target = credential_bucket(credential_id);
        let charges = [
            (
                "credential_global",
                credential_global_bucket(),
                CREDENTIAL_GLOBAL_FAILURE_LIMIT,
            ),
            ("credential_peer", target, CREDENTIAL_PEER_FAILURE_LIMIT),
        ];
        for (class, bucket, limit) in charges {
            if let Err(error) = self
                .runtime
                .admit_security_operation(
                    class.into(),
                    bucket,
                    now,
                    CREDENTIAL_FAILURE_WINDOW_SECONDS,
                    limit,
                )
                .await
            {
                tracing::warn!(error = ?error, class, "failed credential attempt not charged");
            }
        }
    }

    async fn resolve_binding(
        &self,
        credential_id: String,
        digest: Option<[u8; 32]>,
    ) -> Result<BoundAccessGrant, ProductCredentialVerificationError> {
        let now = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| ProductCredentialVerificationError::Unavailable)?
                .as_secs(),
        )
        .map_err(|_| ProductCredentialVerificationError::Unavailable)?;
        let target: [u8; 32] = Sha256::digest(credential_id.as_bytes()).into();
        let pool = self.runtime.credential_reads().await.map_err(|_| {
            tracing::warn!(phase = "runtime_pool", "credential resolution unavailable");
            ProductCredentialVerificationError::Unavailable
        })?;
        let stored = match pool.read(credential_id, digest).await {
            Ok(stored) => stored,
            Err(error) => {
                tracing::warn!(phase = "stored_binding", "credential resolution failed");
                self.runtime
                    .record_security_event_or_warn(
                        "credential_verify",
                        "deny",
                        "credential_denied",
                        target,
                        None,
                        now,
                    )
                    .await;
                return Err(error);
            }
        };
        let live = match self.live.resolve(&stored).await {
            Ok(live) => live,
            Err(error) => {
                let (mapped, reason) = match error {
                    LiveAuthorityError::Denied => (
                        ProductCredentialVerificationError::Denied,
                        "authority_denied",
                    ),
                    LiveAuthorityError::Unavailable => {
                        tracing::warn!(
                            phase = "live_authority",
                            "credential resolution unavailable"
                        );
                        (
                            ProductCredentialVerificationError::Unavailable,
                            "authority_unavailable",
                        )
                    }
                };
                self.runtime
                    .record_security_event_or_warn(
                        "credential_verify",
                        "deny",
                        reason,
                        target,
                        None,
                        now,
                    )
                    .await;
                return Err(mapped);
            }
        };
        if !live_matches(&live, &stored) {
            self.runtime
                .record_security_event_or_warn(
                    "credential_verify",
                    "deny",
                    "binding_mismatch",
                    target,
                    None,
                    now,
                )
                .await;
            return Err(ProductCredentialVerificationError::Denied);
        }
        self.runtime
            .record_security_event_or_warn(
                "credential_verify",
                "allow",
                "verified",
                target,
                None,
                now,
            )
            .await;
        Ok(BoundAccessGrant {
            installation_id: stored.installation_id,
            issuer: stored.issuer,
            subject: stored.subject,
            principal_id: stored.principal_id,
            organization_id: stored.organization_id,
            project_id: stored.project_id,
            loadout_id: stored.loadout_id,
            loadout_generation: stored.loadout_generation,
            assignment_generation: stored.assignment_generation,
            catalog_generation: stored.catalog_generation,
            route_id: stored.route_id,
            route_generation: stored.route_generation,
            membership_epoch: stored.membership_epoch,
            organization_policy_epoch: stored.organization_policy_epoch,
            project_policy_epoch: stored.project_policy_epoch,
            credential_id: stored.credential_id,
            credential_generation: stored.credential_generation,
            scopes: stored.scopes,
            resource: stored.resource,
            audience: stored.audience,
            expires_at: stored.expires_at,
            requires_admin: live.requires_admin,
            destructive: live.destructive,
        })
    }
}

/// Product-credential attempt budget window, shared by verification (charged
/// on failure) and issuance (charged per attempt).
pub(crate) const CREDENTIAL_FAILURE_WINDOW_SECONDS: i64 = 60;
/// Attempts allowed per credential id per window.
pub(crate) const CREDENTIAL_PEER_FAILURE_LIMIT: i64 = 16;
/// Attempts allowed installation-wide per window.
pub(crate) const CREDENTIAL_GLOBAL_FAILURE_LIMIT: i64 = 64;
/// Raw-credential attempts allowed between the read-only budget check and
/// the failure charge. A guess can therefore be evaluated at most this many
/// times beyond the per-window limit.
const MAX_UNVERIFIED_ATTEMPTS: usize = 32;
/// Busy timeout for the uncached credential read; restored afterwards so the
/// shared connection keeps its configured timeout for other operations.
const CREDENTIAL_READ_BUSY_TIMEOUT: Duration = Duration::from_millis(250);

/// Outcome of charging a security-admission budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SecurityAdmission {
    Admitted,
    RateLimited,
    StoreUnavailable,
}

pub(crate) fn classify_security_admission<E>(
    global: Result<bool, E>,
    peer: Result<bool, E>,
) -> SecurityAdmission {
    match (global, peer) {
        (Ok(true), Ok(true)) => SecurityAdmission::Admitted,
        (Ok(_), Ok(_)) => SecurityAdmission::RateLimited,
        (Err(_), _) | (_, Err(_)) => SecurityAdmission::StoreUnavailable,
    }
}

/// Charge one credential-issue attempt by `actor_credential_id` against the
/// shared credential budgets. Issuance is a privileged mutation, so every
/// attempt is charged (unlike verification, which charges only denials).
pub(crate) async fn admit_credential_issue_attempt(
    runtime: &AccessRuntime,
    actor_credential_id: &str,
    now: i64,
) -> SecurityAdmission {
    let global = runtime
        .admit_security_operation(
            "credential_global".into(),
            credential_issue_global_bucket(),
            now,
            CREDENTIAL_FAILURE_WINDOW_SECONDS,
            CREDENTIAL_GLOBAL_FAILURE_LIMIT,
        )
        .await;
    let peer = runtime
        .admit_security_operation(
            "credential_peer".into(),
            credential_bucket(actor_credential_id),
            now,
            CREDENTIAL_FAILURE_WINDOW_SECONDS,
            CREDENTIAL_PEER_FAILURE_LIMIT,
        )
        .await;
    classify_security_admission(global, peer)
}

/// Whether every requested scope is granted by `ceiling`. Credentials may be
/// equal-or-narrower than their authority, never broader.
pub(crate) fn scopes_within(requested: &[String], ceiling: &[String]) -> bool {
    requested.iter().all(|scope| ceiling.contains(scope))
}

/// A credential generation that cannot be represented in the durable store.
/// This is an integrity failure, never an authorization decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("credential generation exceeds the durable range")]
pub(crate) struct CredentialGenerationOutOfRange;

pub(crate) fn credential_generation_sql(
    generation: u64,
) -> Result<i64, CredentialGenerationOutOfRange> {
    i64::try_from(generation).map_err(|_| CredentialGenerationOutOfRange)
}

fn credential_global_bucket() -> [u8; 32] {
    Sha256::digest(b"labby-credential-global-v1").into()
}

fn credential_issue_global_bucket() -> [u8; 32] {
    Sha256::digest(b"labby-credential-issue-global-v1").into()
}

fn credential_bucket(credential_id: &str) -> [u8; 32] {
    Sha256::digest(credential_id.as_bytes()).into()
}

fn unix_now_for_verification() -> Result<i64, ProductCredentialVerificationError> {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ProductCredentialVerificationError::Unavailable)?
            .as_secs(),
    )
    .map_err(|_| ProductCredentialVerificationError::Unavailable)
}

fn live_matches(live: &LiveAuthoritySnapshot, stored: &StoredBinding) -> bool {
    live.loadout_id == stored.loadout_id
        && live.loadout_generation == stored.loadout_generation
        && live.assignment_generation == stored.assignment_generation
        && live.catalog_generation == stored.catalog_generation
        && live.policy_fingerprint == stored.policy_fingerprint
        && live.route_id == stored.route_id
        && live.route_generation == stored.route_generation
        && live.resource == stored.resource
        && live.audience == stored.audience
        // Issued credentials may be equal-or-narrower than their source. The
        // live route is the authority ceiling; requiring exact equality here
        // made every legitimately narrowed credential unverifiable before the
        // route-level scope check could return a stable insufficient-scope
        // denial.
        && scopes_within(&stored.scopes, &live.scopes)
}

impl ProductCredentialVerifier for AccessCredentialAdapter {
    fn verify<'a>(
        &'a self,
        credential: &'a ProductCredential,
    ) -> ProductCredentialVerificationFuture<'a> {
        Box::pin(async move {
            let digest = credential_digest(credential);
            // Held from the budget check through the failure charge so at
            // most MAX_UNVERIFIED_ATTEMPTS guesses are ever uncharged.
            let _attempt = tokio::time::timeout(
                QUEUE_DEADLINE,
                Arc::clone(&self.unverified_attempts).acquire_owned(),
            )
            .await
            .map_err(|_| ProductCredentialVerificationError::Unavailable)?
            .map_err(|_| ProductCredentialVerificationError::Unavailable)?;
            self.admit_credential_attempt(credential.credential_id())
                .await?;
            #[cfg(test)]
            self.guesses_evaluated.fetch_add(1, Ordering::SeqCst);
            let bound = match self
                .resolve_binding(credential.credential_id().to_owned(), Some(digest))
                .await
            {
                Ok(bound) => bound,
                Err(error) => {
                    if error == ProductCredentialVerificationError::Denied {
                        self.charge_failed_credential_attempt(credential.credential_id())
                            .await;
                    }
                    return Err(error);
                }
            };
            Ok(ProductCredentialGrant {
                issuer: bound.issuer,
                subject: bound.subject,
                credential_id: bound.credential_id,
                credential_generation: bound.credential_generation,
                scopes: bound.scopes,
                resource: bound.resource,
                audience: bound.audience,
                expires_at: bound.expires_at,
            })
        })
    }
}

fn credential_digest(credential: &ProductCredential) -> [u8; 32] {
    let wire = format!(
        "{PRODUCT_CREDENTIAL_PREFIX}{}_{}",
        credential.credential_id(),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(credential.secret())
    );
    Sha256::digest(wire.as_bytes()).into()
}

impl ProductAccessGrantResolver for AccessCredentialAdapter {
    fn resolve<'a>(
        &'a self,
        grant: &'a ProductCredentialGrant,
    ) -> ProductAccessGrantResolutionFuture<'a> {
        Box::pin(async move {
            let bound = self
                .resolve_binding(grant.credential_id.clone(), None)
                .await?;
            if bound.issuer != grant.issuer
                || bound.subject != grant.subject
                || bound.credential_generation != grant.credential_generation
                || bound.scopes != grant.scopes
                || bound.resource != grant.resource
                || bound.audience != grant.audience
                || bound.expires_at != grant.expires_at
            {
                return Err(ProductCredentialVerificationError::Denied);
            }
            Ok(bound)
        })
    }
}

impl ProjectSessionRevalidator for AccessCredentialAdapter {
    fn revalidate<'a>(
        &'a self,
        binding: &'a ProjectSessionBinding,
    ) -> ProjectSessionRevalidationFuture<'a> {
        Box::pin(async move {
            // The raw credential's secret was admitted and verified before this
            // session was minted. Revalidation proves the stored binding still
            // matches live authority; it is not another untrusted secret guess
            // and must not consume the brute-force attempt budget.
            let bound = self
                .resolve_binding(binding.source_credential_id.clone(), None)
                .await
                .map_err(|error| match error {
                    ProductCredentialVerificationError::Denied => {
                        ProjectSessionRevalidationError::Denied
                    }
                    ProductCredentialVerificationError::Unavailable => {
                        ProjectSessionRevalidationError::Unavailable
                    }
                })?;
            if ProjectSessionBinding::from(&bound) != *binding {
                return Err(ProjectSessionRevalidationError::Denied);
            }
            Ok(bound)
        })
    }
}

async fn read_current_binding(
    store: AccessStore,
    credential_id: String,
    expected_digest: Option<[u8; 32]>,
) -> Result<StoredBinding, ProductCredentialVerificationError> {
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ProductCredentialVerificationError::Unavailable)?
            .as_secs(),
    )
    .map_err(|_| ProductCredentialVerificationError::Unavailable)?;
    store
        .with_connection(move |connection| {
            // The read pool may share the writer's connection; scope the
            // shortened busy timeout to this read and always restore it.
            let prior_busy_ms: i64 = connection
                .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
                .map_err(super::store::map_sqlite_error)?;
            connection
                .busy_timeout(CREDENTIAL_READ_BUSY_TIMEOUT)
                .map_err(super::store::map_sqlite_error)?;
            let result = query_current_binding(connection, &credential_id, now);
            let restored = connection
                .busy_timeout(Duration::from_millis(
                    u64::try_from(prior_busy_ms).unwrap_or_default(),
                ))
                .map_err(super::store::map_sqlite_error);
            let row = result?;
            restored?;
            if !row.is_current(expected_digest.as_ref()) {
                return Err(AccessStoreError::NotAuthorized);
            }
            Ok(row.binding)
        })
        .await
        .map_err(|error| match error {
            AccessStoreError::NotAuthorized => ProductCredentialVerificationError::Denied,
            _ => {
                tracing::warn!(
                    error_kind = super::runtime::blocked_reason_for_diagnostics(&error),
                    "product credential store read failed"
                );
                ProductCredentialVerificationError::Unavailable
            }
        })
}

/// One decoded credential-currentness row. Every column is bound by alias,
/// so reordering or extending the SELECT cannot silently shift a value into
/// the wrong check.
#[derive(Clone)]
struct CurrentBindingRow {
    credential_digest: Vec<u8>,
    binding: StoredBinding,
    credential_installation_generation: u64,
    live_organization_policy_epoch: u64,
    live_project_policy_epoch: u64,
    membership_status: String,
    project_status: String,
    organization_status: String,
    assigned_loadout_id: String,
    live_membership_generation: u64,
    live_loadout_assignment_generation: u64,
    live_installation_generation: u64,
}

impl CurrentBindingRow {
    fn decode(row: &rusqlite::Row<'_>, credential_id: &str) -> rusqlite::Result<Self> {
        let generation = |alias: &str| row.get::<_, i64>(alias).and_then(to_u64);
        Ok(Self {
            credential_digest: row.get("credential_digest")?,
            binding: StoredBinding {
                installation_id: row.get("installation_id")?,
                issuer: row.get("issuer")?,
                subject: row.get("subject")?,
                principal_id: row.get("principal_id")?,
                organization_id: row.get("organization_id")?,
                project_id: row.get("project_id")?,
                loadout_id: row.get("loadout_id")?,
                loadout_generation: generation("loadout_generation")?,
                assignment_generation: generation("assignment_generation")?,
                catalog_generation: generation("catalog_generation")?,
                policy_fingerprint: row
                    .get::<_, Vec<u8>>("policy_fingerprint")?
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                route_id: row.get("route_id")?,
                route_generation: generation("route_generation")?,
                membership_epoch: generation("membership_epoch")?,
                organization_policy_epoch: generation("organization_policy_epoch")?,
                project_policy_epoch: generation("project_policy_epoch")?,
                credential_id: credential_id.to_owned(),
                credential_generation: generation("credential_generation")?,
                scopes: serde_json::from_str(&row.get::<_, String>("scopes_json")?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                resource: row.get("resource")?,
                audience: row.get("audience")?,
                expires_at: generation("expires_at")?,
            },
            credential_installation_generation: generation("credential_installation_generation")?,
            live_organization_policy_epoch: generation("live_organization_policy_epoch")?,
            live_project_policy_epoch: generation("live_project_policy_epoch")?,
            membership_status: row.get("membership_status")?,
            project_status: row.get("project_status")?,
            organization_status: row.get("organization_status")?,
            assigned_loadout_id: row.get("assigned_loadout_id")?,
            live_membership_generation: generation("live_membership_generation")?,
            live_loadout_assignment_generation: generation("live_loadout_assignment_generation")?,
            live_installation_generation: generation("live_installation_generation")?,
        })
    }

    /// Whether the stored credential still matches live durable authority:
    /// the presented secret (when verifying a raw credential), every policy
    /// epoch and generation the credential was minted under, and the active
    /// status of its organization, project, and membership.
    fn is_current(&self, expected_digest: Option<&[u8; 32]>) -> bool {
        let digest_matches = expected_digest.is_none_or(|expected| {
            self.credential_digest.len() == 32
                && bool::from(self.credential_digest.as_slice().ct_eq(expected.as_slice()))
        });
        digest_matches
            && self.live_organization_policy_epoch == self.binding.organization_policy_epoch
            && self.live_project_policy_epoch == self.binding.project_policy_epoch
            && self.membership_status == "active"
            && self.project_status == "active"
            && self.organization_status == "active"
            && self.assigned_loadout_id == self.binding.loadout_id
            && self.live_membership_generation == self.binding.membership_epoch
            && self.live_loadout_assignment_generation == self.binding.assignment_generation
            && self.credential_installation_generation == self.live_installation_generation
    }
}

fn query_current_binding(
    connection: &rusqlite::Connection,
    credential_id: &str,
    now: i64,
) -> Result<CurrentBindingRow, AccessStoreError> {
    connection
        .query_row(
            "SELECT c.credential_digest AS credential_digest,
                    c.installation_id AS installation_id,
                    c.canonical_issuer AS issuer,
                    c.subject AS subject,
                    c.principal_id AS principal_id,
                    c.organization_id AS organization_id,
                    c.project_id AS project_id,
                    c.loadout_id AS loadout_id,
                    c.loadout_generation AS loadout_generation,
                    c.loadout_assignment_generation AS assignment_generation,
                    c.catalog_generation AS catalog_generation,
                    c.loadout_policy_fingerprint AS policy_fingerprint,
                    c.route_id AS route_id,
                    c.route_generation AS route_generation,
                    c.membership_generation AS membership_epoch,
                    c.organization_policy_epoch AS organization_policy_epoch,
                    c.project_policy_epoch AS project_policy_epoch,
                    c.credential_generation AS credential_generation,
                    c.scopes_json AS scopes_json,
                    c.resource AS resource,
                    c.audience AS audience,
                    c.expires_at AS expires_at,
                    c.installation_generation AS credential_installation_generation,
                    o.policy_epoch AS live_organization_policy_epoch,
                    p.project_policy_epoch AS live_project_policy_epoch,
                    m.status AS membership_status,
                    p.status AS project_status,
                    o.status AS organization_status,
                    pl.loadout_name AS assigned_loadout_id,
                    m.updated_at AS live_membership_generation,
                    pl.updated_at AS live_loadout_assignment_generation,
                    i.installation_generation AS live_installation_generation
             FROM project_credentials c
             JOIN access_installations i ON i.installation_id=c.installation_id
             JOIN organizations o ON o.organization_id=c.organization_id
             JOIN projects p ON p.organization_id=c.organization_id AND p.project_id=c.project_id
             JOIN project_memberships m ON m.organization_id=c.organization_id
               AND m.project_id=c.project_id AND m.principal_id=c.principal_id
             JOIN project_loadouts pl ON pl.organization_id=c.organization_id
               AND pl.project_id=c.project_id
             WHERE c.credential_id=?1 AND c.status='active' AND c.expires_at>?2
               AND NOT EXISTS (SELECT 1 FROM access_tombstones t
                 WHERE t.artifact_kind='credential'
                   AND (t.public_id=c.credential_id OR t.canonical_digest=c.credential_digest))",
            rusqlite::params![credential_id, now],
            |row| CurrentBindingRow::decode(row, credential_id),
        )
        .optional()
        .map_err(super::store::map_sqlite_error)?
        .ok_or(AccessStoreError::NotAuthorized)
}

fn to_u64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DeniedLiveAuthority;

    impl LiveAuthority for DeniedLiveAuthority {
        fn resolve<'a>(&'a self, _: &'a StoredBinding) -> LiveAuthorityFuture<'a> {
            Box::pin(async { Err(LiveAuthorityError::Denied) })
        }
    }

    async fn ready_runtime() -> (tempfile::TempDir, AccessRuntime) {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let path = directory.path().canonicalize().unwrap().join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        store
            .bootstrap_owner(
                super::super::bootstrap::BootstrapOwnerInput::new(
                    labby_auth::VerifiedIdentity::local_credential(
                        labby_auth::Authenticator::StaticBearer,
                        "static-bearer:credential-verifier-test",
                    )
                    .unwrap(),
                    "Local",
                    "Default",
                )
                .unwrap(),
            )
            .await
            .unwrap();
        drop(store);
        let runtime = AccessRuntime::initialize(path).await;
        (directory, runtime)
    }

    fn stored() -> StoredBinding {
        StoredBinding {
            installation_id: "install".into(),
            issuer: "issuer".into(),
            subject: "subject".into(),
            principal_id: "principal".into(),
            organization_id: "org".into(),
            project_id: "project".into(),
            loadout_id: "loadout".into(),
            loadout_generation: 2,
            assignment_generation: 3,
            catalog_generation: 4,
            policy_fingerprint: [42; 32],
            route_id: "route".into(),
            route_generation: 5,
            membership_epoch: 6,
            organization_policy_epoch: 7,
            project_policy_epoch: 8,
            credential_id: "credential".into(),
            credential_generation: 9,
            scopes: vec!["lab:read".into()],
            resource: "lab://project".into(),
            audience: "labby".into(),
            expires_at: 10,
        }
    }

    fn live(binding: &StoredBinding) -> LiveAuthoritySnapshot {
        LiveAuthoritySnapshot {
            loadout_id: binding.loadout_id.clone(),
            loadout_generation: binding.loadout_generation,
            assignment_generation: binding.assignment_generation,
            catalog_generation: binding.catalog_generation,
            route_id: binding.route_id.clone(),
            route_generation: binding.route_generation,
            resource: binding.resource.clone(),
            audience: binding.audience.clone(),
            scopes: binding.scopes.clone(),
            requires_admin: false,
            destructive: false,
            policy_fingerprint: binding.policy_fingerprint,
        }
    }

    /// Live authority that mirrors the stored binding exactly.
    struct MatchingLiveAuthority;

    impl LiveAuthority for MatchingLiveAuthority {
        fn resolve<'a>(&'a self, binding: &'a StoredBinding) -> LiveAuthorityFuture<'a> {
            let snapshot = live(binding);
            Box::pin(async move { Ok(snapshot) })
        }
    }

    const REAL_CREDENTIAL_ID: &str = "credential-real";

    fn wire_credential(secret: u8) -> ProductCredential {
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([secret; 32]);
        ProductCredential::parse(&format!(
            "{PRODUCT_CREDENTIAL_PREFIX}{REAL_CREDENTIAL_ID}_{encoded}"
        ))
        .unwrap()
    }

    /// A Ready runtime holding one genuinely issued product credential whose
    /// secret is `wire_credential(0xA5)`.
    async fn issued_runtime() -> (tempfile::TempDir, AccessRuntime) {
        use super::super::credential_store::{ActivateProofInput, ConsumeBootstrapInput};
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let path = directory.path().canonicalize().unwrap().join("access.db");
        let now = unix_now_for_verification().unwrap();
        let store = AccessStore::open(path.clone()).await.unwrap();
        store
            .activate_bootstrap_proof(ActivateProofInput {
                proof_id: "proof-real".into(),
                prepare_id: "prepare-real".into(),
                installation_id: "installation-real".into(),
                installation_generation: 1,
                proof_digest: [1; 32],
                manifest_digest: [2; 32],
                request_digest: [3; 32],
                idempotency_digest: [4; 32],
                credential_id: REAL_CREDENTIAL_ID.into(),
                credential_digest: credential_digest(&wire_credential(0xA5)),
                proof_generation: 1,
                created_at: now,
                expires_at: now + 600,
            })
            .await
            .unwrap();
        let identity = labby_auth::VerifiedIdentity::external(
            labby_auth::Authenticator::BrowserSession,
            "https://accounts.google.com",
            "operator-real",
        )
        .unwrap();
        store
            .consume_bootstrap_proof(ConsumeBootstrapInput {
                proof_id: "proof-real".into(),
                proof_digest: [1; 32],
                request_digest: [3; 32],
                idempotency_digest: [4; 32],
                organization_name: "Local".into(),
                project_name: "Default".into(),
                canonical_issuer: "https://accounts.google.com".into(),
                subject: "operator-real".into(),
                identity_fingerprint: identity.safe_fingerprint(),
                loadout_id: "production".into(),
                loadout_generation: 1,
                catalog_generation: 1,
                loadout_policy_fingerprint: [6; 32],
                route_id: "root".into(),
                route_generation: 1,
                resource: "http://127.0.0.1/mcp".into(),
                audience: "http://127.0.0.1/mcp".into(),
                scopes_json: r#"["lab:read"]"#.into(),
                now,
                credential_expires_at: now + 3600,
            })
            .await
            .unwrap();
        drop(store);
        (directory, AccessRuntime::initialize(path).await)
    }

    async fn admission_attempts(runtime: &AccessRuntime) -> i64 {
        runtime
            .store()
            .await
            .unwrap()
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT coalesce(sum(attempts),0) FROM access_admission_buckets",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(super::super::store::map_sqlite_error)
            })
            .await
            .unwrap()
    }

    /// TST-H3: a busy valid client is never throttled by its own traffic, and
    /// forged attempts against the same id are still limited.
    #[tokio::test]
    async fn real_credential_success_never_consumes_budget_but_forgeries_still_exhaust_it() {
        let (_directory, runtime) = issued_runtime().await;
        let adapter =
            AccessCredentialAdapter::new(runtime.clone(), Arc::new(MatchingLiveAuthority));
        let valid = wire_credential(0xA5);
        for _ in 0..=CREDENTIAL_GLOBAL_FAILURE_LIMIT {
            let grant = adapter.verify(&valid).await.unwrap();
            assert_eq!(grant.credential_id, REAL_CREDENTIAL_ID);
            assert_eq!(grant.scopes, vec!["lab:read".to_owned()]);
        }
        assert_eq!(admission_attempts(&runtime).await, 0);

        let forged = wire_credential(0x5A);
        for _ in 0..CREDENTIAL_PEER_FAILURE_LIMIT {
            assert_eq!(
                adapter.verify(&forged).await.err(),
                Some(ProductCredentialVerificationError::Denied)
            );
        }
        assert_eq!(
            adapter.verify(&valid).await.err(),
            Some(ProductCredentialVerificationError::Denied),
            "the credential is refused while its failure budget is exhausted"
        );
    }

    /// TST-H3: check-then-charge overshoot is bounded by the unverified
    /// attempt limit even when many forgeries race the budget check.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_forgeries_cannot_overshoot_the_budget_unboundedly() {
        let (_directory, runtime) = issued_runtime().await;
        let adapter =
            AccessCredentialAdapter::new(runtime.clone(), Arc::new(MatchingLiveAuthority));
        let mut attempts = tokio::task::JoinSet::new();
        for _ in 0..128 {
            let adapter = adapter.clone();
            attempts.spawn(async move { adapter.verify(&wire_credential(0x5A)).await.is_ok() });
        }
        while let Some(accepted) = attempts.join_next().await {
            assert!(!accepted.unwrap(), "a forged credential must never verify");
        }
        let evaluated = adapter.guesses_evaluated.load(Ordering::SeqCst);
        let bound =
            usize::try_from(CREDENTIAL_PEER_FAILURE_LIMIT).unwrap() + MAX_UNVERIFIED_ATTEMPTS;
        assert!(
            evaluated <= bound,
            "{evaluated} guesses evaluated; bound is {bound}"
        );
        // Every charged guess increments both buckets; each stays within its
        // own limit and never records more charges than guesses evaluated.
        let (peer, global): (i64, i64) = runtime
            .store()
            .await
            .unwrap()
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT
                           coalesce((SELECT attempts FROM access_admission_buckets WHERE admission_class='credential_peer'),0),
                           coalesce((SELECT attempts FROM access_admission_buckets WHERE admission_class='credential_global'),0)",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(super::super::store::map_sqlite_error)
            })
            .await
            .unwrap();
        assert!(peer <= CREDENTIAL_PEER_FAILURE_LIMIT, "peer bucket {peer}");
        assert!(
            global <= i64::try_from(evaluated).unwrap(),
            "{global} > {evaluated}"
        );
    }

    /// TST-H3: a store outage while resolving a valid credential is an
    /// outage, never a denial, and never charges the failure budget.
    #[tokio::test]
    async fn store_outage_on_the_success_path_is_unavailable_not_denied() {
        let (_directory, runtime) = issued_runtime().await;
        runtime
            .store()
            .await
            .unwrap()
            .with_connection(|connection| {
                connection
                    .execute_batch(
                        "ALTER TABLE project_loadouts RENAME TO project_loadouts_offline",
                    )
                    .map_err(super::super::store::map_sqlite_error)
            })
            .await
            .unwrap();
        let adapter =
            AccessCredentialAdapter::new(runtime.clone(), Arc::new(MatchingLiveAuthority));
        assert_eq!(
            adapter.verify(&wire_credential(0xA5)).await.err(),
            Some(ProductCredentialVerificationError::Unavailable)
        );
        assert_eq!(admission_attempts(&runtime).await, 0);
    }

    /// CQ-H1 / TST-M1: a security event that cannot be written is logged,
    /// redacted, and does not change the verification decision.
    #[tokio::test]
    async fn failed_security_event_write_is_observable_and_redacted() {
        use tracing_subscriber::layer::SubscriberExt as _;
        let (_directory, runtime) = ready_runtime().await;
        runtime
            .store()
            .await
            .unwrap()
            .with_connection(|connection| {
                connection
                    .execute_batch(
                        "CREATE TEMP TRIGGER fail_security_event BEFORE INSERT ON access_security_events BEGIN SELECT RAISE(ABORT,'forced'); END;",
                    )
                    .map_err(super::super::store::map_sqlite_error)
            })
            .await
            .unwrap();
        let adapter = AccessCredentialAdapter::new(runtime, Arc::new(DeniedLiveAuthority));
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32]);
        let credential = ProductCredential::parse(&format!(
            "{PRODUCT_CREDENTIAL_PREFIX}missing-credential_{encoded}"
        ))
        .unwrap();

        let _lock = crate::test_support::TRACING_TEST_LOCK.lock().unwrap();
        let logs = crate::test_support::SharedBuf::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .without_time()
                .with_writer(logs.clone()),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _subscriber = tracing::dispatcher::set_default(&dispatch);
        crate::test_support::rebuild_tracing_interest_cache();

        assert_eq!(
            adapter.verify(&credential).await.err(),
            Some(ProductCredentialVerificationError::Denied)
        );
        let output = crate::test_support::captured_logs(&logs);
        assert!(output.contains("security event not recorded"), "{output}");
        assert!(output.contains("credential_denied"), "{output}");
        assert!(!output.contains("missing-credential"), "{output}");
        assert!(!output.contains(&encoded), "{output}");
    }

    fn current_row() -> CurrentBindingRow {
        let binding = stored();
        CurrentBindingRow {
            credential_digest: vec![7; 32],
            credential_installation_generation: 1,
            live_organization_policy_epoch: binding.organization_policy_epoch,
            live_project_policy_epoch: binding.project_policy_epoch,
            membership_status: "active".into(),
            project_status: "active".into(),
            organization_status: "active".into(),
            assigned_loadout_id: binding.loadout_id.clone(),
            live_membership_generation: binding.membership_epoch,
            live_loadout_assignment_generation: binding.assignment_generation,
            live_installation_generation: 1,
            binding,
        }
    }

    /// CQ-H2: each current-ness condition independently revokes currency.
    #[test]
    fn every_currentness_condition_is_checked_by_name() {
        let current = current_row();
        assert!(current.is_current(None));
        assert!(current.is_current(Some(&[7; 32])));
        assert!(!current.is_current(Some(&[8; 32])), "secret mismatch");
        let mut short_digest = current_row();
        short_digest.credential_digest = vec![7; 31];
        assert!(!short_digest.is_current(Some(&[7; 32])), "malformed digest");

        let mutations: Vec<(&str, Box<dyn Fn(&mut CurrentBindingRow)>)> = vec![
            (
                "organization policy epoch",
                Box::new(|row| row.live_organization_policy_epoch += 1),
            ),
            (
                "project policy epoch",
                Box::new(|row| row.live_project_policy_epoch += 1),
            ),
            (
                "membership status",
                Box::new(|row| row.membership_status = "revoked".into()),
            ),
            (
                "project status",
                Box::new(|row| row.project_status = "archived".into()),
            ),
            (
                "organization status",
                Box::new(|row| row.organization_status = "suspended".into()),
            ),
            (
                "assigned loadout",
                Box::new(|row| row.assigned_loadout_id.push('x')),
            ),
            (
                "membership generation",
                Box::new(|row| row.live_membership_generation += 1),
            ),
            (
                "loadout assignment",
                Box::new(|row| row.live_loadout_assignment_generation += 1),
            ),
            (
                "installation generation",
                Box::new(|row| row.live_installation_generation += 1),
            ),
        ];
        for (condition, mutate) in mutations {
            let mut row = current_row();
            mutate(&mut row);
            assert!(
                !row.is_current(None),
                "{condition} mismatch must not be current"
            );
        }
    }

    /// PERF-M2: the credential read restores the shared connection's busy
    /// timeout instead of leaving it at the shortened read value.
    #[tokio::test]
    async fn credential_read_restores_the_connection_busy_timeout() {
        let (_directory, runtime) = ready_runtime().await;
        let store = runtime.store().await.unwrap();
        let busy_timeout = |store: AccessStore| async move {
            store
                .with_connection(|connection| {
                    connection
                        .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                        .map_err(super::super::store::map_sqlite_error)
                })
                .await
                .unwrap()
        };
        store
            .with_connection(|connection| {
                connection
                    .busy_timeout(Duration::from_millis(4321))
                    .map_err(super::super::store::map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(
            read_current_binding(store.clone(), "missing-credential".into(), None)
                .await
                .err(),
            Some(ProductCredentialVerificationError::Denied)
        );
        assert_eq!(busy_timeout(store).await, 4321);
    }

    #[test]
    fn shared_credential_policy_helpers_are_exact() {
        let granted = vec!["lab".to_owned(), "lab:read".to_owned()];
        assert!(scopes_within(&["lab:read".into()], &granted));
        assert!(scopes_within(&[], &granted));
        assert!(!scopes_within(&["lab:admin".into()], &granted));
        assert_eq!(
            classify_security_admission::<()>(Ok(true), Ok(true)),
            SecurityAdmission::Admitted
        );
        assert_eq!(
            classify_security_admission::<()>(Ok(false), Ok(true)),
            SecurityAdmission::RateLimited
        );
        assert_eq!(
            classify_security_admission(Err::<bool, _>(()), Ok(true)),
            SecurityAdmission::StoreUnavailable
        );
        assert_eq!(
            classify_security_admission(Ok(true), Err::<bool, _>(())),
            SecurityAdmission::StoreUnavailable
        );
        assert_eq!(credential_generation_sql(9), Ok(9));
        assert_eq!(
            credential_generation_sql(u64::MAX),
            Err(CredentialGenerationOutOfRange)
        );
    }

    #[test]
    fn digest_is_over_the_complete_canonical_wire_credential() {
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32]);
        let wire = format!("{PRODUCT_CREDENTIAL_PREFIX}credential-id_{encoded}");
        let parsed = ProductCredential::parse(&wire).unwrap();
        assert_eq!(
            credential_digest(&parsed).as_slice(),
            Sha256::digest(wire.as_bytes()).as_slice()
        );

        let other = ProductCredential::parse(&format!(
            "{PRODUCT_CREDENTIAL_PREFIX}credential-id_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA4; 32])
        ))
        .unwrap();
        assert!(!bool::from(
            credential_digest(&parsed).ct_eq(&credential_digest(&other))
        ));
    }

    #[tokio::test]
    async fn invalid_raw_credentials_are_throttled_after_sixteen_attempts() {
        let (_directory, runtime) = ready_runtime().await;
        let live: Arc<dyn LiveAuthority> = Arc::new(DeniedLiveAuthority);
        let adapter = AccessCredentialAdapter::new(runtime.clone(), live);
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32]);
        let credential = ProductCredential::parse(&format!(
            "{PRODUCT_CREDENTIAL_PREFIX}missing-credential_{encoded}"
        ))
        .unwrap();

        for _ in 0..17 {
            assert!(matches!(
                adapter.verify(&credential).await,
                Err(ProductCredentialVerificationError::Denied)
            ));
        }

        let store = runtime.store().await.unwrap();
        let (attempts, rate_limited_events): (i64, i64) = store
            .with_connection(|connection| {
                Ok((
                    connection
                        .query_row(
                            "SELECT attempts FROM access_admission_buckets WHERE admission_class='credential_peer'",
                            [],
                            |row| row.get(0),
                        )
                        .map_err(super::super::store::map_sqlite_error)?,
                    connection
                        .query_row(
                            "SELECT count(*) FROM access_security_events WHERE event_kind='credential_verify' AND reason_code='rate_limited'",
                            [],
                            |row| row.get(0),
                        )
                        .map_err(super::super::store::map_sqlite_error)?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(attempts, 16);
        assert_eq!(rate_limited_events, 1);
    }

    /// PERF-C1: the admission check runs before every verification, so it
    /// must never consume the budget itself; only denials are charged.
    #[tokio::test]
    async fn admission_checks_never_consume_the_failure_budget() {
        let (_directory, runtime) = ready_runtime().await;
        let live: Arc<dyn LiveAuthority> = Arc::new(DeniedLiveAuthority);
        let adapter = AccessCredentialAdapter::new(runtime.clone(), live);

        for _ in 0..(CREDENTIAL_GLOBAL_FAILURE_LIMIT * 2) {
            adapter
                .admit_credential_attempt("busy-but-valid-credential")
                .await
                .unwrap();
        }

        let store = runtime.store().await.unwrap();
        let buckets: i64 = store
            .with_connection(|connection| {
                connection
                    .query_row("SELECT count(*) FROM access_admission_buckets", [], |row| {
                        row.get(0)
                    })
                    .map_err(super::super::store::map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(buckets, 0, "admission checks must be read-only");
    }

    #[tokio::test]
    async fn read_pool_has_fixed_connections_and_bounded_admission() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let path = directory.path().canonicalize().unwrap().join("access.db");
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&path)
                .unwrap();
        }
        let store = AccessStore::open(path.clone()).await.unwrap();
        store
            .bootstrap_owner(
                super::super::bootstrap::BootstrapOwnerInput::new(
                    labby_auth::VerifiedIdentity::local_credential(
                        labby_auth::Authenticator::StaticBearer,
                        "static-bearer:credential-pool-test",
                    )
                    .unwrap(),
                    "Local",
                    "Default",
                )
                .unwrap(),
            )
            .await
            .unwrap();
        drop(store);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let pool = CredentialReadPool::open(&path).await.unwrap();
        assert_eq!(pool.stores.len(), READ_CONNECTIONS);
        assert_eq!(pool.executions.available_permits(), READ_CONNECTIONS);
        assert_eq!(pool.outstanding.available_permits(), MAX_OUTSTANDING_READS);
        let mut permits = Vec::new();
        for _ in 0..MAX_OUTSTANDING_READS {
            permits.push(Arc::clone(&pool.outstanding).try_acquire_owned().unwrap());
        }
        assert_eq!(permits.len(), MAX_OUTSTANDING_READS);
        assert!(Arc::clone(&pool.outstanding).try_acquire_owned().is_err());
    }

    #[test]
    fn live_authority_comparison_is_exact_except_for_narrower_credential_scopes() {
        let binding = stored();
        let expected = live(&binding);
        assert!(live_matches(&expected, &binding));
        let mutations: Vec<Box<dyn Fn(&mut LiveAuthoritySnapshot)>> = vec![
            Box::new(|v| v.loadout_id.push('x')),
            Box::new(|v| v.loadout_generation += 1),
            Box::new(|v| v.assignment_generation += 1),
            Box::new(|v| v.catalog_generation += 1),
            Box::new(|v| v.route_id.push('x')),
            Box::new(|v| v.route_generation += 1),
            Box::new(|v| v.resource.push('x')),
            Box::new(|v| v.audience.push('x')),
        ];
        for mutate in mutations {
            let mut changed = expected.clone();
            mutate(&mut changed);
            assert!(!live_matches(&changed, &binding));
        }
        let mut authority_ceiling = expected.clone();
        authority_ceiling.scopes.push("lab:admin".into());
        assert!(live_matches(&authority_ceiling, &binding));
        authority_ceiling.scopes = vec!["lab:admin".into()];
        assert!(!live_matches(&authority_ceiling, &binding));
    }
}
