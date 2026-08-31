//! Snapshot-backed bundled and operator-local Skill providers.

use std::collections::BTreeMap;
use std::time::Instant;

use labby_runtime::artifacts::{LibraryOwnership, SkillVisibility};
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

    pub(crate) fn operator_local_from(
        local_skills: impl IntoIterator<Item = local::LocalSkill>,
    ) -> Self {
        let skills = local_skills
            .into_iter()
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
    collision_rejections: Vec<CollisionRejection>,
    artifact_access: BTreeMap<String, ArtifactSkillAccess>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArtifactSkillAccess {
    pub(crate) ownership: LibraryOwnership,
    pub(crate) visibility: SkillVisibility,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollisionRejection {
    pub(crate) skill: String,
    pub(crate) kind: CollisionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollisionKind {
    Name,
    ManifestUri,
    ResourceUri,
}

impl FirstPartySkillProviders {
    fn from_providers(
        bundled: SnapshotSkillProvider,
        operator_local: SnapshotSkillProvider,
    ) -> Self {
        let (merged, collision_rejections) =
            merge_bundled_first(bundled.entries(), operator_local.entries());
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
            collision_rejections,
            artifact_access: BTreeMap::new(),
        }
    }

    pub(crate) fn from_local_skills(
        local_skills: impl IntoIterator<Item = local::LocalSkill>,
    ) -> Self {
        Self::from_providers(
            SnapshotSkillProvider::bundled(),
            SnapshotSkillProvider::operator_local_from(local_skills),
        )
    }

    #[cfg(test)]
    pub(crate) fn from_artifact_skills(
        local_skills: impl IntoIterator<Item = (local::LocalSkill, ArtifactSkillAccess)>,
    ) -> Self {
        Self::from_local_skills([]).with_artifact_skills(local_skills)
    }

    /// Add active Artifact Skills after the immutable bundled and legacy operator-local snapshot.
    /// Existing Artifact entries are first removed, so repeated activation projections retain the
    /// same legacy snapshot without treating the previously active set as operator-owned input.
    pub(crate) fn with_artifact_skills(
        &self,
        local_skills: impl IntoIterator<Item = (local::LocalSkill, ArtifactSkillAccess)>,
    ) -> Self {
        let mut access_by_manifest = BTreeMap::new();
        let locals = local_skills
            .into_iter()
            .map(|(skill, access)| {
                access_by_manifest
                    .entry(skill.entry.uri.clone())
                    .or_insert(access);
                skill
            })
            .collect::<Vec<_>>();
        let artifact = SnapshotSkillProvider::operator_local_from(locals);
        let legacy = SnapshotSkillProvider {
            id: self.operator_local.id.clone(),
            skills: self
                .operator_local
                .skills
                .iter()
                .filter(|(manifest, _)| !self.artifact_access.contains_key(*manifest))
                .map(|(manifest, skill)| (manifest.clone(), skill.clone()))
                .collect(),
        };
        let (legacy_merged, mut collision_rejections) =
            merge_bundled_first(self.bundled.entries(), legacy.entries());
        let (merged, artifact_rejections) = merge_bundled_first(legacy_merged, artifact.entries());
        collision_rejections.extend(artifact_rejections);

        let accepted_artifacts = merged
            .iter()
            .map(|entry| entry.descriptor().id.source_id())
            .filter(|manifest| access_by_manifest.contains_key(*manifest))
            .map(str::to_owned)
            .collect::<std::collections::BTreeSet<_>>();
        let mut operator_skills = legacy.skills;
        operator_skills.extend(
            artifact
                .skills
                .into_iter()
                .filter(|(manifest, _)| accepted_artifacts.contains(manifest)),
        );
        let operator_local = SnapshotSkillProvider {
            id: legacy.id,
            skills: operator_skills,
        };
        let mut uri_index = BTreeMap::new();
        for (index, entry) in merged.iter().enumerate() {
            uri_index.insert(entry.descriptor().id.source_id().to_string(), index);
            for resource in entry.resources() {
                uri_index.insert(resource.source_id, index);
            }
        }
        let mut providers = Self {
            bundled: self.bundled.clone(),
            operator_local,
            merged,
            uri_index,
            collision_rejections,
            artifact_access: BTreeMap::new(),
        };
        for entry in &providers.merged {
            let manifest = entry.descriptor().id.source_id();
            if let Some(access) = access_by_manifest.get(manifest) {
                providers
                    .artifact_access
                    .insert(manifest.to_owned(), access.clone());
                for resource in entry.resources() {
                    providers
                        .artifact_access
                        .insert(resource.source_id.clone(), access.clone());
                }
            }
        }
        providers
    }

    pub(crate) fn artifact_access(&self, uri: &str) -> Option<&ArtifactSkillAccess> {
        self.artifact_access.get(uri)
    }

    pub(crate) fn has_artifact_skills(&self) -> bool {
        !self.artifact_access.is_empty()
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

    pub(crate) fn admission_totals(&self) -> (usize, usize, usize, usize) {
        let mut bytes = 0;
        let mut max_skill_bytes = 0;
        let mut resources = 0;
        for provider in [&self.bundled, &self.operator_local] {
            for skill in provider.skills.values() {
                let skill_bytes = skill.files.values().map(Vec::len).sum::<usize>();
                bytes += skill_bytes;
                max_skill_bytes = max_skill_bytes.max(skill_bytes);
                resources += skill.files.len();
            }
        }
        (self.merged.len(), bytes, max_skill_bytes, resources)
    }

    pub(crate) fn collision_rejections(&self) -> &[CollisionRejection] {
        &self.collision_rejections
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
) -> (Vec<SkillProviderEntry>, Vec<CollisionRejection>) {
    let mut names = std::collections::BTreeSet::new();
    let mut uris = std::collections::BTreeSet::new();
    let mut merged = Vec::with_capacity(bundled.len() + local.len());
    let mut rejections = Vec::new();
    for entry in bundled.into_iter().chain(local) {
        if !entry.is_available() {
            continue;
        }
        let collision = if names.contains(&entry.descriptor().name) {
            Some(CollisionKind::Name)
        } else if uris.contains(entry.descriptor().id.source_id()) {
            Some(CollisionKind::ManifestUri)
        } else if entry
            .resources()
            .any(|resource| uris.contains(&resource.source_id))
        {
            Some(CollisionKind::ResourceUri)
        } else {
            None
        };
        if let Some(kind) = collision {
            tracing::warn!(
                skill = %entry.descriptor().name,
                provider = ?entry.descriptor().id.provider(),
                "excluding a first-party skill that collides with bundled-first URI ownership"
            );
            rejections.push(CollisionRejection {
                skill: entry.descriptor().name.clone(),
                kind,
            });
            continue;
        }
        names.insert(entry.descriptor().name.clone());
        uris.insert(entry.descriptor().id.source_id().to_string());
        uris.extend(entry.resources().map(|resource| resource.source_id.clone()));
        merged.push(entry);
    }
    (merged, rejections)
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
    use labby_runtime::artifacts::{
        LibraryActorId, LibraryOwnership, LibraryTenantId, SkillVisibility,
    };
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
            size: body.len() as u64,
        }];
        let mut files = BTreeMap::from([(manifest.clone(), body.into_bytes())]);
        if let Some(uri) = extra_uri {
            resources.push(SkillResource {
                uri: uri.to_string(),
                digest: labby_runtime::skills::ResourceDigest::of_bytes(b"extra").to_wire(),
                size: b"extra".len() as u64,
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

    fn local_skill(name: &str, namespace: &str, marker: &str) -> local::LocalSkill {
        let manifest = format!("skill://labby/{namespace}/{name}/SKILL.md");
        let body = format!("---\nname: {name}\ndescription: {marker}\n---\n{marker}\n");
        local::LocalSkill {
            entry: SkillEntry {
                uri: manifest.clone(),
                frontmatter: labby_runtime::skills::parse_skill_md_frontmatter(&body).unwrap(),
                resources: Some(vec![SkillResource {
                    uri: manifest.clone(),
                    digest: labby_runtime::skills::ResourceDigest::of_bytes(body.as_bytes())
                        .to_wire(),
                    size: body.len() as u64,
                }]),
                meta: None,
            },
            files: BTreeMap::from([(manifest, body)]),
        }
    }

    fn artifact_access() -> ArtifactSkillAccess {
        ArtifactSkillAccess {
            ownership: LibraryOwnership::canonical(
                LibraryTenantId::from_canonical_projection("tenant-a").unwrap(),
                LibraryActorId::from_canonical_projection("owner-a").unwrap(),
            ),
            visibility: SkillVisibility::Tenant,
        }
    }

    #[test]
    fn bundled_then_legacy_then_artifact_precedence_preserves_every_winner() {
        let legacy = local_skill("legacy-only", "operator", "legacy bytes");
        let legacy_collision = local_skill("shared-name", "operator", "legacy winner");
        let base =
            FirstPartySkillProviders::from_local_skills([legacy.clone(), legacy_collision.clone()]);
        let artifact_only = local_skill("artifact-only", "artifact", "artifact bytes");
        let artifact_legacy_collision =
            local_skill("shared-name", "aaa-artifact", "artifact loser");
        let artifact_bundled_collision = local_skill("using-labby", "artifact", "bundled loser");

        let projected = base.with_artifact_skills([
            (artifact_only.clone(), artifact_access()),
            (artifact_legacy_collision, artifact_access()),
            (artifact_bundled_collision, artifact_access()),
        ]);

        assert!(projected.find(&legacy.entry.uri).is_some());
        assert!(projected.find(&legacy_collision.entry.uri).is_some());
        assert!(projected.find(&artifact_only.entry.uri).is_some());
        assert!(projected.artifact_access(&legacy.entry.uri).is_none());
        assert!(
            projected
                .artifact_access(&artifact_only.entry.uri)
                .is_some()
        );
        assert_eq!(
            projected
                .collision_rejections()
                .iter()
                .filter(|rejection| rejection.kind == CollisionKind::Name)
                .count(),
            2
        );
        assert_eq!(
            projected
                .find(&legacy_collision.entry.uri)
                .unwrap()
                .descriptor()
                .name,
            "shared-name"
        );

        let replacement = local_skill("artifact-next", "artifact", "next activation");
        let reprojected =
            projected.with_artifact_skills([(replacement.clone(), artifact_access())]);
        assert!(reprojected.find(&legacy.entry.uri).is_some());
        assert!(reprojected.find(&legacy_collision.entry.uri).is_some());
        assert!(reprojected.find(&artifact_only.entry.uri).is_none());
        assert!(reprojected.find(&replacement.entry.uri).is_some());
        assert!(reprojected.artifact_access(&legacy.entry.uri).is_none());
        assert!(
            reprojected
                .artifact_access(&replacement.entry.uri)
                .is_some()
        );
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

        let (merged, rejections) =
            merge_bundled_first(vec![bundled.clone()], vec![same_name, same_uri]);
        assert_eq!(merged, vec![bundled]);
        assert_eq!(rejections.len(), 2);
        assert_eq!(rejections[0].kind, CollisionKind::Name);
    }

    #[test]
    fn bundled_collision_winner_never_inherits_artifact_visibility() {
        let manifest = "skill://labby/operator/using-labby/SKILL.md";
        let body = "---\nname: using-labby\ndescription: collision\n---\n";
        let local = local::LocalSkill {
            entry: SkillEntry {
                uri: manifest.to_owned(),
                frontmatter: labby_runtime::skills::parse_skill_md_frontmatter(body).unwrap(),
                resources: Some(vec![SkillResource {
                    uri: manifest.to_owned(),
                    digest: labby_runtime::skills::ResourceDigest::of_bytes(body.as_bytes())
                        .to_wire(),
                    size: body.len() as u64,
                }]),
                meta: None,
            },
            files: BTreeMap::from([(manifest.to_owned(), body.to_owned())]),
        };
        let access = ArtifactSkillAccess {
            ownership: LibraryOwnership::canonical(
                LibraryTenantId::from_canonical_projection("tenant-a").unwrap(),
                LibraryActorId::from_canonical_projection("owner").unwrap(),
            ),
            visibility: SkillVisibility::Private,
        };
        let providers = FirstPartySkillProviders::from_artifact_skills([(local, access)]);

        assert!(providers.artifact_access(manifest).is_none());
        assert!(
            providers
                .artifact_access("skill://labby/using-labby/SKILL.md")
                .is_none()
        );
        assert!(providers.collision_rejections().iter().any(|rejection| {
            rejection.skill == "using-labby" && rejection.kind == CollisionKind::Name
        }));
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
        let (merged, rejections) = merge_bundled_first(vec![first.clone()], vec![second]);
        assert_eq!(merged, vec![first]);
        assert_eq!(rejections[0].kind, CollisionKind::ResourceUri);
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

        assert!(merge_bundled_first(Vec::new(), vec![blocked]).0.is_empty());
    }
}
