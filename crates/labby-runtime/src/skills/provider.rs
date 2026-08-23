//! Transport-neutral, bounded Agent Skill provider contracts.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use thiserror::Error;

use super::limits::{MAX_SKILL_RESOURCE_BYTES, MAX_SKILLS_PER_UPSTREAM};
use super::{SkillDescriptor, SkillId, SkillProviderId, ValidatedSkill};

/// One provider-native resource bound to a Skill manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillProviderResource {
    /// Opaque provider-native identity (a URI for SEP-2640).
    pub source_id: String,
    /// Canonical content digest published by the provider.
    pub digest: String,
    pub media_type: Option<String>,
}

/// Compact descriptor plus the metadata required to project or activate it.
///
/// Resource bodies remain absent and are fetched only through
/// [`SkillProvider::read_resource`].
#[derive(Debug, Clone, PartialEq)]
pub struct SkillProviderEntry {
    pub descriptor: SkillDescriptor,
    /// Exact manifest accepted at the provider's validation boundary.
    ///
    /// This avoids reconstructing a wire entry from the compact projection,
    /// which would lose distinctions such as an absent versus empty manifest.
    validated: ValidatedSkill,
}

impl SkillProviderEntry {
    #[must_use]
    pub fn from_validated(provider: SkillProviderId, skill: ValidatedSkill) -> Self {
        Self {
            descriptor: SkillDescriptor::from_validated_entry(provider, &skill),
            validated: skill,
        }
    }

    /// Iterate the validated resource manifest without retaining a second copy.
    pub fn resources(&self) -> impl ExactSizeIterator<Item = SkillProviderResource> + '_ {
        self.validated
            .entry
            .resources
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|resource| SkillProviderResource {
                source_id: resource.uri.clone(),
                digest: resource.digest.clone(),
                media_type: None,
            })
    }

    /// Borrow the exact manifest already accepted by the provider.
    #[must_use]
    pub const fn validated(&self) -> &ValidatedSkill {
        &self.validated
    }

    /// Consume this projection and recover its exact validated manifest.
    #[must_use]
    pub fn into_validated(self) -> ValidatedSkill {
        self.validated
    }

    /// Whether this entry may be offered by discovery or exact lookup.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.descriptor.availability.available
    }
}

/// Caller-selected bounds for one provider operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillProviderDeadline {
    /// Maximum wall-clock time the provider may spend on the operation.
    pub timeout: Duration,
}

impl SkillProviderDeadline {
    /// Construct a non-zero operation deadline.
    pub fn new(timeout: Duration) -> Result<Self, SkillProviderError> {
        if timeout.is_zero() {
            return Err(SkillProviderError::InvalidRequest {
                field: "timeout",
                reason: "must_be_non_zero",
            });
        }
        Ok(Self { timeout })
    }
}

impl Default for SkillProviderDeadline {
    fn default() -> Self {
        Self {
            // A provider default is intentionally unbounded at this neutral
            // seam. Transport adapters clamp it to their configured operation
            // timeout; callers can still supply a smaller explicit budget.
            timeout: Duration::MAX,
        }
    }
}

/// Bounded request to discover compact Skill descriptors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDiscoverRequest {
    /// Maximum descriptors returned by this call.
    pub max_items: usize,
    /// Wall-clock budget for the complete traversal.
    pub deadline: SkillProviderDeadline,
}

impl Default for SkillDiscoverRequest {
    fn default() -> Self {
        Self {
            max_items: MAX_SKILLS_PER_UPSTREAM,
            deadline: SkillProviderDeadline::default(),
        }
    }
}

impl SkillDiscoverRequest {
    /// Validate that all bounds are non-zero and within Labby's safety caps.
    pub fn validate(&self) -> Result<(), SkillProviderError> {
        validate_bound("max_items", self.max_items, MAX_SKILLS_PER_UPSTREAM)?;
        SkillProviderDeadline::new(self.deadline.timeout)?;
        Ok(())
    }
}

/// Result of bounded descriptor discovery.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillDiscoverResult {
    pub skills: Vec<SkillProviderEntry>,
    /// Whether descriptors came from a provider refresh or an existing cache.
    pub source: SkillDiscoverySource,
    /// Age of the cached snapshot. `None` for refreshed results or when the
    /// provider cannot determine an age.
    pub cache_age: Option<Duration>,
    /// Remaining cache lifetime advertised by the provider, when meaningful.
    pub ttl: Option<Duration>,
    /// Candidates excluded during validation/integrity processing. Exposure
    /// filtering remains a separate policy decision.
    pub excluded_count: usize,
    /// True when a provider stopped because any request bound was reached.
    pub truncated: bool,
}

/// Provider-neutral provenance for a discovery result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SkillDiscoverySource {
    #[default]
    Refreshed,
    Cached,
}

/// Request for one current compact descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillGetRequest {
    pub id: SkillId,
    pub deadline: SkillProviderDeadline,
}

impl SkillGetRequest {
    /// Validate provider-independent lookup fields.
    pub fn validate(&self) -> Result<(), SkillProviderError> {
        validate_skill_id(&self.id)?;
        SkillProviderDeadline::new(self.deadline.timeout)?;
        Ok(())
    }
}

/// Result of an exact descriptor lookup.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillGetResult {
    pub skill: SkillProviderEntry,
}

/// Request to read one manifest-listed Skill resource within a byte budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillResourceReadRequest {
    pub skill_id: SkillId,
    /// Opaque provider-native resource identity (a URI for SEP-2640).
    pub resource_id: String,
    /// Maximum bytes the provider may return. Providers must reject oversized
    /// content rather than truncate it.
    pub max_bytes: usize,
    pub deadline: SkillProviderDeadline,
}

impl SkillResourceReadRequest {
    /// Validate provider-independent read bounds and identity fields.
    pub fn validate(&self) -> Result<(), SkillProviderError> {
        validate_skill_id(&self.skill_id)?;
        if self.resource_id.is_empty() {
            return Err(SkillProviderError::InvalidRequest {
                field: "resource_id",
                reason: "must_not_be_empty",
            });
        }
        if self.max_bytes == 0 {
            return Err(SkillProviderError::InvalidRequest {
                field: "max_bytes",
                reason: "must_be_non_zero",
            });
        }
        if self.max_bytes > MAX_SKILL_RESOURCE_BYTES {
            return Err(SkillProviderError::LimitExceeded {
                what: "resource_bytes",
                limit: MAX_SKILL_RESOURCE_BYTES,
            });
        }
        SkillProviderDeadline::new(self.deadline.timeout)?;
        Ok(())
    }
}

/// Exact bytes returned by a bounded resource read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillResourceReadResult {
    pub skill_id: SkillId,
    pub resource_id: String,
    pub bytes: Vec<u8>,
    pub media_type: Option<String>,
}

impl SkillResourceReadResult {
    /// Verify that a provider answered the exact bounded read requested.
    pub fn validate_for(
        &self,
        request: &SkillResourceReadRequest,
    ) -> Result<(), SkillProviderError> {
        request.validate()?;
        if self.skill_id != request.skill_id || self.resource_id != request.resource_id {
            return Err(SkillProviderError::Integrity {
                reason: "resource_identity_mismatch",
            });
        }
        if self.bytes.len() > request.max_bytes {
            return Err(SkillProviderError::LimitExceeded {
                what: "resource_bytes",
                limit: request.max_bytes,
            });
        }
        Ok(())
    }
}

/// Provider-independent failures. Transport adapters retain their detailed
/// cause internally and classify it at this boundary.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SkillProviderError {
    #[error("invalid provider request field `{field}`: {reason}")]
    InvalidRequest {
        field: &'static str,
        reason: &'static str,
    },
    #[error("skill provider identity does not match the requested skill")]
    WrongProvider,
    #[error("skill was not found")]
    SkillNotFound,
    #[error("skill resource was not found or is not present in the manifest")]
    ResourceNotFound,
    #[error("skill resource manifest is stale or ambiguous")]
    ManifestStale,
    #[error("skill provider operation exceeded its deadline")]
    DeadlineExceeded,
    #[error("skill provider exceeded the `{what}` limit of {limit}")]
    LimitExceeded { what: &'static str, limit: usize },
    #[error("skill provider returned content that failed integrity validation: {reason}")]
    Integrity { reason: &'static str },
    #[error("skill provider is unavailable: {reason}")]
    Unavailable { reason: String },
    #[error("skill provider failed: {reason}")]
    Provider { reason: String },
}

fn validate_bound(
    field: &'static str,
    value: usize,
    ceiling: usize,
) -> Result<(), SkillProviderError> {
    if value == 0 {
        return Err(SkillProviderError::InvalidRequest {
            field,
            reason: "must_be_non_zero",
        });
    }
    if value > ceiling {
        return Err(SkillProviderError::LimitExceeded {
            what: field,
            limit: ceiling,
        });
    }
    Ok(())
}

fn validate_skill_id(id: &SkillId) -> Result<(), SkillProviderError> {
    if id.provider.instance.is_empty() {
        return Err(SkillProviderError::InvalidRequest {
            field: "provider.instance",
            reason: "must_not_be_empty",
        });
    }
    if id.source_id.is_empty() {
        return Err(SkillProviderError::InvalidRequest {
            field: "skill.source_id",
            reason: "must_not_be_empty",
        });
    }
    Ok(())
}

/// Boxed provider future, avoiding an `async_trait` dependency.
pub type SkillProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, SkillProviderError>> + Send + 'a>>;

/// Provider seam for progressively discovering and reading Agent Skills.
///
/// Implementations must enforce request bounds themselves. A provider only
/// reads its source; persistence and activation remain separate concerns.
pub trait SkillProvider: Send + Sync {
    /// Host-assigned identity of this provider instance.
    fn id(&self) -> &SkillProviderId;

    /// Discover compact metadata without fetching resource bodies.
    fn discover<'a>(
        &'a self,
        request: &'a SkillDiscoverRequest,
    ) -> SkillProviderFuture<'a, SkillDiscoverResult>;

    /// Refresh and return one compact descriptor.
    fn get<'a>(&'a self, request: &'a SkillGetRequest) -> SkillProviderFuture<'a, SkillGetResult>;

    /// Read one manifest-listed resource, rejecting content over `max_bytes`.
    fn read_resource<'a>(
        &'a self,
        request: &'a SkillResourceReadRequest,
    ) -> SkillProviderFuture<'a, SkillResourceReadResult>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{
        ResourceDigest, SkillEntry, SkillProviderKind, SkillResource, validate_skill_entry,
    };
    use serde_json::json;

    #[test]
    fn discovery_defaults_are_hard_bounded() {
        let request = SkillDiscoverRequest::default();
        request.validate().unwrap();
        assert_eq!(request.max_items, MAX_SKILLS_PER_UPSTREAM);

        let mut invalid = request;
        invalid.max_items = MAX_SKILLS_PER_UPSTREAM + 1;
        assert!(matches!(
            invalid.validate(),
            Err(SkillProviderError::LimitExceeded {
                what: "max_items",
                ..
            })
        ));
    }

    #[test]
    fn resource_reads_require_explicit_non_zero_bounds() {
        let provider = SkillProviderId::new(SkillProviderKind::Bundled, "built-in");
        let request = SkillResourceReadRequest {
            skill_id: SkillId::new(provider, "review"),
            resource_id: "SKILL.md".to_string(),
            max_bytes: 0,
            deadline: SkillProviderDeadline::default(),
        };
        assert!(matches!(
            request.validate(),
            Err(SkillProviderError::InvalidRequest {
                field: "max_bytes",
                ..
            })
        ));
    }

    #[test]
    fn read_result_must_match_identity_and_byte_bound() {
        let provider = SkillProviderId::new(SkillProviderKind::Bundled, "built-in");
        let request = SkillResourceReadRequest {
            skill_id: SkillId::new(provider.clone(), "review"),
            resource_id: "SKILL.md".to_string(),
            max_bytes: 4,
            deadline: SkillProviderDeadline::default(),
        };
        let oversized = SkillResourceReadResult {
            skill_id: SkillId::new(provider, "review"),
            resource_id: "SKILL.md".to_string(),
            bytes: b"12345".to_vec(),
            media_type: None,
        };
        assert!(matches!(
            oversized.validate_for(&request),
            Err(SkillProviderError::LimitExceeded {
                what: "resource_bytes",
                limit: 4
            })
        ));
    }

    #[test]
    fn provider_contract_is_object_safe() {
        fn accepts_provider(_: &dyn SkillProvider) {}
        let _ = accepts_provider;
    }

    #[test]
    fn projection_recovers_the_exact_validated_manifest() {
        let uri = "skill://catalog/review/SKILL.md";
        let entry = SkillEntry {
            uri: uri.to_string(),
            frontmatter: json!({"name": "review", "description": "demo"})
                .as_object()
                .expect("object")
                .clone(),
            resources: Some(vec![SkillResource {
                uri: uri.to_string(),
                digest: ResourceDigest::of_bytes(b"manifest").to_wire(),
            }]),
            // `Some(empty)` is intentionally distinct from absent provider
            // metadata and used to be collapsed by descriptor reconstruction.
            meta: Some(serde_json::Map::new()),
        };
        let validated = validate_skill_entry(&entry).expect("validated skill");
        let projection = SkillProviderEntry::from_validated(
            SkillProviderId::new(SkillProviderKind::McpUpstream, "catalog"),
            validated.clone(),
        );

        assert_eq!(projection.validated(), &validated);
        assert_eq!(projection.into_validated(), validated);
    }
}
