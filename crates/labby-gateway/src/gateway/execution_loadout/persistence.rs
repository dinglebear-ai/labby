use std::fs;
use std::path::{Path, PathBuf};

use fd_lock::RwLock as FileRwLock;

use super::{ExecutionLoadoutError, ExecutionLoadoutStore};

pub(super) struct MutationOutcome<R> {
    pub store: ExecutionLoadoutStore,
    pub result: R,
    pub durability_error: Option<ExecutionLoadoutError>,
}

impl ExecutionLoadoutStore {
    pub(crate) fn load(config_path: &Path) -> Result<Self, ExecutionLoadoutError> {
        load_path(&store_path(config_path)?)
    }

    pub(super) fn mutate<R>(
        config_path: &Path,
        injected_failure: u8,
        mutate: impl FnOnce(&mut Self) -> Result<R, ExecutionLoadoutError>,
    ) -> Result<MutationOutcome<R>, ExecutionLoadoutError> {
        let path = store_path(config_path)?;
        let lock_path = path.with_extension("lock");
        let lock_file = open_private_lock(&lock_path)?;
        let mut lock = FileRwLock::new(lock_file);
        let _guard = lock
            .write()
            .map_err(|error| storage_error(&lock_path, error))?;
        let mut candidate = load_path(&path)?;
        let result = mutate(&mut candidate)?;
        candidate.validate_integrity()?;
        let bytes =
            serde_json::to_vec_pretty(&candidate).map_err(|error| storage_error(&path, error))?;
        if injected_failure == 1 {
            return Err(ExecutionLoadoutError::Storage {
                message: "injected execution loadout persistence failure".into(),
            });
        }
        let write_result = if injected_failure == 2 {
            labby_runtime::secure_atomic_file::write_secure_atomic_with(&path, &bytes, |stage| {
                if stage == labby_runtime::secure_atomic_file::AtomicWriteStage::BeforeParentSync {
                    Err(std::io::Error::other("injected parent sync failure"))
                } else {
                    Ok(())
                }
            })
            .map_err(
                |source| labby_runtime::secure_atomic_file::AtomicWriteError {
                    source,
                    published: true,
                },
            )
        } else {
            labby_runtime::secure_atomic_file::write_secure_atomic(&path, &bytes)
        };
        let durability_error = match write_result {
            Ok(()) => None,
            Err(error) if error.published => Some(ExecutionLoadoutError::Durability {
                message: format!(
                    "{}: parent directory sync failed after publication: {error}",
                    path.display()
                ),
            }),
            Err(error) => return Err(storage_error(&path, error)),
        };
        Ok(MutationOutcome {
            store: candidate,
            result,
            durability_error,
        })
    }
}

fn open_private_lock(path: &Path) -> Result<fs::File, ExecutionLoadoutError> {
    labby_auth::util::open_restricted_lock_file(path).map_err(|error| storage_error(path, error))
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
