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

    /// Provider requests use the gateway's configured operation timeout unless
    /// the caller deliberately supplies a shorter deadline. This preserves the
    /// gateway timeout contract while still allowing callers to tighten it.
    fn operation_timeout(&self, requested: Duration) -> Duration {
        requested.min(self.pool.request_timeout)
    }

    /// Recover the exact direct-get manifest that uniquely owns `resource_id`.
    ///
    /// The returned entry is still provider-scoped. Callers must pass its id
    /// back to `read_resource`; this lookup does not authorize a resource-only
    /// read and rechecks the live exposure policy before returning anything.
    pub async fn cached_owner_for_resource(&self, resource_id: &str) -> Option<SkillProviderEntry> {
        self.pool
            .cached_unlisted_skill_owner(&self.config, self.subject.as_deref(), resource_id)
            .await
            .map(|skill| SkillProviderEntry::from_validated(self.id.clone(), skill))
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
                self.operation_timeout(request.deadline.timeout),
                self.pool
                    .upstream_skills(&self.config, self.subject.as_deref()),
            )
            .await
            .map_err(|_| SkillProviderError::DeadlineExceeded)?
            .map_err(|reason| SkillProviderError::Unavailable { reason })?;
            let available = exposed.skills.len();
            let skills = exposed
                .skills
                .into_iter()
                .take(request.max_items)
                .map(|skill| SkillProviderEntry::from_validated(self.id.clone(), skill))
                .collect();
            let result = SkillDiscoverResult {
                skills,
                source: exposed.source,
                cache_age: (exposed.source == SkillDiscoverySource::Cached)
                    .then(|| Duration::from_secs(exposed.age_secs)),
                ttl: exposed.ttl_ms.map(Duration::from_millis),
                excluded_count: exposed.excluded_count,
                truncated: exposed.truncated || available > request.max_items,
            };
            result.validate_for(&self.id, request)?;
            Ok(result)
        })
    }

    fn get<'a>(&'a self, request: &'a SkillGetRequest) -> SkillProviderFuture<'a, SkillGetResult> {
        Box::pin(async move {
            request.validate()?;
            self.validate_provider(request.id.provider())?;
            let operation = async {
                let exposed = self
                    .pool
                    .upstream_skills(&self.config, self.subject.as_deref())
                    .await
                    .map_err(|reason| SkillProviderError::Unavailable { reason })?;
                let skill = if let Some(skill) = exposed
                    .skills
                    .iter()
                    .find(|skill| skill.entry.uri == request.id.source_id())
                    .cloned()
                {
                    Some(skill)
                } else {
                    self.pool
                        .fetch_unlisted_skill(
                            &self.config,
                            self.subject.as_deref(),
                            request.id.source_id(),
                        )
                        .await
                        .map_err(|reason| SkillProviderError::Provider { reason })?
                };
                let skill = skill.ok_or(SkillProviderError::SkillNotFound)?;
                let result = SkillGetResult {
                    skill: SkillProviderEntry::from_validated(self.id.clone(), skill),
                };
                result.validate_for(&self.id, request)?;
                Ok(result)
            };
            tokio::time::timeout(self.operation_timeout(request.deadline.timeout), operation)
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
            self.validate_provider(request.skill_id.provider())?;
            let verified = tokio::time::timeout(
                self.operation_timeout(request.deadline.timeout),
                self.pool.read_proxied_skill_file_for_skill(
                    &self.config,
                    self.subject.as_deref(),
                    request.skill_id.source_id(),
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
                    SkillProviderError::ManifestStale
                }
                _ => SkillProviderError::Provider {
                    reason: error.to_string(),
                },
            })?;
            let result = SkillResourceReadResult {
                skill_id: request.skill_id.clone(),
                resource_id: request.resource_id.clone(),
                bytes: verified.bytes,
                media_type: verified.mime_type,
                representation: if verified.is_blob {
                    labby_runtime::skills::SkillResourceRepresentation::Blob
                } else {
                    labby_runtime::skills::SkillResourceRepresentation::Text
                },
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

    #[test]
    fn provider_default_uses_configured_timeout_and_explicit_shorter_deadline_wins() {
        let configured = Duration::from_secs(30);
        let pool = Arc::new(UpstreamPool::new().with_request_timeout(configured));
        let provider = SepSkillProvider::new(
            pool,
            super::super::testsupport::named_test_upstream_config("deadline-test"),
            None,
        );

        assert_eq!(
            provider.operation_timeout(
                labby_runtime::skills::SkillProviderDeadline::default().timeout,
            ),
            configured
        );
        assert_eq!(
            provider.operation_timeout(Duration::from_millis(25)),
            Duration::from_millis(25)
        );
    }
}
