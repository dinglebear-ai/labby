//! Request-bounded discovery without weakening full catalog/cache semantics.

use std::time::{Duration, Instant};

use labby_runtime::gateway_config::UpstreamConfig;
use labby_runtime::skills::{SkillDiscoverySource, limits};

use super::UpstreamPool;
use super::entries::resolve_request_skill_exposure_policy;
use super::skills::{ExposedSkills, skills_cache_subject};
use super::skills_cache::CachedSkills;
use super::skills_list::{UpstreamSkillSearch, UpstreamSkillsError};

impl UpstreamPool {
    pub(super) async fn search_upstream_skills(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        query: &str,
        max_items: usize,
        budget: Duration,
    ) -> Result<UpstreamSkillSearch, UpstreamSkillsError> {
        if !config.proxy_skills {
            return Ok(UpstreamSkillSearch {
                skills: Vec::new(),
                incomplete: false,
            });
        }
        let max_items = max_items.clamp(1, limits::MAX_SKILLS_PER_UPSTREAM);
        let subject = skills_cache_subject(config, subject);
        let key = (config.name.clone(), subject.map(str::to_owned));
        let deadline = Instant::now() + budget;
        let snapshot = if self.cached_skills(&key).await.is_some() {
            self.upstream_skills_snapshot(config, subject).await?.0
        } else {
            let guard = self.skills_fetch_locks.guard_for(&key).await;
            let mut state = tokio::time::timeout(budget, guard.lock())
                .await
                .map_err(|_| UpstreamSkillsError::SearchIncomplete)?;
            let epoch = self.skills_cache_epoch(&config.name).await;
            if let Some(snapshot) = self.cached_skills(&key).await.or_else(|| {
                state.preview(
                    epoch,
                    limits::MAX_SKILLS_PER_UPSTREAM,
                    &config.expose_skills,
                )
            }) {
                snapshot
            } else {
                let Some(peer) = self.skills_discovery_peer(config, subject).await? else {
                    return Ok(UpstreamSkillSearch {
                        skills: Vec::new(),
                        incomplete: false,
                    });
                };
                let catalog = self
                    .fetch_upstream_skills_with_limit(
                        &config.name,
                        &peer,
                        limits::MAX_SKILLS_PER_UPSTREAM,
                        None,
                        Some(deadline),
                    )
                    .await?;
                if self.skills_cache_epoch(&config.name).await != epoch {
                    return Err(UpstreamSkillsError::Invalidated);
                }
                let snapshot = CachedSkills::new(catalog);
                if snapshot.skills.truncated {
                    state.share_preview(
                        epoch,
                        limits::MAX_SKILLS_PER_UPSTREAM,
                        config.expose_skills.clone(),
                        snapshot.read_snapshot(),
                    );
                } else if !self
                    .store_skills(&config.name, subject, epoch, snapshot.read_snapshot())
                    .await
                {
                    return Err(UpstreamSkillsError::Invalidated);
                }
                snapshot
            }
        };
        let query = query.trim().to_ascii_lowercase();
        let policy =
            resolve_request_skill_exposure_policy(&config.name, config.expose_skills.clone());
        let skills = snapshot
            .skills
            .skills
            .iter()
            .filter(|skill| {
                policy.matches(&skill.name)
                    && !snapshot.skills.has_ambiguous_claim(skill)
                    && format!(
                        "{} {} {}",
                        skill.entry.uri,
                        skill.name,
                        skill
                            .entry
                            .frontmatter_str("description")
                            .unwrap_or_default()
                    )
                    .to_ascii_lowercase()
                    .contains(&query)
            })
            .take(max_items)
            .cloned()
            .collect();
        Ok(UpstreamSkillSearch {
            skills,
            incomplete: snapshot.skills.truncated,
        })
    }

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
        let mut state = guard.lock().await;
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
        if let Some(preview) = state.preview(epoch, max_items, &config.expose_skills) {
            return Ok(self.apply_skill_exposure_with_limit(
                config,
                &preview,
                subject,
                SkillDiscoverySource::Cached,
                max_items,
            ));
        }
        let Some(peer) = self.skills_discovery_peer(config, subject).await? else {
            return Ok(ExposedSkills::default());
        };
        let exposure =
            resolve_request_skill_exposure_policy(&config.name, config.expose_skills.clone());
        let skills = self
            .fetch_upstream_skills_with_limit(&config.name, &peer, max_items, Some(&exposure), None)
            .await?;
        if self.skills_cache_epoch(&config.name).await != epoch {
            return Err(UpstreamSkillsError::Invalidated);
        }
        let preview = CachedSkills::new(skills);
        state.share_preview(
            epoch,
            max_items,
            config.expose_skills.clone(),
            preview.read_snapshot(),
        );
        Ok(self.apply_skill_exposure_with_limit(
            config,
            &preview,
            subject,
            SkillDiscoverySource::Refreshed,
            max_items,
        ))
    }
}
