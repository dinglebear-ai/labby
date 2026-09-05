//! Host-owned [`GatewayConfigStore`] implementation for `lab`.
//!
//! `labby-gateway` owns the in-memory [`GatewayConfig`] and all runtime behavior,
//! but it must not own the host's full [`LabConfig`], the `config.toml` render
//! path (with its foreign-key-preservation invariant), or the `.env` credential
//! helpers — those are shared with non-gateway Labby code and stay here.
//!
//! [`LabConfigStore`] is injected into `GatewayManager` at construction. It
//! holds the live `Arc<RwLock<LabConfig>>`, writes the gateway-owned sections
//! back into it on `persist`, and renders the full `LabConfig` through the
//! verbatim `toml_edit` merge path (`write_gateway_config`) that preserves
//! foreign top-level keys byte-for-byte. Env writes go through the host's
//! canonical [`crate::config::env_merge`] backup-first / atomic merge
//! primitive (via [`crate::config::write_service_creds`]) and refresh any
//! cached service clients.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use labby_gateway::gateway::config_store::{GatewayConfigStore, StoreFuture};
use labby_runtime::gateway_config::{GatewayConfig, ResolvedPublicUrls};

use crate::config::{EnvCredential, LabConfig, home_dir};
use crate::dispatch::clients::SharedServiceClients;
use crate::dispatch::error::ToolError;

// `load_gateway_config` is consumed by the gateway API integration tests;
// `write_gateway_config` is used by `LabConfigStore::persist`. Allow the
// bin-target unused-import lint for the test-only re-export.
#[allow(unused_imports)]
pub use host_config::{load_gateway_config, write_gateway_config};

/// Host-owned [`GatewayConfigStore`] backed by the live [`LabConfig`].
pub struct LabConfigStore {
    /// Live config the manager's gateway sections are persisted back into.
    config: Arc<RwLock<LabConfig>>,
    /// Path to the owned `config.toml`.
    config_path: PathBuf,
    /// Cached service clients to refresh after a credential write.
    service_clients: Option<SharedServiceClients>,
}

impl LabConfigStore {
    /// Build a store over the live `config` and the owned `config_path`.
    #[must_use]
    pub fn new(config: Arc<RwLock<LabConfig>>, config_path: PathBuf) -> Self {
        Self {
            config,
            config_path,
            service_clients: None,
        }
    }

    /// Attach cached service clients to refresh after credential writes.
    #[must_use]
    pub fn with_service_clients(mut self, clients: SharedServiceClients) -> Self {
        self.service_clients = Some(clients);
        self
    }

    fn resolved_env_path(&self) -> PathBuf {
        home_dir()
            .map(|h| h.join(".labby").join(".env"))
            .unwrap_or_else(|| PathBuf::from(".env"))
    }

    /// Backup-first atomic write of `creds` via the canonical
    /// [`crate::config::env_merge`] merge primitive, then refresh cached
    /// clients if anything actually changed.
    async fn write_creds_and_refresh(&self, creds: Vec<EnvCredential>) -> Result<(), ToolError> {
        if creds.is_empty() {
            return Ok(());
        }
        let env_path = self.resolved_env_path();
        let env_path_for_write = env_path.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            crate::config::write_service_creds(&env_path_for_write, &creds, true)
        })
        .await
        .map_err(|e| ToolError::internal_message(format!("env write task failed: {e}")))?
        .map_err(|e| ToolError::internal_message(format!("failed to write env file: {e}")))?;

        // merge() is idempotent — `written == 0` means every requested key
        // already matched, so there is nothing for cached clients to pick up.
        if outcome.written == 0 {
            return Ok(());
        }

        if let Some(service_clients) = &self.service_clients {
            service_clients
                .refresh_from_env_path(&env_path)
                .await
                .map_err(|e| {
                    ToolError::internal_message(format!(
                        "failed to refresh service clients from {}: {e}",
                        env_path.display()
                    ))
                })?;
        }
        Ok(())
    }
}

impl GatewayConfigStore for LabConfigStore {
    fn public_urls(&self) -> ResolvedPublicUrls {
        // Read the live LabConfig synchronously. `public_urls` reads `auth`,
        // `public_urls`, and env vars the gateway does not model.
        match self.config.read() {
            Ok(guard) => guard.public_urls(),
            Err(poisoned) => poisoned.into_inner().public_urls(),
        }
    }

    fn set_process_code_mode_enabled(&self, enabled: bool) {
        crate::config::set_process_code_mode_enabled(enabled);
    }

    fn env_path(&self) -> PathBuf {
        self.resolved_env_path()
    }

    fn persist(&self, cfg: &GatewayConfig) -> Result<(), ToolError> {
        // Reload the latest full LabConfig under the write lock before applying
        // gateway-owned sections. Other Labby surfaces may have updated known
        // non-gateway config sections on disk after this store was constructed;
        // persisting gateway state must not overwrite those newer values with a
        // stale in-memory snapshot.
        let host_lock = crate::config::host_write::HostConfigLock::acquire(&self.config_path)
            .map_err(host_config::host_error)?;
        let mut guard = self
            .config
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut snapshot = host_config::load_locked(&host_lock)?;
        snapshot.apply_gateway_config(cfg);
        host_config::write_locked(&host_lock, &snapshot)?;

        *guard = snapshot;
        Ok(())
    }

    fn persist_gateway_bearer_token<'a>(
        &'a self,
        env_name: &'a str,
        token_value: &'a str,
    ) -> StoreFuture<'a, Result<(), ToolError>> {
        // The manager already validated the env name and normalized the header.
        Box::pin(async move {
            let creds = vec![EnvCredential {
                service: "gateway".to_string(),
                url: None,
                secret: Some(token_value.to_string()),
                env_field: env_name.to_string(),
            }];
            self.write_creds_and_refresh(creds).await
        })
    }

    fn persist_service_env<'a>(
        &'a self,
        service: &'a str,
        values: &'a BTreeMap<String, String>,
    ) -> StoreFuture<'a, Result<(), ToolError>> {
        Box::pin(async move {
            let creds = values_to_service_creds(service, values);
            self.write_creds_and_refresh(creds).await
        })
    }
}

/// Map a service's `{FIELD: value}` set to host [`EnvCredential`]s. A
/// `{SERVICE}_URL` field is treated as the service URL; everything else is a
/// secret credential.
fn values_to_service_creds(service: &str, values: &BTreeMap<String, String>) -> Vec<EnvCredential> {
    let url_field = format!("{}_URL", service.to_uppercase());
    values
        .iter()
        .map(|(field, value)| {
            let url = (field == &url_field).then(|| value.clone());
            let secret = if url.is_some() {
                None
            } else {
                Some(value.clone())
            };
            EnvCredential {
                service: service.to_string(),
                url,
                secret,
                env_field: field.clone(),
            }
        })
        .collect()
}

// Used by `#[cfg(test)]` unit tests only.
#[cfg(test)]
static NEXT_TEST_GATEWAY_CONFIG_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
fn isolated_test_config_path(path: PathBuf) -> PathBuf {
    if path != *"config.toml" {
        return path;
    }

    let id = NEXT_TEST_GATEWAY_CONFIG_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "labby-test-gateway-config-{}-{id}.toml",
        std::process::id()
    ))
}

/// [`LabConfigStore`] with the process-wide Code Mode side effect suppressed.
///
/// `set_process_code_mode_enabled` writes a process-global atomic. Test
/// managers reload config freely and concurrently, so letting them drive that
/// global made them clobber whatever a `process_code_mode_test_guard` holder
/// had just set — `cargo test` runs these in parallel, and the guard's mutex
/// only serializes tests that take it, not managers reloading config. The
/// gateway crate's own test stores already no-op this hook
/// (`labby_gateway::gateway::config_store`); this keeps the host's test manager
/// consistent with them. Every other method delegates to the real store, so
/// persistence and credential behavior stay under test.
#[cfg(test)]
struct ProcessCodeModeInertStore(LabConfigStore);

#[cfg(test)]
impl GatewayConfigStore for ProcessCodeModeInertStore {
    fn public_urls(&self) -> ResolvedPublicUrls {
        self.0.public_urls()
    }

    fn set_process_code_mode_enabled(&self, _enabled: bool) {}

    fn env_path(&self) -> PathBuf {
        self.0.env_path()
    }

    fn persist(&self, cfg: &GatewayConfig) -> Result<(), ToolError> {
        self.0.persist(cfg)
    }

    fn persist_gateway_bearer_token<'a>(
        &'a self,
        env_name: &'a str,
        token_value: &'a str,
    ) -> StoreFuture<'a, Result<(), ToolError>> {
        self.0.persist_gateway_bearer_token(env_name, token_value)
    }

    fn persist_service_env<'a>(
        &'a self,
        service: &'a str,
        values: &'a BTreeMap<String, String>,
    ) -> StoreFuture<'a, Result<(), ToolError>> {
        self.0.persist_service_env(service, values)
    }
}

#[cfg(test)]
pub(crate) fn test_gateway_manager(
    path: PathBuf,
    runtime: labby_gateway::gateway::manager::GatewayRuntimeHandle,
) -> labby_gateway::gateway::manager::GatewayManager {
    let path = isolated_test_config_path(path);
    let config = Arc::new(RwLock::new(LabConfig::default()));
    let store = Arc::new(ProcessCodeModeInertStore(LabConfigStore::new(
        config,
        path.clone(),
    )));
    labby_gateway::gateway::manager::GatewayManager::with_store(path, runtime, store)
}

/// Host-owned `config.toml` render path: serialize the full [`LabConfig`] and
/// merge it into the existing document so foreign top-level keys (sections
/// `LabConfig` does not model) survive byte-for-byte.
mod host_config {
    use std::path::Path;

    use crate::config::host_write::{HostConfigLock, HostWriteError};

    use crate::config::LabConfig;
    use crate::dispatch::error::ToolError;

    // Exactly the fields written by LabConfig::apply_gateway_config.
    const GATEWAY_KEYS: &[&str] = &[
        "config_version",
        "code_mode",
        "mcp_apps",
        "upstream_request_timeout_ms",
        "upstream_relay_timeout_ms",
        "upstream",
        "upstream_import_tombstones",
        "upstream_pending",
        "loadouts",
        "protected_mcp_routes",
        "virtual_servers",
        "quarantined_virtual_servers",
        "gateway",
    ];

    pub(super) fn host_error(error: HostWriteError) -> ToolError {
        ToolError::Sdk {
            sdk_kind: match error {
                HostWriteError::Busy => "configuration_busy",
                HostWriteError::Durability => "durability_uncertain",
                _ => "configuration_io_error",
            }
            .into(),
            message: error.to_string(),
        }
    }

    /// Load the gateway-relevant config from `path` as a full [`LabConfig`].
    ///
    /// Consumed by the gateway API integration tests (in the lib test target);
    /// allow dead_code so the bin-target build, which does not compile those
    /// tests, stays lint-clean.
    #[allow(dead_code)]
    pub fn load_gateway_config(path: &Path) -> Result<LabConfig, ToolError> {
        let lock = HostConfigLock::acquire(path).map_err(host_error)?;
        load_locked(&lock)
    }

    pub(super) fn load_locked(lock: &HostConfigLock) -> Result<LabConfig, ToolError> {
        let raw = lock.read_raw().map_err(host_error)?;
        let mut cfg: LabConfig =
            toml::from_str(&raw).map_err(|_| host_error(HostWriteError::InvalidDocument))?;
        cfg.normalize_protected_mcp_routes()
            .map_err(|_| host_error(HostWriteError::InvalidDocument))?;
        Ok(cfg)
    }

    /// Render `cfg` into the existing document (preserving foreign keys) and
    /// atomically replace the file at `path`.
    pub fn write_gateway_config(path: &Path, cfg: &LabConfig) -> Result<(), ToolError> {
        let lock = HostConfigLock::acquire(path).map_err(host_error)?;
        write_locked(&lock, cfg)
    }

    pub(super) fn write_locked(lock: &HostConfigLock, cfg: &LabConfig) -> Result<(), ToolError> {
        cfg.validate().map_err(|_| ToolError::Sdk {
            sdk_kind: "invalid_configuration".into(),
            message: "invalid gateway configuration".into(),
        })?;
        let serialized =
            toml::to_string(cfg).map_err(|_| host_error(HostWriteError::InvalidDocument))?;
        let desired: toml_edit::DocumentMut = serialized
            .parse()
            .map_err(|_| host_error(HostWriteError::InvalidDocument))?;
        let mut document = lock.read().map_err(host_error)?;
        for key in GATEWAY_KEYS {
            document.as_table_mut().remove(key);
            if let Some(item) = desired.get(key) {
                document[key] = item.clone();
            }
        }
        lock.write(&document.to_string()).map_err(host_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_relative_test_config_paths_are_isolated() {
        let first = isolated_test_config_path(PathBuf::from("config.toml"));
        let second = isolated_test_config_path(PathBuf::from("config.toml"));

        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(std::env::temp_dir().as_path()));
        assert_eq!(second.parent(), Some(std::env::temp_dir().as_path()));
    }

    #[test]
    fn explicit_test_config_paths_are_preserved() {
        let explicit = std::env::temp_dir()
            .join("labby-explicit-test-config")
            .join("config.toml");

        assert_eq!(isolated_test_config_path(explicit.clone()), explicit);
    }

    /// Trust invariant: persisting a gateway mutation through the host store must
    /// preserve a FOREIGN top-level section (one `LabConfig` does not model),
    /// including its operator comment and formatting, byte-for-byte.
    ///
    /// `[deploy]`/`[device]` are intentionally NOT covered here: they are in
    /// `KNOWN_LAB_CONFIG_KEYS` and are rewritten from the struct by design.
    #[test]
    fn persist_preserves_foreign_top_level_section_byte_for_byte() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");

        // A foreign section LabConfig does not model + a gateway section the
        // manager owns.
        let initial = "\
[experimental_external_tool]
# operator comment must survive
foo = 1

[gateway]

[[upstream]]
name = \"alpha\"
enabled = true
url = \"https://alpha.example.com/mcp\"
";
        std::fs::write(&path, initial).expect("write initial config");

        // Load the full LabConfig and seed the store with it.
        let loaded = load_gateway_config(&path).expect("load config");
        let store = LabConfigStore::new(Arc::new(RwLock::new(loaded.clone())), path.clone());

        // Mutate a gateway-owned upstream and persist through the host store.
        let mut gw = loaded.to_gateway_config();
        gw.upstream[0].enabled = false;
        store.persist(&gw).expect("persist gateway mutation");

        let rendered = std::fs::read_to_string(&path).expect("read persisted config");

        // The gateway mutation landed.
        let reloaded = load_gateway_config(&path).expect("reload config");
        assert!(
            !reloaded.upstream[0].enabled,
            "gateway upstream mutation must persist"
        );

        // The foreign section's comment + formatting survived byte-for-byte.
        assert!(
            rendered.contains("[experimental_external_tool]"),
            "foreign section header must survive, got:\n{rendered}"
        );
        assert!(
            rendered.contains("# operator comment must survive"),
            "foreign section operator comment must survive byte-for-byte, got:\n{rendered}"
        );
        assert!(
            rendered.contains("foo = 1"),
            "foreign section value must survive, got:\n{rendered}"
        );
    }

    #[test]
    fn persist_preserves_known_non_gateway_disk_changes_after_store_creation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let initial = "\
[api]
cors_origins = [\"https://old.example.com\"]

[gateway]

[[upstream]]
name = \"alpha\"
enabled = true
url = \"https://alpha.example.com/mcp\"
";
        std::fs::write(&path, initial).expect("write initial config");

        let loaded = load_gateway_config(&path).expect("load config");
        let store = LabConfigStore::new(Arc::new(RwLock::new(loaded.clone())), path.clone());

        let edited_on_disk = initial.replace("https://old.example.com", "https://new.example.com");
        std::fs::write(&path, edited_on_disk).expect("simulate separate config edit");

        let mut gw = loaded.to_gateway_config();
        gw.upstream[0].enabled = false;
        store.persist(&gw).expect("persist gateway mutation");

        let reloaded = load_gateway_config(&path).expect("reload config");
        assert!(
            !reloaded.upstream[0].enabled,
            "gateway upstream mutation must persist"
        );
        assert_eq!(
            reloaded.api.cors_origins,
            vec!["https://new.example.com"],
            "known non-gateway disk edits must survive gateway persistence"
        );
    }

    #[cfg(unix)]
    #[test]
    fn persist_restricts_host_config_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[gateway]\n").expect("write initial config");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("loosen config permissions");

        let loaded = load_gateway_config(&path).expect("load config");
        let store = LabConfigStore::new(Arc::new(RwLock::new(loaded.clone())), path.clone());

        store
            .persist(&loaded.to_gateway_config())
            .expect("persist gateway config");

        let mode = std::fs::metadata(&path)
            .expect("config metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
