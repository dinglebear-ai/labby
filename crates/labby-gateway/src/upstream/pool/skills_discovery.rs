//! Request-bounded discovery without weakening full catalog/cache semantics.

use labby_runtime::gateway_config::UpstreamConfig;
use labby_runtime::skills::{SkillDiscoverySource, limits};

use super::UpstreamPool;
use super::skills::{ExposedSkills, skills_cache_subject};
use super::skills_cache::CachedSkills;
use super::skills_list::UpstreamSkillsError;

impl UpstreamPool {
    /// Smaller requests reuse an existing snapshot or perform an uncached
    /// preview under the same acquisition lock and invalidation epoch. A
    /// preview is never authoritative absence for a later full list/get/read.
    pub(super) async fn discover_upstream_skills(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        max_items: usize,
    ) -> Result<ExposedSkills, UpstreamSkillsError> {
        if !config.proxy_skills {
            return Ok(ExposedSkills::default());
        }
        if max_items >= limits::MAX_SKILLS_PER_UPSTREAM {
            return self.upstream_skills(config, subject).await;
        }
        let subject = skills_cache_subject(config, subject);
        let key = (config.name.clone(), subject.map(str::to_owned));
        if let Some(cached) = self
            .cached_skills(&key)
            .await
            .filter(CachedSkills::is_fresh)
        {
            // Reuse only fresh snapshots. Expired catalogs take the bounded
            // fetch path below rather than serving stale data indefinitely or
            // launching an unbounded-for-this-request background traversal.
            return Ok(self.apply_skill_exposure_with_limit(
                config,
                &cached,
                subject,
                SkillDiscoverySource::Cached,
                max_items,
            ));
        }
        let guard = self.skills_fetch_locks.guard_for(&key).await;
        let _held = guard.lock().await;
        if let Some(cached) = self
            .cached_skills(&key)
            .await
            .filter(CachedSkills::is_fresh)
        {
            return Ok(self.apply_skill_exposure_with_limit(
                config,
                &cached,
                subject,
                SkillDiscoverySource::Cached,
                max_items,
            ));
        }
        let epoch = self.skills_cache_epoch(&config.name).await;
        let Some(peer) = self.skills_discovery_peer(config, subject).await? else {
            return Ok(ExposedSkills::default());
        };
        let skills = self
            .fetch_upstream_skills_with_limit(&config.name, &peer, max_items)
            .await?;
        if self.skills_cache_epoch(&config.name).await != epoch {
            return Err(UpstreamSkillsError::Invalidated);
        }
        let preview = CachedSkills::new(skills);
        Ok(self.apply_skill_exposure_with_limit(
            config,
            &preview,
            subject,
            SkillDiscoverySource::Refreshed,
            max_items,
        ))
    }
}
