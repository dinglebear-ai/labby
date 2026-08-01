//! Local stdio-proxy preference and bearer-secret setup.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{ConfigScalarPatch, ConfigScalarValue};
use crate::dispatch::error::ToolError;
use crate::proxy::config::{ProxyAuthMode, ProxyPortPreference, ProxyPreferences};

#[derive(Clone, Deserialize)]
pub struct ProxySetupRequest {
    pub preferences: ProxyPreferences,
    #[serde(default)]
    pub bearer_token: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

impl std::fmt::Debug for ProxySetupRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProxySetupRequest")
            .field("preferences", &self.preferences)
            .field(
                "bearer_token",
                &self.bearer_token.as_ref().map(|_| "<redacted>"),
            )
            .field("dry_run", &self.dry_run)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProxySetupOutcome {
    pub dry_run: bool,
    pub changed: bool,
    pub config_changed: bool,
    pub secret_changed: bool,
    pub config_path: PathBuf,
    pub env_path: PathBuf,
    pub exposure: crate::proxy::config::ProxyExposure,
    pub auth: ProxyAuthMode,
}

pub fn configure(request: ProxySetupRequest) -> Result<ProxySetupOutcome, ToolError> {
    let home = super::client::lab_home();
    configure_at(&home.join("config.toml"), &home.join(".env"), request)
}

pub fn configure_at(
    config_path: &Path,
    env_path: &Path,
    request: ProxySetupRequest,
) -> Result<ProxySetupOutcome, ToolError> {
    request
        .preferences
        .validate()
        .map_err(|error| invalid_proxy_config(error.to_string()))?;
    if request
        .bearer_token
        .as_deref()
        .is_some_and(|token| token.trim().is_empty())
    {
        return Err(invalid_proxy_config(
            "bearer token supplied on stdin must not be empty".to_string(),
        ));
    }

    if request.dry_run {
        return preview_at(config_path, env_path, request);
    }

    apply_at(config_path, env_path, request)
}

fn preview_at(
    config_path: &Path,
    env_path: &Path,
    mut request: ProxySetupRequest,
) -> Result<ProxySetupOutcome, ToolError> {
    let temp = tempfile::tempdir().map_err(io_error)?;
    let temp_config = temp.path().join("config.toml");
    let temp_env = temp.path().join(".env");
    copy_if_present(config_path, &temp_config)?;
    copy_if_present(env_path, &temp_env)?;
    request.dry_run = false;
    let mut outcome = apply_at(&temp_config, &temp_env, request)?;
    outcome.dry_run = true;
    outcome.config_path = config_path.to_path_buf();
    outcome.env_path = env_path.to_path_buf();
    Ok(outcome)
}

fn apply_at(
    config_path: &Path,
    env_path: &Path,
    request: ProxySetupRequest,
) -> Result<ProxySetupOutcome, ToolError> {
    let before_config = read_optional(config_path)?;
    let before_env = read_optional(env_path)?;
    let mut secret_permissions_changed = ensure_secret_parent_permissions(env_path)?;

    if request.preferences.auth == ProxyAuthMode::Bearer {
        let key = request.preferences.bearer_token_env.clone();
        let existing = env_value(env_path, &key)?;
        let token = request
            .bearer_token
            .or(existing)
            .unwrap_or_else(super::token::generate_mcp_token);
        crate::config::env_merge::merge(
            env_path,
            crate::config::env_merge::MergeRequest {
                entries: vec![crate::config::env_merge::EnvEntry::new(key, token)],
                force: true,
                expected_mtime: None,
            },
        )
        .map_err(|error| ToolError::Sdk {
            sdk_kind: error.kind().to_string(),
            message: "failed to store proxy bearer secret".to_string(),
        })?;
        secret_permissions_changed |= ensure_secret_file_permissions(env_path)?;
    }

    crate::config::patch_config_scalars(config_path, &preference_patches(&request.preferences))
        .map_err(|error| {
            invalid_proxy_config(format!("failed to persist proxy config: {error:#}"))
        })?;

    let config_changed = before_config != read_optional(config_path)?;
    let secret_changed = before_env != read_optional(env_path)? || secret_permissions_changed;
    Ok(ProxySetupOutcome {
        dry_run: false,
        changed: config_changed || secret_changed,
        config_changed,
        secret_changed,
        config_path: config_path.to_path_buf(),
        env_path: env_path.to_path_buf(),
        exposure: request.preferences.exposure,
        auth: request.preferences.auth,
    })
}

#[cfg(unix)]
fn ensure_secret_parent_permissions(path: &Path) -> Result<bool, ToolError> {
    use std::os::unix::fs::PermissionsExt as _;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(io_error)?;
    let metadata = std::fs::metadata(parent).map_err(io_error)?;
    let current = metadata.permissions().mode() & 0o777;
    if current == 0o700 {
        return Ok(false);
    }
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    Ok(true)
}

#[cfg(not(unix))]
fn ensure_secret_parent_permissions(path: &Path) -> Result<bool, ToolError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_error)?;
    }
    Ok(false)
}

#[cfg(unix)]
fn ensure_secret_file_permissions(path: &Path) -> Result<bool, ToolError> {
    use std::os::unix::fs::PermissionsExt as _;

    let metadata = std::fs::metadata(path).map_err(io_error)?;
    let current = metadata.permissions().mode() & 0o777;
    if current == 0o600 {
        return Ok(false);
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(io_error)?;
    Ok(true)
}

#[cfg(not(unix))]
fn ensure_secret_file_permissions(_path: &Path) -> Result<bool, ToolError> {
    Ok(false)
}

fn preference_patches(preferences: &ProxyPreferences) -> Vec<ConfigScalarPatch> {
    let exposure = match preferences.exposure {
        crate::proxy::config::ProxyExposure::Tailscale => "tailscale",
        crate::proxy::config::ProxyExposure::Local => "local",
    };
    let auth = match preferences.auth {
        ProxyAuthMode::Tailnet => "tailnet",
        ProxyAuthMode::Bearer => "bearer",
        ProxyAuthMode::Oauth => "oauth",
        ProxyAuthMode::None => "none",
    };
    let port = match preferences.port {
        ProxyPortPreference::Fixed(port) => ConfigScalarValue::I64(i64::from(port)),
        ProxyPortPreference::Mode(_) => ConfigScalarValue::String("random".to_string()),
    };
    vec![
        ConfigScalarPatch::new("proxy.exposure", ConfigScalarValue::String(exposure.into())),
        ConfigScalarPatch::new("proxy.auth", ConfigScalarValue::String(auth.into())),
        ConfigScalarPatch::new(
            "proxy.path",
            ConfigScalarValue::String(preferences.path.clone()),
        ),
        ConfigScalarPatch::new("proxy.port", port),
        ConfigScalarPatch::new(
            "proxy.port_range_start",
            ConfigScalarValue::I64(i64::from(preferences.port_range_start)),
        ),
        ConfigScalarPatch::new(
            "proxy.port_range_end",
            ConfigScalarValue::I64(i64::from(preferences.port_range_end)),
        ),
        ConfigScalarPatch::new(
            "proxy.bearer_token_env",
            ConfigScalarValue::String(preferences.bearer_token_env.clone()),
        ),
        ConfigScalarPatch::new(
            "proxy.oauth_scopes",
            ConfigScalarValue::StringList(preferences.oauth_scopes.clone()),
        ),
        ConfigScalarPatch::new(
            "proxy.inherit_env",
            ConfigScalarValue::StringList(preferences.inherit_env.clone()),
        ),
        ConfigScalarPatch::new(
            "proxy.shutdown_grace_ms",
            ConfigScalarValue::I64(preferences.shutdown_grace_ms as i64),
        ),
    ]
}

fn env_value(path: &Path, key: &str) -> Result<Option<String>, ToolError> {
    if !path.exists() {
        return Ok(None);
    }
    let iter = dotenvy::from_path_iter(path).map_err(|error| ToolError::Sdk {
        sdk_kind: "invalid_config".to_string(),
        message: format!("failed to parse proxy secret file: {error}"),
    })?;
    for item in iter {
        let (candidate, value) = item.map_err(|error| ToolError::Sdk {
            sdk_kind: "invalid_config".to_string(),
            message: format!("failed to parse proxy secret file: {error}"),
        })?;
        if candidate == key && !value.is_empty() {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, ToolError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn copy_if_present(source: &Path, destination: &Path) -> Result<(), ToolError> {
    if source.exists() {
        std::fs::copy(source, destination).map_err(io_error)?;
    }
    Ok(())
}

fn io_error(error: std::io::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: error.to_string(),
    }
}

fn invalid_proxy_config(message: String) -> ToolError {
    ToolError::InvalidParam {
        message,
        param: "proxy".to_string(),
    }
}
