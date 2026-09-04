//! The pool-facing entry point for upstream Agent Skills (SEP-2640).
//!
//! Composes the three layers beneath it: the opt-in `proxy_skills` gate, the
//! per-`(upstream, subject)` cache, and the `expose_skills` allowlist. Callers
//! get a filtered, cached snapshot and never touch the wire directly.
//!
//! # The exposure gate runs on read, not only on fetch
//!
//! The cache holds the *unfiltered* catalog and the allowlist is applied on
//! every read. That ordering matters: an operator narrowing `expose_skills`
//! must take effect immediately, not after a TTL, and a cache populated under
//! one policy must never keep serving under it once the policy changes.

use std::collections::BTreeSet;
use std::sync::Arc;

use base64::Engine as _;
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::UpstreamConfig;
use labby_runtime::skills::{
    SkillDescriptor, SkillDiscoverySource, SkillProviderId, SkillProviderKind, ValidatedSkill,
    limits, parse_skill_resource_uri,
};

use super::UpstreamPool;
use super::entries::{log_exposure_filter, resolve_request_skill_exposure_policy};
use super::skills_cache::{CachedDirectSkill, CachedSkills, evict};
use std::time::Instant;

use super::capability_call::{CapabilityCallError, timed_capability_call};
use super::logging::{UpstreamRequestLog, log_upstream_request_start};
use super::skills_exposure::SkillExposureDecision;
use super::skills_list::{UpstreamSkills, peer_declares_skills};

/// Estimate the retained payload without serializing the complete response
/// into a second buffer. The caller-specific body cap is checked first so an
/// oversized skill body is rejected before the streaming JSON-size pass.
fn skill_read_response_size(result: &rmcp::model::ReadResourceResult, content_cap: usize) -> usize {
    let body_exceeds_cap = result.contents.iter().any(|content| match content {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => {
            text.len() > content_cap
        }
        rmcp::model::ResourceContents::BlobResourceContents { blob, .. } => {
            // Base64 expands the raw body. Bound the decoded size here so a
            // conforming binary resource is not rejected merely because its
            // wire representation is larger than its manifest `size`.
            blob.len().div_ceil(4).saturating_mul(3) > content_cap
        }
        _ => false,
    });
    if body_exceeds_cap {
        usize::MAX
    } else {
        super::helpers::estimate_resource_response_size(result)
    }
}

/// One upstream's exposed skills, plus the completeness bookkeeping a caller
/// needs to report honestly.
#[derive(Debug, Clone, Default)]
pub struct ExposedSkills {
    /// Skills this caller may see, after the allowlist.
    pub skills: Vec<ValidatedSkill>,
    /// Skills dropped for integrity or budget reasons. Surfaced to agents as a
    /// bare count and to operators in full — never as a per-skill list to a
    /// downstream caller, which would leak the shape of an operator's config.
    pub excluded_count: usize,
    /// Whether a budget cut the upstream walk short.
    pub truncated: bool,
    /// Age of the underlying snapshot, for operator display.
    pub age_secs: u64,
    /// Remaining lifetime of this snapshot, clamped from the upstream's
    /// untrusted `ttlMs`. A downstream listing that folds these entries in must
    /// not advertise a longer TTL than the data behind it actually has.
    pub ttl_ms: Option<u64>,
    /// Whether this response reused a catalog snapshot or completed a refresh.
    pub source: SkillDiscoverySource,
    catalog: Arc<UpstreamSkills>,
    exposed_indices: BTreeSet<usize>,
}

/// Operator-only view of one validated upstream skill before exposure filtering.
#[derive(Debug, Clone)]
pub(crate) struct OperatorSkill {
    pub(crate) descriptor: SkillDescriptor,
    pub(crate) exposure: SkillExposureDecision,
}

/// Operator-only reason a skill entry was rejected during ingest.
#[derive(Debug, Clone)]
pub(crate) struct OperatorSkillRejection {
    pub(crate) reason: String,
    pub(crate) uri: String,
    pub(crate) detail: String,
}

/// Operator-only skills snapshot. Unlike the downstream view, this retains
/// validated-but-hidden skills so the admin UI can manage exposure safely.
#[derive(Debug, Clone, Default)]
pub(crate) struct OperatorSkills {
    pub(crate) supports_skills: Option<bool>,
    pub(crate) discovered_count: usize,
    pub(crate) skills: Vec<OperatorSkill>,
    pub(crate) rejected: Vec<OperatorSkillRejection>,
    pub(crate) truncated: bool,
    pub(crate) age_secs: u64,
}

impl UpstreamPool {
    /// Exposed skills for one upstream, fetching or refreshing as needed.
    ///
    /// Returns an empty set — never an error — when the upstream does not
    /// proxy skills or never declared the extension. Neither is a failure, and
    /// treating them as one would put phantom failures on the circuit breaker
    /// for every non-skills upstream in the catalog.
    pub async fn upstream_skills(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
    ) -> Result<ExposedSkills, String> {
        if !config.proxy_skills {
            return Ok(ExposedSkills::default());
        }
        let key = (config.name.clone(), subject.map(str::to_string));

        // Serve a cached snapshot when one is fresh. An expired snapshot is
        // still served; the refresh happens behind it rather than in front.
        if let Some(cached) = self.cached_skills(&key).await {
            if cached.is_fresh() {
                return Ok(self.apply_skill_exposure(
                    config,
                    &cached,
                    subject,
                    SkillDiscoverySource::Cached,
                ));
            }
            let stale =
                self.apply_skill_exposure(config, &cached, subject, SkillDiscoverySource::Cached);
            self.spawn_skills_refresh(config.clone(), subject.map(str::to_string));
            return Ok(stale);
        }

        // Cold: one caller fetches while the rest wait on the same guard, so a
        // burst of downstream listings makes one upstream request.
        let guard = self.skills_fetch_locks.guard_for(&key).await;
        let _held = guard.lock().await;
        if let Some(cached) = self.cached_skills(&key).await {
            return Ok(self.apply_skill_exposure(
                config,
                &cached,
                subject,
                SkillDiscoverySource::Cached,
            ));
        }

        let snapshot = self.fetch_and_cache_skills(config, subject).await?;
        Ok(self.apply_skill_exposure(config, &snapshot, subject, SkillDiscoverySource::Refreshed))
    }

    /// Operator snapshot for the admin UI. This never bypasses the trust gate:
    /// untrusted upstreams report handshake support but are not asked to list skills.
    pub(crate) async fn upstream_skills_operator(
        &self,
        config: &UpstreamConfig,
    ) -> Result<OperatorSkills, String> {
        if !config.proxy_skills {
            return Ok(OperatorSkills {
                supports_skills: self
                    .cached_upstream_summary(&config.name)
                    .await
                    .and_then(|summary| summary.supports_skills),
                ..OperatorSkills::default()
            });
        }

        let exposed = self.upstream_skills(config, None).await?;
        let policy =
            resolve_request_skill_exposure_policy(&config.name, config.expose_skills.clone());
        let provider = SkillProviderId::new(SkillProviderKind::McpUpstream, config.name.clone());
        let skills = exposed
            .catalog
            .skills
            .iter()
            .map(|skill| OperatorSkill {
                descriptor: SkillDescriptor::from_validated_entry(provider.clone(), skill),
                exposure: SkillExposureDecision::evaluate(&policy, &skill.name),
            })
            .collect();
        let rejected = exposed
            .catalog
            .excluded
            .iter()
            .map(|excluded| OperatorSkillRejection {
                reason: excluded.reason.as_str().to_string(),
                uri: excluded.uri.clone(),
                detail: excluded.detail.clone(),
            })
            .collect();

        Ok(OperatorSkills {
            supports_skills: self
                .cached_upstream_summary(&config.name)
                .await
                .and_then(|summary| summary.supports_skills),
            discovered_count: exposed.catalog.discovered_count,
            skills,
            rejected,
            truncated: exposed.truncated,
            age_secs: exposed.age_secs,
        })
    }

    /// Read a cached snapshot, marking it used for idle eviction.
    async fn cached_skills(&self, key: &(String, Option<String>)) -> Option<CachedSkills> {
        let mut cache = self.skills_cache.write().await;
        let entry = cache.get_mut(key)?;
        entry.touch();
        Some(entry.clone())
    }

    /// Fetch one upstream's catalog and store it.
    pub(super) async fn fetch_and_cache_skills(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
    ) -> Result<CachedSkills, String> {
        // Upstreams connect lazily: a cold gateway has a seeded catalog entry but
        // no live connection until something asks for one, so acquiring the peer
        // directly reports "not connected" on every first read — the normal
        // state for `labby mcp`, not an error.
        //
        // Gated on the connection actually being absent, not on the upstream
        // having healthy tools. `ensure_tools_for_upstream` tears down and
        // reconnects whenever an upstream has no healthy tools, and a
        // skills-only upstream never has any — routing through it
        // unconditionally would reconnect on every single read.
        let connected = self.connections.read().await.contains_key(&config.name);
        if !connected
            && let Err(error) = self.ensure_tools_for_upstream(config, subject, None).await
        {
            return Err(format!(
                "upstream `{}` could not be connected for skills: {error}",
                config.name
            ));
        }
        let peer = self
            .acquire_peer(
                &config.name,
                super::super::types::UpstreamCapability::Skills,
                "skills.list",
            )
            .await
            .ok_or_else(|| format!("upstream `{}` is not connected", config.name))?;

        // An upstream that never declared the extension is not a failure — it
        // simply has no skills, and caching that avoids re-asking every read.
        if !peer_declares_skills(&peer) {
            {
                let mut catalog = self.catalog_write().await;
                if let Some(catalog_entry) = catalog.get_mut(&config.name) {
                    catalog_entry.supports_skills = Some(false);
                    catalog_entry.skill_count = 0;
                    catalog_entry.skill_names.clear();
                }
            }
            let empty = CachedSkills::new(UpstreamSkills::default());
            self.store_skills(&config.name, subject, empty.clone())
                .await;
            return Ok(empty);
        }
        {
            let mut catalog = self.catalog_write().await;
            if let Some(catalog_entry) = catalog.get_mut(&config.name) {
                catalog_entry.supports_skills = Some(true);
            }
        }

        match self.fetch_upstream_skills(&config.name, &peer).await {
            Ok(skills) => {
                let discovered_count = skills.discovered_count;
                let skill_names = skills
                    .skills
                    .iter()
                    .map(|skill| skill.name.clone())
                    .collect::<Vec<_>>();
                let excluded = skills.excluded.clone();
                let entry = CachedSkills::new(skills);
                self.store_skills(&config.name, subject, entry.clone())
                    .await;
                self.record_success_for(
                    &config.name,
                    super::super::types::UpstreamCapability::Skills,
                )
                .await;
                {
                    let mut catalog = self.catalog_write().await;
                    if let Some(catalog_entry) = catalog.get_mut(&config.name) {
                        catalog_entry.supports_skills = Some(true);
                        catalog_entry.skill_count = discovered_count;
                        catalog_entry.skill_names = skill_names;
                    }
                }
                for excluded in &excluded {
                    tracing::warn!(
                        upstream = %config.name,
                        reason = excluded.reason.as_str(),
                        skill = %super::helpers::redact_resource_uri_for_logging(&excluded.uri),
                        "excluded an upstream skill at ingest"
                    );
                }
                Ok(entry)
            }
            Err(error) => {
                self.record_failure_for(
                    &config.name,
                    super::super::types::UpstreamCapability::Skills,
                    format!("failed to list skills from upstream: {error}"),
                )
                .await;
                Err(error)
            }
        }
    }

    async fn store_skills(&self, name: &str, subject: Option<&str>, entry: CachedSkills) {
        let mut cache = self.skills_cache.write().await;
        let key = (name.to_string(), subject.map(str::to_string));
        let mut entry = entry;
        if let Some(previous) = cache.get(&key) {
            entry.retain_direct_from(previous);
        }
        cache.insert(key, entry);
        evict(&mut cache);
    }

    /// Refresh an expired snapshot behind the caller.
    ///
    /// Marks the entry refreshing first so a burst of readers past the TTL
    /// spawns one task rather than one per reader.
    fn spawn_skills_refresh(&self, config: UpstreamConfig, subject: Option<String>) {
        let pool = self.clone();
        tokio::spawn(async move {
            let key = (config.name.clone(), subject.clone());
            {
                let mut cache = pool.skills_cache.write().await;
                match cache.get_mut(&key) {
                    Some(entry) if entry.refreshing => return,
                    Some(entry) => entry.refreshing = true,
                    None => return,
                }
            }
            let result = pool
                .fetch_and_cache_skills(&config, subject.as_deref())
                .await;
            if result.is_err() {
                // Keep serving the stale snapshot; clear the flag so a later
                // read can try again rather than pinning it forever.
                let mut cache = pool.skills_cache.write().await;
                if let Some(entry) = cache.get_mut(&key) {
                    entry.refreshing = false;
                }
            }
            pool.skills_fetch_locks.prune().await;
        });
    }

    /// Apply `expose_skills` to a cached snapshot.
    fn apply_skill_exposure(
        &self,
        config: &UpstreamConfig,
        cached: &CachedSkills,
        subject: Option<&str>,
        source: SkillDiscoverySource,
    ) -> ExposedSkills {
        let policy =
            resolve_request_skill_exposure_policy(&config.name, config.expose_skills.clone());
        let total = cached.skills.skills.len();
        let exposed_indices: BTreeSet<usize> = cached
            .skills
            .skills
            .iter()
            .enumerate()
            .filter_map(|(index, skill)| policy.matches(&skill.name).then_some(index))
            .collect();
        let skills = exposed_indices
            .iter()
            .map(|index| cached.skills.skills[*index].clone())
            .collect::<Vec<_>>();
        log_exposure_filter(
            &config.name,
            "skills",
            total - skills.len(),
            skills.len(),
            subject.is_some(),
        );
        ExposedSkills {
            skills,
            excluded_count: cached.skills.excluded_count(),
            truncated: cached.skills.truncated,
            age_secs: cached.age().as_secs(),
            ttl_ms: Some(cached.remaining_ttl().as_millis() as u64),
            source,
            catalog: Arc::clone(&cached.skills),
            exposed_indices,
        }
    }

    /// Fetch one skill by URI from an upstream that did not list it.
    ///
    /// SEP-2640 requires a host to load a skill given only its URI, and says an
    /// empty or partial listing is never proof a server has no skills. Without
    /// this, a skill absent from a cached or budget-truncated listing is
    /// permanently unreachable even though the upstream would serve it.
    ///
    /// Still gated: the upstream must opt in, and the returned entry passes the
    /// same ingest validation and `expose_skills` allowlist a listed skill does,
    /// so unlisted does not mean unfiltered.
    pub async fn fetch_unlisted_skill(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        uri: &str,
    ) -> Result<Option<ValidatedSkill>, String> {
        if !config.proxy_skills {
            return Ok(None);
        }
        let canonical_uri = parse_skill_resource_uri(uri)
            .map_err(|error| error.to_string())?
            .to_uri();
        if let Some(skill) = self
            .cached_direct_skill(config, subject, &canonical_uri)
            .await
        {
            return Ok(Some(skill));
        }
        let peer = self
            .acquire_peer(
                &config.name,
                super::super::types::UpstreamCapability::Skills,
                "skills.get",
            )
            .await
            .ok_or_else(|| format!("upstream `{}` is not connected", config.name))?;
        if !peer_declares_skills(&peer) {
            return Ok(None);
        }
        let Some(skill) = self
            .fetch_upstream_skill(&config.name, &peer, uri, subject)
            .await?
        else {
            return Ok(None);
        };

        // The allowlist applies to a skill fetched by URI exactly as it does to
        // a listed one; filtering only the listing would be a bypass.
        let policy =
            resolve_request_skill_exposure_policy(&config.name, config.expose_skills.clone());
        if !policy.matches(&skill.name) {
            return Ok(None);
        }
        if skill.entry.uri != canonical_uri {
            return Err(format!(
                "upstream `{}` returned a different skill URI than requested",
                config.name
            ));
        }
        self.store_direct_skill(config, subject, skill.clone())
            .await?;
        Ok(Some(skill))
    }

    async fn cached_direct_skill(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        skill_uri: &str,
    ) -> Option<ValidatedSkill> {
        let policy =
            resolve_request_skill_exposure_policy(&config.name, config.expose_skills.clone());
        let key = (config.name.clone(), subject.map(str::to_string));
        let mut cache = self.skills_cache.write().await;
        let cached = cache.get_mut(&key)?;
        cached.touch();
        let snapshot = cached.direct.get_mut(skill_uri)?;
        if !snapshot.is_fresh() {
            cached.direct.remove(skill_uri);
            return None;
        }
        snapshot.touch();
        policy
            .matches(&snapshot.skill.name)
            .then(|| snapshot.skill.clone())
    }

    async fn store_direct_skill(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        skill: ValidatedSkill,
    ) -> Result<(), String> {
        let key = (config.name.clone(), subject.map(str::to_string));
        let mut cache = self.skills_cache.write().await;
        let cached = cache
            .get_mut(&key)
            .ok_or_else(|| "skill catalog cache disappeared during direct get".to_string())?;
        cached.touch();
        let candidate_uris = owned_skill_uris(&skill);

        // Listed collisions are poisoned regardless of exposure. This is
        // stricter than publishing a hidden owner and prevents a policy change
        // from changing which manifest owns bytes already cached by URI.
        if candidate_uris
            .iter()
            .any(|uri| cached.skills.resource_index.contains_key(uri))
            || cached.direct.values().any(|snapshot| {
                snapshot.skill.entry.uri != skill.entry.uri
                    && owned_skill_uris(&snapshot.skill)
                        .iter()
                        .any(|uri| candidate_uris.contains(uri))
            })
        {
            return Err(format!(
                "unlisted skill `{}` collides with existing manifest ownership",
                skill.entry.uri
            ));
        }
        if cached.direct.len() >= limits::MAX_SKILLS_PER_UPSTREAM
            && !cached.direct.contains_key(&skill.entry.uri)
        {
            return Err("direct skill snapshot limit exceeded".to_string());
        }
        cached
            .direct
            .insert(skill.entry.uri.clone(), CachedDirectSkill::new(skill));
        Ok(())
    }

    /// Return the unique cached direct-get owner of a resource for this caller.
    ///
    /// This only recovers a manifest; reads still require its provider-scoped
    /// identity and the exact resource identity separately.
    pub async fn cached_unlisted_skill_owner(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        resource_uri: &str,
    ) -> Option<ValidatedSkill> {
        let canonical_uri = parse_skill_resource_uri(resource_uri).ok()?.to_uri();
        let policy =
            resolve_request_skill_exposure_policy(&config.name, config.expose_skills.clone());
        let key = (config.name.clone(), subject.map(str::to_string));
        let mut cache = self.skills_cache.write().await;
        let cached = cache.get_mut(&key)?;
        cached.touch();
        let mut owners = cached
            .direct
            .values_mut()
            .filter(|snapshot| snapshot.is_fresh())
            .filter(|snapshot| policy.matches(&snapshot.skill.name))
            .filter(|snapshot| owned_skill_uris(&snapshot.skill).contains(&canonical_uri));
        let owner = owners.next()?;
        if owners.next().is_some() {
            return None;
        }
        owner.touch();
        Some(owner.skill.clone())
    }

    /// Drop every cached skill catalog for one upstream, across all subjects.
    ///
    /// Called on reload and on disconnect: a snapshot outliving the connection
    /// it came from would serve a catalog Labby can no longer honor a read
    /// against.
    pub async fn invalidate_upstream_skills(&self, name: &str) {
        let mut cache = self.skills_cache.write().await;
        cache.retain(|(upstream, _), _| upstream != name);
        drop(cache);
        let mut catalog = self.catalog_write().await;
        if let Some(entry) = catalog.get_mut(name) {
            entry.skill_count = 0;
            entry.skill_names.clear();
        }
    }

    /// Drop every cached skill catalog, across all upstreams and subjects.
    ///
    /// Called on pool drain (config reload / swap). Skills are cached against a
    /// connection and a config; when both are replaced wholesale, so is the
    /// cache.
    pub async fn clear_all_cached_skills(&self) {
        let mut cache = self.skills_cache.write().await;
        let count = cache.len();
        cache.clear();
        drop(cache);
        self.skills_fetch_locks.prune().await;
        if count > 0 {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                cleared = count,
                "cleared cached upstream skill catalogs"
            );
        }
    }

    /// Names of upstreams with a cached skill catalog, for operator display.
    pub async fn upstreams_with_cached_skills(&self) -> BTreeSet<String> {
        self.skills_cache
            .read()
            .await
            .keys()
            .map(|(name, _)| name.clone())
            .collect()
    }
}

/// Bytes of one proxied skill file, verified against the manifest that
/// published it.
#[derive(Debug, Clone)]
pub struct VerifiedSkillFile {
    /// Raw bytes covered by the manifest's `size` and `digest` fields.
    pub bytes: Vec<u8>,
    /// Whether the upstream represented these bytes as an MCP blob.
    pub is_blob: bool,
    pub mime_type: Option<String>,
}

fn owned_skill_uris(skill: &ValidatedSkill) -> BTreeSet<String> {
    std::iter::once(&skill.entry.uri)
        .chain(
            skill
                .entry
                .resources
                .iter()
                .flatten()
                .map(|resource| &resource.uri),
        )
        .filter_map(|uri| parse_skill_resource_uri(uri).ok().map(|uri| uri.to_uri()))
        .collect()
}

fn stale_skill_binding(upstream: &str, uri: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: labby_runtime::skills::KIND_SKILL_MANIFEST_STALE.to_string(),
        message: format!(
            "`{uri}` on upstream `{upstream}` does not identify exactly one exposed skill file"
        ),
    }
}

impl UpstreamPool {
    /// Read one file of a proxied skill by its exact native upstream URI.
    ///
    /// Every read is manifest-bound and digest-verified. A URI the manifest does
    /// not list is refused rather than fetched: the SEP treats an unlisted file
    /// within a skill as a change to the skill, equivalent to a digest mismatch.
    pub async fn read_proxied_skill_file(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        // The upstream's complete URI, including its native scheme.
        upstream_uri: &str,
    ) -> Result<VerifiedSkillFile, ToolError> {
        self.read_proxied_skill_file_inner(config, subject, None, upstream_uri, None)
            .await
    }

    /// Read one file while requiring a specific manifest owner.
    ///
    /// Provider-neutral callers carry a provider-scoped Skill identity in
    /// addition to the resource identity. Binding both prevents a caller from
    /// naming one exposed Skill while reading a file owned by another.
    pub async fn read_proxied_skill_file_for_skill(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        skill_uri: &str,
        upstream_uri: &str,
        max_bytes: usize,
    ) -> Result<VerifiedSkillFile, ToolError> {
        self.read_proxied_skill_file_inner(
            config,
            subject,
            Some(skill_uri),
            upstream_uri,
            Some(max_bytes),
        )
        .await
    }

    async fn read_proxied_skill_file_inner(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        expected_skill_uri: Option<&str>,
        upstream_uri: &str,
        max_bytes: Option<usize>,
    ) -> Result<VerifiedSkillFile, ToolError> {
        let canonical_uri = parse_skill_resource_uri(upstream_uri)
            .map_err(|error| ToolError::Sdk {
                sdk_kind: "invalid_param".into(),
                message: error.to_string(),
            })?
            .to_uri();
        let exposed = self
            .upstream_skills(config, subject)
            .await
            .map_err(|error| ToolError::Sdk {
                sdk_kind: "upstream_error".to_string(),
                message: error,
            })?;

        // The owning skill travels with the match: verifying a `SKILL.md`
        // needs the frontmatter its own entry published, not another skill's.
        let bindings = exposed
            .catalog
            .resource_index
            .get(&canonical_uri)
            .into_iter()
            .flatten()
            .filter(|binding| exposed.exposed_indices.contains(&binding.skill))
            .filter(|binding| {
                expected_skill_uri.is_none_or(|expected| {
                    exposed.catalog.skills[binding.skill].entry.uri == expected
                })
            })
            .collect::<Vec<_>>();
        let (skill, resource) = match bindings.as_slice() {
            [binding] => {
                let skill = exposed.catalog.skills[binding.skill].clone();
                let resource = skill.entry.resources.as_ref().expect("validated manifest")
                    [binding.resource]
                    .clone();
                (skill, resource)
            }
            [] => {
                let Some(expected) = expected_skill_uri else {
                    return Err(stale_skill_binding(&config.name, &canonical_uri));
                };
                let Some(skill) = self.cached_direct_skill(config, subject, expected).await else {
                    return Err(stale_skill_binding(&config.name, &canonical_uri));
                };
                let Some(resource) = skill
                    .entry
                    .resources
                    .as_ref()
                    .and_then(|resources| {
                        resources
                            .iter()
                            .find(|resource| resource.uri == canonical_uri)
                    })
                    .cloned()
                else {
                    return Err(stale_skill_binding(&config.name, &canonical_uri));
                };
                (skill, resource)
            }
            _ => return Err(stale_skill_binding(&config.name, &canonical_uri)),
        };
        let upstream_uri = resource.uri.as_str();
        let digest = resource.digest.as_str();

        // Read from the owning upstream directly, preserving its native
        // `skill://` URI. The URI-routed helpers resolve a `lab://upstream/…`
        // gateway namespace and cannot place a native skill URI.
        let peer = self
            .acquire_peer(
                &config.name,
                super::super::types::UpstreamCapability::Skills,
                "skill.read",
            )
            .await
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "upstream_error".to_string(),
                message: format!("upstream `{}` is not connected", config.name),
            })?;

        let start = Instant::now();
        let redacted = super::helpers::redact_resource_uri_for_logging(upstream_uri);
        let event = UpstreamRequestLog::skill(&config.name, redacted, subject.is_some());
        log_upstream_request_start(event);
        let timeout_ms = self.request_timeout.as_millis();
        let contents = timed_capability_call(
            self,
            &config.name,
            super::super::types::UpstreamCapability::Skills,
            event,
            start,
            peer.read_resource(rmcp::model::ReadResourceRequestParams::new(upstream_uri)),
            |result| skill_read_response_size(result, max_bytes.unwrap_or(usize::MAX)),
            subject,
            |error| format!("upstream `{}` skill read failed: {error}", config.name),
            format!(
                "upstream `{}` skill read timed out after {timeout_ms}ms",
                config.name
            ),
        )
        .await
        .map_err(|error| ToolError::Sdk {
            sdk_kind: if matches!(&error, CapabilityCallError::ResponseTooLarge { .. }) {
                "response_too_large"
            } else {
                "upstream_error"
            }
            .to_string(),
            message: error.to_string(),
        })?;

        // Exactly one content block: the digest model is one URI to one blob.
        let content_count = contents.contents.len();
        let Ok([content]) = <[_; 1]>::try_from(contents.contents) else {
            return Err(ToolError::Sdk {
                sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                message: format!(
                    "upstream `{}` returned {} content blocks for one skill file",
                    config.name, content_count
                ),
            });
        };
        let (bytes, is_blob, mime_type) = match content {
            rmcp::model::ResourceContents::TextResourceContents {
                text, mime_type, ..
            } => (text.into_bytes(), false, mime_type),
            rmcp::model::ResourceContents::BlobResourceContents {
                blob, mime_type, ..
            } => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(blob)
                    .map_err(|_| ToolError::Sdk {
                        sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                        message: format!(
                            "upstream `{}` returned malformed base64 for a skill resource",
                            config.name
                        ),
                    })?;
                (bytes, true, mime_type)
            }
            _ => {
                return Err(ToolError::Sdk {
                    sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                    message: "upstream returned an unsupported skill resource representation"
                        .into(),
                });
            }
        };
        if max_bytes.is_some_and(|limit| bytes.len() > limit) {
            return Err(ToolError::Sdk {
                sdk_kind: "response_too_large".to_string(),
                message: format!(
                    "skill resource from upstream `{}` exceeds the {} byte read limit",
                    config.name,
                    max_bytes.expect("checked")
                ),
            });
        }

        let parsed_digest =
            labby_runtime::skills::parse_digest(digest).map_err(|error| ToolError::Sdk {
                sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                message: format!(
                    "upstream `{}` published an unusable digest: {error}",
                    config.name
                ),
            })?;
        let declared_size = usize::try_from(resource.size).map_err(|_| ToolError::Sdk {
            sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
            message: format!(
                "manifest size for `{canonical_uri}` from upstream `{}` cannot be represented by this host",
                config.name
            ),
        })?;
        if bytes.len() != declared_size {
            return Err(ToolError::Sdk {
                sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                message: format!(
                    "content of `{canonical_uri}` from upstream `{}` is {} bytes but its entry declared {declared_size}",
                    config.name,
                    bytes.len()
                ),
            });
        }
        if !parsed_digest.matches(&bytes) {
            // Zero bytes reach the caller. A digest match is a consistency
            // check, not proof of trustworthiness, but a mismatch is proof of
            // inconsistency and the content must not be used.
            return Err(ToolError::Sdk {
                sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                message: format!(
                    "content of `{canonical_uri}` from upstream `{}` does not match the digest its entry published",
                    config.name
                ),
            });
        }

        // A digest match does not subsume the frontmatter check, and this is
        // the gap that check exists to close: an upstream may publish benign
        // `frontmatter` in its `skills/list` entry while the real `SKILL.md`
        // body carries something else — `allowed-tools: ["*"]`, say. The digest
        // is computed over the real body, so it matches, and the body a client
        // acts on grants capabilities the entry a user approved never declared.
        // That is threat-model T3, and only a field-by-field comparison of the
        // served bytes against the published entry catches it.
        if is_skill_md(&skill.entry.uri, &canonical_uri) {
            if is_blob {
                return Err(ToolError::Sdk {
                    sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                    message: format!(
                        "`SKILL.md` from upstream `{}` must be an MCP text resource",
                        config.name
                    ),
                });
            }
            let text = std::str::from_utf8(&bytes).map_err(|_| ToolError::Sdk {
                sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                message: format!(
                    "`SKILL.md` from upstream `{}` is not UTF-8 text",
                    config.name
                ),
            })?;
            let served =
                labby_runtime::skills::parse_skill_md_frontmatter(text).map_err(|error| {
                    ToolError::Sdk {
                        sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                        message: format!(
                            "`SKILL.md` from upstream `{}` has unparseable frontmatter: {error}",
                            config.name
                        ),
                    }
                })?;
            labby_runtime::skills::compare_frontmatter(&skill.entry.frontmatter, &served).map_err(
                |error| ToolError::Sdk {
                    sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
                    message: format!(
                        "`SKILL.md` from upstream `{}` disagrees with the frontmatter its entry published: {error}",
                        config.name
                    ),
                },
            )?;
        }

        Ok(VerifiedSkillFile {
            bytes,
            is_blob,
            mime_type,
        })
    }
}

/// Whether `path` addresses the `SKILL.md` of the skill rooted at `entry_uri`.
///
/// Compared against the entry's own URI rather than by matching the `SKILL.md`
/// suffix, so a nested `references/SKILL.md` is not mistaken for the skill's
/// own definition and cross-verified against the wrong frontmatter.
fn is_skill_md(entry_uri: &str, upstream_path: &str) -> bool {
    // Compared on the full path, matching what the caller passes. Using the
    // post-first-segment remainder here made this silently return false for
    // every skill, which disabled the frontmatter cross-check entirely — a
    // security check that fails open by never firing.
    parse_skill_resource_uri(entry_uri).is_ok_and(|parsed| parsed.to_uri() == upstream_path)
}

#[cfg(test)]
mod provider_response_tests {
    use super::*;

    #[test]
    fn response_sizing_rejects_content_over_the_caller_cap_without_serializing() {
        let result =
            rmcp::model::ReadResourceResult::new(vec![rmcp::model::ResourceContents::text(
                "12345",
                "skill://demo/SKILL.md",
            )]);
        assert_eq!(skill_read_response_size(&result, 4), usize::MAX);
    }

    #[test]
    fn response_sizing_streams_the_exact_escape_heavy_serialized_size() {
        let mut result =
            rmcp::model::ReadResourceResult::new(vec![rmcp::model::ResourceContents::text(
                "\"\\\n\r\t".repeat(64),
                "skill://demo/SKILL.md",
            )]);
        result.meta = Some(
            serde_json::from_value(serde_json::json!({
                "nested": { "payload": "\"\\\n\r\t".repeat(64) }
            }))
            .unwrap(),
        );
        let serialized_len = serde_json::to_vec(&result).unwrap().len();
        assert_eq!(
            skill_read_response_size(&result, usize::MAX),
            serialized_len
        );
        assert!(serialized_len > 1_000, "escape expansion must be counted");
    }
}
