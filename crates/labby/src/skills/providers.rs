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
struct SnapshotSkill {
    validated: ValidatedSkill,
    files: BTreeMap<String, Vec<u8>>,
}

/// Immutable provider whose descriptor and bytes come from one verified snapshot.
#[derive(Debug, Clone)]
pub(crate) struct SnapshotSkillProvider {
    id: SkillProviderId,
    skills: BTreeMap<String, SnapshotSkill>,
}

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
        let skill = self
            .skills
            .get(&id.source_id)
            .ok_or(SkillProviderError::SkillNotFound)?;
        if !skill.validated_descriptor(&self.id).is_available() {
            return Err(SkillProviderError::SkillNotFound);
        }
        Ok(skill)
    }
}

impl SnapshotSkill {
    fn validated_descriptor(&self, provider: &SkillProviderId) -> SkillProviderEntry {
        SkillProviderEntry::from_validated(provider.clone(), self.validated.clone())
    }
}

/// One immutable first-party registry snapshot. Bundled Skills win any name,
/// manifest URI, or resource URI collision with operator-local content.
#[derive(Debug, Clone)]
pub(crate) struct FirstPartySkillProviders {
    bundled: SnapshotSkillProvider,
    operator_local: SnapshotSkillProvider,
}

impl FirstPartySkillProviders {
    pub(crate) fn load() -> Self {
        Self {
            bundled: SnapshotSkillProvider::bundled(),
            operator_local: SnapshotSkillProvider::operator_local(),
        }
    }

    pub(crate) async fn discover(&self) -> Vec<SkillProviderEntry> {
        let bundled = self
            .bundled
            .discover(&SkillDiscoverRequest::default())
            .await
            .map_or_else(|_| Vec::new(), |result| result.skills);
        let local = self
            .operator_local
            .discover(&SkillDiscoverRequest::default())
            .await
            .map_or_else(|_| Vec::new(), |result| result.skills);
        merge_bundled_first(bundled, local)
    }

    pub(crate) async fn find(&self, uri: &str) -> Option<SkillProviderEntry> {
        self.discover().await.into_iter().find(|entry| {
            entry.descriptor.id.source_id == uri
                || entry.resources().any(|resource| resource.source_id == uri)
        })
    }

    pub(crate) async fn read(
        &self,
        entry: &SkillProviderEntry,
        resource_id: &str,
        max_bytes: usize,
    ) -> Result<SkillResourceReadResult, SkillProviderError> {
        let provider = if entry.descriptor.id.provider == *self.bundled.id() {
            &self.bundled
        } else if entry.descriptor.id.provider == *self.operator_local.id() {
            &self.operator_local
        } else {
            return Err(SkillProviderError::WrongProvider);
        };
        provider
            .read_resource(&SkillResourceReadRequest {
                skill_id: entry.descriptor.id.clone(),
                resource_id: resource_id.to_string(),
                max_bytes,
                deadline: Default::default(),
            })
            .await
    }
}

fn merge_bundled_first(
    bundled: Vec<SkillProviderEntry>,
    local: Vec<SkillProviderEntry>,
) -> Vec<SkillProviderEntry> {
    let mut names = std::collections::BTreeSet::new();
    let mut uris = std::collections::BTreeSet::new();
    let mut merged = Vec::with_capacity(bundled.len() + local.len());
    for entry in bundled.into_iter().chain(local) {
        if !entry.is_available() {
            continue;
        }
        let collides = names.contains(&entry.descriptor.name)
            || uris.contains(&entry.descriptor.id.source_id)
            || entry
                .resources()
                .any(|resource| uris.contains(&resource.source_id));
        if collides {
            tracing::warn!(
                skill = %entry.descriptor.name,
                provider = ?entry.descriptor.id.provider,
                "excluding a first-party skill that collides with bundled-first URI ownership"
            );
            continue;
        }
        names.insert(entry.descriptor.name.clone());
        uris.insert(entry.descriptor.id.source_id.clone());
        uris.extend(entry.resources().map(|resource| resource.source_id.clone()));
        merged.push(entry);
    }
    merged
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
            let available = self
                .skills
                .values()
                .filter(|skill| skill.validated_descriptor(&self.id).is_available())
                .count();
            let skills = self
                .skills
                .values()
                .map(|skill| {
                    SkillProviderEntry::from_validated(self.id.clone(), skill.validated.clone())
                })
                .filter(SkillProviderEntry::is_available)
                .take(request.max_items)
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
                skill: SkillProviderEntry::from_validated(self.id.clone(), skill.validated.clone()),
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
    use labby_runtime::skills::{
        SkillAvailabilitySummary, SkillCompatibilityClassification, SkillCompatibilityItem,
        SkillProviderDeadline, SkillResourceReadRequest,
    };

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

    #[tokio::test]
    async fn bundled_entries_win_name_and_uri_collisions() {
        let provider = SnapshotSkillProvider::bundled();
        let bundled = provider
            .discover(&SkillDiscoverRequest::default())
            .await
            .unwrap()
            .skills
            .into_iter()
            .next()
            .unwrap();
        let mut same_name_entry = bundled.validated().entry.clone();
        same_name_entry.uri = format!(
            "skill://labby/operator/{}/SKILL.md",
            bundled.descriptor.name
        );
        let bundled_prefix = format!("skill://labby/{}/", bundled.descriptor.name);
        let operator_prefix = format!("skill://labby/operator/{}/", bundled.descriptor.name);
        for resource in same_name_entry.resources.as_mut().unwrap() {
            resource.uri = resource.uri.replace(&bundled_prefix, &operator_prefix);
        }
        let same_name = SkillProviderEntry::from_validated(
            SkillProviderId::new(SkillProviderKind::OperatorLocal, "labby-home"),
            validate_skill_entry(&same_name_entry).unwrap(),
        );
        let same_uri = SkillProviderEntry::from_validated(
            SkillProviderId::new(SkillProviderKind::OperatorLocal, "labby-home"),
            bundled.validated().clone(),
        );

        let merged = merge_bundled_first(vec![bundled.clone()], vec![same_name, same_uri]);
        assert_eq!(merged, vec![bundled]);
    }

    #[tokio::test]
    async fn blocked_entries_are_not_offered() {
        let provider = SnapshotSkillProvider::bundled();
        let mut blocked = provider
            .discover(&SkillDiscoverRequest::default())
            .await
            .unwrap()
            .skills
            .into_iter()
            .next()
            .unwrap();
        blocked.descriptor.availability =
            SkillAvailabilitySummary::from_items([SkillCompatibilityItem::new(
                "runtime",
                SkillCompatibilityClassification::DependencyUnavailable,
            )]);

        assert!(merge_bundled_first(Vec::new(), vec![blocked]).is_empty());
    }
}
