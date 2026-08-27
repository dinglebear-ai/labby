//! Binary-owned setup checks for manually invoked plugin setup commands.
//!
//! This module inspects and repairs local filesystem prerequisites, syncs
//! CLAUDE_PLUGIN_OPTION_* env vars into ~/.labby/.env, exports current .env
//! values as plugin field names, and validates connectivity to the lab MCP
//! server. The plugin no longer installs lifecycle hooks; operators invoke
//! these compatibility commands manually.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::access::{AccessHealthStatus, inspect_health};
use crate::config::env_merge::{self, EnvEntry, MergeRequest};
use crate::dispatch::error::ToolError;

use super::client::{env_path, key_matches_secret_suffix, lab_home};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Check,
    Repair,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SetupCheck {
    pub name: &'static str,
    pub ok: bool,
    pub severity: SetupSeverity,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repaired: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetupSeverity {
    Blocking,
    Advisory,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SetupReport {
    pub exit_policy: &'static str,
    pub ran_repair: bool,
    pub no_repair: bool,
    pub blocking_failures: Vec<String>,
    pub advisory_failures: Vec<String>,
    pub ok: bool,
    pub changed: bool,
    pub mode: &'static str,
    pub checks: Vec<SetupCheck>,
}

/// Mapping from `CLAUDE_PLUGIN_OPTION_<OPTION>` to the LABBY_* env var name.
///
/// Only options that have a direct LABBY_* env var equivalent are listed.
/// `LABBY_SERVER_URL` is the product-owned client target; it does not configure
/// the daemon bind address. `mcp_host`/`mcp_port` are config.toml fields with no
/// env var override, so they're absent.
const PLUGIN_OPTION_MAP: &[(&str, &str)] = &[
    ("CLAUDE_PLUGIN_OPTION_SERVER_URL", "LABBY_SERVER_URL"),
    ("CLAUDE_PLUGIN_OPTION_API_TOKEN", "LABBY_MCP_HTTP_TOKEN"),
    ("CLAUDE_PLUGIN_OPTION_AUTH_MODE", "LABBY_AUTH_MODE"),
    ("CLAUDE_PLUGIN_OPTION_PUBLIC_URL", "LABBY_PUBLIC_URL"),
    (
        "CLAUDE_PLUGIN_OPTION_MCP_GATEWAY_URL",
        "LABBY_MCP_GATEWAY_URL",
    ),
    ("CLAUDE_PLUGIN_OPTION_ADMIN_ENABLED", "LABBY_ADMIN_ENABLED"),
    ("CLAUDE_PLUGIN_OPTION_LOG_FILTER", "LABBY_LOG"),
    ("CLAUDE_PLUGIN_OPTION_LOG_FORMAT", "LABBY_LOG_FORMAT"),
    ("CLAUDE_PLUGIN_OPTION_CORS_ORIGINS", "LABBY_CORS_ORIGINS"),
    (
        "CLAUDE_PLUGIN_OPTION_GOOGLE_CLIENT_ID",
        "LABBY_GOOGLE_CLIENT_ID",
    ),
    (
        "CLAUDE_PLUGIN_OPTION_GOOGLE_CLIENT_SECRET",
        "LABBY_GOOGLE_CLIENT_SECRET",
    ),
    (
        "CLAUDE_PLUGIN_OPTION_AUTH_ADMIN_EMAIL",
        "LABBY_AUTH_ADMIN_EMAIL",
    ),
];

/// Reverse map: LABBY_* env var → plugin userConfig field name, for export.
const ENV_TO_FIELD_MAP: &[(&str, &str, bool)] = &[
    // (lab_env_var, userConfig_field_name, is_sensitive)
    ("LABBY_SERVER_URL", "server_url", false),
    ("LABBY_MCP_HTTP_TOKEN", "api_token", true),
    ("LABBY_AUTH_MODE", "auth_mode", false),
    ("LABBY_PUBLIC_URL", "public_url", false),
    ("LABBY_MCP_GATEWAY_URL", "mcp_gateway_url", false),
    ("LABBY_ADMIN_ENABLED", "admin_enabled", false),
    ("LABBY_LOG", "log_filter", false),
    ("LABBY_LOG_FORMAT", "log_format", false),
    ("LABBY_CORS_ORIGINS", "cors_origins", false),
    ("LABBY_GOOGLE_CLIENT_ID", "google_client_id", false),
    ("LABBY_GOOGLE_CLIENT_SECRET", "google_client_secret", true),
    ("LABBY_AUTH_ADMIN_EMAIL", "auth_admin_email", false),
];

#[derive(Debug, Clone, Serialize)]
pub struct PluginSyncOutcome {
    pub written: usize,
    pub skipped: Vec<String>,
    pub options_found: usize,
    pub env_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginExportEntry {
    pub field: &'static str,
    pub env_var: &'static str,
    pub value: Option<String>,
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginExportOutcome {
    pub fields: Vec<PluginExportEntry>,
    pub env_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectivityOutcome {
    pub server_url: String,
    pub reachable: bool,
    pub latency_ms: Option<u64>,
    /// Status from the endpoint named by `status_source`.
    pub status: Option<u16>,
    pub status_source: Option<&'static str>,
    /// Status returned by `/health`, whenever that endpoint responded.
    pub health_status: Option<u16>,
    /// Status returned by `/mcp` when the fallback probe received a response.
    pub mcp_status: Option<u16>,
    /// Stable reason the MCP fallback failed, when it was attempted.
    pub mcp_failure: Option<&'static str>,
    pub message: String,
}

/// Composite result for the `plugin_hook` orchestration action.
///
/// `setup` is always present. `sync` is `None` in Check mode (non-mutating).
/// `connectivity` is always probed since it is read-only.
#[derive(Debug, Clone, Serialize)]
pub struct PluginHookReport {
    pub setup: SetupReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync: Option<PluginSyncOutcome>,
    pub connectivity: ConnectivityOutcome,
}

/// Check or repair the local filesystem prerequisites used by plugin setup.
pub fn run(mode: Mode) -> Result<SetupReport, ToolError> {
    let access_store = crate::config::access_db_path().map_err(|_| ToolError::Sdk {
        sdk_kind: "setup_check_failed".into(),
        message: "unable to resolve the access store path".to_string(),
    })?;
    run_for_paths(mode, lab_home(), env_path(), access_store)
}

/// Sync CLAUDE_PLUGIN_OPTION_* env vars into ~/.labby/.env.
///
/// Only non-empty options are written; existing .env values are preserved
/// when the corresponding option var is absent or empty.
pub fn sync_plugin_env() -> Result<PluginSyncOutcome, ToolError> {
    sync_plugin_env_to(env_path())
}

pub fn sync_plugin_env_to(env: PathBuf) -> Result<PluginSyncOutcome, ToolError> {
    let entries = plugin_entries_from(|option_var| std::env::var(option_var).ok());
    sync_plugin_entries_to(env, entries)
}

fn plugin_entries_from(mut read: impl FnMut(&str) -> Option<String>) -> Vec<EnvEntry> {
    PLUGIN_OPTION_MAP
        .iter()
        .filter_map(|(option_var, lab_var)| {
            let value = read(option_var).filter(|value| !value.trim().is_empty())?;
            let entry = EnvEntry::new(lab_var.to_string(), value);
            Some(if *lab_var == "LABBY_SERVER_URL" {
                entry.force()
            } else {
                entry
            })
        })
        .collect()
}

fn sync_plugin_entries_to(
    env: PathBuf,
    entries: Vec<EnvEntry>,
) -> Result<PluginSyncOutcome, ToolError> {
    let options_found = entries.len();
    if options_found == 0 {
        return Ok(PluginSyncOutcome {
            written: 0,
            skipped: vec![],
            options_found: 0,
            env_path: env.display().to_string(),
        });
    }

    // Ensure ~/.labby/ and ~/.labby/.env exist before merging.
    if let Some(parent) = env.parent() {
        fs::create_dir_all(parent).map_err(|e| ToolError::Sdk {
            sdk_kind: "setup_repair_failed".into(),
            message: format!("failed to create {}: {e}", parent.display()),
        })?;
    }
    if !env.exists() {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&env)
            .map_err(|e| ToolError::Sdk {
                sdk_kind: "setup_repair_failed".into(),
                message: format!("failed to create {}: {e}", env.display()),
            })?;
    }

    let outcome = merge_plugin_entries(&env, entries)?;

    Ok(PluginSyncOutcome {
        written: outcome.written,
        skipped: outcome.skipped,
        options_found,
        env_path: env.display().to_string(),
    })
}

fn merge_plugin_entries(
    env: &Path,
    entries: Vec<EnvEntry>,
) -> Result<env_merge::MergeOutcome, ToolError> {
    let expected_mtime = env_merge::snapshot_mtime(env);
    env_merge::merge(
        env,
        MergeRequest {
            entries,
            force: false,
            expected_mtime,
        },
    )
    .map_err(|error| ToolError::Sdk {
        sdk_kind: error.kind().to_string(),
        message: error.to_string(),
    })
}

/// Read ~/.labby/.env and return current values keyed by userConfig field name.
/// Sensitive values are redacted to `"***"`.
pub fn export_plugin_env() -> Result<PluginExportOutcome, ToolError> {
    export_plugin_env_from(env_path())
}

pub fn export_plugin_env_from(env: PathBuf) -> Result<PluginExportOutcome, ToolError> {
    let raw = if env.exists() {
        fs::read_to_string(&env).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("failed to read {}: {e}", env.display()),
        })?
    } else {
        String::new()
    };

    // Parse key=value pairs from the env file.
    let mut env_map: HashMap<&str, String> = HashMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            env_map.insert(k.trim(), env_merge::strip_quotes(v.trim()));
        }
    }

    let fields = ENV_TO_FIELD_MAP
        .iter()
        .map(|(lab_var, field, sensitive)| {
            let raw_value = env_map.get(*lab_var).cloned();
            let value = raw_value.map(|v| {
                if *sensitive || key_matches_secret_suffix(lab_var) {
                    "***".to_string()
                } else {
                    v
                }
            });
            PluginExportEntry {
                field,
                env_var: lab_var,
                value,
                sensitive: *sensitive,
            }
        })
        .collect();

    Ok(PluginExportOutcome {
        fields,
        env_path: env.display().to_string(),
    })
}

/// Validate connectivity to the lab MCP server at `{server_url}/health`.
///
/// Selects `CLAUDE_PLUGIN_OPTION_SERVER_URL`, then `LABBY_SERVER_URL`, then
/// loopback. An optional requested URL must match that active origin.
/// Non-blocking: a failed probe is reported as `reachable: false`, not an error.
pub async fn validate_connectivity(server_url: Option<&str>) -> ConnectivityOutcome {
    let plugin_target = std::env::var("CLAUDE_PLUGIN_OPTION_SERVER_URL").ok();
    let product_target = std::env::var("LABBY_SERVER_URL").ok();
    validate_connectivity_with_targets(
        server_url,
        plugin_target.as_deref(),
        product_target.as_deref(),
    )
    .await
}

async fn validate_connectivity_with_targets(
    server_url: Option<&str>,
    plugin_target: Option<&str>,
    product_target: Option<&str>,
) -> ConnectivityOutcome {
    let configured = resolve_connectivity_target(plugin_target, product_target);
    let requested = server_url.filter(|value| !value.trim().is_empty());
    let (base, health_url) = match validated_connectivity_target(requested, &configured) {
        Ok(target) => target,
        Err(message) => {
            return ConnectivityOutcome {
                server_url: "<rejected>".to_string(),
                reachable: false,
                latency_ms: None,
                status: None,
                status_source: None,
                health_status: None,
                mcp_status: None,
                mcp_failure: None,
                message,
            };
        }
    };

    // See api/state.rs::build_protected_mcp_http_client for why this call is
    // needed under "rustls-no-provider" -- idempotent, safe to ignore Err.
    drop(rustls::crypto::ring::default_provider().install_default());
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return ConnectivityOutcome {
                server_url: base.clone(),
                reachable: false,
                latency_ms: None,
                status: None,
                status_source: None,
                health_status: None,
                mcp_status: None,
                mcp_failure: None,
                message: format!("failed to build HTTP client: {e}"),
            };
        }
    };

    let start = std::time::Instant::now();
    match client.get(health_url).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let mut mcp_status = None;
            let mut mcp_failure = None;
            if !(200..300).contains(&status) {
                let mcp_url = url::Url::parse(&format!("{base}/mcp"));
                match mcp_url {
                    Ok(mcp_url) => match client.get(mcp_url).send().await {
                        Ok(mcp_response) => {
                            let response_status = mcp_response.status().as_u16();
                            mcp_status = Some(response_status);
                            let has_bearer = mcp_response
                                .headers()
                                .get_all(reqwest::header::WWW_AUTHENTICATE)
                                .iter()
                                .filter_map(|value| value.to_str().ok())
                                .any(has_bearer_challenge);
                            if mcp_fallback_is_reachable(status, response_status, has_bearer) {
                                let latency = start.elapsed().as_millis() as u64;
                                return ConnectivityOutcome {
                                    server_url: base,
                                    reachable: true,
                                    latency_ms: Some(latency),
                                    status: Some(response_status),
                                    status_source: Some("mcp"),
                                    health_status: Some(status),
                                    mcp_status: Some(response_status),
                                    mcp_failure: None,
                                    message: format!(
                                        "MCP endpoint reachable ({response_status}) in {latency}ms; health returned {status}"
                                    ),
                                };
                            }
                            mcp_failure = Some(if response_status == 401 {
                                "missing_bearer_challenge"
                            } else {
                                "unexpected_status"
                            });
                        }
                        Err(_) => mcp_failure = Some("transport_error"),
                    },
                    Err(_) => mcp_failure = Some("invalid_url"),
                }
            }
            let latency = start.elapsed().as_millis() as u64;
            ConnectivityOutcome {
                server_url: base.clone(),
                reachable: (200..300).contains(&status),
                latency_ms: Some(latency),
                status: Some(status),
                status_source: Some("health"),
                health_status: Some(status),
                mcp_status,
                mcp_failure,
                message: if let Some(reason) = mcp_failure {
                    format!(
                        "health returned {status}; MCP fallback failed ({reason}) in {latency}ms"
                    )
                } else {
                    format!("health returned {status} in {latency}ms")
                },
            }
        }
        Err(e) => ConnectivityOutcome {
            server_url: base,
            reachable: false,
            latency_ms: None,
            status: None,
            status_source: None,
            health_status: None,
            mcp_status: None,
            mcp_failure: None,
            message: format!("unreachable: {e}"),
        },
    }
}

fn mcp_fallback_is_reachable(
    health_status: u16,
    mcp_status: u16,
    has_bearer_challenge: bool,
) -> bool {
    !(200..300).contains(&health_status) && mcp_status == 401 && has_bearer_challenge
}

fn has_bearer_challenge(value: &str) -> bool {
    value
        .split(|character: char| character.is_ascii_whitespace() || character == ',')
        .any(|part| part.eq_ignore_ascii_case("bearer"))
}

fn resolve_connectivity_target(
    plugin_target: Option<&str>,
    product_target: Option<&str>,
) -> String {
    plugin_target
        .filter(|value| !value.trim().is_empty())
        .or_else(|| product_target.filter(|value| !value.trim().is_empty()))
        .unwrap_or("http://localhost:40100")
        .to_string()
}

/// Resolve the connectivity probe to the single operator-configured origin.
///
/// This is deliberately an allow-list policy rather than a general-purpose
/// URL fetcher: the setup probe has no reason to contact an arbitrary host.
/// A requested URL is accepted only when it normalizes to exactly the same
/// origin as the configured plugin target. Redirects are disabled by the
/// caller, so a trusted origin cannot bounce the probe into metadata/LAN space.
fn validated_connectivity_target(
    requested: Option<&str>,
    configured: &str,
) -> Result<(String, url::Url), String> {
    let configured = normalize_connectivity_base(configured)?;
    let requested = normalize_connectivity_base(requested.unwrap_or(configured.as_str()))?;
    if requested != configured {
        return Err(
            "connectivity target must match the configured plugin server origin".to_string(),
        );
    }
    let health = url::Url::parse(&format!("{configured}/health"))
        .map_err(|_| "configured plugin server URL is invalid".to_string())?;
    Ok((configured, health))
}

fn normalize_connectivity_base(raw: &str) -> Result<String, String> {
    let parsed = url::Url::parse(raw.trim())
        .map_err(|_| "plugin server URL must be an absolute http(s) URL".to_string())?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            "plugin server URL must not include credentials, query, or fragment".to_string(),
        );
    }
    let host = parsed
        .host()
        .ok_or_else(|| "plugin server URL must include a host".to_string())?;
    let loopback = match host {
        url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    };
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback) {
        return Err(
            "plugin server URL must use https (http is allowed only on loopback)".to_string(),
        );
    }
    if !matches!(parsed.path(), "" | "/" | "/mcp" | "/mcp/") {
        return Err("plugin server URL path must be `/` or `/mcp`".to_string());
    }

    let mut base = parsed;
    base.set_path("");
    base.set_query(None);
    base.set_fragment(None);
    Ok(base.as_str().trim_end_matches('/').to_ascii_lowercase())
}

fn run_for_paths(
    mode: Mode,
    lab_home: PathBuf,
    env: PathBuf,
    access_store: PathBuf,
) -> Result<SetupReport, ToolError> {
    let mut checks = Vec::with_capacity(3);
    let mut changed = false;

    checks.push(check_lab_home(mode, &lab_home, &mut changed)?);
    checks.push(check_env_file(mode, &env, &mut changed)?);
    checks.push(check_access_store(&access_store));

    let blocking_failures = checks
        .iter()
        .filter(|check| !check.ok && check.severity == SetupSeverity::Blocking)
        .map(|check| check.name.to_string())
        .collect::<Vec<_>>();
    let advisory_failures = checks
        .iter()
        .filter(|check| !check.ok && check.severity == SetupSeverity::Advisory)
        .map(|check| check.name.to_string())
        .collect::<Vec<_>>();
    let exit_policy = if !blocking_failures.is_empty() {
        "blocking_failure"
    } else if !advisory_failures.is_empty() {
        "advisory_failure"
    } else {
        "success"
    };

    Ok(SetupReport {
        exit_policy,
        ran_repair: mode == Mode::Repair,
        no_repair: mode == Mode::Check,
        ok: blocking_failures.is_empty(),
        changed,
        mode: match mode {
            Mode::Check => "check",
            Mode::Repair => "repair",
        },
        blocking_failures,
        advisory_failures,
        checks,
    })
}

fn check_access_store(path: &Path) -> SetupCheck {
    let health = inspect_health(path);
    let (ok, severity) = match health.status {
        AccessHealthStatus::Ready => (true, SetupSeverity::Advisory),
        AccessHealthStatus::Missing
        | AccessHealthStatus::Uninitialized
        | AccessHealthStatus::Prepared => (false, SetupSeverity::Advisory),
        AccessHealthStatus::Insecure
        | AccessHealthStatus::Corrupt
        | AccessHealthStatus::NewerSchema
        | AccessHealthStatus::Locked
        | AccessHealthStatus::ReadOnly
        | AccessHealthStatus::Unavailable => (false, SetupSeverity::Blocking),
    };
    SetupCheck {
        name: "access_store",
        ok,
        severity,
        path: path.display().to_string(),
        repaired: None,
        message: (!ok).then(|| health.detail.to_string()),
    }
}

fn check_lab_home(mode: Mode, path: &Path, changed: &mut bool) -> Result<SetupCheck, ToolError> {
    if path.is_dir() {
        return Ok(ok_check("lab_home", path, None));
    }
    if path.exists() {
        return Ok(failed_check(
            "lab_home",
            path,
            SetupSeverity::Blocking,
            "path exists but is not a directory",
        ));
    }
    if mode == Mode::Repair {
        create_lab_home(path).map_err(|error| io_error("lab_home", path, error))?;
        *changed = true;
        return Ok(ok_check("lab_home", path, Some(true)));
    }
    Ok(failed_check(
        "lab_home",
        path,
        SetupSeverity::Blocking,
        "directory is missing",
    ))
}

#[cfg(unix)]
fn create_lab_home(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_lab_home(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)
}

fn check_env_file(mode: Mode, path: &Path, changed: &mut bool) -> Result<SetupCheck, ToolError> {
    if path.is_file() {
        return Ok(ok_check("env_file", path, None));
    }
    if path.exists() {
        return Ok(failed_check(
            "env_file",
            path,
            SetupSeverity::Blocking,
            "path exists but is not a regular file",
        ));
    }
    if mode == Mode::Repair {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error("env_file", parent, error))?;
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| io_error("env_file", path, error))?;
        *changed = true;
        return Ok(ok_check("env_file", path, Some(true)));
    }
    Ok(failed_check(
        "env_file",
        path,
        SetupSeverity::Advisory,
        "file is missing; process env can supply setup values",
    ))
}

fn ok_check(name: &'static str, path: &Path, repaired: Option<bool>) -> SetupCheck {
    SetupCheck {
        name,
        ok: true,
        severity: SetupSeverity::Advisory,
        path: path.display().to_string(),
        repaired,
        message: None,
    }
}

fn failed_check(
    name: &'static str,
    path: &Path,
    severity: SetupSeverity,
    message: &'static str,
) -> SetupCheck {
    SetupCheck {
        name,
        ok: false,
        severity,
        path: path.display().to_string(),
        repaired: None,
        message: Some(message.to_string()),
    }
}

fn io_error(check: &'static str, path: &Path, error: std::io::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "setup_repair_failed".into(),
        message: format!("failed to repair {check} at {}: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn secure(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("secure permissions");
    }

    #[cfg(not(unix))]
    fn secure(_path: &Path, _mode: u32) {}

    #[test]
    fn connectivity_target_precedence_ignores_blank_values() {
        assert_eq!(
            resolve_connectivity_target(
                Some("https://plugin.example"),
                Some("https://product.example"),
            ),
            "https://plugin.example"
        );
        assert_eq!(
            resolve_connectivity_target(Some("  "), Some("https://product.example")),
            "https://product.example"
        );
        assert_eq!(
            resolve_connectivity_target(Some(""), Some("")),
            "http://localhost:40100"
        );
        assert_eq!(
            resolve_connectivity_target(None, None),
            "http://localhost:40100"
        );
    }

    #[test]
    fn plugin_server_url_is_persisted_as_the_product_client_target() {
        assert!(
            PLUGIN_OPTION_MAP.contains(&("CLAUDE_PLUGIN_OPTION_SERVER_URL", "LABBY_SERVER_URL",))
        );
        assert!(ENV_TO_FIELD_MAP.contains(&("LABBY_SERVER_URL", "server_url", false,)));
    }

    #[test]
    fn plugin_sync_replaces_stale_server_url_and_preserves_unrelated_content() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env = temp.path().join(".env");
        fs::write(
            &env,
            "# keep this comment\nUNRELATED=value\nLABBY_SERVER_URL=https://old.example\nLABBY_AUTH_MODE=oauth\n",
        )
        .expect("seed env");
        let entries = plugin_entries_from(|key| match key {
            "CLAUDE_PLUGIN_OPTION_SERVER_URL" => Some("https://new.example".to_string()),
            "CLAUDE_PLUGIN_OPTION_AUTH_MODE" => Some("bearer".to_string()),
            _ => None,
        });

        let outcome = sync_plugin_entries_to(env.clone(), entries).expect("sync plugin env");

        assert_eq!(outcome.written, 1);
        assert_eq!(outcome.skipped.len(), 1);
        let contents = fs::read_to_string(&env).expect("read env");
        assert!(contents.contains("# keep this comment"));
        assert!(contents.contains("UNRELATED=value"));
        assert!(contents.contains("LABBY_SERVER_URL=https://new.example"));
        assert!(contents.contains("LABBY_AUTH_MODE=oauth"));
        assert!(!contents.contains("LABBY_AUTH_MODE=bearer"));
        assert!(!contents.contains("https://old.example"));
        let export = export_plugin_env_from(env).expect("export plugin env");
        let server_url = export
            .fields
            .iter()
            .find(|entry| entry.field == "server_url")
            .expect("server_url export");
        assert_eq!(server_url.value.as_deref(), Some("https://new.example"));
        assert!(!server_url.sensitive);
    }

    #[test]
    fn oauth_challenge_proves_mcp_reachability_when_health_is_unpublished() {
        assert!(mcp_fallback_is_reachable(404, 401, true));
        assert!(mcp_fallback_is_reachable(502, 401, true));
        assert!(!mcp_fallback_is_reachable(502, 401, false));
        assert!(!mcp_fallback_is_reachable(502, 502, false));
        assert!(!mcp_fallback_is_reachable(502, 404, false));
        assert!(!mcp_fallback_is_reachable(204, 401, true));
    }

    #[test]
    fn bearer_challenge_scheme_is_case_insensitive() {
        assert!(has_bearer_challenge("Bearer scope=\"lab\""));
        assert!(has_bearer_challenge("bearer scope=\"lab\""));
        assert!(has_bearer_challenge(
            "Basic realm=\"lab\", Bearer scope=\"lab\""
        ));
        assert!(!has_bearer_challenge("Basic realm=\"lab\""));
    }

    #[test]
    fn plugin_manifest_defaults_to_dookie_host_proxy() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../plugins/labby/.claude-plugin/plugin.json"
        ))
        .expect("plugin manifest");

        assert_eq!(
            manifest["userConfig"]["server_url"]["default"],
            "http://localhost:40100"
        );
    }

    #[tokio::test]
    async fn unconfigured_request_cannot_replace_the_loopback_default() {
        let outcome =
            validate_connectivity_with_targets(Some("https://169.254.169.254"), None, None).await;

        assert!(!outcome.reachable);
        assert_eq!(outcome.server_url, "<rejected>");
        assert!(outcome.message.contains("configured plugin server origin"));
    }

    #[tokio::test]
    async fn connectivity_probe_uses_authenticated_mcp_fallback_after_health_failure() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(401).insert_header(
                "WWW-Authenticate",
                "Basic realm=\"lab\", bearer scope=\"lab\"",
            ))
            .expect(1)
            .mount(&server)
            .await;

        let outcome = validate_connectivity_with_targets(None, None, Some(&server.uri())).await;

        assert!(outcome.reachable);
        assert_eq!(outcome.status, Some(401));
        assert_eq!(outcome.status_source, Some("mcp"));
        assert_eq!(outcome.health_status, Some(404));
        assert_eq!(outcome.mcp_status, Some(401));
        assert_eq!(outcome.mcp_failure, None);
        assert!(outcome.message.contains("health returned 404"));
    }

    #[tokio::test]
    async fn failed_mcp_fallback_is_reported_structurally() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(302))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/mcp"))
            .respond_with(
                ResponseTemplate::new(401).insert_header("WWW-Authenticate", "Basic realm=\"lab\""),
            )
            .expect(1)
            .mount(&server)
            .await;

        let outcome = validate_connectivity_with_targets(None, None, Some(&server.uri())).await;

        assert!(!outcome.reachable);
        assert_eq!(outcome.status, Some(302));
        assert_eq!(outcome.status_source, Some("health"));
        assert_eq!(outcome.mcp_status, Some(401));
        assert_eq!(outcome.mcp_failure, Some("missing_bearer_challenge"));
    }

    #[test]
    fn connectivity_target_is_pinned_to_configured_origin() {
        let (base, health) = validated_connectivity_target(
            Some("https://lab.example.com/mcp"),
            "https://lab.example.com",
        )
        .expect("same configured origin");
        assert_eq!(base, "https://lab.example.com");
        assert_eq!(health.as_str(), "https://lab.example.com/health");

        for attacker_url in [
            "http://169.254.169.254/latest/meta-data",
            "http://127.0.0.1:2375",
            "http://10.0.0.5:8080",
            "https://other.example.com",
        ] {
            assert!(
                validated_connectivity_target(Some(attacker_url), "https://lab.example.com")
                    .is_err(),
                "{attacker_url} must not override the configured origin"
            );
        }
    }

    #[test]
    fn connectivity_target_allows_only_https_or_loopback_http() {
        assert!(normalize_connectivity_base("http://localhost:8765/mcp").is_ok());
        assert!(normalize_connectivity_base("http://127.0.0.1:8765").is_ok());
        assert!(normalize_connectivity_base("http://[::1]:8765").is_ok());
        assert!(normalize_connectivity_base("https://lab.example.com").is_ok());

        for blocked in [
            "http://10.0.0.5:8765",
            "http://169.254.169.254",
            "https://user:password@lab.example.com",
            "https://lab.example.com?next=http://169.254.169.254",
            "https://lab.example.com/redirect",
        ] {
            assert!(normalize_connectivity_base(blocked).is_err(), "{blocked}");
        }
    }

    #[test]
    fn check_reports_missing_paths_without_creating_them() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        let env = home.join(".env");

        let report = run_for_paths(
            Mode::Check,
            home.clone(),
            env.clone(),
            home.join("access.db"),
        )
        .expect("check report");

        assert!(!report.ok);
        assert!(!report.changed);
        assert_eq!(report.exit_policy, "blocking_failure");
        assert!(report.no_repair);
        assert!(!report.ran_repair);
        assert_eq!(report.blocking_failures, ["lab_home"]);
        assert_eq!(report.advisory_failures, ["env_file", "access_store"]);
        assert!(!home.exists());
        assert!(!env.exists());
        assert_eq!(report.checks.len(), 3);
        assert_eq!(report.checks[0].name, "lab_home");
        assert_eq!(report.checks[1].name, "env_file");
        assert_eq!(report.checks[2].name, "access_store");
    }

    #[test]
    fn repair_creates_lab_home_and_env_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        let env = home.join(".env");

        let report = run_for_paths(
            Mode::Repair,
            home.clone(),
            env.clone(),
            home.join("access.db"),
        )
        .expect("repair report");

        assert!(report.ok);
        assert!(report.changed);
        assert_eq!(report.exit_policy, "advisory_failure");
        assert!(report.ran_repair);
        assert!(!report.no_repair);
        assert!(report.blocking_failures.is_empty());
        assert_eq!(report.advisory_failures, ["access_store"]);
        assert!(home.is_dir());
        assert!(env.is_file());
        assert!(report.checks[..2].iter().all(|check| check.ok));
        assert_eq!(report.checks[0].repaired, Some(true));
        assert_eq!(report.checks[1].repaired, Some(true));
        assert_eq!(report.checks[2].repaired, None);
    }

    #[test]
    fn repair_is_idempotent_after_paths_exist() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        let env = home.join(".env");
        fs::create_dir_all(&home).expect("lab home");
        secure(&home, 0o700);
        fs::write(&env, "APPRISE_URL=http://localhost\n").expect("env file");

        let access = home.join("access.db");
        let report = run_for_paths(Mode::Repair, home, env, access).expect("repair report");

        assert!(report.ok);
        assert!(!report.changed);
        assert!(report.checks.iter().all(|check| check.repaired.is_none()));
    }

    #[test]
    fn repair_never_mutates_an_uninitialized_access_store() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        let env = home.join(".env");
        let access = home.join("access.db");
        fs::create_dir_all(&home).expect("lab home");
        secure(&home, 0o700);
        fs::write(&env, "").expect("env file");
        fs::write(&access, b"").expect("access store");
        secure(&access, 0o600);

        let before = fs::metadata(&access).expect("metadata");
        let report = run_for_paths(Mode::Repair, home, env, access.clone()).expect("repair report");
        let after = fs::metadata(&access).expect("metadata");

        assert!(report.ok);
        assert!(!report.changed);
        assert_eq!(report.advisory_failures, ["access_store"]);
        assert_eq!(report.checks[2].repaired, None);
        assert_eq!(fs::read(&access).expect("access bytes"), b"");
        assert_eq!(before.permissions(), after.permissions());
        assert_eq!(before.len(), after.len());
    }

    #[test]
    fn corrupt_access_store_is_blocking_and_remains_unrepaired() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        let env = home.join(".env");
        let access = home.join("access.db");
        fs::create_dir_all(&home).expect("lab home");
        secure(&home, 0o700);
        fs::write(&env, "").expect("env file");
        fs::write(&access, b"not a sqlite database").expect("access store");
        secure(&access, 0o600);
        let before = fs::read(&access).expect("access bytes");

        let report = run_for_paths(Mode::Repair, home, env, access.clone()).expect("repair report");

        assert!(!report.ok);
        assert_eq!(report.exit_policy, "blocking_failure");
        assert_eq!(report.blocking_failures, ["access_store"]);
        assert_eq!(report.checks[2].repaired, None);
        assert_eq!(fs::read(&access).expect("access bytes"), before);
    }
}
