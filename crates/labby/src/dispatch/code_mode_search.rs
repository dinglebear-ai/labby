//! Product-owned, query-driven artifact discovery for Code Mode.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::LazyLock;

use futures::{StreamExt, stream};
use labby_codemode::{
    ArtifactSearchResult, CatalogDescriptor, CodeModeCaller, CodeModeCatalogKind, CodeModeSurface,
    ToolScope,
};
use labby_gateway::gateway::code_mode::CodeModeArtifactSearchProvider;
use labby_runtime::artifacts::{ArtifactStore, PublicationState, Visibility};
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::{CodeModeSearchConfig, CodeModeSearchSource};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::config::depot::PUBLIC_ID;
use crate::dispatch::depot::manager::Manager;
use crate::skills::facade::{code_mode_skill_context, search_visible_skills_bounded};

// One sentinel beyond the public 50-result limit lets Code Mode report
// truncation when a queried source has more matches than the caller requested.
const MAX_SEARCH_RESULTS: usize = 51;

enum DepotSearchRequest {
    Skills(String),
    Artifacts(String, &'static str, CodeModeCatalogKind),
}

const DEPOT_SEARCH_CONCURRENCY: usize = 4;
const DEPOT_ADMISSION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(1);
const DEPOT_REQUEST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
// Release a slot before the overall deadline so a stalled provider cannot
// prevent later providers in the same source from being queried at all.
const DEPOT_BUCKET_DEADLINE: std::time::Duration = std::time::Duration::from_secs(1);
const PERSONAL_SEARCH_GRACE_AFTER_DEPOT_HIT: std::time::Duration =
    std::time::Duration::from_millis(500);
const PERSONAL_SEARCH_GRACE_AFTER_EMPTY_DEPOT: std::time::Duration =
    std::time::Duration::from_secs(1);
static PERSONAL_ARTIFACT_SCAN_SLOTS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

type SourceSearch = Result<Option<ArtifactSearchResult>, ToolError>;

async fn join_search_sources<S, A, D>(
    skills: S,
    artifacts: A,
    depot: D,
) -> (SourceSearch, SourceSearch, SourceSearch)
where
    S: Future<Output = SourceSearch>,
    A: Future<Output = SourceSearch>,
    D: Future<Output = SourceSearch>,
{
    tokio::pin!(skills);
    tokio::pin!(artifacts);
    tokio::pin!(depot);
    let mut skill_result = None;
    let mut artifact_result = None;
    let depot_result = loop {
        tokio::select! {
            result = &mut depot => break result,
            result = &mut skills, if skill_result.is_none() => skill_result = Some(result),
            result = &mut artifacts, if artifact_result.is_none() => artifact_result = Some(result),
        }
    };
    if matches!(depot_result, Ok(None)) {
        let (skill_result, artifact_result) = tokio::join!(
            async {
                match skill_result {
                    Some(result) => result,
                    None => skills.await,
                }
            },
            async {
                match artifact_result {
                    Some(result) => result,
                    None => artifacts.await,
                }
            },
        );
        return (skill_result, artifact_result, depot_result);
    }
    let indexed_hit = depot_result
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .is_some_and(|result| !result.entries.is_empty());
    let grace = if indexed_hit {
        PERSONAL_SEARCH_GRACE_AFTER_DEPOT_HIT
    } else {
        PERSONAL_SEARCH_GRACE_AFTER_EMPTY_DEPOT
    };
    let (skill_result, artifact_result) = tokio::join!(
        async {
            match skill_result {
                Some(result) => result,
                None => tokio::time::timeout(grace, &mut skills)
                    .await
                    .unwrap_or_else(|_| {
                        Err(search_error(
                            "timeout",
                            "personal Skill search exceeded the grace period after Depot completed",
                        ))
                    }),
            }
        },
        async {
            match artifact_result {
                Some(result) => result,
                None => tokio::time::timeout(grace, &mut artifacts).await.unwrap_or_else(|_| {
                    Err(search_error("timeout", "personal Artifact search exceeded the grace period after Depot completed"))
                }),
            }
        }
    );
    (skill_result, artifact_result, depot_result)
}

async fn bounded_ordered_until<T, F>(
    futures: Vec<F>,
    max_wait: std::time::Duration,
) -> (Vec<(usize, T)>, Vec<usize>)
where
    F: Future<Output = T>,
{
    let count = futures.len();
    let deadline = tokio::time::Instant::now() + max_wait;
    let requests = stream::iter(futures.into_iter().enumerate())
        .map(|(index, future)| async move {
            (
                index,
                tokio::time::timeout(DEPOT_BUCKET_DEADLINE, future)
                    .await
                    .ok(),
            )
        })
        .buffer_unordered(DEPOT_SEARCH_CONCURRENCY);
    tokio::pin!(requests);
    let mut completed = Vec::new();
    while let Ok(Some((index, result))) = tokio::time::timeout_at(deadline, requests.next()).await {
        if let Some(result) = result {
            completed.push((index, result));
        }
    }
    let mut missing = vec![true; count];
    for (index, _) in &completed {
        missing[*index] = false;
    }
    completed.sort_by_key(|(index, _)| *index);
    let pending = missing
        .into_iter()
        .enumerate()
        .filter_map(|(index, missing)| missing.then_some(index))
        .collect();
    (completed, pending)
}

// Admit one request from each source before filling another slot from the
// same source. Otherwise a slow public provider can occupy every concurrency
// slot while a fast team provider waits behind it for the full deadline.
fn interleave_sources<T>(sources: Vec<Vec<T>>) -> Vec<T> {
    let total = sources.iter().map(Vec::len).sum();
    let mut sources = sources.into_iter().map(Vec::into_iter).collect::<Vec<_>>();
    let mut requests = Vec::with_capacity(total);
    loop {
        let mut added = false;
        for source in &mut sources {
            if let Some(request) = source.next() {
                requests.push(request);
                added = true;
            }
        }
        if !added {
            return requests;
        }
    }
}

fn merge_source_buckets(
    request_buckets: Vec<(usize, Vec<CatalogDescriptor>)>,
    source_count: usize,
    limit: usize,
) -> Vec<CatalogDescriptor> {
    let mut by_source = vec![Vec::new(); source_count];
    for (source, bucket) in request_buckets {
        if let Some(source_buckets) = by_source.get_mut(source) {
            source_buckets.push(bucket);
        }
    }
    let sources = by_source
        .into_iter()
        .map(|buckets| fair_merge(buckets, limit))
        .collect();
    fair_merge(sources, limit)
}

pub(crate) struct ProductCodeModeArtifactSearchProvider {
    depot: Arc<Manager>,
    artifacts: Arc<ArtifactStore>,
}

impl ProductCodeModeArtifactSearchProvider {
    pub(crate) fn production(depot: Arc<Manager>) -> Result<Self, ToolError> {
        let artifacts =
            ArtifactStore::new(labby_runtime::lab_home().join("artifacts")).map_err(|error| {
                ToolError::Sdk {
                    sdk_kind: "storage_error".into(),
                    message: format!("open canonical Artifact store: {error}"),
                }
            })?;
        Ok(Self {
            depot,
            artifacts: Arc::new(artifacts),
        })
    }

    fn personal_skill_context(
        caller: &CodeModeCaller,
        scope: &ToolScope,
    ) -> Option<crate::skills::facade::SkillRegistryContext> {
        let context = match caller.without_authority() {
            CodeModeCaller::ScopedSkills {
                skill_context_token,
                ..
            }
            | CodeModeCaller::ScopedHostProviderSkills {
                skill_context_token,
                ..
            } => code_mode_skill_context(skill_context_token),
            CodeModeCaller::TrustedLocal => Some(Arc::new(
                crate::skills::facade::SkillRegistryContext::first_party_only(),
            )),
            _ => None,
        };
        context.map(|context| context.narrowed_to_upstreams(scope.allowed_namespaces()))
    }

    async fn personal_skills_with_providers(
        &self,
        query: &str,
        limit: usize,
        caller: &CodeModeCaller,
        scope: &ToolScope,
        config: &CodeModeSearchConfig,
        depot_providers: &[(String, String)],
    ) -> Result<ArtifactSearchResult, ToolError> {
        let Some(context) = Self::personal_skill_context(caller, scope) else {
            return Ok(ArtifactSearchResult::default());
        };
        let context = context
            .without_depot_skill_upstreams(depot_providers, &config.depot_skill_upstreams)
            .await;
        let listing = search_visible_skills_bounded(&context, query, limit).await;
        Ok(Self::project_skill_listing(listing, limit))
    }

    fn project_skill_listing(
        listing: labby_runtime::skills::SkillsListResult,
        limit: usize,
    ) -> ArtifactSearchResult {
        let incomplete = listing.meta.as_ref().is_some_and(|meta| {
            meta.contains_key("unreachableUpstreams")
                || meta.contains_key("truncated")
                || meta.contains_key("incompleteSearchUpstreams")
        });
        ArtifactSearchResult {
            entries: listing
                .skills
                .into_iter()
                .take(limit)
                .map(|entry| {
                    let name = entry.frontmatter_str("name").unwrap_or(&entry.uri);
                    let description = entry.frontmatter_str("description").unwrap_or_default();
                    let namespace = entry.origin().unwrap_or_else(|| "labby".into());
                    let mut tags = frontmatter_tags(&entry.frontmatter);
                    tags.push(format!("uri:{}", entry.uri));
                    CatalogDescriptor::metadata(
                        CodeModeCatalogKind::Skill,
                        &namespace,
                        &format!("skill::{}", entry.uri),
                        name,
                        description,
                        tags,
                    )
                })
                .collect(),
            incomplete_sources: if incomplete {
                vec!["personal_labby".to_owned()]
            } else {
                Vec::new()
            },
        }
    }

    fn personal_artifacts(
        artifacts: &ArtifactStore,
        query: &str,
        limit: usize,
        kinds: &[CodeModeCatalogKind],
        caller: &CodeModeCaller,
    ) -> Result<Vec<CatalogDescriptor>, ToolError> {
        let requested_kinds = kinds
            .iter()
            .copied()
            .filter(is_local_artifact_kind)
            .collect::<Vec<_>>();
        if requested_kinds.is_empty() {
            return Ok(Vec::new());
        }
        let records = artifacts.list_records().map_err(|error| ToolError::Sdk {
            sdk_kind: "storage_error".into(),
            message: format!("list personal artifacts: {error}"),
        })?;
        let mut buckets = vec![Vec::new(); requested_kinds.len()];
        let normalized_query = query.trim().to_ascii_lowercase();
        for record in records {
            if !caller.is_admin()
                && (record.publication.state != PublicationState::Published
                    || record.publication.visibility != Visibility::Public)
            {
                continue;
            }
            let Some(position) = local_kind(&record.descriptor.kind).and_then(|kind| {
                requested_kinds
                    .iter()
                    .position(|requested| *requested == kind)
            }) else {
                continue;
            };
            let descriptor = &record.descriptor;
            if !descriptor_matches(descriptor, &normalized_query) {
                continue;
            }
            buckets[position].push(CatalogDescriptor::metadata(
                requested_kinds[position],
                &descriptor.namespace,
                &format!("labby::{}", descriptor.id),
                &descriptor.name,
                descriptor.description.as_deref().unwrap_or_default(),
                descriptor.tags.clone(),
            ));
        }
        Ok(fair_merge(buckets, limit))
    }

    async fn depot_artifacts_with_providers(
        &self,
        topology: &crate::dispatch::depot::manager::Topology,
        query: &str,
        limit: usize,
        kinds: &[CodeModeCatalogKind],
        sources: &BTreeSet<CodeModeSearchSource>,
        depot_providers: &[(String, String)],
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
    ) -> Result<ArtifactSearchResult, ToolError> {
        let short_query = query.trim().chars().count() < 3;
        if query.trim().is_empty() || (short_query && !kinds.contains(&CodeModeCatalogKind::Skill))
        {
            return Ok(ArtifactSearchResult::default());
        }
        let mut groups = Vec::new();
        if sources.contains(&CodeModeSearchSource::PublicDepot)
            && depot_providers.iter().any(|(id, _)| id == PUBLIC_ID)
        {
            groups.push(vec![PUBLIC_ID.to_owned()]);
        }
        if sources.contains(&CodeModeSearchSource::TeamDepot) {
            let team = depot_providers
                .iter()
                .filter(|(id, _)| id != PUBLIC_ID)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            if !team.is_empty() {
                groups.push(team);
            }
        }
        let indexed_skill_search = kinds.contains(&CodeModeCatalogKind::Skill);
        // Generic Depot discovery requires at least three characters. Native
        // Skill search accepts shorter queries, so retain that path instead
        // of silently hiding the corresponding MCP proxy.
        let depot_kinds = if short_query {
            Vec::new()
        } else {
            kinds
                .iter()
                .filter(|kind| **kind != CodeModeCatalogKind::Skill)
                .flat_map(|kind| depot_kinds(*kind))
                .copied()
                .collect::<Vec<_>>()
        };
        if groups.is_empty() || (!indexed_skill_search && depot_kinds.is_empty()) {
            return Ok(ArtifactSearchResult::default());
        }
        let actor = format!(
            "codemode:{}:{}",
            surface.tag(),
            hex::encode(Sha256::digest(
                caller.subject().unwrap_or("trusted-local").as_bytes()
            ))
        );
        // A sparse query may match only one provider and kind. Each request
        // therefore needs the caller's full result budget; dividing it across
        // all possible buckets silently loses matches from the populated one.
        let page_limit = limit.clamp(1, MAX_SEARCH_RESULTS) as u16;
        let mut requests_by_source = Vec::new();
        for (source, providers) in groups.iter().enumerate() {
            let mut requests = Vec::new();
            if indexed_skill_search {
                for provider_id in providers {
                    requests.push((source, DepotSearchRequest::Skills(provider_id.clone())));
                }
            }
            for provider_id in providers {
                for &(wire_kind, catalog_kind) in &depot_kinds {
                    requests.push((
                        source,
                        DepotSearchRequest::Artifacts(provider_id.clone(), wire_kind, catalog_kind),
                    ));
                }
            }
            requests_by_source.push(requests);
        }
        let requests = interleave_sources(requests_by_source);
        let admission = tokio::time::timeout(
            DEPOT_ADMISSION_DEADLINE,
            self.depot
                .scheduler
                .admit(&actor, tokio::time::Instant::now()),
        )
        .await
        .map_err(|_| search_error("capacity", "Depot search admission timed out"))?
        .map_err(|_| search_error("capacity", "Depot search admission is pending"))?;
        let admission = &admission;
        let request_sources = requests
            .iter()
            .map(|(source, _)| *source)
            .collect::<Vec<_>>();
        let request_futures = requests
            .into_iter()
            .map(|(source, request)| async move {
                let bucket = match request {
                    DepotSearchRequest::Skills(provider_id) => {
                        if let Some(provider) = topology.providers.get(&provider_id) {
                            match provider
                                .runtime
                                .search_skills(query, usize::from(page_limit), &admission)
                                .await
                            {
                                Ok(value) => Ok(project_depot_skill_search(
                                    &provider_id,
                                    value,
                                    usize::from(page_limit),
                                )),
                                Err(error) => {
                                    tracing::warn!(
                                        surface = surface.tag(),
                                        provider = %provider_id,
                                        error = %error,
                                        "Code Mode Depot Skill index search degraded"
                                    );
                                    Err(search_error("upstream_error", "Depot Skill search failed"))
                                }
                            }
                        } else {
                            Err(search_error(
                                "upstream_error",
                                "Depot provider disappeared during search",
                            ))
                        }
                    }
                    DepotSearchRequest::Artifacts(provider_id, wire_kind, catalog_kind) => {
                        match crate::dispatch::depot::discovery::discover_selected_admitted(
                            topology,
                            &[provider_id],
                            query,
                            wire_kind,
                            page_limit,
                            &admission,
                        )
                        .await
                        {
                            Ok(response) => project_depot_artifact_search(response, catalog_kind),
                            Err(error) => {
                                tracing::warn!(
                                    surface = surface.tag(),
                                    artifact_kind = wire_kind,
                                    error = %error,
                                    "Code Mode Depot search provider degraded"
                                );
                                Err(search_error(
                                    "upstream_error",
                                    "Depot artifact search failed",
                                ))
                            }
                        }
                    }
                };
                (source, bucket)
            })
            .collect();
        let mut request_buckets = Vec::new();
        let mut failed = 0usize;
        let mut incomplete_sources = BTreeSet::new();
        let (completed, pending) =
            bounded_ordered_until(request_futures, DEPOT_REQUEST_DEADLINE).await;
        for (_, (source, bucket)) in completed {
            match bucket {
                Ok(bucket) => request_buckets.push((source, bucket)),
                Err(_) => {
                    failed += 1;
                    incomplete_sources.insert(if groups[source].iter().any(|id| id == PUBLIC_ID) {
                        "public_depot".to_owned()
                    } else {
                        "team_depot".to_owned()
                    });
                }
            }
        }
        if !pending.is_empty() {
            tracing::warn!(
                surface = surface.tag(),
                pending = pending.len(),
                deadline_ms = DEPOT_REQUEST_DEADLINE.as_millis(),
                "Code Mode Depot search request deadline reached"
            );
        }
        for index in pending {
            let source = request_sources[index];
            failed += 1;
            incomplete_sources.insert(if groups[source].iter().any(|id| id == PUBLIC_ID) {
                "public_depot".to_owned()
            } else {
                "team_depot".to_owned()
            });
        }
        if request_buckets.is_empty() && failed > 0 {
            return Err(search_error(
                "upstream_error",
                "all Depot search providers failed",
            ));
        }
        if failed > 0 {
            tracing::warn!(
                surface = surface.tag(),
                failed,
                succeeded = request_buckets.len(),
                "Code Mode Depot search returned partial results"
            );
        }
        Ok(ArtifactSearchResult {
            entries: merge_source_buckets(request_buckets, groups.len(), limit),
            incomplete_sources: incomplete_sources.into_iter().collect(),
        })
    }

    #[cfg(test)]
    async fn personal_skills(
        &self,
        query: &str,
        limit: usize,
        caller: &CodeModeCaller,
        scope: &ToolScope,
        config: &CodeModeSearchConfig,
    ) -> Result<ArtifactSearchResult, ToolError> {
        let providers =
            authorized_depot_providers(&self.depot.snapshot(), &config.sources, caller).await;
        self.personal_skills_with_providers(query, limit, caller, scope, config, &providers)
            .await
    }

    #[cfg(test)]
    async fn depot_artifacts(
        &self,
        query: &str,
        limit: usize,
        kinds: &[CodeModeCatalogKind],
        sources: &BTreeSet<CodeModeSearchSource>,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
    ) -> Result<ArtifactSearchResult, ToolError> {
        let topology = self.depot.snapshot();
        let providers = authorized_depot_providers(&topology, sources, caller).await;
        self.depot_artifacts_with_providers(
            &topology, query, limit, kinds, sources, &providers, caller, surface,
        )
        .await
    }
}

impl CodeModeArtifactSearchProvider for ProductCodeModeArtifactSearchProvider {
    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
        kinds: &'a [CodeModeCatalogKind],
        config: &'a CodeModeSearchConfig,
        caller: &'a CodeModeCaller,
        surface: CodeModeSurface,
        scope: &'a ToolScope,
    ) -> Pin<Box<dyn Future<Output = Result<ArtifactSearchResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let limit = limit.clamp(1, MAX_SEARCH_RESULTS);
            // Authorize each selected provider once for this request. Both the
            // personal proxy filter and native Depot search use this snapshot.
            let topology = self.depot.snapshot();
            let depot_providers = if kinds.contains(&CodeModeCatalogKind::Skill) {
                Some(bounded_authorized_depot_providers(&topology, &config.sources, caller).await)
            } else {
                None
            };
            // Keep ready snapshot matches even when the federated source exhausts
            // its grace period. The shared projection retains caller visibility.
            let ready_skills = if config
                .sources
                .contains(&CodeModeSearchSource::PersonalLabby)
                && kinds.contains(&CodeModeCatalogKind::Skill)
            {
                Self::personal_skill_context(caller, scope)
                    .map(|context| {
                        Self::project_skill_listing(
                            crate::skills::facade::search_first_party_skills(
                                &context, query, limit,
                            ),
                            limit,
                        )
                        .entries
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let search_personal_skills = async {
                if !config
                    .sources
                    .contains(&CodeModeSearchSource::PersonalLabby)
                    || !kinds.contains(&CodeModeCatalogKind::Skill)
                {
                    return Ok(None);
                }
                let started = std::time::Instant::now();
                let providers = depot_providers
                    .as_ref()
                    .and_then(|result| result.as_ref().ok())
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let result = self
                    .personal_skills_with_providers(query, limit, caller, scope, config, providers)
                    .await;
                tracing::debug!(
                    surface = surface.tag(),
                    source = "personal_labby_skills",
                    elapsed_ms = started.elapsed().as_millis(),
                    result_count = result.as_ref().map_or(0, |value| value.entries.len()),
                    failed = result.is_err(),
                    "Code Mode artifact source search finished"
                );
                result.map(Some)
            };
            let search_personal_artifacts = async {
                if !config
                    .sources
                    .contains(&CodeModeSearchSource::PersonalLabby)
                    || !kinds.iter().any(is_local_artifact_kind)
                {
                    return Ok(None);
                }
                let started = std::time::Instant::now();
                let result: Result<ArtifactSearchResult, ToolError> = async {
                    let artifacts = Arc::clone(&self.artifacts);
                    let artifact_query = query.to_owned();
                    let artifact_kinds = kinds.to_vec();
                    let artifact_caller = caller.clone();
                    let scan_permit = Arc::clone(&PERSONAL_ARTIFACT_SCAN_SLOTS)
                        .acquire_owned()
                        .await
                        .map_err(|error| {
                            search_error(
                                "capacity",
                                &format!("personal Artifact search unavailable: {error}"),
                            )
                        })?;
                    let artifact_result = tokio::task::spawn_blocking(move || {
                        let _permit = scan_permit;
                        Self::personal_artifacts(
                            &artifacts,
                            &artifact_query,
                            limit,
                            &artifact_kinds,
                            &artifact_caller,
                        )
                    })
                    .await
                    .map_err(|error| {
                        search_error(
                            "storage_error",
                            &format!("personal Artifact search worker failed: {error}"),
                        )
                    })
                    .and_then(std::convert::identity)?;
                    Ok(ArtifactSearchResult {
                        entries: artifact_result,
                        incomplete_sources: Vec::new(),
                    })
                }
                .await;
                tracing::debug!(
                    surface = surface.tag(),
                    source = "personal_labby_artifacts",
                    elapsed_ms = started.elapsed().as_millis(),
                    result_count = result.as_ref().map_or(0, |value| value.entries.len()),
                    failed = result.is_err(),
                    "Code Mode artifact source search finished"
                );
                result.map(Some)
            };
            let search_depot = async {
                if !config.sources.contains(&CodeModeSearchSource::PublicDepot)
                    && !config.sources.contains(&CodeModeSearchSource::TeamDepot)
                {
                    return Ok(None);
                }
                let started = std::time::Instant::now();
                let result = async {
                    let independently_authorized;
                    let providers = if let Some(result) = &depot_providers {
                        result.as_ref().map_err(Clone::clone)?
                    } else {
                        independently_authorized =
                            bounded_authorized_depot_providers(&topology, &config.sources, caller)
                                .await?;
                        &independently_authorized
                    };
                    self.depot_artifacts_with_providers(
                        &topology,
                        query,
                        limit,
                        kinds,
                        &config.sources,
                        providers,
                        caller,
                        surface,
                    )
                    .await
                }
                .await;
                tracing::debug!(
                    surface = surface.tag(),
                    source = "depot",
                    elapsed_ms = started.elapsed().as_millis(),
                    result_count = result.as_ref().map_or(0, |value| value.entries.len()),
                    failed = result.is_err(),
                    "Code Mode artifact source search finished"
                );
                result.map(Some)
            };
            let (personal_skills, personal_artifacts, depot) = join_search_sources(
                search_personal_skills,
                search_personal_artifacts,
                search_depot,
            )
            .await;
            let mut source_buckets = Vec::new();
            let mut personal_buckets = Vec::new();
            if !ready_skills.is_empty() {
                personal_buckets.push(ready_skills);
            }
            let mut incomplete_sources = Vec::new();
            match personal_skills {
                Ok(Some(result)) => {
                    personal_buckets.push(result.entries);
                    incomplete_sources.extend(result.incomplete_sources);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(surface = surface.tag(), source = "personal_labby", error = %error, "Code Mode artifact source search degraded");
                    incomplete_sources.push("personal_labby".to_owned());
                }
            }
            match personal_artifacts {
                Ok(Some(result)) => {
                    personal_buckets.push(result.entries);
                    incomplete_sources.extend(result.incomplete_sources);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(surface = surface.tag(), source = "personal_labby_artifacts", error = %error, "Code Mode personal Artifact store search degraded");
                    incomplete_sources.push("personal_labby".to_owned());
                }
            }
            if !personal_buckets.is_empty() {
                source_buckets.push(fair_merge(personal_buckets, limit));
            }
            match depot {
                Ok(Some(result)) => {
                    source_buckets.push(result.entries);
                    incomplete_sources.extend(result.incomplete_sources);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(surface = surface.tag(), source = "depot", error = %error, "Code Mode artifact source search degraded");
                    if config.sources.contains(&CodeModeSearchSource::PublicDepot) {
                        incomplete_sources.push("public_depot".to_owned());
                    }
                    if config.sources.contains(&CodeModeSearchSource::TeamDepot) {
                        incomplete_sources.push("team_depot".to_owned());
                    }
                }
            }
            if source_buckets.is_empty() && !incomplete_sources.is_empty() {
                return Err(search_error(
                    "upstream_error",
                    "all configured artifact search sources failed",
                ));
            }
            incomplete_sources.sort();
            incomplete_sources.dedup();
            Ok(ArtifactSearchResult {
                entries: fair_merge(source_buckets, limit),
                incomplete_sources,
            })
        })
    }
}

fn search_error(kind: &str, message: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: kind.into(),
        message: message.into(),
    }
}

fn project_depot_artifact_search(
    response: crate::dispatch::depot::discovery::DiscoveryResponse,
    kind: CodeModeCatalogKind,
) -> Result<Vec<CatalogDescriptor>, ToolError> {
    if response.coverage_complete {
        return Ok(response
            .items
            .into_iter()
            .filter_map(|item| depot_descriptor(item, kind))
            .collect());
    }
    // A provider need not implement every optional kind (notably all three
    // Subagent aliases). Unsupported kinds are an empty bucket, not an outage.
    if !response.failures.is_empty()
        && response
            .failures
            .iter()
            .all(|failure| failure.kind == "unsupported_kind")
    {
        return Ok(Vec::new());
    }
    Err(search_error(
        "upstream_error",
        "Depot artifact search was incomplete",
    ))
}

fn fair_merge(mut buckets: Vec<Vec<CatalogDescriptor>>, limit: usize) -> Vec<CatalogDescriptor> {
    let mut positions = vec![0; buckets.len()];
    let mut seen = BTreeSet::new();
    let mut merged = Vec::new();
    while merged.len() < limit {
        let mut advanced = false;
        for (bucket, position) in buckets.iter_mut().zip(&mut positions) {
            while let Some(item) = bucket.get(*position).cloned() {
                *position += 1;
                if seen.insert(item.id.clone()) {
                    merged.push(item);
                    advanced = true;
                    break;
                }
            }
            if merged.len() == limit {
                break;
            }
        }
        if !advanced {
            break;
        }
    }
    merged
}

#[cfg(test)]
async fn team_provider_ids(
    topology: &crate::dispatch::depot::manager::Topology,
    caller: &CodeModeCaller,
) -> Vec<String> {
    authorized_depot_providers(
        topology,
        &[CodeModeSearchSource::TeamDepot].into_iter().collect(),
        caller,
    )
    .await
    .into_iter()
    .map(|(id, _)| id)
    .collect()
}

async fn authorized_depot_providers(
    topology: &crate::dispatch::depot::manager::Topology,
    sources: &BTreeSet<CodeModeSearchSource>,
    caller: &CodeModeCaller,
) -> Vec<(String, String)> {
    // A generic custom Depot bearer is not proof of scoped Team authority.
    // Remote callers need a host-managed project binding reauthorized on every
    // search. Bound concurrent checks so one slow project DB read does not
    // serialize every other provider's authorization.
    let checks = topology
        .providers
        .values()
        .enumerate()
        .filter_map(|(index, provider)| {
            let source = if provider.view.id == PUBLIC_ID {
                CodeModeSearchSource::PublicDepot
            } else {
                CodeModeSearchSource::TeamDepot
            };
            sources
                .contains(&source)
                .then(|| (index, provider.view.clone()))
        })
        .collect::<Vec<_>>();
    let caller = Arc::new(caller.clone());
    let checks = checks.into_iter().map(|(index, provider)| {
        let caller = Arc::clone(&caller);
        async move {
            provider_visible_to_caller(&provider, &caller)
                .await
                .then_some((index, provider.id, provider.endpoint))
        }
    });
    let mut selected = stream::iter(checks)
        .buffer_unordered(DEPOT_SEARCH_CONCURRENCY)
        .filter_map(|provider| async move { provider })
        .collect::<Vec<_>>()
        .await;
    selected.sort_by_key(|(index, _, _)| *index);
    selected
        .into_iter()
        .map(|(_, id, endpoint)| (id, endpoint))
        .collect()
}

async fn bounded_authorized_depot_providers(
    topology: &crate::dispatch::depot::manager::Topology,
    sources: &BTreeSet<CodeModeSearchSource>,
    caller: &CodeModeCaller,
) -> Result<Vec<(String, String)>, ToolError> {
    tokio::time::timeout(
        DEPOT_ADMISSION_DEADLINE,
        authorized_depot_providers(topology, sources, caller),
    )
    .await
    .map_err(|_| search_error("timeout", "Depot provider authorization timed out"))
}

async fn provider_visible_to_caller(
    provider: &crate::config::depot::ProviderView,
    caller: &CodeModeCaller,
) -> bool {
    if !provider.enabled {
        return false;
    }
    if !provider.host_managed {
        return provider.id == PUBLIC_ID || matches!(caller, CodeModeCaller::TrustedLocal);
    }
    let Some(project_id) = provider.read_project_id.as_deref() else {
        return false;
    };
    if matches!(caller, CodeModeCaller::TrustedLocal) {
        return true;
    }
    match caller.authority_token() {
        Some(token) => {
            crate::mcp::code_mode_authority::authorize_artifact_discovery(token, project_id)
                .await
                .is_ok()
        }
        None => false,
    }
}

fn depot_kinds(kind: CodeModeCatalogKind) -> &'static [(&'static str, CodeModeCatalogKind)] {
    match kind {
        CodeModeCatalogKind::Skill => &[("skill", CodeModeCatalogKind::Skill)],
        CodeModeCatalogKind::Command => &[("command", CodeModeCatalogKind::Command)],
        CodeModeCatalogKind::Prompt => &[("prompt", CodeModeCatalogKind::Prompt)],
        CodeModeCatalogKind::Subagent => &[
            ("agent-definition", CodeModeCatalogKind::Subagent),
            ("agent", CodeModeCatalogKind::Subagent),
            ("agent-runtime", CodeModeCatalogKind::Subagent),
        ],
        CodeModeCatalogKind::Tool => &[("tool", CodeModeCatalogKind::Tool)],
        CodeModeCatalogKind::Snippet => &[("snippet", CodeModeCatalogKind::Snippet)],
        CodeModeCatalogKind::Resource => &[],
    }
}

fn is_local_artifact_kind(kind: &CodeModeCatalogKind) -> bool {
    matches!(
        kind,
        CodeModeCatalogKind::Skill
            | CodeModeCatalogKind::Command
            | CodeModeCatalogKind::Prompt
            | CodeModeCatalogKind::Subagent
    )
}

fn local_kind(kind: &str) -> Option<CodeModeCatalogKind> {
    match kind {
        "skill" => Some(CodeModeCatalogKind::Skill),
        "command" => Some(CodeModeCatalogKind::Command),
        "prompt" => Some(CodeModeCatalogKind::Prompt),
        "agent" | "agent-definition" | "agent-runtime" => Some(CodeModeCatalogKind::Subagent),
        _ => None,
    }
}

fn depot_descriptor(item: Value, kind: CodeModeCatalogKind) -> Option<CatalogDescriptor> {
    let item = item.as_object()?;
    let provider = item.get("providerId")?.as_str()?;
    let artifact_id = item.get("artifactId")?.as_str()?;
    let descriptor = item
        .get("descriptor")
        .and_then(Value::as_object)
        .unwrap_or(item);
    let name = descriptor
        .get("name")
        .or_else(|| descriptor.get("title"))
        .and_then(Value::as_str)?;
    let namespace = descriptor
        .get("namespace")
        .and_then(Value::as_str)
        .unwrap_or(provider);
    let description = descriptor
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(CatalogDescriptor::metadata(
        kind,
        namespace,
        &format!("depot::{provider}::{artifact_id}"),
        name,
        description,
        vec![format!("provider:{provider}")],
    ))
}

fn project_depot_skill_search(
    provider: &str,
    value: Value,
    limit: usize,
) -> Vec<CatalogDescriptor> {
    value
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(limit)
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let uri = entry.get("uri")?.as_str()?;
            let name = entry.get("name")?.as_str()?;
            let namespace = entry
                .get("namespace")
                .and_then(Value::as_str)
                .unwrap_or(provider);
            let description = entry
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some(CatalogDescriptor::metadata(
                CodeModeCatalogKind::Skill,
                namespace,
                &format!("depot::{provider}::{uri}"),
                name,
                description,
                vec![format!("provider:{provider}"), format!("uri:{uri}")],
            ))
        })
        .collect()
}

fn frontmatter_tags(frontmatter: &serde_json::Map<String, Value>) -> Vec<String> {
    match frontmatter.get("tags") {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn descriptor_matches(
    descriptor: &labby_runtime::artifacts::ArtifactDescriptor,
    normalized_query: &str,
) -> bool {
    normalized_query.is_empty()
        || descriptor
            .name
            .to_ascii_lowercase()
            .contains(normalized_query)
        || descriptor
            .namespace
            .to_ascii_lowercase()
            .contains(normalized_query)
        || descriptor
            .title
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(normalized_query))
        || descriptor
            .description
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(normalized_query))
        || descriptor
            .tags
            .iter()
            .any(|value| value.to_ascii_lowercase().contains(normalized_query))
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_codemode::CodeModeCallerCapabilities;

    #[tokio::test]
    async fn indexed_hit_does_not_wait_for_slow_personal_scan() {
        let started = std::time::Instant::now();
        let personal = async {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            Ok(Some(ArtifactSearchResult::default()))
        };
        let depot = async {
            Ok(Some(ArtifactSearchResult {
                entries: vec![CatalogDescriptor::metadata(
                    CodeModeCatalogKind::Skill,
                    "public",
                    "depot::public::target",
                    "target",
                    "",
                    Vec::new(),
                )],
                incomplete_sources: Vec::new(),
            }))
        };
        let artifacts = async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(Some(ArtifactSearchResult {
                entries: vec![CatalogDescriptor::metadata(
                    CodeModeCatalogKind::Prompt,
                    "labby",
                    "labby::local",
                    "local",
                    "",
                    Vec::new(),
                )],
                incomplete_sources: Vec::new(),
            }))
        };
        let (personal, artifacts, depot) = join_search_sources(personal, artifacts, depot).await;
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(
            personal.is_err(),
            "slow source should be reported incomplete"
        );
        assert_eq!(depot.unwrap().unwrap().entries.len(), 1);
        assert_eq!(artifacts.unwrap().unwrap().entries[0].id, "labby::local");
    }

    #[tokio::test]
    async fn empty_index_search_does_not_wait_for_slow_personal_scan() {
        let started = std::time::Instant::now();
        let personal = async {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            Ok(Some(ArtifactSearchResult::default()))
        };
        let depot = async { Ok(Some(ArtifactSearchResult::default())) };
        let artifacts = async { Ok(Some(ArtifactSearchResult::default())) };
        let (personal, _, depot) = join_search_sources(personal, artifacts, depot).await;
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(
            personal.is_err(),
            "slow source should be reported incomplete"
        );
        assert!(depot.unwrap().unwrap().entries.is_empty());
    }

    #[tokio::test]
    #[cfg(feature = "proxy-testkit")]
    async fn personal_skill_search_skips_depot_mcp_when_index_is_configured() {
        use crate::skills::aggregate::ToolAccess;
        use crate::skills::facade::{
            SkillCallerScope, SkillRegistryContext, register_code_mode_skill_context,
        };
        use labby_gateway::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
        use labby_runtime::gateway_config::{GatewayConfig, UpstreamConfig};

        let temp = tempfile::tempdir().unwrap();
        let depot_endpoint = crate::config::depot::PUBLIC_ENDPOINT;
        let gateway = Arc::new(GatewayManager::new(
            temp.path().join("config.toml"),
            GatewayRuntimeHandle::default(),
        ));
        let upstream: UpstreamConfig = serde_json::from_value(serde_json::json!({
            "name": "public-depot",
            "url": "http://127.0.0.1:49999/mcp",
            "proxy_skills": true
        }))
        .unwrap();
        gateway
            .seed_config_unchecked_for_tests(GatewayConfig {
                upstream: vec![upstream],
                ..GatewayConfig::default()
            })
            .await;
        let guard = register_code_mode_skill_context(SkillRegistryContext::with_manager(
            gateway,
            SkillCallerScope::root(Some("reader".into()), ToolAccess::CodeModeOnly),
        ));
        let caller = CodeModeCaller::ScopedSkills {
            capabilities: CodeModeCallerCapabilities {
                can_read: true,
                can_execute: true,
                ..Default::default()
            },
            sub: Some("reader".into()),
            skill_context_token: guard.token().to_owned(),
        };
        let preferences = crate::config::depot::DepotPreferences::default();
        let provider = ProductCodeModeArtifactSearchProvider {
            depot: Arc::new(Manager::new(
                &preferences,
                crate::dispatch::depot::manager::SecretSnapshot::default(),
                Default::default(),
            )),
            artifacts: Arc::new(ArtifactStore::new(temp.path().join("artifacts")).unwrap()),
        };
        assert!(
            provider
                .depot
                .snapshot()
                .providers
                .values()
                .any(|entry| entry.view.endpoint == depot_endpoint)
        );

        let mut config = CodeModeSearchConfig::default();
        config
            .depot_skill_upstreams
            .insert("public-depot".into(), PUBLIC_ID.into());
        let result = provider
            .personal_skills("deep-skill", 1, &caller, &ToolScope::default(), &config)
            .await;
        let result = result.unwrap();
        assert!(
            result.entries.is_empty(),
            "Depot MCP discovery must not gate indexed search"
        );
        assert!(
            result.incomplete_sources.is_empty(),
            "Depot MCP must not be scanned through an alternate hostname"
        );

        config.sources = [CodeModeSearchSource::PersonalLabby].into_iter().collect();
        let personal_only = provider
            .personal_skills("deep-skill", 1, &caller, &ToolScope::default(), &config)
            .await
            .unwrap();
        assert_eq!(personal_only.incomplete_sources, ["personal_labby"]);

        config.sources.insert(CodeModeSearchSource::PublicDepot);
        config.depot_skill_upstreams.clear();
        let unbound = provider
            .personal_skills("deep-skill", 1, &caller, &ToolScope::default(), &config)
            .await
            .unwrap();
        assert_eq!(unbound.incomplete_sources, ["personal_labby"]);
    }

    #[tokio::test]
    #[cfg(feature = "proxy-testkit")]
    async fn personal_skill_search_keeps_first_party_hits_when_other_upstream_fails() {
        use crate::skills::aggregate::ToolAccess;
        use crate::skills::facade::{
            SkillCallerScope, SkillRegistryContext, register_code_mode_skill_context,
        };
        use labby_gateway::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
        use labby_runtime::gateway_config::{GatewayConfig, UpstreamConfig};

        let temp = tempfile::tempdir().unwrap();
        let gateway = Arc::new(GatewayManager::new(
            temp.path().join("config.toml"),
            GatewayRuntimeHandle::default(),
        ));
        let upstream: UpstreamConfig = serde_json::from_value(serde_json::json!({
            "name": "other-skills", "url": "http://127.0.0.1:49999/mcp", "proxy_skills": true
        }))
        .unwrap();
        gateway
            .seed_config_unchecked_for_tests(GatewayConfig {
                upstream: vec![upstream],
                ..GatewayConfig::default()
            })
            .await;
        let guard = register_code_mode_skill_context(SkillRegistryContext::with_manager(
            gateway,
            SkillCallerScope::root(Some("reader".into()), ToolAccess::CodeModeOnly),
        ));
        let caller = CodeModeCaller::ScopedSkills {
            capabilities: CodeModeCallerCapabilities {
                can_read: true,
                can_execute: true,
                ..Default::default()
            },
            sub: Some("reader".into()),
            skill_context_token: guard.token().to_owned(),
        };
        let provider = ProductCodeModeArtifactSearchProvider {
            depot: Arc::new(Manager::default()),
            artifacts: Arc::new(ArtifactStore::new(temp.path().join("artifacts")).unwrap()),
        };
        let hits = provider
            .personal_skills(
                "using-labby",
                5,
                &caller,
                &ToolScope::default(),
                &CodeModeSearchConfig::default(),
            )
            .await
            .unwrap();
        assert!(hits.entries.iter().any(|hit| hit.name == "using-labby"));
        assert_eq!(hits.incomplete_sources, ["personal_labby"]);
    }

    #[tokio::test]
    #[cfg(feature = "proxy-testkit")]
    async fn ready_first_party_skill_survives_stalled_proxy_source() {
        use crate::skills::aggregate::ToolAccess;
        use crate::skills::facade::{
            SkillCallerScope, SkillRegistryContext, register_code_mode_skill_context,
        };
        use labby_gateway::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
        use labby_gateway::upstream::pool::UpstreamPool;
        use labby_runtime::gateway_config::{GatewayConfig, UpstreamConfig};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let runtime = GatewayRuntimeHandle::default();
        runtime.swap(Some(Arc::new(UpstreamPool::new()))).await;
        let temp = tempfile::tempdir().unwrap();
        let manager = Arc::new(GatewayManager::new(
            temp.path().join("config.toml"),
            runtime,
        ));
        let upstream: UpstreamConfig = serde_json::from_value(serde_json::json!({
            "name":"stalled", "url":format!("http://{address}/mcp"), "proxy_skills":true
        }))
        .unwrap();
        manager
            .seed_config_unchecked_for_tests(GatewayConfig {
                upstream: vec![upstream],
                ..Default::default()
            })
            .await;
        let guard = register_code_mode_skill_context(SkillRegistryContext::with_manager(
            manager,
            SkillCallerScope::root(Some("reader".into()), ToolAccess::CodeModeOnly),
        ));
        let caller = CodeModeCaller::ScopedSkills {
            capabilities: CodeModeCallerCapabilities {
                can_read: true,
                can_execute: true,
                ..Default::default()
            },
            sub: Some("reader".into()),
            skill_context_token: guard.token().into(),
        };
        let provider = ProductCodeModeArtifactSearchProvider {
            depot: Arc::new(Manager::default()),
            artifacts: Arc::new(ArtifactStore::new(temp.path().join("artifacts")).unwrap()),
        };
        let config = CodeModeSearchConfig {
            sources: [
                CodeModeSearchSource::PersonalLabby,
                CodeModeSearchSource::PublicDepot,
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            provider.search(
                "using-labby",
                5,
                &[CodeModeCatalogKind::Skill],
                &config,
                &caller,
                CodeModeSurface::Mcp,
                &ToolScope::default(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            result
                .entries
                .iter()
                .any(|entry| entry.name == "using-labby"),
            "ready authorized first-party matches must survive stalled proxy search"
        );
        assert!(result.incomplete_sources.contains(&"personal_labby".into()));
    }

    #[tokio::test]
    async fn non_artifact_kinds_skip_personal_store_access() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(temp.path().join("store")).unwrap();
        std::fs::remove_dir(artifacts.root().join("artifacts")).unwrap();
        let provider = ProductCodeModeArtifactSearchProvider {
            depot: Arc::new(Manager::default()),
            artifacts: Arc::new(artifacts),
        };
        let config = CodeModeSearchConfig {
            sources: [CodeModeSearchSource::PersonalLabby].into_iter().collect(),
            ..CodeModeSearchConfig::default()
        };
        let result = provider
            .search(
                "needle",
                5,
                &[
                    CodeModeCatalogKind::Tool,
                    CodeModeCatalogKind::Snippet,
                    CodeModeCatalogKind::Resource,
                ],
                &config,
                &CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                &ToolScope::default(),
            )
            .await
            .unwrap();
        assert!(result.entries.is_empty());
        assert!(
            result.incomplete_sources.is_empty(),
            "unrequested local kinds must not read the store"
        );
    }

    #[tokio::test]
    async fn personal_artifact_store_failure_keeps_first_party_skill_hits() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(temp.path().join("store")).unwrap();
        std::fs::remove_dir(artifacts.root().join("artifacts")).unwrap();
        let provider = ProductCodeModeArtifactSearchProvider {
            depot: Arc::new(Manager::default()),
            artifacts: Arc::new(artifacts),
        };
        let config = CodeModeSearchConfig {
            sources: [CodeModeSearchSource::PersonalLabby].into_iter().collect(),
            ..CodeModeSearchConfig::default()
        };
        let result = provider
            .search(
                "using-labby",
                5,
                &[CodeModeCatalogKind::Skill],
                &config,
                &CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                &ToolScope::default(),
            )
            .await
            .unwrap();
        assert!(
            result
                .entries
                .iter()
                .any(|entry| entry.name == "using-labby")
        );
        assert_eq!(result.incomplete_sources, ["personal_labby"]);
    }

    #[tokio::test]
    async fn depot_search_keeps_public_results_when_team_index_fails() {
        use axum::{
            Json, Router,
            http::{HeaderMap, StatusCode},
            routing::{get, post},
        };

        let router = Router::new()
            .route("/api/discovery", get(|| async {
                Json(serde_json::json!({
                    "contractVersion": "depot.discovery/v1",
                    "deploymentId": "catalog", "deploymentEpoch": "boot",
                    "authorityEpoch": "read", "listingEpoch": "1",
                    "snapshotContinuations": true, "maxPageSize": 200
                }))
            }))
            .route("/api/operations/depot.skills.search", post(|headers: HeaderMap| async move {
                if headers.get("authorization").and_then(|value| value.to_str().ok()) == Some("Bearer team-token") {
                    return (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error":"unavailable"})));
                }
                (StatusCode::OK, Json(serde_json::json!({"result": {
                    "results": [{"uri":"skill://depot/public/deep/SKILL.md", "namespace":"public", "name":"deep-public", "description":"indexed"}]
                }})))
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let preferences = crate::config::depot::DepotPreferences {
            read_project_id: Some("catalog-project".into()),
            public_read_binding: Some(crate::config::depot::PublicReadBinding {
                endpoint: endpoint.clone(),
                bearer_token_env: "LABBY_DEPOT_TEST_PUBLIC_TOKEN".into(),
                deployment_id: "catalog".to_owned().try_into().unwrap(),
            }),
            local_providers: vec![crate::config::depot::LocalProviderConfig {
                id: "team".into(),
                name: "Team".into(),
                endpoint,
                bearer_token_env: "LABBY_DEPOT_TEST_TEAM_TOKEN".into(),
            }],
            ..Default::default()
        };
        let secrets = crate::dispatch::depot::manager::SecretSnapshot::from_values(
            [
                (
                    "LABBY_DEPOT_TEST_PUBLIC_TOKEN".into(),
                    "public-token".into(),
                ),
                ("LABBY_DEPOT_TEST_TEAM_TOKEN".into(), "team-token".into()),
            ]
            .into_iter()
            .collect(),
        );
        let temp = tempfile::tempdir().unwrap();
        let provider = ProductCodeModeArtifactSearchProvider {
            depot: Arc::new(Manager::new(&preferences, secrets, Default::default())),
            artifacts: Arc::new(ArtifactStore::new(temp.path().join("artifacts")).unwrap()),
        };
        let results = provider
            .depot_artifacts(
                "deep",
                5,
                &[CodeModeCatalogKind::Skill],
                &[
                    CodeModeSearchSource::PublicDepot,
                    CodeModeSearchSource::TeamDepot,
                ]
                .into_iter()
                .collect(),
                &CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
            )
            .await
            .unwrap();
        assert_eq!(
            results
                .entries
                .iter()
                .map(|result| result.name.as_str())
                .collect::<Vec<_>>(),
            ["deep-public"]
        );
        assert_eq!(results.incomplete_sources, ["team_depot"]);
        server.abort();
    }

    #[tokio::test]
    async fn depot_skill_search_uses_each_provider_index_with_query_and_limit() {
        use axum::{
            Json, Router,
            extract::State,
            http::HeaderMap,
            routing::{get, post},
        };
        use std::sync::Mutex;

        let calls = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let router = Router::new()
            .route(
                "/api/discovery",
                get(|| async {
                    Json(serde_json::json!({
                        "contractVersion": "depot.discovery/v1",
                        "deploymentId": "catalog",
                        "deploymentEpoch": "boot",
                        "authorityEpoch": "read",
                        "listingEpoch": "1",
                        "snapshotContinuations": true,
                        "maxPageSize": 200
                    }))
                }),
            )
            .route(
                "/api/operations/depot.skills.search",
                post(
                    |State(calls): State<Arc<Mutex<Vec<(String, Value)>>>>,
                     headers: HeaderMap,
                     Json(request): Json<Value>| async move {
                        let bearer = headers
                            .get("authorization")
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .to_owned();
                        calls.lock().unwrap().push((bearer.clone(), request));
                        let provider = if bearer == "Bearer public-token" {
                            "public"
                        } else {
                            "team"
                        };
                        Json(serde_json::json!({"result": {
                            "query": "deep skill",
                            "results": [{
                                "uri": format!("skill://depot/{provider}/skill-70001/SKILL.md"),
                                "namespace": provider,
                                "name": format!("{provider}-deep-skill"),
                                "description": "result past the first catalog page"
                            }]
                        }}))
                    },
                ),
            )
            .with_state(Arc::clone(&calls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let preferences = crate::config::depot::DepotPreferences {
            read_project_id: Some("catalog-project".into()),
            public_read_binding: Some(crate::config::depot::PublicReadBinding {
                endpoint: endpoint.clone(),
                bearer_token_env: "LABBY_DEPOT_TEST_PUBLIC_TOKEN".into(),
                deployment_id: "catalog".to_owned().try_into().unwrap(),
            }),
            local_providers: vec![crate::config::depot::LocalProviderConfig {
                id: "team".into(),
                name: "Team".into(),
                endpoint,
                bearer_token_env: "LABBY_DEPOT_TEST_TEAM_TOKEN".into(),
            }],
            ..Default::default()
        };
        let secrets = crate::dispatch::depot::manager::SecretSnapshot::from_values(
            [
                (
                    "LABBY_DEPOT_TEST_PUBLIC_TOKEN".into(),
                    "public-token".into(),
                ),
                ("LABBY_DEPOT_TEST_TEAM_TOKEN".into(), "team-token".into()),
            ]
            .into_iter()
            .collect(),
        );
        let temp = tempfile::tempdir().unwrap();
        let provider = ProductCodeModeArtifactSearchProvider {
            depot: Arc::new(Manager::new(&preferences, secrets, Default::default())),
            artifacts: Arc::new(ArtifactStore::new(temp.path().join("artifacts")).unwrap()),
        };
        let results = provider
            .depot_artifacts(
                "deep skill",
                51,
                &[CodeModeCatalogKind::Skill],
                &[
                    CodeModeSearchSource::PublicDepot,
                    CodeModeSearchSource::TeamDepot,
                ]
                .into_iter()
                .collect(),
                &CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
            )
            .await
            .unwrap();

        assert_eq!(results.entries.len(), 2);
        assert!(
            results
                .entries
                .iter()
                .any(|result| result.name == "public-deep-skill")
        );
        assert!(
            results
                .entries
                .iter()
                .any(|result| result.name == "team-deep-skill")
        );
        let short = provider
            .depot_artifacts(
                "de",
                51,
                &[CodeModeCatalogKind::Skill],
                &[
                    CodeModeSearchSource::PublicDepot,
                    CodeModeSearchSource::TeamDepot,
                ]
                .into_iter()
                .collect(),
                &CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
            )
            .await
            .unwrap();
        assert_eq!(short.entries.len(), 2);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 4, "search must not traverse catalog pages");
        assert!(
            calls
                .iter()
                .any(|(bearer, _)| bearer == "Bearer public-token")
        );
        assert!(
            calls
                .iter()
                .any(|(bearer, _)| bearer == "Bearer team-token")
        );
        assert_eq!(
            calls
                .iter()
                .filter(|(_, request)| request
                    == &serde_json::json!({"query": "deep skill", "limit": 51}))
                .count(),
            2
        );
        assert_eq!(
            calls
                .iter()
                .filter(|(_, request)| request == &serde_json::json!({"query": "de", "limit": 51}))
                .count(),
            2
        );
        server.abort();
    }

    #[test]
    fn user_kinds_map_to_exact_depot_indexes() {
        assert_eq!(
            depot_kinds(CodeModeCatalogKind::Tool),
            &[("tool", CodeModeCatalogKind::Tool)]
        );
        assert_eq!(
            depot_kinds(CodeModeCatalogKind::Snippet),
            &[("snippet", CodeModeCatalogKind::Snippet)]
        );
        assert_eq!(
            depot_kinds(CodeModeCatalogKind::Skill),
            &[("skill", CodeModeCatalogKind::Skill)]
        );
        assert_eq!(
            depot_kinds(CodeModeCatalogKind::Command),
            &[("command", CodeModeCatalogKind::Command)]
        );
        assert_eq!(
            depot_kinds(CodeModeCatalogKind::Prompt),
            &[("prompt", CodeModeCatalogKind::Prompt)]
        );
        assert_eq!(depot_kinds(CodeModeCatalogKind::Subagent).len(), 3);
    }

    #[test]
    fn projected_depot_identity_includes_provider() {
        let descriptor = depot_descriptor(
            serde_json::json!({
                "providerId": "public",
                "artifactId": "skill-70001",
                "descriptor": {
                    "name": "deep-skill",
                    "namespace": "community",
                    "description": "indexed result"
                }
            }),
            CodeModeCatalogKind::Skill,
        )
        .unwrap();
        assert_eq!(descriptor.id, "depot::public::skill-70001");
        assert_eq!(descriptor.namespace, "community");
    }

    #[test]
    fn native_depot_skill_search_projects_deep_index_result_without_listing() {
        let results = project_depot_skill_search(
            "public",
            serde_json::json!({
                "query": "deep skill",
                "results": [{
                    "uri": "skill://depot/community/skill-70001/SKILL.md",
                    "namespace": "community",
                    "name": "skill-70001",
                    "description": "returned directly by Depot's search index"
                }]
            }),
            5,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "skill-70001");
        assert!(
            results[0]
                .tags
                .contains(&"uri:skill://depot/community/skill-70001/SKILL.md".into())
        );
    }

    #[test]
    fn unsupported_optional_depot_kind_is_empty_but_other_incompleteness_fails() {
        let response = |failure_kind: &str| {
            serde_json::from_value(serde_json::json!({
                "schemaVersion": "1", "scope": "public", "scopeEpoch": "epoch",
                "items": [], "providerOutcomes": [{"providerId": "public", "state": "failed"}],
                "failures": [{"providerId": "public", "kind": failure_kind}],
                "coverageComplete": false, "knownTotal": null, "totalIsExact": false,
                "state": "partial", "nextCursor": null
            }))
            .unwrap()
        };
        assert!(
            project_depot_artifact_search(
                response("unsupported_kind"),
                CodeModeCatalogKind::Subagent
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            project_depot_artifact_search(
                response("upstream_error"),
                CodeModeCatalogKind::Subagent
            )
            .is_err()
        );
    }

    #[test]
    fn fair_merge_does_not_let_first_source_consume_the_budget() {
        let entry = |id: &str| {
            CatalogDescriptor::metadata(CodeModeCatalogKind::Skill, "test", id, id, "", Vec::new())
        };
        let merged = fair_merge(
            vec![
                vec![entry("personal-1"), entry("personal-2")],
                vec![entry("public-1"), entry("public-2")],
                vec![entry("team-1"), entry("team-2")],
            ],
            3,
        );
        assert_eq!(
            merged
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["personal-1", "public-1", "team-1"]
        );
    }

    #[tokio::test]
    async fn delayed_public_requests_do_not_starve_queued_team_requests() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let requests = (0..12).map(|index| {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            async move {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                // The first six model Public's default Skill, Command, Prompt,
                // and Subagent index requests; the queued six model Team.
                if index < 6 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                active.fetch_sub(1, Ordering::SeqCst);
                let (source, namespace) = if index < 6 {
                    (0, "public")
                } else {
                    (1, "team")
                };
                (
                    source,
                    vec![CatalogDescriptor::metadata(
                        CodeModeCatalogKind::Skill,
                        namespace,
                        &format!("{namespace}-{index}"),
                        &format!("{namespace}-{index}"),
                        "",
                        Vec::new(),
                    )],
                )
            }
        });

        let (completed, pending) =
            bounded_ordered_until(requests.collect(), Duration::from_secs(1)).await;
        assert!(pending.is_empty());
        let buckets = completed
            .into_iter()
            .map(|(_, bucket)| bucket)
            .collect::<Vec<_>>();

        assert!(peak.load(Ordering::SeqCst) <= DEPOT_SEARCH_CONCURRENCY);
        assert!(buckets[..6].iter().all(|(source, _)| *source == 0));
        assert!(buckets[6..].iter().all(|(source, _)| *source == 1));
        for limit in 2..=5 {
            let merged = merge_source_buckets(buckets.clone(), 2, limit);
            assert_eq!(merged.len(), limit);
            assert!(merged.iter().any(|entry| entry.namespace == "public"));
            assert!(merged.iter().any(|entry| entry.namespace == "team"));
        }
    }

    #[tokio::test]
    async fn bounded_depot_requests_keep_fast_bucket_when_another_stalls() {
        let started = std::time::Instant::now();
        let requests = (0..2).map(|index| async move {
            if index == 1 {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
            index
        });
        let (completed, missing) =
            bounded_ordered_until(requests.collect(), std::time::Duration::from_millis(100)).await;
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(completed, [(0, 0)]);
        assert_eq!(missing, [1]);
    }

    #[tokio::test]
    async fn stalled_public_buckets_do_not_starve_fast_team_skill() {
        use std::time::Duration;

        let make_requests = |source| {
            (0..6)
                .map(|_| async move {
                    if source == 0 {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                    source
                })
                .collect()
        };
        let requests = interleave_sources(vec![make_requests(0), make_requests(1)]);
        let (completed, pending) =
            bounded_ordered_until(requests, Duration::from_millis(100)).await;
        assert!(
            completed
                .iter()
                .any(|(index, source)| *index == 1 && *source == 1)
        );
        assert!(pending.contains(&0));
    }

    #[tokio::test]
    async fn stalled_team_providers_do_not_starve_later_fast_provider() {
        use std::time::Duration;

        let requests = (0..5).map(|index| async move {
            if index < DEPOT_SEARCH_CONCURRENCY {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            index
        });
        let (completed, pending) =
            bounded_ordered_until(requests.collect(), Duration::from_secs(2)).await;
        assert_eq!(completed, [(4, 4)]);
        assert_eq!(pending, [0, 1, 2, 3]);
    }

    #[tokio::test]
    async fn scoped_caller_without_request_authority_gets_no_team_rows() {
        let mut preferences = crate::config::depot::DepotPreferences {
            public_enabled: false,
            read_project_id: Some("project-a".into()),
            ..Default::default()
        };
        preferences
            .local_providers
            .push(crate::config::depot::LocalProviderConfig {
                id: "team".into(),
                name: "Team".into(),
                endpoint: "http://127.0.0.1:9".into(),
                bearer_token_env: "LABBY_DEPOT_TEST_TEAM_TOKEN".into(),
            });
        let secrets = crate::dispatch::depot::manager::SecretSnapshot::from_values(
            [("LABBY_DEPOT_TEST_TEAM_TOKEN".into(), "secret".into())]
                .into_iter()
                .collect(),
        );
        let provider = ProductCodeModeArtifactSearchProvider {
            depot: Arc::new(Manager::new(&preferences, secrets, Default::default())),
            artifacts: Arc::new(
                ArtifactStore::new(tempfile::tempdir().unwrap().path().join("artifacts")).unwrap(),
            ),
        };
        let caller = CodeModeCaller::Scoped {
            capabilities: CodeModeCallerCapabilities {
                can_read: true,
                ..Default::default()
            },
            sub: Some("reader".into()),
        };
        let results = provider
            .depot_artifacts(
                "indexed skill",
                10,
                &[CodeModeCatalogKind::Skill],
                &[CodeModeSearchSource::TeamDepot].into_iter().collect(),
                &caller,
                CodeModeSurface::Mcp,
            )
            .await;
        assert!(results.unwrap().entries.is_empty());
    }

    #[tokio::test]
    async fn scoped_caller_without_request_authority_cannot_use_bound_public_provider() {
        let caller = CodeModeCaller::Scoped {
            capabilities: CodeModeCallerCapabilities {
                can_read: true,
                ..Default::default()
            },
            sub: Some("reader".into()),
        };
        let bound = crate::config::depot::ProviderView {
            id: PUBLIC_ID.into(),
            name: "Bound Public".into(),
            endpoint: "http://127.0.0.1:9".into(),
            enabled: true,
            auth_mode: crate::config::depot::AuthMode::Bearer,
            bearer_token_env: Some("LABBY_TEST_PUBLIC_TOKEN".into()),
            host_managed: true,
            read_project_id: Some("project-a".into()),
            expected_deployment_id: None,
        };
        assert!(!provider_visible_to_caller(&bound, &caller).await);

        let anonymous = crate::config::depot::ProviderView {
            host_managed: false,
            read_project_id: None,
            auth_mode: crate::config::depot::AuthMode::Anonymous,
            bearer_token_env: None,
            ..bound
        };
        assert!(provider_visible_to_caller(&anonymous, &caller).await);
        assert!(
            !provider_visible_to_caller(
                &crate::config::depot::ProviderView {
                    enabled: false,
                    ..anonymous
                },
                &caller,
            )
            .await
        );
    }

    #[tokio::test]
    async fn configured_team_provider_is_local_only_without_project_binding() {
        let team = crate::config::depot::ProviderView {
            id: "team".into(),
            name: "Configured Team".into(),
            endpoint: "https://team.example.test".into(),
            enabled: true,
            auth_mode: crate::config::depot::AuthMode::Bearer,
            bearer_token_env: Some("LABBY_DEPOT_TEST_TEAM_TOKEN".into()),
            host_managed: false,
            read_project_id: None,
            expected_deployment_id: None,
        };
        assert!(provider_visible_to_caller(&team, &CodeModeCaller::TrustedLocal).await);

        let scoped = CodeModeCaller::Scoped {
            capabilities: CodeModeCallerCapabilities {
                can_read: true,
                ..Default::default()
            },
            sub: Some("reader".into()),
        };
        assert!(!provider_visible_to_caller(&team, &scoped).await);

        let preferences = crate::config::depot::DepotPreferences {
            providers: vec![
                toml::Value::try_from(crate::config::depot::ProviderConfig {
                    id: "team".into(),
                    name: "Configured Team".into(),
                    endpoint: "https://team.example.test".into(),
                    enabled: true,
                    auth_mode: crate::config::depot::AuthMode::Bearer,
                    bearer_token_env: Some("LABBY_DEPOT_TEST_TEAM_TOKEN".into()),
                })
                .unwrap(),
            ],
            ..Default::default()
        };
        let manager = Manager::new(
            &preferences,
            crate::dispatch::depot::manager::SecretSnapshot::from_values(
                [("LABBY_DEPOT_TEST_TEAM_TOKEN".into(), "secret".into())]
                    .into_iter()
                    .collect(),
            ),
            Default::default(),
        );
        let topology = manager.snapshot();
        assert_eq!(
            team_provider_ids(&topology, &CodeModeCaller::TrustedLocal).await,
            ["team"]
        );
        assert!(team_provider_ids(&topology, &scoped).await.is_empty());
    }
}
