use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

pub const MAX_RESOURCE_LEASE_TTL: Duration = Duration::from_hours(24);
pub const MAX_RESOURCE_LEASE_OWNER_LEN: usize = 128;
const LEASE_ID_BYTES: usize = 32;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ResourceLease {
    pub id: String,
    pub resource: String,
    pub scopes: Vec<String>,
    pub expires_at_unix: u64,
}

impl std::fmt::Debug for ResourceLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResourceLease")
            .field("id", &"[REDACTED]")
            .field("resource", &self.resource)
            .field("scopes", &self.scopes)
            .field("expires_at_unix", &self.expires_at_unix)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ResourceLeaseDiagnostic {
    pub resource: String,
    pub scopes: Vec<String>,
    pub owner: String,
    pub expires_at_unix: u64,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ResourceRegistryError {
    #[error("resource must be an absolute HTTPS URL without credentials, query, or fragment")]
    InvalidResource,
    #[error("at least one valid OAuth scope is required")]
    InvalidScopes,
    #[error("resource lease TTL must be between 1 and 86400 seconds")]
    InvalidTtl,
    #[error("resource lease owner must be between 1 and 128 bytes")]
    InvalidOwner,
    #[error("resource lease was not found or has expired")]
    LeaseNotFound,
    #[error("failed to generate a resource lease identifier")]
    RandomnessUnavailable,
    #[error("system clock is before the Unix epoch")]
    InvalidClock,
}

#[derive(Clone)]
pub struct ResourceRegistry {
    inner: Arc<RwLock<ResourceRegistryInner>>,
}

#[derive(Default)]
struct ResourceRegistryInner {
    configured: BTreeMap<String, BTreeSet<String>>,
    leases: BTreeMap<String, StoredLease>,
}

#[derive(Clone)]
struct StoredLease {
    resource: String,
    scopes: BTreeSet<String>,
    owner: String,
    expires_at_unix: u64,
}

impl ResourceRegistry {
    fn write_inner(&self) -> std::sync::RwLockWriteGuard<'_, ResourceRegistryInner> {
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(ResourceRegistryInner::default())),
        }
    }

    pub fn replace_configured_resource_scopes(
        &self,
        resources: impl IntoIterator<Item = (String, Vec<String>)>,
    ) -> Result<(), ResourceRegistryError> {
        let mut configured = BTreeMap::new();
        for (resource, scopes) in resources {
            configured.insert(canonical_resource(&resource)?, validated_scopes(scopes)?);
        }
        self.write_inner().configured = configured;
        Ok(())
    }

    pub fn create_resource_lease(
        &self,
        resource: &str,
        scopes: impl IntoIterator<Item = impl AsRef<str>>,
        ttl: Duration,
        owner: &str,
    ) -> Result<ResourceLease, ResourceRegistryError> {
        self.create_resource_lease_at(resource, scopes, ttl, owner, SystemTime::now())
    }

    pub fn create_resource_lease_at(
        &self,
        resource: &str,
        scopes: impl IntoIterator<Item = impl AsRef<str>>,
        ttl: Duration,
        owner: &str,
        now: SystemTime,
    ) -> Result<ResourceLease, ResourceRegistryError> {
        validate_ttl(ttl)?;
        let resource = canonical_resource(resource)?;
        let scopes = validated_scopes(scopes)?;
        let owner = owner.trim();
        if owner.is_empty() || owner.len() > MAX_RESOURCE_LEASE_OWNER_LEN {
            return Err(ResourceRegistryError::InvalidOwner);
        }
        let now_unix = unix_seconds(now)?;
        let expires_at_unix = now_unix
            .checked_add(ttl.as_secs())
            .ok_or(ResourceRegistryError::InvalidTtl)?;
        let id = random_lease_id()?;
        let stored = StoredLease {
            resource: resource.clone(),
            scopes: scopes.clone(),
            owner: owner.to_string(),
            expires_at_unix,
        };
        let mut inner = self.write_inner();
        prune_locked(&mut inner, now_unix);
        inner.leases.insert(id.clone(), stored);
        Ok(ResourceLease {
            id,
            resource,
            scopes: scopes.into_iter().collect(),
            expires_at_unix,
        })
    }

    pub fn renew_resource_lease(
        &self,
        id: &str,
        ttl: Duration,
    ) -> Result<ResourceLease, ResourceRegistryError> {
        self.renew_resource_lease_at(id, ttl, SystemTime::now())
    }

    pub fn renew_resource_lease_at(
        &self,
        id: &str,
        ttl: Duration,
        now: SystemTime,
    ) -> Result<ResourceLease, ResourceRegistryError> {
        validate_ttl(ttl)?;
        let now_unix = unix_seconds(now)?;
        let expires_at_unix = now_unix
            .checked_add(ttl.as_secs())
            .ok_or(ResourceRegistryError::InvalidTtl)?;
        let mut inner = self.write_inner();
        prune_locked(&mut inner, now_unix);
        let lease = inner
            .leases
            .get_mut(id)
            .ok_or(ResourceRegistryError::LeaseNotFound)?;
        lease.expires_at_unix = expires_at_unix;
        Ok(public_lease(id, lease))
    }

    pub fn release_resource_lease(&self, id: &str) -> Result<(), ResourceRegistryError> {
        let now = unix_seconds(SystemTime::now())?;
        let mut inner = self.write_inner();
        prune_locked(&mut inner, now);
        inner
            .leases
            .remove(id)
            .map(|_| ())
            .ok_or(ResourceRegistryError::LeaseNotFound)
    }

    pub fn prune_expired_resource_leases(&self, now: SystemTime) -> usize {
        let Ok(now_unix) = unix_seconds(now) else {
            return 0;
        };
        let mut inner = self.write_inner();
        let before = inner.leases.len();
        prune_locked(&mut inner, now_unix);
        before - inner.leases.len()
    }

    #[must_use]
    pub fn effective_resource_scopes(&self, resource: &str) -> Option<Vec<String>> {
        self.effective_resource_scopes_at(resource, SystemTime::now())
    }

    #[must_use]
    pub fn effective_resource_scopes_at(
        &self,
        resource: &str,
        now: SystemTime,
    ) -> Option<Vec<String>> {
        let resource = canonical_resource(resource).ok()?;
        let now_unix = unix_seconds(now).ok()?;
        let mut inner = self.write_inner();
        prune_locked(&mut inner, now_unix);
        let mut scopes = inner.configured.get(&resource).cloned().unwrap_or_default();
        for lease in inner
            .leases
            .values()
            .filter(|lease| lease.resource == resource)
        {
            scopes.extend(lease.scopes.iter().cloned());
        }
        (!scopes.is_empty()).then(|| scopes.into_iter().collect())
    }

    #[must_use]
    pub fn lease_count(&self) -> usize {
        let Ok(now) = unix_seconds(SystemTime::now()) else {
            return 0;
        };
        let mut inner = self.write_inner();
        prune_locked(&mut inner, now);
        inner.leases.len()
    }

    #[must_use]
    pub fn lease_diagnostics(&self) -> Vec<ResourceLeaseDiagnostic> {
        let Ok(now) = unix_seconds(SystemTime::now()) else {
            return Vec::new();
        };
        let mut inner = self.write_inner();
        prune_locked(&mut inner, now);
        inner
            .leases
            .values()
            .map(|lease| ResourceLeaseDiagnostic {
                resource: lease.resource.clone(),
                scopes: lease.scopes.iter().cloned().collect(),
                owner: lease.owner.clone(),
                expires_at_unix: lease.expires_at_unix,
            })
            .collect()
    }
}

impl Default for ResourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn public_lease(id: &str, lease: &StoredLease) -> ResourceLease {
    ResourceLease {
        id: id.to_string(),
        resource: lease.resource.clone(),
        scopes: lease.scopes.iter().cloned().collect(),
        expires_at_unix: lease.expires_at_unix,
    }
}

fn canonical_resource(resource: &str) -> Result<String, ResourceRegistryError> {
    let trimmed = resource.trim();
    let canonical = trimmed.strip_suffix('/').unwrap_or(trimmed);
    let parsed = Url::parse(canonical).map_err(|_| ResourceRegistryError::InvalidResource)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(ResourceRegistryError::InvalidResource);
    }
    Ok(canonical.to_string())
}

fn validated_scopes(
    scopes: impl IntoIterator<Item = impl AsRef<str>>,
) -> Result<BTreeSet<String>, ResourceRegistryError> {
    let scopes = scopes
        .into_iter()
        .map(|scope| scope.as_ref().trim().to_string())
        .collect::<BTreeSet<_>>();
    if scopes.is_empty() || scopes.iter().any(|scope| !valid_scope(scope)) {
        return Err(ResourceRegistryError::InvalidScopes);
    }
    Ok(scopes)
}

fn valid_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.bytes().all(|byte| {
            byte == 0x21 || (0x23..=0x5b).contains(&byte) || (0x5d..=0x7e).contains(&byte)
        })
}

fn validate_ttl(ttl: Duration) -> Result<(), ResourceRegistryError> {
    if ttl.is_zero() || ttl > MAX_RESOURCE_LEASE_TTL || ttl.subsec_nanos() != 0 {
        return Err(ResourceRegistryError::InvalidTtl);
    }
    Ok(())
}

fn unix_seconds(now: SystemTime) -> Result<u64, ResourceRegistryError> {
    now.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ResourceRegistryError::InvalidClock)
}

fn prune_locked(inner: &mut ResourceRegistryInner, now_unix: u64) {
    inner
        .leases
        .retain(|_, lease| lease.expires_at_unix > now_unix);
}

fn random_lease_id() -> Result<String, ResourceRegistryError> {
    let mut bytes = [0_u8; LEASE_ID_BYTES];
    getrandom::fill(&mut bytes).map_err(|_| ResourceRegistryError::RandomnessUnavailable)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(test)]
#[allow(clippy::panic)]
mod poison_tests {
    use super::ResourceRegistry;

    #[test]
    fn poisoned_registry_lock_is_recovered_without_panicking() {
        let registry = ResourceRegistry::new();
        let shared = registry.inner.clone();
        drop(std::panic::catch_unwind(move || {
            let _guard = shared.write().expect("initial registry write lock");
            panic!("poison registry lock");
        }));

        registry
            .replace_configured_resource_scopes([(
                "https://example.com/mcp".to_string(),
                vec!["lab:read".to_string()],
            )])
            .expect("poisoned registry must recover");
        assert_eq!(
            registry.effective_resource_scopes("https://example.com/mcp"),
            Some(vec!["lab:read".to_string()])
        );
    }
}
