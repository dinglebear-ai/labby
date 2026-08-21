//! Canonical local Artifact store and persistence primitives.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::path_safety::{
    canonicalize_and_reject_write_path, reject_existing_symlink_ancestors,
    reject_existing_symlinks_in_path, reject_symlink,
};

use super::ArtifactError;
use super::local_io::{
    SnapshotFile, ensure_private_dir, materialize_tree, prepare_empty_internal_dir, read_json,
    revision_dir, storage_key, write_json_atomic,
};
use super::model::{
    ARTIFACT_INTERCHANGE_SCHEMA, ArtifactInterchange, ArtifactLicenseState, ArtifactProvenance,
    ArtifactRecord, ArtifactRevision, JsonMap,
};
use super::validation::{
    self, MAX_RECORD_JSON_BYTES, MAX_REVISION_MANIFEST_BYTES, validate_id, validate_reference_id,
};

/// Inputs for importing a local file or multi-file package.
#[derive(Debug, Clone)]
pub struct ArtifactImportRequest {
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub metadata: JsonMap,
    pub provenance: ArtifactProvenance,
    pub license: ArtifactLicenseState,
    pub authored_at: Option<String>,
    pub message: Option<String>,
}

impl ArtifactImportRequest {
    /// Start a local import request with conservative provenance/license defaults.
    #[must_use]
    pub fn new(
        kind: impl Into<String>,
        namespace: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            namespace: namespace.into(),
            name: name.into(),
            title: None,
            description: None,
            tags: Vec::new(),
            metadata: JsonMap::new(),
            provenance: ArtifactProvenance::default(),
            license: ArtifactLicenseState::default(),
            authored_at: None,
            message: None,
        }
    }
}

/// Inputs for a local fork. The source revision is always pinned exactly.
#[derive(Debug, Clone)]
pub struct ArtifactForkRequest {
    pub source_artifact_id: String,
    pub namespace: String,
    pub name: String,
    pub title: Option<String>,
    pub following: bool,
    pub forked_at: Option<String>,
}

/// Safe local export policy.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArtifactExportOptions {
    /// Explicitly allow export of text that matches Labby's secret detector.
    pub include_secrets: bool,
    /// Allow overwriting Artifact-owned destination paths in a non-empty directory.
    pub force: bool,
}

/// Explicit-root local Artifact store.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    pub(crate) root: PathBuf,
}

impl ArtifactStore {
    /// Open or initialize the canonical local store at an explicit root.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ArtifactError> {
        let root = root.as_ref();
        if !root.is_absolute() {
            return Err(ArtifactError::UnsafePath("store_root_relative"));
        }
        validate_store_creation_ancestor(root)?;
        reject_existing_symlinks_in_path(root)
            .map_err(|_| ArtifactError::UnsafePath("store_symlink"))?;
        ensure_private_dir(root)?;
        let root = canonicalize_and_reject_write_path(root)
            .map_err(|_| ArtifactError::UnsafePath("store_root"))?;
        ensure_private_dir(&root.join("artifacts"))?;
        ensure_private_dir(&root.join("locks"))?;
        Ok(Self { root })
    }

    /// Canonical store root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Read one local Artifact head record.
    pub fn get(&self, artifact_id: &str) -> Result<ArtifactRecord, ArtifactError> {
        validate_id(artifact_id, "artifact_id")?;
        self.read_record_optional(artifact_id)?
            .ok_or(ArtifactError::NotFound("record"))
    }

    /// Read and verify an immutable revision.
    pub fn revision(
        &self,
        artifact_id: &str,
        revision_id: &str,
    ) -> Result<ArtifactRevision, ArtifactError> {
        validate_id(artifact_id, "artifact_id")?;
        validate_reference_id(revision_id, "revision_id")?;
        self.read_revision(artifact_id, revision_id)
    }

    /// Project one exact local revision into the frozen cross-product envelope.
    pub fn interchange(
        &self,
        artifact_id: &str,
        revision_id: Option<&str>,
    ) -> Result<ArtifactInterchange, ArtifactError> {
        let record = self.get(artifact_id)?;
        let revision_id = revision_id.unwrap_or(&record.current_revision_id);
        let revision = self.read_revision(artifact_id, revision_id)?;
        let interchange = ArtifactInterchange {
            schema_version: ARTIFACT_INTERCHANGE_SCHEMA.to_string(),
            descriptor: record.descriptor,
            revision,
            provenance: record.provenance,
            license: record.license,
            lineage: record.lineage,
            publication: record.publication,
            downloads: Vec::new(),
            materialization_hints: JsonMap::new(),
        };
        interchange.validate()?;
        Ok(interchange)
    }

    /// Path to the editable workspace for an Artifact. No filesystem mutation occurs.
    pub fn workspace_path(&self, artifact_id: &str) -> Result<PathBuf, ArtifactError> {
        Ok(self.artifact_dir(artifact_id)?.join("workspace"))
    }

    pub(crate) fn materialize_workspace(
        &self,
        artifact_id: &str,
        files: &[SnapshotFile],
    ) -> Result<(), ArtifactError> {
        let workspace = self.workspace_path(artifact_id)?;
        prepare_empty_internal_dir(&workspace)?;
        materialize_tree(&workspace, files, false)
    }

    pub(crate) fn persist_revision(
        &self,
        artifact_id: &str,
        revision: &ArtifactRevision,
        files: &[SnapshotFile],
    ) -> Result<(), ArtifactError> {
        revision.verify_content_digest()?;
        let artifact_dir = self.artifact_dir(artifact_id)?;
        let revisions = artifact_dir.join("revisions");
        ensure_private_dir(&revisions)?;
        let final_dir = revision_dir(&artifact_dir, &revision.id);
        reject_existing_symlinks_in_path(&final_dir)
            .map_err(|_| ArtifactError::UnsafePath("stored_symlink"))?;
        let manifest = final_dir.join("revision.json");
        if manifest.exists() {
            reject_symlink(&manifest).map_err(|_| ArtifactError::UnsafePath("stored_symlink"))?;
            let existing: ArtifactRevision = read_json(&manifest, MAX_REVISION_MANIFEST_BYTES)?;
            existing.verify_content_digest()?;
            if existing != *revision {
                return Err(ArtifactError::Conflict("immutable_revision_reuse"));
            }
            return Ok(());
        }
        if final_dir.exists() {
            prepare_empty_internal_dir(&final_dir)?;
            std::fs::remove_dir_all(&final_dir)?;
        }
        let key = storage_key(&revision.id);
        let staging = revisions.join(format!(".{key}.stage-{}", std::process::id()));
        prepare_empty_internal_dir(&staging)?;
        let files_root = staging.join("files");
        ensure_private_dir(&files_root)?;
        materialize_tree(&files_root, files, false)?;
        write_json_atomic(&staging.join("revision.json"), revision)?;
        std::fs::rename(&staging, &final_dir)?;
        Ok(())
    }

    pub(crate) fn read_revision(
        &self,
        artifact_id: &str,
        revision_id: &str,
    ) -> Result<ArtifactRevision, ArtifactError> {
        validate_reference_id(revision_id, "revision_id")?;
        let artifact_dir = self.artifact_dir(artifact_id)?;
        let revision_root = revision_dir(&artifact_dir, revision_id);
        reject_existing_symlinks_in_path(&revision_root)
            .map_err(|_| ArtifactError::UnsafePath("stored_symlink"))?;
        let path = revision_root.join("revision.json");
        if !path.exists() {
            return Err(ArtifactError::NotFound("revision"));
        }
        reject_symlink(&path).map_err(|_| ArtifactError::UnsafePath("stored_symlink"))?;
        let revision: ArtifactRevision = read_json(&path, MAX_REVISION_MANIFEST_BYTES)?;
        validation::validate_revision(&revision)?;
        revision.verify_content_digest()?;
        if revision.id != revision_id {
            return Err(ArtifactError::Conflict("revision_identity_mismatch"));
        }
        Ok(revision)
    }

    pub(crate) fn persist_record(&self, record: &ArtifactRecord) -> Result<(), ArtifactError> {
        record.validate()?;
        let artifact_dir = self.artifact_dir(&record.descriptor.id)?;
        ensure_private_dir(&artifact_dir)?;
        write_json_atomic(&artifact_dir.join("artifact.json"), record)
    }

    pub(crate) fn persist_record_transition(
        &self,
        expected_current_revision_id: &str,
        record: &ArtifactRecord,
    ) -> Result<(), ArtifactError> {
        validate_reference_id(expected_current_revision_id, "expected_current_revision_id")?;
        record.validate()?;
        let current = self
            .read_record_optional(&record.descriptor.id)?
            .ok_or(ArtifactError::NotFound("record"))?;
        if current.current_revision_id != expected_current_revision_id {
            return Err(ArtifactError::Conflict("head_changed"));
        }
        self.persist_record(record)
    }

    pub(crate) fn read_record_optional(
        &self,
        artifact_id: &str,
    ) -> Result<Option<ArtifactRecord>, ArtifactError> {
        let artifact_dir = self.artifact_dir(artifact_id)?;
        let path = artifact_dir.join("artifact.json");
        if !path.exists() {
            return Ok(None);
        }
        reject_symlink(&path).map_err(|_| ArtifactError::UnsafePath("stored_symlink"))?;
        let record: ArtifactRecord = read_json(&path, MAX_RECORD_JSON_BYTES)?;
        record.validate()?;
        if record.descriptor.id != artifact_id {
            return Err(ArtifactError::Conflict("record_identity_mismatch"));
        }
        Ok(Some(record))
    }

    pub(crate) fn artifact_dir(&self, artifact_id: &str) -> Result<PathBuf, ArtifactError> {
        validate_id(artifact_id, "artifact_id")?;
        let path = self.root.join("artifacts").join(storage_key(artifact_id));
        reject_existing_symlinks_in_path(&path)
            .map_err(|_| ArtifactError::UnsafePath("stored_symlink"))?;
        Ok(path)
    }

    pub(crate) fn lock(&self, artifact_id: &str) -> Result<MutationLock, ArtifactError> {
        validate_id(artifact_id, "artifact_id")?;
        let locks_root = self.root.join("locks");
        let path = locks_root.join(format!("{}.lock", storage_key(artifact_id)));
        reject_existing_symlink_ancestors(&locks_root, &path)
            .map_err(|_| ArtifactError::UnsafePath("lock_symlink"))?;
        MutationLock::acquire(&path)
    }
}

fn validate_store_creation_ancestor(root: &Path) -> Result<(), ArtifactError> {
    let mut probe = root;
    while !probe.exists() {
        probe = probe
            .parent()
            .ok_or(ArtifactError::UnsafePath("store_root"))?;
    }
    reject_existing_symlinks_in_path(probe)
        .map_err(|_| ArtifactError::UnsafePath("store_symlink"))?;
    canonicalize_and_reject_write_path(probe)
        .map_err(|_| ArtifactError::UnsafePath("store_root"))?;
    Ok(())
}

pub(crate) struct MutationLock {
    _file: File,
}

impl MutationLock {
    fn acquire(path: &Path) -> Result<Self, ArtifactError> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(ArtifactError::Busy),
            Err(std::fs::TryLockError::Error(error)) => Err(ArtifactError::Io(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn store_requires_an_explicit_absolute_root() {
        let error = ArtifactStore::new("relative-artifacts").unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::UnsafePath("store_root_relative")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn store_root_and_lock_file_are_private() {
        use std::os::unix::fs::PermissionsExt as _;

        let data = tempdir().unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        assert_eq!(
            std::fs::metadata(store.root())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        let _lock = store.lock("art_test").unwrap();
        let lock_path = store
            .root()
            .join("locks")
            .join(format!("{}.lock", storage_key("art_test")));
        assert_eq!(
            std::fs::metadata(lock_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn stale_head_transition_is_rejected() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        std::fs::write(source.path().join("a.txt"), b"alpha").unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let first = store
            .import_local(
                ArtifactImportRequest::new("resource", "labby", "cas-demo"),
                source.path(),
            )
            .unwrap();

        std::fs::write(source.path().join("a.txt"), b"beta").unwrap();
        let second = store
            .import_local(
                ArtifactImportRequest::new("resource", "labby", "cas-demo"),
                source.path(),
            )
            .unwrap();
        assert_ne!(first.current_revision_id, second.current_revision_id);

        let error = store
            .persist_record_transition(&first.current_revision_id, &second)
            .unwrap_err();
        assert!(matches!(error, ArtifactError::Conflict("head_changed")));
    }

    #[cfg(unix)]
    #[test]
    fn store_rejects_symlink_substitution_inside_hashed_artifact_root() {
        use std::os::unix::fs::symlink;

        let data = tempdir().unwrap();
        let outside = data.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let artifact_path = store.root().join("artifacts").join(storage_key("art_test"));
        symlink(&outside, &artifact_path).unwrap();
        assert!(matches!(
            store.artifact_dir("art_test"),
            Err(ArtifactError::UnsafePath("stored_symlink"))
        ));
    }
}
