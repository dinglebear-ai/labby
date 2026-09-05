//! Secure, crash-durable atomic file replacement.

use std::io::Write;
use std::path::Path;

use atomic_write_file::AtomicWriteFile;

/// Replace `path` atomically with owner-only contents.
///
/// The temporary file is created by `atomic-write-file` in the destination
/// directory, flushed before publication, and the parent directory is flushed
/// after publication on platforms that support directory handles.
pub fn write_secure_atomic(path: &Path, bytes: &[u8]) -> Result<(), AtomicWriteError> {
    let mut published = false;
    write_secure_atomic_with(path, bytes, |stage| {
        if stage == AtomicWriteStage::BeforeParentSync {
            published = true;
        }
        Ok(())
    })
    .map_err(|source| AtomicWriteError { source, published })
}

#[derive(Debug)]
pub struct AtomicWriteError {
    pub source: std::io::Error,
    /// True when atomic replacement completed but parent-directory durability
    /// could not be confirmed.
    pub published: bool,
}

impl std::fmt::Display for AtomicWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(formatter)
    }
}

impl std::error::Error for AtomicWriteError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicWriteStage {
    BeforeWrite,
    BeforeCommit,
    BeforeParentSync,
}

#[doc(hidden)]
pub fn write_secure_atomic_with(
    path: &Path,
    bytes: &[u8],
    mut boundary: impl FnMut(AtomicWriteStage) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("atomic file has no parent directory"))?;
    let mut output = AtomicWriteFile::open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        output
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    boundary(AtomicWriteStage::BeforeWrite)?;
    output.write_all(bytes)?;
    output.sync_all()?;
    boundary(AtomicWriteStage::BeforeCommit)?;
    output.commit()?;
    boundary(AtomicWriteStage::BeforeParentSync)?;
    sync_parent(parent)
}

fn sync_parent(parent: &Path) -> std::io::Result<()> {
    #[cfg(not(windows))]
    {
        std::fs::File::open(parent)?.sync_all()
    }
    #[cfg(windows)]
    {
        let _ = std::fs::metadata(parent)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_file_without_residue() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        write_secure_atomic(&path, b"old").unwrap();
        write_secure_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn precommit_failure_preserves_old_file_and_removes_temporary_residue() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        write_secure_atomic(&path, b"old").unwrap();
        let error = write_secure_atomic_with(&path, b"new", |stage| {
            if stage == AtomicWriteStage::BeforeCommit {
                Err(std::io::Error::other("injected crash boundary"))
            } else {
                Ok(())
            }
        });
        assert!(error.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn parent_sync_failure_is_reported_after_atomic_publication() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let error = write_secure_atomic_with(&path, b"new", |stage| {
            if stage == AtomicWriteStage::BeforeParentSync {
                Err(std::io::Error::other("parent sync failed"))
            } else {
                Ok(())
            }
        });
        assert!(error.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }
}
