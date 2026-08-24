//! Immutable publication of the pool's routable tool and regular-resource projections.
//!
//! The snapshots deliberately exclude prompts, templates, UI resources, skills,
//! OAuth subjects, and unrelated capability health. Those
//! concerns have separate lifecycles and must not perturb tool generations.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rmcp::model::Resource;
use tokio::sync::{RwLockReadGuard, RwLockWriteGuard};

use crate::upstream::types::{UpstreamEntry, UpstreamTool};

use super::UpstreamPool;
use super::tools::MAX_UPSTREAM_TOOLS;

static NEXT_TOOL_CATALOG_GENERATION: AtomicU64 = AtomicU64::new(1);
static NEXT_RESOURCE_CATALOG_GENERATION: AtomicU64 = AtomicU64::new(1);
const MAX_RESOURCE_CATALOG_BYTES: usize = 8 * 1024 * 1024;
const MAX_RESOURCE_ROW_BYTES: usize = 1024 * 1024;

pub(super) fn is_ui_resource_uri(uri: &str) -> bool {
    uri.get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("ui://"))
}

fn next_generation() -> ToolCatalogGeneration {
    ToolCatalogGeneration(
        NEXT_TOOL_CATALOG_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("tool catalog generation exhausted"),
    )
}

fn next_resource_generation() -> ResourceCatalogGeneration {
    ResourceCatalogGeneration(
        NEXT_RESOURCE_CATALOG_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("resource catalog generation exhausted"),
    )
}

/// Opaque process-local identity of one published tool catalog revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolCatalogGeneration(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceCatalogGeneration(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceCatalogPublicationError {
    TooManyRoutes,
    TooManyBytes,
    InvalidResource,
    DuplicateResource,
}

#[derive(Debug, Clone)]
pub struct PublishedResourceRoute {
    pub upstream_name: Arc<str>,
    pub native_uri: Arc<str>,
    pub resource: Resource,
}

/// Immutable observational catalog of generic non-OAuth upstream resources.
///
/// UI resources, templates, OAuth/subject-scoped, synthetic, local, Skills, and
/// application resources are excluded. This is neither read authority,
/// authorization evidence, nor a dispatch grant.
#[derive(Debug, Clone)]
pub struct PublishedResourceCatalogSnapshot {
    generation: ResourceCatalogGeneration,
    routes: Arc<[PublishedResourceRoute]>,
}

impl PublishedResourceCatalogSnapshot {
    #[must_use]
    pub fn generation(&self) -> ResourceCatalogGeneration {
        self.generation
    }
    #[must_use]
    pub fn routes(&self) -> &[PublishedResourceRoute] {
        &self.routes
    }
}

/// Fail-closed reason that no routable tool snapshot can be published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolCatalogPublicationError {
    TooManyRoutes,
    InvalidTool,
}

/// One immutable, routable tool and its owning upstream.
#[derive(Debug, Clone)]
pub struct PublishedToolRoute {
    pub upstream_name: Arc<str>,
    pub tool_name: Arc<str>,
    pub tool: UpstreamTool,
}

/// A coherent point-in-time projection of every routable pool tool.
#[derive(Debug, Clone)]
pub struct PublishedToolCatalogSnapshot {
    generation: ToolCatalogGeneration,
    routes: Arc<[PublishedToolRoute]>,
}

impl PublishedToolCatalogSnapshot {
    #[must_use]
    pub fn generation(&self) -> ToolCatalogGeneration {
        self.generation
    }

    #[must_use]
    pub fn routes(&self) -> &[PublishedToolRoute] {
        &self.routes
    }
}

pub(super) struct CatalogState {
    entries: HashMap<String, UpstreamEntry>,
    incarnations: HashMap<String, super::incarnation::ConnectionIncarnation>,
    published: Result<Arc<PublishedToolCatalogSnapshot>, ToolCatalogPublicationError>,
    determinant: ProjectionDeterminant,
    resource_sources: HashMap<String, ResourceSourceState>,
    published_resources:
        Result<Arc<PublishedResourceCatalogSnapshot>, ResourceCatalogPublicationError>,
    resource_determinant: ResourceProjectionDeterminant,
}

#[derive(PartialEq, Eq)]
enum ResourceProjectionDeterminant {
    Ready(Vec<ResourceRouteDeterminant>),
    Failed(ResourceCatalogPublicationError),
}

#[derive(Clone, PartialEq, Eq)]
struct ResourceRouteDeterminant {
    upstream_name: String,
    native_uri: String,
    incarnation: super::incarnation::ConnectionIncarnation,
    resource: serde_json::Value,
}

struct ResourceSource {
    incarnation: super::incarnation::ConnectionIncarnation,
    resources: Arc<[Resource]>,
    retained_bytes: usize,
}
enum ResourceSourceState {
    Ready(ResourceSource),
    Failed {
        incarnation: super::incarnation::ConnectionIncarnation,
        error: ResourceCatalogPublicationError,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum ProjectionDeterminant {
    Ready(Vec<RouteDeterminant>),
    Failed(ToolCatalogPublicationError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RouteDeterminant {
    upstream_name: String,
    tool_name: String,
    tool: serde_json::Value,
    input_schema: Option<serde_json::Value>,
    output_schema: Option<serde_json::Value>,
    destructive: bool,
}

impl CatalogState {
    pub(super) fn bind_incarnation(
        &mut self,
        upstream: &str,
        incarnation: super::incarnation::ConnectionIncarnation,
    ) {
        self.incarnations.insert(upstream.to_string(), incarnation);
        self.resource_sources.remove(upstream);
    }

    pub(super) fn incarnation(
        &self,
        upstream: &str,
    ) -> Option<super::incarnation::ConnectionIncarnation> {
        self.incarnations.get(upstream).copied()
    }

    pub(super) fn remove_incarnation(&mut self, upstream: &str) {
        self.incarnations.remove(upstream);
        self.resource_sources.remove(upstream);
    }

    pub(super) fn clear_incarnations(&mut self) {
        self.incarnations.clear();
        self.resource_sources.clear();
    }

    pub(super) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            incarnations: HashMap::new(),
            published: Ok(Arc::new(PublishedToolCatalogSnapshot {
                generation: next_generation(),
                routes: Arc::from([]),
            })),
            determinant: ProjectionDeterminant::Ready(Vec::new()),
            resource_sources: HashMap::new(),
            published_resources: Ok(Arc::new(PublishedResourceCatalogSnapshot {
                generation: next_resource_generation(),
                routes: Arc::from([]),
            })),
            resource_determinant: ResourceProjectionDeterminant::Ready(Vec::new()),
        }
    }

    pub(super) fn set_resource_source(
        &mut self,
        upstream: &str,
        incarnation: super::incarnation::ConnectionIncarnation,
        resources: &[Resource],
    ) {
        let resources = resources
            .iter()
            .filter(|resource| !is_ui_resource_uri(&resource.uri))
            .collect::<Vec<_>>();
        let existing_retained = self
            .resource_sources
            .iter()
            .filter(|(name, _)| name.as_str() != upstream)
            .filter_map(|(_, source)| match source {
                ResourceSourceState::Ready(source) => Some(source.retained_bytes),
                ResourceSourceState::Failed { .. } => None,
            })
            .try_fold(0usize, usize::checked_add);
        let mut source_uris = resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>();
        source_uris.sort_unstable();
        let structural = if source_uris
            .iter()
            .any(|uri| uri.is_empty() || uri.chars().any(char::is_control))
        {
            Err(ResourceCatalogPublicationError::InvalidResource)
        } else if source_uris.windows(2).any(|pair| pair[0] == pair[1]) {
            Err(ResourceCatalogPublicationError::DuplicateResource)
        } else {
            Ok(())
        };
        let candidate = resources.iter().try_fold(0usize, |total, resource| {
            let bytes = serde_json::to_vec(resource)
                .map_err(|_| ResourceCatalogPublicationError::InvalidResource)?
                .len();
            if bytes > MAX_RESOURCE_ROW_BYTES {
                return Err(ResourceCatalogPublicationError::TooManyBytes);
            }
            total
                .checked_add(bytes)
                .ok_or(ResourceCatalogPublicationError::TooManyBytes)
        });
        let retained_bytes = structural.and(candidate).and_then(|candidate| {
            existing_retained
                .and_then(|existing| existing.checked_add(candidate))
                .filter(|total| *total <= MAX_RESOURCE_CATALOG_BYTES)
                .map(|_| candidate)
                .ok_or(ResourceCatalogPublicationError::TooManyBytes)
        });
        let source = if let Ok(retained_bytes) = retained_bytes {
            ResourceSourceState::Ready(ResourceSource {
                incarnation,
                resources: resources.into_iter().cloned().collect::<Vec<_>>().into(),
                retained_bytes,
            })
        } else {
            ResourceSourceState::Failed {
                incarnation,
                error: retained_bytes.expect_err("failed source"),
            }
        };
        self.resource_sources.insert(upstream.to_string(), source);
    }

    pub(super) fn remove_resource_source(&mut self, upstream: &str) {
        self.resource_sources.remove(upstream);
    }

    fn resource_projection(
        &self,
    ) -> Result<
        (Vec<ResourceRouteDeterminant>, Arc<[PublishedResourceRoute]>),
        ResourceCatalogPublicationError,
    > {
        let mut upstreams = self.entries.iter().collect::<Vec<_>>();
        upstreams.sort_unstable_by_key(|(name, _)| name.as_str());
        let mut determinant = Vec::new();
        let mut routes = Vec::new();
        let mut total_bytes = 0usize;
        let mut total_retained_bytes = 0usize;
        let mut sources = self.resource_sources.iter().collect::<Vec<_>>();
        sources.sort_unstable_by_key(|(upstream, _)| upstream.as_str());
        for (upstream, source) in sources {
            let entry = self
                .entries
                .get(upstream)
                .ok_or(ResourceCatalogPublicationError::InvalidResource)?;
            let source = match source {
                ResourceSourceState::Ready(source) => source,
                ResourceSourceState::Failed { incarnation, error } => {
                    if self.incarnation(upstream) != Some(*incarnation) {
                        return Err(ResourceCatalogPublicationError::InvalidResource);
                    }
                    if entry.proxy_resources && entry.resource_health.is_routable() {
                        return Err(*error);
                    }
                    continue;
                }
            };
            if entry.name.as_ref() != upstream {
                return Err(ResourceCatalogPublicationError::InvalidResource);
            }
            if self.incarnation(upstream) != Some(source.incarnation)
                || source
                    .resources
                    .iter()
                    .map(|resource| resource.uri.as_str())
                    .ne(entry
                        .resource_uris
                        .iter()
                        .map(String::as_str)
                        .filter(|uri| !is_ui_resource_uri(uri)))
            {
                return Err(ResourceCatalogPublicationError::InvalidResource);
            }
            total_retained_bytes = total_retained_bytes
                .checked_add(source.retained_bytes)
                .ok_or(ResourceCatalogPublicationError::TooManyBytes)?;
            if total_retained_bytes > MAX_RESOURCE_CATALOG_BYTES {
                return Err(ResourceCatalogPublicationError::TooManyBytes);
            }
        }
        for (upstream, entry) in upstreams {
            let Some(ResourceSourceState::Ready(source)) = self.resource_sources.get(upstream)
            else {
                continue;
            };
            let Some(incarnation) = self.incarnation(upstream) else {
                continue;
            };
            if !entry.proxy_resources || !entry.resource_health.is_routable() {
                continue;
            }
            if entry.name.as_ref() != upstream {
                return Err(ResourceCatalogPublicationError::InvalidResource);
            }
            let mut resources = source
                .resources
                .iter()
                .filter(|resource| {
                    !is_ui_resource_uri(&resource.uri)
                        && entry.resource_exposure_policy.matches(&resource.uri)
                })
                .collect::<Vec<_>>();
            resources.sort_unstable_by_key(|resource| resource.uri.as_str());
            let mut previous = None;
            for resource in resources {
                if routes.len() == super::tools::MAX_UPSTREAM_RESOURCES {
                    return Err(ResourceCatalogPublicationError::TooManyRoutes);
                }
                let uri = resource.uri.as_str();
                if uri.is_empty() || uri.chars().any(char::is_control) {
                    return Err(ResourceCatalogPublicationError::InvalidResource);
                }
                if previous == Some(uri) {
                    return Err(ResourceCatalogPublicationError::DuplicateResource);
                }
                previous = Some(uri);
                let bytes = serde_json::to_vec(resource)
                    .map_err(|_| ResourceCatalogPublicationError::InvalidResource)?
                    .len();
                total_bytes = total_bytes
                    .checked_add(bytes)
                    .ok_or(ResourceCatalogPublicationError::TooManyBytes)?;
                if total_bytes > MAX_RESOURCE_CATALOG_BYTES {
                    return Err(ResourceCatalogPublicationError::TooManyBytes);
                }
                let value = serde_json::to_value(resource)
                    .map_err(|_| ResourceCatalogPublicationError::InvalidResource)?;
                determinant.push(ResourceRouteDeterminant {
                    upstream_name: upstream.clone(),
                    native_uri: uri.to_string(),
                    incarnation,
                    resource: value,
                });
                routes.push(PublishedResourceRoute {
                    upstream_name: Arc::from(upstream.as_str()),
                    native_uri: Arc::from(uri),
                    resource: resource.clone(),
                });
            }
        }
        Ok((determinant, Arc::from(routes)))
    }

    fn projection(
        &self,
    ) -> Result<(Vec<RouteDeterminant>, Arc<[PublishedToolRoute]>), ToolCatalogPublicationError>
    {
        let mut upstreams = self.entries.iter().collect::<Vec<_>>();
        upstreams.sort_unstable_by_key(|(name, _)| name.as_str());
        let mut determinant = Vec::new();
        let mut routes = Vec::new();

        for (upstream, entry) in upstreams {
            if !entry.tool_health.is_routable() {
                continue;
            }
            if entry.name.as_ref() != upstream {
                return Err(ToolCatalogPublicationError::InvalidTool);
            }
            let mut tools = entry
                .tools
                .iter()
                .filter(|(name, _)| entry.exposure_policy.matches(name))
                .collect::<Vec<_>>();
            tools.sort_unstable_by_key(|(name, _)| name.as_str());
            for (name, source_tool) in tools {
                if routes.len() == MAX_UPSTREAM_TOOLS {
                    return Err(ToolCatalogPublicationError::TooManyRoutes);
                }
                if source_tool.tool.name.as_ref() != name
                    || source_tool.upstream_name.as_ref() != upstream
                {
                    return Err(ToolCatalogPublicationError::InvalidTool);
                }
                let tool = source_tool.clone();
                let tool_value = serde_json::to_value(&tool.tool)
                    .map_err(|_| ToolCatalogPublicationError::InvalidTool)?;
                determinant.push(RouteDeterminant {
                    upstream_name: upstream.clone(),
                    tool_name: name.clone(),
                    tool: tool_value,
                    input_schema: tool.input_schema.clone(),
                    output_schema: tool.output_schema.clone(),
                    destructive: tool.destructive,
                });
                routes.push(PublishedToolRoute {
                    upstream_name: Arc::from(upstream.as_str()),
                    tool_name: Arc::from(name.as_str()),
                    tool,
                });
            }
        }
        Ok((determinant, Arc::from(routes)))
    }

    fn publish_if_changed(&mut self) {
        self.resource_sources
            .retain(|upstream, _| self.entries.contains_key(upstream));
        let projection = self.projection();
        let determinant = match &projection {
            Ok((determinant, _)) => ProjectionDeterminant::Ready(determinant.clone()),
            Err(error) => ProjectionDeterminant::Failed(*error),
        };
        if determinant != self.determinant {
            self.determinant = determinant;
            self.published = projection.map(|(_, routes)| {
                Arc::new(PublishedToolCatalogSnapshot {
                    generation: next_generation(),
                    routes,
                })
            });
        }
        let resource_projection = self.resource_projection();
        let resource_determinant = match &resource_projection {
            Ok((determinant, _)) => ResourceProjectionDeterminant::Ready(determinant.clone()),
            Err(error) => ResourceProjectionDeterminant::Failed(*error),
        };
        if resource_determinant != self.resource_determinant {
            self.resource_determinant = resource_determinant;
            self.published_resources = resource_projection.map(|(_, routes)| {
                Arc::new(PublishedResourceCatalogSnapshot {
                    generation: next_resource_generation(),
                    routes,
                })
            });
        }
    }
}

impl Deref for CatalogState {
    type Target = HashMap<String, UpstreamEntry>;

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl DerefMut for CatalogState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entries
    }
}

pub(super) struct CatalogWriteGuard<'a>(RwLockWriteGuard<'a, CatalogState>);

impl Deref for CatalogWriteGuard<'_> {
    type Target = CatalogState;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for CatalogWriteGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for CatalogWriteGuard<'_> {
    fn drop(&mut self) {
        self.0.publish_if_changed();
    }
}

impl UpstreamPool {
    pub(super) async fn catalog_write(&self) -> CatalogWriteGuard<'_> {
        CatalogWriteGuard(self.catalog.write().await)
    }

    /// Observe generation and routes from the same locked catalog state.
    pub async fn published_tool_catalog(
        &self,
    ) -> Result<Arc<PublishedToolCatalogSnapshot>, ToolCatalogPublicationError> {
        let state: RwLockReadGuard<'_, CatalogState> = self.catalog.read().await;
        state.published.clone()
    }

    pub async fn published_resource_catalog(
        &self,
    ) -> Result<Arc<PublishedResourceCatalogSnapshot>, ResourceCatalogPublicationError> {
        self.catalog.read().await.published_resources.clone()
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use rmcp::model::{Resource, Tool};

    use super::*;
    use crate::upstream::pool::entries::healthy_in_process_entry;
    use crate::upstream::types::ToolExposurePolicy;

    fn install_resource_source(state: &mut CatalogState, upstream: &str, resources: Vec<Resource>) {
        let incarnation =
            super::super::incarnation::next_connection_incarnation().expect("identity");
        let mut entry = healthy_in_process_entry(Arc::from(upstream), HashMap::new());
        entry.resource_count = resources.len();
        entry.resource_uris = resources
            .iter()
            .map(|resource| resource.uri.clone())
            .collect();
        state.entries.insert(upstream.to_string(), entry);
        state.bind_incarnation(upstream, incarnation);
        state.set_resource_source(upstream, incarnation, &resources);
    }

    #[test]
    fn resource_projection_is_deterministic_bounded_and_immutable() {
        let mut state = CatalogState::new();
        install_resource_source(
            &mut state,
            "zeta",
            vec![
                Resource::new("file:///b", "b"),
                Resource::new("UI://widget", "widget"),
                Resource::new("file:///a", "a"),
            ],
        );
        install_resource_source(&mut state, "alpha", vec![Resource::new("https://a", "a")]);
        state.publish_if_changed();
        let first = state.published_resources.clone().expect("published");
        assert_eq!(
            first
                .routes()
                .iter()
                .map(|route| (route.upstream_name.as_ref(), route.native_uri.as_ref()))
                .collect::<Vec<_>>(),
            vec![
                ("alpha", "https://a"),
                ("zeta", "file:///a"),
                ("zeta", "file:///b")
            ]
        );

        state.entries.get_mut("zeta").expect("zeta").resource_health =
            crate::upstream::types::UpstreamHealth::Unhealthy {
                consecutive_failures: 3,
            };
        state.publish_if_changed();
        let narrowed = state.published_resources.clone().expect("narrowed");
        assert_ne!(first.generation(), narrowed.generation());
        assert_eq!(first.routes().len(), 3, "old snapshot remains immutable");
        assert_eq!(narrowed.routes().len(), 1);

        state.entries.get_mut("zeta").expect("zeta").resource_health =
            crate::upstream::types::UpstreamHealth::Healthy;
        state.publish_if_changed();
        let restored = state.published_resources.clone().expect("restored");
        assert_eq!(restored.routes().len(), 3);
        assert_ne!(
            restored.generation(),
            first.generation(),
            "ABA receives fresh identity"
        );
    }

    #[test]
    fn resource_projection_rejects_duplicate_and_recovers_without_losing_other_sources() {
        let mut state = CatalogState::new();
        install_resource_source(&mut state, "alpha", vec![Resource::new("file:///ok", "ok")]);
        install_resource_source(
            &mut state,
            "beta",
            vec![
                Resource::new("file:///dup", "one"),
                Resource::new("file:///dup", "two"),
            ],
        );
        state.publish_if_changed();
        assert!(matches!(
            state.published_resources,
            Err(ResourceCatalogPublicationError::DuplicateResource)
        ));
        let incarnation = state.incarnation("beta").expect("beta identity");
        state.set_resource_source(
            "beta",
            incarnation,
            &[Resource::new("file:///fixed", "fixed")],
        );
        state.entries.get_mut("beta").expect("beta").resource_count = 1;
        state.entries.get_mut("beta").expect("beta").resource_uris = vec!["file:///fixed".into()];
        state.publish_if_changed();
        let recovered = state.published_resources.clone().expect("recovered");
        assert_eq!(
            recovered
                .routes()
                .iter()
                .map(|route| route.native_uri.as_ref())
                .collect::<Vec<_>>(),
            vec!["file:///ok", "file:///fixed"]
        );
    }

    #[test]
    fn hidden_multi_upstream_source_bytes_fail_closed_globally() {
        let mut state = CatalogState::new();
        for upstream in ["alpha", "beta", "gamma"] {
            let resources = (0..3)
                .map(|index| {
                    Resource::new(format!("file:///{upstream}/{index}"), "x".repeat(950_000))
                })
                .collect();
            install_resource_source(&mut state, upstream, resources);
            state
                .entries
                .get_mut(upstream)
                .expect("entry")
                .resource_exposure_policy = ToolExposurePolicy::AllowList(Vec::new());
        }
        state.publish_if_changed();
        assert!(matches!(
            state.published_resources,
            Err(ResourceCatalogPublicationError::TooManyBytes)
        ));
    }

    #[test]
    fn resource_generation_tracks_source_identity_and_policy_without_relisting() {
        let mut state = CatalogState::new();
        let rows = vec![Resource::new("file:///same", "same")];
        install_resource_source(&mut state, "alpha", rows.clone());
        state.publish_if_changed();
        let first = state.published_resources.clone().expect("first");
        let incarnation = state.incarnation("alpha").expect("identity");
        state.set_resource_source("alpha", incarnation, &rows);
        state.publish_if_changed();
        let identical = state.published_resources.clone().expect("identical");
        assert!(Arc::ptr_eq(&first, &identical));

        state
            .entries
            .get_mut("alpha")
            .expect("alpha")
            .proxy_resources = false;
        state.publish_if_changed();
        assert!(
            state
                .published_resources
                .clone()
                .expect("hidden")
                .routes()
                .is_empty()
        );
        state
            .entries
            .get_mut("alpha")
            .expect("alpha")
            .proxy_resources = true;
        state.publish_if_changed();
        assert_eq!(
            state
                .published_resources
                .clone()
                .expect("restored")
                .routes()
                .len(),
            1
        );
        state
            .entries
            .get_mut("alpha")
            .expect("alpha")
            .resource_exposure_policy = ToolExposurePolicy::AllowList(Vec::new());
        state.publish_if_changed();
        assert!(
            state
                .published_resources
                .clone()
                .expect("allowlist hidden")
                .routes()
                .is_empty()
        );
        state
            .entries
            .get_mut("alpha")
            .expect("alpha")
            .resource_exposure_policy = ToolExposurePolicy::All;
        state.publish_if_changed();
        assert_eq!(
            state
                .published_resources
                .clone()
                .expect("allowlist restored")
                .routes()
                .len(),
            1
        );

        let replacement =
            super::super::incarnation::next_connection_incarnation().expect("replacement identity");
        state.bind_incarnation("alpha", replacement);
        state.publish_if_changed();
        let empty = state
            .published_resources
            .clone()
            .expect("old source cleared");
        assert!(empty.routes().is_empty());
        state.set_resource_source("alpha", replacement, &rows);
        state.publish_if_changed();
        let rebound = state.published_resources.clone().expect("rebound");
        assert_eq!(rebound.routes().len(), 1);
        assert_ne!(first.generation(), rebound.generation());
        state.entries.remove("alpha");
        state.remove_incarnation("alpha");
        state.publish_if_changed();
        assert!(
            state
                .published_resources
                .clone()
                .expect("removed")
                .routes()
                .is_empty()
        );
    }

    #[test]
    fn resource_route_and_structure_failures_recover() {
        let mut state = CatalogState::new();
        let too_many = (0..=super::super::tools::MAX_UPSTREAM_RESOURCES)
            .map(|index| Resource::new(format!("file:///{index}"), "row"))
            .collect::<Vec<_>>();
        install_resource_source(&mut state, "alpha", too_many);
        state.publish_if_changed();
        assert!(matches!(
            state.published_resources,
            Err(ResourceCatalogPublicationError::TooManyRoutes)
        ));
        let incarnation = state.incarnation("alpha").expect("identity");
        state.set_resource_source("alpha", incarnation, &[Resource::new("bad\nuri", "bad")]);
        state
            .entries
            .get_mut("alpha")
            .expect("alpha")
            .resource_count = 1;
        state.entries.get_mut("alpha").expect("alpha").resource_uris = vec!["bad\nuri".into()];
        state.publish_if_changed();
        assert!(matches!(
            state.published_resources,
            Err(ResourceCatalogPublicationError::InvalidResource)
        ));
        state.set_resource_source(
            "alpha",
            incarnation,
            &[Resource::new("file:///fixed", "fixed")],
        );
        state.entries.get_mut("alpha").expect("alpha").resource_uris = vec!["file:///fixed".into()];
        state.publish_if_changed();
        assert_eq!(
            state
                .published_resources
                .clone()
                .expect("recovered")
                .routes()
                .len(),
            1
        );
    }

    fn resource_with_serialized_size(uri: &str, target: usize) -> Resource {
        let base = Resource::new(uri, "row").with_description("");
        let base_len = serde_json::to_vec(&base).expect("serialize base").len();
        assert!(base_len <= target);
        let resource = Resource::new(uri, "row").with_description("x".repeat(target - base_len));
        assert_eq!(
            serde_json::to_vec(&resource).expect("serialize row").len(),
            target
        );
        resource
    }

    #[test]
    fn resource_bounds_accept_exact_limits_and_reject_next_unit() {
        let mut state = CatalogState::new();
        let exact_routes = (0..super::super::tools::MAX_UPSTREAM_RESOURCES)
            .map(|index| Resource::new(format!("file:///{index}"), "row"))
            .collect::<Vec<_>>();
        install_resource_source(&mut state, "alpha", exact_routes);
        state.publish_if_changed();
        assert_eq!(
            state
                .published_resources
                .clone()
                .expect("exact route cap")
                .routes()
                .len(),
            super::super::tools::MAX_UPSTREAM_RESOURCES
        );

        let incarnation = state.incarnation("alpha").expect("identity");
        let exact_bytes = (0..8)
            .map(|index| {
                resource_with_serialized_size(
                    &format!("file:///exact/{index}"),
                    MAX_RESOURCE_ROW_BYTES,
                )
            })
            .collect::<Vec<_>>();
        state.set_resource_source("alpha", incarnation, &exact_bytes);
        state
            .entries
            .get_mut("alpha")
            .expect("alpha")
            .resource_count = exact_bytes.len();
        state.entries.get_mut("alpha").expect("alpha").resource_uris =
            exact_bytes.iter().map(|row| row.uri.clone()).collect();
        state.publish_if_changed();
        assert!(
            state.published_resources.is_ok(),
            "exact aggregate and row byte caps pass"
        );

        let oversized = vec![resource_with_serialized_size(
            "file:///oversized",
            MAX_RESOURCE_ROW_BYTES + 1,
        )];
        state.set_resource_source("alpha", incarnation, &oversized);
        state
            .entries
            .get_mut("alpha")
            .expect("alpha")
            .resource_count = 1;
        state.entries.get_mut("alpha").expect("alpha").resource_uris =
            vec!["file:///oversized".into()];
        state.publish_if_changed();
        assert!(matches!(
            state.published_resources,
            Err(ResourceCatalogPublicationError::TooManyBytes)
        ));
    }

    #[test]
    fn unrelated_no_source_mutation_preserves_resource_arc_and_generation() {
        let mut state = CatalogState::new();
        install_resource_source(
            &mut state,
            "alpha",
            vec![Resource::new("file:///alpha", "alpha")],
        );
        state.publish_if_changed();
        let first = state.published_resources.clone().expect("first");
        let mut unrelated = healthy_in_process_entry(Arc::from("tools-only"), HashMap::new());
        unrelated.prompt_count = 99;
        state.entries.insert("tools-only".into(), unrelated);
        state.publish_if_changed();
        let second = state.published_resources.clone().expect("second");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.generation(), second.generation());
    }
    fn entry(upstream: &str, tool_name: &str) -> UpstreamEntry {
        let upstream_name: Arc<str> = Arc::from(upstream);
        let tool = Tool::new(
            tool_name.to_string(),
            "test tool",
            Arc::new(serde_json::Map::new()),
        );
        let upstream_tool = UpstreamTool {
            input_schema: Some(serde_json::Value::Object((*tool.input_schema).clone())),
            output_schema: None,
            destructive: false,
            upstream_name: Arc::clone(&upstream_name),
            tool,
        };
        healthy_in_process_entry(
            upstream_name,
            HashMap::from([(tool_name.to_string(), upstream_tool)]),
        )
    }

    fn route_names(snapshot: &PublishedToolCatalogSnapshot) -> Vec<(&str, &str)> {
        snapshot
            .routes()
            .iter()
            .map(|route| (route.upstream_name.as_ref(), route.tool_name.as_ref()))
            .collect()
    }

    async fn snapshot(pool: &UpstreamPool) -> Arc<PublishedToolCatalogSnapshot> {
        pool.published_tool_catalog()
            .await
            .expect("published catalog")
    }

    #[tokio::test]
    async fn add_remove_and_identical_replacement_publish_only_semantic_changes() {
        let pool = UpstreamPool::new();
        let empty = snapshot(&pool).await;

        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let added = snapshot(&pool).await;
        assert_ne!(added.generation(), empty.generation());
        assert_eq!(route_names(&added), [("alpha", "read")]);

        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let identical = snapshot(&pool).await;
        assert_eq!(identical.generation(), added.generation());
        assert!(Arc::ptr_eq(&identical, &added));

        pool.catalog_write().await.remove("alpha");
        let removed = snapshot(&pool).await;
        assert_ne!(removed.generation(), added.generation());
        assert!(removed.routes().is_empty());
    }

    #[tokio::test]
    async fn aba_content_receives_a_fresh_generation() {
        let pool = UpstreamPool::new();
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "a"));
        let first_a = snapshot(&pool).await;
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "b"));
        let b = snapshot(&pool).await;
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "a"));
        let second_a = snapshot(&pool).await;

        assert_ne!(first_a.generation(), b.generation());
        assert_ne!(first_a.generation(), second_a.generation());
        assert_eq!(route_names(&first_a), route_names(&second_a));
    }

    #[tokio::test]
    async fn clones_share_publication_but_distinct_pools_never_share_generation() {
        let pool = UpstreamPool::new();
        let clone = pool.clone();
        let other = UpstreamPool::new();
        assert_ne!(
            snapshot(&pool).await.generation(),
            snapshot(&other).await.generation()
        );

        clone
            .catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let original_snapshot = snapshot(&pool).await;
        let clone_snapshot = snapshot(&clone).await;
        assert!(Arc::ptr_eq(&original_snapshot, &clone_snapshot));
        assert_ne!(
            original_snapshot.generation(),
            snapshot(&other).await.generation()
        );
    }

    #[tokio::test]
    async fn reader_cannot_observe_new_routes_with_the_old_generation() {
        let pool = UpstreamPool::new();
        let old = snapshot(&pool).await;
        let mut writer = pool.catalog_write().await;
        writer.insert("alpha".into(), entry("alpha", "read"));

        let reader_pool = pool.clone();
        let reader = tokio::spawn(async move { reader_pool.published_tool_catalog().await });
        tokio::task::yield_now().await;
        assert!(
            !reader.is_finished(),
            "reader must wait for publication lock"
        );

        drop(writer);
        let observed = reader
            .await
            .expect("reader task")
            .expect("published catalog");
        assert_ne!(observed.generation(), old.generation());
        assert_eq!(route_names(&observed), [("alpha", "read")]);
    }

    #[tokio::test]
    async fn exposure_transitions_remove_and_restore_routes() {
        let pool = UpstreamPool::new();
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let visible = snapshot(&pool).await;
        pool.catalog_write()
            .await
            .get_mut("alpha")
            .expect("entry")
            .exposure_policy = ToolExposurePolicy::AllowList(Vec::new());
        let hidden = snapshot(&pool).await;
        assert!(hidden.routes().is_empty());
        assert_ne!(hidden.generation(), visible.generation());

        pool.catalog_write()
            .await
            .get_mut("alpha")
            .expect("entry")
            .exposure_policy = ToolExposurePolicy::All;
        let restored = snapshot(&pool).await;
        assert_eq!(route_names(&restored), [("alpha", "read")]);
        assert_ne!(restored.generation(), visible.generation());
    }

    #[tokio::test]
    async fn only_tool_routability_threshold_changes_generation() {
        let pool = UpstreamPool::new();
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let healthy = snapshot(&pool).await;
        pool.record_failure("alpha", "one").await;
        pool.record_failure("alpha", "two").await;
        assert_eq!(snapshot(&pool).await.generation(), healthy.generation());

        pool.record_failure("alpha", "three").await;
        let open = snapshot(&pool).await;
        assert!(open.routes().is_empty());
        assert_ne!(open.generation(), healthy.generation());
        pool.record_success("alpha").await;
        assert_eq!(
            route_names(snapshot(&pool).await.as_ref()),
            [("alpha", "read")]
        );
    }

    #[tokio::test]
    async fn schema_and_destructive_metadata_changes_advance_generation() {
        let pool = UpstreamPool::new();
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let original = snapshot(&pool).await;
        {
            let mut catalog = pool.catalog_write().await;
            let tool = catalog
                .get_mut("alpha")
                .and_then(|entry| entry.tools.get_mut("read"))
                .expect("tool");
            tool.output_schema = Some(serde_json::json!({"type": "string"}));
            tool.destructive = true;
        }
        assert_ne!(snapshot(&pool).await.generation(), original.generation());
    }

    #[tokio::test]
    async fn mismatched_route_identity_fails_the_whole_publication_closed() {
        let entry_name_pool = UpstreamPool::new();
        let mut wrong_entry_name = entry("wrong", "read");
        wrong_entry_name
            .tools
            .get_mut("read")
            .expect("tool")
            .upstream_name = Arc::from("canonical");
        entry_name_pool
            .catalog_write()
            .await
            .insert("canonical".into(), wrong_entry_name);

        let tool_name_pool = UpstreamPool::new();
        let mut wrong_tool_name = entry("alpha", "read");
        let tool = wrong_tool_name.tools.remove("read").expect("tool");
        wrong_tool_name.tools.insert("alias".into(), tool);
        tool_name_pool
            .catalog_write()
            .await
            .insert("alpha".into(), wrong_tool_name);

        let owner_pool = UpstreamPool::new();
        let mut wrong_owner = entry("alpha", "read");
        wrong_owner
            .tools
            .get_mut("read")
            .expect("tool")
            .upstream_name = Arc::from("wrong");
        owner_pool
            .catalog_write()
            .await
            .insert("alpha".into(), wrong_owner);

        for pool in [&entry_name_pool, &tool_name_pool, &owner_pool] {
            assert!(matches!(
                pool.published_tool_catalog().await,
                Err(ToolCatalogPublicationError::InvalidTool)
            ));
        }
    }

    #[tokio::test]
    async fn non_tool_metadata_does_not_advance_generation() {
        let pool = UpstreamPool::new();
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let original = snapshot(&pool).await;
        {
            let mut catalog = pool.catalog_write().await;
            let entry = catalog.get_mut("alpha").expect("entry");
            entry.prompt_count = 7;
            entry.resource_count = 9;
            entry.skill_names.push("unrelated".into());
        }
        let unchanged = snapshot(&pool).await;
        assert_eq!(unchanged.generation(), original.generation());
        assert!(Arc::ptr_eq(&unchanged, &original));
    }

    #[tokio::test]
    async fn aggregate_overflow_fails_closed_and_recovery_gets_new_generation() {
        let pool = UpstreamPool::new();
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let before = snapshot(&pool).await;
        {
            let mut catalog = pool.catalog_write().await;
            let entry = catalog.get_mut("alpha").expect("entry");
            let template = entry.tools.get("read").expect("tool").clone();
            for index in 0..MAX_UPSTREAM_TOOLS {
                let name = format!("extra-{index}");
                let mut tool = template.clone();
                tool.tool.name = name.clone().into();
                entry.tools.insert(name, tool);
            }
        }
        assert!(matches!(
            pool.published_tool_catalog().await,
            Err(ToolCatalogPublicationError::TooManyRoutes)
        ));
        pool.catalog_write()
            .await
            .get_mut("alpha")
            .expect("entry")
            .tools
            .retain(|name, _| name == "read");
        let recovered = snapshot(&pool).await;
        assert_ne!(recovered.generation(), before.generation());
    }
}
