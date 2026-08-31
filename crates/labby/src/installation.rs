//! Canonical installation paths and the process-wide lifecycle exclusion lock.
//!
//! Product state must resolve through [`InstallationPaths`]. A running daemon
//! owns the exclusive lifecycle lock for its entire lifetime; stopped-daemon
//! setup and recovery commands acquire the same lock before inspecting state.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

const LOCK_FILE_NAME: &str = "lifecycle.lock";

/// Canonical paths owned by one Labby installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationPaths {
    root: PathBuf,
}

impl InstallationPaths {
    /// Resolve the one installation root from `LABBY_HOME`, otherwise
    /// `$HOME/.labby` (`USERPROFILE` is the Windows fallback).
    pub fn resolve() -> Result<Self, InstallationError> {
        if let Some(root) = non_empty_env_path("LABBY_HOME") {
            return Self::from_root(root);
        }
        let home = non_empty_env_path("HOME")
            .or_else(|| non_empty_env_path("USERPROFILE"))
            .ok_or(InstallationError::HomeUnavailable)?;
        Self::from_root(home.join(".labby"))
    }

    /// Validate an explicitly selected installation root.
    pub fn from_root(root: impl Into<PathBuf>) -> Result<Self, InstallationError> {
        let root = root.into();
        if !root.is_absolute() {
            return Err(InstallationError::RelativeRoot(root));
        }
        let root = normalize_absolute(&root);
        if fs::symlink_metadata(&root).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(InstallationError::InsecureRoot(root));
        }
        let root = canonicalize_allow_missing(&root)?;
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn config_toml(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    #[must_use]
    pub fn dotenv(&self) -> PathBuf {
        self.root.join(".env")
    }

    #[must_use]
    pub fn access_db(&self) -> PathBuf {
        self.root.join("access.db")
    }

    #[must_use]
    pub fn lifecycle_lock(&self) -> PathBuf {
        self.root.join(LOCK_FILE_NAME)
    }

    /// Create and validate the root before any durable state is opened.
    pub fn prepare_root(&self) -> Result<(), InstallationError> {
        match fs::symlink_metadata(&self.root) {
            Ok(metadata) => validate_root_metadata(&self.root, &metadata),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_private_dir(&self.root)?;
                let metadata = fs::symlink_metadata(&self.root).map_err(|source| {
                    InstallationError::InspectRoot {
                        path: self.root.clone(),
                        source,
                    }
                })?;
                validate_root_metadata(&self.root, &metadata)
            }
            Err(source) => Err(InstallationError::InspectRoot {
                path: self.root.clone(),
                source,
            }),
        }
    }
}

/// An owning exclusive installation lifecycle lock.
///
/// `fd-lock` guards borrow their lock object. We intentionally forget the
/// acquired guard and retain the owning lock object here: dropping this value
/// closes the underlying OS file handle, which releases the process lock on
/// every supported platform. No method exposes the handle, so it cannot be
/// unlocked while the owner is alive.
#[derive(Debug)]
pub struct InstallationLifecycleLock {
    _lock: fd_lock::RwLock<File>,
    paths: InstallationPaths,
}

impl InstallationLifecycleLock {
    /// Acquire the lock for a long-running daemon.
    pub fn acquire_daemon(paths: &InstallationPaths) -> Result<Self, InstallationError> {
        Self::try_acquire(paths, LifecycleOwner::Daemon)
    }

    /// Acquire the lock for stopped-daemon setup/recovery work.
    pub fn acquire_offline(paths: &InstallationPaths) -> Result<Self, InstallationError> {
        Self::try_acquire(paths, LifecycleOwner::Offline)
    }

    fn try_acquire(
        paths: &InstallationPaths,
        owner: LifecycleOwner,
    ) -> Result<Self, InstallationError> {
        paths.prepare_root()?;
        let lock_path = paths.lifecycle_lock();
        reject_symlink_if_present(&lock_path)?;
        let file = open_private_lock_file(&lock_path)?;
        validate_lock_metadata(&lock_path, &file)?;
        let mut lock = fd_lock::RwLock::new(file);
        let guard = lock
            .try_write()
            .map_err(|source| InstallationError::Locked {
                path: lock_path,
                owner: owner.as_str(),
                source,
            })?;
        std::mem::forget(guard);
        Ok(Self {
            _lock: lock,
            paths: paths.clone(),
        })
    }

    #[must_use]
    pub fn paths(&self) -> &InstallationPaths {
        &self.paths
    }
}

#[derive(Debug, Clone, Copy)]
enum LifecycleOwner {
    Daemon,
    Offline,
}

impl LifecycleOwner {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Offline => "offline operation",
        }
    }
}

#[derive(Debug, Error)]
pub enum InstallationError {
    #[error("neither LABBY_HOME nor HOME is set to an absolute installation root")]
    HomeUnavailable,
    #[error("Labby installation root must be absolute: {0}")]
    RelativeRoot(PathBuf),
    #[error("failed to canonicalize Labby installation root {path}: {source}")]
    CanonicalizeRoot { path: PathBuf, source: io::Error },
    #[error("Labby installation root is not a secure directory: {0}")]
    InsecureRoot(PathBuf),
    #[error("failed to inspect Labby installation root {path}: {source}")]
    InspectRoot { path: PathBuf, source: io::Error },
    #[error("failed to create Labby installation root {path}: {source}")]
    CreateRoot { path: PathBuf, source: io::Error },
    #[error("lifecycle lock path is a symbolic link: {0}")]
    SymlinkLock(PathBuf),
    #[error("failed to open lifecycle lock {path}: {source}")]
    OpenLock { path: PathBuf, source: io::Error },
    #[error("lifecycle lock metadata is insecure: {0}")]
    InsecureLock(PathBuf),
    #[error("cannot start {owner}; installation lifecycle lock {path} is already held: {source}")]
    Locked {
        path: PathBuf,
        owner: &'static str,
        source: io::Error,
    },
}

fn non_empty_env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn validate_root_metadata(path: &Path, metadata: &fs::Metadata) -> Result<(), InstallationError> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(InstallationError::InsecureRoot(path.to_path_buf()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.uid() != nix::unistd::Uid::effective().as_raw() || metadata.mode() & 0o022 != 0
        {
            return Err(InstallationError::InsecureRoot(path.to_path_buf()));
        }
    }
    Ok(())
}

fn canonicalize_allow_missing(path: &Path) -> Result<PathBuf, InstallationError> {
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(existing) {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let name =
                    existing
                        .file_name()
                        .ok_or_else(|| InstallationError::CanonicalizeRoot {
                            path: path.to_path_buf(),
                            source: error,
                        })?;
                missing.push(name.to_os_string());
                existing =
                    existing
                        .parent()
                        .ok_or_else(|| InstallationError::CanonicalizeRoot {
                            path: path.to_path_buf(),
                            source: io::Error::new(io::ErrorKind::NotFound, "no existing ancestor"),
                        })?;
            }
            Err(source) => {
                return Err(InstallationError::CanonicalizeRoot {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }
    }
}

fn normalize_absolute(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::Normal(name) => normalized.push(name),
        }
    }
    normalized
}

fn create_private_dir(path: &Path) -> Result<(), InstallationError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|source| InstallationError::CreateRoot {
            path: path.to_path_buf(),
            source,
        })
}

fn reject_symlink_if_present(path: &Path) -> Result<(), InstallationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(InstallationError::SymlinkLock(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(InstallationError::OpenLock {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn open_private_lock_file(path: &Path) -> Result<File, InstallationError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    options
        .open(path)
        .map_err(|source| InstallationError::OpenLock {
            path: path.to_path_buf(),
            source,
        })
}

fn validate_lock_metadata(path: &Path, file: &File) -> Result<(), InstallationError> {
    let metadata = file
        .metadata()
        .map_err(|source| InstallationError::OpenLock {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.is_file() {
        return Err(InstallationError::InsecureLock(path.to_path_buf()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.uid() != nix::unistd::Uid::effective().as_raw()
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err(InstallationError::InsecureLock(path.to_path_buf()));
        }
    }
    Ok(())
}
