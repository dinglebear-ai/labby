//! Immutable publication of the pool's routable tool, regular-resource,
//! regular-resource-template, and regular-prompt projections.
//!
//! The snapshots deliberately exclude UI resources/templates, skills, OAuth
//! subjects, local and synthetic rows, and unrelated capability health.
//! Each projection has an independent generation so one catalog family cannot
//! perturb another family's publication identity.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rmcp::model::{Prompt, Resource, ResourceTemplate};
use tokio::sync::{RwLockReadGuard, RwLockWriteGuard};

use crate::upstream::types::{UpstreamEntry, UpstreamTool};

use super::UpstreamPool;
use super::tools::MAX_UPSTREAM_TOOLS;

static NEXT_TOOL_CATALOG_GENERATION: AtomicU64 = AtomicU64::new(1);
static NEXT_RESOURCE_CATALOG_GENERATION: AtomicU64 = AtomicU64::new(1);
static NEXT_RESOURCE_TEMPLATE_CATALOG_GENERATION: AtomicU64 = AtomicU64::new(1);
static NEXT_PROMPT_CATALOG_GENERATION: AtomicU64 = AtomicU64::new(1);
const MAX_RESOURCE_CATALOG_BYTES: usize = 8 * 1024 * 1024;
const MAX_RESOURCE_ROW_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy)]
enum SourceAdmissionError {
    Invalid,
    Duplicate,
    TooManyBytes,
}

fn validate_source_rows<'a, T: serde::Serialize + 'a>(
    identifiers: impl IntoIterator<Item = &'a str>,
    rows: impl IntoIterator<Item = &'a T>,
) -> Result<usize, SourceAdmissionError> {
    let mut identifiers = identifiers.into_iter().collect::<Vec<_>>();
    identifiers.sort_unstable();
    if identifiers
        .iter()
        .any(|identifier| identifier.is_empty() || identifier.chars().any(char::is_control))
    {
        return Err(SourceAdmissionError::Invalid);
    }
    if identifiers.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(SourceAdmissionError::Duplicate);
    }
    rows.into_iter().try_fold(0usize, |total, row| {
        let bytes = serde_json::to_vec(row)
            .map_err(|_| SourceAdmissionError::Invalid)?
            .len();
        if bytes > MAX_RESOURCE_ROW_BYTES {
            return Err(SourceAdmissionError::TooManyBytes);
        }
        total
            .checked_add(bytes)
            .ok_or(SourceAdmissionError::TooManyBytes)
    })
}

fn checked_retained_bytes(
    existing: Option<usize>,
    candidate: usize,
) -> Result<usize, SourceAdmissionError> {
    existing
        .and_then(|existing| existing.checked_add(candidate))
        .filter(|total| *total <= MAX_RESOURCE_CATALOG_BYTES)
        .map(|_| candidate)
        .ok_or(SourceAdmissionError::TooManyBytes)
}

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

fn next_resource_template_generation() -> ResourceTemplateCatalogGeneration {
    ResourceTemplateCatalogGeneration(
        NEXT_RESOURCE_TEMPLATE_CATALOG_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("resource template catalog generation exhausted"),
    )
}

fn next_prompt_generation() -> PromptCatalogGeneration {
    PromptCatalogGeneration(
        NEXT_PROMPT_CATALOG_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("prompt catalog generation exhausted"),
    )
}

/// Opaque process-local identity of one published tool catalog revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolCatalogGeneration(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceCatalogGeneration(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceTemplateCatalogGeneration(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PromptCatalogGeneration(u64);

macro_rules! generation_fingerprint_bytes {
    ($($generation:ty),+ $(,)?) => {
        $(impl $generation {
            #[must_use]
            pub fn fingerprint_bytes(self) -> [u8; 8] {
                self.0.to_be_bytes()
            }
        })+
    };
}

generation_fingerprint_bytes!(
    ToolCatalogGeneration,
    ResourceCatalogGeneration,
    ResourceTemplateCatalogGeneration,
    PromptCatalogGeneration,
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceCatalogPublicationError {
    TooManyRoutes,
    TooManyBytes,
    InvalidResource,
    DuplicateResource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceTemplateCatalogPublicationError {
    TooManyRoutes,
    TooManyBytes,
    InvalidTemplate,
    DuplicateTemplate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromptCatalogPublicationError {
    TooManyRoutes,
    TooManyBytes,
    InvalidPrompt,
    DuplicatePrompt,
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

#[derive(Debug, Clone)]
pub struct PublishedResourceTemplateRoute {
    pub upstream_name: Arc<str>,
    pub native_uri_template: Arc<str>,
    pub template: ResourceTemplate,
}

/// Immutable observational catalog of regular non-OAuth ResourceTemplates.
///
/// UI templates, OAuth/subject-scoped, synthetic, local, Skills, and app
/// families are excluded. This is neither read authority nor a grant.
#[derive(Debug, Clone)]
pub struct PublishedResourceTemplateCatalogSnapshot {
    generation: ResourceTemplateCatalogGeneration,
    routes: Arc<[PublishedResourceTemplateRoute]>,
}

impl PublishedResourceTemplateCatalogSnapshot {
    #[must_use]
    pub fn generation(&self) -> ResourceTemplateCatalogGeneration {
        self.generation
    }
    #[must_use]
    pub fn routes(&self) -> &[PublishedResourceTemplateRoute] {
        &self.routes
    }
}

#[derive(Debug, Clone)]
pub struct PublishedPromptRoute {
    pub upstream_name: Arc<str>,
    pub native_name: Arc<str>,
    pub prompt: Prompt,
}

/// Immutable observational catalog of regular non-OAuth upstream Prompts.
///
/// Built-in, OAuth/subject-scoped, synthetic, and local prompt families are
/// excluded. This is neither prompt execution authority nor a grant.
#[derive(Debug, Clone)]
pub struct PublishedPromptCatalogSnapshot {
    generation: PromptCatalogGeneration,
    routes: Arc<[PublishedPromptRoute]>,
}

impl PublishedPromptCatalogSnapshot {
    #[must_use]
    pub fn generation(&self) -> PromptCatalogGeneration {
        self.generation
    }

    #[must_use]
    pub fn routes(&self) -> &[PublishedPromptRoute] {
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
    resource_template_sources: HashMap<String, ResourceTemplateSourceState>,
    published_resource_templates: Result<
        Arc<PublishedResourceTemplateCatalogSnapshot>,
        ResourceTemplateCatalogPublicationError,
    >,
    resource_template_determinant: ResourceTemplateProjectionDeterminant,
    prompt_sources: HashMap<String, PromptSourceState>,
    published_prompts: Result<Arc<PublishedPromptCatalogSnapshot>, PromptCatalogPublicationError>,
    prompt_determinant: PromptProjectionDeterminant,
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

#[derive(PartialEq, Eq)]
enum ResourceTemplateProjectionDeterminant {
    Ready(Vec<ResourceTemplateRouteDeterminant>),
    Failed(ResourceTemplateCatalogPublicationError),
}

#[derive(Clone, PartialEq, Eq)]
struct ResourceTemplateRouteDeterminant {
    upstream_name: String,
    native_uri_template: String,
    incarnation: super::incarnation::ConnectionIncarnation,
    template: serde_json::Value,
}

#[derive(PartialEq, Eq)]
enum PromptProjectionDeterminant {
    Ready(Vec<PromptRouteDeterminant>),
    Failed(PromptCatalogPublicationError),
}

#[derive(Clone, PartialEq, Eq)]
struct PromptRouteDeterminant {
    upstream_name: String,
    native_name: String,
    incarnation: super::incarnation::ConnectionIncarnation,
    prompt: serde_json::Value,
}

struct PromptSource {
    incarnation: super::incarnation::ConnectionIncarnation,
    prompts: Arc<[Prompt]>,
    retained_bytes: usize,
}

enum PromptSourceState {
    Ready(PromptSource),
    Failed {
        incarnation: super::incarnation::ConnectionIncarnation,
        error: PromptCatalogPublicationError,
    },
}

struct ResourceTemplateSource {
    incarnation: super::incarnation::ConnectionIncarnation,
    templates: Arc<[ResourceTemplate]>,
    retained_bytes: usize,
}
enum ResourceTemplateSourceState {
    Ready(ResourceTemplateSource),
    Failed {
        incarnation: super::incarnation::ConnectionIncarnation,
        error: ResourceTemplateCatalogPublicationError,
    },
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
    pub(super) fn contains_tool_route(
        &self,
        generation: ToolCatalogGeneration,
        upstream: &str,
        native_name: &str,
    ) -> bool {
        self.published.as_ref().is_ok_and(|snapshot| {
            snapshot.generation() == generation
                && snapshot.routes().iter().any(|route| {
                    route.upstream_name.as_ref() == upstream
                        && route.tool_name.as_ref() == native_name
                })
        })
    }

    pub(super) fn contains_resource_route(
        &self,
        generation: ResourceCatalogGeneration,
        upstream: &str,
        native_uri: &str,
    ) -> bool {
        self.published_resources.as_ref().is_ok_and(|snapshot| {
            snapshot.generation() == generation
                && snapshot.routes().iter().any(|route| {
                    route.upstream_name.as_ref() == upstream
                        && route.native_uri.as_ref() == native_uri
                })
        })
    }

    pub(super) fn contains_prompt_route(
        &self,
        generation: PromptCatalogGeneration,
        upstream: &str,
        native_name: &str,
    ) -> bool {
        self.published_prompts.as_ref().is_ok_and(|snapshot| {
            snapshot.generation() == generation
                && snapshot.routes().iter().any(|route| {
                    route.upstream_name.as_ref() == upstream
                        && route.native_name.as_ref() == native_name
                })
        })
    }
    pub(super) fn bind_incarnation(
        &mut self,
        upstream: &str,
        incarnation: super::incarnation::ConnectionIncarnation,
    ) {
        self.incarnations.insert(upstream.to_string(), incarnation);
        self.resource_sources.remove(upstream);
        self.resource_template_sources.remove(upstream);
        self.prompt_sources.remove(upstream);
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
        self.resource_template_sources.remove(upstream);
        self.prompt_sources.remove(upstream);
    }

    pub(super) fn clear_incarnations(&mut self) {
        self.incarnations.clear();
        self.resource_sources.clear();
        self.resource_template_sources.clear();
        self.prompt_sources.clear();
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
            resource_template_sources: HashMap::new(),
            published_resource_templates: Ok(Arc::new(PublishedResourceTemplateCatalogSnapshot {
                generation: next_resource_template_generation(),
                routes: Arc::from([]),
            })),
            resource_template_determinant: ResourceTemplateProjectionDeterminant::Ready(Vec::new()),
            prompt_sources: HashMap::new(),
            published_prompts: Ok(Arc::new(PublishedPromptCatalogSnapshot {
                generation: next_prompt_generation(),
                routes: Arc::from([]),
            })),
            prompt_determinant: PromptProjectionDeterminant::Ready(Vec::new()),
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
        let retained_bytes = validate_source_rows(
            resources.iter().map(|resource| resource.uri.as_str()),
            resources.iter().copied(),
        )
        .and_then(|candidate| checked_retained_bytes(existing_retained, candidate))
        .map_err(|error| match error {
            SourceAdmissionError::Invalid => ResourceCatalogPublicationError::InvalidResource,
            SourceAdmissionError::Duplicate => ResourceCatalogPublicationError::DuplicateResource,
            SourceAdmissionError::TooManyBytes => ResourceCatalogPublicationError::TooManyBytes,
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

    pub(super) fn set_prompt_source(
        &mut self,
        upstream: &str,
        incarnation: super::incarnation::ConnectionIncarnation,
        prompts: &[Prompt],
    ) {
        let existing_retained = self
            .prompt_sources
            .iter()
            .filter(|(name, _)| name.as_str() != upstream)
            .filter_map(|(_, source)| match source {
                PromptSourceState::Ready(source) => Some(source.retained_bytes),
                PromptSourceState::Failed { .. } => None,
            })
            .try_fold(0usize, usize::checked_add);
        let retained_bytes =
            validate_source_rows(prompts.iter().map(|prompt| prompt.name.as_ref()), prompts)
                .and_then(|candidate| checked_retained_bytes(existing_retained, candidate))
                .map_err(|error| match error {
                    SourceAdmissionError::Invalid => PromptCatalogPublicationError::InvalidPrompt,
                    SourceAdmissionError::Duplicate => {
                        PromptCatalogPublicationError::DuplicatePrompt
                    }
                    SourceAdmissionError::TooManyBytes => {
                        PromptCatalogPublicationError::TooManyBytes
                    }
                });
        let source = match retained_bytes {
            Ok(retained_bytes) => PromptSourceState::Ready(PromptSource {
                incarnation,
                prompts: prompts.to_vec().into(),
                retained_bytes,
            }),
            Err(error) => PromptSourceState::Failed { incarnation, error },
        };
        self.prompt_sources.insert(upstream.to_string(), source);
    }

    pub(super) fn remove_prompt_source(&mut self, upstream: &str) {
        self.prompt_sources.remove(upstream);
    }

    fn prompt_projection(
        &self,
    ) -> Result<
        (Vec<PromptRouteDeterminant>, Arc<[PublishedPromptRoute]>),
        PromptCatalogPublicationError,
    > {
        let mut sources = self.prompt_sources.iter().collect::<Vec<_>>();
        sources.sort_unstable_by_key(|(upstream, _)| upstream.as_str());
        let mut retained_bytes = 0usize;
        for (upstream, source) in &sources {
            let entry = self
                .entries
                .get(*upstream)
                .ok_or(PromptCatalogPublicationError::InvalidPrompt)?;
            let incarnation = self.incarnation(upstream);
            match source {
                PromptSourceState::Ready(source) => {
                    if incarnation != Some(source.incarnation) || entry.name.as_ref() != *upstream {
                        return Err(PromptCatalogPublicationError::InvalidPrompt);
                    }
                    retained_bytes = retained_bytes
                        .checked_add(source.retained_bytes)
                        .ok_or(PromptCatalogPublicationError::TooManyBytes)?;
                    if retained_bytes > MAX_RESOURCE_CATALOG_BYTES {
                        return Err(PromptCatalogPublicationError::TooManyBytes);
                    }
                }
                PromptSourceState::Failed {
                    incarnation: source_incarnation,
                    error,
                } => {
                    if incarnation != Some(*source_incarnation) {
                        return Err(PromptCatalogPublicationError::InvalidPrompt);
                    }
                    if entry.prompt_health.is_routable() {
                        return Err(*error);
                    }
                }
            }
        }

        let mut determinant = Vec::new();
        let mut routes = Vec::new();
        let mut published_bytes = 0usize;
        for (upstream, source) in sources {
            let PromptSourceState::Ready(source) = source else {
                continue;
            };
            let entry = &self.entries[upstream];
            if !entry.prompt_health.is_routable() {
                continue;
            }
            let mut prompts = source
                .prompts
                .iter()
                .filter(|prompt| {
                    super::entries::prompt_exposed(
                        &entry.prompt_exposure_policy,
                        upstream,
                        prompt.name.as_ref(),
                    )
                })
                .collect::<Vec<_>>();
            prompts.sort_unstable_by_key(|prompt| prompt.name.as_str());
            for prompt in prompts {
                if routes.len() == super::tools::MAX_UPSTREAM_PROMPTS {
                    return Err(PromptCatalogPublicationError::TooManyRoutes);
                }
                let bytes = serde_json::to_vec(prompt)
                    .map_err(|_| PromptCatalogPublicationError::InvalidPrompt)?
                    .len();
                published_bytes = published_bytes
                    .checked_add(bytes)
                    .ok_or(PromptCatalogPublicationError::TooManyBytes)?;
                if published_bytes > MAX_RESOURCE_CATALOG_BYTES {
                    return Err(PromptCatalogPublicationError::TooManyBytes);
                }
                let value = serde_json::to_value(prompt)
                    .map_err(|_| PromptCatalogPublicationError::InvalidPrompt)?;
                determinant.push(PromptRouteDeterminant {
                    upstream_name: upstream.clone(),
                    native_name: prompt.name.to_string(),
                    incarnation: source.incarnation,
                    prompt: value,
                });
                routes.push(PublishedPromptRoute {
                    upstream_name: Arc::from(upstream.as_str()),
                    native_name: Arc::from(prompt.name.as_str()),
                    prompt: prompt.clone(),
                });
            }
        }
        Ok((determinant, Arc::from(routes)))
    }

    pub(super) fn set_resource_template_source(
        &mut self,
        upstream: &str,
        incarnation: super::incarnation::ConnectionIncarnation,
        templates: &[ResourceTemplate],
    ) {
        let templates = templates
            .iter()
            .filter(|template| !is_ui_resource_uri(&template.uri_template))
            .collect::<Vec<_>>();
        let existing_retained = self
            .resource_template_sources
            .iter()
            .filter(|(name, _)| name.as_str() != upstream)
            .filter_map(|(_, source)| match source {
                ResourceTemplateSourceState::Ready(source) => Some(source.retained_bytes),
                ResourceTemplateSourceState::Failed { .. } => None,
            })
            .try_fold(0usize, usize::checked_add);
        let retained_bytes = validate_source_rows(
            templates
                .iter()
                .map(|template| template.uri_template.as_str()),
            templates.iter().copied(),
        )
        .and_then(|candidate| checked_retained_bytes(existing_retained, candidate))
        .map_err(|error| match error {
            SourceAdmissionError::Invalid => {
                ResourceTemplateCatalogPublicationError::InvalidTemplate
            }
            SourceAdmissionError::Duplicate => {
                ResourceTemplateCatalogPublicationError::DuplicateTemplate
            }
            SourceAdmissionError::TooManyBytes => {
                ResourceTemplateCatalogPublicationError::TooManyBytes
            }
        });
        let source = match retained_bytes {
            Ok(retained_bytes) => ResourceTemplateSourceState::Ready(ResourceTemplateSource {
                incarnation,
                templates: templates.into_iter().cloned().collect::<Vec<_>>().into(),
                retained_bytes,
            }),
            Err(error) => ResourceTemplateSourceState::Failed { incarnation, error },
        };
        self.resource_template_sources
            .insert(upstream.to_string(), source);
    }

    pub(super) fn remove_resource_template_source(&mut self, upstream: &str) {
        self.resource_template_sources.remove(upstream);
    }

    fn resource_template_projection(
        &self,
    ) -> Result<
        (
            Vec<ResourceTemplateRouteDeterminant>,
            Arc<[PublishedResourceTemplateRoute]>,
        ),
        ResourceTemplateCatalogPublicationError,
    > {
        let mut sources = self.resource_template_sources.iter().collect::<Vec<_>>();
        sources.sort_unstable_by_key(|(upstream, _)| upstream.as_str());
        let mut retained_bytes = 0usize;
        for (upstream, source) in &sources {
            let entry = self
                .entries
                .get(*upstream)
                .ok_or(ResourceTemplateCatalogPublicationError::InvalidTemplate)?;
            let incarnation = self.incarnation(upstream);
            match source {
                ResourceTemplateSourceState::Ready(source) => {
                    if incarnation != Some(source.incarnation) || entry.name.as_ref() != *upstream {
                        return Err(ResourceTemplateCatalogPublicationError::InvalidTemplate);
                    }
                    retained_bytes = retained_bytes
                        .checked_add(source.retained_bytes)
                        .ok_or(ResourceTemplateCatalogPublicationError::TooManyBytes)?;
                    if retained_bytes > MAX_RESOURCE_CATALOG_BYTES {
                        return Err(ResourceTemplateCatalogPublicationError::TooManyBytes);
                    }
                }
                ResourceTemplateSourceState::Failed {
                    incarnation: source_incarnation,
                    error,
                } => {
                    if incarnation != Some(*source_incarnation) {
                        return Err(ResourceTemplateCatalogPublicationError::InvalidTemplate);
                    }
                    if entry.proxy_resources && entry.resource_health.is_routable() {
                        return Err(*error);
                    }
                }
            }
        }

        let mut determinant = Vec::new();
        let mut routes = Vec::new();
        let mut published_bytes = 0usize;
        for (upstream, source) in sources {
            let ResourceTemplateSourceState::Ready(source) = source else {
                continue;
            };
            let entry = &self.entries[upstream];
            if !entry.proxy_resources || !entry.resource_health.is_routable() {
                continue;
            }
            let mut templates = source.templates.iter().collect::<Vec<_>>();
            templates.sort_unstable_by_key(|template| template.uri_template.as_str());
            for template in templates {
                if routes.len() == super::tools::MAX_UPSTREAM_RESOURCES {
                    return Err(ResourceTemplateCatalogPublicationError::TooManyRoutes);
                }
                let bytes = serde_json::to_vec(template)
                    .map_err(|_| ResourceTemplateCatalogPublicationError::InvalidTemplate)?
                    .len();
                published_bytes = published_bytes
                    .checked_add(bytes)
                    .ok_or(ResourceTemplateCatalogPublicationError::TooManyBytes)?;
                if published_bytes > MAX_RESOURCE_CATALOG_BYTES {
                    return Err(ResourceTemplateCatalogPublicationError::TooManyBytes);
                }
                let value = serde_json::to_value(template)
                    .map_err(|_| ResourceTemplateCatalogPublicationError::InvalidTemplate)?;
                determinant.push(ResourceTemplateRouteDeterminant {
                    upstream_name: upstream.clone(),
                    native_uri_template: template.uri_template.clone(),
                    incarnation: source.incarnation,
                    template: value,
                });
                routes.push(PublishedResourceTemplateRoute {
                    upstream_name: Arc::from(upstream.as_str()),
                    native_uri_template: Arc::from(template.uri_template.as_str()),
                    template: template.clone(),
                });
            }
        }
        Ok((determinant, Arc::from(routes)))
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
        self.resource_template_sources
            .retain(|upstream, _| self.entries.contains_key(upstream));
        self.prompt_sources
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
        let template_projection = self.resource_template_projection();
        let template_determinant = match &template_projection {
            Ok((determinant, _)) => {
                ResourceTemplateProjectionDeterminant::Ready(determinant.clone())
            }
            Err(error) => ResourceTemplateProjectionDeterminant::Failed(*error),
        };
        if template_determinant != self.resource_template_determinant {
            self.resource_template_determinant = template_determinant;
            self.published_resource_templates = template_projection.map(|(_, routes)| {
                Arc::new(PublishedResourceTemplateCatalogSnapshot {
                    generation: next_resource_template_generation(),
                    routes,
                })
            });
        }
        let prompt_projection = self.prompt_projection();
        let prompt_determinant = match &prompt_projection {
            Ok((determinant, _)) => PromptProjectionDeterminant::Ready(determinant.clone()),
            Err(error) => PromptProjectionDeterminant::Failed(*error),
        };
        if prompt_determinant != self.prompt_determinant {
            self.prompt_determinant = prompt_determinant;
            self.published_prompts = prompt_projection.map(|(_, routes)| {
                Arc::new(PublishedPromptCatalogSnapshot {
                    generation: next_prompt_generation(),
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

    pub async fn published_resource_template_catalog(
        &self,
    ) -> Result<
        Arc<PublishedResourceTemplateCatalogSnapshot>,
        ResourceTemplateCatalogPublicationError,
    > {
        self.catalog
            .read()
            .await
            .published_resource_templates
            .clone()
    }

    pub async fn published_prompt_catalog(
        &self,
    ) -> Result<Arc<PublishedPromptCatalogSnapshot>, PromptCatalogPublicationError> {
        self.catalog.read().await.published_prompts.clone()
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

    #[cfg(any(test, feature = "testkit"))]
    pub async fn insert_prompt_routes_for_tests(&self, upstream: &str, prompts: Vec<Prompt>) {
        let mut catalog = self.catalog_write().await;
        if !catalog.contains_key(upstream) {
            catalog.insert(
                upstream.to_string(),
                super::entries::healthy_in_process_entry(Arc::from(upstream), HashMap::new()),
            );
        }
        let incarnation = match catalog.incarnation(upstream) {
            Some(incarnation) => incarnation,
            None => {
                let incarnation =
                    super::incarnation::next_connection_incarnation().expect("test identity");
                catalog.bind_incarnation(upstream, incarnation);
                incarnation
            }
        };
        catalog.set_prompt_source(upstream, incarnation, &prompts);
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn insert_tool_routes_for_tests(&self, upstream: &str, tools: Vec<UpstreamTool>) {
        let mut catalog = self.catalog_write().await;
        if !catalog.contains_key(upstream) {
            catalog.insert(
                upstream.to_string(),
                super::entries::healthy_in_process_entry(Arc::from(upstream), HashMap::new()),
            );
        }
        let entry = catalog.get_mut(upstream).expect("test entry");
        entry.tools = tools
            .into_iter()
            .map(|tool| (tool.tool.name.to_string(), tool))
            .collect();
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn set_prompt_last_error_for_tests(&self, upstream: &str, error: Option<String>) {
        let mut catalog = self.catalog_write().await;
        catalog
            .get_mut(upstream)
            .expect("test entry")
            .prompt_last_error = error;
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn set_tool_last_error_for_tests(&self, upstream: &str, error: Option<String>) {
        let mut catalog = self.catalog_write().await;
        catalog
            .get_mut(upstream)
            .expect("test entry")
            .tool_last_error = error;
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn prompt_last_error_for_tests(&self, upstream: &str) -> Option<String> {
        self.catalog
            .read()
            .await
            .get(upstream)
            .expect("test entry")
            .prompt_last_error
            .clone()
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn set_resource_last_error_for_tests(&self, upstream: &str, error: Option<String>) {
        let mut catalog = self.catalog_write().await;
        catalog
            .get_mut(upstream)
            .expect("test entry")
            .resource_last_error = error;
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn resource_last_error_for_tests(&self, upstream: &str) -> Option<String> {
        self.catalog
            .read()
            .await
            .get(upstream)
            .expect("test entry")
            .resource_last_error
            .clone()
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn insert_resource_routes_for_tests(&self, upstream: &str, resources: Vec<Resource>) {
        let mut catalog = self.catalog_write().await;
        if !catalog.contains_key(upstream) {
            catalog.insert(
                upstream.to_string(),
                super::entries::healthy_in_process_entry(Arc::from(upstream), HashMap::new()),
            );
        }
        let incarnation = match catalog.incarnation(upstream) {
            Some(incarnation) => incarnation,
            None => {
                let incarnation =
                    super::incarnation::next_connection_incarnation().expect("test identity");
                catalog.bind_incarnation(upstream, incarnation);
                incarnation
            }
        };
        let entry = catalog.get_mut(upstream).expect("test entry");
        entry.resource_count = resources.len();
        entry.resource_uris = resources
            .iter()
            .map(|resource| resource.uri.clone())
            .collect();
        catalog.set_resource_source(upstream, incarnation, &resources);
    }

    #[cfg(test)]
    pub(crate) async fn insert_resource_template_routes_for_tests(
        &self,
        upstream: &str,
        templates: Vec<ResourceTemplate>,
    ) {
        let mut catalog = self.catalog_write().await;
        if !catalog.contains_key(upstream) {
            catalog.insert(
                upstream.to_string(),
                super::entries::healthy_in_process_entry(Arc::from(upstream), HashMap::new()),
            );
        }
        let incarnation = match catalog.incarnation(upstream) {
            Some(incarnation) => incarnation,
            None => {
                let incarnation =
                    super::incarnation::next_connection_incarnation().expect("test identity");
                catalog.bind_incarnation(upstream, incarnation);
                incarnation
            }
        };
        catalog.set_resource_template_source(upstream, incarnation, &templates);
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use rmcp::model::{Prompt, Resource, Tool};

    use super::*;
    use crate::upstream::pool::entries::healthy_in_process_entry;
    use crate::upstream::types::{ToolExposurePolicy, ToolPattern};

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

    fn install_prompt_source(state: &mut CatalogState, upstream: &str, prompts: Vec<Prompt>) {
        let incarnation =
            super::super::incarnation::next_connection_incarnation().expect("identity");
        state.entries.insert(
            upstream.to_string(),
            healthy_in_process_entry(Arc::from(upstream), HashMap::new()),
        );
        state.bind_incarnation(upstream, incarnation);
        state.set_prompt_source(upstream, incarnation, &prompts);
    }

    #[test]
    fn prompt_projection_is_exact_deterministic_and_immutable() {
        let mut state = CatalogState::new();
        let beta = Prompt::new("beta", Some("exact metadata"), None);
        install_prompt_source(
            &mut state,
            "zeta",
            vec![beta.clone(), Prompt::new("alpha", Some(""), None)],
        );
        install_prompt_source(
            &mut state,
            "alpha",
            vec![Prompt::new("remote", Some(""), None)],
        );
        state.publish_if_changed();
        let first = state.published_prompts.clone().expect("published");
        assert_eq!(
            first
                .routes()
                .iter()
                .map(|route| (route.upstream_name.as_ref(), route.native_name.as_ref()))
                .collect::<Vec<_>>(),
            vec![("alpha", "remote"), ("zeta", "alpha"), ("zeta", "beta")]
        );
        assert_eq!(first.routes()[2].prompt, beta);

        state.publish_if_changed();
        let identical = state.published_prompts.clone().expect("identical");
        assert!(Arc::ptr_eq(&first, &identical));
        assert_eq!(first.generation(), identical.generation());
    }

    #[test]
    fn prompt_generation_tracks_source_identity_rebind_and_removal() {
        let mut state = CatalogState::new();
        let rows = vec![Prompt::new("same", Some("metadata"), None)];
        install_prompt_source(&mut state, "alpha", rows.clone());
        state.publish_if_changed();
        let first = state.published_prompts.clone().expect("first");
        let first_generation = first.generation();
        let incarnation = state.incarnation("alpha").expect("identity");

        state.set_prompt_source("alpha", incarnation, &rows);
        state.publish_if_changed();
        let identical = state.published_prompts.clone().expect("identical");
        assert!(Arc::ptr_eq(&first, &identical));

        let replacement =
            super::super::incarnation::next_connection_incarnation().expect("replacement");
        state.bind_incarnation("alpha", replacement);
        state.publish_if_changed();
        let rebound_empty = state.published_prompts.clone().expect("cleared on bind");
        assert!(rebound_empty.routes().is_empty());
        assert_ne!(rebound_empty.generation(), first_generation);
        state.set_prompt_source("alpha", replacement, &rows);
        state.publish_if_changed();
        let rebound = state.published_prompts.clone().expect("rebound");
        assert_eq!(rebound.routes()[0].native_name.as_ref(), "same");
        assert_ne!(rebound.generation(), first_generation);
        assert_eq!(first.routes()[0].native_name.as_ref(), "same");

        state.entries.remove("alpha");
        state.remove_incarnation("alpha");
        state.publish_if_changed();
        let removed = state.published_prompts.clone().expect("removed");
        assert!(removed.routes().is_empty());
        assert_ne!(removed.generation(), rebound.generation());
    }

    #[test]
    fn prompt_projection_tracks_policy_health_and_incarnation_not_diagnostic_hints() {
        let mut state = CatalogState::new();
        let rows = vec![
            Prompt::new("one", Some(""), None),
            Prompt::new("two", Some(""), None),
        ];
        install_prompt_source(&mut state, "alpha", rows.clone());
        state.publish_if_changed();
        let first = state.published_prompts.clone().expect("first");

        let entry = state.entries.get_mut("alpha").expect("entry");
        entry.prompt_count = 99;
        entry.prompt_names = vec!["unrelated/hint".to_string()];
        state.publish_if_changed();
        let hints_only = state.published_prompts.clone().expect("hints only");
        assert!(Arc::ptr_eq(&first, &hints_only));

        state
            .entries
            .get_mut("alpha")
            .expect("entry")
            .prompt_exposure_policy =
            ToolExposurePolicy::AllowList(vec![ToolPattern::Exact("one".to_string())]);
        state.publish_if_changed();
        assert_eq!(
            state
                .published_prompts
                .clone()
                .expect("filtered")
                .routes()
                .iter()
                .map(|route| route.native_name.as_ref())
                .collect::<Vec<_>>(),
            vec!["one"]
        );
        state.entries.get_mut("alpha").expect("entry").prompt_health =
            crate::upstream::types::UpstreamHealth::Unhealthy {
                consecutive_failures: crate::upstream::types::CIRCUIT_BREAKER_THRESHOLD,
            };
        state.publish_if_changed();
        assert!(
            state
                .published_prompts
                .clone()
                .expect("unroutable")
                .routes()
                .is_empty()
        );

        let replacement =
            super::super::incarnation::next_connection_incarnation().expect("replacement");
        state.bind_incarnation("alpha", replacement);
        state.publish_if_changed();
        assert!(
            state
                .published_prompts
                .clone()
                .expect("rebound")
                .routes()
                .is_empty()
        );
        state.entries.remove("alpha");
        state.remove_incarnation("alpha");
        state.publish_if_changed();
        assert!(
            state
                .published_prompts
                .clone()
                .expect("removed")
                .routes()
                .is_empty()
        );
    }

    fn prompt_with_serialized_size(name: &str, target: usize) -> Prompt {
        let base = Prompt::new(name, Some(""), None);
        let base_len = serde_json::to_vec(&base).expect("serialize base").len();
        assert!(base_len <= target);
        let prompt = Prompt::new(name, Some("x".repeat(target - base_len)), None);
        assert_eq!(
            serde_json::to_vec(&prompt).expect("serialize prompt").len(),
            target
        );
        prompt
    }

    #[test]
    fn prompt_bounds_and_structural_errors_fail_closed_and_recover() {
        let mut state = CatalogState::new();
        let exact_routes = (0..super::super::tools::MAX_UPSTREAM_PROMPTS)
            .map(|index| Prompt::new(format!("prompt-{index}"), Some(""), None))
            .collect::<Vec<_>>();
        install_prompt_source(&mut state, "alpha", exact_routes);
        state.publish_if_changed();
        assert_eq!(
            state
                .published_prompts
                .clone()
                .expect("exact route cap")
                .routes()
                .len(),
            super::super::tools::MAX_UPSTREAM_PROMPTS
        );
        let incarnation = state.incarnation("alpha").expect("identity");
        let over_routes = (0..=super::super::tools::MAX_UPSTREAM_PROMPTS)
            .map(|index| Prompt::new(format!("over-{index}"), Some(""), None))
            .collect::<Vec<_>>();
        state.set_prompt_source("alpha", incarnation, &over_routes);
        state.publish_if_changed();
        assert!(matches!(
            state.published_prompts,
            Err(PromptCatalogPublicationError::TooManyRoutes)
        ));

        state.set_prompt_source(
            "alpha",
            incarnation,
            &[
                Prompt::new("duplicate", Some(""), None),
                Prompt::new("duplicate", Some(""), None),
            ],
        );
        state.publish_if_changed();
        assert!(matches!(
            state.published_prompts,
            Err(PromptCatalogPublicationError::DuplicatePrompt)
        ));
        state.set_prompt_source(
            "alpha",
            incarnation,
            &[Prompt::new("bad\nname", Some(""), None)],
        );
        state.publish_if_changed();
        assert!(matches!(
            state.published_prompts,
            Err(PromptCatalogPublicationError::InvalidPrompt)
        ));

        let exact_bytes = (0..8)
            .map(|index| {
                prompt_with_serialized_size(&format!("exact-{index}"), MAX_RESOURCE_ROW_BYTES)
            })
            .collect::<Vec<_>>();
        state.set_prompt_source("alpha", incarnation, &exact_bytes);
        state.publish_if_changed();
        assert!(state.published_prompts.is_ok());
        state.set_prompt_source(
            "alpha",
            incarnation,
            &[prompt_with_serialized_size(
                "oversized",
                MAX_RESOURCE_ROW_BYTES + 1,
            )],
        );
        state.publish_if_changed();
        assert!(matches!(
            state.published_prompts,
            Err(PromptCatalogPublicationError::TooManyBytes)
        ));
        state.set_prompt_source(
            "alpha",
            incarnation,
            &[Prompt::new("fixed", Some(""), None)],
        );
        state.publish_if_changed();
        assert_eq!(
            state.published_prompts.clone().expect("recovered").routes()[0]
                .native_name
                .as_ref(),
            "fixed"
        );
    }

    #[test]
    fn prompt_retained_bytes_are_global_across_routable_sources() {
        let mut state = CatalogState::new();
        for upstream in ["alpha", "beta", "gamma"] {
            let rows = (0..3)
                .map(|index| prompt_with_serialized_size(&format!("{upstream}-{index}"), 950_000))
                .collect();
            install_prompt_source(&mut state, upstream, rows);
        }
        state.publish_if_changed();
        assert!(matches!(
            state.published_prompts,
            Err(PromptCatalogPublicationError::TooManyBytes)
        ));
    }

    #[test]
    fn prompt_publication_is_independent_from_tool_resource_and_template_families() {
        let mut state = CatalogState::new();
        state.entries.insert("alpha".into(), entry("alpha", "read"));
        let incarnation =
            super::super::incarnation::next_connection_incarnation().expect("identity");
        state.bind_incarnation("alpha", incarnation);
        state.set_prompt_source("alpha", incarnation, &[Prompt::new("one", Some(""), None)]);
        let resource = Resource::new("file:///one", "one");
        state.entries.get_mut("alpha").expect("entry").resource_uris = vec![resource.uri.clone()];
        state.set_resource_source("alpha", incarnation, &[resource]);
        state.set_resource_template_source(
            "alpha",
            incarnation,
            &[ResourceTemplate::new("file:///{id}", "template")],
        );
        state.publish_if_changed();
        let tool = state.published.clone().expect("tool");
        let resource = state.published_resources.clone().expect("resource");
        let template = state
            .published_resource_templates
            .clone()
            .expect("template");

        state.set_prompt_source("alpha", incarnation, &[Prompt::new("two", Some(""), None)]);
        state.publish_if_changed();
        assert!(Arc::ptr_eq(
            &tool,
            &state.published.clone().expect("tool unchanged")
        ));
        assert!(Arc::ptr_eq(
            &resource,
            &state
                .published_resources
                .clone()
                .expect("resource unchanged")
        ));
        assert!(Arc::ptr_eq(
            &template,
            &state
                .published_resource_templates
                .clone()
                .expect("template unchanged")
        ));
        let prompt = state.published_prompts.clone().expect("prompt");

        state.set_resource_source(
            "alpha",
            incarnation,
            &[Resource::new("file:///changed", "changed")],
        );
        state.set_resource_template_source(
            "alpha",
            incarnation,
            &[ResourceTemplate::new("https://changed/{id}", "changed")],
        );
        state
            .entries
            .get_mut("alpha")
            .expect("entry")
            .tools
            .get_mut("read")
            .expect("tool")
            .destructive = true;
        state.publish_if_changed();
        assert!(Arc::ptr_eq(
            &prompt,
            &state.published_prompts.clone().expect("prompt unchanged")
        ));
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

    fn install_resource_template_source(
        state: &mut CatalogState,
        upstream: &str,
        templates: Vec<ResourceTemplate>,
    ) {
        let incarnation =
            super::super::incarnation::next_connection_incarnation().expect("identity");
        state.entries.insert(
            upstream.to_string(),
            healthy_in_process_entry(Arc::from(upstream), HashMap::new()),
        );
        state.bind_incarnation(upstream, incarnation);
        state.set_resource_template_source(upstream, incarnation, &templates);
    }

    #[test]
    fn resource_template_projection_is_exact_deterministic_and_immutable() {
        let mut state = CatalogState::new();
        let beta =
            ResourceTemplate::new("file:///beta/{id}", "beta").with_description("exact metadata");
        install_resource_template_source(
            &mut state,
            "zeta",
            vec![
                beta.clone(),
                ResourceTemplate::new("UI://widget/{id}", "widget"),
                ResourceTemplate::new("file:///alpha/{id}", "alpha"),
            ],
        );
        install_resource_template_source(
            &mut state,
            "alpha",
            vec![ResourceTemplate::new("https://example/{id}", "remote")],
        );
        state.publish_if_changed();
        let first = state
            .published_resource_templates
            .clone()
            .expect("published");
        assert_eq!(
            first
                .routes()
                .iter()
                .map(|route| (
                    route.upstream_name.as_ref(),
                    route.native_uri_template.as_ref()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("alpha", "https://example/{id}"),
                ("zeta", "file:///alpha/{id}"),
                ("zeta", "file:///beta/{id}"),
            ]
        );
        assert_eq!(
            serde_json::to_value(&first.routes()[2].template).expect("metadata"),
            serde_json::to_value(&beta).expect("metadata")
        );

        state.entries.get_mut("zeta").expect("zeta").resource_health =
            crate::upstream::types::UpstreamHealth::Unhealthy {
                consecutive_failures: 3,
            };
        state.publish_if_changed();
        let narrowed = state
            .published_resource_templates
            .clone()
            .expect("narrowed");
        assert_eq!(narrowed.routes().len(), 1);
        assert_eq!(first.routes().len(), 3, "old snapshot remains immutable");
    }

    #[test]
    fn resource_template_generation_tracks_incarnation_and_proxy_not_exposure_policy() {
        let mut state = CatalogState::new();
        let rows = vec![ResourceTemplate::new("file:///{id}", "same")];
        install_resource_template_source(&mut state, "alpha", rows.clone());
        state.publish_if_changed();
        let first = state.published_resource_templates.clone().expect("first");
        let incarnation = state.incarnation("alpha").expect("identity");
        state.set_resource_template_source("alpha", incarnation, &rows);
        state.publish_if_changed();
        assert!(Arc::ptr_eq(
            &first,
            &state
                .published_resource_templates
                .clone()
                .expect("identical")
        ));

        state
            .entries
            .get_mut("alpha")
            .expect("alpha")
            .resource_exposure_policy = ToolExposurePolicy::AllowList(Vec::new());
        state.publish_if_changed();
        assert!(Arc::ptr_eq(
            &first,
            &state
                .published_resource_templates
                .clone()
                .expect("exposure policy is unrelated")
        ));
        state
            .entries
            .get_mut("alpha")
            .expect("alpha")
            .proxy_resources = false;
        state.publish_if_changed();
        assert!(
            state
                .published_resource_templates
                .clone()
                .expect("hidden")
                .routes()
                .is_empty()
        );

        let replacement =
            super::super::incarnation::next_connection_incarnation().expect("replacement");
        state.bind_incarnation("alpha", replacement);
        state
            .entries
            .get_mut("alpha")
            .expect("alpha")
            .proxy_resources = true;
        state.publish_if_changed();
        assert!(
            state
                .published_resource_templates
                .clone()
                .expect("old source cleared")
                .routes()
                .is_empty()
        );
        state.set_resource_template_source("alpha", replacement, &rows);
        state.publish_if_changed();
        let rebound = state.published_resource_templates.clone().expect("rebound");
        assert_eq!(rebound.routes().len(), 1);
        assert_ne!(first.generation(), rebound.generation());
        state.entries.remove("alpha");
        state.remove_incarnation("alpha");
        state.publish_if_changed();
        let removed = state.published_resource_templates.clone().expect("removed");
        assert!(removed.routes().is_empty());
        assert_ne!(rebound.generation(), removed.generation());
    }

    #[test]
    fn resource_template_structural_failures_are_typed_and_recover() {
        let mut state = CatalogState::new();
        install_resource_template_source(
            &mut state,
            "alpha",
            vec![
                ResourceTemplate::new("file:///{id}", "one"),
                ResourceTemplate::new("file:///{id}", "two"),
            ],
        );
        state.publish_if_changed();
        assert!(matches!(
            state.published_resource_templates,
            Err(ResourceTemplateCatalogPublicationError::DuplicateTemplate)
        ));
        let incarnation = state.incarnation("alpha").expect("identity");
        state.set_resource_template_source(
            "alpha",
            incarnation,
            &[ResourceTemplate::new("bad\ntemplate", "bad")],
        );
        state.publish_if_changed();
        assert!(matches!(
            state.published_resource_templates,
            Err(ResourceTemplateCatalogPublicationError::InvalidTemplate)
        ));
        state.set_resource_template_source(
            "alpha",
            incarnation,
            &[ResourceTemplate::new("file:///fixed/{id}", "fixed")],
        );
        state.publish_if_changed();
        assert_eq!(
            state
                .published_resource_templates
                .clone()
                .expect("recovered")
                .routes()
                .len(),
            1
        );
    }

    #[test]
    fn resource_template_route_cap_fails_closed_and_unrelated_mutation_is_independent() {
        let mut state = CatalogState::new();
        install_resource_template_source(
            &mut state,
            "alpha",
            (0..super::super::tools::MAX_UPSTREAM_RESOURCES)
                .map(|index| ResourceTemplate::new(format!("file:///{{id}}/{index}"), "row"))
                .collect(),
        );
        state.publish_if_changed();
        assert_eq!(
            state
                .published_resource_templates
                .clone()
                .expect("exact route cap")
                .routes()
                .len(),
            super::super::tools::MAX_UPSTREAM_RESOURCES
        );
        let incarnation = state.incarnation("alpha").expect("identity");
        state.set_resource_template_source(
            "alpha",
            incarnation,
            &(0..=super::super::tools::MAX_UPSTREAM_RESOURCES)
                .map(|index| ResourceTemplate::new(format!("file:///{{id}}/{index}"), "row"))
                .collect::<Vec<_>>(),
        );
        state.publish_if_changed();
        assert!(matches!(
            state.published_resource_templates,
            Err(ResourceTemplateCatalogPublicationError::TooManyRoutes)
        ));
        state.set_resource_template_source(
            "alpha",
            incarnation,
            &[ResourceTemplate::new("file:///{id}", "fixed")],
        );
        state.publish_if_changed();
        let recovered = state
            .published_resource_templates
            .clone()
            .expect("recovered");
        let mut unrelated = healthy_in_process_entry(Arc::from("tools-only"), HashMap::new());
        unrelated.prompt_count = 99;
        state.entries.insert("tools-only".into(), unrelated);
        state.publish_if_changed();
        assert!(Arc::ptr_eq(
            &recovered,
            &state
                .published_resource_templates
                .clone()
                .expect("independent")
        ));
        install_resource_source(
            &mut state,
            "resources-only",
            vec![Resource::new("file:///resource", "resource")],
        );
        state.publish_if_changed();
        assert!(Arc::ptr_eq(
            &recovered,
            &state
                .published_resource_templates
                .clone()
                .expect("resource publication is independent")
        ));
        let resource_snapshot = state
            .published_resources
            .clone()
            .expect("resource snapshot");
        let tool_snapshot = state.published.clone().expect("tool snapshot");
        state.set_resource_template_source(
            "alpha",
            incarnation,
            &[ResourceTemplate::new("file:///changed/{id}", "changed")],
        );
        state.publish_if_changed();
        assert!(Arc::ptr_eq(
            &resource_snapshot,
            &state
                .published_resources
                .clone()
                .expect("template-independent")
        ));
        assert!(Arc::ptr_eq(
            &tool_snapshot,
            &state.published.clone().expect("template-independent tools")
        ));
    }

    fn template_with_serialized_size(uri: &str, target: usize) -> ResourceTemplate {
        let base = ResourceTemplate::new(uri, "row").with_description("");
        let base_len = serde_json::to_vec(&base).expect("serialize base").len();
        assert!(base_len <= target);
        let template =
            ResourceTemplate::new(uri, "row").with_description("x".repeat(target - base_len));
        assert_eq!(
            serde_json::to_vec(&template)
                .expect("serialize template")
                .len(),
            target
        );
        template
    }

    #[test]
    fn resource_template_byte_bounds_are_exact_global_and_recoverable() {
        let mut state = CatalogState::new();
        let exact = (0..8)
            .map(|index| {
                template_with_serialized_size(
                    &format!("file:///exact/{index}/{{id}}"),
                    MAX_RESOURCE_ROW_BYTES,
                )
            })
            .collect::<Vec<_>>();
        install_resource_template_source(&mut state, "alpha", exact);
        state.publish_if_changed();
        assert_eq!(
            state
                .published_resource_templates
                .clone()
                .expect("exact byte limits pass")
                .routes()
                .len(),
            8
        );

        let incarnation = state.incarnation("alpha").expect("identity");
        state.set_resource_template_source(
            "alpha",
            incarnation,
            &[template_with_serialized_size(
                "file:///oversized/{id}",
                MAX_RESOURCE_ROW_BYTES + 1,
            )],
        );
        state.publish_if_changed();
        assert!(matches!(
            state.published_resource_templates,
            Err(ResourceTemplateCatalogPublicationError::TooManyBytes)
        ));
        state.set_resource_template_source(
            "alpha",
            incarnation,
            &[ResourceTemplate::new("file:///fixed/{id}", "fixed")],
        );
        state.publish_if_changed();
        assert!(state.published_resource_templates.is_ok());

        for upstream in ["alpha", "beta", "gamma"] {
            let rows = (0..3)
                .map(|index| {
                    ResourceTemplate::new(
                        format!("file:///{upstream}/{index}/{{id}}"),
                        "x".repeat(950_000),
                    )
                })
                .collect::<Vec<_>>();
            if upstream == "alpha" {
                state.set_resource_template_source(upstream, incarnation, &rows);
            } else {
                install_resource_template_source(&mut state, upstream, rows);
            }
        }
        state.publish_if_changed();
        assert!(matches!(
            state.published_resource_templates,
            Err(ResourceTemplateCatalogPublicationError::TooManyBytes)
        ));
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
