//! Surface-neutral canonical Skills registry orchestration.
//!
//! This module combines first-party/operator skills with route-scoped upstream
//! skills without depending on MCP request types. Native SEP handlers, the
//! compatibility service, CLI, and API all consume this facade.

use std::collections::BTreeSet;
#[cfg(feature = "gateway")]
use std::sync::Arc;
use std::sync::OnceLock;

use labby_runtime::error::ToolError;
use labby_runtime::skills::parse_skill_uri;
use labby_runtime::skills::wire::{
    CACHE_SCOPE_PRIVATE, CACHE_SCOPE_PUBLIC, SkillEntry, SkillsListResult,
};
#[cfg(feature = "gateway")]
use labby_runtime::skills::{
    SkillDiscoverRequest, SkillGetRequest, SkillId, SkillProvider, SkillProviderDeadline,
    SkillResourceReadRequest,
};
use labby_runtime::skills::{SkillProviderEntry, SkillProviderError, limits};

#[cfg(feature = "gateway")]
use futures::{StreamExt, stream};
#[cfg(feature = "gateway")]
use labby_gateway::gateway::manager::GatewayManager;
#[cfg(feature = "gateway")]
use labby_gateway::upstream::pool::{SepSkillProvider, UpstreamPool};

use super::aggregate::{self, ToolAccess};
use super::providers::FirstPartySkillProviders;

/// Caller-dependent inputs that affect which skills may be observed.
///
/// None for allowed_upstreams means every configured upstream is route-visible.
/// Some(empty) means first-party only and is the safe default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillCallerScope {
    allowed_upstreams: Option<BTreeSet<String>>,
    subject: Option<String>,
    tool_access: ToolAccess,
}

impl Default for SkillCallerScope {
    fn default() -> Self {
        Self::first_party_only()
    }
}

impl SkillCallerScope {
    #[must_use]
    pub(crate) fn first_party_only() -> Self {
        Self {
            allowed_upstreams: Some(BTreeSet::new()),
            subject: None,
            tool_access: ToolAccess::Direct,
        }
    }

    #[must_use]
    pub(crate) fn root(subject: Option<String>, tool_access: ToolAccess) -> Self {
        Self {
            allowed_upstreams: None,
            subject,
            tool_access,
        }
    }

    #[must_use]
    pub(crate) fn restricted(
        allowed_upstreams: impl IntoIterator<Item = String>,
        subject: Option<String>,
        tool_access: ToolAccess,
    ) -> Self {
        Self {
            allowed_upstreams: Some(allowed_upstreams.into_iter().collect()),
            subject,
            tool_access,
        }
    }

    #[must_use]
    pub(crate) fn allows_upstream(&self, name: &str) -> bool {
        self.allowed_upstreams
            .as_ref()
            .is_none_or(|allowed| allowed.contains(name))
    }

    #[must_use]
    pub(crate) fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    #[must_use]
    pub(crate) const fn tool_access(&self) -> ToolAccess {
        self.tool_access
    }
}

/// Runtime dependencies for canonical Skills operations.
///
/// A missing manager is intentionally first-party-only. The facade never falls
/// back to process-global gateway state because doing so would erase protected
/// route and OAuth-subject boundaries.
pub(crate) struct SkillRegistryContext {
    first_party: &'static FirstPartySkillProviders,
    #[cfg(feature = "gateway")]
    manager: Option<Arc<GatewayManager>>,
    scope: SkillCallerScope,
}

impl SkillRegistryContext {
    #[must_use]
    pub(crate) fn first_party_only() -> Self {
        Self {
            first_party: first_party_providers(),
            #[cfg(feature = "gateway")]
            manager: None,
            scope: SkillCallerScope::first_party_only(),
        }
    }

    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn with_manager(manager: Arc<GatewayManager>, scope: SkillCallerScope) -> Self {
        Self {
            first_party: first_party_providers(),
            manager: Some(manager),
            scope,
        }
    }
}

fn first_party_providers() -> &'static FirstPartySkillProviders {
    static PROVIDERS: OnceLock<FirstPartySkillProviders> = OnceLock::new();
    PROVIDERS.get_or_init(FirstPartySkillProviders::load)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VisibleSkillFile {
    pub(crate) uri: String,
    pub(crate) skill_uri: String,
    pub(crate) origin: String,
    pub(crate) digest: String,
    pub(crate) mime_type: Option<String>,
    pub(crate) text: String,
}

pub(crate) async fn list_visible_skills(context: &SkillRegistryContext) -> SkillsListResult {
    let mut listing = SkillsListResult {
        skills: context
            .first_party
            .discover()
            .iter()
            .cloned()
            .map(provider_entry_to_wire)
            .collect(),
        next_cursor: None,
        ttl_ms: Some(3_600_000),
        cache_scope: Some(CACHE_SCOPE_PUBLIC.to_string()),
        meta: None,
    };

    #[cfg(feature = "gateway")]
    {
        let proxied = proxied_skill_entries(context).await;
        listing.absorb(
            proxied.entries,
            proxied.cache_scope.as_deref(),
            proxied.ttl_ms,
        );
        if proxied.unreachable_upstreams > 0 {
            listing.note_incomplete(
                "unreachableUpstreams",
                serde_json::Value::from(proxied.unreachable_upstreams),
            );
        }
        if proxied.excluded_count > 0 {
            listing.note_incomplete(
                "excludedSkills",
                serde_json::Value::from(proxied.excluded_count),
            );
        }
        if proxied.truncated {
            listing.note_incomplete("truncated", serde_json::Value::Bool(true));
        }
    }

    let _ = context;
    listing
}

pub(crate) async fn get_visible_skill(
    context: &SkillRegistryContext,
    uri: &str,
) -> Option<SkillEntry> {
    if let Some(entry) = context.first_party.find(uri) {
        return Some(provider_entry_to_wire(entry.clone()));
    }

    #[cfg(feature = "gateway")]
    {
        let parsed = parse_skill_uri(uri).ok()?;
        let origin = parsed.origin().to_string();
        if !context.scope.allows_upstream(&origin) {
            return None;
        }
        let manager = context.manager.as_deref()?;
        let config = manager.upstream_config(&origin).await?;
        if !config.enabled || !config.proxy_skills {
            return None;
        }
        let pool = manager.current_pool().await?;
        let provider = SepSkillProvider::new(
            Arc::clone(&pool),
            config.clone(),
            context.scope.subject().map(str::to_string),
        );
        let discovered = provider
            .discover(&SkillDiscoverRequest::default())
            .await
            .ok()?;
        let validated = discovered
            .skills
            .into_iter()
            .map(SkillProviderEntry::into_validated)
            .collect::<Vec<_>>();
        let meta = origin_meta(&origin, &pool, context.scope.tool_access()).await;
        let minted = aggregate::mint_proxied_entries(&config, &validated, Some(&meta));
        if let Some(entry) = minted.entries.iter().find(|entry| {
            entry.uri == uri
                || entry
                    .resources
                    .as_ref()
                    .is_some_and(|resources| resources.iter().any(|resource| resource.uri == uri))
        }) {
            return Some(entry.clone());
        }

        // A URI already owned by a collision-excluded skill stays poisoned.
        // Do not let an inconsistent `skills/get` response resurrect it.
        if minted.excludes_uri(uri) {
            return None;
        }

        let upstream_uri = parsed.upstream_uri_for_origin(&config.name)?;
        if let Some(cached) = provider.cached_owner_for_resource(&upstream_uri).await {
            let entry =
                aggregate::mint_proxied_entry(&config.name, &cached.into_validated(), Some(&meta))?;
            if minted.conflicts_with(&entry) {
                return None;
            }
            return Some(entry);
        }
        let fetched = provider
            .get(&SkillGetRequest {
                id: SkillId::new(provider.id().clone(), upstream_uri),
                deadline: SkillProviderDeadline::default(),
            })
            .await
            .ok()
            .map(|result| result.skill.into_validated())?;
        let entry = aggregate::mint_proxied_entry(&config.name, &fetched, Some(&meta))?;
        if minted.conflicts_with(&entry) {
            tracing::warn!(
                upstream = %config.name,
                skill = %entry.uri,
                "excluding unlisted skill whose manifest collides with published URI ownership"
            );
            return None;
        }
        return Some(entry);
    }

    #[cfg(not(feature = "gateway"))]
    {
        let _ = context;
        let _ = uri;
        None
    }
}

pub(crate) async fn read_visible_skill_file(
    context: &SkillRegistryContext,
    uri: &str,
) -> Result<VisibleSkillFile, ToolError> {
    if let Some(provider_entry) = context.first_party.find(uri) {
        let entry = provider_entry_to_wire(provider_entry.clone());
        let resource = entry
            .resources
            .as_ref()
            .and_then(|resources| resources.iter().find(|resource| resource.uri == uri))
            .ok_or_else(|| stale_manifest(uri))?;
        let verified = context
            .first_party
            .read(&provider_entry, uri, limits::MAX_SKILL_RESOURCE_BYTES)
            .await
            .map_err(first_party_provider_error_to_tool)?;
        let text = verified_text(
            verified.bytes,
            "verified first-party skill resource was not UTF-8 text",
        )?;
        return Ok(VisibleSkillFile {
            uri: uri.to_string(),
            skill_uri: entry.uri,
            origin: labby_runtime::skills::FIRST_PARTY_ORIGIN.to_string(),
            digest: resource.digest.clone(),
            mime_type: None,
            text,
        });
    }

    #[cfg(feature = "gateway")]
    {
        let entry = get_visible_skill(context, uri)
            .await
            .ok_or_else(|| unknown_file(uri))?;
        let resource = entry
            .resources
            .as_ref()
            .and_then(|resources| resources.iter().find(|resource| resource.uri == uri))
            .ok_or_else(|| stale_manifest(uri))?;
        let parsed = parse_skill_uri(uri).map_err(|error| ToolError::InvalidParam {
            message: error.to_string(),
            param: "uri".to_string(),
        })?;
        let origin = parsed.origin().to_string();
        if !context.scope.allows_upstream(&origin) {
            return Err(unknown_file(uri));
        }
        let manager = context
            .manager
            .as_deref()
            .ok_or_else(|| unknown_file(uri))?;
        let config = manager
            .upstream_config(&origin)
            .await
            .filter(|config| config.enabled && config.proxy_skills)
            .ok_or_else(|| unknown_file(uri))?;
        let pool = manager.current_pool().await.ok_or_else(|| ToolError::Sdk {
            sdk_kind: "upstream_unavailable".to_string(),
            message: "gateway runtime is unavailable while reading a skill file".to_string(),
        })?;
        let upstream_uri = parsed
            .upstream_uri_for_origin(&origin)
            .ok_or_else(|| unknown_file(uri))?;
        let provider =
            SepSkillProvider::new(pool, config, context.scope.subject().map(str::to_string));
        let skill_source_id = parse_skill_uri(&entry.uri)
            .ok()
            .and_then(|uri| uri.upstream_uri_for_origin(&origin))
            .ok_or_else(|| stale_manifest(uri))?;
        let verified = provider
            .read_resource(&SkillResourceReadRequest {
                skill_id: SkillId::new(provider.id().clone(), skill_source_id),
                resource_id: upstream_uri,
                max_bytes: limits::MAX_SKILL_RESOURCE_BYTES,
                deadline: SkillProviderDeadline::default(),
            })
            .await
            .map_err(provider_error_to_tool)?;
        let text = verified_text(verified.bytes, "verified skill resource was not UTF-8 text")?;
        return Ok(VisibleSkillFile {
            uri: uri.to_string(),
            skill_uri: entry.uri,
            origin,
            digest: resource.digest.clone(),
            mime_type: verified.media_type,
            text,
        });
    }

    #[cfg(not(feature = "gateway"))]
    {
        let _ = context;
        Err(unknown_file(uri))
    }
}

fn provider_entry_to_wire(skill: SkillProviderEntry) -> SkillEntry {
    skill.into_validated().entry
}

fn first_party_provider_error_to_tool(error: SkillProviderError) -> ToolError {
    provider_error_with_failure_kind(error, "provider_error")
}

fn provider_error_with_failure_kind(
    error: SkillProviderError,
    provider_failure_kind: &'static str,
) -> ToolError {
    let sdk_kind = match error {
        SkillProviderError::InvalidRequest { .. } | SkillProviderError::WrongProvider => {
            "invalid_param"
        }
        SkillProviderError::SkillNotFound | SkillProviderError::ResourceNotFound => "not_found",
        SkillProviderError::ManifestStale => labby_runtime::skills::KIND_SKILL_MANIFEST_STALE,
        SkillProviderError::DeadlineExceeded => "timeout",
        SkillProviderError::LimitExceeded { .. } => "response_too_large",
        SkillProviderError::Integrity { .. } => labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH,
        SkillProviderError::Unavailable { .. } | SkillProviderError::Provider { .. } => {
            provider_failure_kind
        }
    };
    ToolError::Sdk {
        sdk_kind: sdk_kind.to_string(),
        message: error.to_string(),
    }
}

fn verified_text(bytes: Vec<u8>, message: &'static str) -> Result<String, ToolError> {
    String::from_utf8(bytes).map_err(|_| ToolError::Sdk {
        sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
        message: message.to_string(),
    })
}

fn unknown_file(uri: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!("'{uri}' is not a skill file this caller can access"),
    }
}

fn stale_manifest(uri: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: labby_runtime::skills::KIND_SKILL_MANIFEST_STALE.to_string(),
        message: format!("the current skill manifest does not bind '{uri}'"),
    }
}

#[cfg(feature = "gateway")]
#[derive(Debug, Default)]
struct ProxiedSkills {
    entries: Vec<SkillEntry>,
    excluded_uris: BTreeSet<String>,
    unreachable_upstreams: usize,
    excluded_count: usize,
    truncated: bool,
    cache_scope: Option<String>,
    ttl_ms: Option<u64>,
}

#[cfg(feature = "gateway")]
async fn proxied_skill_entries(context: &SkillRegistryContext) -> ProxiedSkills {
    let Some(manager) = context.manager.as_deref() else {
        return ProxiedSkills::default();
    };
    let Some(pool) = manager.current_pool().await else {
        return ProxiedSkills::default();
    };
    let configs = manager
        .current_config()
        .await
        .upstream
        .into_iter()
        .filter(|config| config.enabled && config.proxy_skills)
        .filter(|config| context.scope.allows_upstream(&config.name))
        .collect::<Vec<_>>();

    let subject = context.scope.subject().map(str::to_string);
    let mut results = stream::iter(configs)
        .map(|config| {
            let pool = Arc::clone(&pool);
            let subject = subject.clone();
            async move {
                let provider = SepSkillProvider::new(Arc::clone(&pool), config.clone(), subject);
                let result = provider.discover(&SkillDiscoverRequest::default()).await;
                (config, result)
            }
        })
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
    results.sort_by(|(left, _), (right, _)| left.name.cmp(&right.name));

    let mut aggregated = ProxiedSkills::default();
    if !results.is_empty() {
        aggregated.cache_scope = Some(CACHE_SCOPE_PRIVATE.to_string());
    }
    for (config, result) in results {
        match result {
            Ok(discovered) => {
                aggregated.excluded_count += discovered.excluded_count;
                aggregated.truncated |= discovered.truncated;
                let ttl_ms = discovered
                    .ttl
                    .and_then(|ttl| u64::try_from(ttl.as_millis()).ok());
                aggregated.ttl_ms = min_ttl(aggregated.ttl_ms, ttl_ms);
                let meta = origin_meta(&config.name, &pool, context.scope.tool_access()).await;
                let validated = discovered
                    .skills
                    .into_iter()
                    .map(SkillProviderEntry::into_validated)
                    .collect::<Vec<_>>();
                let minted = aggregate::mint_proxied_entries(&config, &validated, Some(&meta));
                aggregated.excluded_count += minted.excluded_count;
                aggregated.excluded_uris.extend(minted.excluded_uris);
                aggregated.entries.extend(minted.entries);
            }
            Err(error) => {
                aggregated.unreachable_upstreams += 1;
                tracing::warn!(
                    upstream = %config.name,
                    error = %error,
                    "skipping an upstream while aggregating skills"
                );
            }
        }
    }
    aggregated
}

#[cfg(feature = "gateway")]
fn provider_error_to_tool(error: SkillProviderError) -> ToolError {
    provider_error_with_failure_kind(error, "upstream_error")
}

#[cfg(feature = "gateway")]
async fn origin_meta(
    origin: &str,
    pool: &UpstreamPool,
    access: ToolAccess,
) -> serde_json::Map<String, serde_json::Value> {
    let reachable = if access == ToolAccess::Direct {
        pool.healthy_tools_for_upstream(origin)
            .await
            .into_iter()
            .map(|tool| tool.tool.name.to_string())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    aggregate::origin_meta(origin, access, &reachable)
}

#[cfg(feature = "gateway")]
fn min_ttl(current: Option<u64>, incoming: Option<u64>) -> Option<u64> {
    match (current, incoming) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(feature = "gateway", feature = "skills", feature = "proxy-testkit"))]
    use labby_runtime::skills::digest::ResourceDigest;

    #[test]
    fn default_scope_is_first_party_only() {
        let scope = SkillCallerScope::default();
        assert!(!scope.allows_upstream("github"));
        assert!(scope.subject().is_none());
    }

    #[test]
    fn root_scope_allows_every_upstream() {
        let scope = SkillCallerScope::root(Some("alice".to_string()), ToolAccess::Direct);
        assert!(scope.allows_upstream("github"));
        assert!(scope.allows_upstream("gitlab"));
        assert_eq!(scope.subject(), Some("alice"));
    }

    #[test]
    fn protected_scope_is_an_allowlist() {
        let scope = SkillCallerScope::restricted(
            ["github".to_string(), "docs".to_string()],
            None,
            ToolAccess::CodeModeOnly,
        );
        assert!(scope.allows_upstream("github"));
        assert!(scope.allows_upstream("docs"));
        assert!(!scope.allows_upstream("private"));
        assert_eq!(scope.tool_access(), ToolAccess::CodeModeOnly);
    }

    #[tokio::test]
    async fn first_party_context_lists_and_reads_same_registry() {
        let context = SkillRegistryContext::first_party_only();
        let listing = list_visible_skills(&context).await;
        let entry = listing
            .skills
            .iter()
            .find(|entry| entry.uri == "skill://labby/using-labby/SKILL.md")
            .expect("bundled skill");
        let file = read_visible_skill_file(&context, &entry.uri)
            .await
            .expect("read");
        assert_eq!(file.skill_uri, entry.uri);
        assert!(file.text.contains("name: using-labby"));
        let digest = entry
            .resources
            .as_ref()
            .and_then(|resources| resources.iter().find(|resource| resource.uri == entry.uri))
            .expect("SKILL.md digest");
        assert_eq!(file.digest, digest.digest);
    }

    #[tokio::test]
    #[cfg(all(feature = "gateway", feature = "skills", feature = "proxy-testkit"))]
    async fn reminted_unlisted_supporting_uri_resolves_and_reads_through_gateway() {
        use std::collections::HashMap;

        use labby_gateway::gateway::manager::GatewayRuntimeHandle;
        use labby_runtime::gateway_config::{GatewayConfig, UpstreamConfig};
        use serde_json::json;

        let skill_body = "---\nname: unlisted\ndescription: a test skill\n---\n\n# Body\n";
        let native_skill_uri = "skill://native/unlisted/SKILL.md";
        let native_notes_uri = "skill://native/unlisted/notes.md";
        let reminted_skill_uri = "skill://up/skill/native/unlisted/SKILL.md";
        let reminted_notes_uri = "skill://up/skill/native/unlisted/notes.md";
        let notes_digest = ResourceDigest::of_bytes(b"supporting notes").to_wire();
        let unlisted_entry = json!({
            "uri": native_skill_uri,
            "frontmatter": { "name": "unlisted", "description": "a test skill" },
            "resources": [
                {
                    "uri": native_skill_uri,
                    "digest": ResourceDigest::of_bytes(skill_body.as_bytes()).to_wire()
                },
                { "uri": native_notes_uri, "digest": notes_digest }
            ]
        });

        let pool = Arc::new(UpstreamPool::new());
        pool.insert_scripted_skills_server_for_tests(
            "up",
            json!({ "skills": [] }),
            unlisted_entry,
            HashMap::from([
                (native_skill_uri.to_string(), skill_body.to_string()),
                (native_notes_uri.to_string(), "supporting notes".to_string()),
            ]),
        )
        .await;

        let upstream = UpstreamConfig {
            enabled: true,
            name: "up".to_string(),
            url: None,
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: Some("true".to_string()),
            args: Vec::new(),
            env: Default::default(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: true,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        };
        let runtime = GatewayRuntimeHandle::default();
        runtime.swap(Some(pool)).await;
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                std::path::PathBuf::from("config.toml"),
                runtime,
            ),
        );
        manager
            .seed_config_unchecked_for_tests(GatewayConfig {
                upstream: vec![upstream],
                ..GatewayConfig::default()
            })
            .await;
        let context = SkillRegistryContext::with_manager(
            manager,
            SkillCallerScope::root(Some("alice".to_string()), ToolAccess::Direct),
        );

        let fetched = get_visible_skill(&context, reminted_skill_uri)
            .await
            .expect("unlisted skill resolves through skills/get");
        assert_eq!(fetched.uri, reminted_skill_uri);

        let entry = get_visible_skill(&context, reminted_notes_uri)
            .await
            .expect("unlisted supporting URI resolves through cached ownership");
        assert_eq!(entry.uri, reminted_skill_uri);
        assert!(entry.resources.as_ref().is_some_and(|resources| {
            resources
                .iter()
                .any(|resource| resource.uri == reminted_notes_uri)
        }));

        let file = read_visible_skill_file(&context, reminted_notes_uri)
            .await
            .expect("cached owner binds the supporting resource read");
        assert_eq!(file.uri, reminted_notes_uri);
        assert_eq!(file.skill_uri, reminted_skill_uri);
        assert_eq!(file.origin, "up");
        assert_eq!(file.digest, notes_digest);
        assert_eq!(file.text, "supporting notes");
    }
}
