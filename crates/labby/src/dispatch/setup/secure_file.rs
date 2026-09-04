//! Restrictive, non-overwriting bootstrap artifact publication.

#[cfg(not(windows))]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::path::Path;

use sha2::{Digest as _, Sha256};

use super::types::PrepareFileIdentity;

/// Bootstrap proofs, credentials, identity files and journals are each bounded
/// to 1 MiB, including when reading a locally modified or corrupt artifact.
const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;

fn validate_size(bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > MAX_ARTIFACT_BYTES {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bootstrap artifact exceeds 1 MiB",
        ))
    } else {
        Ok(())
    }
}

fn read_bounded_private(path: &Path, file: &mut File) -> io::Result<Vec<u8>> {
    // Verify the exact file type, owner, ACL and link count before reading.
    identity(path, file, &[])?;
    let mut bytes = Vec::new();
    file.take(MAX_ARTIFACT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    validate_size(&bytes)?;
    Ok(bytes)
}

#[cfg(not(windows))]
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
pub(super) fn create_private_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let parents =
                labby_winjob::fs::AncestorGuard::for_file(&path.join(".permission-check"))?;
            return parents.verify_parent_acl();
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("directory has no parent"))?;
    create_private_dir(parent)?;
    let parents = labby_winjob::fs::AncestorGuard::for_file(path)?;
    parents.verify_parent_acl()?;
    match fs::create_dir(path) {
        Ok(()) => {}
        // A concurrent creator owns its directory; validate but never rewrite.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return create_private_dir(path);
        }
        Err(error) => return Err(error),
    }
    let directory = labby_winjob::fs::open_directory(path)?;
    labby_auth::util::harden_secret_file(path).map_err(io::Error::other)?;
    labby_winjob::fs::set_created_owner(path, &directory, true)?;
    labby_winjob::fs::verify_directory_acl(&directory)
}

#[cfg(windows)]
pub(super) fn publish_new(path: &Path, bytes: &[u8]) -> io::Result<PrepareFileIdentity> {
    validate_size(bytes)?;
    let _parents = labby_winjob::fs::AncestorGuard::for_file(path)?;
    _parents.verify_parent_acl()?;
    let temporary = write_private_temporary(path, bytes, |file| file.sync_all())?;
    let original = labby_winjob::fs::identity(&temporary.file, false)?;
    // The shared restricted creator refuses delete sharing, so release its
    // handle before the atomic, non-overwriting Windows rename.
    let PrivateTemporary {
        file,
        path: temporary,
    } = temporary;
    drop(file);
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    let file = labby_winjob::fs::open_read(path, false)?;
    if labby_winjob::fs::identity(&file, false)? != original {
        return Err(io::Error::other("published artifact was replaced"));
    }
    identity(path, &file, bytes)
}

#[cfg(windows)]
struct PrivateTemporary {
    // Rust drops fields in declaration order: close the no-delete-sharing
    // handle before TempPath attempts cleanup, including on any early error.
    file: File,
    path: tempfile::TempPath,
}

#[cfg(windows)]
fn write_private_temporary(
    path: &Path,
    bytes: &[u8],
    finish: impl FnOnce(&File) -> io::Result<()>,
) -> io::Result<PrivateTemporary> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("artifact has no parent"))?;
    let temporary = parent.join(format!(".bootstrap-{}", uuid::Uuid::new_v4()));
    let file =
        labby_auth::util::create_restricted_secret_file(&temporary).map_err(io::Error::other)?;
    let mut temporary = PrivateTemporary {
        file,
        path: tempfile::TempPath::try_from_path(temporary)?,
    };
    labby_winjob::fs::set_created_owner(&temporary.path, &temporary.file, false)?;
    labby_winjob::fs::verify_private_acl(&temporary.file)?;
    temporary.file.write_all(bytes)?;
    finish(&temporary.file)?;
    Ok(temporary)
}

#[cfg(not(windows))]
pub(super) fn publish_new(path: &Path, bytes: &[u8]) -> io::Result<PrepareFileIdentity> {
    publish_new_with_writer(path, bytes, |file| {
        file.write_all(bytes)?;
        file.sync_all()
    })
}

#[cfg(not(windows))]
fn publish_new_with_writer(
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<PrepareFileIdentity> {
    validate_size(bytes)?;
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
    // Publish complete bytes, never a visible empty/partial credential or ID.
    // NamedTempFile defaults to owner-only mode; no-clobber publication keeps
    // an existing destination (including a symlink) untouched.
    let mut temporary = tempfile::Builder::new()
        .prefix(".bootstrap-publication-")
        .tempfile_in(parent)?;
    write(temporary.as_file_mut())?;
    let file = temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    let identity = identity(path, &file, bytes)?;
    sync_parent(parent)?;
    Ok(identity)
}

pub(super) fn replace_journal(path: &Path, bytes: &[u8]) -> io::Result<()> {
    validate_size(bytes)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("journal has no parent"))?;
    create_private_dir(parent)?;
    #[cfg(windows)]
    {
        let _parents = labby_winjob::fs::AncestorGuard::for_file(path)?;
        _parents.verify_parent_acl()?;
        // Refuse an unsafe existing destination; a valid journal is replaced
        // atomically, never opened with truncate or followed through a reparse.
        match labby_winjob::fs::open_read(path, false) {
            Ok(file) => {
                identity(path, &file, &[])?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let PrivateTemporary {
            file,
            path: temporary,
        } = write_private_temporary(path, bytes, |file| file.sync_all())?;
        drop(file);
        temporary.persist(path).map_err(|error| error.error)?;
        return sync_parent(parent);
    }
    #[cfg(not(windows))]
    {
        let mut temporary = tempfile::Builder::new()
            .prefix(".bootstrap-journal-")
            .tempfile_in(parent)?;
        temporary.write_all(bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        sync_parent(parent)
    }
}

pub(super) fn verify_identity(expected: &PrepareFileIdentity) -> io::Result<()> {
    read_verified(expected).map(drop)
}

pub(super) fn read_private(path: &Path) -> io::Result<Vec<u8>> {
    #[cfg(windows)]
    let _parents = labby_winjob::fs::AncestorGuard::for_file(path)?;
    #[cfg(windows)]
    _parents.verify_parent_acl()?;
    #[cfg(windows)]
    let mut file = labby_winjob::fs::open_read(path, false)?;
    #[cfg(not(windows))]
    let mut file = {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
        }
        options.open(path)?
    };
    let bytes = read_bounded_private(path, &mut file)?;
    identity(path, &file, &bytes)?;
    Ok(bytes)
}

pub(super) fn read_verified(expected: &PrepareFileIdentity) -> io::Result<Vec<u8>> {
    #[cfg(windows)]
    let _parents = labby_winjob::fs::AncestorGuard::for_file(&expected.path)?;
    #[cfg(windows)]
    _parents.verify_parent_acl()?;
    #[cfg(windows)]
    let mut file = labby_winjob::fs::open_read(&expected.path, false)?;
    #[cfg(not(windows))]
    let mut file = {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
        }
        options.open(&expected.path)?
    };
    let bytes = read_bounded_private(&expected.path, &mut file)?;
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
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK,
        Mode::empty(),
    )
    .map_err(io::Error::other)?;
    let mut child = File::from(child_fd);
    let bytes = read_bounded_private(&expected.path, &mut child)?;
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

#[cfg(windows)]
pub(super) fn delete_exact(expected: &PrepareFileIdentity) -> io::Result<()> {
    let _parents = labby_winjob::fs::AncestorGuard::for_file(&expected.path)?;
    _parents.verify_parent_acl()?;
    let mut file = labby_winjob::fs::open_read(&expected.path, true)?;
    let bytes = read_bounded_private(&expected.path, &mut file)?;
    let actual = identity(&expected.path, &file, &bytes)?;
    if &actual != expected {
        return Err(io::Error::other("prepared file identity changed"));
    }
    labby_winjob::fs::delete_on_close(&file)?;
    drop(file);
    match fs::symlink_metadata(&expected.path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(io::Error::other(
            "artifact path still exists after verified deletion",
        )),
        Err(error) => Err(error),
    }
}

#[cfg(not(any(unix, windows)))]
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
    #[cfg(windows)]
    {
        let parents = labby_winjob::fs::AncestorGuard::for_file(path)?;
        let parent = parents.parent_identity()?;
        let file_identity = labby_winjob::fs::identity(file, false)?;
        labby_winjob::fs::verify_private_acl(file)?;
        Ok(PrepareFileIdentity {
            path: path.to_path_buf(),
            digest_hex: hex::encode(Sha256::digest(bytes)),
            volume: file_identity.volume,
            file_id: file_identity.id,
            parent_volume: parent.volume,
            parent_file_id: parent.id,
            links: file_identity.links,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(PrepareFileIdentity {
            path: path.to_path_buf(),
            digest_hex: hex::encode(Sha256::digest(bytes)),
        })
    }
}

#[cfg(not(windows))]
fn reject_insecure_dir(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::other("output parent is not a secure directory"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(io::Error::other("output parent is a reparse point"));
        }
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

    #[cfg(unix)]
    #[test]
    fn concurrent_reader_never_observes_partial_publication() {
        let directory = tempfile::tempdir().unwrap();
        create_private_dir(directory.path()).unwrap();
        let path = directory.path().join("identity");
        let (partial, observed) = std::sync::mpsc::channel();
        let (resume, proceed) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let path = &path;
            let writer = scope.spawn(move || {
                publish_new_with_writer(path, b"complete identity", |file| {
                    file.write_all(b"complete ")?;
                    partial.send(()).unwrap();
                    proceed
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    file.write_all(b"identity")?;
                    file.sync_all()
                })
            });
            observed
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            assert_eq!(
                read_private(path).unwrap_err().kind(),
                io::ErrorKind::NotFound
            );
            resume.send(()).unwrap();
            writer.join().unwrap().unwrap();
        });
        assert_eq!(read_private(&path).unwrap(), b"complete identity");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn publication_write_failure_and_lost_race_clean_temporary_files() {
        let directory = tempfile::tempdir().unwrap();
        create_private_dir(directory.path()).unwrap();
        let path = directory.path().join("identity");
        let failure = publish_new_with_writer(&path, b"complete", |file| {
            file.write_all(b"partial")?;
            Err(io::Error::other("injected sync failure"))
        });
        assert!(failure.is_err());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
        publish_new(&path, b"winner").unwrap();
        assert_eq!(
            publish_new(&path, b"loser").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(read_private(&path).unwrap(), b"winner");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn oversized_artifacts_are_refused_before_publication_and_during_read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secret");
        let oversized = vec![0; MAX_ARTIFACT_BYTES + 1];
        assert!(publish_new(&path, &oversized).is_err());
        assert!(!path.exists());
        let expected = publish_new(&path, b"valid").unwrap();
        fs::write(&path, &oversized).unwrap();
        assert!(read_private(&path).is_err());
        assert!(read_verified(&expected).is_err());
        assert!(delete_exact(&expected).is_err());
        assert!(path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_refused_without_waiting_for_a_writer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fifo");
        nix::unistd::mkfifo(
            &path,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        assert!(read_private(&path).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_failed_temporary_write_closes_handle_before_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secret");
        let result = write_private_temporary(&path, b"sensitive", |_| {
            Err(io::Error::other("injected synchronization failure"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

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
    fn windows_publication_and_exact_deletion_preserve_replacements() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secret");
        let original = publish_new(&path, b"secret").unwrap();
        assert_eq!(original.links, 1);
        assert_ne!(original.file_id, [0; 16]);
        verify_identity(&original).unwrap();
        delete_exact(&original).unwrap();
        assert!(!path.exists());

        let original = publish_new(&path, b"first").unwrap();
        fs::rename(&path, directory.path().join("retained-first")).unwrap();
        publish_new(&path, b"first").unwrap();
        assert!(delete_exact(&original).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"first");
    }

    #[cfg(windows)]
    #[test]
    fn windows_verification_refuses_changed_parent_hardlinks_and_content() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("original");
        create_private_dir(&parent).unwrap();
        let path = parent.join("secret");
        let original = publish_new(&path, b"secret").unwrap();
        fs::rename(&parent, directory.path().join("retained-parent")).unwrap();
        create_private_dir(&parent).unwrap();
        publish_new(&path, b"secret").unwrap();
        assert!(delete_exact(&original).is_err());

        let expected = publish_new(&parent.join("linked"), b"private").unwrap();
        fs::hard_link(&expected.path, parent.join("alias")).unwrap();
        assert!(verify_identity(&expected).is_err());
        assert!(delete_exact(&expected).is_err());
        assert_eq!(fs::read(&expected.path).unwrap(), b"private");

        let expected = publish_new(&parent.join("changed"), b"before").unwrap();
        fs::write(&expected.path, b"after").unwrap();
        assert!(delete_exact(&expected).is_err());
        assert_eq!(fs::read(&expected.path).unwrap(), b"after");
    }

    #[cfg(windows)]
    #[test]
    fn windows_journal_replacement_remains_private_and_verifiable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal");
        replace_journal(&path, b"first").unwrap();
        replace_journal(&path, b"second").unwrap();
        assert_eq!(read_private(&path).unwrap(), b"second");
    }

    #[cfg(windows)]
    #[test]
    fn windows_publication_refuses_junctions_and_alternate_data_streams() {
        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real");
        create_private_dir(&real).unwrap();
        let junction = directory.path().join("junction");
        let status = std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&real)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(publish_new(&junction.join("secret"), b"secret").is_err());
        assert!(!real.join("secret").exists());
        assert!(publish_new(&real.join("base:stream"), b"secret").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_existing_inherited_acl_is_refused_without_rewriting() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("untrusted");
        fs::write(&path, b"unchanged").unwrap();
        assert!(read_private(&path).is_err());
        assert!(replace_journal(&path, b"replacement").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"unchanged");
    }

    #[cfg(windows)]
    #[test]
    fn windows_foreign_writable_parent_is_refused_without_rewriting() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("foreign-write");
        create_private_dir(&parent).unwrap();
        let sentinel = parent.join("sentinel");
        fs::write(&sentinel, b"unchanged").unwrap();
        let icacls = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/icacls.exe");
        assert!(
            std::process::Command::new(&icacls)
                .arg(&parent)
                .args(["/grant", "*S-1-1-0:(F)"])
                .status()
                .unwrap()
                .success()
        );
        let before = std::process::Command::new(&icacls)
            .arg(&parent)
            .output()
            .unwrap();
        assert!(before.status.success());
        assert!(create_private_dir(&parent).is_err());
        assert!(publish_new(&parent.join("secret"), b"secret").is_err());
        let after = std::process::Command::new(&icacls)
            .arg(&parent)
            .output()
            .unwrap();
        assert!(after.status.success());
        assert_eq!(before.stdout, after.stdout);
        assert_eq!(fs::read(sentinel).unwrap(), b"unchanged");
        assert_eq!(fs::read_dir(parent).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn windows_pinned_parent_cannot_be_renamed_until_guard_is_released() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("parent");
        create_private_dir(&parent).unwrap();
        let guard = labby_winjob::fs::AncestorGuard::for_file(&parent.join("secret")).unwrap();
        guard.verify_parent_acl().unwrap();
        let moved = directory.path().join("moved");
        assert!(fs::rename(&parent, &moved).is_err());
        drop(guard);
        fs::rename(&parent, moved).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_legacy_identity_without_file_ids_cannot_authorize_deletion() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secret");
        let expected = publish_new(&path, b"retained").unwrap();
        let mut legacy = serde_json::to_value(&expected).unwrap();
        for field in [
            "volume",
            "file_id",
            "parent_volume",
            "parent_file_id",
            "links",
        ] {
            legacy.as_object_mut().unwrap().remove(field);
        }
        let legacy = serde_json::from_value(legacy).unwrap();
        assert!(delete_exact(&legacy).is_err());
        assert_eq!(fs::read(path).unwrap(), b"retained");
    }
}
