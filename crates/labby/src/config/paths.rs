//! Configuration, workspace, and durable-store path resolution.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{DEFAULT_MCPREGISTRY_URL, LabConfig};

pub fn toml_candidates() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("config.toml")];
    if let Some(home) = home_dir() {
        paths.push(home.join(".labby/config.toml"));
        paths.push(home.join(".config/labby/config.toml"));
    }
    paths
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[must_use]
pub fn mcpregistry_url(config: &LabConfig) -> &str {
    config
        .mcpregistry
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .unwrap_or(DEFAULT_MCPREGISTRY_URL)
}

#[must_use]
pub fn workspace_root_for_home(config: &LabConfig, home: &Path) -> PathBuf {
    config
        .workspace
        .root
        .as_deref()
        .map(|root| expand_home_path(root, home))
        .unwrap_or_else(|| home.join(".labby/stash"))
}

pub fn workspace_root_path(config: &LabConfig) -> Result<PathBuf> {
    let home = home_dir().ok_or_else(|| anyhow::anyhow!("HOME env var not set"))?;
    Ok(workspace_root_for_home(config, &home))
}

fn expand_home_path(path: &Path, home: &Path) -> PathBuf {
    let raw = path.as_os_str().to_string_lossy();
    if raw == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    path.to_path_buf()
}

pub fn dotenv_path() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".labby/.env"))
}

pub fn config_toml_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = super::TEST_CONFIG_TOML_PATH
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .expect("test config path lock")
        .clone()
    {
        return Some(path);
    }
    toml_candidates()
        .into_iter()
        .find(|path| path.exists())
        .or_else(|| home_dir().map(|home| home.join(".config/labby/config.toml")))
}

fn labby_db(name: &str) -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".labby")
        .join(name)
}
pub fn registry_db_path() -> PathBuf {
    labby_db("registry.db")
}
pub fn usage_db_path() -> PathBuf {
    labby_db("usage.db")
}
pub fn codemode_journal_db_path() -> PathBuf {
    labby_db("codemode_journal.db")
}
pub fn codemode_journal_enabled() -> bool {
    std::env::var("LABBY_CODE_MODE_JOURNAL_DISABLED")
        .ok()
        .as_deref()
        != Some("1")
}
pub fn usage_telemetry_enabled() -> bool {
    resolve_usage_telemetry_enabled(
        std::env::var("LABBY_GATEWAY_USAGE_DISABLED")
            .ok()
            .as_deref(),
    )
}
pub(super) fn resolve_usage_telemetry_enabled(raw: Option<&str>) -> bool {
    raw != Some("1")
}
