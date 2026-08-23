//! Coherent, generation-bearing views of the published gateway runtime.

use std::future::{Future, ready};
use std::sync::atomic::{AtomicU64, Ordering};

use labby_runtime::gateway_config::{GatewayLoadoutConfig, VirtualServerConfig};

use crate::gateway::runtime::{PoolPublicationGeneration, PublishedPoolSnapshot};
use crate::upstream::pool::{
    PublishedToolRoute, ToolCatalogGeneration, ToolCatalogPublicationError,
};

use super::GatewayManager;
use crate::gateway::service_registry::{
    PublishedService, ServiceRegistryPublicationError, ServiceRegistryPublicationGeneration,
};

static NEXT_RUNTIME_CONFIG_GENERATION: AtomicU64 = AtomicU64::new(1);

pub(super) fn next_runtime_config_generation() -> u64 {
    NEXT_RUNTIME_CONFIG_GENERATION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |generation| {
            generation.checked_add(1)
        })
        .expect("gateway runtime-config generation exhausted")
}

/// Opaque process-local identity of a published gateway runtime configuration
/// revision.
///
/// Callers may compare generations for equality. The numeric representation is
/// deliberately private: it is not durable state and must not be synthesized
/// from config content. In particular, an A -> B -> A publication produces
/// three distinct generations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GatewayRuntimeConfigGeneration(u64);

/// A Loadout resolved only from the runtime configuration published by this
/// process, paired with the exact publication generation that supplied it.
///
/// This never reads the durable desired configuration, where restart-bound
/// Loadout edits may be staged but not active.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedRuntimeLoadoutSnapshot {
    generation: GatewayRuntimeConfigGeneration,
    loadout: Option<GatewayLoadoutConfig>,
}

/// A fail-closed reason that a coherent runtime Loadout tool projection could
/// not be observed.
///
/// These variants deliberately carry no configured names, upstream errors, or
/// catalog contents so callers cannot accidentally disclose authority state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadoutToolCatalogPublicationError {
    MissingLoadout,
    MissingPool,
    CatalogUnavailable,
    Unstable,
}

/// A fail-closed reason that a coherent Loadout built-in service projection
/// could not be observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadoutServiceCatalogPublicationError {
    MissingLoadout,
    CatalogUnavailable,
    Unstable,
}

impl std::fmt::Display for LoadoutServiceCatalogPublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MissingLoadout => "runtime Loadout is unavailable",
            Self::CatalogUnavailable => "built-in service catalog is unavailable",
            Self::Unstable => "runtime service catalog changed during observation",
        })
    }
}

impl std::error::Error for LoadoutServiceCatalogPublicationError {}

/// Immutable built-in service/action projection for one running Loadout.
///
/// This mirrors current MCP discovery visibility, including virtual-server
/// enablement, MCP surface policy, and non-empty action allowlists. It remains
/// observational: it is not a dispatch grant or proof of peer routability.
pub struct PublishedLoadoutServiceCatalogSnapshot {
    runtime_config_generation: GatewayRuntimeConfigGeneration,
    service_registry_generation: ServiceRegistryPublicationGeneration,
    services: std::sync::Arc<[PublishedLoadoutService]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedLoadoutService {
    service: PublishedService,
}

impl PublishedLoadoutService {
    #[must_use]
    pub fn name(&self) -> &str {
        self.service.name()
    }
    #[must_use]
    pub fn actions(&self) -> &[crate::gateway::service_registry::PublishedServiceAction] {
        self.service.actions()
    }
    /// Generic MCP `help` and `schema` remain allowed even when absent from the
    /// registry-backed action metadata returned by [`Self::actions`].
    #[must_use]
    pub fn allows_implicit_help_and_schema(&self) -> bool {
        true
    }
}

impl PublishedLoadoutServiceCatalogSnapshot {
    #[must_use]
    pub fn runtime_config_generation(&self) -> GatewayRuntimeConfigGeneration {
        self.runtime_config_generation
    }

    #[must_use]
    pub fn service_registry_generation(&self) -> ServiceRegistryPublicationGeneration {
        self.service_registry_generation
    }

    #[must_use]
    pub fn services(&self) -> &[PublishedLoadoutService] {
        &self.services
    }
}

impl std::fmt::Display for LoadoutToolCatalogPublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MissingLoadout => "runtime Loadout is unavailable",
            Self::MissingPool => "runtime upstream pool is unavailable",
            Self::CatalogUnavailable => "runtime tool catalog is unavailable",
            Self::Unstable => "runtime tool catalog changed during observation",
        })
    }
}

impl std::error::Error for LoadoutToolCatalogPublicationError {}

/// An immutable tools-only projection composed from the running Loadout, the
/// exact published pool, and that pool's routable tool catalog.
///
/// This is observation, not an execution grant. It excludes services, OAuth
/// subject catalogs, resources, prompts, skills, Code Mode, protected routes,
/// and every transport/enforcement decision.
pub struct PublishedLoadoutToolCatalogSnapshot {
    runtime_config_generation: GatewayRuntimeConfigGeneration,
    pool_publication_generation: PoolPublicationGeneration,
    tool_catalog_generation: ToolCatalogGeneration,
    routes: std::sync::Arc<[PublishedToolRoute]>,
}

impl PublishedLoadoutToolCatalogSnapshot {
    #[must_use]
    pub fn runtime_config_generation(&self) -> GatewayRuntimeConfigGeneration {
        self.runtime_config_generation
    }

    #[must_use]
    pub fn pool_publication_generation(&self) -> PoolPublicationGeneration {
        self.pool_publication_generation
    }

    #[must_use]
    pub fn tool_catalog_generation(&self) -> ToolCatalogGeneration {
        self.tool_catalog_generation
    }

    #[must_use]
    pub fn routes(&self) -> &[PublishedToolRoute] {
        &self.routes
    }
}

struct ManagerPublicationObservation {
    runtime_generation: GatewayRuntimeConfigGeneration,
    loadout: Option<GatewayLoadoutConfig>,
    pool_snapshot: PublishedPoolSnapshot,
}

impl ManagerPublicationObservation {
    fn same_publication(&self, other: &Self) -> bool {
        self.runtime_generation == other.runtime_generation
            && self.loadout == other.loadout
            && self.pool_snapshot.generation() == other.pool_snapshot.generation()
            && match (self.pool_snapshot.pool(), other.pool_snapshot.pool()) {
                (Some(left), Some(right)) => std::sync::Arc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            }
    }
}

impl PublishedRuntimeLoadoutSnapshot {
    #[must_use]
    pub fn generation(&self) -> GatewayRuntimeConfigGeneration {
        self.generation
    }

    #[must_use]
    pub fn loadout(&self) -> Option<&GatewayLoadoutConfig> {
        self.loadout.as_ref()
    }

    #[must_use]
    pub fn into_loadout(self) -> Option<GatewayLoadoutConfig> {
        self.loadout
    }
}

impl GatewayManager {
    async fn manager_publication_observation(&self, name: &str) -> ManagerPublicationObservation {
        let _publication = self.publication_barrier.read().await;
        let config = self.config.read().await;
        let loadout = config
            .loadouts
            .iter()
            .find(|loadout| loadout.name == name)
            .cloned();
        ManagerPublicationObservation {
            runtime_generation: GatewayRuntimeConfigGeneration(
                self.runtime_config_generation.load(Ordering::Relaxed),
            ),
            loadout,
            pool_snapshot: self.runtime.published_pool_snapshot(),
        }
    }

    async fn service_publication_observation(
        &self,
        name: &str,
    ) -> ServiceManagerPublicationObservation {
        let _publication = self.publication_barrier.read().await;
        let config = self.config.read().await;
        ServiceManagerPublicationObservation {
            runtime_generation: GatewayRuntimeConfigGeneration(
                self.runtime_config_generation.load(Ordering::Relaxed),
            ),
            loadout: config
                .loadouts
                .iter()
                .find(|loadout| loadout.name == name)
                .cloned(),
            virtual_servers: config.virtual_servers.clone(),
        }
    }

    /// Compose the running named Loadout with the exact published built-in
    /// service registry. Three bounded G-S-G-S attempts reject config/registry
    /// churn, including ABA replacement publications.
    pub async fn published_loadout_service_catalog_snapshot(
        &self,
        name: &str,
    ) -> Result<PublishedLoadoutServiceCatalogSnapshot, LoadoutServiceCatalogPublicationError> {
        self.compose_loadout_service_catalog(name, |_: usize| ready(()))
            .await
    }

    pub(super) async fn compose_loadout_service_catalog<F, Fut>(
        &self,
        name: &str,
        mut after_first_catalog: F,
    ) -> Result<PublishedLoadoutServiceCatalogSnapshot, LoadoutServiceCatalogPublicationError>
    where
        F: FnMut(usize) -> Fut,
        Fut: Future<Output = ()>,
    {
        const MAX_ATTEMPTS: usize = 3;

        for attempt in 0..MAX_ATTEMPTS {
            let first_gateway = self.service_publication_observation(name).await;
            let first_catalog = self.published_service_registry_snapshot();
            after_first_catalog(attempt).await;
            let second_gateway = self.service_publication_observation(name).await;
            if !first_gateway.same_publication(&second_gateway) {
                continue;
            }
            let second_catalog = self.published_service_registry_snapshot();
            let (first_catalog, second_catalog) = match (first_catalog, second_catalog) {
                (Ok(first), Ok(second)) => (first, second),
                (Err(first), Err(second)) if first == second => {
                    return Err(map_service_catalog_error(first));
                }
                _ => continue,
            };
            if first_catalog.generation() != second_catalog.generation() {
                continue;
            }
            let loadout = first_gateway
                .loadout
                .as_ref()
                .ok_or(LoadoutServiceCatalogPublicationError::MissingLoadout)?;
            let selected = if loadout.expose_tools {
                let by_name = first_catalog
                    .services()
                    .iter()
                    .map(|service| (service.name(), service))
                    .collect::<std::collections::BTreeMap<_, _>>();
                let mut selected = std::collections::BTreeMap::new();
                for member in &loadout.services {
                    let Some(server) = first_gateway
                        .virtual_servers
                        .iter()
                        .find(|server| server.service == *member || server.id == *member)
                    else {
                        continue;
                    };
                    let Some(service) = by_name.get(server.service.as_str()) else {
                        continue;
                    };
                    let projected = match super::views::mcp_service_policy_for_config(
                        &first_gateway.virtual_servers,
                        member,
                    ) {
                        super::views::McpServicePolicy::Absent
                        | super::views::McpServicePolicy::Hidden => continue,
                        super::views::McpServicePolicy::Unrestricted => (*service).clone(),
                        super::views::McpServicePolicy::Allowlisted(actions) => {
                            let allowed = actions
                                .iter()
                                .map(String::as_str)
                                .collect::<std::collections::BTreeSet<_>>();
                            PublishedService::from_filtered_actions(service, |action| {
                                matches!(action, "help" | "schema") || allowed.contains(action)
                            })
                        }
                    };
                    if selected
                        .insert(
                            projected.name().to_string(),
                            PublishedLoadoutService { service: projected },
                        )
                        .is_some()
                    {
                        return Err(LoadoutServiceCatalogPublicationError::CatalogUnavailable);
                    }
                }
                selected.into_values().collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            return Ok(PublishedLoadoutServiceCatalogSnapshot {
                runtime_config_generation: first_gateway.runtime_generation,
                service_registry_generation: first_catalog.generation(),
                services: std::sync::Arc::from(selected),
            });
        }
        Err(LoadoutServiceCatalogPublicationError::Unstable)
    }

    /// Compose the running named Loadout with the exact published pool's
    /// routable tools. Successful snapshots use three bounded G-C-G-C attempts
    /// to reject torn observations and fail closed under sustained config,
    /// pool, or catalog churn. Catalog publication failures are returned as a
    /// stable redacted error because failed catalogs do not expose generations.
    pub async fn published_loadout_tool_catalog_snapshot(
        &self,
        name: &str,
    ) -> Result<PublishedLoadoutToolCatalogSnapshot, LoadoutToolCatalogPublicationError> {
        self.compose_loadout_tool_catalog(name, |_: usize| ready(()))
            .await
    }

    pub(super) async fn compose_loadout_tool_catalog<F, Fut>(
        &self,
        name: &str,
        mut after_first_catalog: F,
    ) -> Result<PublishedLoadoutToolCatalogSnapshot, LoadoutToolCatalogPublicationError>
    where
        F: FnMut(usize) -> Fut,
        Fut: Future<Output = ()>,
    {
        const MAX_ATTEMPTS: usize = 3;

        for attempt in 0..MAX_ATTEMPTS {
            let first_gateway = self.manager_publication_observation(name).await;
            let first_catalog = match first_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_tool_catalog().await),
                None => None,
            };
            after_first_catalog(attempt).await;

            let second_gateway = self.manager_publication_observation(name).await;
            if !first_gateway.same_publication(&second_gateway) {
                continue;
            }
            let second_catalog = match second_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_tool_catalog().await),
                None => None,
            };

            let loadout = match first_gateway.loadout.as_ref() {
                Some(loadout) => loadout,
                None => return Err(LoadoutToolCatalogPublicationError::MissingLoadout),
            };
            let (first_catalog, second_catalog) = match (first_catalog, second_catalog) {
                (None, None) => return Err(LoadoutToolCatalogPublicationError::MissingPool),
                (Some(Err(first)), Some(Err(second))) if first == second => {
                    return Err(map_catalog_error(first));
                }
                (Some(Ok(first)), Some(Ok(second))) => (first, second),
                _ => continue,
            };
            if first_catalog.generation() != second_catalog.generation() {
                continue;
            }

            let selected = if loadout.expose_tools {
                let upstreams = loadout
                    .upstreams
                    .iter()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>();
                first_catalog
                    .routes()
                    .iter()
                    .filter(|route| upstreams.contains(route.upstream_name.as_ref()))
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };

            return Ok(PublishedLoadoutToolCatalogSnapshot {
                runtime_config_generation: first_gateway.runtime_generation,
                pool_publication_generation: first_gateway.pool_snapshot.generation(),
                tool_catalog_generation: first_catalog.generation(),
                routes: std::sync::Arc::from(selected),
            });
        }
        Err(LoadoutToolCatalogPublicationError::Unstable)
    }

    /// Resolve a named Loadout from one coherent published runtime revision.
    pub async fn published_runtime_loadout_snapshot(
        &self,
        name: &str,
    ) -> PublishedRuntimeLoadoutSnapshot {
        let _publication = self.publication_barrier.read().await;
        let loadout = self
            .config
            .read()
            .await
            .loadouts
            .iter()
            .find(|loadout| loadout.name == name)
            .cloned();
        let generation =
            GatewayRuntimeConfigGeneration(self.runtime_config_generation.load(Ordering::Relaxed));
        PublishedRuntimeLoadoutSnapshot {
            generation,
            loadout,
        }
    }

    /// Advance the runtime-config publication identity while the caller holds
    /// the publication writer. Pool/catalog-only mutations are deliberately
    /// outside this Loadout snapshot contract. Overflow is a process-fatal
    /// invariant breach rather than an ABA collision.
    pub(super) fn advance_runtime_config_generation(&self) {
        self.runtime_config_generation
            .store(next_runtime_config_generation(), Ordering::Relaxed);
    }
}

struct ServiceManagerPublicationObservation {
    runtime_generation: GatewayRuntimeConfigGeneration,
    loadout: Option<GatewayLoadoutConfig>,
    virtual_servers: Vec<VirtualServerConfig>,
}

impl ServiceManagerPublicationObservation {
    fn same_publication(&self, other: &Self) -> bool {
        self.runtime_generation == other.runtime_generation
            && self.loadout == other.loadout
            && self.virtual_servers == other.virtual_servers
    }
}

fn map_catalog_error(_error: ToolCatalogPublicationError) -> LoadoutToolCatalogPublicationError {
    LoadoutToolCatalogPublicationError::CatalogUnavailable
}

fn map_service_catalog_error(
    _error: ServiceRegistryPublicationError,
) -> LoadoutServiceCatalogPublicationError {
    LoadoutServiceCatalogPublicationError::CatalogUnavailable
}
