//! Cross-process serialization for host config read/modify/write operations.
//! Order: acquire this lock before any sanctioned environment-file lock.

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HostWriteError {
    #[error("host configuration is busy")]
    Busy,
    #[error("unsafe configuration path")]
    UnsafePath,
    #[error("host configuration exceeds size limit")]
    TooLarge,
    #[error("invalid host configuration document")]
    InvalidDocument,
    #[error("host configuration I/O failed")]
    Io,
    #[error("host configuration durability is uncertain")]
    Durability,
}

/// Owns the stable sibling lock across reload, validation, and durable commit.
/// This synchronous helper belongs in blocking work, never around network I/O.
#[derive(Debug)]
pub struct HostConfigLock {
    path: PathBuf,
    _lock_file: File,
}

impl HostConfigLock {
    pub fn acquire(path: &Path) -> Result<Self, HostWriteError> {
        Self::acquire_with_timeout(path, Duration::from_secs(2))
    }

    pub fn acquire_with_timeout(path: &Path, timeout: Duration) -> Result<Self, HostWriteError> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).map_err(|_| HostWriteError::Io)?;
        let parent = parent.canonicalize().map_err(|_| HostWriteError::Io)?;
        let name = path.file_name().ok_or(HostWriteError::UnsafePath)?;
        let path = parent.join(name);
        check_regular_or_missing(&path)?;
        let mut lock_name = name.to_os_string();
        lock_name.push(".lock");
        let lock_path = parent.join(lock_name);
        check_regular_or_missing(&lock_path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        no_follow(&mut options);
        let file = options
            .open(&lock_path)
            .map_err(|_| HostWriteError::UnsafePath)?;
        if !file.metadata().map_err(|_| HostWriteError::Io)?.is_file() {
            return Err(HostWriteError::UnsafePath);
        }
        super::secret_files::restrict_secret_file_permissions(&lock_path)
            .map_err(|_| HostWriteError::Io)?;
        let start = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < timeout => {
                    std::thread::sleep(
                        Duration::from_millis(5).min(timeout.saturating_sub(start.elapsed())),
                    );
                }
                Err(std::fs::TryLockError::WouldBlock) => return Err(HostWriteError::Busy),
                Err(_) => return Err(HostWriteError::Io),
            }
        }
        check_regular_or_missing(&path)?;
        Ok(Self {
            path,
            _lock_file: file,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read_raw(&self) -> Result<String, HostWriteError> {
        let mut options = OpenOptions::new();
        options.read(true);
        no_follow(&mut options);
        let file = match options.open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
            Err(_) => return Err(HostWriteError::Io),
        };
        let metadata = file.metadata().map_err(|_| HostWriteError::Io)?;
        if !metadata.is_file() {
            return Err(HostWriteError::UnsafePath);
        }
        if metadata.len() > MAX_CONFIG_BYTES {
            return Err(HostWriteError::TooLarge);
        }
        let mut raw = String::new();
        file.take(MAX_CONFIG_BYTES + 1)
            .read_to_string(&mut raw)
            .map_err(|_| HostWriteError::Io)?;
        if raw.len() as u64 > MAX_CONFIG_BYTES {
            return Err(HostWriteError::TooLarge);
        }
        Ok(raw)
    }

    pub fn read(&self) -> Result<toml_edit::DocumentMut, HostWriteError> {
        self.read_raw()?
            .parse()
            .map_err(|_| HostWriteError::InvalidDocument)
    }

    pub fn write(&self, raw: &str) -> Result<(), HostWriteError> {
        if raw.len() as u64 > MAX_CONFIG_BYTES {
            return Err(HostWriteError::TooLarge);
        }
        check_regular_or_missing(&self.path)?;
        let parent = self.path.parent().ok_or(HostWriteError::UnsafePath)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| HostWriteError::Io)?;
        super::secret_files::restrict_secret_file_permissions(temp.path())
            .map_err(|_| HostWriteError::Io)?;
        temp.write_all(raw.as_bytes())
            .map_err(|_| HostWriteError::Io)?;
        temp.as_file()
            .sync_all()
            .map_err(|_| HostWriteError::Durability)?;
        temp.persist(&self.path).map_err(|_| HostWriteError::Io)?;
        sync_parent(parent)
    }
}

fn check_regular_or_missing(path: &Path) -> Result<(), HostWriteError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(HostWriteError::UnsafePath),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(HostWriteError::Io),
    }
}

/// Patch only discovery-owned fields of one uniquely identified provider.
/// Authorization, expected-version checks and tombstones belong to dispatch.
pub fn upsert_depot_provider(
    document: &mut toml_edit::DocumentMut,
    provider: &super::depot::ProviderConfig,
) -> Result<(), HostWriteError> {
    provider
        .validate()
        .map_err(|_| HostWriteError::InvalidDocument)?;
    if matches!(provider.id.as_str(), "public" | "all") {
        return Err(HostWriteError::InvalidDocument);
    }
    if document.get("depot").is_none() {
        document["depot"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let depot = document["depot"]
        .as_table_mut()
        .ok_or(HostWriteError::InvalidDocument)?;
    if depot.get("providers").is_none() {
        depot["providers"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
    let entries = depot["providers"]
        .as_array_of_tables_mut()
        .ok_or(HostWriteError::InvalidDocument)?;
    let matching: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            (entry.get("id").and_then(toml_edit::Item::as_str) == Some(provider.id.as_str()))
                .then_some(index)
        })
        .collect();
    if matching.len() > 1 {
        return Err(HostWriteError::InvalidDocument);
    }
    let index = if let Some(index) = matching.first() {
        *index
    } else {
        if entries.len() >= super::depot::MAX_PROVIDERS - 1 {
            return Err(HostWriteError::TooLarge);
        }
        entries.push(toml_edit::Table::new());
        entries.len() - 1
    };
    let entry = entries
        .get_mut(index)
        .ok_or(HostWriteError::InvalidDocument)?;
    entry["id"] = toml_edit::value(&provider.id);
    entry["name"] = toml_edit::value(&provider.name);
    entry["endpoint"] = toml_edit::value(&provider.endpoint);
    entry["enabled"] = toml_edit::value(provider.enabled);
    entry["auth_mode"] = toml_edit::value(match provider.auth_mode {
        super::depot::AuthMode::Anonymous => "anonymous",
        super::depot::AuthMode::Bearer => "bearer",
    });
    if let Some(key) = &provider.bearer_token_env {
        entry["bearer_token_env"] = toml_edit::value(key);
    } else {
        entry.remove("bearer_token_env");
    }
    Ok(())
}

fn no_follow(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
}

#[cfg(unix)]
pub(super) fn sync_parent(parent: &Path) -> Result<(), HostWriteError> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| HostWriteError::Durability)
}

#[cfg(not(unix))]
pub(super) fn sync_parent(_parent: &Path) -> Result<(), HostWriteError> {
    // Windows does not expose a reliable directory fsync equivalent. The
    // temporary file itself is flushed before the atomic replacement above.
    Ok(())
}
