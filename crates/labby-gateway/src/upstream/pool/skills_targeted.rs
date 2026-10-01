//! Targeted retrieval overlaps cold catalog I/O while retaining ownership validation.

use labby_runtime::gateway_config::UpstreamConfig;
use labby_runtime::skills::{ValidatedSkill, parse_skill_resource_uri};

use super::UpstreamPool;
use super::entries::resolve_request_skill_exposure_policy;
use super::skills::{ExposedSkills, skills_cache_subject};
use super::skills_list::UpstreamSkillsError;

impl UpstreamPool {
    pub(super) async fn targeted_skill_with_catalog(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        uri: &str,
    ) -> Result<(Option<ValidatedSkill>, ExposedSkills), UpstreamSkillsError> {
        let canonical_uri = parse_skill_resource_uri(uri)
            .map_err(|_| UpstreamSkillsError::InvalidUri)?
            .to_uri();
        let subject = skills_cache_subject(config, subject);
        let epoch = self.skills_cache_epoch(&config.name).await;
        let key = (config.name.clone(), subject.map(str::to_owned));
        let cold = self.cached_skills(&key).await.is_none();
        tracing::debug!(surface = "dispatch", service = "skills", action = "get", upstream = %config.name,
            cold_catalog = cold, subject_scoped = subject.is_some(), "starting targeted Skill ownership resolution");
        // Speculation may use spare RPC capacity, never the only permit needed
        // by the authoritative catalog. Existing callers can exhaust a larger
        // configured bulkhead too, so inspect currently available capacity.
        let spare_capacity = self.call_concurrency > 1
            && self
                .call_semaphores
                .read()
                .await
                .get(&config.name)
                .map_or(self.call_concurrency, |semaphore| {
                    semaphore.available_permits()
                })
                >= 2;
        let catalog = self.upstream_skills(config, subject);
        let candidate = async {
            if let Some(skill) = self
                .cached_direct_skill(config, subject, &canonical_uri)
                .await
            {
                return Ok(Some(skill));
            }
            let Some(peer) = self.skills_discovery_peer(config, subject).await? else {
                return Ok(None);
            };
            self.fetch_upstream_skill(&config.name, &peer, &canonical_uri, subject)
                .await
        };
        tokio::pin!(catalog, candidate);
        // Warm listed entries need no direct RPC. On a cold cache, get and list
        // overlap, but catalog authority always wins and admission waits for it.
        let (exposed, fetched) = if cold && config.proxy_skills && spare_capacity {
            tokio::select! {
                biased;
                exposed = &mut catalog => (exposed?, None),
                fetched = &mut candidate => (catalog.await?, Some(fetched)),
            }
        } else {
            (catalog.await?, None)
        };
        if let Some(skill) = exposed
            .skills
            .iter()
            .find(|skill| skill.entry.uri == canonical_uri)
            .cloned()
        {
            return Ok((Some(skill), exposed));
        }
        if !config.proxy_skills {
            return Ok((None, exposed));
        }
        let fetched = match fetched {
            Some(fetched) => fetched?,
            None => candidate.await?,
        };
        let Some(skill) = fetched else {
            return Ok((None, exposed));
        };
        let policy =
            resolve_request_skill_exposure_policy(&config.name, config.expose_skills.clone());
        if !policy.matches(&skill.name) {
            return Ok((None, exposed));
        }
        if skill.entry.uri != canonical_uri {
            return Err(UpstreamSkillsError::IdentityMismatch);
        }
        // A late get must not attach its manifest to a newer credential/catalog.
        // Keep the lifecycle fence held through the existing collision admission.
        let epochs = self.skills_cache_epochs.read().await;
        if epochs.get(&config.name).copied().unwrap_or_default() != epoch {
            return Err(UpstreamSkillsError::Invalidated);
        }
        self.store_direct_skill(config, subject, skill.clone())
            .await?;
        drop(epochs);
        Ok((Some(skill), exposed))
    }
}
