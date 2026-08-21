//! Local filesystem mechanics for the Artifact store.

use atomic_write_file::AtomicWriteFile;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use crate::path_safety::{
    canonicalize_and_reject_read_path, canonicalize_and_reject_write_path,
    reject_existing_symlink_ancestors, reject_existing_symlinks_in_path, reject_symlink,
    rel_to_unix_string,
};
use crate::redact::redact_secret_like_segments;

use super::canonical_json;
use super::model::ArtifactComponent;
use super::validation::{
    MAX_COMPONENTS, MAX_DIRECTORY_DEPTH, MAX_DIRECTORY_ENTRIES, MAX_FILE_BYTES, MAX_PACKAGE_BYTES,
    validate_relative_path,
};
use super::{ArtifactError, invalid};

#[derive(Debug, Clone)]
pub(crate) struct SnapshotFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub unix_mode: Option<u32>,
}

pub(crate) fn snapshot_local_path(source: &Path) -> Result<Vec<SnapshotFile>, ArtifactError> {
    reject_existing_symlinks_in_path(source).map_err(|_| ArtifactError::UnsafePath("symlink"))?;
    reject_symlink(source).map_err(|_| ArtifactError::UnsafePath("symlink"))?;
    let root = canonicalize_and_reject_read_path(source)
        .map_err(|_| ArtifactError::UnsafePath("source_root"))?;
    let metadata = std::fs::metadata(&root)?;
    let mut files = Vec::new();
    let mut total = 0_u64;
    let mut entries_seen = 0_usize;

    if metadata.is_file() {
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("path", "non_utf8"))?;
        files.push(read_one(&root, &root, name, &mut total)?);
    } else if metadata.is_dir() {
        collect_directory(&root, &root, 0, &mut entries_seen, &mut files, &mut total)?;
    } else {
        return Err(invalid("source", "not_regular_file_or_directory"));
    }

    if files.is_empty() {
        return Err(invalid("source", "empty_package"));
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn collect_directory(
    root: &Path,
    current: &Path,
    depth: usize,
    entries_seen: &mut usize,
    files: &mut Vec<SnapshotFile>,
    total: &mut u64,
) -> Result<(), ArtifactError> {
    if depth > MAX_DIRECTORY_DEPTH {
        return Err(ArtifactError::LimitExceeded {
            what: "directory_depth",
            limit: MAX_DIRECTORY_DEPTH as u64,
        });
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(current)? {
        *entries_seen = entries_seen
            .checked_add(1)
            .ok_or(ArtifactError::LimitExceeded {
                what: "directory_entries",
                limit: MAX_DIRECTORY_ENTRIES as u64,
            })?;
        if *entries_seen > MAX_DIRECTORY_ENTRIES {
            return Err(ArtifactError::LimitExceeded {
                what: "directory_entries",
                limit: MAX_DIRECTORY_ENTRIES as u64,
            });
        }
        entries.push(entry?);
    }
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(ArtifactError::UnsafePath("symlink"));
        }
        if metadata.is_dir() {
            collect_directory(root, &path, depth + 1, entries_seen, files, total)?;
        } else if metadata.is_file() {
            if files.len() >= MAX_COMPONENTS {
                return Err(ArtifactError::LimitExceeded {
                    what: "component_count",
                    limit: MAX_COMPONENTS as u64,
                });
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| ArtifactError::UnsafePath("source_escape"))?;
            let relative = relative
                .to_str()
                .ok_or_else(|| invalid("path", "non_utf8"))?;
            let relative = rel_to_unix_string(Path::new(relative));
            files.push(read_one(root, &path, &relative, total)?);
        } else {
            return Err(invalid("source", "special_file"));
        }
    }
    Ok(())
}

fn read_one(
    root: &Path,
    path: &Path,
    relative: &str,
    total: &mut u64,
) -> Result<SnapshotFile, ArtifactError> {
    validate_relative_path(relative)?;
    let canonical = std::fs::canonicalize(path)?;
    if canonical != root && !canonical.starts_with(root) {
        return Err(ArtifactError::UnsafePath("source_escape"));
    }
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("source", "not_regular_file"));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(ArtifactError::LimitExceeded {
            what: "file_size",
            limit: MAX_FILE_BYTES,
        });
    }
    let next_total = total
        .checked_add(metadata.len())
        .ok_or(ArtifactError::LimitExceeded {
            what: "package_size",
            limit: MAX_PACKAGE_BYTES,
        })?;
    if next_total > MAX_PACKAGE_BYTES {
        return Err(ArtifactError::LimitExceeded {
            what: "package_size",
            limit: MAX_PACKAGE_BYTES,
        });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    std::io::Read::by_ref(&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
        return Err(ArtifactError::LimitExceeded {
            what: "file_size",
            limit: MAX_FILE_BYTES,
        });
    }
    *total = total
        .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
        .ok_or(ArtifactError::LimitExceeded {
            what: "package_size",
            limit: MAX_PACKAGE_BYTES,
        })?;
    if *total > MAX_PACKAGE_BYTES {
        return Err(ArtifactError::LimitExceeded {
            what: "package_size",
            limit: MAX_PACKAGE_BYTES,
        });
    }

    #[cfg(unix)]
    let unix_mode = {
        use std::os::unix::fs::PermissionsExt as _;
        Some(metadata.permissions().mode() & 0o0755)
    };
    #[cfg(not(unix))]
    let unix_mode = None;

    Ok(SnapshotFile {
        path: relative.to_string(),
        bytes,
        unix_mode,
    })
}

pub(crate) fn materialize_tree(
    root: &Path,
    files: &[SnapshotFile],
    allow_overwrite: bool,
) -> Result<(), ArtifactError> {
    reject_existing_symlinks_in_path(root).map_err(|_| ArtifactError::UnsafePath("symlink"))?;
    if !root.exists() {
        let parent = root
            .parent()
            .ok_or(ArtifactError::UnsafePath("destination_root"))?;
        canonicalize_and_reject_write_path(parent)
            .map_err(|_| ArtifactError::UnsafePath("destination_root"))?;
        std::fs::create_dir_all(root)?;
    }
    canonicalize_and_reject_write_path(root)
        .map_err(|_| ArtifactError::UnsafePath("destination_root"))?;

    for file in files {
        validate_relative_path(&file.path)?;
        let target = root.join(&file.path);
        reject_existing_symlink_ancestors(root, &target)
            .map_err(|_| ArtifactError::UnsafePath("symlink"))?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
            reject_existing_symlink_ancestors(root, parent)
                .map_err(|_| ArtifactError::UnsafePath("symlink"))?;
        }
        let mut options = OpenOptions::new();
        options.write(true).create(true);
        if allow_overwrite {
            options.truncate(true);
        } else {
            options.create_new(true);
        }
        let mut output = options.open(&target)?;
        output.write_all(&file.bytes)?;
        output.sync_all()?;
        apply_safe_mode(&target, file.unix_mode)?;
    }
    Ok(())
}

pub(crate) fn ensure_export_destination(root: &Path, force: bool) -> Result<(), ArtifactError> {
    if root.exists() {
        reject_symlink(root).map_err(|_| ArtifactError::UnsafePath("symlink"))?;
        if !std::fs::metadata(root)?.is_dir() {
            return Err(invalid("destination", "not_directory"));
        }
        if !force && std::fs::read_dir(root)?.next().transpose()?.is_some() {
            return Err(ArtifactError::Conflict("export_target_not_empty"));
        }
    } else {
        let parent = root
            .parent()
            .ok_or(ArtifactError::UnsafePath("destination_root"))?;
        canonicalize_and_reject_write_path(parent)
            .map_err(|_| ArtifactError::UnsafePath("destination_root"))?;
    }
    Ok(())
}

pub(crate) fn blocks_safe_export(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    redact_secret_like_segments(text) != text
}

pub(crate) fn load_revision_files(
    files_root: &Path,
    components: &[ArtifactComponent],
) -> Result<Vec<SnapshotFile>, ArtifactError> {
    reject_existing_symlinks_in_path(files_root)
        .map_err(|_| ArtifactError::UnsafePath("stored_symlink"))?;
    let canonical_root = std::fs::canonicalize(files_root)?;
    let mut files = Vec::with_capacity(components.len());
    let mut total = 0_u64;
    for component in components {
        validate_relative_path(&component.path)?;
        let path = files_root.join(&component.path);
        reject_existing_symlink_ancestors(files_root, &path)
            .map_err(|_| ArtifactError::UnsafePath("stored_symlink"))?;
        reject_symlink(&path).map_err(|_| ArtifactError::UnsafePath("stored_symlink"))?;
        let canonical = std::fs::canonicalize(&path)?;
        if !canonical.starts_with(&canonical_root) {
            return Err(ArtifactError::UnsafePath("stored_escape"));
        }
        let mut file = File::open(&path)?;
        let metadata = file.metadata()?;
        if metadata.len() != component.size || metadata.len() > MAX_FILE_BYTES {
            return Err(ArtifactError::Conflict("stored_file_size_mismatch"));
        }
        total = total
            .checked_add(metadata.len())
            .ok_or(ArtifactError::LimitExceeded {
                what: "package_size",
                limit: MAX_PACKAGE_BYTES,
            })?;
        if total > MAX_PACKAGE_BYTES {
            return Err(ArtifactError::LimitExceeded {
                what: "package_size",
                limit: MAX_PACKAGE_BYTES,
            });
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
        file.read_to_end(&mut bytes)?;
        if canonical_json::sha256_bytes(&bytes) != component.digest {
            return Err(ArtifactError::Conflict("stored_file_digest_mismatch"));
        }
        files.push(SnapshotFile {
            path: component.path.clone(),
            bytes,
            unix_mode: component.unix_mode(),
        });
    }
    Ok(files)
}

pub(crate) fn write_json_atomic<T: Serialize + ?Sized>(
    path: &Path,
    value: &T,
) -> Result<(), ArtifactError> {
    let bytes = canonical_json::to_canonical_vec(value)?;
    write_bytes_atomic(path, &bytes)
}

pub(crate) fn read_json<T: DeserializeOwned>(
    path: &Path,
    max_bytes: u64,
) -> Result<T, ArtifactError> {
    let file = File::open(path)?;
    if file.metadata()?.len() > max_bytes {
        return Err(ArtifactError::LimitExceeded {
            what: "stored_json_bytes",
            limit: max_bytes,
        });
    }
    let reader = file.take(max_bytes.saturating_add(1));
    Ok(serde_json::from_reader(reader)?)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), ArtifactError> {
    let parent = path
        .parent()
        .ok_or(ArtifactError::UnsafePath("store_parent"))?;
    std::fs::create_dir_all(parent)?;
    reject_existing_symlink_ancestors(parent, path)
        .map_err(|_| ArtifactError::UnsafePath("symlink"))?;
    if path.exists() {
        reject_symlink(path).map_err(|_| ArtifactError::UnsafePath("symlink"))?;
    }

    let mut output = AtomicWriteFile::open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        output
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    output.write_all(bytes)?;
    output.sync_all()?;
    output.commit()?;
    Ok(())
}

fn apply_safe_mode(path: &Path, unix_mode: Option<u32>) -> Result<(), ArtifactError> {
    #[cfg(unix)]
    if let Some(mode) = unix_mode {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o0755))?;
    }
    #[cfg(not(unix))]
    let _ = (path, unix_mode);
    Ok(())
}

pub(crate) fn storage_key(value: &str) -> String {
    canonical_json::sha256_bytes(value.as_bytes())
        .strip_prefix("sha256:")
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn ensure_private_dir(path: &Path) -> Result<(), ArtifactError> {
    reject_existing_symlinks_in_path(path).map_err(|_| ArtifactError::UnsafePath("symlink"))?;
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(crate) fn prepare_empty_internal_dir(path: &Path) -> Result<(), ArtifactError> {
    reject_existing_symlinks_in_path(path).map_err(|_| ArtifactError::UnsafePath("symlink"))?;
    if path.exists() {
        reject_symlink(path).map_err(|_| ArtifactError::UnsafePath("symlink"))?;
        std::fs::remove_dir_all(path)?;
    }
    ensure_private_dir(path)
}

pub(crate) fn revision_dir(artifact_dir: &Path, revision_id: &str) -> PathBuf {
    artifact_dir
        .join("revisions")
        .join(storage_key(revision_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn bounded_json_read_rejects_oversized_internal_state() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("state.json");
        std::fs::write(&path, format!("{{\"value\":\"{}\"}}", "x".repeat(64))).unwrap();
        let error = read_json::<serde_json::Value>(&path, 16).unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::LimitExceeded {
                what: "stored_json_bytes",
                limit: 16
            }
        ));
    }

    #[test]
    fn package_import_rejects_excessive_directory_depth() {
        let temp = tempdir().unwrap();
        let mut current = temp.path().to_path_buf();
        for index in 0..=MAX_DIRECTORY_DEPTH {
            current = current.join(format!("d{index}"));
            std::fs::create_dir(&current).unwrap();
        }
        std::fs::write(current.join("payload.txt"), b"payload").unwrap();
        let error = snapshot_local_path(temp.path()).unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::LimitExceeded {
                what: "directory_depth",
                ..
            }
        ));
    }
}
