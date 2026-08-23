//! Service-registry seam for `GatewayManager`.
//!
//! The manager needs three things from Labby's `ToolRegistry`: the set of
//! registered service names, each service's actions (name/description/
//! destructive/admin requirement), and the `&'static PluginMeta` for a service. It also hands the
//! registry to the upstream pool for in-process peer discovery.
//!
//! Rather than depend on Labby's concrete `ToolRegistry` (which carries
//! `ActionSpec` dispatch function pointers and the default-registry builder),
//! the manager depends only on this trait. Labby implements it for `ToolRegistry`
//! and injects it. The trait is a supertrait of [`InProcessServiceRegistry`] so
//! the same value can be passed to the pool's discovery entry points.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use labby_primitives::plugin::PluginMeta;

use crate::registry::InProcessServiceRegistry;

/// A single action exposed by a registered service, projected to the data the
/// gateway dispatch surface needs (no `ActionSpec` dispatch pointers).
#[derive(Debug, Clone)]
pub struct ServiceActionInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub destructive: bool,
    pub requires_admin: bool,
}

static NEXT_SERVICE_REGISTRY_GENERATION: AtomicU64 = AtomicU64::new(1);
const MAX_PUBLISHED_SERVICES: usize = 256;
const MAX_PUBLISHED_ACTIONS: usize = 4096;

fn next_service_registry_generation() -> u64 {
    NEXT_SERVICE_REGISTRY_GENERATION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |generation| {
            generation.checked_add(1)
        })
        .expect("gateway service-registry generation exhausted")
}

/// Opaque process-local identity of one built-in service-registry publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceRegistryPublicationGeneration(u64);

/// One immutable action in a published built-in service catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedServiceAction {
    name: Arc<str>,
    description: Arc<str>,
    destructive: bool,
    requires_admin: bool,
}

impl PublishedServiceAction {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }
    #[must_use]
    pub fn destructive(&self) -> bool {
        self.destructive
    }
    #[must_use]
    pub fn requires_admin(&self) -> bool {
        self.requires_admin
    }
}

/// One immutable service and its exact published action metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedService {
    name: Arc<str>,
    actions: Arc<[PublishedServiceAction]>,
}

impl PublishedService {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[must_use]
    pub fn actions(&self) -> &[PublishedServiceAction] {
        &self.actions
    }
}

/// Redacted reason a registry could not be projected unambiguously.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceRegistryPublicationError {
    TooLarge,
    InvalidRegistry,
    DuplicateService,
    DuplicateAction,
}

impl std::fmt::Display for ServiceRegistryPublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("built-in service catalog is unavailable")
    }
}

impl std::error::Error for ServiceRegistryPublicationError {}

/// Immutable observational view of one service-registry publication.
///
/// This is not a dispatch grant and does not prove that an in-process peer is
/// currently routable through the published upstream pool.
pub struct PublishedServiceRegistrySnapshot {
    generation: ServiceRegistryPublicationGeneration,
    services: Arc<[PublishedService]>,
}

impl PublishedServiceRegistrySnapshot {
    #[must_use]
    pub fn generation(&self) -> ServiceRegistryPublicationGeneration {
        self.generation
    }
    #[must_use]
    pub fn services(&self) -> &[PublishedService] {
        &self.services
    }
}

pub(crate) struct PublishedServiceRegistryState {
    generation: ServiceRegistryPublicationGeneration,
    registry: Arc<dyn GatewayServiceRegistry>,
    catalog: Result<Arc<[PublishedService]>, ServiceRegistryPublicationError>,
}

impl PublishedServiceRegistryState {
    pub(crate) fn new(registry: Arc<dyn GatewayServiceRegistry>) -> Self {
        let catalog = project_catalog(registry.as_ref());
        Self {
            generation: ServiceRegistryPublicationGeneration(next_service_registry_generation()),
            registry,
            catalog,
        }
    }

    pub(crate) fn registry(&self) -> Arc<dyn GatewayServiceRegistry> {
        Arc::clone(&self.registry)
    }

    pub(crate) fn snapshot(
        &self,
    ) -> Result<PublishedServiceRegistrySnapshot, ServiceRegistryPublicationError> {
        Ok(PublishedServiceRegistrySnapshot {
            generation: self.generation,
            services: Arc::clone(self.catalog.as_ref().map_err(|error| *error)?),
        })
    }
}

fn project_catalog(
    registry: &dyn GatewayServiceRegistry,
) -> Result<Arc<[PublishedService]>, ServiceRegistryPublicationError> {
    let mut names = registry.service_names();
    if names.len() > MAX_PUBLISHED_SERVICES {
        return Err(ServiceRegistryPublicationError::TooLarge);
    }
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ServiceRegistryPublicationError::DuplicateService);
    }

    let mut total_actions = 0usize;
    let mut services = Vec::with_capacity(names.len());
    for name in names {
        if !registry.contains_service(name) {
            return Err(ServiceRegistryPublicationError::InvalidRegistry);
        }
        let mut actions = registry
            .service_actions(name)
            .ok_or(ServiceRegistryPublicationError::InvalidRegistry)?;
        total_actions = total_actions
            .checked_add(actions.len())
            .ok_or(ServiceRegistryPublicationError::TooLarge)?;
        if total_actions > MAX_PUBLISHED_ACTIONS {
            return Err(ServiceRegistryPublicationError::TooLarge);
        }
        actions.sort_unstable_by_key(|action| action.name);
        if actions.windows(2).any(|pair| pair[0].name == pair[1].name) {
            return Err(ServiceRegistryPublicationError::DuplicateAction);
        }
        let actions = actions
            .into_iter()
            .map(|action| PublishedServiceAction {
                name: Arc::from(action.name),
                description: Arc::from(action.description),
                destructive: action.destructive,
                requires_admin: action.requires_admin,
            })
            .collect::<Vec<_>>();
        services.push(PublishedService {
            name: Arc::from(name),
            actions: Arc::from(actions),
        });
    }
    Ok(Arc::from(services))
}

/// Read-only view of Labby's service registry the gateway manager depends on.
///
/// `InProcessServiceRegistry` is a supertrait so the same trait object can be
/// passed to `UpstreamPool::discover_all_*_with_in_process_peers`.
///
/// # Contract
///
/// Implementations must remain immutable after being handed to a manager.
/// Publication generations cover replacement, not hidden interior mutation.
pub trait GatewayServiceRegistry: InProcessServiceRegistry {
    /// Stable names of every registered service.
    fn service_names(&self) -> Vec<&'static str>;

    /// Whether a service with this name is registered.
    fn contains_service(&self, name: &str) -> bool;

    /// Actions exposed by a registered service, or `None` if not registered.
    fn service_actions(&self, name: &str) -> Option<Vec<ServiceActionInfo>>;

    /// `PluginMeta` for a registered service, or `None` if it has no metadata.
    fn service_meta(&self, name: &str) -> Option<&'static PluginMeta>;
}

/// An empty service registry: no registered services, no in-process peers.
///
/// Used as the default before a real registry is injected (e.g. a freshly
/// constructed manager in a test that does not exercise service lookups).
#[derive(Debug, Default, Clone, Copy)]
pub struct EmptyServiceRegistry;

impl InProcessServiceRegistry for EmptyServiceRegistry {
    fn in_process_services(&self) -> Vec<Box<dyn crate::registry::InProcessService>> {
        Vec::new()
    }
}

impl GatewayServiceRegistry for EmptyServiceRegistry {
    fn service_names(&self) -> Vec<&'static str> {
        Vec::new()
    }

    fn contains_service(&self, _name: &str) -> bool {
        false
    }

    fn service_actions(&self, _name: &str) -> Option<Vec<ServiceActionInfo>> {
        None
    }

    fn service_meta(&self, _name: &str) -> Option<&'static PluginMeta> {
        None
    }
}
