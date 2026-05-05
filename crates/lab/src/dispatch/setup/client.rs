//! Path resolution helpers + cached registry views for the `setup`
//! dispatch service.
//!
//! Honors `LAB_HOME` for tests; defaults to `~/.lab/` in production. The
//! registry-derived caches live here so dispatch and secret_mask don't
//! rebuild them on every call.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use lab_apis::core::EnvVar;
use serde::Serialize;
use tokio::process::Command;

use crate::registry::{ToolRegistry, build_default_registry, service_meta};

/// Resolve the lab home directory: `$LAB_HOME` if set, else `$HOME/.lab/`.
#[must_use]
pub fn lab_home() -> PathBuf {
    if let Ok(home) = std::env::var("LAB_HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home);
    }
    let base = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join(".lab")
}

#[must_use]
pub fn env_path() -> PathBuf {
    lab_home().join(".env")
}

#[must_use]
pub fn draft_path() -> PathBuf {
    lab_home().join(".env.draft")
}

// ─── cached registry views ──────────────────────────────────────────────

static CACHED_REGISTRY: OnceLock<ToolRegistry> = OnceLock::new();
static CACHED_SECRET_KEYS: OnceLock<HashSet<&'static str>> = OnceLock::new();
static CACHED_ENV_VAR_INDEX: OnceLock<HashMap<&'static str, &'static EnvVar>> = OnceLock::new();

/// Returns the lazy-initialized default registry. Built once per process.
pub fn cached_registry() -> &'static ToolRegistry {
    CACHED_REGISTRY.get_or_init(build_default_registry)
}

/// Returns a `HashSet` of every env var name where the registered
/// `EnvVar.secret == true`. O(1) lookup replaces the per-call registry walk.
pub fn cached_secret_keys() -> &'static HashSet<&'static str> {
    CACHED_SECRET_KEYS.get_or_init(|| {
        let mut keys = HashSet::new();
        for entry in cached_registry().services() {
            if let Some(meta) = service_meta(entry.name) {
                for var in meta.required_env.iter().chain(meta.optional_env.iter()) {
                    if var.secret {
                        keys.insert(var.name);
                    }
                }
            }
        }
        keys
    })
}

/// Suffixes that mark a key as secret-by-naming-convention. Used by
/// [`super::secret_mask::is_secret_key`] to mask values for keys that are
/// not in the explicit registry — third-party env vars pasted into the
/// draft, or services compiled out via feature flags whose secret flag
/// would otherwise be lost.
pub const SECRET_SUFFIX_DEFAULT_MASK: &[&str] = &["_API_KEY", "_TOKEN", "_PASSWORD", "_SECRET"];

/// Returns `true` when `key` ends with any of [`SECRET_SUFFIX_DEFAULT_MASK`].
#[must_use]
pub fn key_matches_secret_suffix(key: &str) -> bool {
    SECRET_SUFFIX_DEFAULT_MASK
        .iter()
        .any(|suffix| key.ends_with(suffix))
}

/// Returns a `HashMap` from env var name to the registered `EnvVar` declaration.
/// O(1) lookup replaces the per-entry registry rebuild in
/// `validate_against_registry`.
pub fn cached_env_var_index() -> &'static HashMap<&'static str, &'static EnvVar> {
    CACHED_ENV_VAR_INDEX.get_or_init(|| {
        let mut idx = HashMap::new();
        for entry in cached_registry().services() {
            if let Some(meta) = service_meta(entry.name) {
                for var in meta.required_env.iter().chain(meta.optional_env.iter()) {
                    idx.insert(var.name, var);
                }
            }
        }
        idx
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct InstalledPlugin {
    pub id: String,
    pub service: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginLifecycleOutcome {
    pub service: String,
    pub package_id: String,
    pub status: String,
    pub message: String,
}

static INSTALL_LOCKS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn install_locks() -> &'static Mutex<HashSet<String>> {
    INSTALL_LOCKS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn claude_bin() -> String {
    std::env::var("LAB_CLAUDE_BIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "claude".to_string())
}

fn timeout() -> Duration {
    let seconds = std::env::var("LAB_PLUGIN_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(300);
    Duration::from_secs(seconds)
}

fn lab_plugin_org() -> &'static str {
    "lab"
}

pub fn package_for_service(service: &str) -> Result<String, crate::dispatch::error::ToolError> {
    validate_package_segment(service)?;
    let package_id = format!("lab-{service}@{}", lab_plugin_org());
    validate_allowed_org(&package_id)?;
    Ok(package_id)
}

fn validate_package_segment(segment: &str) -> Result<(), crate::dispatch::error::ToolError> {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return invalid_package(segment);
    };
    if !first.is_ascii_alphanumeric() {
        return invalid_package(segment);
    }
    if chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
        Ok(())
    } else {
        invalid_package(segment)
    }
}

fn invalid_package(segment: &str) -> Result<(), crate::dispatch::error::ToolError> {
    Err(crate::dispatch::error::ToolError::InvalidParam {
        message: format!("invalid service/package segment `{segment}`"),
        param: "service".into(),
    })
}

fn validate_allowed_org(package_id: &str) -> Result<(), crate::dispatch::error::ToolError> {
    let Some((_, org)) = package_id.rsplit_once('@') else {
        return Err(crate::dispatch::error::ToolError::InvalidParam {
            message: "plugin package id must include an org segment".into(),
            param: "service".into(),
        });
    };
    let allowed = std::env::var("LAB_PLUGIN_ALLOWLIST").unwrap_or_else(|_| "lab,yourorg".into());
    let allowed = allowed
        .split(',')
        .map(|value| value.trim().trim_start_matches('@'))
        .filter(|value| !value.is_empty());
    if allowed.into_iter().any(|candidate| candidate == org) {
        Ok(())
    } else {
        Err(crate::dispatch::error::ToolError::Sdk {
            sdk_kind: "package_not_allowlisted".into(),
            message: format!("plugin org `{org}` is not allowed"),
        })
    }
}

pub async fn installed_plugins() -> Result<Vec<InstalledPlugin>, crate::dispatch::error::ToolError>
{
    let output = run_claude(&["plugin", "list"]).await?;
    Ok(parse_installed_plugins(&output))
}

pub async fn install_plugin(
    service: &str,
) -> Result<PluginLifecycleOutcome, crate::dispatch::error::ToolError> {
    run_plugin_lifecycle("install", service).await
}

pub async fn uninstall_plugin(
    service: &str,
) -> Result<PluginLifecycleOutcome, crate::dispatch::error::ToolError> {
    run_plugin_lifecycle("uninstall", service).await
}

async fn run_plugin_lifecycle(
    verb: &str,
    service: &str,
) -> Result<PluginLifecycleOutcome, crate::dispatch::error::ToolError> {
    let package_id = package_for_service(service)?;
    acquire_package_lock(&package_id)?;
    let result = async {
        let args = match verb {
            "install" => vec![
                "plugin",
                "install",
                "--scope",
                "user",
                "--",
                package_id.as_str(),
            ],
            "uninstall" => vec!["plugin", "uninstall", "--", package_id.as_str()],
            _ => unreachable!("validated lifecycle verb"),
        };
        let stdout = run_claude(&args).await?;
        Ok(PluginLifecycleOutcome {
            service: service.to_string(),
            package_id: package_id.clone(),
            status: verb.to_string(),
            message: summarize_output(&stdout),
        })
    }
    .await;
    release_package_lock(&package_id);
    result
}

fn acquire_package_lock(package_id: &str) -> Result<(), crate::dispatch::error::ToolError> {
    let mut locks = install_locks()
        .lock()
        .map_err(|_| crate::dispatch::error::ToolError::internal_message("plugin lock poisoned"))?;
    if !locks.insert(package_id.to_string()) {
        return Err(crate::dispatch::error::ToolError::Conflict {
            message: format!("plugin lifecycle already running for `{package_id}`"),
            existing_id: package_id.to_string(),
        });
    }
    Ok(())
}

fn release_package_lock(package_id: &str) {
    if let Ok(mut locks) = install_locks().lock() {
        locks.remove(package_id);
    }
}

async fn run_claude(args: &[&str]) -> Result<String, crate::dispatch::error::ToolError> {
    let bin = claude_bin();
    let mut cmd = Command::new(&bin);
    cmd.args(args);
    let output = tokio::time::timeout(timeout(), cmd.output())
        .await
        .map_err(|_| crate::dispatch::error::ToolError::Sdk {
            sdk_kind: "claude_cli_timeout".into(),
            message: "claude plugin command timed out".into(),
        })?
        .map_err(|e| crate::dispatch::error::ToolError::Sdk {
            sdk_kind: "claude_cli_unavailable".into(),
            message: format!("failed to run `{bin}`: {e}"),
        })?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }

    let stderr = redact_output(&String::from_utf8_lossy(&output.stderr));
    tracing::warn!(
        surface = "dispatch",
        service = "setup",
        action = "plugin.lifecycle",
        stderr = %stderr,
        "claude plugin command failed"
    );
    Err(crate::dispatch::error::ToolError::Sdk {
        sdk_kind: "plugin_command_failed".into(),
        message: summarize_output(&stderr),
    })
}

fn parse_installed_plugins(output: &str) -> Vec<InstalledPlugin> {
    output
        .lines()
        .filter_map(|line| {
            let id = line
                .split_whitespace()
                .map(|token| {
                    token.trim_matches(|ch: char| {
                        ch == ',' || ch == '"' || ch == '\'' || ch == '[' || ch == ']'
                    })
                })
                .find(|token| token.starts_with("lab-") && token.contains('@'))?;
            let service = id
                .strip_prefix("lab-")
                .and_then(|rest| rest.split_once('@').map(|(service, _)| service.to_string()));
            Some(InstalledPlugin {
                id: id.to_string(),
                service,
            })
        })
        .collect()
}

fn summarize_output(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("ok")
        .chars()
        .take(240)
        .collect()
}

fn redact_output(output: &str) -> String {
    let mut redacted = Vec::new();
    for token in output.split_whitespace() {
        if token.eq_ignore_ascii_case("bearer") {
            redacted.push("Bearer".to_string());
            continue;
        }
        if token.contains("://") && token.contains('@') {
            redacted.push(redact_url_userinfo(token));
        } else if token.len() > 32
            && token
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
        {
            redacted.push("<redacted-token>".to_string());
        } else {
            redacted.push(token.to_string());
        }
    }
    redacted.join(" ")
}

fn redact_url_userinfo(token: &str) -> String {
    let Some(scheme_pos) = token.find("://") else {
        return token.to_string();
    };
    let after_scheme = scheme_pos + 3;
    let Some(at_rel) = token[after_scheme..].find('@') else {
        return token.to_string();
    };
    let at_pos = after_scheme + at_rel;
    format!(
        "{}<redacted>@{}",
        &token[..after_scheme],
        &token[at_pos + 1..]
    )
}
