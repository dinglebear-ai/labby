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

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use labby_runtime::gateway_config::UpstreamConfig;
use labby_runtime::skills::{
    SkillDescriptor, SkillDiscoverySource, SkillProviderId, SkillProviderKind, ValidatedSkill,
    limits, parse_skill_resource_uri,
};

use super::UpstreamPool;
use super::entries::{log_exposure_filter, resolve_request_skill_exposure_policy};
use super::skills_cache::{CachedDirectSkill, CachedSkills, SkillsCacheKey, evict};
use super::skills_exposure::SkillExposureDecision;
use super::skills_list::{UpstreamSkills, UpstreamSkillsError, peer_declares_skills};

struct RefreshStateGuard {
    cache: Arc<tokio::sync::RwLock<HashMap<SkillsCacheKey, CachedSkills>>>,
    key: SkillsCacheKey,
    armed: bool,
}

impl RefreshStateGuard {
    fn new(
        cache: Arc<tokio::sync::RwLock<HashMap<SkillsCacheKey, CachedSkills>>>,
        key: SkillsCacheKey,
    ) -> Self {
        Self {
            cache,
            key,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RefreshStateGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let cache = Arc::clone(&self.cache);
        let key = self.key.clone();
        runtime.spawn(async move {
            let mut cache = cache.write().await;
            if let Some(entry) = cache.get_mut(&key) {
                entry.fail_refresh();
            }
        });
    }
}

pub(super) fn skills_cache_subject<'a>(
    config: &UpstreamConfig,
    subject: Option<&'a str>,
) -> Option<&'a str> {
    if config.oauth.is_some() {
        subject
    } else {
        None
    }
}

/// One upstream's exposed skills, plus the completeness bookkeeping a caller
/// needs to report honestly.
#[derive(Debug, Clone, Default)]
pub(crate) struct ExposedSkills {
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
    /// Whether a stale-while-revalidate refresh is currently in flight.
    pub refreshing: bool,
    /// Age of the current background refresh, when one is in flight.
    pub refresh_age_secs: Option<u64>,
    /// Remaining negative-cache cooldown after the most recent refresh failure.
    pub retry_after_ms: Option<u64>,
    pub(super) catalog: Arc<UpstreamSkills>,
    pub(super) exposed_indices: BTreeSet<usize>,
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
    pub(crate) refreshing: bool,
    pub(crate) refresh_age_secs: Option<u64>,
    pub(crate) retry_after_ms: Option<u64>,
}

impl UpstreamPool {
    /// Exposed skills for one upstream, fetching or refreshing as needed.
    ///
    /// Returns an empty set — never an error — when the upstream does not
    /// proxy skills or never declared the extension. Neither is a failure, and
    /// treating them as one would put phantom failures on the circuit breaker
    /// for every non-skills upstream in the catalog.
    pub(crate) async fn upstream_skills(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
    ) -> Result<ExposedSkills, UpstreamSkillsError> {
        if !config.proxy_skills {
            return Ok(ExposedSkills::default());
        }
        let subject = skills_cache_subject(config, subject);
        let (snapshot, source) = self.upstream_skills_snapshot(config, subject).await?;
        Ok(self.apply_skill_exposure(config, &snapshot, subject, source))
    }

    /// Fetch or reuse one caller-safe catalog snapshot without materializing a cloned
    /// exposed-skills vector. Targeted reads use this path and inspect only the bound
    /// manifest they need; downstream list discovery layers exposure on top separately.
    pub(super) async fn upstream_skills_snapshot(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
    ) -> Result<(CachedSkills, SkillDiscoverySource), UpstreamSkillsError> {
        let subject = skills_cache_subject(config, subject);
        let key = (config.name.clone(), subject.map(str::to_string));

        // Serve a cached snapshot when one is fresh. An expired snapshot is
        // still served; the refresh happens behind it rather than in front.
        if let Some(cached) = self.cached_skills(&key).await {
            if cached.is_fresh() {
                return Ok((cached, SkillDiscoverySource::Cached));
            }
            self.spawn_skills_refresh(config.clone(), subject.map(str::to_string));
            return Ok((cached, SkillDiscoverySource::Cached));
        }

        // Cold: one caller fetches while the rest wait on the same guard, so a
        // burst of downstream listings makes one upstream request.
        let guard = self.skills_fetch_locks.guard_for(&key).await;
        let _held = guard.lock().await;
        if let Some(cached) = self.cached_skills(&key).await {
            return Ok((cached, SkillDiscoverySource::Cached));
        }

        let snapshot = self.fetch_and_cache_skills(config, subject).await?;
        Ok((snapshot, SkillDiscoverySource::Refreshed))
    }

    /// Operator snapshot for the admin UI. This never bypasses the trust gate:
    /// untrusted upstreams report handshake support but are not asked to list skills.
    pub(crate) async fn upstream_skills_operator(
        &self,
        config: &UpstreamConfig,
    ) -> Result<OperatorSkills, UpstreamSkillsError> {
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
            refreshing: exposed.refreshing,
            refresh_age_secs: exposed.refresh_age_secs,
            retry_after_ms: exposed.retry_after_ms,
        })
    }

    /// Read a cached catalog without taking the pool-wide write lock or deep
    /// cloning the direct-get map. Recency maintenance is best-effort and never
    /// blocks the hot read path.
    async fn cached_skills(&self, key: &(String, Option<String>)) -> Option<CachedSkills> {
        let snapshot = {
            let cache = self.skills_cache.read().await;
            cache.get(key)?.read_snapshot()
        };
        if let Ok(mut cache) = self.skills_cache.try_write()
            && let Some(entry) = cache.get_mut(key)
        {
            entry.touch();
        }
        Some(snapshot)
    }

    pub(super) async fn skills_cache_epoch(&self, name: &str) -> u64 {
        let mut epochs = self.skills_cache_epochs.write().await;
        *epochs.entry(name.to_owned()).or_insert(0)
    }

    /// Fetch one upstream's catalog and store it.
    pub(super) async fn fetch_and_cache_skills(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
    ) -> Result<CachedSkills, UpstreamSkillsError> {
        // Capture the per-upstream epoch before any network I/O. Invalidation
        // advances the epoch first, so a response from an older connection or
        // configuration can never repopulate the cache afterward.
        let expected_epoch = self.skills_cache_epoch(&config.name).await;

        // Upstreams connect lazily: a cold gateway has a seeded catalog entry but
        // no live connection until something asks for one, so acquiring the peer
        // directly reports "not connected" on every first read — the normal
        // state for `labby mcp`, not an error.
        //
        // The pool checks connection presence under its lazy-connect lock;
        // skills-only peers do not need an exposed tool to remain reusable.
        if self
            .ensure_connection_for_upstream(config, subject, None)
            .await
            .is_err()
        {
            return Err(UpstreamSkillsError::Unavailable);
        }
        let peer = self
            .acquire_peer(
                &config.name,
                super::super::types::UpstreamCapability::Skills,
                "skills.list",
            )
            .await
            .ok_or(UpstreamSkillsError::Unavailable)?;

        // An upstream that never declared the extension is not a failure — it
        // simply has no skills, and caching that avoids re-asking every read.
        if !peer_declares_skills(&peer) {
            let empty = CachedSkills::new(UpstreamSkills::default());
            if !self
                .store_skills(&config.name, subject, expected_epoch, empty.clone())
                .await
            {
                return Err(UpstreamSkillsError::Invalidated);
            }
            let mut catalog = self.catalog_write().await;
            if let Some(catalog_entry) = catalog.get_mut(&config.name) {
                catalog_entry.supports_skills = Some(false);
                catalog_entry.skill_count = 0;
                catalog_entry.skill_names.clear();
            }
            return Ok(empty);
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
                if !self
                    .store_skills(&config.name, subject, expected_epoch, entry.clone())
                    .await
                {
                    return Err(UpstreamSkillsError::Invalidated);
                }
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
            // The timed capability call owns connection-health accounting. A valid
            // MCP error proves the transport is alive and must not be reclassified as
            // a circuit-breaker failure here.
            Err(error) => Err(error),
        }
    }

    pub(super) async fn store_skills(
        &self,
        name: &str,
        subject: Option<&str>,
        expected_epoch: u64,
        entry: CachedSkills,
    ) -> bool {
        // Hold the epoch read guard across publication. Invalidation advances the
        // epoch before taking the cache lock, so either this publish wins first and
        // is then removed, or it observes the newer epoch and is discarded.
        let epochs = self.skills_cache_epochs.read().await;
        if epochs.get(name).copied().unwrap_or_default() != expected_epoch {
            return false;
        }
        let mut cache = self.skills_cache.write().await;
        let key = (name.to_string(), subject.map(str::to_string));
        let mut entry = entry;
        if let Some(previous) = cache.get(&key) {
            entry.retain_direct_from(previous);
        }
        cache.insert(key, entry);
        let evicted = evict(&mut cache);
        if evicted > 0 {
            tracing::info!(
                action = "skills.cache.evict",
                upstream = %name,
                evicted,
                retained = cache.len(),
                "evicted bounded Skill catalog cache entries"
            );
        }
        true
    }

    /// Refresh an expired snapshot behind the caller.
    ///
    /// Marks the entry refreshing first so a burst of readers past the TTL
    /// spawns one task rather than one per reader.
    fn spawn_skills_refresh(&self, config: UpstreamConfig, subject: Option<String>) {
        let pool = self.clone();
        let cancel = self.skills_refresh_cancel.clone();
        self.skills_refresh_tasks.spawn(async move {
            let key = (config.name.clone(), subject.clone());
            {
                let mut cache = pool.skills_cache.write().await;
                let Some(entry) = cache.get_mut(&key) else {
                    return;
                };
                if !entry.begin_refresh() {
                    return;
                }
            }
            // If this task panics or is aborted, Drop schedules a fail-refresh
            // transition so stale-while-revalidate cannot remain permanently armed.
            let mut state_guard =
                RefreshStateGuard::new(Arc::clone(&pool.skills_cache), key.clone());
            let result = tokio::select! {
                _ = cancel.cancelled() => return,
                result = pool.fetch_and_cache_skills(&config, subject.as_deref()) => result,
            };
            if result.is_err() {
                // Keep serving the stale snapshot, but do not retry a failed
                // refresh on every read. The bounded cooldown grows with repeated
                // failures and resets when a new snapshot is published.
                let mut cache = pool.skills_cache.write().await;
                if let Some(entry) = cache.get_mut(&key) {
                    entry.fail_refresh();
                }
            }
            state_guard.disarm();
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
            refreshing: cached.refreshing,
            refresh_age_secs: cached.refresh_age().map(|age| age.as_secs()),
            retry_after_ms: cached
                .retry_after()
                .map(|remaining| remaining.as_millis().min(u128::from(u64::MAX)) as u64),
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
    pub(crate) async fn fetch_unlisted_skill(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        uri: &str,
    ) -> Result<Option<ValidatedSkill>, UpstreamSkillsError> {
        if !config.proxy_skills {
            return Ok(None);
        }
        let canonical_uri = parse_skill_resource_uri(uri)
            .map_err(|_| UpstreamSkillsError::InvalidUri)?
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
            .ok_or(UpstreamSkillsError::Unavailable)?;
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
            return Err(UpstreamSkillsError::IdentityMismatch);
        }
        self.store_direct_skill(config, subject, skill.clone())
            .await?;
        Ok(Some(skill))
    }

    pub(super) async fn cached_direct_skill(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        skill_uri: &str,
    ) -> Option<ValidatedSkill> {
        let policy =
            resolve_request_skill_exposure_policy(&config.name, config.expose_skills.clone());
        let subject = skills_cache_subject(config, subject);
        let key = (config.name.clone(), subject.map(str::to_string));
        let mut cache = self.skills_cache.write().await;
        let cached = cache.get_mut(&key)?;
        cached.touch();
        cached.prune_direct();
        let snapshot = cached.direct.get_mut(skill_uri)?;
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
    ) -> Result<(), UpstreamSkillsError> {
        let subject = skills_cache_subject(config, subject);
        let key = (config.name.clone(), subject.map(str::to_string));
        let mut cache = self.skills_cache.write().await;
        let cached = cache
            .get_mut(&key)
            .ok_or(UpstreamSkillsError::CacheMissing)?;
        cached.touch();
        cached.prune_direct();
        let owner = skill.entry.uri.clone();
        let candidate = CachedDirectSkill::new(skill);

        // Listed collisions are poisoned regardless of exposure. Direct-owner
        // collisions use the precomputed resource index rather than reparsing
        // every resource of every retained direct manifest under the cache lock.
        if candidate
            .owned_uris
            .iter()
            .any(|uri| cached.skills.resource_index.contains_key(uri))
            || candidate.owned_uris.iter().any(|uri| {
                cached
                    .direct_resource_index
                    .get(uri)
                    .is_some_and(|existing| existing != &owner)
            })
        {
            return Err(UpstreamSkillsError::Collision);
        }
        if cached.direct.len() >= limits::MAX_SKILLS_PER_UPSTREAM
            && !cached.direct.contains_key(&owner)
        {
            return Err(UpstreamSkillsError::LimitExceeded);
        }
        cached.insert_direct(owner, candidate);
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
        let subject = skills_cache_subject(config, subject);
        let key = (config.name.clone(), subject.map(str::to_string));
        let mut cache = self.skills_cache.write().await;
        let cached = cache.get_mut(&key)?;
        cached.touch();
        cached.prune_direct();
        let owner_uri = cached.direct_resource_index.get(&canonical_uri)?.clone();
        let owner = cached.direct.get_mut(&owner_uri)?;
        if !policy.matches(&owner.skill.name) {
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
        // Advance the epoch before touching the cache. Any refresh that captured
        // the previous value is prevented from publishing after this point.
        {
            let mut epochs = self.skills_cache_epochs.write().await;
            let epoch = epochs.entry(name.to_owned()).or_insert(0);
            *epoch = epoch.wrapping_add(1);
        }
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
        {
            let mut epochs = self.skills_cache_epochs.write().await;
            for epoch in epochs.values_mut() {
                *epoch = epoch.wrapping_add(1);
            }
        }
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
