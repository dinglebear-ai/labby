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
use super::capability_call::CapabilityCallError;
use super::skills_list::UpstreamSkillsError;

/// Provider entries and whether an upstream traversal stopped before exhaustion.
pub struct SkillSearchResult {
    pub skills: Vec<SkillProviderEntry>,
    pub incomplete: bool,
}

fn map_upstream_skills_error(error: UpstreamSkillsError) -> SkillProviderError {
    match error {
        UpstreamSkillsError::Capability(CapabilityCallError::Timeout { .. })
        | UpstreamSkillsError::SearchIncomplete => SkillProviderError::DeadlineExceeded,
        UpstreamSkillsError::Capability(CapabilityCallError::ResponseTooLarge { .. }) => {
            SkillProviderError::LimitExceeded {
                what: "response_bytes",
                limit: labby_runtime::skills::limits::MAX_SKILL_RESOURCE_BYTES,
            }
        }
        UpstreamSkillsError::Capability(CapabilityCallError::Transport { .. })
        | UpstreamSkillsError::Unavailable
        | UpstreamSkillsError::Invalidated
        | UpstreamSkillsError::CacheMissing => SkillProviderError::Unavailable {
            reason: "upstream_unavailable".to_owned(),
        },
        UpstreamSkillsError::Capability(CapabilityCallError::QueueSaturated { .. }) => {
            SkillProviderError::Unavailable {
                reason: "upstream_queue_saturated".to_owned(),
            }
        }
        UpstreamSkillsError::LimitExceeded => SkillProviderError::LimitExceeded {
            what: "direct_skills",
            limit: labby_runtime::skills::limits::MAX_SKILLS_PER_UPSTREAM,
        },
        UpstreamSkillsError::Collision => SkillProviderError::ManifestStale,
        UpstreamSkillsError::InvalidUri => SkillProviderError::InvalidRequest {
            field: "skill.source_id",
            reason: "invalid_skill_uri",
        },
        UpstreamSkillsError::InvalidManifest { reason } => SkillProviderError::Integrity { reason },
        UpstreamSkillsError::Capability(CapabilityCallError::Mcp { .. }) => {
            SkillProviderError::Provider {
                reason: "upstream_mcp_error".to_owned(),
            }
        }
        UpstreamSkillsError::Capability(CapabilityCallError::Protocol { .. })
        | UpstreamSkillsError::CacheScopeChanged
        | UpstreamSkillsError::IdentityMismatch => SkillProviderError::Provider {
            reason: "upstream_protocol_error".to_owned(),
        },
        UpstreamSkillsError::Capability(CapabilityCallError::Cancelled { .. }) => {
            SkillProviderError::Provider {
                reason: "upstream_cancelled".to_owned(),
            }
        }
        UpstreamSkillsError::Capability(CapabilityCallError::InputRequiredRoundsExceeded {
            ..
        }) => SkillProviderError::Provider {
            reason: "input_required_rounds_exceeded".to_owned(),
        },
        UpstreamSkillsError::Capability(CapabilityCallError::Other { .. }) => {
            SkillProviderError::Provider {
                reason: "upstream_error".to_owned(),
            }
        }
    }
}

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

    /// Peek at a fresh caller-scoped immutable catalog without network I/O.
    /// Cold and expired catalogs return None so the ordinary list operation
    /// owns the single bounded discovery attempt and background refresh.
    pub async fn catalog_revision(&self) -> Option<(u64, Duration)> {
        let subject = super::skills::skills_cache_subject(&self.config, self.subject.as_deref());
        let key = (self.config.name.clone(), subject.map(str::to_string));
        let snapshot = self.pool.cached_skills(&key).await?;
        snapshot
            .is_fresh()
            .then(|| (snapshot.revision, snapshot.remaining_ttl()))
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

    /// Resolve a target and its ownership catalog under one operation deadline.
    /// Cold direct retrieval overlaps enumeration; no target is admitted before
    /// catalog-wide collisions and lifecycle invalidation have been checked.
    pub async fn get_with_catalog(
        &self,
        request: &SkillGetRequest,
    ) -> Result<(SkillGetResult, SkillDiscoverResult), SkillProviderError> {
        request.validate()?;
        self.validate_provider(request.id.provider())?;
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            self.operation_timeout(request.deadline.timeout),
            self.pool.targeted_skill_with_catalog(
                &self.config,
                self.subject.as_deref(),
                request.id.source_id(),
            ),
        )
        .await
        .map_err(|_| SkillProviderError::DeadlineExceeded)?
        .map_err(map_upstream_skills_error)?;
        let (skill, exposed) = result;
        let skill = skill.ok_or(SkillProviderError::SkillNotFound)?;
        let get = SkillGetResult {
            skill: SkillProviderEntry::from_validated(self.id.clone(), skill),
        };
        get.validate_for(&self.id, request)?;
        let catalog = SkillDiscoverResult {
            skills: exposed
                .skills
                .into_iter()
                .map(|skill| SkillProviderEntry::from_validated(self.id.clone(), skill))
                .collect(),
            source: exposed.source,
            cache_age: (exposed.source == SkillDiscoverySource::Cached)
                .then(|| Duration::from_secs(exposed.age_secs)),
            ttl: exposed.ttl_ms.map(Duration::from_millis),
            excluded_count: exposed.excluded_count,
            truncated: exposed.truncated,
        };
        tracing::debug!(surface = "dispatch", service = "skills", action = "get", upstream = %self.config.name,
            source = ?catalog.source, catalog_items = catalog.skills.len(), elapsed_ms = started.elapsed().as_millis(),
            "completed targeted Skill retrieval with ownership catalog");
        Ok((get, catalog))
    }

    /// Search paginated Skill metadata without letting non-matching prefix
    /// entries consume the caller's result budget.
    pub async fn search(
        &self,
        query: &str,
        max_items: usize,
    ) -> Result<SkillSearchResult, SkillProviderError> {
        let timeout = self.operation_timeout(labby_runtime::skills::limits::SKILLS_LIST_TIMEOUT);
        // Leave a small margin so the inner page deadline can return partial
        // matches before the provider's outer deadline cancels the whole search.
        let traversal_budget =
            timeout.saturating_sub((timeout / 5).min(Duration::from_millis(250)));
        let skills = tokio::time::timeout(
            timeout,
            self.pool.search_upstream_skills(
                &self.config,
                self.subject.as_deref(),
                query,
                max_items,
                traversal_budget,
            ),
        )
        .await
        .map_err(|_| SkillProviderError::DeadlineExceeded)?
        .map_err(map_upstream_skills_error)?;
        Ok(SkillSearchResult {
            skills: skills
                .skills
                .into_iter()
                .map(|skill| SkillProviderEntry::from_validated(self.id.clone(), skill))
                .collect(),
            incomplete: skills.incomplete,
        })
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
            let started = std::time::Instant::now();
            let exposed = tokio::time::timeout(
                self.operation_timeout(request.deadline.timeout),
                self.pool.discover_upstream_skills(
                    &self.config,
                    self.subject.as_deref(),
                    request.max_items,
                ),
            )
            .await
            .map_err(|_| SkillProviderError::DeadlineExceeded)?
            .map_err(map_upstream_skills_error)?;
            let skills = exposed
                .skills
                .into_iter()
                .map(|skill| SkillProviderEntry::from_validated(self.id.clone(), skill))
                .collect();
            let result = SkillDiscoverResult {
                skills,
                source: exposed.source,
                cache_age: (exposed.source == SkillDiscoverySource::Cached)
                    .then(|| Duration::from_secs(exposed.age_secs)),
                ttl: exposed.ttl_ms.map(Duration::from_millis),
                excluded_count: exposed.excluded_count,
                truncated: exposed.truncated,
            };
            result.validate_for(&self.id, request)?;
            tracing::debug!(
                surface = "dispatch",
                service = "skills",
                action = "discover",
                upstream = %self.config.name,
                requested_items = request.max_items,
                returned_items = result.skills.len(),
                excluded_count = result.excluded_count,
                truncated = result.truncated,
                source = ?result.source,
                elapsed_ms = started.elapsed().as_millis(),
                "completed bounded Skill discovery"
            );
            Ok(result)
        })
    }

    fn get<'a>(&'a self, request: &'a SkillGetRequest) -> SkillProviderFuture<'a, SkillGetResult> {
        Box::pin(async move { self.get_with_catalog(request).await.map(|(skill, _)| skill) })
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
