//! Local filesystem mechanics for the Artifact store.

use atomic_write_file::AtomicWriteFile;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

#[cfg(not(any(target_os = "linux", target_os = "android")))]
use crate::path_safety::{canonicalize_and_reject_read_path, rel_to_unix_string};
use crate::path_safety::{
    canonicalize_and_reject_write_path, reject_existing_symlink_ancestors,
    reject_existing_symlinks_in_path, reject_symlink,
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
    #[cfg(any(target_os = "linux", target_os = "android"))]
    return snapshot_local_path_descriptor_relative(source, |_| {});

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    snapshot_local_path_portable(source)
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn snapshot_local_path_portable(source: &Path) -> Result<Vec<SnapshotFile>, ArtifactError> {
    reject_symlink(source).map_err(|_| ArtifactError::UnsafePath("symlink"))?;
    let root = canonicalize_and_reject_read_path(source)
        .map_err(|_| ArtifactError::UnsafePath("source_root"))?;
    // Treat the validated, non-symlink source directory as the ownership
    // boundary. Canonicalizing first normalizes platform-managed ancestor
    // aliases such as macOS `/var` -> `/private/var`; all traversal below the
    // canonical boundary remains lstat/openat guarded.
    reject_existing_symlinks_in_path(&root).map_err(|_| ArtifactError::UnsafePath("symlink"))?;
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

#[cfg(any(target_os = "linux", target_os = "android"))]
fn snapshot_local_path_descriptor_relative(
    source: &Path,
    before_open: impl Fn(&str),
) -> Result<Vec<SnapshotFile>, ArtifactError> {
    if !source.is_absolute() {
        return Err(ArtifactError::UnsafePath("source_root"));
    }
    let source_handle = open_absolute_no_follow(source)?;
    let metadata = source_handle.metadata()?;
    let mut files = Vec::new();
    let mut total = 0;
    let mut entries_seen = 0;
    if metadata.is_file() {
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("path", "non_utf8"))?;
        files.push(read_open_file(source_handle, name, &mut total)?);
    } else if metadata.is_dir() {
        collect_directory_fd(
            &source_handle,
            "",
            0,
            &mut entries_seen,
            &mut files,
            &mut total,
            &before_open,
        )?;
    } else {
        return Err(invalid("source", "not_regular_file_or_directory"));
    }
    if files.is_empty() {
        return Err(invalid("source", "empty_package"));
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn open_absolute_no_follow(path: &Path) -> Result<File, ArtifactError> {
    use std::path::Component;
    let mut current = open_directory(Path::new("/"))?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(segment) => {
                let candidate = proc_fd_path(&current).join(segment);
                current = open_no_follow(&candidate, false)?;
            }
            _ => return Err(ArtifactError::UnsafePath("source_root")),
        }
    }
    Ok(current)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn collect_directory_fd(
    directory: &File,
    prefix: &str,
    depth: usize,
    entries_seen: &mut usize,
    files: &mut Vec<SnapshotFile>,
    total: &mut u64,
    before_open: &impl Fn(&str),
) -> Result<(), ArtifactError> {
    if depth > MAX_DIRECTORY_DEPTH {
        return Err(ArtifactError::LimitExceeded {
            what: "directory_depth",
            limit: MAX_DIRECTORY_DEPTH as u64,
        });
    }
    let mut names = std::fs::read_dir(proc_fd_path(directory))?
        .map(|entry| entry.and_then(|entry| Ok((entry.file_name(), entry.metadata()?))))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, expected) in names {
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
        let name_text = name.to_str().ok_or_else(|| invalid("path", "non_utf8"))?;
        let relative = if prefix.is_empty() {
            name_text.to_owned()
        } else {
            format!("{prefix}/{name_text}")
        };
        validate_relative_path(&relative)?;
        before_open(&relative);
        let handle = open_no_follow(&proc_fd_path(directory).join(&name), false)?;
        let metadata = handle.metadata()?;
        if !same_file_identity(&expected, &metadata) {
            return Err(ArtifactError::UnsafePath("source_replaced"));
        }
        if metadata.is_dir() {
            collect_directory_fd(
                &handle,
                &relative,
                depth + 1,
                entries_seen,
                files,
                total,
                before_open,
            )?;
        } else if metadata.is_file() {
            if files.len() >= MAX_COMPONENTS {
                return Err(ArtifactError::LimitExceeded {
                    what: "component_count",
                    limit: MAX_COMPONENTS as u64,
                });
            }
            files.push(read_open_file(handle, &relative, total)?);
        } else {
            return Err(invalid("source", "special_file"));
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn proc_fd_path(file: &File) -> PathBuf {
    use std::os::fd::AsRawFd as _;
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn open_directory(path: &Path) -> Result<File, ArtifactError> {
    open_no_follow(path, true)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn open_no_follow(path: &Path, directory: bool) -> Result<File, ArtifactError> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut options = OpenOptions::new();
    options.read(true);
    let directory_flag = if directory { 0o200_000 } else { 0 };
    options.custom_flags(0o400_000 | directory_flag);
    options
        .open(path)
        .map_err(|error| match error.raw_os_error() {
            Some(40) => ArtifactError::UnsafePath("symlink"),
            _ => ArtifactError::Io(error),
        })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn read_open_file(
    mut file: File,
    relative: &str,
    total: &mut u64,
) -> Result<SnapshotFile, ArtifactError> {
    validate_relative_path(relative)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("source", "not_regular_file"));
    }
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    if metadata.nlink() != 1 {
        return Err(ArtifactError::UnsafePath("hardlink"));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(ArtifactError::LimitExceeded {
            what: "file_size",
            limit: MAX_FILE_BYTES,
        });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    std::io::Read::by_ref(&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if size > MAX_FILE_BYTES {
        return Err(ArtifactError::LimitExceeded {
            what: "file_size",
            limit: MAX_FILE_BYTES,
        });
    }
    *total = total
        .checked_add(size)
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
    Ok(SnapshotFile {
        path: relative.to_owned(),
        bytes,
        unix_mode: Some(metadata.permissions().mode() & 0o0755),
    })
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
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

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn read_one(
    root: &Path,
    path: &Path,
    relative: &str,
    total: &mut u64,
) -> Result<SnapshotFile, ArtifactError> {
    read_one_with_hook(root, path, relative, total, || {})
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn read_one_with_hook(
    root: &Path,
    path: &Path,
    relative: &str,
    total: &mut u64,
    after_open: impl FnOnce(),
) -> Result<SnapshotFile, ArtifactError> {
    validate_relative_path(relative)?;
    let root = std::fs::canonicalize(root)?;
    let canonical = std::fs::canonicalize(path)?;
    if canonical != root && !canonical.starts_with(&root) {
        return Err(ArtifactError::UnsafePath("source_escape"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // O_NOFOLLOW is 0o400000 on Linux/Android. The subsequent
        // handle/path identity check also rejects a replacement after open.
        options.custom_flags(0o400_000);
    }
    let mut file = options.open(path)?;
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.nlink() != 1 {
            return Err(ArtifactError::UnsafePath("hardlink"));
        }
    }
    after_open();
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
    let current = std::fs::symlink_metadata(path)?;
    if current.file_type().is_symlink() || !same_file_identity(&metadata, &current) {
        return Err(ArtifactError::UnsafePath("source_replaced"));
    }
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

#[cfg(unix)]
fn same_file_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.is_file() && right.is_file() && left.len() == right.len()
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

pub(crate) fn write_json_atomic_with_faults<T: Serialize + ?Sized>(
    path: &Path,
    value: &T,
    boundaries: [super::library::SkillTransactionBoundary; 4],
    fault: &mut impl FnMut(super::library::SkillTransactionBoundary) -> Result<(), ArtifactError>,
) -> Result<(), ArtifactError> {
    let bytes = canonical_json::to_canonical_vec(value)?;
    write_bytes_atomic_with_faults(path, &bytes, boundaries, fault)
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
    write_bytes_atomic_with_faults(
        path,
        bytes,
        [
            super::library::SkillTransactionBoundary::IntentWrite,
            super::library::SkillTransactionBoundary::IntentFileSync,
            super::library::SkillTransactionBoundary::IntentRename,
            super::library::SkillTransactionBoundary::IntentParentSync,
        ],
        &mut |_| Ok(()),
    )
}

pub(crate) fn write_bytes_atomic_with_faults(
    path: &Path,
    bytes: &[u8],
    boundaries: [super::library::SkillTransactionBoundary; 4],
    fault: &mut impl FnMut(super::library::SkillTransactionBoundary) -> Result<(), ArtifactError>,
) -> Result<(), ArtifactError> {
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
    fault(boundaries[0])?;
    output.write_all(bytes)?;
    fault(boundaries[1])?;
    output.sync_all()?;
    fault(boundaries[2])?;
    output.commit()?;
    fault(boundaries[3])?;
    File::open(parent)?.sync_all()?;
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

    #[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
    #[test]
    fn opened_file_replaced_by_symlink_is_rejected() {
        use std::os::unix::fs::symlink;
        let temp = tempdir().unwrap();
        let path = temp.path().join("payload.txt");
        let outside = temp.path().join("outside.txt");
        std::fs::write(&path, b"original").unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        let mut total = 0;
        let error = read_one_with_hook(temp.path(), &path, "payload.txt", &mut total, || {
            std::fs::remove_file(&path).unwrap();
            symlink(&outside, &path).unwrap();
        })
        .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::UnsafePath("source_replaced")
        ));
    }

    #[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
    #[test]
    fn opened_file_replaced_by_regular_file_is_rejected() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("payload.txt");
        let replacement = temp.path().join("replacement.txt");
        std::fs::write(&path, b"original").unwrap();
        std::fs::write(&replacement, b"replacement").unwrap();
        let mut total = 0;
        let error = read_one_with_hook(temp.path(), &path, "payload.txt", &mut total, || {
            std::fs::rename(&replacement, &path).unwrap();
        })
        .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::UnsafePath("source_replaced")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn package_import_rejects_hardlinks() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("source.txt");
        std::fs::write(&source, b"payload").unwrap();
        std::fs::hard_link(&source, temp.path().join("linked.txt")).unwrap();
        assert!(matches!(
            snapshot_local_path(temp.path()).unwrap_err(),
            ArtifactError::UnsafePath("hardlink")
        ));
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn ancestor_replaced_by_symlink_between_enumeration_and_open_is_rejected() {
        use std::os::unix::fs::symlink;
        let temp = tempdir().unwrap();
        let root = temp.path().join("root");
        let nested = root.join("nested");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(nested.join("payload.txt"), b"original").unwrap();
        std::fs::write(outside.join("payload.txt"), b"outside-secret").unwrap();
        let error = snapshot_local_path_descriptor_relative(&root, |relative| {
            if relative == "nested" {
                std::fs::rename(&nested, root.join("old-nested")).unwrap();
                symlink(&outside, &nested).unwrap();
            }
        })
        .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::UnsafePath("symlink" | "source_replaced")
        ));
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn ancestor_replaced_by_directory_between_enumeration_and_open_is_rejected() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("root");
        let nested = root.join("nested");
        let replacement = temp.path().join("replacement");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&replacement).unwrap();
        std::fs::write(nested.join("payload.txt"), b"original").unwrap();
        std::fs::write(replacement.join("payload.txt"), b"replacement-secret").unwrap();
        let error = snapshot_local_path_descriptor_relative(&root, |relative| {
            if relative == "nested" {
                std::fs::rename(&nested, root.join("old-nested")).unwrap();
                std::fs::rename(&replacement, &nested).unwrap();
            }
        })
        .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::UnsafePath("source_replaced")
        ));
    }
}
