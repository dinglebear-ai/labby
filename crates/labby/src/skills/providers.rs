//! Snapshot-backed bundled and operator-local Skill providers.

use std::collections::BTreeMap;
use std::time::Instant;

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
            let validated = match validate_skill_entry(&skill.entry) {
                Ok(validated) => validated,
                Err(reason) => {
                    tracing::error!(skill = %name, reason = %reason.as_str(), "validated bundled registry produced an invalid provider entry");
                    continue;
                }
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
                let validated = match validate_skill_entry(&skill.entry) {
                    Ok(validated) => validated,
                    Err(reason) => {
                        tracing::warn!(skill = %skill.entry.uri, reason = %reason.as_str(), "operator skill became invalid while constructing provider snapshot");
                        return None;
                    }
                };
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
        if id.provider() != &self.id {
            return Err(SkillProviderError::WrongProvider);
        }
        let skill = self
            .skills
            .get(id.source_id())
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
    merged: Vec<SkillProviderEntry>,
    uri_index: BTreeMap<String, usize>,
}

impl FirstPartySkillProviders {
    pub(crate) fn load() -> Self {
        Self::from_providers(
            SnapshotSkillProvider::bundled(),
            SnapshotSkillProvider::operator_local(),
        )
    }

    fn from_providers(
        bundled: SnapshotSkillProvider,
        operator_local: SnapshotSkillProvider,
    ) -> Self {
        let merged = merge_bundled_first(bundled.entries(), operator_local.entries());
        let mut uri_index = BTreeMap::new();
        for (index, entry) in merged.iter().enumerate() {
            uri_index.insert(entry.descriptor().id.source_id().to_string(), index);
            for resource in entry.resources() {
                uri_index.insert(resource.source_id, index);
            }
        }
        Self {
            bundled,
            operator_local,
            merged,
            uri_index,
        }
    }

    pub(crate) fn discover(&self) -> &[SkillProviderEntry] {
        &self.merged
    }

    pub(crate) fn find(&self, uri: &str) -> Option<&SkillProviderEntry> {
        self.uri_index.get(uri).map(|index| &self.merged[*index])
    }

    pub(crate) async fn read(
        &self,
        entry: &SkillProviderEntry,
        resource_id: &str,
        max_bytes: usize,
    ) -> Result<SkillResourceReadResult, SkillProviderError> {
        let provider = if entry.descriptor().id.provider() == self.bundled.id() {
            &self.bundled
        } else if entry.descriptor().id.provider() == self.operator_local.id() {
            &self.operator_local
        } else {
            return Err(SkillProviderError::WrongProvider);
        };
        provider
            .read_resource(&SkillResourceReadRequest {
                skill_id: entry.descriptor().id.clone(),
                resource_id: resource_id.to_string(),
                max_bytes,
                deadline: Default::default(),
            })
            .await
    }
}

impl SnapshotSkillProvider {
    fn entries(&self) -> Vec<SkillProviderEntry> {
        self.skills
            .values()
            .map(|skill| skill.validated_descriptor(&self.id))
            .filter(SkillProviderEntry::is_available)
            .collect()
    }
}

fn ensure_deadline(
    started: Instant,
    timeout: std::time::Duration,
) -> Result<(), SkillProviderError> {
    if started.elapsed() >= timeout {
        Err(SkillProviderError::DeadlineExceeded)
    } else {
        Ok(())
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
        let collides = names.contains(&entry.descriptor().name)
            || uris.contains(entry.descriptor().id.source_id())
            || entry
                .resources()
                .any(|resource| uris.contains(&resource.source_id));
        if collides {
            tracing::warn!(
                skill = %entry.descriptor().name,
                provider = ?entry.descriptor().id.provider(),
                "excluding a first-party skill that collides with bundled-first URI ownership"
            );
            continue;
        }
        names.insert(entry.descriptor().name.clone());
        uris.insert(entry.descriptor().id.source_id().to_string());
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
            let started = Instant::now();
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
            let result = SkillDiscoverResult {
                skills,
                source: SkillDiscoverySource::Cached,
                cache_age: None,
                ttl: None,
                excluded_count: 0,
                truncated: available > request.max_items,
            };
            ensure_deadline(started, request.deadline.timeout)?;
            result.validate_for(&self.id, request)?;
            Ok(result)
        })
    }

    fn get<'a>(&'a self, request: &'a SkillGetRequest) -> SkillProviderFuture<'a, SkillGetResult> {
        Box::pin(async move {
            let started = Instant::now();
            request.validate()?;
            let skill = self.requested_skill(&request.id)?;
            let result = SkillGetResult {
                skill: SkillProviderEntry::from_validated(self.id.clone(), skill.validated.clone()),
            };
            ensure_deadline(started, request.deadline.timeout)?;
            result.validate_for(&self.id, request)?;
            Ok(result)
        })
    }

    fn read_resource<'a>(
        &'a self,
        request: &'a SkillResourceReadRequest,
    ) -> SkillProviderFuture<'a, SkillResourceReadResult> {
        Box::pin(async move {
            let started = Instant::now();
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
            ensure_deadline(started, request.deadline.timeout)?;
            result.validate_for(request)?;
            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_runtime::skills::wire::{SkillEntry, SkillResource};
    use labby_runtime::skills::{
        SkillAvailabilitySummary, SkillCompatibilityClassification, SkillCompatibilityItem,
        SkillProviderDeadline, SkillResourceReadRequest,
    };

    fn provider_with(name: &str, extra_uri: Option<&str>) -> SnapshotSkillProvider {
        let manifest = format!("skill://test/{name}/SKILL.md");
        let body = format!("---\nname: {name}\ndescription: test\n---\n");
        let mut resources = vec![SkillResource {
            uri: manifest.clone(),
            digest: labby_runtime::skills::ResourceDigest::of_bytes(body.as_bytes()).to_wire(),
        }];
        let mut files = BTreeMap::from([(manifest.clone(), body.into_bytes())]);
        if let Some(uri) = extra_uri {
            resources.push(SkillResource {
                uri: uri.to_string(),
                digest: labby_runtime::skills::ResourceDigest::of_bytes(b"extra").to_wire(),
            });
            files.insert(uri.to_string(), b"extra".to_vec());
        }
        let entry = SkillEntry {
            uri: manifest.clone(),
            frontmatter: labby_runtime::skills::parse_skill_md_frontmatter(&format!(
                "---\nname: {name}\ndescription: test\n---\n"
            ))
            .unwrap(),
            resources: Some(resources),
            meta: None,
        };
        let validated = validate_skill_entry(&entry).unwrap();
        SnapshotSkillProvider {
            id: SkillProviderId::new(SkillProviderKind::Bundled, "test"),
            skills: BTreeMap::from([(manifest, SnapshotSkill { validated, files })]),
        }
    }

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
            .find(|skill| skill.descriptor().name == "using-labby")
            .unwrap();
        assert!(
            !descriptor
                .descriptor()
                .provider_metadata
                .contains_key("instructions")
        );
        let request = SkillResourceReadRequest {
            skill_id: descriptor.descriptor().id.clone(),
            resource_id: descriptor.descriptor().id.source_id().to_string(),
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

    #[tokio::test]
    async fn exact_operations_reject_wrong_provider_missing_skill_and_unmanifested_resource() {
        let provider = provider_with("one", None);
        let entry = provider.entries().pop().unwrap();
        let wrong = labby_runtime::skills::SkillId::new(
            SkillProviderId::new(SkillProviderKind::Bundled, "other"),
            entry.descriptor().id.source_id().to_string(),
        );
        assert_eq!(
            provider
                .get(&SkillGetRequest {
                    id: wrong,
                    deadline: Default::default()
                })
                .await
                .unwrap_err(),
            SkillProviderError::WrongProvider
        );
        let missing = labby_runtime::skills::SkillId::new(
            provider.id().clone(),
            "skill://test/missing/SKILL.md",
        );
        assert_eq!(
            provider
                .get(&SkillGetRequest {
                    id: missing,
                    deadline: Default::default()
                })
                .await
                .unwrap_err(),
            SkillProviderError::SkillNotFound
        );
        let read = SkillResourceReadRequest {
            skill_id: entry.descriptor().id.clone(),
            resource_id: "skill://test/one/not-manifested.md".to_string(),
            max_bytes: 64,
            deadline: Default::default(),
        };
        assert_eq!(
            provider.read_resource(&read).await.unwrap_err(),
            SkillProviderError::ResourceNotFound
        );
    }

    #[tokio::test]
    async fn discovery_truncates_at_max_items_and_reads_reject_small_byte_budget() {
        let mut provider = provider_with("one", None);
        let second = provider_with("two", None);
        provider.skills.extend(second.skills);
        let request = SkillDiscoverRequest {
            max_items: 1,
            ..SkillDiscoverRequest::default()
        };
        let result = provider.discover(&request).await.unwrap();
        assert_eq!(result.skills.len(), 1);
        assert!(result.truncated);
        let entry = &result.skills[0];
        let read = SkillResourceReadRequest {
            skill_id: entry.descriptor().id.clone(),
            resource_id: entry.descriptor().id.source_id().to_string(),
            max_bytes: 1,
            deadline: Default::default(),
        };
        assert!(matches!(
            provider.read_resource(&read).await,
            Err(SkillProviderError::LimitExceeded {
                what: "resource_bytes",
                limit: 1
            })
        ));
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
            bundled.descriptor().name
        );
        let bundled_prefix = format!("skill://labby/{}/", bundled.descriptor().name);
        let operator_prefix = format!("skill://labby/operator/{}/", bundled.descriptor().name);
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

    #[test]
    fn auxiliary_resource_only_collision_excludes_the_later_owner() {
        let shared = "skill://test/first/second/notes.md";
        let first = provider_with("first", Some(shared))
            .entries()
            .pop()
            .unwrap();
        let mut second_wire = provider_with("second", Some("skill://test/second/notes.md"))
            .entries()
            .pop()
            .unwrap()
            .into_validated()
            .entry;
        second_wire.uri = "skill://test/first/second/SKILL.md".to_string();
        for resource in second_wire.resources.as_mut().unwrap() {
            resource.uri = resource
                .uri
                .replace("skill://test/second/", "skill://test/first/second/");
        }
        let second = SkillProviderEntry::from_validated(
            SkillProviderId::new(SkillProviderKind::OperatorLocal, "local"),
            validate_skill_entry(&second_wire).unwrap(),
        );
        assert_eq!(
            merge_bundled_first(vec![first.clone()], vec![second]),
            vec![first]
        );
    }

    #[test]
    fn first_party_projection_is_immutable_after_startup_snapshot() {
        let bundled = provider_with("first", None);
        let local = SnapshotSkillProvider {
            id: SkillProviderId::new(SkillProviderKind::OperatorLocal, "local"),
            skills: BTreeMap::new(),
        };
        let mut snapshot = FirstPartySkillProviders::from_providers(bundled, local);
        let before = snapshot.discover().to_vec();
        snapshot
            .operator_local
            .skills
            .extend(provider_with("late", None).skills);
        assert_eq!(snapshot.discover(), before);
        assert!(snapshot.find("skill://test/late/SKILL.md").is_none());
    }

    #[tokio::test]
    async fn blocked_entries_are_not_offered() {
        let provider = SnapshotSkillProvider::bundled();
        let blocked = provider
            .discover(&SkillDiscoverRequest::default())
            .await
            .unwrap()
            .skills
            .into_iter()
            .next()
            .unwrap();
        let blocked = blocked.with_availability(SkillAvailabilitySummary::from_items([
            SkillCompatibilityItem::new(
                "runtime",
                SkillCompatibilityClassification::DependencyUnavailable,
            ),
        ]));

        assert!(merge_bundled_first(Vec::new(), vec![blocked]).is_empty());
    }
}
