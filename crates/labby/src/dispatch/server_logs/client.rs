use std::path::{Path, PathBuf};

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::redact_home;

#[derive(Debug, Clone)]
pub(super) struct LogFile {
    pub path: PathBuf,
    pub name: String,
    pub bytes: u64,
    pub modified_unix_ms: Option<u128>,
}

pub(super) fn log_dir() -> PathBuf {
    if let Ok(path) = std::env::var("LABBY_LOG_DIR")
        && !path.trim().is_empty()
    {
        return PathBuf::from(path);
    }

    crate::config::config_toml_path()
        .and_then(|path| crate::config::load_toml(&[path]).ok())
        .and_then(|config| config.log.dir)
        .unwrap_or_else(default_log_dir)
}

pub(super) fn display_path(path: &Path) -> String {
    redact_home(&path.display().to_string())
}

pub(super) fn log_files(dir: &Path) -> Result<Vec<LogFile>, ToolError> {
    let read_dir = std::fs::read_dir(dir).map_err(|err| ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!(
            "server log directory `{}` is not readable: {err}",
            dir.display()
        ),
    })?;

    let mut files = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to read server log directory entry: {err}"),
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!(
                "failed to inspect server log file `{}`: {err}",
                display_path(&path)
            ),
        })?;
        if !file_type.is_file() || !looks_like_lab_log(&path) {
            continue;
        }
        let metadata = entry.metadata().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!(
                "failed to stat server log file `{}`: {err}",
                display_path(&path)
            ),
        })?;
        let modified_unix_ms = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis());
        files.push(LogFile {
            name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string(),
            path,
            bytes: metadata.len(),
            modified_unix_ms,
        });
    }

    files.sort_by(|a, b| {
        a.modified_unix_ms
            .cmp(&b.modified_unix_ms)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(files)
}

fn default_log_dir() -> PathBuf {
    std::env::var("HOME").map_or_else(
        |_| std::env::temp_dir().join("labby").join("logs"),
        |home| PathBuf::from(home).join(".local/share/labby/logs"),
    )
}

fn looks_like_lab_log(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.starts_with("lab") && name.ends_with(".log")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn log_files_skip_symlinked_lab_logs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("lab.real.log");
        let target = dir.path().join("outside.log");
        let link = dir.path().join("lab.link.log");
        std::fs::write(&real, "real\n").expect("write real");
        std::fs::write(&target, "secret\n").expect("write target");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        let files = log_files(dir.path()).expect("log files");

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "lab.real.log");
    }
}
