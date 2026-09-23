//! Local Artifact import, export, fork, and upstream observation operations.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::path_safety::reject_existing_symlinks_in_path;

use super::lifecycle::{
    ArtifactRevisionDiff, ArtifactUpdatePlan, ArtifactWorkspaceSnapshot,
    ArtifactWorkspaceSnapshotRequest,
};
use super::local_io::{
    SnapshotFile, blocks_safe_export, ensure_export_destination, load_revision_files,
    materialize_tree, revision_dir, snapshot_local_path, sync_directory,
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
        materialize_tree(&resolved_destination, &files, options.force)?;
        Ok(files.len())
    }

    /// Export one already-verified exact provider acquisition without installing a local head.
    ///
    /// This is the detached-export primitive. It reuses the same path containment and secret
    /// guards as local export while deliberately creating no managed mirror or local authority.
    pub fn export_acquisition_exact(
        &self,
        acquisition: &ArtifactAcquisition,
        destination: &Path,
        options: ArtifactExportOptions,
    ) -> Result<usize, ArtifactError> {
        let files = snapshot_from_acquisition(acquisition)?;
        if !options.include_secrets
            && let Some(file) = files.iter().find(|file| blocks_safe_export(&file.bytes))
        {
            return Err(ArtifactError::SecretMaterialDetected {
                path: file.path.clone(),
            });
        }
        ensure_export_destination(destination, options.force)?;
        let resolved_destination = resolve_export_destination(destination)?;
        if resolved_destination == self.root
            || resolved_destination.starts_with(&self.root)
            || self.root.starts_with(&resolved_destination)
        {
            return Err(ArtifactError::UnsafePath("export_store_overlap"));
        }
        materialize_tree(&resolved_destination, &files, options.force)?;
        Ok(files.len())
    }

    /// Purge one exact local Artifact head and all of its immutable managed revisions.
    ///
    /// Callers must disable managed use before invoking this primitive. A missing Artifact is an
    /// idempotent success reported as false; a changed head fails closed.
    pub fn purge_artifact_exact(
        &self,
        artifact_id: &str,
        expected_current_revision_id: &str,
    ) -> Result<bool, ArtifactError> {
        validate_id(artifact_id, "artifact_id")?;
        validate_reference_id(expected_current_revision_id, "expected_current_revision_id")?;
        let _lock = self.lock(artifact_id)?;
        let Some(record) = self.read_record_optional(artifact_id)? else {
            // A crash between persisting a revision and publishing the record leaves bytes with no
            // record. Purge is a byte-removal guarantee, so remove whatever is on disk.
            return self.purge_artifact_directory(artifact_id);
        };
        if record.current_revision_id != expected_current_revision_id {
            return Err(ArtifactError::Conflict("head_changed"));
        }
        self.purge_artifact_directory(artifact_id)
    }

    /// Remove an Artifact's directory if it exists, reporting whether anything was removed.
    fn purge_artifact_directory(&self, artifact_id: &str) -> Result<bool, ArtifactError> {
        let artifact_dir = self.artifact_dir(artifact_id)?;
        if !artifact_dir.exists() {
            return Ok(false);
        }
        reject_existing_symlinks_in_path(&artifact_dir)
            .map_err(|_| ArtifactError::UnsafePath("stored_symlink"))?;
        std::fs::remove_dir_all(&artifact_dir)?;
        sync_directory(&self.root.join("artifacts"))?;
        Ok(true)
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

    /// Install one already-verified exact provider acquisition as a new local Artifact head.
    ///
    /// Policy and ownership decisions are made by the product layer before this call. This
    /// primitive only commits verified immutable bytes and portable metadata.
    pub fn install_acquisition_exact(
        &self,
        acquisition: ArtifactAcquisition,
    ) -> Result<ArtifactRecord, ArtifactError> {
        acquisition.validate()?;
        let artifact_id = acquisition.interchange.descriptor.id.clone();
        let revision_id = acquisition.interchange.revision.id.clone();
        let files = snapshot_from_acquisition(&acquisition)?;
        let _lock = self.lock(&artifact_id)?;

        if let Some(existing) = self.read_record_optional(&artifact_id)? {
            if existing.current_revision_id != revision_id {
                return Err(ArtifactError::Conflict("artifact_head_exists"));
            }
            let stored = self.read_revision(&artifact_id, &revision_id)?;
            if stored != acquisition.interchange.revision {
                return Err(ArtifactError::Conflict("immutable_revision_reuse"));
            }
            if !same_portable_head(&existing, &acquisition) {
                return Err(ArtifactError::Conflict("artifact_metadata_mismatch"));
            }
            return Ok(existing);
        }

        self.persist_revision(&artifact_id, &acquisition.interchange.revision, &files)?;
        self.materialize_workspace(&artifact_id, &files)?;
        let record = record_from_acquisition(&acquisition, vec![revision_id]);
        record.validate()?;
        self.persist_record(&record)?;
        Ok(record)
    }

    /// Apply one exact provider acquisition to an existing managed local Artifact.
    ///
    /// Retries are idempotent when the acquired revision is already the current head.
    pub fn apply_acquisition_update(
        &self,
        expected_current_revision_id: &str,
        acquisition: ArtifactAcquisition,
    ) -> Result<ArtifactRecord, ArtifactError> {
        validate_reference_id(expected_current_revision_id, "expected_current_revision_id")?;
        acquisition.validate()?;
        let artifact_id = acquisition.interchange.descriptor.id.clone();
        let incoming_revision_id = acquisition.interchange.revision.id.clone();
        let files = snapshot_from_acquisition(&acquisition)?;
        let _lock = self.lock(&artifact_id)?;
        let existing = self
            .read_record_optional(&artifact_id)?
            .ok_or(ArtifactError::NotFound("record"))?;

        if !same_artifact_identity(&existing.descriptor, &acquisition.interchange.descriptor) {
            return Err(ArtifactError::Conflict("upstream_identity_mismatch"));
        }

        if existing.current_revision_id == incoming_revision_id {
            let stored = self.read_revision(&artifact_id, &incoming_revision_id)?;
            if stored != acquisition.interchange.revision {
                return Err(ArtifactError::Conflict("immutable_revision_reuse"));
            }
            let mut refreshed =
                record_from_acquisition(&acquisition, existing.revision_ids.clone());
            refreshed.current_revision_id = existing.current_revision_id.clone();
            refreshed.validate()?;
            if refreshed != existing {
                self.persist_record_transition(&existing.current_revision_id, &refreshed)?;
            }
            return Ok(refreshed);
        }

        if existing.current_revision_id != expected_current_revision_id {
            return Err(ArtifactError::Conflict("head_changed"));
        }
        let mut revision_ids = existing.revision_ids.clone();
        if !revision_ids.contains(&incoming_revision_id) {
            if revision_ids.len() >= MAX_REVISIONS_PER_ARTIFACT {
                return Err(ArtifactError::LimitExceeded {
                    what: "revision_count",
                    limit: MAX_REVISIONS_PER_ARTIFACT as u64,
                });
            }
            revision_ids.push(incoming_revision_id.clone());
        }

        self.persist_revision(&artifact_id, &acquisition.interchange.revision, &files)?;
        self.materialize_workspace(&artifact_id, &files)?;
        let record = record_from_acquisition(&acquisition, revision_ids);
        record.validate()?;
        self.persist_record_transition(expected_current_revision_id, &record)?;
        Ok(record)
    }

    /// Fork one exact verified provider acquisition into a new independent local Artifact identity.
    ///
    /// The source Artifact itself is never installed as a managed local head. The fork copies the
    /// exact immutable source revision and records exact source lineage while defaulting the new
    /// Artifact publication state to private metadata. Retries for the same target identity and
    /// source revision are idempotent; divergent reuse fails closed.
    pub fn fork_acquisition_exact(
        &self,
        acquisition: ArtifactAcquisition,
        request: ArtifactForkRequest,
    ) -> Result<ArtifactRecord, ArtifactError> {
        acquisition.validate()?;
        if request.source_artifact_id != acquisition.interchange.descriptor.id {
            return Err(ArtifactError::Conflict("fork_source_identity_mismatch"));
        }
        let source_artifact_id = acquisition.interchange.descriptor.id.clone();
        let source_revision = acquisition.interchange.revision.clone();
        let source_revision_id = source_revision.id.clone();
        let files = snapshot_from_acquisition(&acquisition)?;

        let mut descriptor = ArtifactDescriptor::for_identity(
            &acquisition.interchange.descriptor.kind,
            &request.namespace,
            &request.name,
        )?;
        descriptor.title = request
            .title
            .or_else(|| acquisition.interchange.descriptor.title.clone());
        descriptor.description = acquisition.interchange.descriptor.description.clone();
        descriptor.tags = acquisition.interchange.descriptor.tags.clone();
        descriptor.metadata = acquisition.interchange.descriptor.metadata.clone();
        validation::validate_descriptor(&descriptor)?;
        if descriptor.id == source_artifact_id {
            return Err(ArtifactError::Conflict("fork_identity_matches_source"));
        }

        let _lock = self.lock(&descriptor.id)?;
        if let Some(existing) = self.read_record_optional(&descriptor.id)? {
            let same = existing.current_revision_id == source_revision_id
                && existing.lineage.forked_from_artifact_id.as_deref()
                    == Some(source_artifact_id.as_str())
                && existing.lineage.forked_from_revision_id.as_deref()
                    == Some(source_revision_id.as_str());
            if !same {
                return Err(ArtifactError::Conflict("fork_target_exists"));
            }
            let stored = self.read_revision(&descriptor.id, &source_revision_id)?;
            if stored != source_revision {
                return Err(ArtifactError::Conflict("immutable_revision_reuse"));
            }
            return Ok(existing);
        }

        self.persist_revision(&descriptor.id, &source_revision, &files)?;
        self.materialize_workspace(&descriptor.id, &files)?;
        let lineage = ArtifactLineage {
            schema_version: 1,
            upstream_artifact_id: Some(source_artifact_id.clone()),
            upstream_revision_id: Some(source_revision_id.clone()),
            forked_from_artifact_id: Some(source_artifact_id),
            forked_from_revision_id: Some(source_revision_id.clone()),
            forked_at: request.forked_at,
            following: request.following,
            last_observed_upstream_revision_id: Some(source_revision_id.clone()),
            metadata: JsonMap::new(),
        };
        validation::validate_lineage(&lineage)?;
        let record = ArtifactRecord {
            schema_version: 1,
            descriptor,
            current_revision_id: source_revision_id.clone(),
            revision_ids: vec![source_revision_id],
            provenance: acquisition.interchange.provenance,
            license: acquisition.interchange.license,
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

fn snapshot_from_acquisition(
    acquisition: &ArtifactAcquisition,
) -> Result<Vec<SnapshotFile>, ArtifactError> {
    acquisition.validate()?;
    let modes = acquisition
        .interchange
        .revision
        .components
        .iter()
        .map(|component| (component.path.as_str(), component.unix_mode()))
        .collect::<BTreeMap<_, _>>();
    let mut files = acquisition
        .files
        .iter()
        .map(|file| {
            let unix_mode = modes
                .get(file.path.as_str())
                .copied()
                .ok_or_else(|| invalid("provider_files", "missing_component"))?;
            Ok(SnapshotFile {
                path: file.path.clone(),
                bytes: file.bytes.clone(),
                unix_mode,
            })
        })
        .collect::<Result<Vec<_>, ArtifactError>>()?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn record_from_acquisition(
    acquisition: &ArtifactAcquisition,
    revision_ids: Vec<String>,
) -> ArtifactRecord {
    ArtifactRecord {
        schema_version: 1,
        descriptor: acquisition.interchange.descriptor.clone(),
        current_revision_id: acquisition.interchange.revision.id.clone(),
        revision_ids,
        provenance: acquisition.interchange.provenance.clone(),
        license: acquisition.interchange.license.clone(),
        lineage: acquisition.interchange.lineage.clone(),
        publication: acquisition.interchange.publication.clone(),
    }
}

fn same_artifact_identity(left: &ArtifactDescriptor, right: &ArtifactDescriptor) -> bool {
    left.id == right.id
        && left.kind == right.kind
        && left.namespace == right.namespace
        && left.name == right.name
}

fn same_portable_head(record: &ArtifactRecord, acquisition: &ArtifactAcquisition) -> bool {
    record.descriptor == acquisition.interchange.descriptor
        && record.provenance == acquisition.interchange.provenance
        && record.license == acquisition.interchange.license
        && record.lineage == acquisition.interchange.lineage
        && record.publication == acquisition.interchange.publication
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

    #[tokio::test]
    async fn exact_acquisition_install_is_idempotent_and_preserves_verified_bytes() {
        let source_data = tempdir().unwrap();
        let source_package = tempdir().unwrap();
        std::fs::write(source_package.path().join("a.txt"), b"alpha").unwrap();
        let source_store = ArtifactStore::new(source_data.path().join("source-store")).unwrap();
        let source = source_store
            .import_local(
                ArtifactImportRequest::new("resource", "upstream", "managed-demo"),
                source_package.path(),
            )
            .unwrap();
        let acquisition = LocalArtifactProvider::new(source_store.clone())
            .acquire(
                &ArtifactProviderRequest::new(
                    source.descriptor.id.clone(),
                    Some(source.current_revision_id.clone()),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let destination_data = tempdir().unwrap();
        let destination =
            ArtifactStore::new(destination_data.path().join("destination-store")).unwrap();
        let first = destination
            .install_acquisition_exact(acquisition.clone())
            .unwrap();
        let repeated = destination.install_acquisition_exact(acquisition).unwrap();
        assert_eq!(first, repeated);
        assert_eq!(first.current_revision_id, source.current_revision_id);
        assert_eq!(first.revision_ids, vec![source.current_revision_id.clone()]);
        assert_eq!(
            std::fs::read(
                destination
                    .workspace_path(&source.descriptor.id)
                    .unwrap()
                    .join("a.txt")
            )
            .unwrap(),
            b"alpha"
        );
    }

    #[tokio::test]
    async fn exact_acquisition_install_refuses_to_replace_a_divergent_local_head() {
        let source_data = tempdir().unwrap();
        let source_package = tempdir().unwrap();
        std::fs::write(source_package.path().join("a.txt"), b"upstream").unwrap();
        let source_store = ArtifactStore::new(source_data.path().join("source-store")).unwrap();
        let source = source_store
            .import_local(
                ArtifactImportRequest::new("resource", "shared", "collision"),
                source_package.path(),
            )
            .unwrap();
        let acquisition = LocalArtifactProvider::new(source_store.clone())
            .acquire(
                &ArtifactProviderRequest::new(
                    source.descriptor.id.clone(),
                    Some(source.current_revision_id.clone()),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let destination_data = tempdir().unwrap();
        let local_package = tempdir().unwrap();
        std::fs::write(local_package.path().join("a.txt"), b"local").unwrap();
        let destination =
            ArtifactStore::new(destination_data.path().join("destination-store")).unwrap();
        destination
            .import_local(
                ArtifactImportRequest::new("resource", "shared", "collision"),
                local_package.path(),
            )
            .unwrap();
        let error = destination
            .install_acquisition_exact(acquisition)
            .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::Conflict("artifact_head_exists")
        ));
    }

    #[tokio::test]
    async fn acquisition_update_requires_expected_head_and_retry_is_idempotent() {
        let source_data = tempdir().unwrap();
        let source_package = tempdir().unwrap();
        std::fs::write(source_package.path().join("a.txt"), b"alpha").unwrap();
        let source_store = ArtifactStore::new(source_data.path().join("source-store")).unwrap();
        let first_source = source_store
            .import_local(
                ArtifactImportRequest::new("resource", "upstream", "follow-demo"),
                source_package.path(),
            )
            .unwrap();
        let provider = LocalArtifactProvider::new(source_store.clone());
        let first_acquisition = provider
            .acquire(
                &ArtifactProviderRequest::new(
                    first_source.descriptor.id.clone(),
                    Some(first_source.current_revision_id.clone()),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let destination_data = tempdir().unwrap();
        let destination =
            ArtifactStore::new(destination_data.path().join("destination-store")).unwrap();
        let installed = destination
            .install_acquisition_exact(first_acquisition)
            .unwrap();

        std::fs::write(source_package.path().join("a.txt"), b"beta").unwrap();
        let second_source = source_store
            .import_local(
                ArtifactImportRequest::new("resource", "upstream", "follow-demo"),
                source_package.path(),
            )
            .unwrap();
        let second_acquisition = provider
            .acquire(
                &ArtifactProviderRequest::new(
                    second_source.descriptor.id.clone(),
                    Some(second_source.current_revision_id.clone()),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let wrong = format!("sha256:{}", "f".repeat(64));
        let stale = destination
            .apply_acquisition_update(&wrong, second_acquisition.clone())
            .unwrap_err();
        assert!(matches!(stale, ArtifactError::Conflict("head_changed")));

        let updated = destination
            .apply_acquisition_update(&installed.current_revision_id, second_acquisition.clone())
            .unwrap();
        assert_eq!(
            updated.current_revision_id,
            second_source.current_revision_id
        );
        assert_eq!(updated.revision_ids.len(), 2);
        let retried = destination
            .apply_acquisition_update(&installed.current_revision_id, second_acquisition)
            .unwrap();
        assert_eq!(retried, updated);
        assert_eq!(
            std::fs::read(
                destination
                    .workspace_path(&updated.descriptor.id)
                    .unwrap()
                    .join("a.txt")
            )
            .unwrap(),
            b"beta"
        );
    }

    #[tokio::test]
    async fn exact_artifact_purge_requires_the_expected_head_and_retry_is_idempotent() {
        let source_data = tempdir().unwrap();
        let source_package = tempdir().unwrap();
        std::fs::write(source_package.path().join("a.txt"), b"alpha").unwrap();
        let source_store = ArtifactStore::new(source_data.path().join("source-store")).unwrap();
        let source = source_store
            .import_local(
                ArtifactImportRequest::new("resource", "remote", "purge-source"),
                source_package.path(),
            )
            .unwrap();
        let acquisition = LocalArtifactProvider::new(source_store)
            .acquire(
                &ArtifactProviderRequest::new(
                    source.descriptor.id.clone(),
                    Some(source.current_revision_id.clone()),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let destination_data = tempdir().unwrap();
        let destination = ArtifactStore::new(destination_data.path().join("store")).unwrap();
        destination.install_acquisition_exact(acquisition).unwrap();
        let wrong = format!("sha256:{}", "f".repeat(64));
        assert!(matches!(
            destination.purge_artifact_exact(&source.descriptor.id, &wrong),
            Err(ArtifactError::Conflict("head_changed"))
        ));
        assert!(destination.get(&source.descriptor.id).is_ok());
        assert!(
            destination
                .purge_artifact_exact(&source.descriptor.id, &source.current_revision_id)
                .unwrap()
        );
        assert!(matches!(
            destination.get(&source.descriptor.id),
            Err(ArtifactError::NotFound("record"))
        ));
        assert!(
            !destination
                .purge_artifact_exact(&source.descriptor.id, &source.current_revision_id)
                .unwrap()
        );
    }

    #[tokio::test]
    async fn detached_provider_export_preserves_exact_bytes_without_installing_a_local_head() {
        let source_data = tempdir().unwrap();
        let source_package = tempdir().unwrap();
        std::fs::create_dir_all(source_package.path().join("refs")).unwrap();
        std::fs::write(source_package.path().join("a.txt"), b"alpha").unwrap();
        std::fs::write(source_package.path().join("refs/b.txt"), b"bravo").unwrap();
        let source_store = ArtifactStore::new(source_data.path().join("source-store")).unwrap();
        let source = source_store
            .import_local(
                ArtifactImportRequest::new("resource", "remote", "export-source"),
                source_package.path(),
            )
            .unwrap();
        let acquisition = LocalArtifactProvider::new(source_store)
            .acquire(
                &ArtifactProviderRequest::new(
                    source.descriptor.id.clone(),
                    Some(source.current_revision_id.clone()),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let destination_data = tempdir().unwrap();
        let store = ArtifactStore::new(destination_data.path().join("local-store")).unwrap();
        let export_root = destination_data.path().join("detached-export");
        std::fs::create_dir(&export_root).unwrap();
        let count = store
            .export_acquisition_exact(&acquisition, &export_root, ArtifactExportOptions::default())
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(std::fs::read(export_root.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(
            std::fs::read(export_root.join("refs/b.txt")).unwrap(),
            b"bravo"
        );
        assert!(matches!(
            store.get(&source.descriptor.id),
            Err(ArtifactError::NotFound("record"))
        ));
    }

    #[tokio::test]
    async fn detached_provider_export_blocks_secret_like_text_by_default() {
        let source_data = tempdir().unwrap();
        let source_package = tempdir().unwrap();
        std::fs::write(
            source_package.path().join("secret.txt"),
            b"Authorization: Bearer abcdef1234567890",
        )
        .unwrap();
        let source_store = ArtifactStore::new(source_data.path().join("source-store")).unwrap();
        let source = source_store
            .import_local(
                ArtifactImportRequest::new("resource", "remote", "secret-export-source"),
                source_package.path(),
            )
            .unwrap();
        let acquisition = LocalArtifactProvider::new(source_store)
            .acquire(
                &ArtifactProviderRequest::new(
                    source.descriptor.id.clone(),
                    Some(source.current_revision_id),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let destination_data = tempdir().unwrap();
        let store = ArtifactStore::new(destination_data.path().join("local-store")).unwrap();
        let error = store
            .export_acquisition_exact(
                &acquisition,
                &{
                    let path = destination_data.path().join("detached-export");
                    std::fs::create_dir(&path).unwrap();
                    path
                },
                ArtifactExportOptions::default(),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::SecretMaterialDetected { .. }
        ));
    }

    #[tokio::test]
    async fn provider_acquisition_can_fork_directly_to_a_new_personal_identity() {
        let source_data = tempdir().unwrap();
        let source_package = tempdir().unwrap();
        std::fs::write(source_package.path().join("a.txt"), b"alpha").unwrap();
        let source_store = ArtifactStore::new(source_data.path().join("source-store")).unwrap();
        let source = source_store
            .import_local(
                ArtifactImportRequest::new("resource", "remote", "fork-source"),
                source_package.path(),
            )
            .unwrap();
        let provider = LocalArtifactProvider::new(source_store.clone());
        let acquisition = provider
            .acquire(
                &ArtifactProviderRequest::new(
                    source.descriptor.id.clone(),
                    Some(source.current_revision_id.clone()),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let destination_data = tempdir().unwrap();
        let destination =
            ArtifactStore::new(destination_data.path().join("destination-store")).unwrap();
        let request = ArtifactForkRequest {
            source_artifact_id: source.descriptor.id.clone(),
            namespace: "personal".into(),
            name: "my-fork".into(),
            title: None,
            following: false,
            forked_at: Some("2026-09-17T12:00:00Z".into()),
        };
        let fork = destination
            .fork_acquisition_exact(acquisition.clone(), request.clone())
            .unwrap();
        assert_ne!(fork.descriptor.id, source.descriptor.id);
        assert_eq!(fork.current_revision_id, source.current_revision_id);
        assert_eq!(
            fork.lineage.forked_from_artifact_id.as_deref(),
            Some(source.descriptor.id.as_str())
        );
        assert_eq!(
            fork.lineage.forked_from_revision_id.as_deref(),
            Some(source.current_revision_id.as_str())
        );
        assert!(!fork.lineage.following);
        assert_eq!(
            std::fs::read(
                destination
                    .workspace_path(&fork.descriptor.id)
                    .unwrap()
                    .join("a.txt")
            )
            .unwrap(),
            b"alpha"
        );
        assert_eq!(
            destination
                .fork_acquisition_exact(acquisition, request)
                .unwrap(),
            fork
        );
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
