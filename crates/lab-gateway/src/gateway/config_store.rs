//! Host-owned persistence and environment seam for [`GatewayManager`].
//!
//! `lab-gateway` owns the gateway's in-memory [`GatewayConfig`] and all runtime
//! behavior, but it must NOT own the host's full `LabConfig`, the `config.toml`
//! render path (with its foreign-key preservation invariant), or the `.env`
//! credential file helpers — those are shared with non-gateway Labby code and
//! stay in the `lab` binary.
//!
//! The manager reaches those host concerns exclusively through this trait. The
//! host (`lab`) implements it over its live `Arc<RwLock<LabConfig>>` + the
//! existing `write_gateway_config`/`render_gateway_config` toml_edit logic,
//! reused verbatim. The manager mutates its in-memory `GatewayConfig` and then
//! calls [`GatewayConfigStore::persist`] to write it back through the host.
//!
//! **Consistency invariant.** The gateway-owned config sections (`upstream`,
//! `virtual_servers`, `code_mode`, …) are only ever mutated through
//! `GatewayManager`, which always persists through this store. The host's
//! `LabConfig` and the manager's `GatewayConfig` therefore stay in sync for
//! those sections; non-gateway sections and foreign top-level keys are never
//! touched by the manager.

use std::collections::BTreeMap;
use std::path::PathBuf;

use lab_runtime::error::ToolError;
use lab_runtime::gateway_config::{GatewayConfig, ResolvedPublicUrls};

/// Host-owned persistence + environment seam for the gateway manager.
///
/// Native `async fn in trait` (no `#[async_trait]`). Implemented by `lab` and
/// injected into the manager at construction.
pub trait GatewayConfigStore: Send + Sync {
    /// Resolve the canonical public URL pair (env over config over legacy
    /// `[auth].public_url`). Host-owned because it reads `LabConfig` sections
    /// (`auth`, `public_urls`) the gateway does not model.
    fn public_urls(&self) -> ResolvedPublicUrls;

    /// Apply a side effect when the process-wide Code Mode flag changes. The
    /// host owns the global atomic shared with non-gateway code.
    fn set_process_code_mode_enabled(&self, enabled: bool);

    /// The canonical `.env` path used for credential persistence. `None` means
    /// "use the host default" (`~/.lab/.env`); tests inject an override.
    fn env_path(&self) -> PathBuf;

    /// Persist the gateway-owned config sections back to `config.toml`.
    ///
    /// The host writes `cfg` into its live `LabConfig`, renders via the existing
    /// foreign-key-preserving toml_edit path, and atomically replaces the file.
    async fn persist(&self, cfg: &GatewayConfig) -> Result<(), ToolError>;

    /// Idempotently write the gateway HTTP bearer token to the `.env` file
    /// (backup-first) and refresh any cached service clients.
    async fn persist_gateway_bearer_token(
        &self,
        env_name: &str,
        token_value: &str,
    ) -> Result<(), ToolError>;

    /// Idempotently write a registered service's credential env vars and refresh
    /// cached service clients. `values` maps env field name → value.
    async fn persist_service_env(
        &self,
        service: &str,
        values: &BTreeMap<String, String>,
    ) -> Result<(), ToolError>;

    /// Read raw `KEY=value` pairs from the `.env` file (best-effort).
    fn read_env_values(&self, path: &std::path::Path) -> BTreeMap<String, String>;
}

/// Default filesystem-backed [`GatewayConfigStore`].
///
/// Persists a bare [`GatewayConfig`] to `config.toml` via the gateway crate's
/// own foreign-key-preserving render path (gateway sections only) and writes
/// credentials to a sibling `.env` file. This is the store used by tests and by
/// any standalone caller that does not need the host's full `LabConfig`-backed
/// preservation of non-gateway sections.
///
/// Production Labby injects its own store (which keeps `LabConfig` and the
/// verbatim host render path) through `GatewayManager::from_config`.
pub struct FsGatewayConfigStore {
    config_path: PathBuf,
    env_path: PathBuf,
}

impl FsGatewayConfigStore {
    /// Build a store for `config_path`, deriving the `.env` path as a sibling
    /// `.env` file (or `~/.lab/.env` when `config_path` has no parent).
    #[must_use]
    pub fn new(config_path: PathBuf) -> Self {
        let env_path = config_path
            .parent()
            .map(|p| p.join(".env"))
            .unwrap_or_else(|| PathBuf::from(".env"));
        Self {
            config_path,
            env_path,
        }
    }

    /// Override the `.env` path (used by tests writing beside a temp config).
    #[must_use]
    pub fn with_env_path(mut self, env_path: PathBuf) -> Self {
        self.env_path = env_path;
        self
    }

    fn write_env_pairs(&self, pairs: &[(String, String)]) -> Result<(), ToolError> {
        let mut existing: BTreeMap<String, String> = self.read_env_values(&self.env_path);
        for (k, v) in pairs {
            existing.insert(k.clone(), v.clone());
        }
        if let Some(parent) = self.env_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ToolError::internal_message(format!("failed to create env dir: {e}"))
            })?;
        }
        let body: String = existing
            .iter()
            .map(|(k, v)| format!("{k}={v}\n"))
            .collect();
        std::fs::write(&self.env_path, body)
            .map_err(|e| ToolError::internal_message(format!("failed to write env file: {e}")))
    }
}

impl GatewayConfigStore for FsGatewayConfigStore {
    fn public_urls(&self) -> ResolvedPublicUrls {
        ResolvedPublicUrls::default()
    }

    fn set_process_code_mode_enabled(&self, _enabled: bool) {}

    fn env_path(&self) -> PathBuf {
        self.env_path.clone()
    }

    async fn persist(&self, cfg: &GatewayConfig) -> Result<(), ToolError> {
        let path = self.config_path.clone();
        let cfg = cfg.clone();
        tokio::task::spawn_blocking(move || super::config::write_gateway_config(&path, &cfg))
            .await
            .map_err(|e| ToolError::internal_message(format!("config write task failed: {e}")))?
    }

    async fn persist_gateway_bearer_token(
        &self,
        env_name: &str,
        token_value: &str,
    ) -> Result<(), ToolError> {
        let trimmed = token_value.trim();
        let header = if trimmed
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("bearer "))
        {
            format!("Bearer {}", &trimmed[7..])
        } else {
            format!("Bearer {trimmed}")
        };
        self.write_env_pairs(&[(env_name.to_string(), header)])
    }

    async fn persist_service_env(
        &self,
        _service: &str,
        values: &BTreeMap<String, String>,
    ) -> Result<(), ToolError> {
        let pairs: Vec<(String, String)> = values
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.write_env_pairs(&pairs)
    }

    fn read_env_values(&self, path: &std::path::Path) -> BTreeMap<String, String> {
        dotenvy::from_path_iter(path)
            .ok()
            .map(|iter| iter.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }
}
