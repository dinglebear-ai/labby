//! Coherent, generation-bearing views of the published gateway runtime.

use std::future::{Future, ready};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use labby_runtime::gateway_config::{GatewayLoadoutConfig, VirtualServerConfig};
use labby_runtime::gateway_config::{ProtectedMcpRouteConfig, ProtectedMcpRouteTarget};
use sha2::{Digest as _, Sha256};
use tokio::sync::OwnedRwLockReadGuard;

use crate::gateway::runtime::{PoolPublicationGeneration, PublishedPoolSnapshot};
use crate::upstream::pool::{
    PromptCatalogGeneration, PromptCatalogPublicationError, PublishedPromptCatalogSnapshot,
    PublishedPromptRoute, PublishedResourceCatalogSnapshot, PublishedResourceRoute,
    PublishedResourceTemplateCatalogSnapshot, PublishedResourceTemplateRoute,
    PublishedToolCatalogSnapshot, PublishedToolRoute, ResourceCatalogGeneration,
    ResourceCatalogPublicationError, ResourceTemplateCatalogGeneration,
    ResourceTemplateCatalogPublicationError, ToolCatalogGeneration, ToolCatalogPublicationError,
};

use super::GatewayManager;
use crate::gateway::service_registry::{
    PublishedService, PublishedServiceRegistrySnapshot, ServiceRegistryPublicationError,
    ServiceRegistryPublicationGeneration,
};

static NEXT_RUNTIME_CONFIG_GENERATION: AtomicU64 = AtomicU64::new(1);
const PUBLICATION_ATTEMPTS: usize = 3;

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

impl GatewayRuntimeConfigGeneration {
    #[must_use]
    pub fn fingerprint_bytes(self) -> [u8; 8] {
        self.0.to_be_bytes()
    }
}

const BOOTSTRAP_POLICY_LEASE_DEADLINE: Duration = Duration::from_millis(100);

/// Immutable authorization-policy publication held stable across the short
/// access bootstrap transaction. This is intentionally distinct from the
/// observational MCP item catalogs: credential authority binds the published
/// Loadout and protected-route policy, not transient upstream discovery.
pub struct PublishedBootstrapPolicyLease {
    _publication: OwnedRwLockReadGuard<()>,
    loadout_id: String,
    loadout_generation: u64,
    catalog_generation: u64,
    policy_fingerprint: [u8; 32],
    route_id: String,
    route_generation: u64,
    resource: String,
    audience: String,
    scopes: Vec<String>,
}

impl PublishedBootstrapPolicyLease {
    #[must_use]
    pub fn loadout_id(&self) -> &str {
        &self.loadout_id
    }
    #[must_use]
    pub fn loadout_generation(&self) -> u64 {
        self.loadout_generation
    }
    #[must_use]
    pub fn catalog_generation(&self) -> u64 {
        self.catalog_generation
    }
    #[must_use]
    pub fn policy_fingerprint(&self) -> [u8; 32] {
        self.policy_fingerprint
    }
    #[must_use]
    pub fn route_id(&self) -> &str {
        &self.route_id
    }
    #[must_use]
    pub fn route_generation(&self) -> u64 {
        self.route_generation
    }
    #[must_use]
    pub fn resource(&self) -> &str {
        &self.resource
    }
    #[must_use]
    pub fn audience(&self) -> &str {
        &self.audience
    }
    #[must_use]
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapPolicyLeaseError {
    Unavailable,
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadoutResourceCatalogPublicationError {
    MissingLoadout,
    MissingPool,
    CatalogUnavailable,
    Unstable,
}

impl std::fmt::Display for LoadoutResourceCatalogPublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MissingLoadout => "runtime Loadout is unavailable",
            Self::MissingPool => "runtime upstream pool is unavailable",
            Self::CatalogUnavailable => "runtime resource catalog is unavailable",
            Self::Unstable => "runtime resource catalog changed during observation",
        })
    }
}
impl std::error::Error for LoadoutResourceCatalogPublicationError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadoutResourceTemplateCatalogPublicationError {
    MissingLoadout,
    MissingPool,
    CatalogUnavailable,
    Unstable,
}

impl std::fmt::Display for LoadoutResourceTemplateCatalogPublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MissingLoadout => "runtime Loadout is unavailable",
            Self::MissingPool => "runtime upstream pool is unavailable",
            Self::CatalogUnavailable => "runtime resource template catalog is unavailable",
            Self::Unstable => "runtime resource template catalog changed during observation",
        })
    }
}
impl std::error::Error for LoadoutResourceTemplateCatalogPublicationError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadoutPromptCatalogPublicationError {
    MissingLoadout,
    MissingPool,
    CatalogUnavailable,
    Unstable,
}

impl std::fmt::Display for LoadoutPromptCatalogPublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MissingLoadout => "runtime Loadout is unavailable",
            Self::MissingPool => "runtime upstream pool is unavailable",
            Self::CatalogUnavailable => "runtime prompt catalog is unavailable",
            Self::Unstable => "runtime prompt catalog changed during observation",
        })
    }
}
impl std::error::Error for LoadoutPromptCatalogPublicationError {}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadoutMcpCatalogPublicationError {
    MissingLoadout,
    MissingPool,
    CatalogUnavailable,
    Unstable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectRoutePublicationError {
    Unavailable,
    Unstable,
}

impl std::fmt::Display for ProjectRoutePublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "project route publication is unavailable",
            Self::Unstable => "project route publication changed during observation",
        })
    }
}
impl std::error::Error for ProjectRoutePublicationError {}

/// Immutable Project-bound protected-route narrowing policy from one runtime
/// configuration publication. Observational and unmounted; not a grant.
pub struct PublishedProjectRouteSnapshot {
    runtime_config_generation: GatewayRuntimeConfigGeneration,
    route_name: std::sync::Arc<str>,
    resource: std::sync::Arc<str>,
    project_id: std::sync::Arc<str>,
    assigned_loadout_name: std::sync::Arc<str>,
    effective_loadout: GatewayLoadoutConfig,
    effective_service_names: std::sync::Arc<[std::sync::Arc<str>]>,
}

impl PublishedProjectRouteSnapshot {
    pub fn runtime_config_generation(&self) -> GatewayRuntimeConfigGeneration {
        self.runtime_config_generation
    }
    pub fn route_name(&self) -> &str {
        &self.route_name
    }
    pub fn resource(&self) -> &str {
        &self.resource
    }
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
    pub fn assigned_loadout_name(&self) -> &str {
        &self.assigned_loadout_name
    }
    pub fn effective_loadout(&self) -> &GatewayLoadoutConfig {
        &self.effective_loadout
    }
    pub fn effective_service_names(&self) -> &[std::sync::Arc<str>] {
        &self.effective_service_names
    }

    /// Whether both snapshots bind the same complete route publication.
    #[must_use]
    pub fn same_publication_as(&self, other: &Self) -> bool {
        self.runtime_config_generation == other.runtime_config_generation
            && self.route_name == other.route_name
            && self.resource == other.resource
            && self.project_id == other.project_id
            && self.assigned_loadout_name == other.assigned_loadout_name
            && self.effective_loadout == other.effective_loadout
            && self.effective_service_names == other.effective_service_names
    }
}

impl std::fmt::Display for LoadoutMcpCatalogPublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MissingLoadout => "runtime Loadout is unavailable",
            Self::MissingPool => "runtime upstream pool is unavailable",
            Self::CatalogUnavailable => "runtime MCP catalog is unavailable",
            Self::Unstable => "runtime MCP catalog changed during observation",
        })
    }
}

impl std::error::Error for LoadoutMcpCatalogPublicationError {}

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
    pub fn description(&self) -> &str {
        self.service.description()
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

    /// Resolve a raw advertised Tool name only when exactly one published
    /// regular upstream route matches it.
    #[must_use]
    pub fn unique_route_for_wire_name(&self, wire_name: &str) -> Option<&PublishedToolRoute> {
        unique_tool_route_for_wire_name(&self.routes, wire_name)
    }
}

fn unique_tool_route_for_wire_name<'a>(
    routes: &'a [PublishedToolRoute],
    wire_name: &str,
) -> Option<&'a PublishedToolRoute> {
    let mut matches = routes
        .iter()
        .filter(|route| route.tool_name.as_ref() == wire_name);
    let route = matches.next()?;
    matches.next().is_none().then_some(route)
}

/// Immutable observational Resource projection for one running Loadout.
/// This is unmounted and is neither read authority nor an authorization grant.
pub struct PublishedLoadoutResourceCatalogSnapshot {
    runtime_config_generation: GatewayRuntimeConfigGeneration,
    pool_publication_generation: PoolPublicationGeneration,
    resource_catalog_generation: ResourceCatalogGeneration,
    routes: std::sync::Arc<[PublishedResourceRoute]>,
}

impl PublishedLoadoutResourceCatalogSnapshot {
    #[must_use]
    pub fn runtime_config_generation(&self) -> GatewayRuntimeConfigGeneration {
        self.runtime_config_generation
    }
    #[must_use]
    pub fn pool_publication_generation(&self) -> PoolPublicationGeneration {
        self.pool_publication_generation
    }
    #[must_use]
    pub fn resource_catalog_generation(&self) -> ResourceCatalogGeneration {
        self.resource_catalog_generation
    }
    #[must_use]
    pub fn routes(&self) -> &[PublishedResourceRoute] {
        &self.routes
    }

    /// Resolve a canonical gateway Resource URI only when exactly one route
    /// constructs it. Ambiguous namespace shapes fail closed.
    #[must_use]
    pub fn unique_route_for_wire_uri(&self, wire_uri: &str) -> Option<&PublishedResourceRoute> {
        unique_resource_route_for_wire_uri(&self.routes, wire_uri)
    }
}

fn unique_resource_route_for_wire_uri<'a>(
    routes: &'a [PublishedResourceRoute],
    wire_uri: &str,
) -> Option<&'a PublishedResourceRoute> {
    let suffix = wire_uri.strip_prefix("lab://upstream/")?;
    let mut matches = routes.iter().filter(|route| {
        suffix
            .strip_prefix(route.upstream_name.as_ref())
            .and_then(|native| native.strip_prefix('/'))
            .is_some_and(|native| native == route.native_uri.as_ref())
    });
    let route = matches.next()?;
    matches.next().is_none().then_some(route)
}

/// Immutable observational ResourceTemplate projection for one running Loadout.
/// This is unmounted and is neither read authority nor an authorization grant.
pub struct PublishedLoadoutResourceTemplateCatalogSnapshot {
    runtime_config_generation: GatewayRuntimeConfigGeneration,
    pool_publication_generation: PoolPublicationGeneration,
    resource_template_catalog_generation: ResourceTemplateCatalogGeneration,
    routes: std::sync::Arc<[PublishedResourceTemplateRoute]>,
}

impl PublishedLoadoutResourceTemplateCatalogSnapshot {
    #[must_use]
    pub fn runtime_config_generation(&self) -> GatewayRuntimeConfigGeneration {
        self.runtime_config_generation
    }
    #[must_use]
    pub fn pool_publication_generation(&self) -> PoolPublicationGeneration {
        self.pool_publication_generation
    }
    #[must_use]
    pub fn resource_template_catalog_generation(&self) -> ResourceTemplateCatalogGeneration {
        self.resource_template_catalog_generation
    }
    #[must_use]
    pub fn routes(&self) -> &[PublishedResourceTemplateRoute] {
        &self.routes
    }
}

/// Immutable observational Prompt projection for one running Loadout.
/// This is unmounted and is neither prompt execution authority nor a grant.
pub struct PublishedLoadoutPromptCatalogSnapshot {
    runtime_config_generation: GatewayRuntimeConfigGeneration,
    pool_publication_generation: PoolPublicationGeneration,
    prompt_catalog_generation: PromptCatalogGeneration,
    routes: std::sync::Arc<[PublishedPromptRoute]>,
}

impl PublishedLoadoutPromptCatalogSnapshot {
    #[must_use]
    pub fn runtime_config_generation(&self) -> GatewayRuntimeConfigGeneration {
        self.runtime_config_generation
    }
    #[must_use]
    pub fn pool_publication_generation(&self) -> PoolPublicationGeneration {
        self.pool_publication_generation
    }
    #[must_use]
    pub fn prompt_catalog_generation(&self) -> PromptCatalogGeneration {
        self.prompt_catalog_generation
    }
    #[must_use]
    pub fn routes(&self) -> &[PublishedPromptRoute] {
        &self.routes
    }

    /// Resolve one canonical gateway Prompt name (`upstream/native`) only when
    /// exactly one published route produces it. Ambiguous namespace shapes
    /// fail closed rather than parsing the owner at the first slash or relying
    /// on route order.
    #[must_use]
    pub fn unique_route_for_wire_name(&self, wire_name: &str) -> Option<&PublishedPromptRoute> {
        unique_prompt_route_for_wire_name(&self.routes, wire_name)
    }
}

fn unique_prompt_route_for_wire_name<'a>(
    routes: &'a [PublishedPromptRoute],
    wire_name: &str,
) -> Option<&'a PublishedPromptRoute> {
    let mut matches = routes.iter().filter(|route| {
        wire_name
            .strip_prefix(route.upstream_name.as_ref())
            .and_then(|suffix| suffix.strip_prefix('/'))
            .is_some_and(|native| native == route.native_name.as_ref())
    });
    let route = matches.next()?;
    matches.next().is_none().then_some(route)
}

/// One common-interval view of the Loadout's tools, regular Resources,
/// ResourceTemplates, Prompts, and services.
/// This remains observational and unmounted; it is not an execution grant.
pub struct PublishedLoadoutMcpCatalogSnapshot {
    tools: PublishedLoadoutToolCatalogSnapshot,
    resources: PublishedLoadoutResourceCatalogSnapshot,
    resource_templates: PublishedLoadoutResourceTemplateCatalogSnapshot,
    prompts: PublishedLoadoutPromptCatalogSnapshot,
    services: PublishedLoadoutServiceCatalogSnapshot,
}

impl PublishedLoadoutMcpCatalogSnapshot {
    #[must_use]
    pub fn tools(&self) -> &PublishedLoadoutToolCatalogSnapshot {
        &self.tools
    }
    #[must_use]
    pub fn resources(&self) -> &PublishedLoadoutResourceCatalogSnapshot {
        &self.resources
    }
    #[must_use]
    pub fn resource_templates(&self) -> &PublishedLoadoutResourceTemplateCatalogSnapshot {
        &self.resource_templates
    }
    #[must_use]
    pub fn prompts(&self) -> &PublishedLoadoutPromptCatalogSnapshot {
        &self.prompts
    }
    #[must_use]
    pub fn services(&self) -> &PublishedLoadoutServiceCatalogSnapshot {
        &self.services
    }

    /// Equality of every source publication identity in this unified snapshot.
    /// Construction guarantees all five child snapshots carry the same runtime
    /// generation, so one runtime identity plus the six remaining source
    /// identities is the complete comparison tuple.
    #[must_use]
    pub fn same_publication_as(&self, other: &Self) -> bool {
        self.tools.runtime_config_generation() == other.tools.runtime_config_generation()
            && self.tools.pool_publication_generation() == other.tools.pool_publication_generation()
            && self.tools.tool_catalog_generation() == other.tools.tool_catalog_generation()
            && self.resources.resource_catalog_generation()
                == other.resources.resource_catalog_generation()
            && self
                .resource_templates
                .resource_template_catalog_generation()
                == other
                    .resource_templates
                    .resource_template_catalog_generation()
            && self.prompts.prompt_catalog_generation() == other.prompts.prompt_catalog_generation()
            && self.services.service_registry_generation()
                == other.services.service_registry_generation()
    }
}

struct ManagerPublicationObservation {
    runtime_generation: GatewayRuntimeConfigGeneration,
    loadout: Option<GatewayLoadoutConfig>,
    pool_snapshot: PublishedPoolSnapshot,
}

struct McpManagerPublicationObservation {
    runtime_generation: GatewayRuntimeConfigGeneration,
    loadout: Option<GatewayLoadoutConfig>,
    virtual_servers: Vec<VirtualServerConfig>,
    pool_snapshot: PublishedPoolSnapshot,
}

impl McpManagerPublicationObservation {
    fn same_publication(&self, other: &Self) -> bool {
        self.runtime_generation == other.runtime_generation
            && self.loadout == other.loadout
            && self.virtual_servers == other.virtual_servers
            && self.pool_snapshot.generation() == other.pool_snapshot.generation()
            && match (self.pool_snapshot.pool(), other.pool_snapshot.pool()) {
                (Some(left), Some(right)) => std::sync::Arc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            }
    }
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
    /// Acquire a bounded lease over the exact published Loadout and protected
    /// route policy used for first-owner credential admission.
    pub async fn acquire_published_bootstrap_policy_lease(
        &self,
        loadout_id: &str,
        route_id: &str,
    ) -> Result<PublishedBootstrapPolicyLease, BootstrapPolicyLeaseError> {
        let started = tokio::time::Instant::now();
        let publication = tokio::time::timeout(
            BOOTSTRAP_POLICY_LEASE_DEADLINE,
            std::sync::Arc::clone(&self.publication_barrier).read_owned(),
        )
        .await
        .map_err(|_| BootstrapPolicyLeaseError::Unavailable)?;
        let remaining = BOOTSTRAP_POLICY_LEASE_DEADLINE
            .checked_sub(started.elapsed())
            .ok_or(BootstrapPolicyLeaseError::Unavailable)?;
        let config = tokio::time::timeout(remaining, self.config.read())
            .await
            .map_err(|_| BootstrapPolicyLeaseError::Unavailable)?;
        let loadout = config
            .loadouts
            .iter()
            .find(|candidate| candidate.name == loadout_id)
            .filter(|loadout| !loadout.name.is_empty())
            .ok_or(BootstrapPolicyLeaseError::Unavailable)?;
        let route = config
            .protected_mcp_routes
            .iter()
            .find(|candidate| candidate.name == route_id && candidate.enabled)
            .filter(|route| {
                route
                    .gateway_subset_target()
                    .and_then(|target| target.loadout.as_deref())
                    == Some(loadout_id)
            })
            .ok_or(BootstrapPolicyLeaseError::Unavailable)?;
        let generation =
            GatewayRuntimeConfigGeneration(self.runtime_config_generation.load(Ordering::Relaxed));
        let generation_value = u64::from_be_bytes(generation.fingerprint_bytes());
        let mut scopes = route.scopes.clone();
        scopes.sort();
        scopes.dedup();
        if scopes.is_empty() {
            return Err(BootstrapPolicyLeaseError::Unavailable);
        }
        let resource = route.public_resource();
        let policy = serde_json::to_vec(&(loadout, route, &scopes))
            .map_err(|_| BootstrapPolicyLeaseError::Unavailable)?;
        let policy_fingerprint = Sha256::digest(policy).into();
        let admitted_loadout_id = loadout.name.clone();
        let admitted_route_id = route.name.clone();
        drop(config);
        Ok(PublishedBootstrapPolicyLease {
            _publication: publication,
            loadout_id: admitted_loadout_id,
            loadout_generation: generation_value,
            catalog_generation: generation_value,
            policy_fingerprint,
            route_id: admitted_route_id,
            route_generation: generation_value,
            audience: resource.clone(),
            resource,
            scopes,
        })
    }

    async fn route_publication_observation(
        &self,
        route_name: &str,
        project_id: &str,
        assigned_loadout_name: &str,
    ) -> RoutePublicationObservation {
        let _publication = self.publication_barrier.read().await;
        let config = self.config.read().await;
        let generation =
            GatewayRuntimeConfigGeneration(self.runtime_config_generation.load(Ordering::Relaxed));
        RoutePublicationObservation {
            generation,
            snapshot: project_route_snapshot(
                generation,
                &config.protected_mcp_routes,
                &config.loadouts,
                &config.virtual_servers,
                route_name,
                project_id,
                assigned_loadout_name,
            ),
        }
    }

    pub async fn published_project_route_snapshot(
        &self,
        route_name: &str,
        project_id: &str,
        assigned_loadout_name: &str,
    ) -> Result<PublishedProjectRouteSnapshot, ProjectRoutePublicationError> {
        self.compose_project_route_snapshot(route_name, project_id, assigned_loadout_name, |_| {
            ready(())
        })
        .await
    }

    pub(super) async fn compose_project_route_snapshot<F, Fut>(
        &self,
        route_name: &str,
        project_id: &str,
        assigned_loadout_name: &str,
        mut after_first: F,
    ) -> Result<PublishedProjectRouteSnapshot, ProjectRoutePublicationError>
    where
        F: FnMut(usize) -> Fut,
        Fut: Future<Output = ()>,
    {
        for attempt in 0..PUBLICATION_ATTEMPTS {
            let first = self
                .route_publication_observation(route_name, project_id, assigned_loadout_name)
                .await;
            after_first(attempt).await;
            let second = self
                .route_publication_observation(route_name, project_id, assigned_loadout_name)
                .await;
            if first.generation != second.generation {
                continue;
            }
            return first.snapshot;
        }
        Err(ProjectRoutePublicationError::Unstable)
    }

    async fn mcp_publication_observation(&self, name: &str) -> McpManagerPublicationObservation {
        let _publication = self.publication_barrier.read().await;
        let config = self.config.read().await;
        McpManagerPublicationObservation {
            runtime_generation: GatewayRuntimeConfigGeneration(
                self.runtime_config_generation.load(Ordering::Relaxed),
            ),
            loadout: config
                .loadouts
                .iter()
                .find(|loadout| loadout.name == name)
                .cloned(),
            virtual_servers: config.virtual_servers.clone(),
            pool_snapshot: self.runtime.published_pool_snapshot(),
        }
    }

    pub async fn published_loadout_mcp_catalog_snapshot(
        &self,
        name: &str,
    ) -> Result<PublishedLoadoutMcpCatalogSnapshot, LoadoutMcpCatalogPublicationError> {
        self.compose_loadout_mcp_catalog(name, |_: usize| ready(()))
            .await
    }

    pub(super) async fn compose_loadout_mcp_catalog<F, Fut>(
        &self,
        name: &str,
        mut after_first_catalogs: F,
    ) -> Result<PublishedLoadoutMcpCatalogSnapshot, LoadoutMcpCatalogPublicationError>
    where
        F: FnMut(usize) -> Fut,
        Fut: Future<Output = ()>,
    {
        for attempt in 0..PUBLICATION_ATTEMPTS {
            let first_gateway = self.mcp_publication_observation(name).await;
            let first_tools = match first_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_tool_catalog().await),
                None => None,
            };
            let first_resources = match first_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_resource_catalog().await),
                None => None,
            };
            let first_resource_templates = match first_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_resource_template_catalog().await),
                None => None,
            };
            let first_prompts = match first_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_prompt_catalog().await),
                None => None,
            };
            let first_services = self.published_service_registry_snapshot();
            after_first_catalogs(attempt).await;
            let second_gateway = self.mcp_publication_observation(name).await;
            if !first_gateway.same_publication(&second_gateway) {
                continue;
            }
            let second_tools = match second_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_tool_catalog().await),
                None => None,
            };
            let second_resources = match second_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_resource_catalog().await),
                None => None,
            };
            let second_resource_templates = match second_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_resource_template_catalog().await),
                None => None,
            };
            let second_prompts = match second_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_prompt_catalog().await),
                None => None,
            };
            let second_services = self.published_service_registry_snapshot();
            let loadout = first_gateway
                .loadout
                .as_ref()
                .ok_or(LoadoutMcpCatalogPublicationError::MissingLoadout)?;
            let (first_tools, second_tools) = match (first_tools, second_tools) {
                (None, None) => return Err(LoadoutMcpCatalogPublicationError::MissingPool),
                (Some(Ok(first)), Some(Ok(second))) => (first, second),
                (Some(Err(first)), Some(Err(second))) if first == second => {
                    return Err(LoadoutMcpCatalogPublicationError::CatalogUnavailable);
                }
                _ => continue,
            };
            let (first_services, second_services) = match (first_services, second_services) {
                (Ok(first), Ok(second)) => (first, second),
                (Err(first), Err(second)) if first == second => {
                    return Err(LoadoutMcpCatalogPublicationError::CatalogUnavailable);
                }
                _ => continue,
            };
            let (first_resources, second_resources) = match (first_resources, second_resources) {
                (None, None) => return Err(LoadoutMcpCatalogPublicationError::MissingPool),
                (Some(Ok(first)), Some(Ok(second))) => (first, second),
                (Some(Err(first)), Some(Err(second))) if first == second => {
                    return Err(LoadoutMcpCatalogPublicationError::CatalogUnavailable);
                }
                _ => continue,
            };
            let (first_resource_templates, second_resource_templates) =
                match (first_resource_templates, second_resource_templates) {
                    (None, None) => return Err(LoadoutMcpCatalogPublicationError::MissingPool),
                    (Some(Ok(first)), Some(Ok(second))) => (first, second),
                    (Some(Err(first)), Some(Err(second))) if first == second => {
                        return Err(LoadoutMcpCatalogPublicationError::CatalogUnavailable);
                    }
                    _ => continue,
                };
            let (first_prompts, second_prompts) = match (first_prompts, second_prompts) {
                (None, None) => return Err(LoadoutMcpCatalogPublicationError::MissingPool),
                (Some(Ok(first)), Some(Ok(second))) => (first, second),
                (Some(Err(first)), Some(Err(second))) if first == second => {
                    return Err(LoadoutMcpCatalogPublicationError::CatalogUnavailable);
                }
                _ => continue,
            };
            if first_tools.generation() != second_tools.generation()
                || first_resources.generation() != second_resources.generation()
                || first_resource_templates.generation() != second_resource_templates.generation()
                || first_prompts.generation() != second_prompts.generation()
                || first_services.generation() != second_services.generation()
            {
                continue;
            }
            let tools = build_tool_snapshot(
                first_gateway.runtime_generation,
                first_gateway.pool_snapshot.generation(),
                loadout,
                &first_tools,
            );
            let services = build_service_snapshot(
                first_gateway.runtime_generation,
                loadout,
                &first_gateway.virtual_servers,
                &first_services,
            )
            .map_err(|_| LoadoutMcpCatalogPublicationError::CatalogUnavailable)?;
            let resources = build_resource_snapshot(
                first_gateway.runtime_generation,
                first_gateway.pool_snapshot.generation(),
                loadout,
                &first_resources,
            );
            let resource_templates = build_resource_template_snapshot(
                first_gateway.runtime_generation,
                first_gateway.pool_snapshot.generation(),
                loadout,
                &first_resource_templates,
            );
            let prompts = build_prompt_snapshot(
                first_gateway.runtime_generation,
                first_gateway.pool_snapshot.generation(),
                loadout,
                &first_prompts,
            );
            debug_assert_eq!(
                tools.runtime_config_generation(),
                services.runtime_config_generation()
            );
            debug_assert_eq!(
                tools.runtime_config_generation(),
                resources.runtime_config_generation()
            );
            debug_assert_eq!(
                tools.pool_publication_generation(),
                resources.pool_publication_generation()
            );
            debug_assert_eq!(
                tools.runtime_config_generation(),
                resource_templates.runtime_config_generation()
            );
            debug_assert_eq!(
                tools.pool_publication_generation(),
                resource_templates.pool_publication_generation()
            );
            debug_assert_eq!(
                tools.runtime_config_generation(),
                prompts.runtime_config_generation()
            );
            debug_assert_eq!(
                tools.pool_publication_generation(),
                prompts.pool_publication_generation()
            );
            return Ok(PublishedLoadoutMcpCatalogSnapshot {
                tools,
                resources,
                resource_templates,
                prompts,
                services,
            });
        }
        Err(LoadoutMcpCatalogPublicationError::Unstable)
    }
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
        for attempt in 0..PUBLICATION_ATTEMPTS {
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
            return build_service_snapshot(
                first_gateway.runtime_generation,
                loadout,
                &first_gateway.virtual_servers,
                &first_catalog,
            );
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
        for attempt in 0..PUBLICATION_ATTEMPTS {
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

            return Ok(build_tool_snapshot(
                first_gateway.runtime_generation,
                first_gateway.pool_snapshot.generation(),
                loadout,
                &first_catalog,
            ));
        }
        Err(LoadoutToolCatalogPublicationError::Unstable)
    }

    pub async fn published_loadout_resource_catalog_snapshot(
        &self,
        name: &str,
    ) -> Result<PublishedLoadoutResourceCatalogSnapshot, LoadoutResourceCatalogPublicationError>
    {
        self.compose_loadout_resource_catalog(name, |_: usize| ready(()))
            .await
    }

    pub(super) async fn compose_loadout_resource_catalog<F, Fut>(
        &self,
        name: &str,
        mut after_first_catalog: F,
    ) -> Result<PublishedLoadoutResourceCatalogSnapshot, LoadoutResourceCatalogPublicationError>
    where
        F: FnMut(usize) -> Fut,
        Fut: Future<Output = ()>,
    {
        for attempt in 0..PUBLICATION_ATTEMPTS {
            let first_gateway = self.manager_publication_observation(name).await;
            let first_catalog = match first_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_resource_catalog().await),
                None => None,
            };
            after_first_catalog(attempt).await;
            let second_gateway = self.manager_publication_observation(name).await;
            if !first_gateway.same_publication(&second_gateway) {
                continue;
            }
            let second_catalog = match second_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_resource_catalog().await),
                None => None,
            };
            let Some(loadout) = first_gateway.loadout.as_ref() else {
                return Err(LoadoutResourceCatalogPublicationError::MissingLoadout);
            };
            let (first_catalog, second_catalog) = match (first_catalog, second_catalog) {
                (None, None) => return Err(LoadoutResourceCatalogPublicationError::MissingPool),
                (Some(Err(first)), Some(Err(second))) if first == second => {
                    return Err(map_resource_catalog_error(first));
                }
                (Some(Ok(first)), Some(Ok(second))) => (first, second),
                _ => continue,
            };
            if first_catalog.generation() != second_catalog.generation() {
                continue;
            }
            return Ok(build_resource_snapshot(
                first_gateway.runtime_generation,
                first_gateway.pool_snapshot.generation(),
                loadout,
                &first_catalog,
            ));
        }
        Err(LoadoutResourceCatalogPublicationError::Unstable)
    }

    pub async fn published_loadout_resource_template_catalog_snapshot(
        &self,
        name: &str,
    ) -> Result<
        PublishedLoadoutResourceTemplateCatalogSnapshot,
        LoadoutResourceTemplateCatalogPublicationError,
    > {
        self.compose_loadout_resource_template_catalog(name, |_: usize| ready(()))
            .await
    }

    pub(super) async fn compose_loadout_resource_template_catalog<F, Fut>(
        &self,
        name: &str,
        mut after_first_catalog: F,
    ) -> Result<
        PublishedLoadoutResourceTemplateCatalogSnapshot,
        LoadoutResourceTemplateCatalogPublicationError,
    >
    where
        F: FnMut(usize) -> Fut,
        Fut: Future<Output = ()>,
    {
        for attempt in 0..PUBLICATION_ATTEMPTS {
            let first_gateway = self.manager_publication_observation(name).await;
            let first_catalog = match first_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_resource_template_catalog().await),
                None => None,
            };
            after_first_catalog(attempt).await;
            let second_gateway = self.manager_publication_observation(name).await;
            if !first_gateway.same_publication(&second_gateway) {
                continue;
            }
            let second_catalog = match second_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_resource_template_catalog().await),
                None => None,
            };
            let Some(loadout) = first_gateway.loadout.as_ref() else {
                return Err(LoadoutResourceTemplateCatalogPublicationError::MissingLoadout);
            };
            let (first_catalog, second_catalog) = match (first_catalog, second_catalog) {
                (None, None) => {
                    return Err(LoadoutResourceTemplateCatalogPublicationError::MissingPool);
                }
                (Some(Err(first)), Some(Err(second))) if first == second => {
                    return Err(map_resource_template_catalog_error(first));
                }
                (Some(Ok(first)), Some(Ok(second))) => (first, second),
                _ => continue,
            };
            if first_catalog.generation() != second_catalog.generation() {
                continue;
            }
            return Ok(build_resource_template_snapshot(
                first_gateway.runtime_generation,
                first_gateway.pool_snapshot.generation(),
                loadout,
                &first_catalog,
            ));
        }
        Err(LoadoutResourceTemplateCatalogPublicationError::Unstable)
    }

    pub async fn published_loadout_prompt_catalog_snapshot(
        &self,
        name: &str,
    ) -> Result<PublishedLoadoutPromptCatalogSnapshot, LoadoutPromptCatalogPublicationError> {
        self.compose_loadout_prompt_catalog(name, |_: usize| ready(()))
            .await
    }

    pub(super) async fn compose_loadout_prompt_catalog<F, Fut>(
        &self,
        name: &str,
        mut after_first_catalog: F,
    ) -> Result<PublishedLoadoutPromptCatalogSnapshot, LoadoutPromptCatalogPublicationError>
    where
        F: FnMut(usize) -> Fut,
        Fut: Future<Output = ()>,
    {
        for attempt in 0..PUBLICATION_ATTEMPTS {
            let first_gateway = self.manager_publication_observation(name).await;
            let first_catalog = match first_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_prompt_catalog().await),
                None => None,
            };
            after_first_catalog(attempt).await;
            let second_gateway = self.manager_publication_observation(name).await;
            if !first_gateway.same_publication(&second_gateway) {
                continue;
            }
            let second_catalog = match second_gateway.pool_snapshot.pool() {
                Some(pool) => Some(pool.published_prompt_catalog().await),
                None => None,
            };
            let Some(loadout) = first_gateway.loadout.as_ref() else {
                return Err(LoadoutPromptCatalogPublicationError::MissingLoadout);
            };
            let (first_catalog, second_catalog) = match (first_catalog, second_catalog) {
                (None, None) => return Err(LoadoutPromptCatalogPublicationError::MissingPool),
                (Some(Err(first)), Some(Err(second))) if first == second => {
                    return Err(map_prompt_catalog_error(first));
                }
                (Some(Ok(first)), Some(Ok(second))) => (first, second),
                _ => continue,
            };
            if first_catalog.generation() != second_catalog.generation() {
                continue;
            }
            return Ok(build_prompt_snapshot(
                first_gateway.runtime_generation,
                first_gateway.pool_snapshot.generation(),
                loadout,
                &first_catalog,
            ));
        }
        Err(LoadoutPromptCatalogPublicationError::Unstable)
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

fn map_resource_catalog_error(
    _error: ResourceCatalogPublicationError,
) -> LoadoutResourceCatalogPublicationError {
    LoadoutResourceCatalogPublicationError::CatalogUnavailable
}

fn map_resource_template_catalog_error(
    _error: ResourceTemplateCatalogPublicationError,
) -> LoadoutResourceTemplateCatalogPublicationError {
    LoadoutResourceTemplateCatalogPublicationError::CatalogUnavailable
}

fn map_prompt_catalog_error(
    _error: PromptCatalogPublicationError,
) -> LoadoutPromptCatalogPublicationError {
    LoadoutPromptCatalogPublicationError::CatalogUnavailable
}

fn map_service_catalog_error(
    _error: ServiceRegistryPublicationError,
) -> LoadoutServiceCatalogPublicationError {
    LoadoutServiceCatalogPublicationError::CatalogUnavailable
}

fn build_tool_snapshot(
    runtime_config_generation: GatewayRuntimeConfigGeneration,
    pool_publication_generation: PoolPublicationGeneration,
    loadout: &GatewayLoadoutConfig,
    catalog: &PublishedToolCatalogSnapshot,
) -> PublishedLoadoutToolCatalogSnapshot {
    let routes = if loadout.expose_tools {
        let upstreams = loadout
            .upstreams
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        catalog
            .routes()
            .iter()
            .filter(|route| upstreams.contains(route.upstream_name.as_ref()))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    PublishedLoadoutToolCatalogSnapshot {
        runtime_config_generation,
        pool_publication_generation,
        tool_catalog_generation: catalog.generation(),
        routes: std::sync::Arc::from(routes),
    }
}

fn build_resource_snapshot(
    runtime_config_generation: GatewayRuntimeConfigGeneration,
    pool_publication_generation: PoolPublicationGeneration,
    loadout: &GatewayLoadoutConfig,
    catalog: &PublishedResourceCatalogSnapshot,
) -> PublishedLoadoutResourceCatalogSnapshot {
    let routes = if loadout.expose_resources {
        let upstreams = loadout
            .upstreams
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        catalog
            .routes()
            .iter()
            .filter(|route| upstreams.contains(route.upstream_name.as_ref()))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    PublishedLoadoutResourceCatalogSnapshot {
        runtime_config_generation,
        pool_publication_generation,
        resource_catalog_generation: catalog.generation(),
        routes: std::sync::Arc::from(routes),
    }
}

fn build_resource_template_snapshot(
    runtime_config_generation: GatewayRuntimeConfigGeneration,
    pool_publication_generation: PoolPublicationGeneration,
    loadout: &GatewayLoadoutConfig,
    catalog: &PublishedResourceTemplateCatalogSnapshot,
) -> PublishedLoadoutResourceTemplateCatalogSnapshot {
    let routes = if loadout.expose_resources {
        let upstreams = loadout
            .upstreams
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        catalog
            .routes()
            .iter()
            .filter(|route| upstreams.contains(route.upstream_name.as_ref()))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    PublishedLoadoutResourceTemplateCatalogSnapshot {
        runtime_config_generation,
        pool_publication_generation,
        resource_template_catalog_generation: catalog.generation(),
        routes: std::sync::Arc::from(routes),
    }
}

fn build_prompt_snapshot(
    runtime_config_generation: GatewayRuntimeConfigGeneration,
    pool_publication_generation: PoolPublicationGeneration,
    loadout: &GatewayLoadoutConfig,
    catalog: &PublishedPromptCatalogSnapshot,
) -> PublishedLoadoutPromptCatalogSnapshot {
    let routes = if loadout.expose_prompts {
        let upstreams = loadout
            .upstreams
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        catalog
            .routes()
            .iter()
            .filter(|route| upstreams.contains(route.upstream_name.as_ref()))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    PublishedLoadoutPromptCatalogSnapshot {
        runtime_config_generation,
        pool_publication_generation,
        prompt_catalog_generation: catalog.generation(),
        routes: std::sync::Arc::from(routes),
    }
}

fn build_service_snapshot(
    runtime_config_generation: GatewayRuntimeConfigGeneration,
    loadout: &GatewayLoadoutConfig,
    virtual_servers: &[VirtualServerConfig],
    catalog: &PublishedServiceRegistrySnapshot,
) -> Result<PublishedLoadoutServiceCatalogSnapshot, LoadoutServiceCatalogPublicationError> {
    let mut selected = std::collections::BTreeMap::new();
    if loadout.expose_tools {
        let by_name = catalog
            .services()
            .iter()
            .map(|service| (service.name(), service))
            .collect::<std::collections::BTreeMap<_, _>>();
        for member in &loadout.services {
            let server = resolve_virtual_server_member(virtual_servers, member);
            let service_name = server.map_or(member.as_str(), |server| server.service.as_str());
            let Some(service) = by_name.get(service_name) else {
                continue;
            };
            let projected = match server {
                None => (*service).clone(),
                Some(_) => {
                    match super::views::mcp_service_policy_for_config(virtual_servers, member) {
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
                    }
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
    }
    Ok(PublishedLoadoutServiceCatalogSnapshot {
        runtime_config_generation,
        service_registry_generation: catalog.generation(),
        services: std::sync::Arc::from(selected.into_values().collect::<Vec<_>>()),
    })
}

fn resolve_virtual_server_member<'a>(
    virtual_servers: &'a [VirtualServerConfig],
    member: &str,
) -> Option<&'a VirtualServerConfig> {
    virtual_servers.iter().find(|server| server.id == member)
}

struct RoutePublicationObservation {
    generation: GatewayRuntimeConfigGeneration,
    snapshot: Result<PublishedProjectRouteSnapshot, ProjectRoutePublicationError>,
}

fn project_route_snapshot(
    generation: GatewayRuntimeConfigGeneration,
    routes: &[ProtectedMcpRouteConfig],
    loadouts: &[GatewayLoadoutConfig],
    virtual_servers: &[VirtualServerConfig],
    route_name: &str,
    project_id: &str,
    assigned_loadout_name: &str,
) -> Result<PublishedProjectRouteSnapshot, ProjectRoutePublicationError> {
    let mut matching = routes.iter().filter(|route| route.name == route_name);
    let route = matching
        .next()
        .ok_or(ProjectRoutePublicationError::Unavailable)?;
    if matching.next().is_some() || !route.enabled {
        return Err(ProjectRoutePublicationError::Unavailable);
    }
    let resource = crate::gateway::protected_routes::canonical_route_resource(route)
        .ok_or(ProjectRoutePublicationError::Unavailable)?;
    if routes
        .iter()
        .filter(|candidate| {
            candidate.enabled
                && crate::gateway::protected_routes::canonical_route_resource(candidate).as_deref()
                    == Some(resource.as_str())
        })
        .count()
        != 1
    {
        return Err(ProjectRoutePublicationError::Unavailable);
    }
    let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = route.target.as_ref() else {
        return Err(ProjectRoutePublicationError::Unavailable);
    };
    if !labby_runtime::gateway_config::is_canonical_project_id(project_id)
        || target.canonical_project_id() != Some(project_id)
    {
        return Err(ProjectRoutePublicationError::Unavailable);
    }
    let mut assigned_matches = loadouts
        .iter()
        .filter(|loadout| loadout.name == assigned_loadout_name);
    let assigned = assigned_matches
        .next()
        .ok_or(ProjectRoutePublicationError::Unavailable)?;
    if assigned_matches.next().is_some() {
        return Err(ProjectRoutePublicationError::Unavailable);
    }
    let effective = assigned
        .intersect_gateway_subset(target)
        .map_err(|_| ProjectRoutePublicationError::Unavailable)?;
    let mut effective_service_names = std::collections::BTreeSet::new();
    for member in &effective.services {
        let service_name = resolve_virtual_server_member(virtual_servers, member)
            .map_or(member.as_str(), |server| server.service.as_str());
        if !effective_service_names.insert(service_name.to_string()) {
            return Err(ProjectRoutePublicationError::Unavailable);
        }
    }
    Ok(PublishedProjectRouteSnapshot {
        runtime_config_generation: generation,
        route_name: std::sync::Arc::from(route.name.as_str()),
        resource: std::sync::Arc::from(resource),
        project_id: std::sync::Arc::from(project_id),
        assigned_loadout_name: std::sync::Arc::from(assigned_loadout_name),
        effective_loadout: effective,
        effective_service_names: std::sync::Arc::from(
            effective_service_names
                .into_iter()
                .map(std::sync::Arc::<str>::from)
                .collect::<Vec<_>>(),
        ),
    })
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // Test fixture constructs upstream-owned descriptors directly.
mod canonical_route_match_tests {
    use std::sync::Arc;

    use rmcp::model::{Prompt, Resource, Tool};

    use super::{
        PublishedPromptRoute, PublishedResourceRoute, PublishedToolRoute,
        unique_prompt_route_for_wire_name, unique_resource_route_for_wire_uri,
        unique_tool_route_for_wire_name,
    };
    use crate::upstream::types::UpstreamTool;

    #[test]
    fn raw_tool_name_is_unique_and_order_independent() {
        let make = |upstream: &str, native: &str| PublishedToolRoute {
            upstream_name: Arc::from(upstream),
            tool_name: Arc::from(native),
            tool: UpstreamTool {
                tool: Tool::new(native.to_string(), "test", serde_json::Map::new()),
                input_schema: None,
                output_schema: None,
                upstream_name: Arc::from(upstream),
                destructive: false,
            },
        };
        let routes = vec![make("alpha", "nested/name"), make("bravo", "nested/name")];
        let wire = "nested/name";
        assert!(unique_tool_route_for_wire_name(&routes, wire).is_none());
        let reversed = routes.iter().cloned().rev().collect::<Vec<_>>();
        assert!(unique_tool_route_for_wire_name(&reversed, wire).is_none());
        assert_eq!(
            unique_tool_route_for_wire_name(&routes[..1], wire)
                .unwrap()
                .tool_name
                .as_ref(),
            "nested/name"
        );
        assert!(unique_tool_route_for_wire_name(&routes[..1], "alpha::nested/name").is_none());
        assert!(unique_tool_route_for_wire_name(&routes[..1], "alpha/nested/name").is_none());
    }

    #[test]
    fn canonical_prompt_wire_name_rejects_ambiguous_decompositions() {
        let routes = vec![
            PublishedPromptRoute {
                upstream_name: Arc::from("alpha"),
                native_name: Arc::from("bravo/name"),
                prompt: Prompt::new("bravo/name", None::<String>, None),
            },
            PublishedPromptRoute {
                upstream_name: Arc::from("alpha/bravo"),
                native_name: Arc::from("name"),
                prompt: Prompt::new("name", None::<String>, None),
            },
        ];

        assert!(unique_prompt_route_for_wire_name(&routes, "alpha/bravo/name").is_none());
        assert_eq!(
            unique_prompt_route_for_wire_name(&routes[..1], "alpha/bravo/name")
                .unwrap()
                .native_name
                .as_ref(),
            "bravo/name"
        );
    }

    #[test]
    fn canonical_resource_wire_uri_rejects_ambiguous_decompositions() {
        let routes = vec![
            PublishedResourceRoute {
                upstream_name: Arc::from("alpha"),
                native_uri: Arc::from("bravo/file:///one"),
                resource: Resource::new("bravo/file:///one", "one"),
            },
            PublishedResourceRoute {
                upstream_name: Arc::from("alpha/bravo"),
                native_uri: Arc::from("file:///one"),
                resource: Resource::new("file:///one", "one"),
            },
        ];

        let wire = "lab://upstream/alpha/bravo/file:///one";
        assert!(unique_resource_route_for_wire_uri(&routes, wire).is_none());
        let reversed = routes.iter().cloned().rev().collect::<Vec<_>>();
        assert!(unique_resource_route_for_wire_uri(&reversed, wire).is_none());
        assert_eq!(
            unique_resource_route_for_wire_uri(&routes[..1], wire)
                .unwrap()
                .native_uri
                .as_ref(),
            "bravo/file:///one"
        );
    }
}
