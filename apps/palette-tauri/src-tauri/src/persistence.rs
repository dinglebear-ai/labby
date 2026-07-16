//! Disk persistence for the palette: reads/writes the palette preferences file
//! (`settings.json`) beside the OAuth credential file in the app config dir.
//!
//! The palette does not manage a `labby serve` instance's `~/.labby/.env` or
//! `config.toml` — that is owned by `labby setup`. This module only persists the
//! desktop app's own preferences (server URL, optional static bearer token,
//! shortcut, theme, and UX toggles).
//!
//! # Atomic writes
//!
//! `settings.json` writes use an atomic rename pattern: write to a per-write
//! unique temp file, fsync, then `rename` to the target. On Unix the target file
//! is created with mode `0o600`.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use tauri::{AppHandle, Manager};

use crate::{LabbySettings, PartialPaletteSettings, SETTINGS_FILE};

pub(crate) async fn run_blocking_io<T, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|err| format!("persistence worker failed: {err}"))?
}

pub(crate) fn read_settings_result(app: &AppHandle) -> Result<PartialPaletteSettings, String> {
    let path = match settings_path(app) {
        Ok(p) => p,
        Err(err) => {
            crate::warn(err);
            return Ok(PartialPaletteSettings::default());
        }
    };
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(PartialPaletteSettings::default());
        }
        Err(err) => {
            return Err(format!(
                "failed to read palette settings at {}: {err}",
                path.display()
            ));
        }
    };
    parse_settings_json(&contents, &path)
}

pub(crate) fn parse_settings_json(
    contents: &str,
    path: &Path,
) -> Result<PartialPaletteSettings, String> {
    serde_json::from_str(contents).map_err(|err| {
        format!(
            "failed to parse palette settings at {}: {err}",
            path.display()
        )
    })
}

pub(crate) fn write_settings(
    app: &AppHandle,
    settings: &LabbySettings,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = settings_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write(&path, serde_json::to_string_pretty(settings)?.as_bytes())?;
    Ok(())
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(SETTINGS_FILE))
        .map_err(|err| format!("failed to resolve app config directory: {err}"))
}

/// Read an environment variable, returning `None` for a missing or blank value.
pub(crate) fn value_for(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Write `data` to `path` atomically: write to a per-write unique temp file,
/// then rename.
///
/// The temp name carries a UUID so two concurrent writers of the same `path`
/// (e.g. a login racing a refresh writing `oauth.json`) do not collide on a
/// fixed `<path>.tmp`. If any step fails the temp file is best-effort removed
/// so unique temps don't accumulate on error.
///
/// On Unix, the temp file is created with mode `0o600` atomically via
/// `OpenOptions::mode`, so it is never world-readable even momentarily (no
/// umask window between `open` and a separate `chmod`). On Windows no explicit
/// permission change is applied; rely on the directory ACL to restrict access.
pub(crate) fn atomic_write(path: &Path, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        {
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);

            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }

            let mut file = opts.open(&tmp)?;

            use std::io::Write;
            file.write_all(data)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    };
    write().inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod async_tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn slow_disk_work_does_not_stall_async_commands() {
        let write = run_blocking_io(|| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            Ok(())
        });
        let timer = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            tokio::time::sleep(std::time::Duration::from_millis(5)),
        );
        let (write_result, timer_result) = tokio::join!(write, timer);
        assert!(write_result.is_ok());
        assert!(timer_result.is_ok(), "disk work stalled the async executor");
    }
}
