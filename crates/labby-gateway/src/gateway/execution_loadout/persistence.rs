use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::{ExecutionLoadoutError, ExecutionLoadoutStore};

impl ExecutionLoadoutStore {
    pub(crate) fn load(config_path: &Path) -> Result<Self, ExecutionLoadoutError> {
        let path = store_path(config_path);
        match fs::read(&path) {
            Ok(bytes) => {
                let mut store: Self =
                    serde_json::from_slice(&bytes).map_err(|error| storage_error(&path, error))?;
                store.records = store
                    .records
                    .into_values()
                    .map(|record| {
                        let key = format!("{}\0{}", record.draft.owner_principal, record.draft.id);
                        (key, record)
                    })
                    .collect();
                Ok(store)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(storage_error(&path, error)),
        }
    }

    pub(super) fn persist(&self, config_path: &Path) -> Result<(), ExecutionLoadoutError> {
        let path = store_path(config_path);
        let parent = path
            .parent()
            .ok_or_else(|| ExecutionLoadoutError::Storage {
                message: "execution loadout store has no parent directory".into(),
            })?;
        fs::create_dir_all(parent).map_err(|error| storage_error(&path, error))?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| storage_error(&path, error))?;
        let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| storage_error(&temporary, error))?;
        let result = (|| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &path)?;
            Ok::<_, std::io::Error>(())
        })();
        if result.is_err() {
            drop(fs::remove_file(&temporary));
        }
        result.map_err(|error| storage_error(&path, error))
    }
}

fn store_path(config_path: &Path) -> PathBuf {
    config_path.with_extension("execution-loadouts.json")
}

fn storage_error(path: &Path, error: impl std::fmt::Display) -> ExecutionLoadoutError {
    ExecutionLoadoutError::Storage {
        message: format!("{}: {error}", path.display()),
    }
}
