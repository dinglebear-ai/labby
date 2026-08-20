//! Local Artifact import, export, fork, and upstream observation operations.

use std::path::{Path, PathBuf};

use super::lifecycle::{
    ArtifactRevisionDiff, ArtifactUpdatePlan, ArtifactWorkspaceSnapshot,
    ArtifactWorkspaceSnapshotRequest,
};
use super::local_io::{
    blocks_safe_export, ensure_export_destination, load_revision_files, materialize_tree,
    revision_dir, snapshot_local_path,
};
use super::model::{
    ArtifactComponent, ArtifactDescriptor, ArtifactLineage, ArtifactPublication, ArtifactRecord,
    ArtifactRevision, JsonMap,
};
use super::provider::ArtifactAcquisition;
use super::store::{
    ArtifactExportOptions, ArtifactForkRequest, ArtifactImportRequest, ArtifactStore,
};
use super::validation::{
    self, MAX_COMPONENTS, MAX_REVISIONS_PER_ARTIFACT, validate_id, validate_reference_id,
};
use super::{ArtifactError, invalid};

impl ArtifactStore {
    /// Import a local file or directory as a new immutable revision.
    pub fn import_local(
        &self,
        request: ArtifactImportRequest,
        source: &Path,
    ) -> Result<ArtifactRecord, ArtifactError> {
        let descriptor = descriptor_from_import(&request)?;
        validation::validate_provenance(&request.provenance)?;
        validation::validate_license(&request.license)?;
        let _lock = self.lock(&descriptor.id)?;
        let snapshot = snapshot_local_path(source)?;
        if snapshot.len() > MAX_COMPONENTS {
            return Err(ArtifactError::LimitExceeded {
                what: "component_count",
                limit: MAX_COMPONENTS as u64,
            });
        }
        let components = snapshot
            .iter()
            .map(|file| ArtifactComponent::from_bytes(&file.path, &file.bytes, file.unix_mode))
            .collect::<Result<Vec<_>, _>>()?;
        let candidate = ArtifactRevision::from_components(
            components.clone(),
            None,
            request.authored_at.clone(),
            request.message.clone(),
            JsonMap::new(),
        )?;
        let existing = self.read_record_optional(&descriptor.id)?;
        if existing.as_ref().is_some_and(|record| {
            !record.revision_ids.contains(&candidate.id)
                && record.revision_ids.len() >= MAX_REVISIONS_PER_ARTIFACT
        }) {
            return Err(ArtifactError::LimitExceeded {
                what: "revision_count",
                limit: MAX_REVISIONS_PER_ARTIFACT as u64,
            });
        }

        let revision = if let Some(record) = existing.as_ref() {
            if record.revision_ids.contains(&candidate.id) {
                let stored = self.read_revision(&descriptor.id, &candidate.id)?;
                if stored.components != candidate.components {
                    return Err(ArtifactError::Conflict("revision_content_mismatch"));
                }
                stored
            } else {
                ArtifactRevision::from_components(
                    components,
                    Some(record.current_revision_id.clone()),
                    request.authored_at,
                    request.message,
                    JsonMap::new(),
                )?
            }
        } else {
            candidate
        };

        self.persist_revision(&descriptor.id, &revision, &snapshot)?;
        self.materialize_workspace(&descriptor.id, &snapshot)?;
        let mut revision_ids = existing
            .as_ref()
            .map_or_else(Vec::new, |record| record.revision_ids.clone());
        if !revision_ids.contains(&revision.id) {
            revision_ids.push(revision.id.clone());
        }
        let record = ArtifactRecord {
            schema_version: 1,
            descriptor,
            current_revision_id: revision.id.clone(),
            revision_ids,
            provenance: request.provenance,
            license: request.license,
            lineage: existing
                .as_ref()
                .map_or_else(ArtifactLineage::default, |record| record.lineage.clone()),
            publication: existing
                .as_ref()
                .map_or_else(ArtifactPublication::default, |record| {
                    record.publication.clone()
                }),
        };
        record.validate()?;
        self.persist_record(&record)?;
        Ok(record)
    }

    /// Export one exact revision to a local directory with path and secret guards.
    pub fn export_local(
        &self,
        artifact_id: &str,
        revision_id: Option<&str>,
        destination: &Path,
        options: ArtifactExportOptions,
    ) -> Result<usize, ArtifactError> {
        let record = self.get(artifact_id)?;
        let revision_id = revision_id.unwrap_or(&record.current_revision_id);
        let revision = self.read_revision(artifact_id, revision_id)?;
        let artifact_dir = self.artifact_dir(artifact_id)?;
        let files_root = revision_dir(&artifact_dir, revision_id).join("files");
        let files = load_revision_files(&files_root, &revision.components)?;
        if !options.include_secrets {
            if let Some(file) = files.iter().find(|file| blocks_safe_export(&file.bytes)) {
                return Err(ArtifactError::SecretMaterialDetected {
                    path: file.path.clone(),
                });
            }
        }
        ensure_export_destination(destination, options.force)?;
        let resolved_destination = resolve_export_destination(destination)?;
        if resolved_destination == self.root
            || resolved_destination.starts_with(&self.root)
            || self.root.starts_with(&resolved_destination)
        {
            return Err(ArtifactError::UnsafePath("export_store_overlap"));
        }
        materialize_tree(destination, &files, options.force)?;
        Ok(files.len())
    }

    /// Fork the source head into a new stable Artifact identity and pin lineage.
    pub fn fork(&self, request: ArtifactForkRequest) -> Result<ArtifactRecord, ArtifactError> {
        let source = self.get(&request.source_artifact_id)?;
        let source_revision =
            self.read_revision(&request.source_artifact_id, &source.current_revision_id)?;
        let source_dir = self.artifact_dir(&request.source_artifact_id)?;
        let source_files = load_revision_files(
            &revision_dir(&source_dir, &source_revision.id).join("files"),
            &source_revision.components,
        )?;

        let mut descriptor = ArtifactDescriptor::for_identity(
            &source.descriptor.kind,
            &request.namespace,
            &request.name,
        )?;
        descriptor.title = request.title.or_else(|| source.descriptor.title.clone());
        descriptor.description = source.descriptor.description.clone();
        descriptor.tags = source.descriptor.tags.clone();
        descriptor.metadata = source.descriptor.metadata.clone();
        validation::validate_descriptor(&descriptor)?;
        if descriptor.id == source.descriptor.id {
            return Err(ArtifactError::Conflict("fork_identity_matches_source"));
        }

        let _lock = self.lock(&descriptor.id)?;
        if self.read_record_optional(&descriptor.id)?.is_some() {
            return Err(ArtifactError::Conflict("fork_target_exists"));
        }
        self.persist_revision(&descriptor.id, &source_revision, &source_files)?;
        self.materialize_workspace(&descriptor.id, &source_files)?;

        let lineage = ArtifactLineage {
            schema_version: 1,
            upstream_artifact_id: Some(source.descriptor.id.clone()),
            upstream_revision_id: Some(source_revision.id.clone()),
            forked_from_artifact_id: Some(source.descriptor.id),
            forked_from_revision_id: Some(source_revision.id.clone()),
            forked_at: request.forked_at,
            following: request.following,
            last_observed_upstream_revision_id: Some(source_revision.id.clone()),
            metadata: JsonMap::new(),
        };
        validation::validate_lineage(&lineage)?;
        let record = ArtifactRecord {
            schema_version: 1,
            descriptor,
            current_revision_id: source_revision.id.clone(),
            revision_ids: vec![source_revision.id],
            provenance: source.provenance,
            license: source.license,
            lineage,
            publication: ArtifactPublication::default(),
        };
        record.validate()?;
        self.persist_record(&record)?;
        Ok(record)
    }

    /// Snapshot the editable workspace as an immutable revision and move the local head explicitly.
    pub fn snapshot_workspace(
        &self,
        artifact_id: &str,
        request: ArtifactWorkspaceSnapshotRequest,
    ) -> Result<ArtifactWorkspaceSnapshot, ArtifactError> {
        validate_id(artifact_id, "artifact_id")?;
        let _lock = self.lock(artifact_id)?;
        let mut record = self.get(artifact_id)?;
        let base_revision_id = record.current_revision_id.clone();
        let workspace = self.workspace_path(artifact_id)?;
        let snapshot = snapshot_local_path(&workspace)?;
        if snapshot.len() > MAX_COMPONENTS {
            return Err(ArtifactError::LimitExceeded {
                what: "component_count",
                limit: MAX_COMPONENTS as u64,
            });
        }
        let components = snapshot
            .iter()
            .map(|file| ArtifactComponent::from_bytes(&file.path, &file.bytes, file.unix_mode))
            .collect::<Result<Vec<_>, _>>()?;
        let content_candidate = ArtifactRevision::from_components(
            components,
            None,
            request.authored_at.clone(),
            request.message.clone(),
            request.metadata.clone(),
        )?;

        if content_candidate.id == base_revision_id {
            let revision = self.read_revision(artifact_id, &base_revision_id)?;
            return Ok(ArtifactWorkspaceSnapshot {
                record,
                revision,
                created_revision: false,
                moved_head: false,
            });
        }

        let (revision, created_revision) = if record.revision_ids.contains(&content_candidate.id) {
            let stored = self.read_revision(artifact_id, &content_candidate.id)?;
            if stored.components != content_candidate.components {
                return Err(ArtifactError::Conflict("revision_content_mismatch"));
            }
            (stored, false)
        } else {
            if record.revision_ids.len() >= MAX_REVISIONS_PER_ARTIFACT {
                return Err(ArtifactError::LimitExceeded {
                    what: "revision_count",
                    limit: MAX_REVISIONS_PER_ARTIFACT as u64,
                });
            }
            let revision = ArtifactRevision::from_components(
                content_candidate.components,
                Some(base_revision_id.clone()),
                request.authored_at,
                request.message,
                request.metadata,
            )?;
            self.persist_revision(artifact_id, &revision, &snapshot)?;
            record.revision_ids.push(revision.id.clone());
            (revision, true)
        };

        record.current_revision_id = revision.id.clone();
        record.validate()?;
        self.persist_record_transition(&base_revision_id, &record)?;
        Ok(ArtifactWorkspaceSnapshot {
            record,
            revision,
            created_revision,
            moved_head: true,
        })
    }

    /// Diff two exact local immutable revisions without mutating the Artifact.
    pub fn diff_local_revisions(
        &self,
        artifact_id: &str,
        from_revision_id: &str,
        to_revision_id: &str,
    ) -> Result<ArtifactRevisionDiff, ArtifactError> {
        validate_id(artifact_id, "artifact_id")?;
        validate_reference_id(from_revision_id, "from_revision_id")?;
        validate_reference_id(to_revision_id, "to_revision_id")?;
        let from = self.read_revision(artifact_id, from_revision_id)?;
        let to = self.read_revision(artifact_id, to_revision_id)?;
        ArtifactRevisionDiff::between(&from, &to)
    }

    /// Build a read-only update plan from an exact provider acquisition.
    ///
    /// Planning never changes the local head, workspace, lineage, or stored bytes.
    pub fn plan_update_from_acquisition(
        &self,
        target_artifact_id: &str,
        acquisition: &ArtifactAcquisition,
    ) -> Result<ArtifactUpdatePlan, ArtifactError> {
        validate_id(target_artifact_id, "target_artifact_id")?;
        acquisition.validate()?;
        let record = self.get(target_artifact_id)?;
        if acquisition.interchange.descriptor.kind != record.descriptor.kind {
            return Err(ArtifactError::Conflict("source_kind_mismatch"));
        }
        let expected_source_artifact_id = record
            .lineage
            .upstream_artifact_id
            .as_deref()
            .unwrap_or(record.descriptor.id.as_str());
        if acquisition.interchange.descriptor.id != expected_source_artifact_id {
            return Err(ArtifactError::Conflict("upstream_identity_mismatch"));
        }

        let base = self.read_revision(target_artifact_id, &record.current_revision_id)?;
        let diff = ArtifactRevisionDiff::between(&base, &acquisition.interchange.revision)?;
        let plan = ArtifactUpdatePlan {
            schema_version: 1,
            target_artifact_id: record.descriptor.id,
            base_revision_id: base.id,
            source_artifact_id: acquisition.interchange.descriptor.id.clone(),
            source_revision_id: acquisition.interchange.revision.id.clone(),
            source_provenance: acquisition.interchange.provenance.clone(),
            diff,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// Record a newly observed upstream revision without changing local bytes.
    pub fn observe_upstream(
        &self,
        artifact_id: &str,
        upstream_artifact_id: &str,
        observed_revision_id: &str,
    ) -> Result<ArtifactRecord, ArtifactError> {
        validate_id(artifact_id, "artifact_id")?;
        validate_id(upstream_artifact_id, "upstream_artifact_id")?;
        validate_reference_id(observed_revision_id, "observed_revision_id")?;
        let _lock = self.lock(artifact_id)?;
        let mut record = self.get(artifact_id)?;
        if record.lineage.upstream_artifact_id.as_deref() != Some(upstream_artifact_id) {
            return Err(ArtifactError::Conflict("upstream_identity_mismatch"));
        }
        record.lineage.last_observed_upstream_revision_id = Some(observed_revision_id.to_string());
        record.validate()?;
        self.persist_record(&record)?;
        Ok(record)
    }
}

fn resolve_export_destination(destination: &Path) -> Result<PathBuf, ArtifactError> {
    if !destination.is_absolute() {
        return Err(ArtifactError::UnsafePath("export_root_relative"));
    }
    if destination.exists() {
        return std::fs::canonicalize(destination).map_err(ArtifactError::from);
    }
    let parent = destination
        .parent()
        .ok_or(ArtifactError::UnsafePath("destination_root"))?;
    let parent = std::fs::canonicalize(parent)?;
    let name = destination
        .file_name()
        .ok_or(ArtifactError::UnsafePath("destination_root"))?;
    Ok(parent.join(name))
}

fn descriptor_from_import(
    request: &ArtifactImportRequest,
) -> Result<ArtifactDescriptor, ArtifactError> {
    let mut descriptor =
        ArtifactDescriptor::for_identity(&request.kind, &request.namespace, &request.name)?;
    descriptor.title = request.title.clone();
    descriptor.description = request.description.clone();
    descriptor.tags = request.tags.clone();
    descriptor.metadata = request.metadata.clone();
    validation::validate_descriptor(&descriptor)?;
    if request
        .message
        .as_ref()
        .is_some_and(|message| message.len() > 4_096)
    {
        return Err(invalid("message", "too_long"));
    }
    Ok(descriptor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::{ArtifactProvider, ArtifactProviderRequest, LocalArtifactProvider};
    use tempfile::tempdir;

    fn write_package(root: &Path) {
        std::fs::create_dir_all(root.join("references")).unwrap();
        std::fs::write(root.join("SKILL.md"), b"skill").unwrap();
        std::fs::write(root.join("references/REF.md"), b"reference").unwrap();
    }

    #[test]
    fn import_creates_immutable_revision_and_workspace() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        write_package(source.path());
        let store = ArtifactStore::new(data.path().join("artifacts-store")).unwrap();
        let record = store
            .import_local(
                ArtifactImportRequest::new("skill", "labby", "demo"),
                source.path(),
            )
            .unwrap();
        let revision = store
            .revision(&record.descriptor.id, &record.current_revision_id)
            .unwrap();
        assert_eq!(revision.components.len(), 2);
        assert!(
            store
                .workspace_path(&record.descriptor.id)
                .unwrap()
                .join("SKILL.md")
                .exists()
        );
    }

    #[test]
    fn safe_export_blocks_secret_like_text_by_default() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        std::fs::write(
            source.path().join("config.txt"),
            b"Authorization: Bearer abcdef1234567890",
        )
        .unwrap();
        let store = ArtifactStore::new(data.path().join("artifacts-store")).unwrap();
        let record = store
            .import_local(
                ArtifactImportRequest::new("resource", "labby", "config"),
                source.path(),
            )
            .unwrap();
        let output = tempdir().unwrap();
        let err = store
            .export_local(
                &record.descriptor.id,
                None,
                output.path(),
                ArtifactExportOptions::default(),
            )
            .unwrap_err();
        assert!(matches!(err, ArtifactError::SecretMaterialDetected { .. }));
    }

    #[test]
    fn export_refuses_store_overlap_even_with_force() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        write_package(source.path());
        let store = ArtifactStore::new(data.path().join("artifacts-store")).unwrap();
        let record = store
            .import_local(
                ArtifactImportRequest::new("skill", "labby", "demo"),
                source.path(),
            )
            .unwrap();
        let error = store
            .export_local(
                &record.descriptor.id,
                None,
                store.root(),
                ArtifactExportOptions {
                    include_secrets: true,
                    force: true,
                },
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::UnsafePath("export_store_overlap")
        ));
    }

    #[test]
    fn workspace_snapshot_reuses_content_revisions_and_diff_is_deterministic() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        std::fs::write(source.path().join("a.txt"), b"alpha").unwrap();
        let store = ArtifactStore::new(data.path().join("artifacts-store")).unwrap();
        let imported = store
            .import_local(
                ArtifactImportRequest::new("resource", "labby", "snapshot-demo"),
                source.path(),
            )
            .unwrap();
        let initial_revision_id = imported.current_revision_id.clone();
        let workspace = store.workspace_path(&imported.descriptor.id).unwrap();
        std::fs::write(workspace.join("a.txt"), b"beta").unwrap();
        std::fs::write(workspace.join("b.txt"), b"bravo").unwrap();

        let changed = store
            .snapshot_workspace(
                &imported.descriptor.id,
                ArtifactWorkspaceSnapshotRequest {
                    message: Some("edit workspace".to_string()),
                    ..ArtifactWorkspaceSnapshotRequest::default()
                },
            )
            .unwrap();
        assert!(changed.created_revision);
        assert!(changed.moved_head);
        assert_eq!(
            changed.revision.parent_revision_id.as_deref(),
            Some(initial_revision_id.as_str())
        );
        assert_eq!(changed.record.revision_ids.len(), 2);

        let diff = store
            .diff_local_revisions(
                &imported.descriptor.id,
                &initial_revision_id,
                &changed.revision.id,
            )
            .unwrap();
        assert_eq!(
            diff.changes
                .iter()
                .map(|change| (change.path.as_str(), change.kind))
                .collect::<Vec<_>>(),
            vec![
                (
                    "a.txt",
                    super::super::lifecycle::ArtifactChangeKind::Modified
                ),
                ("b.txt", super::super::lifecycle::ArtifactChangeKind::Added),
            ]
        );

        let unchanged = store
            .snapshot_workspace(
                &imported.descriptor.id,
                ArtifactWorkspaceSnapshotRequest::default(),
            )
            .unwrap();
        assert!(!unchanged.created_revision);
        assert!(!unchanged.moved_head);
        assert_eq!(unchanged.record.revision_ids.len(), 2);

        std::fs::write(workspace.join("a.txt"), b"alpha").unwrap();
        std::fs::remove_file(workspace.join("b.txt")).unwrap();
        let reverted = store
            .snapshot_workspace(
                &imported.descriptor.id,
                ArtifactWorkspaceSnapshotRequest::default(),
            )
            .unwrap();
        assert!(!reverted.created_revision);
        assert!(reverted.moved_head);
        assert_eq!(reverted.revision.id, initial_revision_id);
        assert_eq!(reverted.record.revision_ids.len(), 2);
    }

    #[tokio::test]
    async fn provider_update_plan_never_applies_source_revision() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        std::fs::write(source.path().join("a.txt"), b"alpha").unwrap();
        let store = ArtifactStore::new(data.path().join("artifacts-store")).unwrap();
        let upstream = store
            .import_local(
                ArtifactImportRequest::new("resource", "labby", "provider-upstream"),
                source.path(),
            )
            .unwrap();
        let fork = store
            .fork(ArtifactForkRequest {
                source_artifact_id: upstream.descriptor.id.clone(),
                namespace: "personal".to_string(),
                name: "provider-fork".to_string(),
                title: None,
                following: true,
                forked_at: None,
            })
            .unwrap();
        let fork_head = fork.current_revision_id.clone();

        std::fs::write(source.path().join("a.txt"), b"beta").unwrap();
        let advanced_upstream = store
            .import_local(
                ArtifactImportRequest::new("resource", "labby", "provider-upstream"),
                source.path(),
            )
            .unwrap();
        assert_ne!(advanced_upstream.current_revision_id, fork_head);

        let provider = LocalArtifactProvider::new(store.clone());
        let acquisition = provider
            .acquire(
                &ArtifactProviderRequest::new(
                    advanced_upstream.descriptor.id.clone(),
                    Some(advanced_upstream.current_revision_id.clone()),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let plan = store
            .plan_update_from_acquisition(&fork.descriptor.id, &acquisition)
            .unwrap();
        assert_eq!(plan.base_revision_id, fork_head);
        assert_eq!(plan.source_artifact_id, upstream.descriptor.id);
        assert_eq!(
            plan.source_revision_id,
            advanced_upstream.current_revision_id
        );
        assert_eq!(plan.diff.changes.len(), 1);
        assert_eq!(
            plan.diff.changes[0].kind,
            super::super::lifecycle::ArtifactChangeKind::Modified
        );

        let unchanged_fork = store.get(&fork.descriptor.id).unwrap();
        assert_eq!(unchanged_fork.current_revision_id, fork.current_revision_id);
        assert_eq!(unchanged_fork.revision_ids, fork.revision_ids);
    }

    #[test]
    fn fork_pins_source_revision_and_observation_does_not_update_bytes() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        write_package(source.path());
        let store = ArtifactStore::new(data.path().join("artifacts-store")).unwrap();
        let upstream = store
            .import_local(
                ArtifactImportRequest::new("skill", "labby", "demo"),
                source.path(),
            )
            .unwrap();
        let fork = store
            .fork(ArtifactForkRequest {
                source_artifact_id: upstream.descriptor.id.clone(),
                namespace: "personal".to_string(),
                name: "demo-fork".to_string(),
                title: None,
                following: true,
                forked_at: None,
            })
            .unwrap();
        assert_eq!(
            fork.lineage.forked_from_revision_id.as_deref(),
            Some(upstream.current_revision_id.as_str())
        );
        let fake = format!("sha256:{}", "f".repeat(64));
        let observed = store
            .observe_upstream(&fork.descriptor.id, &upstream.descriptor.id, &fake)
            .unwrap();
        assert_eq!(observed.current_revision_id, fork.current_revision_id);
        assert_eq!(
            observed
                .lineage
                .last_observed_upstream_revision_id
                .as_deref(),
            Some(fake.as_str())
        );
    }
}
