//! Canonical local Artifact store and persistence primitives.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use atomic_write_file::AtomicWriteFile;

use crate::path_safety::{
    canonicalize_and_reject_write_path, reject_existing_symlink_ancestors,
    reject_existing_symlinks_in_path, reject_symlink,
};

use super::ArtifactError;
use super::library::{LibrarySnapshot, MAX_LIBRARY_STATE_BYTES};
use super::local_io::{
    SnapshotFile, ensure_private_dir, materialize_tree, normalize_verified_macos_var_alias,
    prepare_empty_internal_dir, read_json, revision_dir, storage_key, write_json_atomic,
};
use super::model::{
    ARTIFACT_INTERCHANGE_SCHEMA, ArtifactInterchange, ArtifactLicenseState, ArtifactProvenance,
    ArtifactRecord, ArtifactRevision, JsonMap,
};
use super::validation::{
    self, MAX_RECORD_JSON_BYTES, MAX_REVISION_MANIFEST_BYTES, validate_id, validate_reference_id,
};

/// Maximum immutable revision manifests accepted by one bounded read batch.
pub const MAX_REVISION_READ_BATCH: usize = 100;
static NEXT_WORKSPACE_STAGING_ID: AtomicU64 = AtomicU64::new(1);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "fault stages are exercised by deterministic persistence tests"
)]
pub(crate) enum LibraryPersistFault {
    Write,
    FileSync,
    Commit,
    DirectorySync,
    Enospc,
}

#[cfg(test)]
fn library_faults()
-> &'static std::sync::Mutex<std::collections::BTreeMap<PathBuf, LibraryPersistFault>> {
    static FAULTS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::BTreeMap<PathBuf, LibraryPersistFault>>,
    > = std::sync::OnceLock::new();
    FAULTS.get_or_init(Default::default)
}

impl ArtifactStore {
    /// Open or initialize the canonical local store at an explicit root.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ArtifactError> {
        let root = root.as_ref();
        if !root.is_absolute() {
            return Err(ArtifactError::UnsafePath("store_root_relative"));
        }
        let root = normalize_verified_macos_var_alias(root)
            .map_err(|_| ArtifactError::UnsafePath("store_symlink"))?;
        let root = root.as_path();
        validate_store_creation_ancestor(root)?;
        reject_existing_symlinks_in_path(root)
            .map_err(|_| ArtifactError::UnsafePath("store_symlink"))?;
        ensure_private_dir(root)?;
        let root = canonicalize_and_reject_write_path(root)
            .map_err(|_| ArtifactError::UnsafePath("store_root"))?;
        ensure_private_dir(&root.join("artifacts"))?;
        ensure_private_dir(&root.join("locks"))?;
        ensure_private_dir(&root.join("library"))?;
        Ok(Self { root })
    }

    /// Canonical store root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[cfg(test)]
    pub(crate) fn inject_library_persist_fault(&self, fault: LibraryPersistFault) {
        library_faults()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(self.root.clone(), fault);
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

    /// Read and verify a bounded batch of immutable revisions in input order.
    ///
    /// Reads are deliberately sequential. Callers admit this blocking operation through their
    /// shared bounded executor, so spawning another per-request worker set here would bypass that
    /// process-wide capacity limit.
    pub fn revision_batch(
        &self,
        artifact_id: &str,
        revision_ids: &[&str],
    ) -> Result<Vec<ArtifactRevision>, ArtifactError> {
        validate_id(artifact_id, "artifact_id")?;
        if revision_ids.len() > MAX_REVISION_READ_BATCH {
            return Err(ArtifactError::LimitExceeded {
                what: "revision_batch",
                limit: MAX_REVISION_READ_BATCH as u64,
            });
        }
        for revision_id in revision_ids {
            validate_reference_id(revision_id, "revision_id")?;
        }
        revision_ids
            .iter()
            .map(|revision_id| self.read_revision(artifact_id, revision_id))
            .collect()
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
        let artifact_dir = workspace
            .parent()
            .ok_or(ArtifactError::UnsafePath("workspace_parent"))?;
        let staging_id = NEXT_WORKSPACE_STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let staging = artifact_dir.join(format!(
            ".workspace.stage-{}-{staging_id}",
            std::process::id()
        ));
        let previous = artifact_dir.join(format!(
            ".workspace.previous-{}-{staging_id}",
            std::process::id()
        ));

        prepare_empty_internal_dir(&staging)?;
        if let Err(error) = materialize_tree(&staging, files, false).and_then(|()| {
            File::open(&staging)
                .map_err(ArtifactError::from)?
                .sync_all()
                .map_err(ArtifactError::from)
        }) {
            cleanup_recovery_dir(&staging, artifact_id, "staging");
            return Err(error);
        }

        if workspace.exists()
            && let Err(error) =
                reject_symlink(&workspace).map_err(|_| ArtifactError::UnsafePath("stored_symlink"))
        {
            cleanup_recovery_dir(&staging, artifact_id, "staging");
            return Err(error);
        }
        // A promotion failure has already rolled back internally, so the fully
        // materialized staging tree is now unreferenced. Without this it is left in
        // the artifact directory with nothing to reap it, and repeated failures
        // accumulate full copies of the workspace on disk.
        let promotion = match promote_workspace(&staging, &workspace, &previous) {
            Ok(promotion) => promotion,
            Err(error) => {
                cleanup_recovery_dir(&staging, artifact_id, "staging");
                return Err(error);
            }
        };
        if let Err(sync_error) = File::open(artifact_dir).and_then(|dir| dir.sync_all()) {
            rollback_promoted_workspace(&workspace, &previous, &staging, promotion).map_err(
                |rollback_error| {
                    ArtifactError::Io(std::io::Error::new(
                        rollback_error.kind(),
                        format!(
                            "workspace parent sync failed ({sync_error}); rollback failed ({rollback_error}); recovery trees retained at {} and {}",
                            staging.display(),
                            previous.display()
                        ),
                    ))
                },
            )?;
            if let Err(resync_error) = File::open(artifact_dir).and_then(|dir| dir.sync_all()) {
                return Err(ArtifactError::Io(std::io::Error::new(
                    resync_error.kind(),
                    format!(
                        "workspace parent sync failed ({sync_error}); rollback succeeded but its parent sync failed ({resync_error}); recovery tree retained at {}",
                        staging.display()
                    ),
                )));
            }
            cleanup_recovery_dir(&staging, artifact_id, "rolled-back staging");
            return Err(ArtifactError::Io(sync_error));
        }
        if promotion == WorkspacePromotion::Replaced {
            cleanup_recovery_dir(&previous, artifact_id, "previous workspace");
        }
        Ok(())
    }

    pub(crate) fn persist_revision(
        &self,
        artifact_id: &str,
        revision: &ArtifactRevision,
        files: &[SnapshotFile],
    ) -> Result<(), ArtifactError> {
        self.persist_revision_with_faults(revision, artifact_id, files, &mut |_| Ok(()))
    }

    pub(crate) fn persist_revision_with_faults(
        &self,
        revision: &ArtifactRevision,
        artifact_id: &str,
        files: &[SnapshotFile],
        fault: &mut impl FnMut(super::library::SkillTransactionBoundary) -> Result<(), ArtifactError>,
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
        fault(super::library::SkillTransactionBoundary::PromotionWrite)?;
        materialize_tree(&files_root, files, false)?;
        write_json_atomic(&staging.join("revision.json"), revision)?;
        fault(super::library::SkillTransactionBoundary::PromotionFileSync)?;
        File::open(&staging)?.sync_all()?;
        fault(super::library::SkillTransactionBoundary::PromotionRename)?;
        std::fs::rename(&staging, &final_dir)?;
        fault(super::library::SkillTransactionBoundary::PromotionParentSync)?;
        File::open(&revisions)?.sync_all()?;
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

    pub(crate) fn persist_record_with_faults(
        &self,
        record: &ArtifactRecord,
        boundaries: [super::library::SkillTransactionBoundary; 4],
        fault: &mut impl FnMut(super::library::SkillTransactionBoundary) -> Result<(), ArtifactError>,
    ) -> Result<(), ArtifactError> {
        record.validate()?;
        let artifact_dir = self.artifact_dir(&record.descriptor.id)?;
        ensure_private_dir(&artifact_dir)?;
        super::local_io::write_json_atomic_with_faults(
            &artifact_dir.join("artifact.json"),
            record,
            boundaries,
            fault,
        )
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

    #[allow(
        dead_code,
        reason = "used by the sealed Skill Library mutation primitive"
    )]
    pub(crate) fn library_lock(&self) -> Result<MutationLock, ArtifactError> {
        let locks_root = self.root.join("locks");
        let path = locks_root.join("skill-library.lock");
        reject_existing_symlink_ancestors(&locks_root, &path)
            .map_err(|_| ArtifactError::UnsafePath("lock_symlink"))?;
        MutationLock::acquire_wait(&path)
    }

    pub(crate) fn read_library_snapshot(&self) -> Result<LibrarySnapshot, ArtifactError> {
        let state = self.read_library_snapshot_unvalidated()?;
        state.validate(self)?;
        Ok(state)
    }

    pub(crate) fn read_library_snapshot_unvalidated(
        &self,
    ) -> Result<LibrarySnapshot, ArtifactError> {
        let path = self.root.join("library").join("state.json");
        if !path.exists() {
            return Ok(LibrarySnapshot::default());
        }
        reject_symlink(&path).map_err(|_| ArtifactError::UnsafePath("stored_symlink"))?;
        let state: LibrarySnapshot =
            read_json(&path, MAX_LIBRARY_STATE_BYTES).map_err(|error| match error {
                ArtifactError::Json(_) => ArtifactError::LibraryCorrupt("invalid_json"),
                other => other,
            })?;
        let bytes = super::canonical_json::to_canonical_vec(&state)?;
        if bytes.len() as u64 > MAX_LIBRARY_STATE_BYTES {
            return Err(ArtifactError::LimitExceeded {
                what: "library_state_bytes",
                limit: MAX_LIBRARY_STATE_BYTES,
            });
        }
        Ok(state)
    }

    #[allow(
        dead_code,
        reason = "used by the sealed Skill Library mutation primitive"
    )]
    pub(crate) fn persist_library_snapshot(
        &self,
        state: &LibrarySnapshot,
    ) -> Result<(), ArtifactError> {
        self.persist_library_snapshot_with_faults(state, &mut |_| Ok(()))
    }

    pub(crate) fn persist_library_snapshot_with_faults(
        &self,
        state: &LibrarySnapshot,
        fault: &mut impl FnMut(super::library::SkillTransactionBoundary) -> Result<(), ArtifactError>,
    ) -> Result<(), ArtifactError> {
        state.validate_metadata()?;
        let library_root = self.root.join("library");
        reject_existing_symlinks_in_path(&library_root)
            .map_err(|_| ArtifactError::UnsafePath("stored_symlink"))?;
        let path = library_root.join("state.json");
        let bytes = super::canonical_json::to_canonical_vec(state)?;
        let mut output = AtomicWriteFile::open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            output
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        self.fail_library_persist_if_injected(LibraryPersistFault::Enospc)?;
        self.fail_library_persist_if_injected(LibraryPersistFault::Write)?;
        fault(super::library::SkillTransactionBoundary::LibraryWrite)?;
        output.write_all(&bytes)?;
        self.fail_library_persist_if_injected(LibraryPersistFault::FileSync)?;
        fault(super::library::SkillTransactionBoundary::LibraryFileSync)?;
        output.sync_all()?;
        self.fail_library_persist_if_injected(LibraryPersistFault::Commit)?;
        fault(super::library::SkillTransactionBoundary::LibraryRename)?;
        output.commit()?;
        self.fail_library_persist_if_injected(LibraryPersistFault::DirectorySync)?;
        fault(super::library::SkillTransactionBoundary::LibraryParentSync)?;
        File::open(&library_root)?.sync_all()?;
        Ok(())
    }

    fn fail_library_persist_if_injected(
        &self,
        stage: LibraryPersistFault,
    ) -> Result<(), ArtifactError> {
        #[cfg(test)]
        {
            let mut faults = library_faults()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if faults.get(&self.root) == Some(&stage) {
                faults.remove(&self.root);
                return Err(ArtifactError::Io(std::io::Error::other(format!(
                    "injected library persistence failure at {stage:?}"
                ))));
            }
        }
        let _ = stage;
        Ok(())
    }
}

fn promote_workspace(
    staging: &Path,
    workspace: &Path,
    previous: &Path,
) -> Result<WorkspacePromotion, ArtifactError> {
    promote_workspace_with(staging, workspace, previous, &mut |from, to| {
        std::fs::rename(from, to)
    })
}

fn promote_workspace_with(
    staging: &Path,
    workspace: &Path,
    previous: &Path,
    rename: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<WorkspacePromotion, ArtifactError> {
    let had_workspace = workspace.exists();
    if had_workspace {
        rename(workspace, previous)?;
    }
    if let Err(promotion_error) = rename(staging, workspace) {
        if had_workspace {
            rename(previous, workspace).map_err(|rollback_error| {
                ArtifactError::Io(std::io::Error::new(
                    rollback_error.kind(),
                    format!(
                        "workspace promotion failed ({promotion_error}); rollback failed ({rollback_error})"
                    ),
                ))
            })?;
        }
        return Err(ArtifactError::Io(promotion_error));
    }
    Ok(if had_workspace {
        WorkspacePromotion::Replaced
    } else {
        WorkspacePromotion::Created
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspacePromotion {
    Created,
    Replaced,
}

fn rollback_promoted_workspace(
    workspace: &Path,
    previous: &Path,
    staging: &Path,
    promotion: WorkspacePromotion,
) -> std::io::Result<()> {
    rollback_promoted_workspace_with(workspace, previous, staging, promotion, &mut |from, to| {
        std::fs::rename(from, to)
    })
}

/// Rollback with an injectable rename, so its recovery arms are reachable from
/// tests. The production path only reaches this after a parent-directory
/// `sync_all` failure, which a test cannot provoke.
fn rollback_promoted_workspace_with(
    workspace: &Path,
    previous: &Path,
    staging: &Path,
    promotion: WorkspacePromotion,
    rename: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    rename(workspace, staging)?;
    if promotion == WorkspacePromotion::Replaced {
        rename(previous, workspace).map_err(|error| {
            let restore_new_error = rename(staging, workspace).err();
            std::io::Error::new(
                error.kind(),
                match restore_new_error {
                    Some(restore_error) => format!(
                        "restoring previous workspace failed ({error}); restoring promoted workspace also failed ({restore_error})"
                    ),
                    None => format!("restoring previous workspace failed ({error})"),
                },
            )
        })?;
    }
    Ok(())
}

fn cleanup_recovery_dir(path: &Path, artifact_id: &str, kind: &str) {
    if let Err(error) = std::fs::remove_dir_all(path) {
        tracing::warn!(
            artifact_id,
            recovery_path = %path.display(),
            error = %error,
            "failed to remove {kind}; recovery tree retained"
        );
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

    fn acquire_wait(path: &Path) -> Result<Self, ArtifactError> {
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
        file.lock()?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn revision_batch_rejects_more_than_the_page_cap() {
        let data = tempdir().unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let ids = vec!["rev_0000000000000000"; MAX_REVISION_READ_BATCH + 1];
        assert!(matches!(
            store.revision_batch("art_batch", &ids),
            Err(ArtifactError::LimitExceeded {
                what: "revision_batch",
                limit: 100
            })
        ));
    }

    #[test]
    fn store_requires_an_explicit_absolute_root() {
        let error = ArtifactStore::new("relative-artifacts").unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::UnsafePath("store_root_relative")
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn store_accepts_private_root_below_verified_macos_var_alias() {
        let data = tempdir().unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();

        assert!(store.root().starts_with("/private/var"));
    }

    #[cfg(unix)]
    #[test]
    fn store_rejects_arbitrary_symlinked_ancestor() {
        use std::os::unix::fs::symlink;

        let data = tempdir().unwrap();
        let actual = data.path().join("actual");
        std::fs::create_dir(&actual).unwrap();
        let alias = data.path().join("alias");
        symlink(&actual, &alias).unwrap();

        assert!(matches!(
            ArtifactStore::new(alias.join("store")),
            Err(ArtifactError::UnsafePath("store_symlink"))
        ));
        assert!(!actual.join("store").exists());
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

    #[test]
    fn failed_workspace_materialization_preserves_the_previous_tree() {
        let data = tempdir().unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let artifact_id = "art_workspace";
        let workspace = store.workspace_path(artifact_id).unwrap();
        ensure_private_dir(&workspace).unwrap();
        std::fs::write(workspace.join("preserved.txt"), b"preserved").unwrap();

        let duplicate = SnapshotFile {
            path: "duplicate.txt".to_string(),
            bytes: b"replacement".to_vec(),
            unix_mode: None,
        };
        assert!(
            store
                .materialize_workspace(artifact_id, &[duplicate.clone(), duplicate])
                .is_err()
        );
        assert_eq!(
            std::fs::read(workspace.join("preserved.txt")).unwrap(),
            b"preserved"
        );
    }

    #[test]
    fn failed_workspace_promotion_restores_the_previous_tree() {
        let data = tempdir().unwrap();
        let workspace = data.path().join("workspace");
        let staging = data.path().join("staging");
        let previous = data.path().join("previous");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(workspace.join("preserved.txt"), b"preserved").unwrap();
        std::fs::write(staging.join("replacement.txt"), b"replacement").unwrap();
        let mut rename_count = 0;
        let error = promote_workspace_with(&staging, &workspace, &previous, &mut |from, to| {
            rename_count += 1;
            if rename_count == 2 {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected promotion failure",
                ))
            } else {
                std::fs::rename(from, to)
            }
        })
        .unwrap_err();
        assert!(matches!(error, ArtifactError::Io(_)));
        assert_eq!(
            std::fs::read(workspace.join("preserved.txt")).unwrap(),
            b"preserved"
        );
        assert!(staging.join("replacement.txt").exists());
        assert!(!previous.exists());
    }

    #[test]
    fn rollback_restores_the_previous_tree_and_parks_the_promoted_one() {
        // The production caller only reaches rollback after a parent-directory
        // `sync_all` failure, which a test cannot provoke — hence the injectable
        // rename. Without coverage this recovery path ships unexercised.
        let data = tempdir().unwrap();
        let workspace = data.path().join("workspace");
        let staging = data.path().join("staging");
        let previous = data.path().join("previous");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&previous).unwrap();
        std::fs::write(workspace.join("promoted.txt"), b"promoted").unwrap();
        std::fs::write(previous.join("preserved.txt"), b"preserved").unwrap();

        rollback_promoted_workspace_with(
            &workspace,
            &previous,
            &staging,
            WorkspacePromotion::Replaced,
            &mut |from, to| std::fs::rename(from, to),
        )
        .expect("rollback succeeds");

        assert_eq!(
            std::fs::read(workspace.join("preserved.txt")).unwrap(),
            b"preserved",
            "the previous tree must be back in place"
        );
        assert!(
            staging.join("promoted.txt").exists(),
            "the promoted tree must be parked in staging, not destroyed"
        );
        assert!(!previous.exists());
    }

    #[test]
    fn a_rollback_that_cannot_restore_the_previous_tree_puts_the_promoted_one_back() {
        // Worst case: the previous tree cannot be restored. Rather than leaving
        // no workspace at all, the promoted tree is put back and the error names
        // both failures.
        let data = tempdir().unwrap();
        let workspace = data.path().join("workspace");
        let staging = data.path().join("staging");
        let previous = data.path().join("previous");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&previous).unwrap();
        std::fs::write(workspace.join("promoted.txt"), b"promoted").unwrap();

        let mut rename_count = 0;
        let error = rollback_promoted_workspace_with(
            &workspace,
            &previous,
            &staging,
            WorkspacePromotion::Replaced,
            &mut |from, to| {
                rename_count += 1;
                if rename_count == 2 {
                    Err(std::io::Error::other("injected restore failure"))
                } else {
                    std::fs::rename(from, to)
                }
            },
        )
        .expect_err("an unrestorable previous tree must surface as an error");

        assert!(
            error
                .to_string()
                .contains("restoring previous workspace failed"),
            "the error must name the restore failure, got: {error}"
        );
        assert!(
            workspace.join("promoted.txt").exists(),
            "the promoted tree must be put back rather than leaving no workspace"
        );
    }

    #[test]
    fn a_successful_materialization_leaves_no_recovery_trees_behind() {
        // A leaked `.workspace.stage-*` / `.workspace.previous-*` is a full copy
        // of a possibly-private workspace with nothing to reap it.
        let data = tempdir().unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let artifact_id = "artifact-recovery-cleanup";
        let file = SnapshotFile {
            path: "only.txt".into(),
            bytes: b"first".to_vec(),
            unix_mode: None,
        };
        store.materialize_workspace(artifact_id, &[file]).unwrap();
        let replacement = SnapshotFile {
            path: "only.txt".into(),
            bytes: b"second".to_vec(),
            unix_mode: None,
        };
        store
            .materialize_workspace(artifact_id, &[replacement])
            .unwrap();

        let workspace = store.workspace_path(artifact_id).unwrap();
        let artifact_dir = workspace.parent().unwrap();
        let leaked: Vec<String> = std::fs::read_dir(artifact_dir)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| {
                name.starts_with(".workspace.stage-") || name.starts_with(".workspace.previous-")
            })
            .collect();
        assert!(
            leaked.is_empty(),
            "a successful promotion must reap its recovery trees, found: {leaked:?}"
        );
    }

    #[test]
    fn failed_workspace_rollback_reports_the_recovery_failure() {
        let data = tempdir().unwrap();
        let workspace = data.path().join("workspace");
        let staging = data.path().join("staging");
        let previous = data.path().join("previous");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&staging).unwrap();
        let mut rename_count = 0;
        let error = promote_workspace_with(&staging, &workspace, &previous, &mut |from, to| {
            rename_count += 1;
            match rename_count {
                2 => Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected promotion failure",
                )),
                3 => Err(std::io::Error::other("injected rollback failure")),
                _ => std::fs::rename(from, to),
            }
        })
        .unwrap_err();
        assert!(error.to_string().contains("rollback failed"));
        assert!(previous.exists());
        assert!(staging.exists());
        assert!(!workspace.exists());
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
