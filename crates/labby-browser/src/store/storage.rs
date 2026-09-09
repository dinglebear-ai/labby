//! Private SQLite files and one process owner for a browser runtime.

use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::{BrowserError, Result};

pub(super) struct Ownership {
    // Never unlink the lock: waiters must continue to contend on the same inode.
    _lock: File,
    _directories: Vec<File>,
}

/// Offline ownership guard sharing the live browser daemon's lock.
pub struct BrowserStorageLock {
    _ownership: Ownership,
}

impl BrowserStorageLock {
    /// Lock an existing browser directory without opening or changing SQLite.
    /// Returns `None` when the directory does not exist. A missing database is
    /// allowed so restore can hold ownership before publishing its files.
    pub fn acquire_existing(path: impl AsRef<Path>) -> Result<Option<Self>> {
        Ok(acquire(path.as_ref(), false)?.map(|(_, ownership)| Self {
            _ownership: ownership,
        }))
    }
}

pub(super) fn prepare(path: &Path) -> Result<(PathBuf, Ownership)> {
    let (path, ownership) =
        acquire(path, true)?.ok_or_else(|| insecure("missing browser directory"))?;
    // Protect every filename SQLite can open before allowing schema writes.
    // The private parent protects subsequently recreated sidecars as well.
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let file_path = sidecar(&path, suffix);
        reject_alias(&file_path)?;
        drop(restricted_file(&file_path)?);
    }
    Ok((path, ownership))
}

fn acquire(path: &Path, create_parent: bool) -> Result<Option<(PathBuf, Ownership)>> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(insecure(
            "browser database requires an absolute traversal-free path",
        ));
    }
    let path = system_path(path);
    let parent = path
        .parent()
        .ok_or_else(|| insecure("missing database parent"))?;
    let Some(directories) = private_directory(parent, create_parent)? else {
        return Ok(None);
    };
    let lock_path = sidecar(&path, ".lock");
    reject_alias(&lock_path)?;
    let lock = restricted_file(&lock_path)?;
    lock.try_lock().map_err(|error| {
        BrowserError::StorageIo(io::Error::new(io::ErrorKind::WouldBlock, error))
    })?;
    if !create_parent {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            reject_alias(&sidecar(&path, suffix))?;
        }
    }
    Ok(Some((
        path,
        Ownership {
            _lock: lock,
            _directories: directories,
        },
    )))
}

pub(super) fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}

fn insecure(message: &'static str) -> BrowserError {
    io::Error::new(io::ErrorKind::PermissionDenied, message).into()
}

fn reject_alias(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(insecure("browser database files must be ordinary files"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 || metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(insecure(
                "browser database files must be owned and unaliased",
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(insecure("browser database reparse points are forbidden"));
        }
        let file = labby_winjob::fs::open_read(path, false)?;
        if labby_winjob::fs::identity(&file, false)?.links != 1 {
            return Err(insecure(
                "browser database files must have exactly one hard link",
            ));
        }
    }
    Ok(())
}

fn system_path(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    for (alias, target) in [
        ("/var", "/private/var"),
        ("/tmp", "/private/tmp"),
        ("/etc", "/private/etc"),
    ] {
        if let Ok(suffix) = path.strip_prefix(alias)
            && std::fs::read_link(alias)
                .is_ok_and(|link| link == Path::new(target) || link == Path::new(&target[1..]))
        {
            return Path::new(target).join(suffix);
        }
    }
    path.to_path_buf()
}

#[cfg(unix)]
fn private_directory(path: &Path, create: bool) -> Result<Option<Vec<File>>> {
    use rustix::fs::{Mode, OFlags, mkdirat, openat};
    use std::os::fd::AsFd;
    use std::os::unix::fs::MetadataExt;

    let mut directory = File::open("/")?;
    let components: Vec<_> = path
        .components()
        .filter(|part| matches!(part, Component::Normal(_)))
        .collect();
    if components.is_empty() {
        return Err(insecure("browser database cannot live in filesystem root"));
    }
    for (index, part) in components.iter().enumerate() {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let opened = match openat(directory.as_fd(), part.as_os_str(), flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => {
                if !create {
                    return Ok(None);
                }
                match mkdirat(directory.as_fd(), part.as_os_str(), Mode::RWXU) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(error) => return Err(io::Error::from(error).into()),
                }
                openat(directory.as_fd(), part.as_os_str(), flags, Mode::empty())
                    .map_err(io::Error::from)?
            }
            Err(error) => return Err(io::Error::from(error).into()),
        };
        directory = File::from(opened);
        let metadata = directory.metadata()?;
        let owned = metadata.uid() == rustix::process::geteuid().as_raw();
        if index + 1 == components.len() {
            if !owned || metadata.mode() & 0o077 != 0 {
                return Err(insecure(
                    "browser directory must be private and belong to the current user",
                ));
            }
        } else if (!owned && metadata.uid() != 0)
            || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
        {
            return Err(insecure(
                "browser directory ancestor is writable by another user",
            ));
        }
    }
    Ok(Some(vec![directory]))
}

#[cfg(windows)]
fn private_directory(path: &Path, create: bool) -> Result<Option<Vec<File>>> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    let mut guards = Vec::new();
    let mut current = PathBuf::new();
    let mut created_final = false;
    for component in path.components() {
        current.push(component.as_os_str());
        if !matches!(component, Component::Normal(_)) {
            continue;
        }
        match std::fs::symlink_metadata(&current) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match std::fs::create_dir(&current) {
                    Ok(()) => created_final = current == path,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        }
        let guard = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2) // Deny rename/deletion while SQLite uses this path.
            .custom_flags(0x0200_0000 | 0x0020_0000) // BACKUP_SEMANTICS | OPEN_REPARSE_POINT
            .open(&current)?;
        let metadata = guard.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
            return Err(insecure("browser directory reparse points are forbidden"));
        }
        if current != path {
            // A private child cannot be safely created beneath foreign write authority.
            labby_winjob::fs::verify_directory_acl(&guard)?;
        }
        guards.push(guard);
    }
    if guards.is_empty() {
        return Err(insecure("browser database cannot live in filesystem root"));
    }
    if created_final {
        protect_new_windows_directory(path)?;
    } else {
        // Do not rewrite ACLs on an arbitrary existing caller-selected parent.
        // Legacy shared directories require an explicit operator repair.
        verify_private_windows_acl(path)?;
    }
    Ok(Some(guards))
}

#[cfg(windows)]
pub(super) fn verify_private_windows_acl(path: &Path) -> Result<()> {
    let file = if path.is_dir() {
        labby_winjob::fs::open_directory(path)?
    } else {
        labby_winjob::fs::open_read(path, false)?
    };
    if path.is_dir() {
        labby_winjob::fs::verify_private_directory_dacl(&file)?;
    } else {
        labby_winjob::fs::verify_private_acl(&file)?;
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn protect_new_windows_directory(path: &Path) -> Result<()> {
    let file = labby_winjob::fs::open_directory(path)?;
    labby_winjob::fs::set_created_owner(path, &file, true)?;
    labby_winjob::fs::harden_private_directory_dacl(path, &file)?;
    verify_private_windows_acl(path)
}

#[cfg(not(any(unix, windows)))]
fn private_directory(_path: &Path, _create: bool) -> Result<Option<Vec<File>>> {
    Err(insecure(
        "browser storage protection is unsupported on this platform",
    ))
}

fn restricted_file(path: &Path) -> Result<File> {
    labby_auth::util::open_restricted_lock_file(path)
        .map_err(|error| io::Error::other(error.to_string()).into())
}
