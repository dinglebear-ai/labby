//! Product-owned, query-driven artifact discovery for Code Mode.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::{StreamExt, stream};
use labby_codemode::{
    CatalogDescriptor, CodeModeCaller, CodeModeCatalogKind, CodeModeSurface, ToolScope,
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

const MAX_SEARCH_RESULTS: usize = 50;

enum DepotSearchRequest {
    Skills(String),
    Artifacts(String, &'static str, CodeModeCatalogKind),
}

const DEPOT_SEARCH_CONCURRENCY: usize = 4;

async fn bounded_ordered<T, F>(futures: Vec<F>) -> Vec<T>
where
    F: Future<Output = T>,
{
    let mut completed = stream::iter(futures.into_iter().enumerate())
        .map(|(index, future)| async move { (index, future.await) })
        .buffer_unordered(DEPOT_SEARCH_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    completed.sort_by_key(|(index, _)| *index);
    completed.into_iter().map(|(_, value)| value).collect()
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

    async fn personal_skills(
        &self,
        query: &str,
        limit: usize,
        caller: &CodeModeCaller,
        scope: &ToolScope,
    ) -> Vec<CatalogDescriptor> {
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
        let Some(context) = context else {
            return Vec::new();
        };
        let context = context.narrowed_to_upstreams(scope.allowed_namespaces());
        let listing = search_visible_skills_bounded(&context, query, limit).await;
        listing
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
            .collect()
    }

    fn personal_artifacts(
        &self,
        query: &str,
        limit: usize,
        kinds: &[CodeModeCatalogKind],
        caller: &CodeModeCaller,
    ) -> Vec<CatalogDescriptor> {
        let Ok(records) = self.artifacts.list_records() else {
            return Vec::new();
        };
        let records = records
            .into_iter()
            .filter(|record| {
                caller.is_admin()
                    || (record.publication.state == PublicationState::Published
                        && record.publication.visibility == Visibility::Public)
            })
            .collect::<Vec<_>>();
        let buckets = kinds
            .iter()
            .copied()
            .filter(|kind| {
                matches!(
                    kind,
                    CodeModeCatalogKind::Skill
                        | CodeModeCatalogKind::Command
                        | CodeModeCatalogKind::Prompt
                        | CodeModeCatalogKind::Subagent
                )
            })
            .map(|kind| {
                records
                    .iter()
                    .filter(|record| local_kind(&record.descriptor.kind) == Some(kind))
                    .filter(|record| descriptor_matches(&record.descriptor, query))
                    .map(|record| {
                        let descriptor = &record.descriptor;
                        CatalogDescriptor::metadata(
                            kind,
                            &descriptor.namespace,
                            &format!("labby::{}", descriptor.id),
                            &descriptor.name,
                            descriptor.description.as_deref().unwrap_or_default(),
                            descriptor.tags.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        fair_merge(buckets, limit)
    }

    async fn depot_artifacts(
        &self,
        query: &str,
        limit: usize,
        kinds: &[CodeModeCatalogKind],
        sources: &BTreeSet<CodeModeSearchSource>,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
    ) -> Vec<CatalogDescriptor> {
        if query.trim().chars().count() < 3 {
            return Vec::new();
        }
        let topology = self.depot.snapshot();
        let mut groups = Vec::new();
        if sources.contains(&CodeModeSearchSource::PublicDepot)
            && let Some(public) = topology.providers.get(PUBLIC_ID)
            && provider_visible_to_caller(&public.view, caller).await
        {
            groups.push(vec![PUBLIC_ID.to_owned()]);
        }
        if sources.contains(&CodeModeSearchSource::TeamDepot) {
            let team = team_provider_ids(&topology, caller).await;
            if !team.is_empty() {
                groups.push(team);
            }
        }
        let indexed_skill_search = kinds.contains(&CodeModeCatalogKind::Skill);
        let depot_kinds = kinds
            .iter()
            .filter(|kind| **kind != CodeModeCatalogKind::Skill)
            .flat_map(|kind| depot_kinds(*kind))
            .copied()
            .collect::<Vec<_>>();
        if groups.is_empty() || (!indexed_skill_search && depot_kinds.is_empty()) {
            return Vec::new();
        }
        let actor = format!(
            "codemode:{}:{}",
            surface.tag(),
            hex::encode(Sha256::digest(
                caller.subject().unwrap_or("trusted-local").as_bytes()
            ))
        );
        let provider_count = groups.iter().map(Vec::len).sum::<usize>();
        let request_count = provider_count
            .saturating_mul(depot_kinds.len() + usize::from(indexed_skill_search))
            .max(1);
        let page_limit = limit.div_ceil(request_count).clamp(1, MAX_SEARCH_RESULTS) as u16;
        let mut requests = Vec::new();
        for (source, providers) in groups.iter().enumerate() {
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
        }
        let Ok(admission) = self
            .depot
            .scheduler
            .admit(&actor, tokio::time::Instant::now())
            .await
        else {
            return Vec::new();
        };
        let topology = &topology;
        let admission = &admission;
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
                                Ok(value) => project_depot_skill_search(
                                    &provider_id,
                                    value,
                                    usize::from(page_limit),
                                ),
                                Err(error) => {
                                    tracing::warn!(
                                        surface = surface.tag(),
                                        provider = %provider_id,
                                        error = %error,
                                        "Code Mode Depot Skill index search degraded"
                                    );
                                    Vec::new()
                                }
                            }
                        } else {
                            Vec::new()
                        }
                    }
                    DepotSearchRequest::Artifacts(provider_id, wire_kind, catalog_kind) => {
                        match crate::dispatch::depot::discovery::discover_selected_admitted(
                            &self.depot,
                            &[provider_id],
                            query,
                            wire_kind,
                            page_limit,
                            &admission,
                        )
                        .await
                        {
                            Ok(response) => response
                                .items
                                .into_iter()
                                .filter_map(|item| depot_descriptor(item, catalog_kind))
                                .collect(),
                            Err(error) => {
                                tracing::warn!(
                                    surface = surface.tag(),
                                    artifact_kind = wire_kind,
                                    error = %error,
                                    "Code Mode Depot search provider degraded"
                                );
                                Vec::new()
                            }
                        }
                    }
                };
                (source, bucket)
            })
            .collect();
        let request_buckets = bounded_ordered(request_futures).await;
        merge_source_buckets(request_buckets, groups.len(), limit)
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
    ) -> Pin<Box<dyn Future<Output = Result<Vec<CatalogDescriptor>, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let limit = limit.clamp(1, MAX_SEARCH_RESULTS);
            let mut source_buckets = Vec::new();
            if config
                .sources
                .contains(&CodeModeSearchSource::PersonalLabby)
            {
                let mut personal_buckets = Vec::new();
                if kinds.contains(&CodeModeCatalogKind::Skill) {
                    personal_buckets.push(self.personal_skills(query, limit, caller, scope).await);
                }
                personal_buckets.push(self.personal_artifacts(query, limit, kinds, caller));
                source_buckets.push(fair_merge(personal_buckets, limit));
            }
            if config.sources.contains(&CodeModeSearchSource::PublicDepot)
                || config.sources.contains(&CodeModeSearchSource::TeamDepot)
            {
                source_buckets.push(
                    self.depot_artifacts(query, limit, kinds, &config.sources, caller, surface)
                        .await,
                );
            }
            Ok(fair_merge(source_buckets, limit))
        })
    }
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

async fn team_provider_ids(
    topology: &crate::dispatch::depot::manager::Topology,
    caller: &CodeModeCaller,
) -> Vec<String> {
    let mut selected = Vec::new();
    for provider in topology.providers.values() {
        // A generic custom Depot bearer is not proof of scoped Team authority.
        // Trusted-local execution may use explicitly configured providers;
        // remote scoped callers require a host-managed project binding
        // reauthorized on every search.
        if provider.view.id != PUBLIC_ID && provider_visible_to_caller(&provider.view, caller).await
        {
            selected.push(provider.view.id.clone());
        }
    }
    selected
}

async fn provider_visible_to_caller(
    provider: &crate::config::depot::ProviderView,
    caller: &CodeModeCaller,
) -> bool {
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
        CodeModeCatalogKind::Tool
        | CodeModeCatalogKind::Snippet
        | CodeModeCatalogKind::Resource => &[],
    }
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
    query: &str,
) -> bool {
    let query = query.trim().to_ascii_lowercase();
    query.is_empty()
        || descriptor.name.to_ascii_lowercase().contains(&query)
        || descriptor.namespace.to_ascii_lowercase().contains(&query)
        || descriptor
            .title
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(&query))
        || descriptor
            .description
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(&query))
        || descriptor
            .tags
            .iter()
            .any(|value| value.to_ascii_lowercase().contains(&query))
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_codemode::CodeModeCallerCapabilities;

    #[test]
    fn user_kinds_map_to_exact_depot_indexes() {
        assert_eq!(depot_kinds(CodeModeCatalogKind::Tool), &[]);
        assert_eq!(depot_kinds(CodeModeCatalogKind::Snippet), &[]);
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

        let buckets = bounded_ordered(requests.collect()).await;

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
        assert!(results.is_empty());
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
