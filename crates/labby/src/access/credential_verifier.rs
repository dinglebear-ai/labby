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
const QUEUE_DEADLINE: Duration = Duration::from_millis(100);

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
}

impl AccessCredentialAdapter {
    pub(crate) fn new(runtime: AccessRuntime, live: Arc<dyn LiveAuthority>) -> Self {
        Self { runtime, live }
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
        let global: [u8; 32] = Sha256::digest(b"labby-credential-global-v1").into();
        let target: [u8; 32] = Sha256::digest(credential_id.as_bytes()).into();
        let global_admitted = self
            .runtime
            .admit_security_operation("credential_global".into(), global, now, 60, 64)
            .await
            .map_err(|_| ProductCredentialVerificationError::Unavailable)?;
        let target_admitted = self
            .runtime
            .admit_security_operation("credential_peer".into(), target, now, 60, 16)
            .await
            .map_err(|_| ProductCredentialVerificationError::Unavailable)?;
        if !global_admitted || !target_admitted {
            let _ = self
                .runtime
                .record_security_event(
                    "credential_verify".into(),
                    "deny".into(),
                    "rate_limited".into(),
                    target,
                    None,
                    now,
                )
                .await;
            return Err(ProductCredentialVerificationError::Denied);
        }
        let pool = self.runtime.credential_reads().await.map_err(|_| {
            tracing::warn!(phase = "runtime_pool", "credential resolution unavailable");
            ProductCredentialVerificationError::Unavailable
        })?;
        let stored = match pool.read(credential_id, digest).await {
            Ok(stored) => stored,
            Err(error) => {
                tracing::warn!(phase = "stored_binding", "credential resolution failed");
                let _ = self
                    .runtime
                    .record_security_event(
                        "credential_verify".into(),
                        "deny".into(),
                        "credential_denied".into(),
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
                let _ = self
                    .runtime
                    .record_security_event(
                        "credential_verify".into(),
                        "deny".into(),
                        reason.into(),
                        target,
                        None,
                        now,
                    )
                    .await;
                return Err(mapped);
            }
        };
        if !live_matches(&live, &stored) {
            let _ = self
                .runtime
                .record_security_event(
                    "credential_verify".into(),
                    "deny".into(),
                    "binding_mismatch".into(),
                    target,
                    None,
                    now,
                )
                .await;
            return Err(ProductCredentialVerificationError::Denied);
        }
        let _ = self
            .runtime
            .record_security_event(
                "credential_verify".into(),
                "allow".into(),
                "verified".into(),
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
        && live.scopes == stored.scopes
}

impl ProductCredentialVerifier for AccessCredentialAdapter {
    fn verify<'a>(
        &'a self,
        credential: &'a ProductCredential,
    ) -> ProductCredentialVerificationFuture<'a> {
        Box::pin(async move {
            let digest = credential_digest(credential);
            let bound = self
                .resolve_binding(credential.credential_id().to_owned(), Some(digest))
                .await?;
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
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ProductCredentialVerificationError::Unavailable)?
        .as_secs();
    store
        .with_connection(move |connection| {
            connection
                .busy_timeout(Duration::from_millis(250))
                .map_err(super::store::map_sqlite_error)?;
            let row = connection.query_row(
                "SELECT c.credential_digest,c.installation_id,c.canonical_issuer,c.subject,
                        c.principal_id,c.organization_id,c.project_id,c.loadout_id,
                        c.loadout_generation,c.loadout_assignment_generation,c.catalog_generation,
                        c.loadout_policy_fingerprint,
                        c.route_id,c.route_generation,c.membership_generation,
                        c.organization_policy_epoch,c.project_policy_epoch,c.credential_generation,
                        c.scopes_json,c.resource,c.audience,c.expires_at,
                        o.policy_epoch,p.project_policy_epoch,
                        m.status,p.status,o.status,pl.loadout_name,m.updated_at,pl.updated_at,
                        c.installation_generation,i.installation_generation
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
                rusqlite::params![credential_id, i64::try_from(now).unwrap_or(i64::MAX)],
                |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, StoredBinding {
                        installation_id: row.get(1)?, issuer: row.get(2)?, subject: row.get(3)?,
                        principal_id: row.get(4)?, organization_id: row.get(5)?, project_id: row.get(6)?,
                        loadout_id: row.get(7)?, loadout_generation: to_u64(row.get(8)?)?,
                        assignment_generation: to_u64(row.get(9)?)?, catalog_generation: to_u64(row.get(10)?)?,
                        policy_fingerprint: row.get::<_, Vec<u8>>(11)?.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?,
                        route_id: row.get(12)?, route_generation: to_u64(row.get(13)?)?,
                        membership_epoch: to_u64(row.get(14)?)?, organization_policy_epoch: to_u64(row.get(15)?)?,
                        project_policy_epoch: to_u64(row.get(16)?)?, credential_id: credential_id.clone(),
                        credential_generation: to_u64(row.get(17)?)?,
                        scopes: serde_json::from_str(&row.get::<_, String>(18)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                        resource: row.get(19)?, audience: row.get(20)?, expires_at: to_u64(row.get(21)?)?,
                    }, row.get::<_, i64>(22)?, row.get::<_, i64>(23)?, row.get::<_, String>(24)?,
                       row.get::<_, String>(25)?, row.get::<_, String>(26)?, row.get::<_, String>(27)?,
                       row.get::<_, i64>(28)?, row.get::<_, i64>(29)?, row.get::<_, i64>(30)?,
                       row.get::<_, i64>(31)?))
                },
            ).optional().map_err(super::store::map_sqlite_error)?
                .ok_or(AccessStoreError::NotAuthorized)?;
            if expected_digest.as_ref().is_some_and(|expected| {
                row.0.len() != 32 || !bool::from(row.0.as_slice().ct_eq(expected.as_slice()))
            }) || row.2 != i64::try_from(row.1.organization_policy_epoch).unwrap_or(-1)
                || row.3 != i64::try_from(row.1.project_policy_epoch).unwrap_or(-1)
                || row.4 != "active" || row.5 != "active" || row.6 != "active"
                || row.7 != row.1.loadout_id
                || row.8 != i64::try_from(row.1.membership_epoch).unwrap_or(-1)
                || row.9 != i64::try_from(row.1.assignment_generation).unwrap_or(-1)
                || row.10 != row.11
            {
                return Err(AccessStoreError::NotAuthorized);
            }
            Ok(row.1)
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

fn to_u64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, value))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            policy_fingerprint: [42; 32],
        }
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
    fn live_authority_comparison_is_exact_for_every_bound_dimension() {
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
            Box::new(|v| v.scopes.push("lab:admin".into())),
        ];
        for mutate in mutations {
            let mut changed = expected.clone();
            mutate(&mut changed);
            assert!(!live_matches(&changed, &binding));
        }
    }
}
