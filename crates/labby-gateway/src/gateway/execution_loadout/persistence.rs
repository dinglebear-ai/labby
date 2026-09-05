use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use fd_lock::RwLock as FileRwLock;

use super::{ExecutionLoadoutError, ExecutionLoadoutStore};

impl ExecutionLoadoutStore {
    pub(crate) fn load(config_path: &Path) -> Result<Self, ExecutionLoadoutError> {
        load_path(&store_path(config_path)?)
    }

    pub(super) fn mutate<R>(
        config_path: &Path,
        fail_before_publish: bool,
        mutate: impl FnOnce(&mut Self) -> Result<R, ExecutionLoadoutError>,
    ) -> Result<(Self, R), ExecutionLoadoutError> {
        let path = store_path(config_path)?;
        let lock_path = path.with_extension("lock");
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| storage_error(&lock_path, error))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600))
                .map_err(|error| storage_error(&lock_path, error))?;
        }
        let mut lock = FileRwLock::new(lock_file);
        let _guard = lock
            .write()
            .map_err(|error| storage_error(&lock_path, error))?;
        let mut candidate = load_path(&path)?;
        let result = mutate(&mut candidate)?;
        candidate.validate_integrity()?;
        let bytes =
            serde_json::to_vec_pretty(&candidate).map_err(|error| storage_error(&path, error))?;
        if fail_before_publish {
            return Err(ExecutionLoadoutError::Storage {
                message: "injected execution loadout persistence failure".into(),
            });
        }
        labby_runtime::secure_atomic_file::write_secure_atomic(&path, &bytes)
            .map_err(|error| storage_error(&path, error))?;
        Ok((candidate, result))
    }
}

fn load_path(path: &Path) -> Result<ExecutionLoadoutStore, ExecutionLoadoutError> {
    match fs::read(path) {
        Ok(bytes) => {
            let store: ExecutionLoadoutStore =
                serde_json::from_slice(&bytes).map_err(|error| storage_error(path, error))?;
            store.validate_integrity()?;
            Ok(store)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ExecutionLoadoutStore::default())
        }
        Err(error) => Err(storage_error(path, error)),
    }
}

fn store_path(config_path: &Path) -> Result<PathBuf, ExecutionLoadoutError> {
    let parent = config_path
        .parent()
        .ok_or_else(|| ExecutionLoadoutError::Storage {
            message: "gateway configuration has no parent directory".into(),
        })?;
    let root = fs::canonicalize(parent).map_err(|error| storage_error(parent, error))?;
    Ok(root.join("execution-loadouts.json"))
}

fn storage_error(path: &Path, error: impl std::fmt::Display) -> ExecutionLoadoutError {
    ExecutionLoadoutError::Storage {
        message: format!("{}: {error}", path.display()),
    }
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
