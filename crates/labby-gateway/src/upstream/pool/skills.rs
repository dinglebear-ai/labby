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

use labby_runtime::gateway_config::UpstreamConfig;
use labby_runtime::skills::ValidatedSkill;

use super::UpstreamPool;
use super::entries::{log_exposure_filter, resolve_request_skill_exposure_policy};
use super::skills_cache::{CachedSkills, evict};
use super::skills_list::{UpstreamSkills, peer_declares_skills};

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
                return Ok(self.apply_skill_exposure(config, &cached, subject));
            }
            let stale = self.apply_skill_exposure(config, &cached, subject);
            self.spawn_skills_refresh(config.clone(), subject.map(str::to_string));
            return Ok(stale);
        }

        // Cold: one caller fetches while the rest wait on the same guard, so a
        // burst of downstream listings makes one upstream request.
        let guard = self.skills_fetch_locks.guard_for(&key).await;
        let _held = guard.lock().await;
        if let Some(cached) = self.cached_skills(&key).await {
            return Ok(self.apply_skill_exposure(config, &cached, subject));
        }

        let snapshot = self.fetch_and_cache_skills(config, subject).await?;
        Ok(self.apply_skill_exposure(config, &snapshot, subject))
    }

    /// Read a cached snapshot, marking it used for idle eviction.
    async fn cached_skills(&self, key: &(String, Option<String>)) -> Option<CachedSkills> {
        let mut cache = self.skills_cache.write().await;
        let entry = cache.get_mut(key)?;
        entry.touch();
        Some(entry.clone())
    }

    /// Fetch one upstream's catalog and store it.
    async fn fetch_and_cache_skills(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
    ) -> Result<CachedSkills, String> {
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
            let empty = CachedSkills::new(UpstreamSkills::default());
            self.store_skills(&config.name, subject, empty.clone())
                .await;
            return Ok(empty);
        }

        match self.fetch_upstream_skills(&config.name, &peer).await {
            Ok(skills) => {
                let count = skills.skills.len();
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
                    let mut catalog = self.catalog.write().await;
                    if let Some(catalog_entry) = catalog.get_mut(&config.name) {
                        catalog_entry.skill_count = count;
                    }
                }
                for (reason, uri) in &excluded {
                    tracing::warn!(
                        upstream = %config.name,
                        reason = reason.as_str(),
                        skill = %super::helpers::redact_resource_uri_for_logging(uri),
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
        cache.insert((name.to_string(), subject.map(str::to_string)), entry);
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
    ) -> ExposedSkills {
        let policy =
            resolve_request_skill_exposure_policy(&config.name, config.expose_skills.clone());
        let total = cached.skills.skills.len();
        let skills: Vec<ValidatedSkill> = cached
            .skills
            .skills
            .iter()
            .filter(|skill| policy.matches(&skill.name))
            .cloned()
            .collect();
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
        }
    }

    /// Drop every cached skill catalog for one upstream, across all subjects.
    ///
    /// Called on reload and on disconnect: a snapshot outliving the connection
    /// it came from would serve a catalog Labby can no longer honor a read
    /// against.
    pub async fn invalidate_upstream_skills(&self, name: &str) {
        let mut cache = self.skills_cache.write().await;
        cache.retain(|(upstream, _), _| upstream != name);
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
