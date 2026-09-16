//! Config loading for the `lab` binary.
//!
//! Order of precedence (highest wins):
//!   1. CLI flags / process environment variables
//!   2. `$LABBY_HOME/.env` (normally `~/.labby/.env`, loaded via `dotenvy`)
//!   3. `$LABBY_HOME/config.toml` (normally `~/.labby/config.toml`)
//!   4. Built-in defaults
//!
//! Service credentials and instance endpoints belong in `.env`. Non-secret
//! operator preferences and defaults (logging, CORS, MCP transport, admin
//! flags and workspace roots belong in `config.toml`.
//!
//! Multi-instance services follow the `S_<LABEL>_URL` pattern: a service
//! like `unraid` reads `UNRAID_URL` as the default instance and
//! `UNRAID_NODE2_URL` as an additional instance labeled `node2`.

pub mod depot;
#[cfg(test)]
mod depot_tests;
pub mod env_merge;
mod env_writer;
pub mod host_write;
#[cfg(test)]
mod host_write_tests;
mod paths;
pub(crate) mod secret_files;

pub use env_writer::{EnvCredential, write_env_pairs, write_service_creds};
#[cfg(test)]
use paths::resolve_usage_telemetry_enabled;
pub(crate) use paths::{access_db_path, file_stash_root_path, home_dir};
pub use paths::{
    codemode_journal_db_path, codemode_journal_enabled, config_toml_path, dotenv_path,
    toml_candidates, usage_db_path, usage_telemetry_enabled, workspace_root_for_home,
    workspace_root_path,
};
pub use secret_files::heal_env_file_permissions;

#[cfg(test)]
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::{
    collections::BTreeMap,
    collections::HashMap,
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};

// Gateway startup/reload writes this process-wide flag whenever root
// `[code_mode]` changes. In-process peer MCP servers do not hold a
// GatewayManager, but they must still hide raw built-in tools when the root
// server is operating in Code Mode.
static PROCESS_CODE_MODE_ENABLED: AtomicBool = AtomicBool::new(false);
pub const CURRENT_CONFIG_VERSION: u32 = 1;

const fn current_config_version() -> u32 {
    CURRENT_CONFIG_VERSION
}

#[cfg(test)]
static PROCESS_CODE_MODE_TEST_LOCK: Mutex<()> = Mutex::new(());
#[cfg(test)]
static PROCESS_CODE_MODE_TEST_OVERRIDE: AtomicU8 = AtomicU8::new(0);

#[cfg(test)]
pub(crate) struct ProcessCodeModeTestGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: bool,
}

#[cfg(test)]
impl Drop for ProcessCodeModeTestGuard {
    fn drop(&mut self) {
        set_process_code_mode_enabled(self.previous);
        PROCESS_CODE_MODE_TEST_OVERRIDE.store(0, Ordering::Release);
    }
}

#[cfg(test)]
pub(crate) fn process_code_mode_test_guard() -> ProcessCodeModeTestGuard {
    let lock = PROCESS_CODE_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = PROCESS_CODE_MODE_ENABLED.load(Ordering::Acquire);
    PROCESS_CODE_MODE_TEST_OVERRIDE.store(u8::from(previous) + 1, Ordering::Release);
    ProcessCodeModeTestGuard {
        _lock: lock,
        previous,
    }
}

pub(crate) fn set_process_code_mode_enabled(enabled: bool) {
    let previous = PROCESS_CODE_MODE_ENABLED.swap(enabled, Ordering::AcqRel);
    if previous != enabled {
        tracing::info!(
            surface = "mcp",
            service = "code_mode",
            action = "code_mode.process_enablement",
            previous_enabled = previous,
            enabled,
            "process-wide code mode enablement changed"
        );
    }
}

#[cfg(test)]
pub(crate) fn set_process_code_mode_enabled_for_test(enabled: bool) {
    set_process_code_mode_enabled(enabled);
    PROCESS_CODE_MODE_TEST_OVERRIDE.store(u8::from(enabled) + 1, Ordering::Release);
}

pub(crate) fn process_code_mode_enabled() -> bool {
    #[cfg(test)]
    if let Some(enabled) = match PROCESS_CODE_MODE_TEST_OVERRIDE.load(Ordering::Acquire) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    } {
        return enabled;
    }
    PROCESS_CODE_MODE_ENABLED.load(Ordering::Acquire)
}

/// Parse a boolean env flag using the standard truthy set
/// (`1` / `true` / `TRUE` / `yes` / `YES`). Absent or any other value is false.
pub(crate) fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

fn parse_bounded_ms(raw: &str, max: u64) -> Option<u64> {
    raw.parse::<u64>()
        .ok()
        .filter(|value| (1..=max).contains(value))
}

fn parse_bounded_ms_env(name: &str, raw: &str, max: u64) -> Option<u64> {
    let parsed = parse_bounded_ms(raw, max);
    if parsed.is_none() {
        tracing::warn!(
            env_var = name,
            value = raw,
            max_ms = max,
            "ignoring invalid millisecond timeout environment variable; expected 1..=max_ms"
        );
    }
    parsed
}

/// Whether mcp-ui widget -> host tool callbacks are permitted while the Code
/// Mode synthetic surface (`codemode`) is active.
///
/// Default: **off**. When the synthetic surface is on, raw upstream tools are
/// hidden from `list_tools` and normally not callable by name. Setting
/// `LABBY_CODE_MODE_WIDGET_CALLBACKS=1` (or `true`/`yes`) lets a rendered widget's
/// callback reach the upstream proxy by tool name — the tool stays out of
/// `list_tools`, so this only relaxes callability, never visibility. Operators
/// opt in knowingly because it also lets any caller on the session (including
/// the model) invoke a known upstream tool by name.
pub(crate) fn code_mode_widget_callbacks_enabled() -> bool {
    resolved_widget_callbacks_enabled()
}

// ─── Resolved config.toml/env preferences, process-wide ───────────────────
//
// These vars are read from deep call sites (tool dispatch, CLI theming, HTTP
// state construction) that don't have a `&LabConfig` in scope. Rather than
// thread a config reference through every caller, resolve config.toml +
// env-var precedence once at startup and cache the result process-wide,
// mirroring the existing `PROCESS_CODE_MODE_ENABLED` pattern above. Plain
// atomics/mutexes (not `OnceLock`) so tests can freely re-resolve.

static RESOLVED_SHOW_ALL: AtomicBool = AtomicBool::new(false);
static RESOLVED_DEV_MODE: AtomicBool = AtomicBool::new(false);
static RESOLVED_WIDGET_CALLBACKS: AtomicBool = AtomicBool::new(false);
static RESOLVED_INSTALL_ANDROID_SDK: AtomicBool = AtomicBool::new(false);
static RESOLVED_SYMBOLS: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static RESOLVED_PROTECTED_MCP_TIMEOUT_SECS: OnceLock<Mutex<Option<u64>>> = OnceLock::new();
static RESOLVED_CATALOG_NOTIFICATION_TIMEOUT_MS: OnceLock<Mutex<Option<u64>>> = OnceLock::new();

fn resolved_symbols_cell() -> &'static Mutex<Option<String>> {
    RESOLVED_SYMBOLS.get_or_init(|| Mutex::new(None))
}

fn resolved_protected_mcp_timeout_cell() -> &'static Mutex<Option<u64>> {
    RESOLVED_PROTECTED_MCP_TIMEOUT_SECS.get_or_init(|| Mutex::new(None))
}

fn resolved_catalog_notification_timeout_cell() -> &'static Mutex<Option<u64>> {
    RESOLVED_CATALOG_NOTIFICATION_TIMEOUT_MS.get_or_init(|| Mutex::new(None))
}

/// Resolve config.toml + env-var precedence for the small set of
/// preferences read from call sites without direct config access, and cache
/// the result process-wide. Call once, early, right after `config.toml`
/// loads (before `.env` loads and before dispatch) — see `entrypoint.rs`.
pub(crate) fn install_resolved_preferences(config: &LabConfig) {
    RESOLVED_SHOW_ALL.store(
        env_flag_enabled("LABBY_SHOW_ALL") || config.mcp.show_all.unwrap_or(false),
        Ordering::Release,
    );
    RESOLVED_DEV_MODE.store(
        std::env::var("LABBY_DEV_MODE").as_deref() == Ok("1")
            || config.api.dev_mode.unwrap_or(false),
        Ordering::Release,
    );
    RESOLVED_WIDGET_CALLBACKS.store(
        env_flag_enabled("LABBY_CODE_MODE_WIDGET_CALLBACKS")
            || config.code_mode.widget_callbacks.unwrap_or(false),
        Ordering::Release,
    );
    RESOLVED_INSTALL_ANDROID_SDK.store(
        env_flag_enabled("LABBY_ENABLE_ANDROID_SDK")
            || config.setup.install_android_sdk.unwrap_or(false),
        Ordering::Release,
    );
    let symbols = std::env::var("LABBY_SYMBOLS")
        .ok()
        .or_else(|| config.output.symbols.clone());
    *resolved_symbols_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = symbols;
    let protected_mcp_timeout_secs = std::env::var("LABBY_PROTECTED_MCP_CONNECT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .or(config.api.protected_mcp_connect_timeout_secs);
    *resolved_protected_mcp_timeout_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = protected_mcp_timeout_secs;
    let catalog_notification_timeout_ms =
        std::env::var("LABBY_MCP_CATALOG_NOTIFICATION_TIMEOUT_MS")
            .ok()
            .and_then(|raw| {
                parse_bounded_ms_env(
                    "LABBY_MCP_CATALOG_NOTIFICATION_TIMEOUT_MS",
                    &raw,
                    MAX_CATALOG_NOTIFICATION_TIMEOUT_MS,
                )
            })
            .or(config.mcp.catalog_notification_timeout_ms);
    *resolved_catalog_notification_timeout_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = catalog_notification_timeout_ms;
}

pub(crate) fn resolved_show_all() -> bool {
    RESOLVED_SHOW_ALL.load(Ordering::Acquire)
}

pub(crate) fn resolved_dev_mode() -> bool {
    RESOLVED_DEV_MODE.load(Ordering::Acquire)
}

pub(crate) fn resolved_widget_callbacks_enabled() -> bool {
    RESOLVED_WIDGET_CALLBACKS.load(Ordering::Acquire)
}

/// Resolved "install android-sdk during provision" flag, folding
/// `LABBY_ENABLE_ANDROID_SDK=1` env over `[setup].install_android_sdk` config.
/// Read by the provision plan builder (`ActionKind::AndroidSdk`).
pub(crate) fn resolved_install_android_sdk() -> bool {
    RESOLVED_INSTALL_ANDROID_SDK.load(Ordering::Acquire)
}

pub(crate) fn resolved_symbols() -> Option<String> {
    resolved_symbols_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

pub(crate) fn resolved_protected_mcp_connect_timeout_secs() -> Option<u64> {
    *resolved_protected_mcp_timeout_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn resolved_catalog_notification_timeout() -> Duration {
    Duration::from_millis(
        resolved_catalog_notification_timeout_cell()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unwrap_or(DEFAULT_CATALOG_NOTIFICATION_TIMEOUT_MS),
    )
}

use anyhow::{Context, Result};
use labby_auth::config as auth_config;
use serde::{Deserialize, Serialize, Serializer};

pub const WEB_UI_AUTH_DISABLED_ENV: &str = "LABBY_WEB_UI_AUTH_DISABLED";
pub const WEB_UI_AUTH_DISABLED_LEGACY_ENV: &str = "LABBY_WEB_UI_DISABLE_AUTH";
const DEFAULT_UPSTREAM_REQUEST_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_CATALOG_NOTIFICATION_TIMEOUT_MS: u64 = 5_000;
const MAX_CATALOG_NOTIFICATION_TIMEOUT_MS: u64 = 60_000;
/// Default deadline for a *relayed* upstream tool call (see
/// [`LabConfig::upstream_relay_timeout`]).
///
/// Relayed calls carry a human-in-the-loop round trip — the upstream raises an
/// `elicitation/create` that is forwarded to the downstream agent and answered
/// by a person — so the ordinary 30s `upstream_request_timeout` would abort
/// legitimate confirmations. The relay deadline defaults to 5 minutes to give a
/// human time to respond while still bounding the dedicated connection's
/// lifetime. Only the relay path uses this; the pooled hot path keeps
/// [`DEFAULT_UPSTREAM_REQUEST_TIMEOUT_MS`].
const DEFAULT_UPSTREAM_RELAY_TIMEOUT_MS: u64 = 300_000;
/// Headroom added to the longest configured upstream deadline when deriving the
/// hosted HTTP transport timeout (see [`LabConfig::http_request_timeout`]).
///
/// Covers the work bracketing the upstream call itself — auth, Code Mode
/// compilation, connection-pool checkout, response serialization — so the inner
/// deadline is always the one that fires on a slow upstream.
const HTTP_REQUEST_TIMEOUT_MARGIN: Duration = Duration::from_secs(30);
const CONFIG_BACKUP_RETENTION: usize = 10;
const CONFIG_BACKUP_MAX_AGE: Duration = Duration::from_hours(30 * 24);
const CONFIG_BACKUP_MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
struct ConfigBackupCandidate {
    path: PathBuf,
    modified: std::time::SystemTime,
    bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct ConfigBackupRetention {
    max_count: usize,
    max_age: Duration,
    max_bytes: u64,
}

#[cfg(test)]
impl ConfigBackupCandidate {
    fn fixture(path: &str, bytes: u64, modified: std::time::SystemTime) -> Self {
        Self {
            path: PathBuf::from(path),
            modified,
            bytes,
        }
    }
}

#[cfg(test)]
static TEST_CONFIG_TOML_PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn set_test_config_toml_path(path: Option<PathBuf>) {
    let slot = TEST_CONFIG_TOML_PATH.get_or_init(|| Mutex::new(None));
    *slot.lock().expect("test config path lock") = path;
}

/// Fully-resolved `lab` configuration, assembled from env + TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabConfig {
    /// Persisted configuration schema version. Missing legacy values migrate to v1.
    #[serde(default = "current_config_version")]
    pub config_version: u32,
    /// Instance-shared Depot discovery providers, independent of acquisition.
    #[serde(default)]
    pub depot: depot::DepotPreferences,
    /// Default output format for CLI commands that print tables.
    #[serde(default)]
    pub output: OutputPreferences,
    /// MCP server defaults.
    #[serde(default)]
    pub mcp: McpPreferences,
    /// Ephemeral stdio MCP proxy defaults.
    #[serde(default)]
    pub proxy: crate::proxy::config::ProxyPreferences,
    /// Logging preferences (overridden by `LABBY_LOG` / `LABBY_LOG_FORMAT` env vars).
    #[serde(default)]
    pub log: LogPreferences,
    /// Local Labby server-log subsystem preferences.
    #[serde(default)]
    pub local_logs: Option<LocalLogsPreferences>,
    /// HTTP API preferences.
    #[serde(default)]
    pub api: ApiPreferences,
    /// Web UI preferences.
    #[serde(default)]
    pub web: WebPreferences,
    /// Shared Labby workspace root for the optional filesystem browser.
    #[serde(default)]
    pub workspace: WorkspacePreferences,
    /// Principal-scoped durable File Stash storage.
    #[serde(default)]
    pub file_stash: FileStashPreferences,
    /// OAuth callback relay preferences.
    #[serde(default)]
    pub oauth: OauthPreferences,
    /// Admin tool settings.
    #[serde(default)]
    pub admin: AdminPreferences,
    /// Per-service preference overrides.
    #[serde(default)]
    pub services: ServicePreferences,
    /// Setup/provision preferences (operator toggles for `labby setup --provision`).
    #[serde(default)]
    pub setup: SetupPreferences,
    /// HTTP auth mode preferences.
    #[serde(default)]
    pub auth: Option<AuthFileConfig>,
    /// Gateway-wide Code Mode exposure and execution settings.
    #[serde(default)]
    pub code_mode: CodeModeConfig,
    /// Visibility of Labby-owned MCP App surfaces other than Code Mode.
    #[serde(default)]
    pub mcp_apps: McpAppsConfig,
    /// Optional server-held exact-revision Skill acquisition connections.
    #[serde(default)]
    pub artifacts: ArtifactPreferences,
    /// Maximum time to wait for one proxied upstream MCP tool/resource/prompt response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_request_timeout_ms: Option<u64>,
    /// Maximum time to wait for one proxied MRTR-capable upstream tool call.
    /// This path preserves `input_required` responses for the downstream
    /// client and gets its own longer deadline (default 5 minutes; see
    /// [`LabConfig::upstream_relay_timeout`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_relay_timeout_ms: Option<u64>,
    /// Upstream MCP servers to proxy through the gateway.
    #[serde(default)]
    pub upstream: Vec<UpstreamConfig>,
    /// Imported upstreams removed by an operator. Auto-import honors this list
    /// so deleted external-config entries do not immediately return on restart.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream_import_tombstones: Vec<UpstreamImportTombstone>,
    /// Discovered upstreams waiting for operator approval. Populated when
    /// `gateway_import_mode = "pending"`. Empty when mode is `"off"` or `"auto"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream_pending: Vec<UpstreamConfig>,
    /// Controls how external MCP config discovery behaves on startup.
    /// - `"off"` (default): discovery is disabled; no auto-import.
    /// - `"pending"`: discover on startup, queue for approval — never auto-apply.
    /// - `"auto"`: auto-import everything not tombstoned (legacy behavior).
    #[serde(default)]
    pub gateway_import_mode: GatewayImportMode,
    /// Named reusable gateway capability projections.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loadouts: Vec<GatewayLoadoutConfig>,
    /// Public HTTP MCP routes protected by Lab OAuth and proxied by Lab.
    ///
    /// These are intentionally separate from `upstream`: upstreams import tools
    /// into Lab, while protected MCP routes expose a backend MCP server through
    /// Lab as an OAuth resource server.
    #[serde(default)]
    pub protected_mcp_routes: Vec<ProtectedMcpRouteConfig>,
    /// Virtual MCP servers backed by canonically configured Lab services.
    #[serde(default)]
    pub virtual_servers: Vec<VirtualServerConfig>,
    /// Virtual servers whose backing service is no longer registered in this binary.
    #[serde(default)]
    pub quarantined_virtual_servers: Vec<VirtualServerConfig>,
    /// Canonical public URL model for the app and MCP gateway.
    ///
    /// Use [`LabConfig::public_urls()`] to read resolved values with env-var
    /// precedence rather than accessing this field directly.
    #[serde(default)]
    pub public_urls: Option<PublicUrlsConfig>,
    /// Gateway spawn-guard and command-allowlist preferences.
    #[serde(default)]
    pub gateway: GatewayPreferences,
    /// Code Mode `openapi` local-provider spec configuration.
    ///
    /// Non-secret only (spec URL/path, label, mandatory base_url, allowlist);
    /// credentials are read from `OPENAPI_<LABEL>_*` env vars, never TOML.
    #[serde(default)]
    pub openapi: OpenApiTomlSection,
}

impl Default for LabConfig {
    fn default() -> Self {
        toml::from_str("").expect("the empty built-in LabConfig must deserialize")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPreferences {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<ArtifactSourceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSourceConfig {
    pub id: String,
    pub kind: ArtifactSourceKind,
    /// Exact-revision acquisition endpoint used by `artifacts.import`.
    pub endpoint: String,
    /// Depot HTTP origin used for curated operations and raw uploads.
    ///
    /// This is deliberately separate from `endpoint`: the latter accepts the
    /// exact-acquisition POST contract, while this value is an origin to which
    /// Labby appends fixed `/api/operations/...` and `/uploads/...` paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_plane_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_addresses: Vec<std::net::IpAddr>,
    /// Name of an environment variable containing the server-held bearer secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token_env: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactSourceKind {
    Depot,
    Repository,
}

/// `[openapi]` config section: a list of `[[openapi.specs]]` tables.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenApiTomlSection {
    /// Configured specs.
    #[serde(default)]
    pub specs: Vec<OpenApiSpecToml>,
}

/// One `[[openapi.specs]]` table. Non-secret fields only.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenApiSpecToml {
    /// Provider label (`openapi::<label>.<operationId>`).
    #[serde(default)]
    pub label: String,
    /// Mandatory base URL for outbound requests (validated at load time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Spec document URL (mutually exclusive with `spec_path`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_url: Option<String>,
    /// Spec document filesystem path (mutually exclusive with `spec_url`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_path: Option<String>,
    /// Header name for `OPENAPI_<LABEL>_API_KEY` injection (default `X-API-Key`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_header: Option<String>,
    /// Deny-by-default allowlist of raw operationIds.
    #[serde(default)]
    pub allowed_operations: Vec<String>,
}

// `GatewayPreferences` moved to `labby_runtime::gateway_config`; re-exported above.

impl LabConfig {
    /// Resolve the canonical public URL pair after env-over-config merge.
    ///
    /// Precedence (highest wins):
    ///   1. `LABBY_PUBLIC_URL` env var (app), `LABBY_MCP_GATEWAY_URL` env var (gateway)
    ///   2. `config.toml` `[public_urls]` section
    ///   3. Legacy `[auth].public_url` field (app only, for backward compat)
    pub fn public_urls(&self) -> ResolvedPublicUrls {
        // Env wins
        let env_app = std::env::var("LABBY_PUBLIC_URL")
            .ok()
            .filter(|v| !v.is_empty());
        let env_gw = std::env::var("LABBY_MCP_GATEWAY_URL")
            .ok()
            .filter(|v| !v.is_empty());

        let app = env_app
            .or_else(|| self.public_urls.as_ref().and_then(|p| p.app.clone()))
            .or_else(|| {
                // Backward compat: fall back to [auth].public_url
                self.auth.as_ref().and_then(|a| a.public_url.clone())
            });

        let mcp_gateway = env_gw.or_else(|| {
            self.public_urls
                .as_ref()
                .and_then(|p| p.mcp_gateway.clone())
        });

        ResolvedPublicUrls { app, mcp_gateway }
    }

    /// Project the gateway-relevant slice of this config into the surface-neutral
    /// [`GatewayConfig`] DTO the `GatewayManager` owns in memory.
    #[must_use]
    pub fn to_gateway_config(&self) -> GatewayConfig {
        GatewayConfig {
            code_mode: self.code_mode.clone(),
            mcp_apps: self.mcp_apps,
            upstream_request_timeout_ms: self.upstream_request_timeout_ms,
            upstream_relay_timeout_ms: self.upstream_relay_timeout_ms,
            upstream: self.upstream.clone(),
            upstream_import_tombstones: self.upstream_import_tombstones.clone(),
            upstream_pending: self.upstream_pending.clone(),
            loadouts: self.loadouts.clone(),
            protected_mcp_routes: self.protected_mcp_routes.clone(),
            virtual_servers: self.virtual_servers.clone(),
            quarantined_virtual_servers: self.quarantined_virtual_servers.clone(),
            gateway: self.gateway.clone(),
        }
    }

    /// Overwrite the gateway-owned sections of this config from `gw`, leaving
    /// every non-gateway section (and any foreign top-level keys preserved by
    /// the toml_edit render path) untouched.
    pub fn apply_gateway_config(&mut self, gw: &GatewayConfig) {
        self.code_mode = gw.code_mode.clone();
        self.mcp_apps = gw.mcp_apps;
        self.upstream_request_timeout_ms = gw.upstream_request_timeout_ms;
        self.upstream_relay_timeout_ms = gw.upstream_relay_timeout_ms;
        self.upstream = gw.upstream.clone();
        self.upstream_import_tombstones = gw.upstream_import_tombstones.clone();
        self.upstream_pending = gw.upstream_pending.clone();
        self.loadouts = gw.loadouts.clone();
        self.protected_mcp_routes = gw.protected_mcp_routes.clone();
        self.virtual_servers = gw.virtual_servers.clone();
        self.quarantined_virtual_servers = gw.quarantined_virtual_servers.clone();
        self.gateway = gw.gateway.clone();
    }
}

impl From<&LabConfig> for GatewayConfig {
    fn from(cfg: &LabConfig) -> Self {
        cfg.to_gateway_config()
    }
}

impl LabConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.config_version != CURRENT_CONFIG_VERSION {
            return Err(ConfigError::InvalidProxyConfig {
                reason: format!(
                    "config_version {} is unsupported; expected {}",
                    self.config_version, CURRENT_CONFIG_VERSION
                ),
            });
        }
        self.code_mode.validate()?;
        self.file_stash.validate()?;
        self.proxy
            .validate()
            .map_err(|error| ConfigError::InvalidProxyConfig {
                reason: error.to_string(),
            })?;
        if let Some(value) = self.upstream_request_timeout_ms
            && !(1..=300_000).contains(&value)
        {
            return Err(ConfigError::InvalidUpstreamRequestTimeout { value });
        }
        // The relay deadline allows a wider ceiling (30 min) than the pooled
        // request timeout because it spans a human answering an elicitation.
        if let Some(value) = self.upstream_relay_timeout_ms
            && !(1..=1_800_000).contains(&value)
        {
            return Err(ConfigError::InvalidUpstreamRelayTimeout { value });
        }
        if let Some(value) = self.mcp.catalog_notification_timeout_ms
            && !(1..=MAX_CATALOG_NOTIFICATION_TIMEOUT_MS).contains(&value)
        {
            return Err(ConfigError::InvalidCatalogNotificationTimeout { value });
        }
        for upstream in &self.upstream {
            upstream.validate()?;
        }
        validate_protected_mcp_routes_for_startup(self)?;
        Ok(())
    }

    pub fn upstream_request_timeout(&self) -> Duration {
        Duration::from_millis(
            self.upstream_request_timeout_ms
                .unwrap_or(DEFAULT_UPSTREAM_REQUEST_TIMEOUT_MS),
        )
    }

    /// Deadline for a single *relayed* upstream tool call.
    ///
    /// Distinct from [`Self::upstream_request_timeout`] because the relay path
    /// blocks on a human answering an elicitation forwarded from the upstream;
    /// reusing the 30s request timeout would abort real confirmations. Defaults
    /// to `DEFAULT_UPSTREAM_RELAY_TIMEOUT_MS` (5 minutes) when unset.
    pub fn upstream_relay_timeout(&self) -> Duration {
        Duration::from_millis(
            self.upstream_relay_timeout_ms
                .unwrap_or(DEFAULT_UPSTREAM_RELAY_TIMEOUT_MS),
        )
    }

    /// Transport-level deadline for one hosted HTTP request.
    ///
    /// This is a backstop for requests that outlive every inner deadline, not a
    /// product timeout. It is derived from the configured upstream deadlines so
    /// it can never fire *before* the timeout it is supposed to wrap: a request
    /// that exceeds `upstream_request_timeout` / `upstream_relay_timeout` must
    /// fail with a structured MCP error from the dispatch layer, not a bare 504
    /// from the HTTP stack.
    ///
    /// A fixed cap here previously overrode both settings — an operator raising
    /// `upstream_request_timeout_ms` past 30s got no effect, because the
    /// transport killed the response first and discarded a tool call that had
    /// already succeeded.
    ///
    /// Two more inner deadlines ride on a single hosted request and are covered
    /// the same way: a Code Mode run (`code_mode.timeout_ms`, carried by the
    /// `/mcp` request that started it) and a synchronous `agents.run`, which
    /// holds its request for the fixed Agent runtime bound.
    pub fn http_request_timeout(&self) -> Duration {
        self.upstream_request_timeout()
            .max(self.upstream_relay_timeout())
            .max(Duration::from_millis(self.code_mode.timeout_ms))
            .max(Duration::from_millis(
                labby_runtime::agent_runtime::AGENT_MAX_RUNTIME_MILLIS,
            ))
            .saturating_add(HTTP_REQUEST_TIMEOUT_MARGIN)
    }

    pub fn normalize_protected_mcp_routes(&mut self) -> Result<(), ConfigError> {
        for route in &mut self.protected_mcp_routes {
            route.upstream = route
                .upstream
                .take()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty());
            if let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = &mut route.target {
                if let Some(project_id) = target.project_id.take() {
                    let project_id = project_id.trim().to_string();
                    if !labby_runtime::gateway_config::is_canonical_project_id(&project_id) {
                        return Err(ConfigError::InvalidProtectedRoute {
                            name: route.name.clone(),
                            field: "target.project_id",
                            value: "project binding must not be empty".to_string(),
                        });
                    }
                    target.project_id = Some(project_id);
                }
                target.loadout = target
                    .loadout
                    .take()
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty());
                normalize_string_list(&mut target.upstreams, "target.upstreams").map_err(
                    |field| ConfigError::InvalidProtectedRoute {
                        name: route.name.clone(),
                        field,
                        value: "gateway_subset target entries must not be empty".to_string(),
                    },
                )?;
                normalize_string_list(&mut target.services, "target.services").map_err(
                    |field| ConfigError::InvalidProtectedRoute {
                        name: route.name.clone(),
                        field,
                        value: "gateway_subset target entries must not be empty".to_string(),
                    },
                )?;
                // Mirrors the identical guard in
                // `labby_runtime::gateway_config::GatewayConfig::normalize_protected_mcp_routes`.
                // THIS is the copy `load_toml` runs, and therefore the copy the
                // mounted route scopes in `cli/serve.rs` are built from — the
                // runtime copy alone left the guard off the serve path
                // (review finding on lab-eyeuv). Keep the two in sync.
                if let Some(reserved) = target
                    .upstreams
                    .iter()
                    .find(|name| name.starts_with(IN_PROCESS_UPSTREAM_PREFIX))
                {
                    return Err(ConfigError::InvalidProtectedRoute {
                        name: route.name.clone(),
                        field: "target.upstreams",
                        value: format!(
                            "`{reserved}` uses the reserved `{IN_PROCESS_UPSTREAM_PREFIX}` \
                             prefix; built-in service peers cannot be routed to a protected \
                             subset — list the service under `target.services` instead"
                        ),
                    });
                }
            }
            if route.target.is_some()
                && (route.upstream.is_some() || !route.backend_url.trim().is_empty())
            {
                return Err(ConfigError::InvalidProtectedRoute {
                    name: route.name.clone(),
                    field: "target",
                    value:
                        "protected MCP route target cannot be combined with upstream or backend_url"
                            .to_string(),
                });
            }
            if route.target.is_some() {
                route.backend_url = String::new();
                route.backend_mcp_path = default_mcp_path();
                continue;
            }
            if route.upstream.is_some() && route.backend_url.trim().is_empty() {
                route.backend_url = String::new();
            } else {
                route.backend_url =
                    normalize_protected_backend_url(&route.backend_url, &route.backend_mcp_path)
                        .map_err(|_| ConfigError::InvalidProtectedRoute {
                            name: route.name.clone(),
                            field: "backend_url",
                            value: route.backend_url.clone(),
                        })?;
            }
            route.backend_mcp_path = default_mcp_path();
        }
        Ok(())
    }
}

fn normalize_string_list(
    values: &mut Vec<String>,
    field: &'static str,
) -> Result<(), &'static str> {
    let mut normalized = Vec::new();
    for value in std::mem::take(values) {
        let name = value.trim().to_string();
        if name.is_empty() {
            return Err(field);
        }
        if !normalized.contains(&name) {
            normalized.push(name);
        }
    }
    *values = normalized;
    Ok(())
}

fn validate_protected_mcp_routes_for_startup(cfg: &LabConfig) -> Result<(), ConfigError> {
    let mut names = std::collections::HashSet::new();
    let mut enabled_keys = std::collections::HashSet::new();
    let upstream_names: std::collections::HashSet<&str> = cfg
        .upstream
        .iter()
        .map(|upstream| upstream.name.as_str())
        .collect();
    let registry = crate::registry::build_docs_registry();
    let service_names: std::collections::HashSet<&str> = registry
        .services()
        .iter()
        .filter(|service| registry.supports_context_free_dispatch(service.name))
        .map(|service| service.name)
        .collect();
    let loadout_names: std::collections::HashSet<&str> = cfg
        .loadouts
        .iter()
        .map(|loadout| loadout.name.as_str())
        .collect();

    for route in &cfg.protected_mcp_routes {
        validate_protected_mcp_route_for_startup(
            route,
            &upstream_names,
            &service_names,
            &loadout_names,
        )?;
        if !names.insert(route.name.trim().to_string()) {
            return Err(ConfigError::InvalidProtectedRoute {
                name: route.name.clone(),
                field: "name",
                value: format!(
                    "protected MCP route `{}` appears more than once",
                    route.name
                ),
            });
        }
        if route.enabled {
            let key = (
                route.public_host.trim().to_ascii_lowercase(),
                route.public_path.trim().to_string(),
            );
            if !enabled_keys.insert(key) {
                return Err(ConfigError::InvalidProtectedRoute {
                    name: route.name.clone(),
                    field: "public_path",
                    value: format!(
                        "duplicate enabled protected MCP route for {}{}",
                        route.public_host, route.public_path
                    ),
                });
            }
        }
    }
    Ok(())
}

fn validate_protected_mcp_route_for_startup(
    route: &ProtectedMcpRouteConfig,
    upstream_names: &std::collections::HashSet<&str>,
    service_names: &std::collections::HashSet<&str>,
    loadout_names: &std::collections::HashSet<&str>,
) -> Result<(), ConfigError> {
    if route.name.trim().is_empty() {
        return invalid_protected_route(
            route,
            "name",
            "protected MCP route name must not be empty",
        );
    }
    validate_protected_public_path_for_startup(route, route.public_path.trim())?;
    if route.target.is_some() && (route.upstream.is_some() || !route.backend_url.trim().is_empty())
    {
        return invalid_protected_route(
            route,
            "target",
            "protected MCP route target cannot be combined with upstream or backend_url",
        );
    }

    if let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = &route.target {
        if target.loadout.is_some()
            && (!target.upstreams.is_empty()
                || !target.services.is_empty()
                || target.expose_code_mode)
        {
            return invalid_protected_route(
                route,
                "target.loadout",
                "gateway_subset target with `loadout` cannot also set inline upstreams, services, or expose_code_mode",
            );
        }
        if target.loadout.is_none()
            && target.upstreams.is_empty()
            && target.services.is_empty()
            && !target.expose_code_mode
        {
            return invalid_protected_route(
                route,
                "target",
                "gateway_subset target must set a loadout or expose at least one upstream, service, or Code Mode",
            );
        }
        if let Some(loadout) = target.loadout.as_deref()
            && !loadout_names.contains(loadout)
        {
            return invalid_protected_route(
                route,
                "target.loadout",
                format!("unknown gateway_subset loadout `{loadout}`"),
            );
        }
        if route.enabled {
            for upstream in &target.upstreams {
                if !upstream_names.contains(upstream.as_str()) {
                    return invalid_protected_route(
                        route,
                        "target.upstreams",
                        format!("unknown gateway_subset upstream `{upstream}`"),
                    );
                }
            }
            for service in &target.services {
                if !service_names.contains(service.as_str()) {
                    return invalid_protected_route(
                        route,
                        "target.services",
                        format!("unknown gateway_subset service `{service}`"),
                    );
                }
            }
        }
        return Ok(());
    }

    match (
        route.upstream.as_deref(),
        route.backend_url.trim().is_empty(),
    ) {
        (Some(_), true) | (None, false) => Ok(()),
        (Some(_), false) => invalid_protected_route(
            route,
            "upstream",
            "protected MCP route must set either upstream or backend_url, not both",
        ),
        (None, true) => invalid_protected_route(
            route,
            "backend_url",
            "protected MCP route must set upstream or backend_url",
        ),
    }
}

fn validate_protected_public_path_for_startup(
    route: &ProtectedMcpRouteConfig,
    path: &str,
) -> Result<(), ConfigError> {
    if path == "/" {
        return invalid_protected_route(
            route,
            "public_path",
            "public_path must include a service segment",
        );
    }
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("/.well-known")
        || lower.starts_with("/v1")
        || crate::oauth::public_relay::is_reserved_public_relay_path(path)
    {
        return invalid_protected_route(
            route,
            "public_path",
            "public_path conflicts with Lab reserved routes",
        );
    }
    if lower.contains("%2f")
        || lower.contains("%5c")
        || lower.contains("%2e")
        || path.contains('\\')
        || path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
        || path.contains("//")
    {
        return invalid_protected_route(
            route,
            "public_path",
            "public_path contains unsafe or ambiguous path segments",
        );
    }
    Ok(())
}

fn invalid_protected_route(
    route: &ProtectedMcpRouteConfig,
    field: &'static str,
    value: impl Into<String>,
) -> Result<(), ConfigError> {
    Err(ConfigError::InvalidProtectedRoute {
        name: route.name.clone(),
        field,
        value: value.into(),
    })
}

// Gateway config DTOs and their dependency closure now live in
// `labby_runtime::gateway_config`. They are re-exported below so the rest of
// this module and all external callers keep their existing import paths.
// Serde shape (defaults, renames, skip rules) is preserved exactly there.
// Some entries are only referenced from tests after the gateway runtime moved to
// `labby-gateway`; keep them as the public `labby::config` surface and silence the
// bin-target unused-import lint.
#[allow(unused_imports)]
pub use labby_runtime::gateway_config::IN_PROCESS_UPSTREAM_PREFIX;
pub use labby_runtime::gateway_config::{
    CodeModeConfig, CodeModeResultShapePolicy, ConfigError, GatewayConfig, GatewayImportMode,
    GatewayLoadoutConfig, GatewayPreferences, ImportSource, McpAppsConfig,
    ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig, ProtectedMcpRouteEffectiveTarget,
    ProtectedMcpRouteTarget, ResolvedPublicUrls, UpstreamConfig, UpstreamImportTombstone,
    UpstreamOauthConfig, UpstreamOauthCredentialSource, UpstreamOauthMode,
    UpstreamOauthRegistration, VirtualServerConfig, VirtualServerMcpPolicyConfig,
    VirtualServerSurfacesConfig, WebPreferences, default_mcp_path, default_true,
    normalize_protected_backend_url,
};
// Re-exported for the public `labby::config` API surface (consumed by the
// `upstream_oauth` integration test); not referenced within the binary build,
// so silence the bin-target unused-import lint.
#[allow(unused_imports)]
pub use labby_runtime::gateway_config::canonicalize_upstream_url;

/// Table/json formatting defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputPreferences {
    /// Default format: `human` or `json`. Honored unless `--json` overrides.
    #[serde(default)]
    pub format: Option<String>,
    /// Symbol set for CLI output: `"unicode"` (default) or `"ascii"`.
    /// Overridden by `LABBY_SYMBOLS` env var.
    #[serde(default)]
    pub symbols: Option<String>,
}

/// MCP server defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpPreferences {
    /// Default transport (`stdio`, `http`, or `unix_socket`).
    #[serde(default)]
    pub transport: Option<String>,
    /// Default bind address for the HTTP transport.
    #[serde(default)]
    pub host: Option<String>,
    /// Default port for the HTTP transport.
    #[serde(default)]
    pub port: Option<u16>,
    /// Filesystem Unix-domain socket path, or Linux abstract `@name` notation,
    /// used when `transport = "unix_socket"`.
    #[serde(default)]
    pub socket_path: Option<PathBuf>,
    /// Filesystem socket mode in octal, such as `0660` or `0o660`.
    #[serde(default)]
    pub socket_mode: Option<String>,
    /// Optional owner UID applied after binding a filesystem socket.
    #[serde(default)]
    pub socket_uid: Option<u32>,
    /// Optional owner GID applied after binding a filesystem socket.
    #[serde(default)]
    pub socket_gid: Option<u32>,
    /// Optional kernel peer UID allowlist for Unix-socket authorization.
    #[serde(default)]
    pub peer_uid: Option<u32>,
    /// Optional kernel peer GID allowlist for Unix-socket authorization.
    #[serde(default)]
    pub peer_gid: Option<u32>,
    /// Additional allowed hosts for DNS rebinding protection.
    #[serde(default)]
    pub allowed_hosts: Option<Vec<String>>,
    /// Show the full service catalog regardless of env-var presence.
    /// Overridden by `LABBY_SHOW_ALL` env var.
    #[serde(default)]
    pub show_all: Option<bool>,
    /// Maximum time to wait for one MCP peer catalog-change notification.
    /// Overridden by `LABBY_MCP_CATALOG_NOTIFICATION_TIMEOUT_MS`.
    #[serde(default)]
    pub catalog_notification_timeout_ms: Option<u64>,
}

/// Canonical public URL model.
///
/// `app` is the Lab UI and OAuth issuer, e.g. `https://lab.example.com`.
/// `mcp_gateway` is the MCP endpoint base URL when hosted on a separate hostname,
/// e.g. `https://mcp.example.com`.  When absent the gateway is assumed to be
/// reachable at the app URL.
///
/// Values are read from config.toml; env vars `LABBY_PUBLIC_URL` (app) and
/// `LABBY_MCP_GATEWAY_URL` (mcp_gateway) take precedence and may be set in
/// `~/.labby/.env`.
///
/// Accessor: [`LabConfig::public_urls()`] returns a resolved [`ResolvedPublicUrls`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PublicUrlsConfig {
    /// Public app (UI + OAuth) base URL, e.g. `https://lab.example.com`.
    #[serde(default)]
    pub app: Option<String>,
    /// Separate MCP gateway base URL, e.g. `https://mcp.example.com`.
    /// Leave blank when the app and MCP gateway share the same hostname.
    #[serde(default)]
    pub mcp_gateway: Option<String>,
}

// `ResolvedPublicUrls` moved to `labby_runtime::gateway_config`; re-exported above.

/// File-backed auth preferences merged with environment variables at startup.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthFileConfig {
    /// `bearer` preserves LABBY_MCP_HTTP_TOKEN; `oauth` enables the internal auth server.
    #[serde(default)]
    pub mode: Option<String>,
    /// Public URL used for metadata and Google callback construction.
    #[serde(default)]
    pub public_url: Option<String>,
    /// Optional path override for the SQLite auth store.
    #[serde(default)]
    pub sqlite_path: Option<PathBuf>,
    /// Optional path override for the persisted JWT signing key.
    #[serde(default)]
    pub key_path: Option<PathBuf>,
    /// Enable dynamic client registration (default true). CIMD remains available.
    #[serde(default)]
    pub enable_dynamic_registration: Option<bool>,
    /// Legacy bootstrap secret retained for configuration compatibility.
    #[serde(default)]
    pub bootstrap_secret: Option<String>,
    /// Additional redirect URI patterns allowed for dynamic client registration.
    #[serde(default)]
    pub allowed_client_redirect_uris: Option<Vec<String>>,
    /// Google Workspace hosted domains whose members may log in, in addition to
    /// the admin email and the per-email allowlist.
    ///
    /// Matched against the Google ID token's `hd` claim, so only accounts truly
    /// hosted in the domain qualify.
    #[serde(default)]
    pub allowed_email_domains: Option<Vec<String>>,
    /// Exact verified email domains eligible for automatic Viewer membership.
    #[serde(default)]
    pub viewer_email_domains: Option<Vec<String>>,
    /// Host-selected project for domain Viewers; never selected by a request.
    #[serde(default)]
    pub viewer_project_id: Option<String>,
    /// Active inbound human identity provider (`google` or `authelia`).
    #[serde(default)]
    pub provider: Option<String>,
    /// Authelia OpenID Connect issuer URL.
    #[serde(default)]
    pub authelia_issuer_url: Option<String>,
    /// Authelia confidential client ID.
    #[serde(default)]
    pub authelia_client_id: Option<String>,
    /// Authelia confidential client secret.
    #[serde(default)]
    pub authelia_client_secret: Option<String>,
    /// Exact explicitly trusted private issuer origin.
    #[serde(default)]
    pub authelia_trusted_private_origin: Option<String>,
    /// Optional PEM CA certificate used only for this exact Authelia origin.
    #[serde(default)]
    pub authelia_ca_certificate_path: Option<PathBuf>,
    /// Google OAuth client ID.
    #[serde(default)]
    pub google_client_id: Option<String>,
    /// Google OAuth client secret.
    #[serde(default)]
    pub google_client_secret: Option<String>,
    /// Optional callback path override.
    #[serde(default)]
    pub google_callback_path: Option<String>,
    /// Optional comma-separated scope list.
    #[serde(default)]
    pub google_scopes: Option<Vec<String>>,
    /// Optional access-token lifetime override in seconds.
    #[serde(default)]
    pub access_token_ttl_secs: Option<u64>,
    /// Optional refresh-token lifetime override in seconds.
    #[serde(default)]
    pub refresh_token_ttl_secs: Option<u64>,
    /// Optional authorization-code lifetime override in seconds.
    #[serde(default)]
    pub auth_code_ttl_secs: Option<u64>,
    /// Bootstrap admin Google email — required in oauth mode.
    #[serde(default)]
    pub admin_email: Option<String>,
    /// Per-IP rate limit for the dynamic-client-registration endpoint
    /// (requests per minute). Overridden by `LABBY_AUTH_REGISTER_REQUESTS_PER_MINUTE`.
    #[serde(default)]
    pub register_requests_per_minute: Option<u32>,
    /// Per-IP rate limit for the `/authorize` endpoint (requests per minute).
    /// Overridden by `LABBY_AUTH_AUTHORIZE_REQUESTS_PER_MINUTE`.
    #[serde(default)]
    pub authorize_requests_per_minute: Option<u32>,
    /// Per-IP rate limit for the `/token` endpoint (requests per minute).
    /// Overridden by `LABBY_AUTH_TOKEN_REQUESTS_PER_MINUTE`.
    #[serde(default)]
    pub token_requests_per_minute: Option<u32>,
    /// Out-of-band machine OAuth clients.
    #[serde(default)]
    pub machine_clients: Option<Vec<auth_config::MachineClientConfig>>,
    /// Trusted enterprise ID-JAG issuers.
    #[serde(default)]
    pub enterprise_issuers: Option<Vec<auth_config::EnterpriseIssuerConfig>>,
    /// Max in-flight OAuth state rows. Overridden by
    /// `LABBY_AUTH_MAX_PENDING_OAUTH_STATES`.
    #[serde(default)]
    pub max_pending_oauth_states: Option<usize>,
    /// Work around Codex clients that strip the RFC 9207 response issuer.
    /// Overridden by `LABBY_AUTH_CODEX_ISSUER_COMPATIBILITY`.
    #[serde(default)]
    pub codex_issuer_compatibility: Option<bool>,
}

const DEFAULT_CLIENT_REDIRECT_URI_PATTERNS: &[&str] = &[
    "https://chatgpt.com/aip/plugin-callback",
    "https://chat.openai.com/aip/plugin-callback",
    "https://chatgpt.com/connector/oauth/*",
    "https://chatgpt.com/connector_platform_oauth_redirect",
    "https://claude.ai/api/mcp/auth_callback",
    "https://claude.com/api/mcp/auth_callback",
];

/// Resolve auth configuration from a full `LabConfig`.
///
/// This is the preferred entry point. Precedence for the public URL is:
/// 1. `[auth].public_url` (legacy field, preserved for backward compatibility)
/// 2. `[public_urls].app` (canonical new location)
/// 3. `LABBY_PUBLIC_URL` env var (handled downstream by [`resolve_auth`])
///
/// When `[auth].public_url` is absent, `[public_urls].app` is promoted into the
/// auth config so downstream code resolves a consistent effective URL.
pub fn resolve_auth_for_config(cfg: &LabConfig) -> Result<auth_config::AuthConfig> {
    // Compute the effective public URL: [auth].public_url > [public_urls].app.
    // The env var LABBY_PUBLIC_URL is handled downstream by resolve_auth().
    let effective_public_url = cfg
        .auth
        .as_ref()
        .and_then(|a| a.public_url.clone())
        .or_else(|| cfg.public_urls().app);

    // Build a synthetic auth config that overlays the effective public URL.
    let mut auth = cfg.auth.clone().unwrap_or_default();
    if auth.public_url.is_none() {
        auth.public_url = effective_public_url;
    }
    resolve_auth(Some(&auth))
}

/// Resolve auth configuration from config file + environment variables.
///
/// Env vars take precedence over config file values.
/// Prefer [`resolve_auth_for_config`] when a full `LabConfig` is available,
/// so that `[public_urls].app` is used as a fallback for `LABBY_PUBLIC_URL`.
pub fn resolve_auth(config: Option<&AuthFileConfig>) -> Result<auth_config::AuthConfig> {
    resolve_auth_with_env(config, std::env::vars())
}

fn resolve_auth_with_env(
    config: Option<&AuthFileConfig>,
    env_vars: impl IntoIterator<Item = (String, String)>,
) -> Result<auth_config::AuthConfig> {
    let mut merged: HashMap<String, String> = HashMap::new();

    if let Some(config) = config {
        insert_if_some(&mut merged, "LABBY_AUTH_MODE", config.mode.clone());
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_ENABLE_DYNAMIC_REGISTRATION",
            config
                .enable_dynamic_registration
                .map(|enabled| enabled.to_string()),
        );
        insert_if_some(&mut merged, "LABBY_PUBLIC_URL", config.public_url.clone());
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_SQLITE_PATH",
            config
                .sqlite_path
                .as_ref()
                .map(|path| path.display().to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_KEY_PATH",
            config
                .key_path
                .as_ref()
                .map(|path| path.display().to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_BOOTSTRAP_SECRET",
            config.bootstrap_secret.clone(),
        );
        if let Some(patterns) = config.allowed_client_redirect_uris.as_ref() {
            merged.insert(
                "LABBY_AUTH_ALLOWED_REDIRECT_URIS".to_string(),
                patterns.join(","),
            );
        }
        if let Some(domains) = config.allowed_email_domains.as_ref() {
            merged.insert(
                "LABBY_AUTH_ALLOWED_EMAIL_DOMAINS".to_string(),
                domains.join(","),
            );
        }
        if let Some(domains) = config.viewer_email_domains.as_ref() {
            merged.insert(
                "LABBY_AUTH_VIEWER_EMAIL_DOMAINS".to_string(),
                domains.join(","),
            );
        }
        insert_if_some(
            &mut merged,
            "LABBY_GOOGLE_CLIENT_ID",
            config.google_client_id.clone(),
        );
        insert_if_some(&mut merged, "LABBY_AUTH_PROVIDER", config.provider.clone());
        insert_if_some(
            &mut merged,
            "LABBY_AUTHELIA_ISSUER_URL",
            config.authelia_issuer_url.clone(),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTHELIA_CLIENT_ID",
            config.authelia_client_id.clone(),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTHELIA_CLIENT_SECRET",
            config.authelia_client_secret.clone(),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTHELIA_TRUSTED_PRIVATE_ORIGIN",
            config.authelia_trusted_private_origin.clone(),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTHELIA_CA_CERT_PATH",
            config
                .authelia_ca_certificate_path
                .as_ref()
                .map(|path| path.display().to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_GOOGLE_CLIENT_SECRET",
            config.google_client_secret.clone(),
        );
        insert_if_some(
            &mut merged,
            "LABBY_GOOGLE_CALLBACK_PATH",
            config.google_callback_path.clone(),
        );
        if let Some(scopes) = config.google_scopes.as_ref() {
            insert_if_some(&mut merged, "LABBY_GOOGLE_SCOPES", Some(scopes.join(",")));
        }
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_ACCESS_TOKEN_TTL_SECS",
            config.access_token_ttl_secs.map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_REFRESH_TOKEN_TTL_SECS",
            config.refresh_token_ttl_secs.map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_CODE_TTL_SECS",
            config.auth_code_ttl_secs.map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_ADMIN_EMAIL",
            config.admin_email.clone(),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_REGISTER_REQUESTS_PER_MINUTE",
            config
                .register_requests_per_minute
                .map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_AUTHORIZE_REQUESTS_PER_MINUTE",
            config
                .authorize_requests_per_minute
                .map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_TOKEN_REQUESTS_PER_MINUTE",
            config
                .token_requests_per_minute
                .map(|value| value.to_string()),
        );
        if let Some(machine_clients) = config.machine_clients.as_ref() {
            merged.insert(
                "LABBY_AUTH_MACHINE_CLIENTS_JSON".to_string(),
                serde_json::to_string(machine_clients).context("serialize auth.machine_clients")?,
            );
        }
        if let Some(enterprise_issuers) = config.enterprise_issuers.as_ref() {
            merged.insert(
                "LABBY_AUTH_ENTERPRISE_ISSUERS_JSON".to_string(),
                serde_json::to_string(enterprise_issuers)
                    .context("serialize auth.enterprise_issuers")?,
            );
        }
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_MAX_PENDING_OAUTH_STATES",
            config
                .max_pending_oauth_states
                .map(|value| value.to_string()),
        );
        insert_if_some(
            &mut merged,
            "LABBY_AUTH_CODEX_ISSUER_COMPATIBILITY",
            config
                .codex_issuer_compatibility
                .map(|value| value.to_string()),
        );
    }

    for (key, value) in env_vars {
        if key.starts_with("LABBY_AUTH_")
            || key == "LABBY_PUBLIC_URL"
            || key.starts_with("LABBY_GOOGLE_")
            || key.starts_with("LABBY_AUTHELIA_")
            || key == "LABBY_TOKEN_ENCRYPTION_KEY"
        {
            merged.insert(key, value);
        }
    }

    merged
        .entry("LABBY_AUTH_ALLOWED_REDIRECT_URIS".to_string())
        .or_insert_with(|| DEFAULT_CLIENT_REDIRECT_URI_PATTERNS.join(","));

    // An explicit provider selection owns precedence across configuration
    // sources. Credentials for the non-selected legacy provider must not make
    // an env override appear ambiguous.
    match merged.get("LABBY_AUTH_PROVIDER").map(String::as_str) {
        Some("authelia") => {
            merged.remove("LABBY_GOOGLE_CLIENT_ID");
            merged.remove("LABBY_GOOGLE_CLIENT_SECRET");
        }
        Some("google") => {
            merged.remove("LABBY_AUTHELIA_ISSUER_URL");
            merged.remove("LABBY_AUTHELIA_CLIENT_ID");
            merged.remove("LABBY_AUTHELIA_CLIENT_SECRET");
            merged.remove("LABBY_AUTHELIA_TRUSTED_PRIVATE_ORIGIN");
            merged.remove("LABBY_AUTHELIA_CA_CERT_PATH");
        }
        _ => {}
    }

    let dynamic_registration = match merged
        .get("LABBY_AUTH_ENABLE_DYNAMIC_REGISTRATION")
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        None | Some("true" | "1") => true,
        Some("false" | "0") => false,
        Some(_) => {
            anyhow::bail!("LABBY_AUTH_ENABLE_DYNAMIC_REGISTRATION must be true, false, 1, or 0")
        }
    };
    let resolved = auth_config::AuthConfigBuilder::new()
        .enable_dynamic_registration(dynamic_registration)
        .env_prefix("LABBY")
        .build_from_sources(merged)
        .map_err(anyhow::Error::from)?;
    if !resolved.viewer_email_domains.is_empty()
        && config
            .and_then(|config| config.viewer_project_id.as_deref())
            .is_none_or(|project| {
                project.is_empty()
                    || project.len() > 96
                    || project.trim() != project
                    || project.chars().any(char::is_control)
            })
    {
        anyhow::bail!("auth.viewer_project_id is required when viewer_email_domains is enabled");
    }
    Ok(resolved)
}

fn insert_if_some(target: &mut HashMap<String, String>, key: &str, value: Option<String>) {
    if let Some(value) = value
        && !value.trim().is_empty()
    {
        target.insert(key.to_string(), value);
    }
}

/// Load `.env` + `config.toml` from the standard locations.
///
/// These map to `LABBY_LOG` and `LABBY_LOG_FORMAT` env vars but live in TOML so
/// operators don't need to clutter `.env` with non-secret preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogPreferences {
    /// Tracing filter directive (e.g. `"labby=info,labby_apis=warn"`).
    /// Overridden by `LABBY_LOG` env var.
    #[serde(default)]
    pub filter: Option<String>,
    /// Log format: `"text"` (default) or `"json"`.
    /// Overridden by `LABBY_LOG_FORMAT` env var.
    #[serde(default)]
    pub format: Option<String>,
    /// Force or disable ANSI color: `"force"`/`"always"`/`"1"` or
    /// `"plain"`/`"never"`/`"0"`. Overridden by `LABBY_LOG_COLOR` env var.
    /// This field is read directly from `config.toml` at startup, before
    /// `.env` loads, so it is the only reliable way to set log color from a
    /// file rather than real process/shell env.
    #[serde(default)]
    pub color: Option<String>,
    /// Directory for rolling log files. Defaults to `~/.local/share/labby/logs`.
    /// Overridden by `LABBY_LOG_DIR` env var. Read directly from `config.toml`
    /// at startup, before `.env` loads, for the same reason as `color`.
    #[serde(default)]
    pub dir: Option<PathBuf>,
}

/// Local-master log store and retention preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalLogsPreferences {
    /// Optional path override for the embedded log store.
    #[serde(default)]
    pub store_path: Option<PathBuf>,
    /// Retention window in days.
    #[serde(default)]
    pub retention_days: Option<u64>,
    /// Max retained logical bytes. Oldest events are evicted first.
    #[serde(default)]
    pub max_bytes: Option<u64>,
    /// Bounded ingest queue size for the long-lived runtime.
    #[serde(default)]
    pub queue_capacity: Option<usize>,
    /// Bounded live-subscriber ring size for the SSE stream hub.
    #[serde(default)]
    pub subscriber_capacity: Option<usize>,
}

/// HTTP API preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiPreferences {
    /// Additional CORS origins (comma-separated string or TOML array).
    /// Loopback origins are always included.
    /// Overridden by `LABBY_CORS_ORIGINS` env var.
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// Enable additional dev-only CORS origins (3000/5173/8080). Default: off.
    /// Overridden by `LABBY_DEV_MODE=1` env var.
    #[serde(default)]
    pub dev_mode: Option<bool>,
    /// Connect timeout in seconds for protected MCP route backends.
    /// Overridden by `LABBY_PROTECTED_MCP_CONNECT_TIMEOUT_SECS` env var.
    #[serde(default)]
    pub protected_mcp_connect_timeout_secs: Option<u64>,
    /// Trust reverse-proxy authority headers for virtual protected-route
    /// selection. Off by default: direct clients control these headers.
    #[serde(default)]
    pub trust_forwarded_headers: bool,
}

// `WebPreferences` moved to `labby_runtime::gateway_config`; re-exported above.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebUiAuthDisabledEnv {
    pub disabled: bool,
    pub source: &'static str,
    pub legacy_alias: bool,
}

pub fn resolve_web_ui_auth_disabled_env() -> Result<Option<WebUiAuthDisabledEnv>> {
    resolve_web_ui_auth_disabled_values(
        std::env::var(WEB_UI_AUTH_DISABLED_ENV).ok().as_deref(),
        std::env::var(WEB_UI_AUTH_DISABLED_LEGACY_ENV)
            .ok()
            .as_deref(),
    )
}

pub fn resolve_web_ui_auth_disabled_values(
    canonical: Option<&str>,
    legacy: Option<&str>,
) -> Result<Option<WebUiAuthDisabledEnv>> {
    if let Some(value) = canonical.filter(|value| !value.trim().is_empty()) {
        return Ok(Some(WebUiAuthDisabledEnv {
            disabled: parse_web_ui_auth_disabled_bool(WEB_UI_AUTH_DISABLED_ENV, value)?,
            source: WEB_UI_AUTH_DISABLED_ENV,
            legacy_alias: false,
        }));
    }

    if let Some(value) = legacy.filter(|value| !value.trim().is_empty()) {
        return Ok(Some(WebUiAuthDisabledEnv {
            disabled: parse_web_ui_auth_disabled_bool(WEB_UI_AUTH_DISABLED_LEGACY_ENV, value)?,
            source: WEB_UI_AUTH_DISABLED_LEGACY_ENV,
            legacy_alias: true,
        }));
    }

    Ok(None)
}

fn parse_web_ui_auth_disabled_bool(name: &str, value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => anyhow::bail!("invalid {name} value `{value}`; expected true/false or 1/0"),
    }
}

/// Shared workspace root for Lab-managed files.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePreferences {
    /// Root directory used by the supported filesystem browser.
    /// Defaults to `~/.labby/workspace`.
    #[serde(default)]
    pub root: Option<PathBuf>,
}

/// Durable storage preferences for the principal-scoped File Stash.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileStashPreferences {
    /// Dedicated metadata and blob root. Defaults to `~/.labby/file-stash`.
    #[serde(default)]
    pub root: Option<PathBuf>,
    #[serde(default = "default_stash_file_bytes")]
    pub max_file_bytes: u64,
    #[serde(default = "default_stash_principal_bytes")]
    pub principal_quota_bytes: u64,
    #[serde(default = "default_stash_instance_bytes")]
    pub instance_quota_bytes: u64,
    #[serde(default = "default_stash_live_files")]
    pub max_live_files_per_principal: u32,
    #[serde(default = "default_stash_instance_live_files")]
    pub max_live_files_per_instance: u32,
    #[serde(default = "default_stash_page_size")]
    pub page_size: u16,
    #[serde(default = "default_stash_query_bytes")]
    pub max_query_bytes: usize,
    #[serde(default = "default_stash_header_bytes")]
    pub max_header_bytes: usize,
    #[serde(default = "default_stash_recipients")]
    pub grant_recipients_page_size: u16,
    #[serde(default = "default_stash_mcp_read_bytes")]
    pub max_mcp_read_bytes: u64,
    #[serde(default = "default_stash_queue_capacity")]
    pub queue_capacity: usize,
    #[serde(default = "default_stash_database_deadline_ms")]
    pub database_deadline_ms: u64,
    #[serde(default = "default_stash_principal_uploads")]
    pub max_concurrent_uploads_per_principal: usize,
    #[serde(default = "default_stash_instance_uploads")]
    pub max_concurrent_uploads_per_instance: usize,
    #[serde(default = "default_stash_downloads")]
    pub max_concurrent_downloads: usize,
    #[serde(default = "default_stash_mcp_reads")]
    pub max_concurrent_mcp_reads: usize,
    #[serde(default = "default_stash_idle_seconds")]
    pub upload_idle_seconds: u64,
    #[serde(default = "default_stash_total_seconds")]
    pub upload_total_seconds: u64,
    #[serde(default = "default_stash_idle_seconds")]
    pub download_idle_seconds: u64,
    #[serde(default = "default_stash_total_seconds")]
    pub download_total_seconds: u64,
    #[serde(default = "default_stash_pending_seconds")]
    pub pending_ttl_seconds: u64,
    #[serde(default = "default_stash_janitor_batch")]
    pub janitor_batch_size: usize,
    #[serde(default = "default_stash_janitor_backoff_seconds")]
    pub janitor_backoff_max_seconds: u64,
    #[serde(default = "default_stash_janitor_interval_seconds")]
    pub janitor_interval_seconds: u64,
}

impl Default for FileStashPreferences {
    fn default() -> Self {
        Self {
            root: None,
            max_file_bytes: default_stash_file_bytes(),
            principal_quota_bytes: default_stash_principal_bytes(),
            instance_quota_bytes: default_stash_instance_bytes(),
            max_live_files_per_principal: default_stash_live_files(),
            max_live_files_per_instance: default_stash_instance_live_files(),
            page_size: default_stash_page_size(),
            max_query_bytes: default_stash_query_bytes(),
            max_header_bytes: default_stash_header_bytes(),
            grant_recipients_page_size: default_stash_recipients(),
            max_mcp_read_bytes: default_stash_mcp_read_bytes(),
            queue_capacity: default_stash_queue_capacity(),
            database_deadline_ms: default_stash_database_deadline_ms(),
            max_concurrent_uploads_per_principal: default_stash_principal_uploads(),
            max_concurrent_uploads_per_instance: default_stash_instance_uploads(),
            max_concurrent_downloads: default_stash_downloads(),
            max_concurrent_mcp_reads: default_stash_mcp_reads(),
            upload_idle_seconds: default_stash_idle_seconds(),
            upload_total_seconds: default_stash_total_seconds(),
            download_idle_seconds: default_stash_idle_seconds(),
            download_total_seconds: default_stash_total_seconds(),
            pending_ttl_seconds: default_stash_pending_seconds(),
            janitor_batch_size: default_stash_janitor_batch(),
            janitor_backoff_max_seconds: default_stash_janitor_backoff_seconds(),
            janitor_interval_seconds: default_stash_janitor_interval_seconds(),
        }
    }
}

fn default_stash_file_bytes() -> u64 {
    104_857_600
}
fn default_stash_principal_bytes() -> u64 {
    1_073_741_824
}
fn default_stash_instance_bytes() -> u64 {
    10_737_418_240
}
fn default_stash_live_files() -> u32 {
    1_000
}
fn default_stash_instance_live_files() -> u32 {
    100_000
}
fn default_stash_page_size() -> u16 {
    50
}
fn default_stash_query_bytes() -> usize {
    128
}
fn default_stash_header_bytes() -> usize {
    16_384
}
fn default_stash_recipients() -> u16 {
    50
}
fn default_stash_mcp_read_bytes() -> u64 {
    10_485_760
}
fn default_stash_queue_capacity() -> usize {
    64
}
fn default_stash_database_deadline_ms() -> u64 {
    100
}
fn default_stash_principal_uploads() -> usize {
    2
}
fn default_stash_instance_uploads() -> usize {
    8
}
fn default_stash_downloads() -> usize {
    16
}
fn default_stash_mcp_reads() -> usize {
    4
}
fn default_stash_idle_seconds() -> u64 {
    30
}
fn default_stash_total_seconds() -> u64 {
    600
}
fn default_stash_pending_seconds() -> u64 {
    1_800
}
const STASH_PENDING_MARGIN_SECONDS: u64 = 60;
fn default_stash_janitor_batch() -> usize {
    100
}
fn default_stash_janitor_backoff_seconds() -> u64 {
    300
}
fn default_stash_janitor_interval_seconds() -> u64 {
    60
}

impl FileStashPreferences {
    fn validate(&self) -> Result<(), ConfigError> {
        let valid = (1..=1_073_741_824).contains(&self.max_file_bytes)
            && (self.max_file_bytes..=107_374_182_400).contains(&self.principal_quota_bytes)
            && (self.principal_quota_bytes..=1_099_511_627_776)
                .contains(&self.instance_quota_bytes)
            && (1..=100_000).contains(&self.max_live_files_per_principal)
            && (self.max_live_files_per_principal..=1_000_000)
                .contains(&self.max_live_files_per_instance)
            && (1..=200).contains(&self.page_size)
            && (1..=1_024).contains(&self.max_query_bytes)
            && (1..=65_536).contains(&self.max_header_bytes)
            && (1..=200).contains(&self.grant_recipients_page_size)
            && (1..=26_214_400).contains(&self.max_mcp_read_bytes)
            && (1..=1_024).contains(&self.queue_capacity)
            && (1..=30_000).contains(&self.database_deadline_ms)
            && (1..=2).contains(&self.max_concurrent_uploads_per_principal)
            && (self.max_concurrent_uploads_per_principal..=8)
                .contains(&self.max_concurrent_uploads_per_instance)
            && (1..=256).contains(&self.max_concurrent_downloads)
            && (1..=4).contains(&self.max_concurrent_mcp_reads)
            && (1..=30).contains(&self.upload_idle_seconds)
            && (self.upload_idle_seconds..=600).contains(&self.upload_total_seconds)
            && (1..=30).contains(&self.download_idle_seconds)
            && (self.download_idle_seconds..=600).contains(&self.download_total_seconds)
            && self.pending_ttl_seconds
                >= self
                    .upload_total_seconds
                    .saturating_add(STASH_PENDING_MARGIN_SECONDS)
            && self.pending_ttl_seconds <= 1_800
            && (1..=100).contains(&self.janitor_batch_size)
            && (1..=300).contains(&self.janitor_backoff_max_seconds)
            && (1..=3_600).contains(&self.janitor_interval_seconds)
            && self.janitor_backoff_max_seconds >= self.janitor_interval_seconds;
        if !valid {
            return Err(ConfigError::InvalidProxyConfig {
                reason:
                    "file_stash limits must be positive and within their documented safety bounds"
                        .into(),
            });
        }
        Ok(())
    }
}

/// OAuth local relay preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OauthPreferences {
    /// Named callback relay targets.
    #[serde(default)]
    pub machines: BTreeMap<String, OauthMachineConfig>,
}

/// A named OAuth callback relay target.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OauthMachineConfig {
    /// Full callback target base URL.
    pub target_url: String,
    /// Optional operator-facing description.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional preferred callback port for the browser-local listener.
    #[serde(default)]
    pub default_port: Option<u16>,
}

/// Admin tool settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminPreferences {
    /// Enable the `lab_admin` MCP tool. Default: `false`.
    /// Overridden by `LABBY_ADMIN_ENABLED=1` env var.
    #[serde(default)]
    pub enabled: bool,
}

/// Per-service preference overrides (non-secret values only).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServicePreferences {
    /// Enable built-in integrations that call external service APIs.
    ///
    /// Default: true. When false, runtime registries keep bootstrap/operator
    /// tools available but remove built-in upstream API integrations.
    #[serde(default = "default_true")]
    pub built_in_upstream_apis_enabled: bool,
    /// Tailscale preferences.
    #[serde(default)]
    pub tailscale: TailscalePreferences,
}

impl Default for ServicePreferences {
    fn default() -> Self {
        Self {
            built_in_upstream_apis_enabled: true,
            tailscale: TailscalePreferences::default(),
        }
    }
}

/// Tailscale non-secret preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TailscalePreferences {
    /// Tailnet name. Overridden by `TAILSCALE_TAILNET` env var.
    /// Default: `"-"` (auto-detect).
    #[serde(default)]
    pub tailnet: Option<String>,
}

/// `[setup]` preferences: operator toggles for `labby setup --provision`.
///
/// These are non-secret capabilities that provision can install on demand,
/// kept out of the baked Incus image to slim it. Env overrides still apply.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupPreferences {
    /// Install `android-sdk` during provision. Needed only by the
    /// claude-in-mobile MCP server. Default: false.
    /// Env override: `LABBY_ENABLE_ANDROID_SDK=1`.
    #[serde(default)]
    pub install_android_sdk: Option<bool>,
}

/// Load `config.toml` only — no `.env`, no side effects beyond file reads.
///
/// Called early in `main()` before tracing is initialized so that `[log]`
/// preferences can feed into `init_tracing()`. Safe to call before any
/// other subsystem.
///
/// Config TOML resolves from the one installation root selected by
/// `LABBY_HOME`, normally `~/.labby/config.toml`.
pub fn load_toml(candidates: &[PathBuf]) -> Result<LabConfig> {
    // Do not let an explicitly invalid LABBY_HOME silently fall through to a
    // different installation's user-home config.
    if std::env::var_os("LABBY_HOME").is_some_and(|value| !value.is_empty()) {
        crate::installation::InstallationPaths::resolve().context("invalid explicit LABBY_HOME")?;
    }
    load_toml_from_paths(candidates)
}

/// Load an already-authoritative fixed path without consulting caller process
/// root variables. Used for lifecycle management of the fixed daemon account.
pub(crate) fn load_toml_from_fixed_root(candidates: &[PathBuf]) -> Result<LabConfig> {
    load_toml_from_paths(candidates)
}

fn load_toml_from_paths(candidates: &[PathBuf]) -> Result<LabConfig> {
    for path in candidates {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => {
                let lock = host_write::HostConfigLock::acquire(path)?;
                let raw = lock.read_raw()?;
                validate_top_level_extension_boundary(&raw)
                    .with_context(|| format!("failed to parse {}", path.display()))?;
                let mut cfg = toml::from_str::<LabConfig>(&raw)
                    .with_context(|| format!("failed to parse {}", path.display()))?;
                cfg.normalize_protected_mcp_routes()
                    .with_context(|| format!("invalid config {}", path.display()))?;
                // Validate all upstream configs eagerly at startup so that
                // invalid configuration (conflicting auth, bad URL scheme, etc.)
                // is discovered immediately rather than at first OAuth attempt.
                cfg.validate()
                    .with_context(|| format!("invalid config {}", path.display()))?;
                return Ok(cfg);
            }
        }
    }
    Ok(LabConfig::default())
}

/// Labby-owned app-surface sections that `raw` (a `config.toml` document)
/// does not declare and therefore inherits at their on-by-default posture:
/// `code_mode` (Code Mode plus its inspector UI) and `mcp_apps` (every
/// Labby-owned MCP App UI). Startup names these once so an install upgraded
/// from a release where they defaulted off sees the change. Unparseable input
/// yields nothing; the config loader owns that error.
#[must_use]
pub fn inherited_app_surface_sections(raw: &str) -> Vec<&'static str> {
    let Ok(table) = raw.parse::<toml::Table>() else {
        return Vec::new();
    };
    ["code_mode", "mcp_apps"]
        .into_iter()
        .filter(|section| !table.contains_key(*section))
        .collect()
}

fn validate_top_level_extension_boundary(raw: &str) -> Result<()> {
    let table = raw.parse::<toml::Table>()?;
    const OWNED: &[&str] = &[
        "config_version",
        "output",
        "mcp",
        "proxy",
        "log",
        "local_logs",
        "api",
        "web",
        "workspace",
        "file_stash",
        "oauth",
        "admin",
        "services",
        "setup",
        "auth",
        "code_mode",
        "mcp_apps",
        "skill_library",
        "upstream_request_timeout_ms",
        "upstream_relay_timeout_ms",
        "upstream",
        "upstream_import_tombstones",
        "upstream_pending",
        "gateway_import_mode",
        "loadouts",
        "protected_mcp_routes",
        "virtual_servers",
        "quarantined_virtual_servers",
        "public_urls",
        "gateway",
        "openapi",
    ];
    for (key, value) in table {
        if !OWNED.contains(&key.as_str()) && !value.is_table() {
            anyhow::bail!("unknown top-level scalar `{key}`; extensions must use a named table")
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigScalarValue {
    Bool(bool),
    I64(i64),
    String(String),
    StringList(Vec<String>),
    UnsetOptional,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigScalarPatch {
    pub path: String,
    pub value: ConfigScalarValue,
}

impl ConfigScalarPatch {
    #[must_use]
    pub fn new(path: impl Into<String>, value: ConfigScalarValue) -> Self {
        Self {
            path: path.into(),
            value,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConfigPatchOutcome {
    pub config: LabConfig,
    pub backup_path: Option<PathBuf>,
    /// A durable commit succeeded, but best-effort backup maintenance did not.
    /// Callers must report this without claiming the requested mutation failed.
    pub maintenance_warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExpectedConfigScalar {
    pub path: String,
    pub value: serde_json::Value,
}

impl ExpectedConfigScalar {
    #[must_use]
    pub fn new(path: impl Into<String>, value: serde_json::Value) -> Self {
        Self {
            path: path.into(),
            value,
        }
    }
}

static CONFIG_BACKUP_COUNTER: AtomicU32 = AtomicU32::new(0);

fn inline_table_to_table(inline: &toml_edit::InlineTable) -> toml_edit::Table {
    let mut table = toml_edit::Table::new();
    for (key, value) in inline {
        table[key] = toml_edit::Item::Value(value.clone());
    }
    table
}

fn set_toml_scalar_path(
    document: &mut toml_edit::DocumentMut,
    dotted_path: &str,
    value: ConfigScalarValue,
) -> Result<()> {
    let parts: Vec<&str> = dotted_path
        .split('.')
        .filter(|part| !part.is_empty())
        .collect();
    anyhow::ensure!(!parts.is_empty(), "config path must not be empty");
    let (leaf, parents) = parts.split_last().expect("non-empty parts");
    let mut item = document.as_item_mut();
    for part in parents {
        let table = item
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("config parent `{part}` is not a table"))?;
        if !table.contains_key(part) {
            table.insert(part, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        let child = table
            .get_mut(part)
            .ok_or_else(|| anyhow::anyhow!("config parent `{part}` was not created"))?;
        if !child.is_table() {
            let converted = child
                .as_value()
                .and_then(toml_edit::Value::as_inline_table)
                .map(inline_table_to_table);
            if let Some(table) = converted {
                *child = toml_edit::Item::Table(table);
            } else {
                anyhow::bail!("config parent `{part}` is not a table");
            }
        }
        item = child;
    }
    if matches!(value, ConfigScalarValue::UnsetOptional) {
        if let Some(table) = item.as_table_mut() {
            table.remove(leaf);
            return Ok(());
        }
        anyhow::bail!("config parent for `{dotted_path}` is not a table");
    }
    item[*leaf] = toml_edit::Item::Value(match value {
        ConfigScalarValue::Bool(value) => toml_edit::Value::from(value),
        ConfigScalarValue::I64(value) => toml_edit::Value::from(value),
        ConfigScalarValue::String(value) => toml_edit::Value::from(value),
        ConfigScalarValue::StringList(values) => {
            let mut array = toml_edit::Array::default();
            for value in values {
                array.push(value);
            }
            toml_edit::Value::Array(array)
        }
        ConfigScalarValue::UnsetOptional => unreachable!("handled above"),
    });
    Ok(())
}

pub fn patch_config_scalars(
    path: &Path,
    entries: &[ConfigScalarPatch],
) -> Result<ConfigPatchOutcome> {
    patch_config_scalars_checked(path, entries, &[])
}

pub fn patch_config_scalars_checked(
    path: &Path,
    entries: &[ConfigScalarPatch],
    expected: &[ExpectedConfigScalar],
) -> Result<ConfigPatchOutcome> {
    let host_lock = host_write::HostConfigLock::acquire(path)?;
    let raw = host_lock.read_raw()?;
    let mut document = raw
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if !expected.is_empty() {
        let mut current_cfg = toml::from_str::<LabConfig>(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        current_cfg
            .normalize_protected_mcp_routes()
            .with_context(|| format!("invalid config {}", path.display()))?;
        current_cfg
            .validate()
            .with_context(|| format!("invalid config {}", path.display()))?;
        for item in expected {
            let current = config_json_value_for_path(&current_cfg, &item.path);
            anyhow::ensure!(
                current == item.value,
                "setting `{}` changed since it was loaded",
                item.path
            );
        }
    }
    for entry in entries {
        set_toml_scalar_path(&mut document, &entry.path, entry.value.clone())
            .with_context(|| format!("failed to patch {}", entry.path))?;
    }
    if document.to_string() != raw && !document.contains_key("config_version") {
        document["config_version"] = toml_edit::value(i64::from(CURRENT_CONFIG_VERSION));
    }
    let patched = document.to_string();
    let mut cfg = toml::from_str::<LabConfig>(&patched)
        .with_context(|| format!("failed to parse patched {}", path.display()))?;
    cfg.normalize_protected_mcp_routes()
        .with_context(|| format!("invalid patched config {}", path.display()))?;
    cfg.validate()
        .with_context(|| format!("invalid patched config {}", path.display()))?;

    if patched == raw {
        return Ok(ConfigPatchOutcome {
            config: cfg,
            backup_path: None,
            maintenance_warning: None,
        });
    }

    let backup_path = if path.exists() {
        Some(backup_config_file(path, &raw)?)
    } else {
        None
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    host_lock.write(&patched)?;
    let maintenance_warning = (|| -> Result<()> {
        #[cfg(test)]
        if parent
            .join(".labby-test-config-maintenance-failure")
            .exists()
        {
            anyhow::bail!("injected post-commit maintenance failure");
        }
        sync_config_parent(parent)
            .with_context(|| format!("parent sync failed for {}", path.display()))?;
        if prune_config_backups(parent, path)? > 0 {
            sync_config_parent(parent)
                .with_context(|| format!("backup-prune sync failed for {}", path.display()))?;
        }
        Ok(())
    })()
    .err()
    .map(|error| {
        format!("configuration was committed, but post-commit maintenance failed: {error:#}")
    });

    Ok(ConfigPatchOutcome {
        config: cfg,
        backup_path,
        maintenance_warning,
    })
}

#[cfg(unix)]
fn sync_config_parent(parent: &Path) -> std::io::Result<()> {
    OpenOptions::new().read(true).open(parent)?.sync_all()
}

#[cfg(windows)]
fn sync_config_parent(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

fn prune_config_backups(parent: &Path, target: &Path) -> Result<usize> {
    let Some(target_name) = target.file_name().and_then(|name| name.to_str()) else {
        return Ok(0);
    };
    let prefix = format!("{target_name}.bak.");
    let backups = std::fs::read_dir(parent)
        .with_context(|| format!("read config backup directory {}", parent.display()))?
        .map(|entry| {
            entry.with_context(|| format!("read config backup entry in {}", parent.display()))
        })
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_name().to_string_lossy().starts_with(&prefix) => {
                Some(Ok(entry))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .map(|entry| {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry