//! Restrictive, non-overwriting bootstrap artifact publication.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::Path;

use sha2::{Digest as _, Sha256};

use super::types::PrepareFileIdentity;

pub(super) fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)?;
    reject_insecure_dir(path)
}

#[cfg(windows)]
pub(super) fn publish_new(_path: &Path, _bytes: &[u8]) -> io::Result<PrepareFileIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "access-bootstrap prepare is unavailable on Windows until owner-only ACL and verified file-ID deletion are implemented",
    ))
}

#[cfg(not(windows))]
pub(super) fn publish_new(path: &Path, bytes: &[u8]) -> io::Result<PrepareFileIdentity> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "output path must be absolute",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "output has no parent"))?;
    reject_insecure_dir(parent)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    let identity = identity(path, &file, bytes)?;
    sync_parent(parent)?;
    Ok(identity)
}

pub(super) fn replace_journal(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("journal has no parent"))?;
    create_private_dir(parent)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".bootstrap-journal-")
        .tempfile_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    sync_parent(parent)
}

pub(super) fn verify_identity(expected: &PrepareFileIdentity) -> io::Result<()> {
    read_verified(expected).map(drop)
}

pub(super) fn read_private(path: &Path) -> io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    identity(path, &file, &bytes)?;
    Ok(bytes)
}

pub(super) fn read_verified(expected: &PrepareFileIdentity) -> io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(&expected.path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let actual = identity(&expected.path, &file, &bytes)?;
    if &actual != expected {
        return Err(io::Error::other("prepared file identity changed"));
    }
    Ok(bytes)
}

#[cfg(unix)]
pub(super) fn delete_exact(expected: &PrepareFileIdentity) -> io::Result<()> {
    use nix::fcntl::{OFlag, openat};
    use nix::sys::stat::Mode;
    use nix::unistd::{UnlinkatFlags, unlinkat};
    use std::os::unix::fs::OpenOptionsExt as _;

    let parent = expected
        .path
        .parent()
        .ok_or_else(|| io::Error::other("artifact has no parent"))?;
    let name = expected
        .path
        .file_name()
        .ok_or_else(|| io::Error::other("artifact has no basename"))?;
    reject_insecure_dir(parent)?;
    let parent_file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW)
        .open(parent)?;
    {
        use std::os::unix::fs::MetadataExt as _;
        let metadata = parent_file.metadata()?;
        if metadata.dev() != expected.parent_device || metadata.ino() != expected.parent_inode {
            return Err(io::Error::other(
                "prepared artifact parent identity changed",
            ));
        }
    }
    let child_fd = openat(
        &parent_file,
        name,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map_err(io::Error::other)?;
    let mut child = File::from(child_fd);
    let mut bytes = Vec::new();
    child.read_to_end(&mut bytes)?;
    let actual = identity(&expected.path, &child, &bytes)?;
    if &actual != expected {
        return Err(io::Error::other("prepared file identity changed"));
    }
    unlinkat(&parent_file, name, UnlinkatFlags::NoRemoveDir).map_err(io::Error::other)?;
    match fs::symlink_metadata(parent.join(name)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(io::Error::other("artifact path still exists after unlink")),
        Err(error) => Err(error),
    }
}

#[cfg(not(unix))]
pub(super) fn delete_exact(_expected: &PrepareFileIdentity) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "verified handle deletion is unavailable on this platform",
    ))
}

fn identity(path: &Path, file: &File, bytes: &[u8]) -> io::Result<PrepareFileIdentity> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("prepared artifact is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("artifact has no parent"))?;
        let parent_metadata = fs::metadata(parent)?;
        if metadata.uid() != nix::unistd::Uid::effective().as_raw()
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err(io::Error::other("prepared artifact metadata is insecure"));
        }
        Ok(PrepareFileIdentity {
            path: path.to_path_buf(),
            digest_hex: hex::encode(Sha256::digest(bytes)),
            device: metadata.dev(),
            inode: metadata.ino(),
            parent_device: parent_metadata.dev(),
            parent_inode: parent_metadata.ino(),
            owner: metadata.uid(),
            mode: metadata.mode() & 0o777,
            links: metadata.nlink(),
        })
    }
    #[cfg(not(unix))]
    {
        Ok(PrepareFileIdentity {
            path: path.to_path_buf(),
            digest_hex: hex::encode(Sha256::digest(bytes)),
        })
    }
}

fn reject_insecure_dir(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::other("output parent is not a secure directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.uid() != nix::unistd::Uid::effective().as_raw() || metadata.mode() & 0o022 != 0
        {
            return Err(io::Error::other(
                "output parent is writable by another user",
            ));
        }
    }
    Ok(())
}

fn sync_parent(parent: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secret");
        publish_new(&path, b"first").unwrap();
        assert_eq!(
            publish_new(&path, b"second").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(path).unwrap(), b"first");
    }

    #[cfg(unix)]
    #[test]
    fn unix_publication_records_restrictive_exact_identity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secret");
        let identity = publish_new(&path, b"secret").unwrap();
        assert_eq!(identity.mode, 0o600);
        assert_eq!(identity.links, 1);
        verify_identity(&identity).unwrap();
        fs::write(&path, b"changed").unwrap();
        assert!(verify_identity(&identity).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unix_delete_unlinks_only_the_recorded_inode() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secret");
        let identity = publish_new(&path, b"secret").unwrap();
        delete_exact(&identity).unwrap();
        assert!(!path.exists());

        let identity = publish_new(&path, b"first").unwrap();
        fs::remove_file(&path).unwrap();
        publish_new(&path, b"replacement").unwrap();
        assert!(delete_exact(&identity).is_err());
        assert_eq!(fs::read(path).unwrap(), b"replacement");
    }

    #[cfg(unix)]
    #[test]
    fn unix_delete_refuses_a_replaced_parent_directory() {
        let base = tempfile::tempdir().unwrap();
        let parent = base.path().join("artifacts");
        create_private_dir(&parent).unwrap();
        let path = parent.join("secret");
        let identity = publish_new(&path, b"secret").unwrap();

        fs::rename(&parent, base.path().join("original-artifacts")).unwrap();
        create_private_dir(&parent).unwrap();
        let replacement = parent.join("secret");
        publish_new(&replacement, b"replacement").unwrap();

        assert!(delete_exact(&identity).is_err());
        assert_eq!(fs::read(replacement).unwrap(), b"replacement");
    }

    #[cfg(windows)]
    #[test]
    fn windows_prepare_fails_closed_until_verified_acl_and_deletion_exist() {
        let directory = tempfile::tempdir().unwrap();
        let error = publish_new(&directory.path().join("secret"), b"secret").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }
}
