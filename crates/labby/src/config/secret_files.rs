//! Cross-platform ownership policy for secret-bearing configuration files.

use std::fs::OpenOptions;
use std::path::Path;

pub(super) fn open_secret_file(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
    }
    #[cfg(windows)]
    {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        labby_auth::util::harden_secret_file(path)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(file)
    }
}

pub(super) fn restrict_secret_file_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    labby_auth::util::harden_secret_file(path)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(())
}

/// Heal the environment file and every retained backup before secrets load.
#[allow(dead_code)]
pub fn heal_env_file_permissions(path: &Path) {
    heal_one_file(path);
    let Some(parent) = path.parent() else { return };
    let Some(stem) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let prefix = format!("{stem}.bak.");
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.filter_map(Result::ok) {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix))
            {
                heal_one_file(&entry.path());
            }
        }
    }
}

fn heal_one_file(path: &Path) {
    if !path.exists() {
        return;
    }
    if let Err(error) = restrict_secret_file_permissions(path) {
        tracing::warn!(path = %path.display(), error = %error, "failed to tighten secret file permissions");
    }
}
