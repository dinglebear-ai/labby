//! Filesystem storage provider for stash.
//!
//! Copies revision content between the local stash store and another directory
//! on the same host (the "remote root"). This is useful for syncing stash
//! revisions to a NAS mount, a shared directory, or another path on the same
//! machine.
//!
//! # Configuration
//!
//! The provider record's `config` object must contain `"root"`: an absolute
//! path to the remote directory root.

use std::path::{Path, PathBuf};

use lab_apis::stash::types::{StashProviderRecord, StashRevision};

use crate::dispatch::error::ToolError;
use crate::dispatch::stash::provider::StashProvider;
use crate::dispatch::stash::revision::{compute_digest, walk_files_sorted};
use crate::dispatch::stash::store::StashStore;

/// Filesystem-backed storage provider.
///
/// Stores revision content under `<root>/<component_id>/<rev_id>/`.
pub struct FilesystemProvider {
    /// Absolute path to the remote filesystem root.
    root: PathBuf,
}

// Manual Debug impl — `root` is not a secret but we follow the convention
// of explicit Debug impls for all provider types to make future credential
// fields easier to redact.
impl std::fmt::Debug for FilesystemProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesystemProvider")
            .field("root", &self.root)
            .finish()
    }
}

impl FilesystemProvider {
    /// Construct a `FilesystemProvider` from a provider record.
    ///
    /// Expects `record.config["root"]` to be a non-empty string.
    pub fn from_record(record: &StashProviderRecord) -> Result<Self, ToolError> {
        let root = record
            .config
            .get("root")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| ToolError::InvalidParam {
                param: "config.root".into(),
                message: "filesystem provider requires config.root (non-empty string path)".into(),
            })?;
        if !root.is_absolute() {
            return Err(ToolError::InvalidParam {
                param: "config.root".into(),
                message: "filesystem provider config.root must be an absolute path".into(),
            });
        }
        Ok(Self { root })
    }

    /// Return the directory used to store all revisions for a component.
    fn component_remote_dir(&self, component_id: &str) -> PathBuf {
        self.root.join(component_id)
    }

    /// Return the directory for a specific revision.
    fn revision_remote_dir(&self, component_id: &str, rev_id: &str) -> PathBuf {
        self.component_remote_dir(component_id).join(rev_id)
    }
}

impl StashProvider for FilesystemProvider {
    fn kind(&self) -> &'static str {
        "filesystem"
    }

    fn push_revision(
        &self,
        store: &StashStore,
        component_id: &str,
        rev: &StashRevision,
    ) -> Result<(), ToolError> {
        let src = store.revision_files_path(&rev.id);
        let dst = self.revision_remote_dir(component_id, &rev.id);

        std::fs::create_dir_all(&dst).map_err(|e| ToolError::Sdk {
            sdk_kind: "sync_failed".into(),
            message: format!("create remote revision dir `{}`: {e}", dst.display()),
        })?;

        copy_dir_recursive(&src, &dst)
    }

    fn pull_latest(
        &self,
        store: &StashStore,
        component_id: &str,
    ) -> Result<Option<StashRevision>, ToolError> {
        let remote_dir = self.component_remote_dir(component_id);
        if !remote_dir.exists() {
            return Ok(None);
        }

        // Collect valid remote revision IDs (directory names). Ignore stray
        // directories so an arbitrary name cannot become "latest".
        let remote_ids = list_valid_revision_directory_names(&remote_dir)?;
        if remote_ids.is_empty() {
            return Ok(None);
        }

        // Use the lexicographically last ID (ULIDs sort chronologically).
        let latest_id = remote_ids
            .into_iter()
            .max()
            .expect("non-empty vec has a max");

        let remote_rev_dir = remote_dir.join(&latest_id);

        // Generate a fresh revision ID for the pulled content.
        let new_rev_id = ulid::Ulid::new().to_string().to_lowercase();
        let dst = store.revision_files_path(&new_rev_id);

        std::fs::create_dir_all(&dst).map_err(|e| ToolError::Sdk {
            sdk_kind: "sync_failed".into(),
            message: format!("create local revision dir `{}`: {e}", dst.display()),
        })?;

        copy_dir_recursive(&remote_rev_dir, &dst)?;

        // Walk and compute digest over the pulled files.
        let file_entries = walk_files_sorted(&dst)?;
        let file_count = file_entries.len();
        let content_digest = compute_digest(&file_entries)?;
        let now = jiff::Timestamp::now().to_string();

        // Return the revision WITHOUT writing meta to the store.
        // lab-qytb: write_revision_meta calls append_revision_to_index which is
        // a read-modify-write on the index file. That write must happen inside
        // the component advisory lock in service.rs to prevent races with
        // concurrent component.save operations. The caller is responsible for
        // calling store.write_revision_meta(&rev) under the lock.
        let rev = StashRevision {
            id: new_rev_id.clone(),
            component_id: component_id.to_string(),
            label: Some(format!("pulled from {latest_id}")),
            content_digest,
            created_at: now,
            file_count,
            unix_mode: None,
        };

        Ok(Some(rev))
    }

    fn list_remote(&self, component_id: &str) -> Result<Vec<String>, ToolError> {
        let dir = self.component_remote_dir(component_id);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        list_valid_revision_directory_names(&dir)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Recursively copy `src/` into `dst/`.
///
/// Symlinks are silently skipped to prevent traversal outside the revision
/// directory boundary (lab-qz6a.22).
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), ToolError> {
    let src_meta = src.symlink_metadata().map_err(|e| ToolError::Sdk {
        sdk_kind: "sync_failed".into(),
        message: format!("symlink_metadata `{}`: {e}", src.display()),
    })?;
    if !src_meta.file_type().is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(src).map_err(|e| ToolError::Sdk {
        sdk_kind: "sync_failed".into(),
        message: format!("read_dir `{}`: {e}", src.display()),
    })? {
        let entry = entry.map_err(|e| ToolError::Sdk {
            sdk_kind: "sync_failed".into(),
            message: format!("read_dir entry: {e}"),
        })?;
        let src_path = entry.path();
        let rel = entry.file_name();
        let dst_path = dst.join(&rel);

        let meta = src_path.symlink_metadata().map_err(|e| ToolError::Sdk {
            sdk_kind: "sync_failed".into(),
            message: format!("symlink_metadata `{}`: {e}", src_path.display()),
        })?;

        if meta.file_type().is_symlink() {
            // Reject symlinks — prevents traversal outside the revision store boundary.
            return Err(ToolError::Sdk {
                sdk_kind: "symlink_rejected".into(),
                message: format!(
                    "symlink `{}` rejected during recursive copy",
                    src_path.display()
                ),
            });
        }

        if meta.file_type().is_dir() {
            std::fs::create_dir_all(&dst_path).map_err(|e| ToolError::Sdk {
                sdk_kind: "sync_failed".into(),
                message: format!("create dir `{}`: {e}", dst_path.display()),
            })?;
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).map_err(|e| ToolError::Sdk {
                sdk_kind: "sync_failed".into(),
                message: format!(
                    "copy `{}` → `{}`: {e}",
                    src_path.display(),
                    dst_path.display()
                ),
            })?;
        }
    }
    Ok(())
}

/// List names of subdirectories under `dir`.
fn list_subdirectory_names(dir: &Path) -> Result<Vec<String>, ToolError> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| ToolError::Sdk {
        sdk_kind: "sync_failed".into(),
        message: format!("read_dir `{}`: {e}", dir.display()),
    })? {
        let entry = entry.map_err(|e| ToolError::Sdk {
            sdk_kind: "sync_failed".into(),
            message: format!("read_dir entry: {e}"),
        })?;
        if entry.path().is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
    }
    Ok(names)
}

fn list_valid_revision_directory_names(dir: &Path) -> Result<Vec<String>, ToolError> {
    let mut names: Vec<String> = list_subdirectory_names(dir)?
        .into_iter()
        .filter(|name| is_valid_revision_id(name))
        .collect();
    names.sort();
    Ok(names)
}

fn is_valid_revision_id(name: &str) -> bool {
    name.parse::<ulid::Ulid>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn list_valid_revision_directory_names_ignores_stray_dirs() {
        let dir = tempdir().unwrap();
        let valid_old = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let valid_new = "01BRZ3NDEKTSV4RRFFQ69G5FAV";
        for name in [valid_new, "zzzz-not-a-ulid", "notes", valid_old] {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
        }
        std::fs::write(dir.path().join("01CRZ3NDEKTSV4RRFFQ69G5FAV"), b"not dir").unwrap();

        let names = list_valid_revision_directory_names(dir.path()).unwrap();
        assert_eq!(names, vec![valid_old.to_string(), valid_new.to_string()]);
    }
}
