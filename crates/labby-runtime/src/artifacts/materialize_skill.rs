//! Canonical conversion between bounded logical Skill files and Artifacts.

use std::collections::{BTreeMap, BTreeSet};

use std::path::Path;

use crate::skills::{
    ResourceDigest, SkillEntry, SkillResource, ValidatedSkill, parse_skill_md_frontmatter,
    validate_skill_entry_detailed,
};

use super::local_io::SnapshotFile;
use super::model::ArtifactRecord;
use super::model::{ArtifactInterchange, ArtifactProvenance};
use super::provider::ArtifactAcquisition;
use super::skill::interchange_from_validated_skill;
use super::store::ArtifactStore;
use super::validation::{self, MAX_SKILL_PACKAGE_BYTES};
use super::{ArtifactError, invalid};

/// One browser-safe logical file. `path` is package-relative and `content` is UTF-8 text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalSkillFile {
    pub path: String,
    pub content: String,
}

impl LogicalSkillFile {
    #[must_use]
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
        }
    }
}

/// A fully verified candidate. Creating this value never activates the Skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedSkill {
    pub skill: ValidatedSkill,
    pub resources: BTreeMap<String, Vec<u8>>,
    pub interchange: ArtifactInterchange,
}

impl ArtifactStore {
    /// Read one manifest-bound file from an exact immutable Skill revision.
    pub fn read_skill_revision_file(
        &self,
        artifact_id: &str,
        revision_id: &str,
        path: &str,
    ) -> Result<Vec<u8>, ArtifactError> {
        validation::validate_relative_path(path)?;
        let revision = self.revision(artifact_id, revision_id)?;
        let component = revision
            .components
            .iter()
            .find(|component| component.path == path)
            .ok_or(ArtifactError::NotFound("revision_file"))?;
        let artifact_dir = self.artifact_dir(artifact_id)?;
        let files = super::local_io::load_revision_files(
            &super::local_io::revision_dir(&artifact_dir, revision_id).join("files"),
            std::slice::from_ref(component),
        )?;
        Ok(files
            .into_iter()
            .next()
            .expect("one requested component")
            .bytes)
    }

    /// Persist the exact retained bytes as a new immutable Skill revision.
    ///
    /// `expected_revision_id=None` creates a new Artifact; `Some` is a head CAS for save.
    pub fn persist_materialized_skill(
        &self,
        mut materialized: MaterializedSkill,
        expected_revision_id: Option<&str>,
    ) -> Result<ArtifactRecord, ArtifactError> {
        let artifact_id = materialized.interchange.descriptor.id.clone();
        let _lock = self.lock(&artifact_id)?;
        let existing = self.read_record_optional(&artifact_id)?;
        match (existing.as_ref(), expected_revision_id) {
            (None, None) => {}
            (Some(record), Some(expected)) if record.current_revision_id == expected => {}
            (None, Some(_)) => return Err(ArtifactError::NotFound("record")),
            (Some(_), None) => return Err(ArtifactError::Conflict("artifact_exists")),
            (Some(_), Some(_)) => return Err(ArtifactError::Conflict("revision_changed")),
        }
        if let Some(record) = &existing {
            materialized.interchange.revision.parent_revision_id =
                Some(record.current_revision_id.clone());
        }
        let root = format!(
            "skill://labby/{}/",
            materialized.interchange.descriptor.name
        );
        let files = materialized
            .resources
            .into_iter()
            .map(|(uri, bytes)| {
                let path = uri
                    .strip_prefix(&root)
                    .ok_or_else(|| logical_error(&uri, "uri_root"))?;
                Ok(SnapshotFile {
                    path: path.to_owned(),
                    bytes,
                    unix_mode: None,
                })
            })
            .collect::<Result<Vec<_>, ArtifactError>>()?;
        let revision = materialized.interchange.revision;
        self.persist_revision(&artifact_id, &revision, &files)?;
        self.materialize_workspace(&artifact_id, &files)?;
        let mut revision_ids = existing
            .as_ref()
            .map_or_else(Vec::new, |record| record.revision_ids.clone());
        if !revision_ids.contains(&revision.id) {
            revision_ids.push(revision.id.clone());
        }
        let record = ArtifactRecord {
            schema_version: 1,
            descriptor: materialized.interchange.descriptor,
            current_revision_id: revision.id,
            revision_ids,
            provenance: materialized.interchange.provenance,
            license: materialized.interchange.license,
            lineage: existing.as_ref().map_or_else(
                || materialized.interchange.lineage,
                |record| record.lineage.clone(),
            ),
            publication: existing.as_ref().map_or_else(
                || materialized.interchange.publication,
                |record| record.publication.clone(),
            ),
        };
        record.validate()?;
        self.persist_record(&record)?;
        Ok(record)
    }
}

/// Convert logical authored/imported files through the existing Skills validator.
pub fn materialize_logical_skill(
    name: &str,
    files: Vec<LogicalSkillFile>,
    provenance: ArtifactProvenance,
) -> Result<MaterializedSkill, ArtifactError> {
    materialize_skill_bytes(
        name,
        files
            .into_iter()
            .map(|file| (file.path, file.content.into_bytes()))
            .collect(),
        provenance,
    )
}

fn materialize_skill_bytes(
    name: &str,
    files: Vec<(String, Vec<u8>)>,
    provenance: ArtifactProvenance,
) -> Result<MaterializedSkill, ArtifactError> {
    let mut by_path = BTreeMap::new();
    let mut folded = BTreeSet::new();
    let mut total = 0usize;
    for (path, bytes) in files {
        logical_path(&path)?;
        if by_path.contains_key(&path) {
            return Err(logical_error(&path, "duplicate_path"));
        }
        if !folded.insert(path.to_lowercase()) {
            return Err(logical_error(&path, "case_fold_collision"));
        }
        if by_path.len() >= crate::skills::limits::MAX_RESOURCES_PER_SKILL {
            return Err(ArtifactError::LimitExceeded {
                what: "skill_file_count",
                limit: crate::skills::limits::MAX_RESOURCES_PER_SKILL as u64,
            });
        }
        let content = std::str::from_utf8(&bytes).map_err(|_| logical_error(&path, "non_utf8"))?;
        if content.contains('\0') {
            return Err(logical_error(&path, "nul_content"));
        }
        total = total
            .checked_add(bytes.len())
            .ok_or(ArtifactError::LimitExceeded {
                what: "skill_package_size",
                limit: MAX_SKILL_PACKAGE_BYTES as u64,
            })?;
        if bytes.len() > crate::skills::limits::MAX_SKILL_RESOURCE_BYTES {
            return Err(ArtifactError::LimitExceeded {
                what: "skill_file_size",
                limit: crate::skills::limits::MAX_SKILL_RESOURCE_BYTES as u64,
            });
        }
        if total > MAX_SKILL_PACKAGE_BYTES {
            return Err(ArtifactError::LimitExceeded {
                what: "skill_package_size",
                limit: MAX_SKILL_PACKAGE_BYTES as u64,
            });
        }
        by_path.insert(path, bytes);
    }
    let skill_md = by_path
        .get("SKILL.md")
        .ok_or_else(|| logical_error("SKILL.md", "missing"))?;
    let text = std::str::from_utf8(skill_md).map_err(|_| logical_error("SKILL.md", "non_utf8"))?;
    let frontmatter = parse_skill_md_frontmatter(text)
        .map_err(|_| logical_error("SKILL.md", "invalid_frontmatter"))?;
    if let Some(metadata) = frontmatter.get("metadata") {
        validation::validate_json(metadata, "skill_metadata", 32_768, true)?;
    }

    let root = format!("skill://labby/{name}");
    let resources_manifest = by_path
        .iter()
        .map(|(path, bytes)| SkillResource {
            uri: format!("{root}/{path}"),
            digest: ResourceDigest::of_bytes(bytes).to_wire(),
            size: bytes.len() as u64,
        })
        .collect();
    let entry = SkillEntry {
        uri: format!("{root}/SKILL.md"),
        frontmatter,
        resources: Some(resources_manifest),
        meta: None,
    };
    let skill =
        validate_skill_entry_detailed(&entry).map_err(|_| ArtifactError::SkillVerification)?;
    let resources = by_path
        .into_iter()
        .map(|(path, bytes)| (format!("{root}/{path}"), bytes))
        .collect();
    let interchange = interchange_from_validated_skill(&skill, &resources, provenance)?;
    Ok(MaterializedSkill {
        skill,
        resources,
        interchange,
    })
}

/// Import a Skill from operator-trusted private staging.
///
/// The staging directory must not be concurrently writable by an untrusted
/// principal. The snapshot reader rejects links, hardlinks, special files, and
/// replacement races; callers must still establish this ownership boundary.
pub fn materialize_skill_from_trusted_staging(
    name: &str,
    source: &Path,
    provenance: ArtifactProvenance,
) -> Result<MaterializedSkill, ArtifactError> {
    let files = super::local_io::snapshot_local_path(source)?
        .into_iter()
        .map(|file| (file.path, file.bytes))
        .collect();
    materialize_skill_bytes(name, files, provenance)
}

/// Reconstruct acquired bytes through the same canonical logical-file path.
pub fn materialize_acquired_skill(
    acquisition: &ArtifactAcquisition,
) -> Result<MaterializedSkill, ArtifactError> {
    acquisition.validate()?;
    if acquisition.interchange.descriptor.kind != "skill" {
        return Err(invalid("artifact_kind", "not_skill"));
    }
    let files = acquisition
        .files
        .iter()
        .map(|file| (file.path.clone(), file.bytes.clone()))
        .collect();
    let result = materialize_skill_bytes(
        &acquisition.interchange.descriptor.name,
        files,
        acquisition.interchange.provenance.clone(),
    )?;
    if result.interchange != acquisition.interchange {
        return Err(ArtifactError::Conflict("skill_artifact_not_canonical"));
    }
    Ok(result)
}

/// Consume an acquisition and reuse its payload allocations in the canonical result.
pub fn materialize_acquired_skill_owned(
    acquisition: ArtifactAcquisition,
) -> Result<MaterializedSkill, ArtifactError> {
    acquisition.validate()?;
    let ArtifactAcquisition { interchange, files } = acquisition;
    if interchange.descriptor.kind != "skill" {
        return Err(invalid("artifact_kind", "not_skill"));
    }
    let files = files
        .into_iter()
        .map(|file| (file.path, file.bytes))
        .collect();
    let result = materialize_skill_bytes(
        &interchange.descriptor.name,
        files,
        interchange.provenance.clone(),
    )?;
    if result.interchange != interchange {
        return Err(ArtifactError::Conflict("skill_artifact_not_canonical"));
    }
    Ok(result)
}

fn logical_path(path: &str) -> Result<(), ArtifactError> {
    validation::validate_relative_path(path).map_err(|error| match error {
        ArtifactError::UnsafePath(reason) => logical_error(path, reason),
        ArtifactError::InvalidField { reason, .. } => logical_error(path, reason),
        other => other,
    })?;
    if path.as_bytes().contains(&0) {
        return Err(logical_error(path, "nul_byte"));
    }
    Ok(())
}

fn logical_error(path: &str, reason: &'static str) -> ArtifactError {
    ArtifactError::LogicalSkillFile {
        path: path.chars().take(256).collect(),
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::{ArtifactAcquisition, ArtifactPayloadFile};

    fn files() -> Vec<LogicalSkillFile> {
        vec![
            LogicalSkillFile::new(
                "SKILL.md",
                "---\nname: demo\ndescription: Demo\n---\nBody\n",
            ),
            LogicalSkillFile::new("references/REF.md", "reference\n"),
        ]
    }

    fn acquisition_from_materialized(materialized: MaterializedSkill) -> ArtifactAcquisition {
        let MaterializedSkill {
            resources,
            interchange,
            ..
        } = materialized;
        let root = format!("skill://labby/{}/", interchange.descriptor.name);
        let files = resources
            .into_iter()
            .map(|(uri, bytes)| ArtifactPayloadFile {
                path: uri.strip_prefix(&root).unwrap().to_owned(),
                bytes,
            })
            .collect();
        ArtifactAcquisition { interchange, files }
    }

    #[test]
    fn authored_and_acquired_round_trip_exactly() {
        let authored =
            materialize_logical_skill("demo", files(), ArtifactProvenance::default()).unwrap();
        let acquisition = ArtifactAcquisition {
            interchange: authored.interchange.clone(),
            files: authored
                .interchange
                .revision
                .components
                .iter()
                .map(|component| ArtifactPayloadFile {
                    path: component.path.clone(),
                    bytes: authored.resources[&format!("skill://labby/demo/{}", component.path)]
                        .clone(),
                })
                .collect(),
        };
        let acquired = materialize_acquired_skill(&acquisition).unwrap();
        assert_eq!(authored, acquired);
    }

    #[test]
    fn owned_acquisition_reuses_payload_allocation() {
        let acquisition = acquisition_from_materialized(
            materialize_logical_skill("demo", files(), ArtifactProvenance::default()).unwrap(),
        );
        let payload = acquisition
            .files
            .iter()
            .find(|file| file.path == "references/REF.md")
            .unwrap();
        let pointer = payload.bytes.as_ptr();
        let result = materialize_acquired_skill_owned(acquisition).unwrap();
        assert_eq!(
            result.resources["skill://labby/demo/references/REF.md"].as_ptr(),
            pointer
        );
    }

    #[test]
    fn exact_materialized_bytes_persist_with_head_cas() {
        let root = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(root.path()).unwrap();
        let first =
            materialize_logical_skill("demo", files(), ArtifactProvenance::default()).unwrap();
        let record = store.persist_materialized_skill(first, None).unwrap();
        let first_revision = record.current_revision_id.clone();
        assert!(
            store
                .revision(&record.descriptor.id, &first_revision)
                .is_ok()
        );

        let mut changed = files();
        changed[1].content = "changed reference\n".to_owned();
        let second =
            materialize_logical_skill("demo", changed, ArtifactProvenance::default()).unwrap();
        let record = store
            .persist_materialized_skill(second, Some(&first_revision))
            .unwrap();
        assert_ne!(record.current_revision_id, first_revision);
        let stale =
            materialize_logical_skill("demo", files(), ArtifactProvenance::default()).unwrap();
        assert!(matches!(
            store.persist_materialized_skill(stale, Some(&first_revision)),
            Err(ArtifactError::Conflict("revision_changed"))
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "max-package RSS regression harness; run explicitly on a quiet host"]
    fn owned_max_package_materialization_does_not_duplicate_the_corpus() {
        fn rss() -> u64 {
            let status = std::fs::read_to_string("/proc/self/status").unwrap();
            let kib = status
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap()
                .parse::<u64>()
                .unwrap();
            kib * 1024
        }
        let mut logical = Vec::with_capacity(crate::skills::limits::MAX_RESOURCES_PER_SKILL);
        let mut skill_md = "---\nname: maximal\ndescription: Maximal\n---\n".to_owned();
        skill_md.push_str(
            &"x".repeat(crate::skills::limits::MAX_SKILL_RESOURCE_BYTES - skill_md.len()),
        );
        logical.push(LogicalSkillFile::new("SKILL.md", skill_md));
        for index in 1..crate::skills::limits::MAX_RESOURCES_PER_SKILL {
            logical.push(LogicalSkillFile::new(
                format!("resource-{index:02}.txt"),
                "x".repeat(crate::skills::limits::MAX_SKILL_RESOURCE_BYTES),
            ));
        }
        let acquisition = acquisition_from_materialized(
            materialize_logical_skill("maximal", logical, ArtifactProvenance::default()).unwrap(),
        );
        let before = rss();
        let result = materialize_acquired_skill_owned(acquisition).unwrap();
        let after = rss();
        assert_eq!(
            result.resources.len(),
            crate::skills::limits::MAX_RESOURCES_PER_SKILL
        );
        assert!(after.saturating_sub(before) < 8 * 1024 * 1024);
    }

    #[test]
    fn rejects_paths_frontmatter_and_tampering_without_content_in_error() {
        for path in ["../x", "/x", "a\\b"] {
            let err = materialize_logical_skill(
                "demo",
                vec![LogicalSkillFile::new(path, "secret-body")],
                ArtifactProvenance::default(),
            )
            .unwrap_err();
            assert!(!err.to_string().contains("secret-body"));
        }
        let mut collision = files();
        collision.push(LogicalSkillFile::new("skill.md", "x"));
        assert!(
            materialize_logical_skill("demo", collision, ArtifactProvenance::default()).is_err()
        );
        let mut mismatch = files();
        mismatch[0].content = "---\nname: other\ndescription: Demo\n---\n".into();
        assert!(
            materialize_logical_skill("demo", mismatch, ArtifactProvenance::default()).is_err()
        );

        let authored =
            materialize_logical_skill("demo", files(), ArtifactProvenance::default()).unwrap();
        let mut acquisition = ArtifactAcquisition {
            interchange: authored.interchange,
            files: vec![],
        };
        acquisition.files = acquisition
            .interchange
            .revision
            .components
            .iter()
            .map(|component| ArtifactPayloadFile {
                path: component.path.clone(),
                bytes: b"tampered".to_vec(),
            })
            .collect();
        assert!(materialize_acquired_skill(&acquisition).is_err());
    }

    #[test]
    fn rejects_secret_shaped_frontmatter_metadata() {
        let files = vec![LogicalSkillFile::new(
            "SKILL.md",
            "---\nname: demo\ndescription: Demo\nmetadata:\n  api_token: nope\n---\n",
        )];
        let err =
            materialize_logical_skill("demo", files, ArtifactProvenance::default()).unwrap_err();
        assert!(matches!(
            err,
            ArtifactError::InvalidField {
                reason: "secret_key",
                ..
            }
        ));
        assert!(!err.to_string().contains("nope"));
    }

    #[test]
    fn immutable_revision_id_cannot_be_reused_for_divergent_content() {
        let authored =
            materialize_logical_skill("demo", files(), ArtifactProvenance::default()).unwrap();
        let mut changed_components = authored.interchange.revision.components.clone();
        let component = changed_components
            .iter_mut()
            .find(|file| file.path == "references/REF.md")
            .unwrap();
        let divergent = b"divergent\n".to_vec();
        component.digest = crate::artifacts::canonical_json::sha256_bytes(&divergent);
        component.size = divergent.len() as u64;
        let replacement = crate::artifacts::ArtifactRevision::from_components(
            changed_components,
            None,
            None,
            None,
            Default::default(),
        )
        .unwrap();
        let mut interchange = authored.interchange.clone();
        interchange.revision.components = replacement.components;
        interchange.revision.content_digest = replacement.content_digest;
        let acquisition = ArtifactAcquisition {
            interchange,
            files: authored
                .interchange
                .revision
                .components
                .iter()
                .map(|component| ArtifactPayloadFile {
                    path: component.path.clone(),
                    bytes: if component.path == "references/REF.md" {
                        divergent.clone()
                    } else {
                        authored.resources[&format!("skill://labby/demo/{}", component.path)]
                            .clone()
                    },
                })
                .collect(),
        };
        assert!(matches!(
            materialize_acquired_skill(&acquisition).unwrap_err(),
            ArtifactError::Conflict("skill_artifact_not_canonical")
        ));
    }
}
