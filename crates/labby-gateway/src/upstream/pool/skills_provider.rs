//! Provider-neutral adapter over the existing SEP-2640 upstream runtime.

use std::sync::Arc;
use std::time::Duration;

use labby_runtime::gateway_config::UpstreamConfig;
use labby_runtime::skills::{
    SkillDiscoverRequest, SkillDiscoverResult, SkillDiscoverySource, SkillGetRequest,
    SkillGetResult, SkillProvider, SkillProviderEntry, SkillProviderError, SkillProviderFuture,
    SkillProviderId, SkillProviderKind, SkillResourceReadRequest, SkillResourceReadResult,
};

use super::UpstreamPool;

/// One caller-scoped SEP-2640 upstream exposed through the neutral provider seam.
///
/// The subject is captured by the adapter so cached discovery and every direct
/// get/read retain the gateway's existing isolation boundary.
#[derive(Clone)]
pub struct SepSkillProvider {
    id: SkillProviderId,
    pool: Arc<UpstreamPool>,
    config: UpstreamConfig,
    subject: Option<String>,
}

impl SepSkillProvider {
    #[must_use]
    pub fn new(pool: Arc<UpstreamPool>, config: UpstreamConfig, subject: Option<String>) -> Self {
        let id = SkillProviderId::new(SkillProviderKind::McpUpstream, config.name.clone());
        Self {
            id,
            pool,
            config,
            subject,
        }
    }

    fn validate_provider(&self, requested: &SkillProviderId) -> Result<(), SkillProviderError> {
        if requested != &self.id {
            return Err(SkillProviderError::WrongProvider);
        }
        Ok(())
    }
}

impl SkillProvider for SepSkillProvider {
    fn id(&self) -> &SkillProviderId {
        &self.id
    }

    fn discover<'a>(
        &'a self,
        request: &'a SkillDiscoverRequest,
    ) -> SkillProviderFuture<'a, SkillDiscoverResult> {
        Box::pin(async move {
            request.validate()?;
            let exposed = tokio::time::timeout(
                request.deadline.timeout,
                self.pool
                    .upstream_skills(&self.config, self.subject.as_deref()),
            )
            .await
            .map_err(|_| SkillProviderError::DeadlineExceeded)?
            .map_err(|reason| SkillProviderError::Unavailable { reason })?;
            let available = exposed.skills.len();
            let skills = exposed
                .skills
                .iter()
                .take(request.max_items)
                .map(|skill| SkillProviderEntry::from_validated(self.id.clone(), skill))
                .collect();
            Ok(SkillDiscoverResult {
                skills,
                source: exposed.source,
                cache_age: (exposed.source == SkillDiscoverySource::Cached)
                    .then(|| Duration::from_secs(exposed.age_secs)),
                ttl: exposed.ttl_ms.map(Duration::from_millis),
                excluded_count: exposed.excluded_count,
                truncated: exposed.truncated || available > request.max_items,
            })
        })
    }

    fn get<'a>(&'a self, request: &'a SkillGetRequest) -> SkillProviderFuture<'a, SkillGetResult> {
        Box::pin(async move {
            request.validate()?;
            self.validate_provider(&request.id.provider)?;
            let operation = async {
                let exposed = self
                    .pool
                    .upstream_skills(&self.config, self.subject.as_deref())
                    .await
                    .map_err(|reason| SkillProviderError::Unavailable { reason })?;
                let skill = if let Some(skill) = exposed
                    .skills
                    .iter()
                    .find(|skill| skill.entry.uri == request.id.source_id)
                    .cloned()
                {
                    Some(skill)
                } else {
                    self.pool
                        .fetch_unlisted_skill(
                            &self.config,
                            self.subject.as_deref(),
                            &request.id.source_id,
                        )
                        .await
                };
                let skill = skill.ok_or(SkillProviderError::SkillNotFound)?;
                Ok(SkillGetResult {
                    skill: SkillProviderEntry::from_validated(self.id.clone(), &skill),
                })
            };
            tokio::time::timeout(request.deadline.timeout, operation)
                .await
                .map_err(|_| SkillProviderError::DeadlineExceeded)?
        })
    }

    fn read_resource<'a>(
        &'a self,
        request: &'a SkillResourceReadRequest,
    ) -> SkillProviderFuture<'a, SkillResourceReadResult> {
        Box::pin(async move {
            request.validate()?;
            self.validate_provider(&request.skill_id.provider)?;
            let verified = tokio::time::timeout(
                request.deadline.timeout,
                self.pool.read_proxied_skill_file_for_skill(
                    &self.config,
                    self.subject.as_deref(),
                    &request.skill_id.source_id,
                    &request.resource_id,
                    request.max_bytes,
                ),
            )
            .await
            .map_err(|_| SkillProviderError::DeadlineExceeded)?
            .map_err(|error| match error.kind() {
                "response_too_large" => SkillProviderError::LimitExceeded {
                    what: "resource_bytes",
                    limit: request.max_bytes,
                },
                labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH => {
                    SkillProviderError::Integrity {
                        reason: "digest_or_frontmatter_mismatch",
                    }
                }
                labby_runtime::skills::KIND_SKILL_MANIFEST_STALE => {
                    SkillProviderError::ResourceNotFound
                }
                _ => SkillProviderError::Provider {
                    reason: error.to_string(),
                },
            })?;
            let result = SkillResourceReadResult {
                skill_id: request.skill_id.clone(),
                resource_id: request.resource_id.clone(),
                bytes: verified.text.into_bytes(),
                media_type: verified.mime_type,
            };
            result.validate_for(request)?;
            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_identity_is_scoped_to_the_upstream() {
        let id = SkillProviderId::new(SkillProviderKind::McpUpstream, "docs");
        let other = SkillProviderId::new(SkillProviderKind::McpUpstream, "private");
        assert_ne!(id, other);
    }
}
