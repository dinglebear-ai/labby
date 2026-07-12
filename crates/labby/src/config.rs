//! Config loading for the `lab` binary.
//!
//! Order of precedence (highest wins):
//!   1. CLI flags / process environment variables
//!   2. `~/.labby/.env` (loaded via `dotenvy`)
//!   3. `config.toml` (searched: `./` → `~/.labby/` → `~/.config/labby/`)
//!   4. Built-in defaults
//!
//! Service credentials and instance endpoints belong in `.env`. Non-secret
//! operator preferences and defaults (logging, CORS, MCP transport, admin
//! flags, registry URLs, workspace roots) belong in `config.toml`.
//!
//! Multi-instance services follow the `S_<LABEL>_URL` pattern: a service
//! like `unraid` reads `UNRAID_URL` as the default instance and
//! `UNRAID_NODE2_URL` as an additional instance labeled `node2`.

pub mod env_merge;

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

pub(crate) fn process_code_mode_enabled() -> bool {
    PROCESS_CODE_MODE_ENABLED.load(Ordering::Acquire)
}

/// Parse a boolean env flag using the standard truthy set
/// (`1` / `true` / `TRUE` / `yes` / `YES`). Absent or any other value is false.
pub(crate) fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

fn truthy_env_flag_value(value: &str) -> bool {
    matches!(value, "1" | "true" | "TRUE" | "yes" | "YES")
}

/// Whether upstream server→client request relay is enabled for proxied tool calls.
///
/// `LABBY_UPSTREAM_RELAY_ENABLED` is the clearer current name because the relay
/// covers every mirrored server→client capability (elicitation, sampling, and
/// roots). `LABBY_UPSTREAM_RELAY_ELICITATION` remains supported for existing
/// deployments that enabled the original narrower flag.
pub(crate) fn upstream_relay_enabled() -> bool {
    resolve_upstream_relay_enabled(
        std::env::var("LABBY_UPSTREAM_RELAY_ENABLED")
            .ok()
            .as_deref(),
        std::env::var("LABBY_UPSTREAM_RELAY_ELICITATION")
            .ok()
            .as_deref(),
    )
}

fn resolve_upstream_relay_enabled(current: Option<&str>, legacy: Option<&str>) -> bool {
    current.is_some_and(truthy_env_flag_value) || legacy.is_some_and(truthy_env_flag_value)
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

#[cfg(test)]
fn resolve_destructive_elicitation_timeout_ms(
    env_value: Option<&str>,
    config_value: Option<u64>,
) -> Option<u64> {
    env_value
        .and_then(|raw| parse_bounded_ms(raw, MAX_DESTRUCTIVE_ELICITATION_TIMEOUT_MS))
        .or(config_value)
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
static RESOLVED_SYMBOLS: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static RESOLVED_PROTECTED_MCP_TIMEOUT_SECS: OnceLock<Mutex<Option<u64>>> = OnceLock::new();
static RESOLVED_DESTRUCTIVE_ELICITATION_TIMEOUT_MS: OnceLock<Mutex<Option<u64>>> = OnceLock::new();
static RESOLVED_CATALOG_NOTIFICATION_TIMEOUT_MS: OnceLock<Mutex<Option<u64>>> = OnceLock::new();

fn resolved_symbols_cell() -> &'static Mutex<Option<String>> {
    RESOLVED_SYMBOLS.get_or_init(|| Mutex::new(None))
}

fn resolved_protected_mcp_timeout_cell() -> &'static Mutex<Option<u64>> {
    RESOLVED_PROTECTED_MCP_TIMEOUT_SECS.get_or_init(|| Mutex::new(None))
}

fn resolved_destructive_elicitation_timeout_cell() -> &'static Mutex<Option<u64>> {
    RESOLVED_DESTRUCTIVE_ELICITATION_TIMEOUT_MS.get_or_init(|| Mutex::new(None))
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
    let destructive_elicitation_timeout_ms =
        std::env::var("LABBY_MCP_DESTRUCTIVE_ELICITATION_TIMEOUT_MS")
            .ok()
            .and_then(|raw| {
                parse_bounded_ms_env(
                    "LABBY_MCP_DESTRUCTIVE_ELICITATION_TIMEOUT_MS",
                    &raw,
                    MAX_DESTRUCTIVE_ELICITATION_TIMEOUT_MS,
                )
            })
            .or(config.mcp.destructive_elicitation_timeout_ms);
    *resolved_destructive_elicitation_timeout_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = destructive_elicitation_timeout_ms;
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

pub(crate) fn resolved_destructive_elicitation_timeout() -> Duration {
    Duration::from_millis(
        resolved_destructive_elicitation_timeout_cell()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unwrap_or(DEFAULT_DESTRUCTIVE_ELICITATION_TIMEOUT_MS),
    )
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
use tempfile::NamedTempFile;

pub const DEFAULT_MCPREGISTRY_URL: &str = "https://registry.modelcontextprotocol.io";
pub const WEB_UI_AUTH_DISABLED_ENV: &str = "LABBY_WEB_UI_AUTH_DISABLED";
pub const WEB_UI_AUTH_DISABLED_LEGACY_ENV: &str = "LABBY_WEB_UI_DISABLE_AUTH";
const DEFAULT_UPSTREAM_REQUEST_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_DESTRUCTIVE_ELICITATION_TIMEOUT_MS: u64 = 120_000;
const MAX_DESTRUCTIVE_ELICITATION_TIMEOUT_MS: u64 = 600_000;
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

#[cfg(test)]
static TEST_CONFIG_TOML_PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn set_test_config_toml_path(path: Option<PathBuf>) {
    let slot = TEST_CONFIG_TOML_PATH.get_or_init(|| Mutex::new(None));
    *slot.lock().expect("test config path lock") = path;
}

/// Fully-resolved `lab` configuration, assembled from env + TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LabConfig {
    /// Default output format for CLI commands that print tables.
    #[serde(default)]
    pub output: OutputPreferences,
    /// MCP server defaults.
    #[serde(default)]
    pub mcp: McpPreferences,
    /// Logging preferences (overridden by `LABBY_LOG` / `LABBY_LOG_FORMAT` env vars).
    #[serde(default)]
    pub log: LogPreferences,
    /// Local-master log subsystem preferences.
    #[serde(default)]
    pub local_logs: Option<LocalLogsPreferences>,
    /// HTTP API preferences.
    #[serde(default)]
    pub api: ApiPreferences,
    /// Web UI preferences.
    #[serde(default)]
    pub web: WebPreferences,
    /// Shared Lab workspace root. Backs the read-only attachment picker and
    /// local writable stash workspaces.
    #[serde(default)]
    pub workspace: WorkspacePreferences,
    /// MCP Registry upstream preferences.
    #[serde(default)]
    pub mcpregistry: McpRegistryPreferences,
    /// OAuth callback relay preferences.
    #[serde(default)]
    pub oauth: OauthPreferences,
    /// Device runtime preferences.
    #[serde(default)]
    pub device: Option<DevicePreferences>,
    /// Node runtime preferences.
    #[serde(default)]
    pub node: Option<NodePreferences>,
    /// Admin tool settings.
    #[serde(default)]
    pub admin: AdminPreferences,
    /// Per-service preference overrides.
    #[serde(default)]
    pub services: ServicePreferences,
    /// HTTP auth mode preferences.
    #[serde(default)]
    pub auth: Option<AuthFileConfig>,
    /// Gateway-wide Code Mode exposure and execution settings.
    #[serde(default)]
    pub code_mode: CodeModeConfig,
    /// Maximum time to wait for one proxied upstream MCP tool/resource/prompt response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_request_timeout_ms: Option<u64>,
    /// Maximum time to wait for one *relayed* upstream tool call — the opt-in
    /// path (`LABBY_UPSTREAM_RELAY_ELICITATION`) that forwards an upstream's
    /// `elicitation/create`/`sampling`/`roots` request down to the downstream
    /// agent. Because a relayed call blocks on a human answering an elicitation,
    /// it gets its own, longer deadline instead of `upstream_request_timeout_ms`
    /// (default 5 minutes; see [`LabConfig::upstream_relay_timeout`]).
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
    /// Deploy service preferences (feature-gated at the consumer level).
    #[serde(default)]
    pub deploy: Option<DeployPreferences>,
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

/// `[openapi]` config section: a list of `[[openapi.specs]]` tables.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenApiTomlSection {
    /// Configured specs.
    #[serde(default)]
    pub specs: Vec<OpenApiSpecToml>,
}

/// One `[[openapi.specs]]` table. Non-secret fields only.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
            upstream_request_timeout_ms: self.upstream_request_timeout_ms,
            upstream_relay_timeout_ms: self.upstream_relay_timeout_ms,
            upstream: self.upstream.clone(),
            upstream_import_tombstones: self.upstream_import_tombstones.clone(),
            upstream_pending: self.upstream_pending.clone(),
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
        self.upstream_request_timeout_ms = gw.upstream_request_timeout_ms;
        self.upstream_relay_timeout_ms = gw.upstream_relay_timeout_ms;
        self.upstream = gw.upstream.clone();
        self.upstream_import_tombstones = gw.upstream_import_tombstones.clone();
        self.upstream_pending = gw.upstream_pending.clone();
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

/// Deploy service preferences — defaults plus per-host overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployPreferences {
    #[serde(default)]
    pub defaults: Option<DeployDefaults>,
    #[serde(default)]
    pub hosts: BTreeMap<String, DeployHostOverride>,
}

/// Artifact role for deploy targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRole {
    Controller,
    Node,
}

/// Default policy applied to every deploy target unless overridden.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployDefaults {
    pub remote_path: Option<String>,
    pub service: Option<String>,
    #[serde(default)]
    pub restart: Option<RestartModel>,
    pub service_scope: Option<ServiceScope>,
    pub max_parallel: Option<u32>,
    #[serde(default)]
    pub canary_hosts: Vec<String>,
    /// Base URL of the master lab instance that deployed hosts should phone home to.
    /// e.g. "http://node-a:8765". If absent, phone-home is skipped.
    pub master_url: Option<String>,
    /// Artifact role for this deploy target.
    #[serde(default)]
    pub artifact_role: Option<ArtifactRole>,
    /// Cross-compilation target triple, e.g. "aarch64-unknown-linux-gnu".
    #[serde(default)]
    pub target_triple: Option<String>,
    /// Maximum build time in seconds before declaring the build failed.
    #[serde(default)]
    pub build_timeout_secs: Option<u64>,
}

/// Per-host policy overrides for deploy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployHostOverride {
    pub remote_path: Option<String>,
    pub service: Option<String>,
    #[serde(default)]
    pub restart: Option<RestartModel>,
    pub service_scope: Option<ServiceScope>,
    /// Artifact role override for this specific host.
    #[serde(default)]
    pub artifact_role: Option<ArtifactRole>,
    /// Cross-compilation target triple override for this specific host.
    #[serde(default)]
    pub target_triple: Option<String>,
    /// Build timeout override in seconds for this specific host.
    #[serde(default)]
    pub build_timeout_secs: Option<u64>,
}

/// Restart policy used by rollout/update flows after a binary install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RestartModel {
    SystemService { service: String },
    UserService { service: String },
    WrapperCommand { command: Vec<String> },
}

/// Systemd scope for the unit restarted by deploy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceScope {
    System,
    User,
}

/// Device runtime preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DevicePreferences {
    #[serde(default)]
    pub master: Option<String>,
}

/// Explicit runtime role for this node, set in config or via CLI `--role`.
///
/// This is the user-facing vocabulary; the internal runtime maps
/// `Controller → NodeRole::Master` and `Node → NodeRole::NonMaster`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRuntimeRole {
    Controller,
    Node,
}

/// Node runtime preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodePreferences {
    #[serde(default)]
    pub controller: Option<String>,
    /// How many days of node logs to retain in the SQLite log store.
    /// Defaults to 30 days when absent.
    #[serde(default)]
    pub log_retention_days: Option<u32>,
    /// Explicit runtime role for this device.
    /// When present, skips hostname-based role inference.
    #[serde(default)]
    pub role: Option<NodeRuntimeRole>,
}

/// Runtime role for the current device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceRole {
    Master,
    NonMaster,
}

/// Alias for [`DeviceRole`] used after the `device → node` module rename.
pub type NodeRole = DeviceRole;

/// Alias for [`ResolvedDeviceRuntime`] used after the `device → node` module rename.
pub type ResolvedNodeRuntime = ResolvedDeviceRuntime;

/// Resolved device runtime configuration after comparing local and master hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDeviceRuntime {
    pub local_host: String,
    pub master_host: String,
    pub role: DeviceRole,
}

impl LabConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.code_mode.validate()?;
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
        if let Some(value) = self.mcp.destructive_elicitation_timeout_ms
            && !(1..=MAX_DESTRUCTIVE_ELICITATION_TIMEOUT_MS).contains(&value)
        {
            return Err(ConfigError::InvalidDestructiveElicitationTimeout { value });
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
    /// to [`DEFAULT_UPSTREAM_RELAY_TIMEOUT_MS`] (5 minutes) when unset.
    pub fn upstream_relay_timeout(&self) -> Duration {
        Duration::from_millis(
            self.upstream_relay_timeout_ms
                .unwrap_or(DEFAULT_UPSTREAM_RELAY_TIMEOUT_MS),
        )
    }

    pub fn normalize_protected_mcp_routes(&mut self) -> Result<(), ConfigError> {
        for route in &mut self.protected_mcp_routes {
            route.upstream = route
                .upstream
                .take()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty());
            if let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = &mut route.target {
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
        validate_gateway_subset_paths_are_unique(&self.protected_mcp_routes)?;
        Ok(())
    }

    #[must_use]
    pub fn controller_host(&self) -> Option<&str> {
        self.node
            .as_ref()
            .and_then(|prefs| prefs.controller.as_deref())
            .or_else(|| {
                self.device
                    .as_ref()
                    .and_then(|prefs| prefs.master.as_deref())
            })
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

fn validate_gateway_subset_paths_are_unique(
    routes: &[ProtectedMcpRouteConfig],
) -> Result<(), ConfigError> {
    let mut paths = std::collections::HashSet::new();
    for route in routes
        .iter()
        .filter(|route| route.enabled && route.is_gateway_subset())
    {
        if !paths.insert(route.public_path.clone()) {
            return Err(ConfigError::InvalidProtectedRoute {
                name: route.name.clone(),
                field: "public_path",
                value: format!(
                    "gateway_subset routes must use unique public_path values; `{}` is already mounted",
                    route.public_path
                ),
            });
        }
    }
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
        .map(|service| service.name)
        .collect();

    validate_gateway_subset_paths_are_unique(&cfg.protected_mcp_routes)?;
    for route in &cfg.protected_mcp_routes {
        validate_protected_mcp_route_for_startup(route, &upstream_names, &service_names)?;
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
        if target.upstreams.is_empty() && target.services.is_empty() && !target.expose_code_mode {
            return invalid_protected_route(
                route,
                "target",
                "gateway_subset target must expose at least one upstream, service, or Code Mode",
            );
        }
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
    if lower.starts_with("/.well-known") || lower.starts_with("/v1") {
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
// `lab-gateway`; keep them as the public `labby::config` surface and silence the
// bin-target unused-import lint.
#[allow(unused_imports)]
pub use labby_runtime::gateway_config::{
    CodeModeConfig, CodeModeResultShapePolicy, ConfigError, GatewayConfig, GatewayImportMode,
    GatewayPreferences, ImportSource, ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig,
    ProtectedMcpRouteEffectiveTarget, ProtectedMcpRouteTarget, ResolvedPublicUrls, UpstreamConfig,
    UpstreamImportTombstone, UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
    VirtualServerConfig, VirtualServerMcpPolicyConfig, VirtualServerSurfacesConfig, WebPreferences,
    default_mcp_path, default_true, normalize_protected_backend_url,
};
// Re-exported for the public `labby::config` API surface (consumed by the
// `upstream_oauth` integration test); not referenced within the binary build,
// so silence the bin-target unused-import lint.
#[allow(unused_imports)]
pub use labby_runtime::gateway_config::canonicalize_upstream_url;

/// Table/json formatting defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
pub struct McpPreferences {
    /// Default transport (`stdio` or `http`).
    #[serde(default)]
    pub transport: Option<String>,
    /// Default bind address for the HTTP transport.
    #[serde(default)]
    pub host: Option<String>,
    /// Default port for the HTTP transport.
    #[serde(default)]
    pub port: Option<u16>,
    /// Default session keep-alive TTL in seconds for HTTP MCP sessions.
    #[serde(default)]
    pub session_ttl_secs: Option<u64>,
    /// Whether HTTP MCP should use stateful sessions by default.
    #[serde(default)]
    pub stateful: Option<bool>,
    /// Additional allowed hosts for DNS rebinding protection.
    #[serde(default)]
    pub allowed_hosts: Option<Vec<String>>,
    /// Show the full service catalog regardless of env-var presence.
    /// Overridden by `LABBY_SHOW_ALL` env var.
    #[serde(default)]
    pub show_all: Option<bool>,
    /// Maximum time to wait for a downstream MCP client to answer a destructive
    /// action confirmation elicitation.
    /// Overridden by `LABBY_MCP_DESTRUCTIVE_ELICITATION_TIMEOUT_MS`.
    #[serde(default)]
    pub destructive_elicitation_timeout_ms: Option<u64>,
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
    /// Bootstrap secret required for dynamic client registration.
    #[serde(default)]
    pub bootstrap_secret: Option<String>,
    /// Additional redirect URI patterns allowed for dynamic client registration.
    #[serde(default)]
    pub allowed_client_redirect_uris: Option<Vec<String>>,
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
    /// Max in-flight OAuth state rows. Overridden by
    /// `LABBY_AUTH_MAX_PENDING_OAUTH_STATES`.
    #[serde(default)]
    pub max_pending_oauth_states: Option<usize>,
}

const DEFAULT_CLIENT_REDIRECT_URI_PATTERNS: &[&str] = &[
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
    let mut merged: HashMap<String, String> = HashMap::new();

    if let Some(config) = config {
        insert_if_some(&mut merged, "LABBY_AUTH_MODE", config.mode.clone());
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
        insert_if_some(
            &mut merged,
            "LABBY_GOOGLE_CLIENT_ID",
            config.google_client_id.clone(),
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
            "LABBY_AUTH_MAX_PENDING_OAUTH_STATES",
            config
                .max_pending_oauth_states
                .map(|value| value.to_string()),
        );
    }

    for (key, value) in std::env::vars() {
        if key.starts_with("LABBY_AUTH_")
            || key == "LABBY_PUBLIC_URL"
            || key.starts_with("LABBY_GOOGLE_")
        {
            merged.insert(key, value);
        }
    }

    merged
        .entry("LABBY_AUTH_ALLOWED_REDIRECT_URIS".to_string())
        .or_insert_with(|| DEFAULT_CLIENT_REDIRECT_URI_PATTERNS.join(","));

    auth_config::AuthConfigBuilder::new()
        .env_prefix("LABBY")
        .build_from_sources(merged)
        .map_err(anyhow::Error::from)
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
pub struct WorkspacePreferences {
    /// Root directory used by fs browsing and stash-backed writable workspaces.
    /// Defaults to `~/.labby/stash`.
    #[serde(default)]
    pub root: Option<PathBuf>,
}

/// MCP Registry upstream preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRegistryPreferences {
    /// Upstream MCP Registry base URL.
    #[serde(default = "default_mcpregistry_url_option")]
    pub url: Option<String>,
}

impl Default for McpRegistryPreferences {
    fn default() -> Self {
        Self {
            url: default_mcpregistry_url_option(),
        }
    }
}

fn default_mcpregistry_url_option() -> Option<String> {
    Some(DEFAULT_MCPREGISTRY_URL.to_string())
}

/// OAuth local relay preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OauthPreferences {
    /// Named callback relay targets.
    #[serde(default)]
    pub machines: BTreeMap<String, OauthMachineConfig>,
}

/// A named OAuth callback relay target.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct AdminPreferences {
    /// Enable the `lab_admin` MCP tool. Default: `false`.
    /// Overridden by `LABBY_ADMIN_ENABLED=1` env var.
    #[serde(default)]
    pub enabled: bool,
}

/// Per-service preference overrides (non-secret values only).
#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct TailscalePreferences {
    /// Tailnet name. Overridden by `TAILSCALE_TAILNET` env var.
    /// Default: `"-"` (auto-detect).
    #[serde(default)]
    pub tailnet: Option<String>,
}

/// Load `config.toml` only — no `.env`, no side effects beyond file reads.
///
/// Called early in `main()` before tracing is initialized so that `[log]`
/// preferences can feed into `init_tracing()`. Safe to call before any
/// other subsystem.
///
/// Config TOML resolution (first found wins):
///   1. `./config.toml` (repo/CWD override)
///   2. `~/.labby/config.toml` (user-level, colocated with `.env`)
///   3. `~/.config/labby/config.toml` (XDG-style fallback)
pub fn load_toml(candidates: &[PathBuf]) -> Result<LabConfig> {
    for path in candidates {
        match std::fs::read_to_string(path) {
            Ok(raw) => {
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
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(
                    anyhow::Error::new(e).context(format!("failed to read {}", path.display()))
                );
            }
        }
    }
    Ok(LabConfig::default())
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let lock_path = config_lock_path(path);
    let lock_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open {}", lock_path.display()))?;
    let mut lock = fd_lock::RwLock::new(lock_file);
    let _guard = lock
        .try_write()
        .with_context(|| format!("config is locked: {}", lock_path.display()))?;

    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("failed to read {}", path.display())));
        }
    };
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
        });
    }

    let backup_path = if path.exists() {
        Some(backup_config_file(path, &raw)?)
    } else {
        None
    };
    let old_mode = std::fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temp file in {}", parent.display()))?;
    tmp.write_all(patched.as_bytes())
        .context("failed to write temp config")?;
    tmp.as_file()
        .sync_all()
        .context("failed to sync temp config")?;
    if let Some(mode) = old_mode {
        tmp.as_file()
            .set_permissions(mode)
            .context("failed to preserve config mode")?;
    }
    tmp.persist(path)
        .map_err(|e| anyhow::Error::new(e.error))
        .with_context(|| format!("failed to persist {}", path.display()))?;
    if let Ok(parent_dir) = OpenOptions::new().read(true).open(parent) {
        drop(parent_dir.sync_all());
    }

    Ok(ConfigPatchOutcome {
        config: cfg,
        backup_path,
    })
}

fn backup_config_file(path: &Path, raw: &str) -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let pid = std::process::id();
    for _ in 0..10 {
        let counter = CONFIG_BACKUP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let backup = path.with_extension(format!("toml.bak.{nanos}.{pid}.{counter}"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&backup) {
            Ok(mut file) => {
                file.write_all(raw.as_bytes())
                    .with_context(|| format!("write backup {}", backup.display()))?;
                file.sync_all()
                    .with_context(|| format!("sync backup {}", backup.display()))?;
                return Ok(backup);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(
                    anyhow::Error::new(e).context(format!("create backup {}", backup.display()))
                );
            }
        }
    }
    anyhow::bail!("failed to create unique backup for {}", path.display())
}

pub(crate) fn config_json_value_for_path(cfg: &LabConfig, path: &str) -> serde_json::Value {
    match path {
        "output.format" => serde_json::json!(cfg.output.format),
        "mcp.transport" => serde_json::json!(cfg.mcp.transport),
        "mcp.host" => serde_json::json!(cfg.mcp.host),
        "mcp.port" => serde_json::json!(cfg.mcp.port),
        "mcp.session_ttl_secs" => serde_json::json!(cfg.mcp.session_ttl_secs),
        "mcp.stateful" => serde_json::json!(cfg.mcp.stateful),
        "mcp.allowed_hosts" => serde_json::json!(cfg.mcp.allowed_hosts),
        "log.filter" => serde_json::json!(cfg.log.filter),
        "log.format" => serde_json::json!(cfg.log.format),
        "local_logs.retention_days" => {
            serde_json::json!(
                cfg.local_logs
                    .as_ref()
                    .and_then(|value| value.retention_days)
            )
        }
        "local_logs.max_bytes" => {
            serde_json::json!(cfg.local_logs.as_ref().and_then(|value| value.max_bytes))
        }
        "local_logs.queue_capacity" => {
            serde_json::json!(
                cfg.local_logs
                    .as_ref()
                    .and_then(|value| value.queue_capacity)
            )
        }
        "local_logs.subscriber_capacity" => {
            serde_json::json!(
                cfg.local_logs
                    .as_ref()
                    .and_then(|value| value.subscriber_capacity)
            )
        }
        "api.cors_origins" => serde_json::json!(cfg.api.cors_origins),
        "web.assets_dir" => {
            serde_json::json!(
                cfg.web
                    .assets_dir
                    .as_ref()
                    .map(|path| path.display().to_string())
            )
        }
        "workspace.root" => {
            serde_json::json!(
                cfg.workspace
                    .root
                    .as_ref()
                    .map(|path| path.display().to_string())
            )
        }
        "mcpregistry.url" => serde_json::json!(cfg.mcpregistry.url),
        "public_urls.app" => {
            serde_json::json!(cfg.public_urls.as_ref().and_then(|value| value.app.clone()))
        }
        "public_urls.mcp_gateway" => serde_json::json!(
            cfg.public_urls
                .as_ref()
                .and_then(|value| value.mcp_gateway.clone())
        ),
        "services.built_in_upstream_apis_enabled" => {
            serde_json::json!(cfg.services.built_in_upstream_apis_enabled)
        }
        "services.tailscale.tailnet" => serde_json::json!(cfg.services.tailscale.tailnet),
        "admin.enabled" => serde_json::json!(cfg.admin.enabled),
        "code_mode.trace_params" => serde_json::json!(cfg.code_mode.trace_params),
        "code_mode.timeout_ms" => serde_json::json!(cfg.code_mode.timeout_ms),
        "code_mode.max_response_bytes" => serde_json::json!(cfg.code_mode.max_response_bytes),
        "code_mode.max_response_tokens" => serde_json::json!(cfg.code_mode.max_response_tokens),
        "code_mode.token_estimate_divisor" => {
            serde_json::json!(cfg.code_mode.token_estimate_divisor)
        }
        "code_mode.max_log_entries" => serde_json::json!(cfg.code_mode.max_log_entries),
        "code_mode.max_log_bytes" => serde_json::json!(cfg.code_mode.max_log_bytes),
        "gateway_import_mode" => serde_json::json!(cfg.gateway_import_mode),
        "gateway.extra_stdio_commands" => serde_json::json!(cfg.gateway.extra_stdio_commands),
        "upstream_request_timeout_ms" => serde_json::json!(cfg.upstream_request_timeout_ms),
        "upstream_relay_timeout_ms" => serde_json::json!(cfg.upstream_relay_timeout_ms),
        "node.controller" => {
            serde_json::json!(cfg.node.as_ref().and_then(|value| value.controller.clone()))
        }
        "node.log_retention_days" => {
            serde_json::json!(cfg.node.as_ref().and_then(|value| value.log_retention_days))
        }
        "node.role" => serde_json::json!(cfg.node.as_ref().and_then(|value| value.role).map(
            |role| match role {
                NodeRuntimeRole::Controller => "controller",
                NodeRuntimeRole::Node => "node",
            }
        )),
        "device.master" => {
            serde_json::json!(cfg.device.as_ref().and_then(|value| value.master.clone()))
        }
        "web.disable_auth" => serde_json::json!(cfg.web.disable_auth),
        "auth" => serde_json::to_value(&cfg.auth).unwrap_or(serde_json::Value::Null),
        "code_mode.enabled" => serde_json::json!(cfg.code_mode.enabled),
        "gateway.disable_spawn_guard" => serde_json::json!(cfg.gateway.disable_spawn_guard),
        "oauth.machines" => {
            serde_json::to_value(&cfg.oauth.machines).unwrap_or(serde_json::Value::Null)
        }
        "deploy" => serde_json::to_value(&cfg.deploy).unwrap_or(serde_json::Value::Null),
        "upstream" => serde_json::to_value(&cfg.upstream).unwrap_or(serde_json::Value::Null),
        "upstream_pending" => {
            serde_json::to_value(&cfg.upstream_pending).unwrap_or(serde_json::Value::Null)
        }
        "upstream_import_tombstones" => {
            serde_json::to_value(&cfg.upstream_import_tombstones).unwrap_or(serde_json::Value::Null)
        }
        "protected_mcp_routes" => {
            serde_json::to_value(&cfg.protected_mcp_routes).unwrap_or(serde_json::Value::Null)
        }
        "virtual_servers" => {
            serde_json::to_value(&cfg.virtual_servers).unwrap_or(serde_json::Value::Null)
        }
        "quarantined_virtual_servers" => serde_json::to_value(&cfg.quarantined_virtual_servers)
            .unwrap_or(serde_json::Value::Null),
        _ => serde_json::Value::Null,
    }
}

/// Patch the non-secret built-in upstream API preference without rewriting
/// unrelated TOML content.
///
/// This intentionally edits only `[services].built_in_upstream_apis_enabled`.
/// It preserves comments, unknown keys, and plugin-owned sections that the
/// full typed `LabConfig` serializer cannot round-trip.
pub fn patch_built_in_upstream_apis_enabled(path: &Path, enabled: bool) -> Result<LabConfig> {
    Ok(patch_config_scalars(
        path,
        &[ConfigScalarPatch::new(
            "services.built_in_upstream_apis_enabled",
            ConfigScalarValue::Bool(enabled),
        )],
    )?
    .config)
}

fn config_lock_path(path: &Path) -> PathBuf {
    let mut lock = path.to_path_buf();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    lock.set_file_name(format!("{file_name}.lock"));
    lock
}

/// Load `.env` files into the process environment.
///
/// Called after `load_toml()` and tracing init. Env vars loaded here
/// override config.toml values at the point of use (each consumer checks
/// env first, then falls back to config).
pub fn load_dotenv() -> Result<()> {
    // Load ~/.labby/.env first (user-level secrets).
    if let Some(env_path) = dotenv_path()
        && env_path.exists()
    {
        dotenvy::from_path(&env_path)
            .with_context(|| format!("failed to load {}", env_path.display()))?;
    }

    // Also load .env from the current working directory (dev convenience).
    // Does not override vars already set by the user-level file.
    let cwd_env = Path::new(".env");
    if cwd_env.exists()
        && let Err(e) = dotenvy::from_path(cwd_env)
    {
        tracing::debug!(path = ".env", error = %e, "failed to load local .env (skipping)");
    }

    Ok(())
}

/// Load `.env` + `config.toml` in a single call (convenience for tests).
#[allow(dead_code)]
pub fn load() -> Result<LabConfig> {
    let cfg = load_toml(&toml_candidates())?;
    load_dotenv()?;
    Ok(cfg)
}

/// Resolve the Code Mode `openapi` provider config from the parsed `[openapi]`
/// TOML section plus `OPENAPI_<LABEL>_*` env vars.
///
/// Non-secret fields come from TOML; credentials (`OPENAPI_<LABEL>_TOKEN` /
/// `OPENAPI_<LABEL>_API_KEY`) come from `env`. `base_url` is mandatory; reserved
/// or duplicate labels, a missing/invalid base_url, an invalid spec_url, and an
/// ambiguous spec source are all hard config errors that fail boot.
///
/// `env` is injected (rather than read from the process environment directly) so
/// tests stay hermetic. In production callers pass a `std::env::var`-backed closure.
#[cfg(feature = "gateway")]
pub fn load_openapi_provider_config(
    section: &OpenApiTomlSection,
    env: &dyn Fn(&str) -> Option<String>,
) -> std::result::Result<labby_openapi::OpenApiProviderConfig, ConfigError> {
    use labby_openapi::{OpenApiCredential, OpenApiProviderConfig, OpenApiSpecConfig, SpecSource};

    let mut specs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for raw in &section.specs {
        let label = raw.label.trim().to_string();
        // The wire dispatch key is `openapi::<label>.<operationId>`, split on the
        // first `.` (operationIds may themselves contain `.`). A label containing
        // `.`, `:`, or whitespace would misroute that split, so restrict labels to
        // an unambiguous charset. Also keeps the `OPENAPI_<LABEL>_*` credential
        // env-var lookup well-formed.
        if label.is_empty()
            || !label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(ConfigError::InvalidLabel { label });
        }
        if labby_openapi::RESERVED_NAMESPACES.contains(&label.as_str()) {
            return Err(ConfigError::ReservedLabel { label });
        }
        if !seen.insert(label.clone()) {
            return Err(ConfigError::DuplicateLabel { label });
        }
        let base_url: url::Url = raw
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ConfigError::MissingBaseUrl {
                label: label.clone(),
            })?
            .parse()
            .map_err(|_| ConfigError::InvalidBaseUrl {
                label: label.clone(),
            })?;

        let upper = label.to_uppercase();
        let credential = env(&format!("OPENAPI_{upper}_TOKEN"))
            .filter(|t| !t.is_empty())
            .map(OpenApiCredential::BearerToken)
            .or_else(|| {
                env(&format!("OPENAPI_{upper}_API_KEY"))
                    .filter(|k| !k.is_empty())
                    .map(|value| OpenApiCredential::ApiKey {
                        header: raw
                            .api_key_header
                            .clone()
                            .filter(|h| !h.trim().is_empty())
                            .unwrap_or_else(|| "X-API-Key".into()),
                        value,
                    })
            });

        let spec_source = match (
            raw.spec_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            raw.spec_path
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        ) {
            (Some(u), None) => {
                SpecSource::Url(u.parse().map_err(|_| ConfigError::InvalidSpecUrl {
                    label: label.clone(),
                })?)
            }
            (None, Some(p)) => SpecSource::Path(p.into()),
            _ => {
                return Err(ConfigError::SpecSourceAmbiguous {
                    label: label.clone(),
                });
            }
        };

        specs.push(OpenApiSpecConfig {
            label,
            spec_source,
            base_url,
            allowed_operations: raw.allowed_operations.clone(),
            credential,
        });
    }
    Ok(OpenApiProviderConfig { specs })
}

/// Candidate paths for `config.toml`, ordered by priority (highest first).
pub fn toml_candidates() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("config.toml")];
    if let Some(home) = home_dir() {
        paths.push(home.join(".labby").join("config.toml"));
        paths.push(home.join(".config").join("labby").join("config.toml"));
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
        .unwrap_or_else(|| home.join(".labby").join("stash"))
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

/// Standard location for the `.env` file: `~/.labby/.env`.
pub fn dotenv_path() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".labby").join(".env"))
}

pub fn config_toml_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_CONFIG_TOML_PATH
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("test config path lock")
        .clone()
    {
        return Some(path);
    }

    toml_candidates()
        .into_iter()
        .find(|path| path.exists())
        .or_else(|| home_dir().map(|home| home.join(".config").join("labby").join("config.toml")))
}

/// Path to the SQLite registry database: `~/.labby/registry.db`.
///
/// Creates no files — callers are responsible for opening/creating the store.
pub fn registry_db_path() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".labby")
        .join("registry.db")
}

/// Path to the SQLite gateway usage-telemetry database: `~/.labby/usage.db`.
///
/// Creates no files — callers are responsible for opening/creating the store.
pub fn usage_db_path() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".labby")
        .join("usage.db")
}

/// Path to the SQLite Code Mode step-journal database:
/// `~/.labby/codemode_journal.db`.
///
/// Creates no files — callers are responsible for opening/creating the store.
pub fn codemode_journal_db_path() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".labby")
        .join("codemode_journal.db")
}

/// Whether the durable Code Mode step journal is enabled.
///
/// Set `LABBY_CODE_MODE_JOURNAL_DISABLED=1` to opt out — e.g. for a throwaway
/// dev instance where step-boundary journaling is unwanted.
pub fn codemode_journal_enabled() -> bool {
    std::env::var("LABBY_CODE_MODE_JOURNAL_DISABLED")
        .ok()
        .as_deref()
        != Some("1")
}

/// Whether gateway call-usage telemetry capture is enabled.
///
/// Set `LABBY_GATEWAY_USAGE_DISABLED=1` to opt out — e.g. for a throwaway
/// dev instance where nobody looks at the usage dashboard.
pub fn usage_telemetry_enabled() -> bool {
    resolve_usage_telemetry_enabled(
        std::env::var("LABBY_GATEWAY_USAGE_DISABLED")
            .ok()
            .as_deref(),
    )
}

/// Pure decision function behind `usage_telemetry_enabled()`, split out so
/// tests can exercise every branch without mutating process env (this crate
/// forbids `unsafe`, so `std::env::set_var`/`remove_var` — `unsafe fn` as of
/// Rust 2024 — are not available even in tests).
fn resolve_usage_telemetry_enabled(raw: Option<&str>) -> bool {
    raw != Some("1")
}

/// A string value that redacts itself in `Debug` and `Display` output.
///
/// Use for secret env values (`API_KEY`, `TOKEN`, `PASSWORD`) so they
/// never leak through `Debug`-printing config structs or tracing fields.
#[allow(dead_code)]
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    #[must_use]
    pub const fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("***REDACTED***")
    }
}

/// Value from an instance env var — either plain text or a secret.
///
/// Always constructed programmatically via [`scan_instances_from`]; never
/// deserialized from JSON. `Deserialize` is intentionally omitted — `Secret`
/// serializes as `"***REDACTED***"` (a plain string), so an `#[serde(untagged)]`
/// impl would silently pick `Plain` for every value, bypassing redaction.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
pub enum InstanceValue {
    Plain(String),
    Redacted(Secret),
}

impl InstanceValue {
    #[must_use]
    #[allow(dead_code)]
    pub fn expose(&self) -> &str {
        match self {
            Self::Plain(s) => s,
            Self::Redacted(s) => s.expose(),
        }
    }
}

/// Suffixes that carry secret values and must be wrapped in [`Secret`].
#[allow(dead_code)]
const SECRET_SUFFIXES: &[&str] = &["API_KEY", "TOKEN", "PASSWORD"];

/// Parse multi-instance env vars for a given service prefix.
///
/// Returns a map from instance label (`"default"` or `"<label>"`) to the
/// set of `(suffix, value)` pairs. Example: for prefix `UNRAID`, env vars
/// `UNRAID_URL`, `UNRAID_API_KEY`, `UNRAID_NODE2_URL`, `UNRAID_NODE2_API_KEY`
/// yield two entries keyed `"default"` and `"node2"`.
///
/// Suffixes are matched longest-first to avoid collisions when a label
/// contains a shorter suffix as a substring.
#[must_use]
#[allow(dead_code)]
pub fn scan_instances(prefix: &str) -> HashMap<String, HashMap<String, InstanceValue>> {
    scan_instances_from(prefix, std::env::vars())
}

/// Inner implementation testable without mutating process env.
fn scan_instances_from(
    prefix: &str,
    vars: impl Iterator<Item = (String, String)>,
) -> HashMap<String, HashMap<String, InstanceValue>> {
    let mut out: HashMap<String, HashMap<String, InstanceValue>> = HashMap::new();

    let mut known_suffixes = ["URL", "API_KEY", "TOKEN", "USERNAME", "PASSWORD"];
    known_suffixes.sort_by_key(|s| std::cmp::Reverse(s.len()));

    let prefix_under = format!("{prefix}_");

    for (key, value) in vars {
        let Some(rest) = key.strip_prefix(&prefix_under) else {
            continue;
        };

        for suffix in &known_suffixes {
            let wrap = |v: String| {
                if SECRET_SUFFIXES.contains(suffix) {
                    InstanceValue::Redacted(Secret::new(v))
                } else {
                    InstanceValue::Plain(v)
                }
            };

            if rest == *suffix {
                out.entry("default".to_string())
                    .or_default()
                    .insert((*suffix).to_string(), wrap(value.clone()));
                break;
            }
            if let Some(label) = rest.strip_suffix(&format!("_{suffix}"))
                && !label.is_empty()
            {
                out.entry(label.to_ascii_lowercase())
                    .or_default()
                    .insert((*suffix).to_string(), wrap(value.clone()));
                break;
            }
        }
    }

    out
}

// ─── .env writer ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvCredential {
    pub service: String,
    pub url: Option<String>,
    pub secret: Option<String>,
    pub env_field: String,
}

/// Merge `creds` into the `.env` file at `path` via the canonical
/// [`env_merge::merge`] primitive. The single sanctioned way to write
/// [`EnvCredential`]s: handles backup, atomic write, mtime-skew detection,
/// retention pruning, and 0600 perms in one call. [`write_env_pairs`] remains
/// for callers that already have flat `(key, value)` pairs instead of
/// [`EnvCredential`]s.
///
/// Returns the underlying merge outcome (skipped conflicts, backup path,
/// prune stats).
///
/// # Errors
/// Returns the typed [`env_merge::MergeError`] on any merge failure.
pub fn write_service_creds(
    path: &Path,
    creds: &[EnvCredential],
    force: bool,
) -> Result<env_merge::MergeOutcome, env_merge::MergeError> {
    let mut entries: Vec<env_merge::EnvEntry> = Vec::new();
    for cred in creds {
        let svc_upper = cred.service.to_uppercase();
        if let Some(url) = &cred.url {
            entries.push(env_merge::EnvEntry::new(
                format!("{svc_upper}_URL"),
                url.clone(),
            ));
        }
        if let Some(secret) = &cred.secret {
            entries.push(env_merge::EnvEntry::new(
                cred.env_field.clone(),
                secret.clone(),
            ));
        }
    }
    env_merge::merge(
        path,
        env_merge::MergeRequest {
            entries,
            force,
            expected_mtime: None,
        },
    )
}

/// Write raw `(key, value)` pairs into the `.env` file at `path`.
///
/// Atomic write, existing order preserved, conflicts skipped unless
/// `force=true`, idempotent on same values. Returns a `Vec<String>` of
/// conflict warnings.
///
/// Prefer [`write_service_creds`] when callers have [`EnvCredential`]s
/// instead of flat env pairs.
///
/// # Errors
/// Returns an error if the tmp file cannot be written or renamed.
pub fn write_env_pairs(
    path: &Path,
    pairs: &[(String, String)],
    force: bool,
) -> Result<Vec<String>> {
    let existing_raw = if path.exists() {
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
    } else {
        String::new()
    };
    let existing_lines: Vec<&str> = existing_raw.lines().collect();

    let mut existing: HashMap<String, String> = HashMap::new();
    for line in &existing_lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            existing.insert(k.trim().to_owned(), v.trim().to_owned());
        }
    }

    let mut conflicts: Vec<String> = Vec::new();
    let mut override_keys: HashMap<String, String> = HashMap::new();
    let mut new_keys: Vec<(String, String)> = Vec::new();

    for (key, value) in pairs {
        match existing.get(key) {
            None => new_keys.push((key.clone(), value.clone())),
            Some(existing_val) if existing_val == value => {}
            Some(existing_val) => {
                if force {
                    override_keys.insert(key.clone(), value.clone());
                } else {
                    conflicts.push(format!(
                        "CONFLICT: {key} already set to {existing_val:?}; skipping (use --force to overwrite)"
                    ));
                }
            }
        }
    }

    let mut out_lines: Vec<String> = Vec::new();
    for line in &existing_lines {
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && let Some((k, _)) = trimmed.split_once('=')
        {
            let key = k.trim();
            if let Some(new_val) = override_keys.get(key) {
                out_lines.push(format!("{}={}", key, quote_env_value(new_val)));
                continue;
            }
        }
        out_lines.push((*line).to_owned());
    }

    if !new_keys.is_empty() {
        if !out_lines.last().is_none_or(|l| l.trim().is_empty()) {
            out_lines.push(String::new());
        }
        for (key, value) in &new_keys {
            out_lines.push(format!("{}={}", key, quote_env_value(value)));
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }

    let tmp_path = PathBuf::from(format!("{}.tmp", path.display()));
    {
        let mut file = open_secret_file(&tmp_path)
            .with_context(|| format!("create {}", tmp_path.display()))?;
        for line in &out_lines {
            writeln!(file, "{line}").with_context(|| format!("write {}", tmp_path.display()))?;
        }
        file.sync_all()
            .with_context(|| format!("sync {}", tmp_path.display()))?;
    }
    std::fs::rename(&tmp_path, path)
        .with_context(|| format!("rename {} → {}", tmp_path.display(), path.display()))?;
    restrict_secret_file_permissions(path)
        .with_context(|| format!("chmod 0o600 {}", path.display()))?;

    Ok(conflicts)
}

// ── Secret file helpers (S2 / O-M4) ─────────────────────────────────────────

/// Create (or truncate) a file at `path` with mode 0o600 from the start.
///
/// On non-Unix platforms falls back to a regular `File::create` (homelab
/// is Linux-only; the chmod concern is moot on non-Unix).
fn open_secret_file(path: &Path) -> std::io::Result<std::fs::File> {
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
    #[cfg(not(unix))]
    {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
    }
}

/// Set `path` permissions to 0o600 (no-op on non-Unix).
fn restrict_secret_file_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Startup self-heal: chmod `path` (and any `.bak.*` siblings) to 0o600 if
/// they exist and are currently not restricted. Called once at gateway startup.
/// Silently ignores files that do not exist or are already correctly permissioned.
#[allow(dead_code)]
pub fn heal_env_file_permissions(path: &Path) {
    heal_one_file(path);

    let Some(parent) = path.parent() else { return };
    let Some(stem) = path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let prefix = format!("{stem}.bak.");
    if let Ok(rd) = std::fs::read_dir(parent) {
        for entry in rd.filter_map(|e| e.ok()) {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(&prefix))
            {
                heal_one_file(&entry.path());
            }
        }
    }
}

#[allow(dead_code)]
fn heal_one_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode != 0o600 {
                if let Err(e) =
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "failed to tighten permissions on secret file"
                    );
                } else {
                    tracing::info!(
                        path = %path.display(),
                        old_mode = format!("{mode:04o}"),
                        "tightened secret file to 0o600"
                    );
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Quote a value that contains shell-significant characters.
fn quote_env_value(v: &str) -> String {
    let needs_quotes = v
        .chars()
        .any(|c| matches!(c, ' ' | '\t' | '#' | '$' | '\\' | '"' | '\'' | '`'));
    if needs_quotes {
        let escaped = v.replace('\\', r"\\").replace('"', r#"\""#);
        format!("\"{escaped}\"")
    } else {
        v.to_owned()
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    /// `install_resolved_preferences` must pick up config.toml values when no
    /// overriding env var is set. This test does not touch process env, so
    /// it's safe under both nextest's per-process isolation and cargo test's
    /// threaded model, unlike a test that would need to mutate `std::env`.
    #[test]
    fn install_resolved_preferences_picks_up_config_toml_values() {
        let mut config = LabConfig::default();
        config.mcp.show_all = Some(true);
        config.api.dev_mode = Some(true);
        config.api.protected_mcp_connect_timeout_secs = Some(42);
        config.mcp.destructive_elicitation_timeout_ms = Some(45_000);
        config.mcp.catalog_notification_timeout_ms = Some(2_500);
        config.code_mode.widget_callbacks = Some(true);
        config.output.symbols = Some("ascii".to_string());

        install_resolved_preferences(&config);

        assert!(resolved_show_all(), "mcp.show_all should resolve true");
        assert!(resolved_dev_mode(), "api.dev_mode should resolve true");
        assert!(
            resolved_widget_callbacks_enabled(),
            "code_mode.widget_callbacks should resolve true"
        );
        assert_eq!(resolved_symbols().as_deref(), Some("ascii"));
        assert_eq!(resolved_protected_mcp_connect_timeout_secs(), Some(42));
        assert_eq!(
            resolved_destructive_elicitation_timeout(),
            Duration::from_millis(45_000)
        );
        assert_eq!(
            resolved_catalog_notification_timeout(),
            Duration::from_millis(2_500)
        );

        // Restore defaults so this test doesn't leak state into whichever
        // test the process/thread runs next (matches the existing
        // process_code_mode_enabled restore-after-test convention below).
        install_resolved_preferences(&LabConfig::default());
        assert!(!resolved_show_all());
        assert!(!resolved_dev_mode());
        assert!(!resolved_widget_callbacks_enabled());
        assert_eq!(resolved_symbols(), None);
        assert_eq!(resolved_protected_mcp_connect_timeout_secs(), None);
        assert_eq!(
            resolved_destructive_elicitation_timeout(),
            Duration::from_millis(DEFAULT_DESTRUCTIVE_ELICITATION_TIMEOUT_MS)
        );
        assert_eq!(
            resolved_catalog_notification_timeout(),
            Duration::from_millis(DEFAULT_CATALOG_NOTIFICATION_TIMEOUT_MS)
        );
    }

    fn parse_normalized_config(toml: &str) -> LabConfig {
        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        cfg.normalize_protected_mcp_routes().expect("normalize");
        cfg
    }

    #[cfg(feature = "gateway")]
    fn openapi_section(toml: &str) -> OpenApiTomlSection {
        toml::from_str::<LabConfig>(toml).expect("parse").openapi
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_reserved_label_rejected() {
        let toml = r#"[[openapi.specs]]
label = "git"
base_url = "https://api.example.com"
spec_url = "https://api.example.com/openapi.json"
allowed_operations = ["getUser"]"#;
        let err = load_openapi_provider_config(&openapi_section(toml), &|_| None).unwrap_err();
        assert!(matches!(err, ConfigError::ReservedLabel { ref label } if label == "git"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_dotted_label_rejected() {
        // A label containing `.` would misroute the `openapi::<label>.<operationId>`
        // dispatch split — reject it at config load.
        let toml = r#"[[openapi.specs]]
label = "ven.dor"
base_url = "https://api.example.com"
spec_url = "https://api.example.com/openapi.json"
allowed_operations = ["getUser"]"#;
        let err = load_openapi_provider_config(&openapi_section(toml), &|_| None).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidLabel { ref label } if label == "ven.dor"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_missing_base_url_rejected() {
        let toml = r#"[[openapi.specs]]
label = "vendor"
spec_url = "https://api.example.com/openapi.json"
allowed_operations = ["getUser"]"#;
        let err = load_openapi_provider_config(&openapi_section(toml), &|_| None).unwrap_err();
        assert!(matches!(err, ConfigError::MissingBaseUrl { .. }));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_duplicate_label_rejected() {
        let toml = r#"[[openapi.specs]]
label = "vendor"
base_url = "https://api.example.com"
spec_url = "https://api.example.com/openapi.json"

[[openapi.specs]]
label = "vendor"
base_url = "https://api2.example.com"
spec_url = "https://api2.example.com/openapi.json""#;
        let err = load_openapi_provider_config(&openapi_section(toml), &|_| None).unwrap_err();
        assert!(matches!(err, ConfigError::DuplicateLabel { ref label } if label == "vendor"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_ambiguous_spec_source_rejected() {
        let toml = r#"[[openapi.specs]]
label = "vendor"
base_url = "https://api.example.com"
spec_url = "https://api.example.com/openapi.json"
spec_path = "/tmp/openapi.json""#;
        let err = load_openapi_provider_config(&openapi_section(toml), &|_| None).unwrap_err();
        assert!(matches!(err, ConfigError::SpecSourceAmbiguous { .. }));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_credential_read_from_env_not_toml() {
        let toml = r#"[[openapi.specs]]
label = "vendor"
base_url = "https://api.example.com"
spec_url = "https://api.example.com/openapi.json"
allowed_operations = ["getUser"]"#;
        let env = |k: &str| (k == "OPENAPI_VENDOR_TOKEN").then(|| "tok-123".to_string());
        let cfg = load_openapi_provider_config(&openapi_section(toml), &env).unwrap();
        assert!(cfg.specs[0].credential.is_some());
        // Credential must NEVER round-trip through the TOML struct.
        assert!(!format!("{:?}", cfg.specs[0]).contains("tok-123"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn openapi_api_key_uses_configured_header() {
        let toml = r#"[[openapi.specs]]
label = "vendor"
base_url = "https://api.example.com"
spec_url = "https://api.example.com/openapi.json"
api_key_header = "X-Custom-Key""#;
        let env = |k: &str| (k == "OPENAPI_VENDOR_API_KEY").then(|| "sk-abc".to_string());
        let cfg = load_openapi_provider_config(&openapi_section(toml), &env).unwrap();
        match &cfg.specs[0].credential {
            Some(labby_openapi::OpenApiCredential::ApiKey { header, .. }) => {
                assert_eq!(header, "X-Custom-Key");
            }
            other => panic!("expected ApiKey credential, got {other:?}"),
        }
    }

    fn vars<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Iterator<Item = (String, String)> + 'a {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
    }

    #[test]
    fn service_preferences_default_enable_upstream_apis() {
        let cfg = toml::from_str::<LabConfig>("").expect("empty config should parse");
        assert!(cfg.services.built_in_upstream_apis_enabled);
    }

    #[test]
    fn service_preferences_can_disable_upstream_apis() {
        let cfg = toml::from_str::<LabConfig>(
            r"
            [services]
            built_in_upstream_apis_enabled = false
            ",
        )
        .expect("services config should parse");

        assert!(!cfg.services.built_in_upstream_apis_enabled);
    }

    #[test]
    fn patch_built_in_upstream_apis_preserves_comments_and_unknown_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"# operator note
[services]
# keep this comment
built_in_upstream_apis_enabled = true

[plugin_owned]
future = "keep"
"#,
        )
        .unwrap();

        let cfg = patch_built_in_upstream_apis_enabled(&path, false).unwrap();
        assert!(!cfg.services.built_in_upstream_apis_enabled);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# operator note"));
        assert!(raw.contains("# keep this comment"));
        assert!(raw.contains("[plugin_owned]"));
        assert!(raw.contains("future = \"keep\""));
        assert!(raw.contains("built_in_upstream_apis_enabled = false"));
    }

    #[test]
    fn patch_config_scalars_rejects_non_table_parent_without_mutating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "mcp = \"bad\"\n").unwrap();
        let err = patch_config_scalars(
            &path,
            &[ConfigScalarPatch::new(
                "mcp.port",
                ConfigScalarValue::I64(8765),
            )],
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("not a table"),
            "unexpected error: {err:#}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mcp = \"bad\"\n");
    }

    #[test]
    fn patch_config_scalars_updates_inline_table_parent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "services = { built_in_upstream_apis_enabled = true }\n",
        )
        .unwrap();
        let outcome = patch_config_scalars(
            &path,
            &[ConfigScalarPatch::new(
                "services.built_in_upstream_apis_enabled",
                ConfigScalarValue::Bool(false),
            )],
        )
        .unwrap();
        assert!(!outcome.config.services.built_in_upstream_apis_enabled);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("built_in_upstream_apis_enabled = false"));
    }

    #[test]
    fn patch_config_scalars_unsets_optional_instead_of_empty_string() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[mcp]\nport = 8765\n").unwrap();
        let outcome = patch_config_scalars(
            &path,
            &[ConfigScalarPatch::new(
                "mcp.port",
                ConfigScalarValue::UnsetOptional,
            )],
        )
        .unwrap();
        assert_eq!(outcome.config.mcp.port, None);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("port"));
    }

    #[test]
    fn patch_config_scalars_creates_backup_and_preserves_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# keep\n[mcp]\nhost = \"127.0.0.1\"\n").unwrap();
        let outcome = patch_config_scalars(
            &path,
            &[ConfigScalarPatch::new(
                "mcp.port",
                ConfigScalarValue::I64(8765),
            )],
        )
        .unwrap();
        let backup_path = outcome.backup_path.unwrap();
        assert!(backup_path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&backup_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# keep"));
        assert!(raw.contains("port = 8765"));
    }

    #[test]
    fn patch_config_scalars_skips_backup_and_write_for_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let raw = "[mcp]\nport = 8765\n";
        std::fs::write(&path, raw).unwrap();
        let outcome = patch_config_scalars(
            &path,
            &[ConfigScalarPatch::new(
                "mcp.port",
                ConfigScalarValue::I64(8765),
            )],
        )
        .unwrap();
        assert_eq!(outcome.backup_path, None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }

    #[test]
    fn patch_config_scalars_checked_rejects_stale_expected_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let raw = "[mcp]\nport = 8765\n";
        std::fs::write(&path, raw).unwrap();
        let err = patch_config_scalars_checked(
            &path,
            &[ConfigScalarPatch::new(
                "mcp.port",
                ConfigScalarValue::I64(8766),
            )],
            &[ExpectedConfigScalar::new(
                "mcp.port",
                serde_json::json!(9000),
            )],
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("changed since it was loaded"),
            "unexpected error: {err:#}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }

    #[test]
    fn resolve_auth_reads_ttls_from_config_toml_fields() {
        let cfg = AuthFileConfig {
            mode: Some("oauth".to_string()),
            public_url: Some("https://lab.example.com".to_string()),
            sqlite_path: None,
            key_path: None,
            bootstrap_secret: Some("bootstrap".to_string()),
            allowed_client_redirect_uris: Some(vec![
                "https://callback.example.com/callback/*".to_string(),
            ]),
            google_client_id: Some("client-id".to_string()),
            google_client_secret: Some("client-secret".to_string()),
            google_callback_path: Some("/auth/google/callback".to_string()),
            google_scopes: Some(vec!["openid".to_string(), "email".to_string()]),
            access_token_ttl_secs: Some(120),
            refresh_token_ttl_secs: Some(3600),
            auth_code_ttl_secs: Some(45),
            admin_email: Some("admin@example.com".to_string()),
            register_requests_per_minute: Some(5),
            authorize_requests_per_minute: Some(15),
            max_pending_oauth_states: Some(256),
        };

        let resolved = resolve_auth(Some(&cfg)).expect("auth config should resolve");
        assert_eq!(resolved.access_token_ttl.as_secs(), 120);
        assert_eq!(resolved.refresh_token_ttl.as_secs(), 3600);
        assert_eq!(resolved.auth_code_ttl.as_secs(), 45);
        assert_eq!(
            resolved.allowed_client_redirect_uris,
            vec!["https://callback.example.com/callback/*".to_string()]
        );
        assert_eq!(resolved.register_requests_per_minute, 5);
        assert_eq!(resolved.authorize_requests_per_minute, 15);
        assert_eq!(resolved.max_pending_oauth_states, 256);
    }

    #[test]
    fn resolve_auth_uses_curated_client_redirects_by_default() {
        let cfg = AuthFileConfig {
            mode: Some("oauth".to_string()),
            public_url: Some("https://lab.example.com".to_string()),
            google_client_id: Some("client-id".to_string()),
            google_client_secret: Some("client-secret".to_string()),
            admin_email: Some("admin@example.com".to_string()),
            ..AuthFileConfig::default()
        };

        let resolved = resolve_auth(Some(&cfg)).expect("auth config should resolve");

        assert_eq!(
            resolved.allowed_client_redirect_uris,
            vec![
                "https://chatgpt.com/connector/oauth/*".to_string(),
                "https://chatgpt.com/connector_platform_oauth_redirect".to_string(),
                "https://claude.ai/api/mcp/auth_callback".to_string(),
                "https://claude.com/api/mcp/auth_callback".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_auth_explicit_empty_redirects_disable_product_defaults() {
        let cfg = AuthFileConfig {
            mode: Some("oauth".to_string()),
            public_url: Some("https://lab.example.com".to_string()),
            google_client_id: Some("client-id".to_string()),
            google_client_secret: Some("client-secret".to_string()),
            admin_email: Some("admin@example.com".to_string()),
            allowed_client_redirect_uris: Some(Vec::new()),
            ..AuthFileConfig::default()
        };

        let resolved = resolve_auth(Some(&cfg)).expect("auth config should resolve");

        assert_eq!(resolved.allowed_client_redirect_uris, Vec::<String>::new());
    }

    #[test]
    fn resolve_auth_preserves_explicit_all_https_redirect_opt_in() {
        let cfg = AuthFileConfig {
            mode: Some("oauth".to_string()),
            public_url: Some("https://lab.example.com".to_string()),
            google_client_id: Some("client-id".to_string()),
            google_client_secret: Some("client-secret".to_string()),
            admin_email: Some("admin@example.com".to_string()),
            allowed_client_redirect_uris: Some(vec!["https://*".to_string()]),
            ..AuthFileConfig::default()
        };

        let resolved = resolve_auth(Some(&cfg)).expect("auth config should resolve");

        assert_eq!(
            resolved.allowed_client_redirect_uris,
            vec!["https://*".to_string()]
        );
    }

    #[test]
    fn oauth_machine_config_deserializes() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[oauth.machines.node-a]
target_url = "http://100.64.0.10:38935/callback/node-a"
description = "Node A Claude callback target"
default_port = 38935
"#,
        )
        .expect("oauth machine config should parse");

        assert_eq!(
            cfg.oauth.machines["node-a"].target_url,
            "http://100.64.0.10:38935/callback/node-a"
        );
        assert_eq!(
            cfg.oauth.machines["node-a"].description.as_deref(),
            Some("Node A Claude callback target")
        );
        assert_eq!(cfg.oauth.machines["node-a"].default_port, Some(38935));
    }

    #[test]
    fn oauth_machine_defaults_keep_partial_configs_valid() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[web]
assets_dir = "/tmp/labby"
"#,
        )
        .expect("config without oauth section should still parse");

        assert!(cfg.oauth.machines.is_empty());
        assert_eq!(cfg.web.assets_dir, Some(PathBuf::from("/tmp/labby")));
    }

    #[test]
    fn mcpregistry_url_defaults_to_official_registry() {
        let cfg = toml::from_str::<LabConfig>("").expect("empty config should parse");

        assert_eq!(
            cfg.mcpregistry.url.as_deref(),
            Some(DEFAULT_MCPREGISTRY_URL)
        );
    }

    #[test]
    fn quarantined_virtual_servers_round_trip_through_toml() {
        let raw = r#"
[[quarantined_virtual_servers]]
id = "stale-registry"
service = "mcpregistry"
enabled = true

[quarantined_virtual_servers.surfaces]
mcp = true
"#;
        let cfg = toml::from_str::<LabConfig>(raw).expect("quarantine config should parse");
        assert_eq!(cfg.quarantined_virtual_servers.len(), 1);
        assert_eq!(cfg.quarantined_virtual_servers[0].id, "stale-registry");
        assert_eq!(cfg.quarantined_virtual_servers[0].service, "mcpregistry");
        assert!(cfg.quarantined_virtual_servers[0].surfaces.mcp);

        let serialized = toml::to_string(&cfg).expect("config should serialize");
        let reparsed =
            toml::from_str::<LabConfig>(&serialized).expect("serialized config should parse");
        assert_eq!(reparsed.quarantined_virtual_servers.len(), 1);
        assert_eq!(reparsed.quarantined_virtual_servers[0].id, "stale-registry");
    }

    #[test]
    fn workspace_root_defaults_to_lab_stash_under_home() {
        let cfg = toml::from_str::<LabConfig>("").expect("empty config should parse");
        let home = Path::new("/tmp/lab-home");

        assert_eq!(
            workspace_root_for_home(&cfg, home),
            home.join(".labby").join("stash")
        );
    }

    #[test]
    fn workspace_root_reads_config_toml_value() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[workspace]
root = "/srv/lab-stash"
"#,
        )
        .expect("workspace config should parse");

        assert_eq!(
            workspace_root_for_home(&cfg, Path::new("/tmp/ignored")),
            PathBuf::from("/srv/lab-stash")
        );
    }

    #[test]
    fn web_ui_auth_disabled_env_prefers_canonical_alias() {
        let setting = resolve_web_ui_auth_disabled_values(Some("true"), Some("false"))
            .expect("env values should parse")
            .expect("setting should resolve");

        assert!(setting.disabled);
        assert_eq!(setting.source, WEB_UI_AUTH_DISABLED_ENV);
        assert!(!setting.legacy_alias);
    }

    #[test]
    fn web_ui_auth_disabled_env_accepts_legacy_alias() {
        let setting = resolve_web_ui_auth_disabled_values(None, Some("1"))
            .expect("env values should parse")
            .expect("setting should resolve");

        assert!(setting.disabled);
        assert_eq!(setting.source, WEB_UI_AUTH_DISABLED_LEGACY_ENV);
        assert!(setting.legacy_alias);
    }

    #[test]
    fn web_ui_auth_disabled_env_rejects_invalid_values() {
        let error = resolve_web_ui_auth_disabled_values(Some("sometimes"), None)
            .expect_err("invalid bool should fail");

        assert!(
            error
                .to_string()
                .contains("invalid LABBY_WEB_UI_AUTH_DISABLED value")
        );
    }

    #[test]
    fn secret_debug_redacts() {
        let s = Secret::new("hunter2".into());
        assert_eq!(format!("{s:?}"), "[REDACTED]");
        assert_eq!(format!("{s}"), "[REDACTED]");
        assert_eq!(s.expose(), "hunter2");
    }

    #[test]
    fn secret_serialize_emits_placeholder_not_plaintext() {
        let s = Secret::new("super-secret-api-key".into());
        let json = serde_json::to_string(&s).expect("serialize must not fail");
        assert_eq!(
            json, "\"***REDACTED***\"",
            "Secret must serialize to placeholder"
        );
        assert!(
            !json.contains("super-secret-api-key"),
            "Secret must never emit plaintext through serde"
        );
    }

    #[test]
    fn suffix_collision_longest_wins() {
        let env = [("S_NODE_API_KEY_URL", "http://example.com")];
        let result = scan_instances_from("S", vars(&env));
        let inst = result
            .get("node_api_key")
            .expect("should find instance node_api_key");
        assert_eq!(
            inst.get("URL").expect("should have URL").expose(),
            "http://example.com"
        );
    }

    #[test]
    fn default_instance_parsed() {
        let env = [
            ("SVC_URL", "http://localhost"),
            ("SVC_API_KEY", "secret123"),
        ];
        let result = scan_instances_from("SVC", vars(&env));
        let def = result.get("default").expect("should find default");
        assert_eq!(def.get("URL").expect("URL").expose(), "http://localhost");
        assert_eq!(def.get("API_KEY").expect("API_KEY").expose(), "secret123");
        assert!(format!("{:?}", def.get("API_KEY").unwrap()).contains("[REDACTED]"));
    }

    #[test]
    fn named_instance_parsed() {
        let env = [
            ("UNRAID_NODE2_URL", "http://node2"),
            ("UNRAID_NODE2_TOKEN", "tok"),
        ];
        let result = scan_instances_from("UNRAID", vars(&env));
        let inst = result.get("node2").expect("should find node2");
        assert_eq!(inst.get("URL").expect("URL").expose(), "http://node2");
        assert_eq!(inst.get("TOKEN").expect("TOKEN").expose(), "tok");
        assert!(format!("{:?}", inst.get("TOKEN").unwrap()).contains("[REDACTED]"));
    }

    #[test]
    fn unrelated_vars_ignored() {
        let env = [
            ("SVC_URL", "http://localhost"),
            ("OTHER_URL", "http://other"),
        ];
        let result = scan_instances_from("SVC", vars(&env));
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("default"));
    }

    #[test]
    fn username_is_plain_not_secret() {
        let env = [("SVC_USERNAME", "admin")];
        let result = scan_instances_from("SVC", vars(&env));
        let def = result.get("default").expect("should find default");
        assert!(!format!("{:?}", def.get("USERNAME").unwrap()).contains("[REDACTED]"));
    }

    // ─── write_service_creds tests ──────────────────────────────────────────

    fn radarr_cred() -> EnvCredential {
        EnvCredential {
            service: "radarr".to_owned(),
            url: Some("http://localhost:7878".to_owned()),
            secret: Some("abc123".to_owned()),
            env_field: "RADARR_API_KEY".to_owned(),
        }
    }

    #[test]
    fn write_service_creds_adds_new_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let outcome = write_service_creds(&path, &[radarr_cred()], false).unwrap();
        assert!(outcome.skipped.is_empty());
        assert_eq!(outcome.written, 2);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("RADARR_URL=http://localhost:7878"));
        assert!(content.contains("RADARR_API_KEY=abc123"));
    }

    #[test]
    fn write_service_creds_preserves_comments_and_blanks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "# my comment\nOTHER=val\n").unwrap();
        write_service_creds(&path, &[radarr_cred()], false).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# my comment"));
        assert!(content.contains("OTHER=val"));
    }

    #[test]
    fn write_service_creds_conflict_skip_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "RADARR_API_KEY=oldvalue\n").unwrap();
        let outcome = write_service_creds(&path, &[radarr_cred()], false).unwrap();
        assert!(!outcome.skipped.is_empty());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("oldvalue"));
        assert!(!content.contains("abc123"));
    }

    #[test]
    fn write_service_creds_conflict_overwrite_with_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "RADARR_API_KEY=oldvalue\n").unwrap();
        let outcome = write_service_creds(&path, &[radarr_cred()], true).unwrap();
        assert!(outcome.skipped.is_empty());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("abc123"));
        assert!(!content.contains("oldvalue"));
    }

    #[test]
    fn write_service_creds_is_idempotent_when_matching() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        write_service_creds(&path, &[radarr_cred()], false).unwrap();
        // Re-running with the exact same creds must be a written=0 no-op --
        // this is the signal crate::dispatch::gateway::config_store relies on
        // to skip a service-client refresh cycle.
        let outcome = write_service_creds(&path, &[radarr_cred()], false).unwrap();
        assert_eq!(outcome.written, 0);
        assert!(outcome.backup_path.is_none());
    }

    #[test]
    fn write_service_creds_quotes_value_with_special_chars() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let cred = EnvCredential {
            service: "svc".to_owned(),
            url: None,
            secret: Some("has space".to_owned()),
            env_field: "SVC_KEY".to_owned(),
        };
        write_service_creds(&path, &[cred], false).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("SVC_KEY=\"has space\""));
    }

    #[test]
    fn upstream_oauth_pkce_parses() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"

[upstream.oauth]
mode = "authorization_code_pkce"
scopes = ["mcp"]

[upstream.oauth.registration]
strategy = "client_metadata_document"
url = "https://acme.example.com/.well-known/oauth-client"
"#,
        )
        .expect("pkce config should parse");

        let upstream = &cfg.upstream[0];
        let oauth = upstream.oauth.as_ref().expect("oauth present");
        assert!(matches!(
            oauth.mode,
            UpstreamOauthMode::AuthorizationCodePkce
        ));
        assert_eq!(oauth.scopes.as_deref(), Some(&["mcp".to_string()][..]));
        match &oauth.registration {
            UpstreamOauthRegistration::ClientMetadataDocument { url } => {
                assert_eq!(url, "https://acme.example.com/.well-known/oauth-client");
            }
            other => panic!("unexpected registration: {other:?}"),
        }
        upstream.validate().expect("validate ok");
    }

    #[test]
    fn upstream_oauth_preregistered_parses() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"

[upstream.oauth]
mode = "authorization_code_pkce"

[upstream.oauth.registration]
strategy = "preregistered"
client_id = "my-client"
"#,
        )
        .expect("preregistered config should parse");

        let upstream = &cfg.upstream[0];
        let oauth = upstream.oauth.as_ref().unwrap();
        match &oauth.registration {
            UpstreamOauthRegistration::Preregistered {
                client_id,
                client_secret_env,
            } => {
                assert_eq!(client_id, "my-client");
                assert!(client_secret_env.is_none());
            }
            other => panic!("unexpected registration: {other:?}"),
        }
    }

    #[test]
    fn upstream_oauth_preregistered_with_secret_parses() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"

[upstream.oauth]
mode = "authorization_code_pkce"

[upstream.oauth.registration]
strategy = "preregistered"
client_id = "my-client"
client_secret_env = "ACME_CLIENT_SECRET"
"#,
        )
        .expect("preregistered+secret config should parse");

        let upstream = &cfg.upstream[0];
        let oauth = upstream.oauth.as_ref().unwrap();
        match &oauth.registration {
            UpstreamOauthRegistration::Preregistered {
                client_id,
                client_secret_env,
            } => {
                assert_eq!(client_id, "my-client");
                assert_eq!(client_secret_env.as_deref(), Some("ACME_CLIENT_SECRET"));
            }
            other => panic!("unexpected registration: {other:?}"),
        }
    }

    #[test]
    fn upstream_oauth_dynamic_parses() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"

[upstream.oauth]
mode = "authorization_code_pkce"

[upstream.oauth.registration]
strategy = "dynamic"
"#,
        )
        .expect("dynamic config should parse");

        let upstream = &cfg.upstream[0];
        let oauth = upstream.oauth.as_ref().unwrap();
        assert!(matches!(
            oauth.registration,
            UpstreamOauthRegistration::Dynamic
        ));
    }

    #[test]
    fn upstream_oauth_conflicts_with_bearer_token_env() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"
bearer_token_env = "ACME_TOKEN"

[upstream.oauth]
mode = "authorization_code_pkce"

[upstream.oauth.registration]
strategy = "dynamic"
"#,
        )
        .expect("config parses; validation is a separate step");

        let err = cfg.upstream[0].validate().unwrap_err();
        match err {
            ConfigError::ConflictingAuth { name } => assert_eq!(name, "acme"),
            other => panic!("expected ConflictingAuth, got {other:?}"),
        }
    }

    #[test]
    fn code_mode_is_root_level_config() {
        let cfg = toml::from_str::<LabConfig>(
            r#"
[code_mode]
enabled = true
timeout_ms = 2500

[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"
"#,
        )
        .expect("root code_mode parses");

        assert!(cfg.code_mode.enabled);
        assert_eq!(cfg.code_mode.timeout_ms, 2500);
        cfg.validate().expect("root code_mode validates");
    }

    #[test]
    fn code_mode_is_root_level_config_with_default_limits() {
        let default_cfg = LabConfig::default();
        assert_eq!(default_cfg.code_mode.timeout_ms, 30_000);
        assert_eq!(default_cfg.code_mode.max_response_bytes, 24 * 1024);
        assert_eq!(default_cfg.code_mode.max_response_tokens, 6000);

        let cfg = toml::from_str::<LabConfig>(
            r"
[code_mode]
timeout_ms = 2500
max_response_bytes = 12000
max_response_tokens = 3000
",
        )
        .expect("root code_mode parses");

        assert_eq!(cfg.code_mode.timeout_ms, 2500);
        assert_eq!(cfg.code_mode.max_response_bytes, 12000);
        assert_eq!(cfg.code_mode.max_response_tokens, 3000);
    }

    #[test]
    fn upstream_request_timeout_is_root_level_config() {
        let default_cfg = LabConfig::default();
        assert_eq!(
            default_cfg.upstream_request_timeout(),
            Duration::from_millis(30_000)
        );

        let cfg = toml::from_str::<LabConfig>(
            r"
upstream_request_timeout_ms = 60000
",
        )
        .expect("root upstream request timeout parses");

        assert_eq!(cfg.upstream_request_timeout_ms, Some(60_000));
        assert_eq!(
            cfg.upstream_request_timeout(),
            Duration::from_millis(60_000)
        );
        cfg.validate().expect("timeout validates");
    }

    #[test]
    fn upstream_relay_timeout_defaults_to_five_minutes_and_is_configurable() {
        // Unset → 5 minute default (NOT the 30s request-timeout default), so a
        // relayed elicitation is not aborted while a human is answering.
        let default_cfg = LabConfig::default();
        assert_eq!(default_cfg.upstream_relay_timeout_ms, None);
        assert_eq!(
            default_cfg.upstream_relay_timeout(),
            Duration::from_millis(300_000)
        );

        let cfg = toml::from_str::<LabConfig>(
            r"
upstream_relay_timeout_ms = 600000
",
        )
        .expect("root upstream relay timeout parses");
        assert_eq!(cfg.upstream_relay_timeout_ms, Some(600_000));
        assert_eq!(cfg.upstream_relay_timeout(), Duration::from_millis(600_000));
        cfg.validate().expect("relay timeout validates");
    }

    #[test]
    fn upstream_relay_timeout_rejects_out_of_range() {
        // Above the 30 min ceiling.
        let too_big = LabConfig {
            upstream_relay_timeout_ms: Some(1_800_001),
            ..LabConfig::default()
        };
        assert!(matches!(
            too_big.validate(),
            Err(ConfigError::InvalidUpstreamRelayTimeout { value: 1_800_001 })
        ));

        // Zero is rejected just like the request timeout.
        let zero = LabConfig {
            upstream_relay_timeout_ms: Some(0),
            ..LabConfig::default()
        };
        assert!(matches!(
            zero.validate(),
            Err(ConfigError::InvalidUpstreamRelayTimeout { value: 0 })
        ));
    }

    #[test]
    fn destructive_elicitation_timeout_defaults_to_two_minutes_and_is_configurable() {
        let default_cfg = LabConfig::default();
        assert_eq!(default_cfg.mcp.destructive_elicitation_timeout_ms, None);

        let cfg = toml::from_str::<LabConfig>(
            r"
[mcp]
destructive_elicitation_timeout_ms = 45000
",
        )
        .expect("mcp destructive elicitation timeout parses");

        assert_eq!(cfg.mcp.destructive_elicitation_timeout_ms, Some(45_000));
        cfg.validate().expect("destructive timeout validates");
        assert_eq!(
            resolve_destructive_elicitation_timeout_ms(
                None,
                cfg.mcp.destructive_elicitation_timeout_ms
            ),
            Some(45_000)
        );
        assert_eq!(
            resolve_destructive_elicitation_timeout_ms(
                Some("90000"),
                cfg.mcp.destructive_elicitation_timeout_ms,
            ),
            Some(90_000)
        );
        assert_eq!(
            resolve_destructive_elicitation_timeout_ms(
                Some("900000"),
                cfg.mcp.destructive_elicitation_timeout_ms,
            ),
            Some(45_000)
        );
    }

    #[test]
    fn destructive_elicitation_timeout_rejects_out_of_range() {
        let too_big = LabConfig {
            mcp: McpPreferences {
                destructive_elicitation_timeout_ms: Some(600_001),
                ..McpPreferences::default()
            },
            ..LabConfig::default()
        };
        assert!(matches!(
            too_big.validate(),
            Err(ConfigError::InvalidDestructiveElicitationTimeout { value: 600_001 })
        ));

        let zero = LabConfig {
            mcp: McpPreferences {
                destructive_elicitation_timeout_ms: Some(0),
                ..McpPreferences::default()
            },
            ..LabConfig::default()
        };
        assert!(matches!(
            zero.validate(),
            Err(ConfigError::InvalidDestructiveElicitationTimeout { value: 0 })
        ));
    }

    #[test]
    fn catalog_notification_timeout_defaults_to_five_seconds_and_is_configurable() {
        let default_cfg = LabConfig::default();
        assert_eq!(default_cfg.mcp.catalog_notification_timeout_ms, None);

        let cfg = toml::from_str::<LabConfig>(
            r"
[mcp]
catalog_notification_timeout_ms = 2500
",
        )
        .expect("mcp catalog notification timeout parses");

        assert_eq!(cfg.mcp.catalog_notification_timeout_ms, Some(2_500));
        cfg.validate()
            .expect("catalog notification timeout validates");
    }

    #[test]
    fn catalog_notification_timeout_rejects_out_of_range() {
        let too_big = LabConfig {
            mcp: McpPreferences {
                catalog_notification_timeout_ms: Some(60_001),
                ..McpPreferences::default()
            },
            ..LabConfig::default()
        };
        assert!(matches!(
            too_big.validate(),
            Err(ConfigError::InvalidCatalogNotificationTimeout { value: 60_001 })
        ));

        let zero = LabConfig {
            mcp: McpPreferences {
                catalog_notification_timeout_ms: Some(0),
                ..McpPreferences::default()
            },
            ..LabConfig::default()
        };
        assert!(matches!(
            zero.validate(),
            Err(ConfigError::InvalidCatalogNotificationTimeout { value: 0 })
        ));
    }

    #[test]
    fn upstream_relay_enabled_accepts_current_and_legacy_env_names() {
        assert!(!resolve_upstream_relay_enabled(None, None));
        assert!(resolve_upstream_relay_enabled(Some("1"), None));
        assert!(resolve_upstream_relay_enabled(Some("yes"), None));
        assert!(resolve_upstream_relay_enabled(None, Some("true")));
        assert!(resolve_upstream_relay_enabled(Some("0"), Some("TRUE")));
        assert!(!resolve_upstream_relay_enabled(Some("0"), Some("false")));
    }

    #[test]
    fn code_mode_validation_rejects_unbounded_execution_settings() {
        let cfg = toml::from_str::<LabConfig>(
            r"
[code_mode]
timeout_ms = 0
",
        )
        .expect("code_mode parses");
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::InvalidCodeModeTimeout { value: 0 })
        ));

        let cfg = toml::from_str::<LabConfig>(
            r"
[code_mode]
timeout_ms = 5000
max_response_bytes = 100
",
        )
        .expect("code_mode parses");
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::InvalidCodeModeMaxResponseBytes { value: 100 })
        ));

        let cfg = toml::from_str::<LabConfig>(
            r"
[code_mode]
timeout_ms = 5000
max_response_tokens = 100
",
        )
        .expect("code_mode parses");
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::InvalidCodeModeMaxResponseTokens { value: 100 })
        ));
    }

    #[test]
    fn protected_route_legacy_backend_path_folds_into_backend_url() {
        let mut cfg = toml::from_str::<LabConfig>(
            r#"
[[protected_mcp_routes]]
name = "tools"
enabled = true
public_host = "mcp.example.com"
public_path = "/tools"
backend_url = "http://10.0.0.12:3100"
backend_mcp_path = "/mcp"
"#,
        )
        .expect("protected route parses");

        cfg.normalize_protected_mcp_routes()
            .expect("protected route normalizes");

        assert_eq!(
            cfg.protected_mcp_routes[0].backend_url,
            "http://10.0.0.12:3100/mcp"
        );
        assert_eq!(cfg.protected_mcp_routes[0].backend_mcp_path, "/mcp");
    }

    #[test]
    fn protected_route_named_upstream_allows_empty_backend_url() {
        let mut cfg = toml::from_str::<LabConfig>(
            r#"
[[protected_mcp_routes]]
name = "telemetry"
enabled = true
public_host = "mcp.example.com"
public_path = "/telemetry"
upstream = " telemetry "
"#,
        )
        .expect("protected route parses");

        cfg.normalize_protected_mcp_routes()
            .expect("upstream route normalizes");

        assert_eq!(
            cfg.protected_mcp_routes[0].upstream.as_deref(),
            Some("telemetry")
        );
        assert_eq!(cfg.protected_mcp_routes[0].backend_url, "");
        assert_eq!(cfg.protected_mcp_routes[0].backend_mcp_path, "/mcp");
    }

    #[test]
    fn protected_route_gateway_subset_target_parses() {
        let toml = r#"
[[protected_mcp_routes]]
name = "media"
public_host = "mcp.example.com"
public_path = "/media"
scopes = ["mcp:media"]

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["sonarr", "radarr", " prowlarr "]
services = ["gateway"]
expose_code_mode = true
"#;

        let cfg = parse_normalized_config(toml);
        let route = &cfg.protected_mcp_routes[0];

        assert_eq!(route.name, "media");
        assert_eq!(route.backend_url, "");
        assert_eq!(route.upstream, None);
        assert!(route.is_gateway_subset());
        let target = route.gateway_subset_target().expect("gateway subset");
        assert_eq!(target.upstreams, vec!["sonarr", "radarr", "prowlarr"]);
        assert_eq!(target.services, vec!["gateway"]);
        assert!(target.expose_code_mode);
    }

    #[test]
    fn protected_route_legacy_backend_url_maps_to_proxy_target() {
        let toml = r#"
[[protected_mcp_routes]]
name = "telemetry"
public_host = "mcp.example.com"
public_path = "/telemetry"
backend_url = "http://10.0.0.2:3100/mcp"
"#;

        let cfg = parse_normalized_config(toml);
        let route = &cfg.protected_mcp_routes[0];

        assert!(matches!(
            route.effective_target(),
            ProtectedMcpRouteEffectiveTarget::BackendUrl { .. }
        ));
    }

    #[test]
    fn protected_route_rejects_target_with_legacy_backend() {
        let toml = r#"
[[protected_mcp_routes]]
name = "bad"
public_host = "mcp.example.com"
public_path = "/bad"
backend_url = "http://10.0.0.2:3100/mcp"

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["sonarr"]
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        let err = cfg
            .normalize_protected_mcp_routes()
            .expect_err("target and backend_url must conflict");
        assert!(err.to_string().contains(
            "protected MCP route target cannot be combined with upstream or backend_url"
        ));
    }

    #[test]
    fn protected_route_rejects_empty_gateway_subset_entries() {
        let toml = r#"
[[protected_mcp_routes]]
name = "bad"
public_host = "mcp.example.com"
public_path = "/bad"

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["sonarr", " "]
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        let err = cfg
            .normalize_protected_mcp_routes()
            .expect_err("empty upstream entry must fail");
        assert!(err.to_string().contains("target.upstreams"));
        assert!(
            err.to_string()
                .contains("gateway_subset target entries must not be empty")
        );
    }

    #[test]
    fn protected_route_rejects_duplicate_gateway_subset_public_paths() {
        let toml = r#"
[[protected_mcp_routes]]
name = "media-a"
public_host = "mcp-a.example.com"
public_path = "/media"

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["sonarr"]

[[protected_mcp_routes]]
name = "media-b"
public_host = "mcp-b.example.com"
public_path = "/media"

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["radarr"]
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        let err = cfg
            .normalize_protected_mcp_routes()
            .expect_err("duplicate gateway_subset public_path must fail");

        assert!(err.to_string().contains("public_path"));
        assert!(err.to_string().contains("gateway_subset"));
    }

    #[test]
    fn config_validation_rejects_reserved_protected_route_path() {
        let toml = r#"
[[protected_mcp_routes]]
name = "bad"
public_host = "mcp.example.com"
public_path = "/v1"
backend_url = "http://10.0.0.2:3100/mcp"
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        cfg.normalize_protected_mcp_routes()
            .expect("normalization should not hide validation failure");
        let err = cfg
            .validate()
            .expect_err("reserved protected route path must fail validation");

        assert!(err.to_string().contains("public_path"));
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn config_validation_rejects_empty_gateway_subset_target() {
        let toml = r#"
[[protected_mcp_routes]]
name = "empty"
public_host = "mcp.example.com"
public_path = "/empty"

[protected_mcp_routes.target]
kind = "gateway_subset"
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        cfg.normalize_protected_mcp_routes()
            .expect("normalization should not hide validation failure");
        let err = cfg
            .validate()
            .expect_err("empty gateway_subset target must fail validation");

        assert!(err.to_string().contains("gateway_subset target"));
    }

    #[test]
    fn config_validation_rejects_unknown_gateway_subset_targets() {
        let toml = r#"
[[upstream]]
name = "sonarr"
url = "https://sonarr.example.com/mcp"

[[protected_mcp_routes]]
name = "media"
public_host = "mcp.example.com"
public_path = "/media"

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["sonnar"]
services = ["gateway", "nope"]
"#;

        let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
        cfg.normalize_protected_mcp_routes()
            .expect("normalization should not hide validation failure");
        let err = cfg
            .validate()
            .expect_err("unknown gateway_subset targets must fail validation");

        assert!(
            err.to_string().contains("sonnar") || err.to_string().contains("nope"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parses_deploy_defaults_and_host_overrides() {
        let raw = r#"
[deploy.defaults]
remote_path = "/usr/local/bin/labby"
service = "labby"
service_scope = "system"
max_parallel = 4
canary_hosts = ["mini1"]

[deploy.hosts.mini2]
remote_path = "/opt/lab/bin/labby"
service = "lab-worker"
service_scope = "user"
"#;
        let parsed: LabConfig = toml::from_str(raw).unwrap();
        let d = parsed.deploy.expect("deploy present");
        let defaults = d.defaults.expect("defaults present");
        assert_eq!(
            defaults.remote_path.as_deref(),
            Some("/usr/local/bin/labby")
        );
        assert_eq!(defaults.service.as_deref(), Some("labby"));
        assert_eq!(defaults.service_scope, Some(ServiceScope::System));
        assert_eq!(defaults.max_parallel, Some(4));
        assert_eq!(defaults.canary_hosts, vec!["mini1".to_string()]);
        let mini2 = d.hosts.get("mini2").expect("mini2 override");
        assert_eq!(mini2.remote_path.as_deref(), Some("/opt/lab/bin/labby"));
        assert_eq!(mini2.service_scope, Some(ServiceScope::User));
    }

    #[test]
    fn deploy_config_absent_is_none_not_error() {
        let raw = "[output]\n";
        let parsed: LabConfig = toml::from_str(raw).unwrap();
        assert!(parsed.deploy.is_none());
    }

    #[test]
    fn deploy_max_parallel_defaults_to_one_for_safety_at_read_time() {
        let raw = "[deploy.defaults]\nremote_path = \"/usr/local/bin/labby\"\n";
        let parsed: LabConfig = toml::from_str(raw).unwrap();
        let d = parsed.deploy.unwrap().defaults.unwrap();
        // unset remains None; safe default applied at orchestrator entry
        assert!(d.max_parallel.is_none());
    }

    // ── Code Mode: CodeModeConfig defaults ───────────────────────────────────

    #[test]
    fn code_mode_config_token_estimate_divisor_defaults_to_4() {
        let config = CodeModeConfig::default();
        // PRESENCE: default divisor is exactly 4
        assert_eq!(
            config.token_estimate_divisor, 4,
            "token_estimate_divisor default must be 4"
        );
        // ABSENCE: it is not 0 or 1 (which would drastically change truncation)
        assert_ne!(config.token_estimate_divisor, 0);
        assert_ne!(config.token_estimate_divisor, 1);
    }

    #[test]
    fn code_mode_config_defaults_are_sane() {
        let config = CodeModeConfig::default();
        // PRESENCE: timeout and output limits are positive
        assert!(config.timeout_ms > 0);
        assert!(config.max_response_bytes > 0);
        assert!(config.max_response_tokens > 0);
        // ABSENCE: not wildly large (sanity bounds)
        assert!(config.timeout_ms <= 60_000);
    }

    // ── Process-wide atomic flags ─────────────────────────────────────────────

    #[test]
    fn process_code_mode_flag_round_trips() {
        let prev_ts = process_code_mode_enabled();

        set_process_code_mode_enabled(true);
        assert!(
            process_code_mode_enabled(),
            "code_mode must be true after set_process_code_mode_enabled(true)"
        );

        set_process_code_mode_enabled(false);
        assert!(
            !process_code_mode_enabled(),
            "code_mode must be false after set_process_code_mode_enabled(false)"
        );

        // Restore
        set_process_code_mode_enabled(prev_ts);
    }

    // ── T3: secret file permission tests (S2) ────────────────────────────────

    #[cfg(unix)]
    fn file_mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .unwrap_or_else(|e| panic!("metadata {}: {e}", path.display()))
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    #[cfg(unix)]
    fn write_service_creds_creates_file_with_mode_0o600() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");

        let creds = [EnvCredential {
            service: "myservice".to_string(),
            url: None,
            secret: Some("supersecret".to_string()),
            env_field: "MYSERVICE_TOKEN".to_string(),
        }];

        write_service_creds(&env_path, &creds, false).expect("write_service_creds");

        assert_eq!(
            file_mode(&env_path),
            0o600,
            ".env must be 0o600 after write_service_creds"
        );
    }

    // Backup-file 0o600 perms and retention pruning are covered directly by
    // env_merge's own unix_perms_set_to_0600 / backup_pruning_keeps_last_ten
    // tests -- write_service_creds delegates entirely to env_merge::merge for
    // that behavior and adds no file-handling logic of its own.

    #[test]
    #[cfg(unix)]
    fn heal_env_file_permissions_tightens_loose_env() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        std::fs::write(&env_path, "TOKEN=secret\n").expect("write");
        std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o644))
            .expect("chmod 644");

        heal_env_file_permissions(&env_path);

        assert_eq!(
            file_mode(&env_path),
            0o600,
            "heal must tighten .env to 0o600"
        );
    }

    #[test]
    #[cfg(unix)]
    fn heal_env_file_permissions_tightens_backup_files() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        let bak_path = dir.path().join(".env.bak.1234567890");

        std::fs::write(&env_path, "TOKEN=secret\n").expect("write env");
        std::fs::write(&bak_path, "TOKEN=oldsecret\n").expect("write bak");

        for p in [&env_path, &bak_path] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o644)).expect("chmod 644");
        }

        heal_env_file_permissions(&env_path);

        assert_eq!(file_mode(&env_path), 0o600, ".env must be healed");
        assert_eq!(file_mode(&bak_path), 0o600, ".env.bak.* must be healed");
    }

    // usage_telemetry_enabled() delegates to the pure resolve_usage_telemetry_enabled()
    // so these tests never need to mutate process env (this crate forbids
    // `unsafe`, and `std::env::set_var`/`remove_var` are `unsafe fn` as of
    // Rust 2024) — same shape as the resolve_web_ui_auth_disabled_values
    // tests above.
    #[test]
    fn usage_telemetry_enabled_defaults_true_when_unset() {
        assert!(
            resolve_usage_telemetry_enabled(None),
            "usage telemetry must default to enabled when the env var is unset"
        );
    }

    #[test]
    fn usage_telemetry_enabled_false_when_set_to_1() {
        assert!(
            !resolve_usage_telemetry_enabled(Some("1")),
            "usage telemetry must be disabled when the env var is \"1\""
        );
    }

    #[test]
    fn usage_telemetry_enabled_true_for_other_values() {
        assert!(
            resolve_usage_telemetry_enabled(Some("true")),
            "only the exact value \"1\" should disable usage telemetry"
        );
        assert!(
            resolve_usage_telemetry_enabled(Some("0")),
            "\"0\" is not the disable sentinel; telemetry stays enabled"
        );
    }

    #[test]
    fn usage_db_path_is_under_dot_labby_home_dir() {
        let path = usage_db_path();
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("usage.db"));
        assert_eq!(
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str()),
            Some(".labby")
        );
    }
}
