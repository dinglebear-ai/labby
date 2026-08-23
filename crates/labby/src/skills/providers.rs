//! Snapshot-backed bundled and operator-local Skill providers.

use std::collections::BTreeMap;

use labby_runtime::skills::{
    SkillDiscoverRequest, SkillDiscoverResult, SkillDiscoverySource, SkillGetRequest,
    SkillGetResult, SkillProvider, SkillProviderEntry, SkillProviderError, SkillProviderFuture,
    SkillProviderId, SkillProviderKind, SkillResourceReadRequest, SkillResourceReadResult,
    ValidatedSkill, validate_skill_entry,
};

use super::{EMBEDDED_FILES, first_party_uri, local};

#[derive(Debug, Clone)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "wired into the facade in the next migration slice"
    )
)]
struct SnapshotSkill {
    validated: ValidatedSkill,
    files: BTreeMap<String, Vec<u8>>,
}

/// Immutable provider whose descriptor and bytes come from one verified snapshot.
#[derive(Debug, Clone)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "wired into the facade in the next migration slice"
    )
)]
pub(crate) struct SnapshotSkillProvider {
    id: SkillProviderId,
    skills: BTreeMap<String, SnapshotSkill>,
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "wired into the facade in the next migration slice"
    )
)]
impl SnapshotSkillProvider {
    pub(crate) fn bundled() -> Self {
        let mut grouped: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
        for (name, path, body) in EMBEDDED_FILES {
            grouped.entry(name).or_default().push((path, body));
        }
        let entries = super::build_first_party_skills();
        let mut skills = BTreeMap::new();
        for (name, skill) in entries {
            let Ok(validated) = validate_skill_entry(&skill.entry) else {
                tracing::error!(skill = %name, "validated bundled registry produced an invalid provider entry");
                continue;
            };
            let Some(group) = grouped.remove(name.as_str()) else {
                tracing::error!(skill = %name, "bundled provider entry has no embedded file group");
                continue;
            };
            let files = group
                .into_iter()
                .map(|(path, body)| (first_party_uri(&name, path), body.as_bytes().to_vec()))
                .collect();
            skills.insert(skill.entry.uri.clone(), SnapshotSkill { validated, files });
        }
        for name in grouped.keys() {
            tracing::error!(skill = %name, "embedded file group produced no bundled provider entry");
        }
        Self {
            id: SkillProviderId::new(SkillProviderKind::Bundled, "labby"),
            skills,
        }
    }

    #[cfg_attr(
        test,
        expect(dead_code, reason = "ambient LABBY_HOME is not read by unit tests")
    )]
    pub(crate) fn operator_local() -> Self {
        let skills = local::load_local_skills()
            .into_values()
            .filter_map(|skill| {
                let validated = validate_skill_entry(&skill.entry).ok()?;
                let source_id = skill.entry.uri.clone();
                let files = skill
                    .files
                    .into_iter()
                    .map(|(uri, body)| (uri, body.into_bytes()))
                    .collect();
                Some((source_id, SnapshotSkill { validated, files }))
            })
            .collect();
        Self {
            id: SkillProviderId::new(SkillProviderKind::OperatorLocal, "labby-home"),
            skills,
        }
    }

    fn requested_skill(
        &self,
        id: &labby_runtime::skills::SkillId,
    ) -> Result<&SnapshotSkill, SkillProviderError> {
        if id.provider != self.id {
            return Err(SkillProviderError::WrongProvider);
        }
        self.skills
            .get(&id.source_id)
            .ok_or(SkillProviderError::SkillNotFound)
    }
}

impl SkillProvider for SnapshotSkillProvider {
    fn id(&self) -> &SkillProviderId {
        &self.id
    }

    fn discover<'a>(
        &'a self,
        request: &'a SkillDiscoverRequest,
    ) -> SkillProviderFuture<'a, SkillDiscoverResult> {
        Box::pin(async move {
            request.validate()?;
            let available = self.skills.len();
            let skills = self
                .skills
                .values()
                .take(request.max_items)
                .map(|skill| SkillProviderEntry::from_validated(self.id.clone(), &skill.validated))
                .collect();
            Ok(SkillDiscoverResult {
                skills,
                source: SkillDiscoverySource::Cached,
                cache_age: None,
                ttl: None,
                excluded_count: 0,
                truncated: available > request.max_items,
            })
        })
    }

    fn get<'a>(&'a self, request: &'a SkillGetRequest) -> SkillProviderFuture<'a, SkillGetResult> {
        Box::pin(async move {
            request.validate()?;
            let skill = self.requested_skill(&request.id)?;
            Ok(SkillGetResult {
                skill: SkillProviderEntry::from_validated(self.id.clone(), &skill.validated),
            })
        })
    }

    fn read_resource<'a>(
        &'a self,
        request: &'a SkillResourceReadRequest,
    ) -> SkillProviderFuture<'a, SkillResourceReadResult> {
        Box::pin(async move {
            request.validate()?;
            let skill = self.requested_skill(&request.skill_id)?;
            let bytes = skill
                .files
                .get(&request.resource_id)
                .ok_or(SkillProviderError::ResourceNotFound)?;
            if bytes.len() > request.max_bytes {
                return Err(SkillProviderError::LimitExceeded {
                    what: "resource_bytes",
                    limit: request.max_bytes,
                });
            }
            let result = SkillResourceReadResult {
                skill_id: request.skill_id.clone(),
                resource_id: request.resource_id.clone(),
                bytes: bytes.clone(),
                media_type: None,
            };
            result.validate_for(request)?;
            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_runtime::skills::{SkillProviderDeadline, SkillResourceReadRequest};

    #[tokio::test]
    async fn bundled_provider_discovers_metadata_then_reads_manifest_bytes() {
        let provider = SnapshotSkillProvider::bundled();
        let listing = provider
            .discover(&SkillDiscoverRequest::default())
            .await
            .unwrap();
        let descriptor = listing
            .skills
            .iter()
            .find(|skill| skill.descriptor.name == "using-labby")
            .unwrap();
        assert!(
            !descriptor
                .descriptor
                .provider_metadata
                .contains_key("instructions")
        );
        let request = SkillResourceReadRequest {
            skill_id: descriptor.descriptor.id.clone(),
            resource_id: descriptor.descriptor.id.source_id.clone(),
            max_bytes: 1024 * 1024,
            deadline: SkillProviderDeadline::default(),
        };
        let read = provider.read_resource(&request).await.unwrap();
        assert!(
            std::str::from_utf8(&read.bytes)
                .unwrap()
                .contains("name: using-labby")
        );
    }

    #[test]
    fn bundled_and_operator_ids_keep_equal_source_ids_distinct() {
        let source = "skill://labby/shared/SKILL.md";
        let bundled = labby_runtime::skills::SkillId::new(
            SkillProviderId::new(SkillProviderKind::Bundled, "labby"),
            source,
        );
        let local = labby_runtime::skills::SkillId::new(
            SkillProviderId::new(SkillProviderKind::OperatorLocal, "labby-home"),
            source,
        );
        assert_ne!(bundled, local);
    }
}
