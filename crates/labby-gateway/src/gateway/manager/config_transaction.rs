//! Serialized, rollback-capable gateway configuration transactions.

use std::sync::Arc;
use std::time::Instant;

use fd_lock::RwLock;
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::{
    GatewayConfig, GatewayLoadoutConfig, ProtectedMcpRouteConfig, ProtectedMcpRouteTarget,
};
use tokio::sync::oneshot;

use crate::gateway::config::load_gateway_config;
use crate::upstream::types::UpstreamRuntimeOwner;

use super::{ConfigMutationGuard, GatewayManager};

/// Project durable desired config onto what this running process can honestly
/// claim is live without a restart.
///
/// Protected gateway-subset routes are mounted by the host router at process
/// startup, and Loadouts referenced by those routes are part of that mounted
/// projection. Staged mutations intentionally update durable config only. An
/// unrelated hot-safe mutation must therefore never publish those staged fields
/// into `self.config` as a side effect.
///
/// Every other GatewayConfig field remains hot-publishable and comes from the
/// desired config. Protected routes are transactional as a collection once any
/// change crosses a gateway-subset boundary, which keeps staged renames from
/// half-publishing. Loadouts remain mergeable name-by-name: only values used by
/// enabled desired or runtime subset routes stay pinned to the runtime version.
pub(crate) fn runtime_config_for_desired(
    current: &GatewayConfig,
    desired: &GatewayConfig,
) -> GatewayConfig {
    let mut effective = desired.clone();
    effective.protected_mcp_routes = runtime_protected_routes(current, desired);
    effective.loadouts = runtime_loadouts(current, desired);
    effective
}

fn runtime_protected_routes(
    current: &GatewayConfig,
    desired: &GatewayConfig,
) -> Vec<ProtectedMcpRouteConfig> {
    // A staged gateway-subset change is one startup-router transaction. Freeze
    // the complete route collection until restart rather than trying to merge
    // entries by name: a rename makes the old runtime route and new desired
    // route look like unrelated remove/add rows, and a name-by-name merge would
    // otherwise hot-publish the new half of that staged rename.
    if protected_routes_have_restart_debt(current, desired) {
        return current.protected_mcp_routes.clone();
    }
    desired.protected_mcp_routes.clone()
}

pub(crate) fn protected_routes_have_restart_debt(
    current: &GatewayConfig,
    desired: &GatewayConfig,
) -> bool {
    for runtime_route in &current.protected_mcp_routes {
        match desired
            .protected_mcp_routes
            .iter()
            .find(|route| route.name == runtime_route.name)
        {
            Some(desired_route) => {
                if desired_route != runtime_route
                    && (desired_route.is_gateway_subset() || runtime_route.is_gateway_subset())
                {
                    return true;
                }
            }
            None if runtime_route.is_gateway_subset() => return true,
            None => {}
        }
    }

    desired.protected_mcp_routes.iter().any(|desired_route| {
        desired_route.is_gateway_subset()
            && !current
                .protected_mcp_routes
                .iter()
                .any(|route| route.name == desired_route.name)
    })
}

fn runtime_loadouts(current: &GatewayConfig, desired: &GatewayConfig) -> Vec<GatewayLoadoutConfig> {
    let mut effective = Vec::with_capacity(desired.loadouts.len().max(current.loadouts.len()));

    for desired_loadout in &desired.loadouts {
        let runtime_loadout = current
            .loadouts
            .iter()
            .find(|loadout| loadout.name == desired_loadout.name);
        let restart_bound = runtime_loadout != Some(desired_loadout)
            && (loadout_has_enabled_route(desired, &desired_loadout.name)
                || loadout_has_enabled_route(current, &desired_loadout.name));
        if restart_bound {
            if let Some(runtime_loadout) = runtime_loadout {
                effective.push(runtime_loadout.clone());
            }
        } else {
            effective.push(desired_loadout.clone());
        }
    }

    for runtime_loadout in &current.loadouts {
        if desired
            .loadouts
            .iter()
            .any(|loadout| loadout.name == runtime_loadout.name)
        {
            continue;
        }
        if loadout_has_enabled_route(desired, &runtime_loadout.name)
            || loadout_has_enabled_route(current, &runtime_loadout.name)
        {
            effective.push(runtime_loadout.clone());
        }
    }

    effective
}

fn loadout_has_enabled_route(cfg: &GatewayConfig, loadout: &str) -> bool {
    cfg.protected_mcp_routes.iter().any(|route| {
        if !route.enabled {
            return false;
        }
        let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = route.target.as_ref() else {
            return false;
        };
        target.loadout.as_deref() == Some(loadout)
    })
}

impl GatewayManager {
    /// Cancellation-safe direct persistence for mutations that do not require
    /// an upstream pool reload. The owned task retains both mutation leases
    /// through durable write and in-memory publication.
    pub(crate) async fn persist_config_owned(
        &self,
        mutation_guard: ConfigMutationGuard,
        cfg: GatewayConfig,
    ) -> Result<(), ToolError> {
        let manager = self.clone();
        tokio::spawn(async move {
            let _mutation_guard = mutation_guard;
            manager.persist_config(cfg).await
        })
        .await
        .map_err(|error| {
            ToolError::internal_message(format!("gateway config persist task failed: {error}"))
        })?
    }

    /// Cancellation-safe persistence for desired config that must not be
    /// published into the running process yet. The owned task retains both
    /// mutation leases until the atomic file write has completed, which keeps
    /// staged restart mutations serialized even if the request future drops.
    pub(crate) async fn persist_desired_config_owned(
        &self,
        mutation_guard: ConfigMutationGuard,
        cfg: GatewayConfig,
    ) -> Result<(), ToolError> {
        let manager = self.clone();
        tokio::spawn(async move {
            let _mutation_guard = mutation_guard;
            manager.write_config_file(&cfg).await
        })
        .await
        .map_err(|error| {
            ToolError::internal_message(format!(
                "gateway desired config persist task failed: {error}"
            ))
        })?
    }

    /// Serialize a full read-modify-persist-reconcile transaction both within
    /// this manager and against other Labby processes targeting the same file.
    pub(crate) async fn acquire_config_mutation(&self) -> Result<ConfigMutationGuard, ToolError> {
        let started = Instant::now();
        let local = Arc::clone(&self.config_mutation).lock_owned().await;
        let path = mutation_lock_path(&self.path);
        let (ready_tx, ready_rx) = oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        tokio::task::spawn_blocking(move || {
            let mut ready_tx = Some(ready_tx);
            let result = (|| {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| {
                        ToolError::internal_message(format!(
                            "failed to create gateway mutation lock directory {}: {error}",
                            parent.display()
                        ))
                    })?;
                }
                let file = open_config_mutation_lock(&path)?;
                let mut lock = RwLock::new(file);
                let _guard = lock.write().map_err(|error| {
                    ToolError::internal_message(format!(
                        "failed to acquire gateway mutation lock {}: {error}",
                        path.display()
                    ))
                })?;
                if ready_tx
                    .take()
                    .is_some_and(|sender| sender.send(Ok(())).is_ok())
                {
                    let _ = release_rx.recv();
                }
                Ok::<(), ToolError>(())
            })();
            if let (Err(error), Some(sender)) = (result, ready_tx.take()) {
                drop(sender.send(Err(error)));
            }
        });
        ready_rx.await.map_err(|_| {
            ToolError::internal_message("gateway mutation lock task ended before acquisition")
        })??;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.config.mutation_lock",
            event = "lock.acquired",
            elapsed_ms = started.elapsed().as_millis(),
            "gateway config mutation lock acquired"
        );
        Ok(ConfigMutationGuard {
            _local: local,
            release: Some(release_tx),
        })
    }

    /// Read the latest durable revision while the mutation guard is held.
    pub(crate) async fn load_config_for_mutation(&self) -> Result<GatewayConfig, ToolError> {
        let path = self.path.clone();
        let durable = tokio::task::spawn_blocking(move || {
            if !path.exists() {
                return Ok(None);
            }
            load_gateway_config(&path).map(Some)
        })
        .await
        .map_err(|error| {
            ToolError::internal_message(format!(
                "gateway config mutation read task failed: {error}"
            ))
        })??;
        Ok(match durable {
            Some(config) => config,
            None => self.config.read().await.clone(),
        })
    }

    /// Persist and reconcile a candidate as one observable commit. If
    /// reconciliation fails, restore and reconcile the prior durable revision
    /// before returning the original error.
    pub(super) async fn commit_config_and_reload(
        &self,
        mutation_guard: ConfigMutationGuard,
        previous: GatewayConfig,
        candidate: GatewayConfig,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<crate::gateway::types::GatewayCatalogDiff, ToolError> {
        let manager = self.clone();
        let origin = origin.map(str::to_owned);
        tokio::spawn(async move {
            // The owned guard deliberately lives inside the detached task. If
            // the request future is cancelled after persistence, this task
            // still finishes commit or rollback before another mutation can
            // acquire either the process-local or cross-process lease.
            let _mutation_guard = mutation_guard;
            manager
                .commit_config_and_reload_owned(previous, candidate, origin, owner)
                .await
        })
        .await
        .map_err(|error| {
            ToolError::internal_message(format!("gateway config transaction task failed: {error}"))
        })?
    }

    async fn commit_config_and_reload_owned(
        &self,
        previous: GatewayConfig,
        candidate: GatewayConfig,
        origin: Option<String>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<crate::gateway::types::GatewayCatalogDiff, ToolError> {
        let previous_revision = config_revision(&previous);
        let candidate_revision = config_revision(&candidate);
        self.backup_config_before_commit(&previous_revision).await?;
        self.write_config_file(&candidate).await?;
        match self
            .reload_with_origin_unlocked_transactional(origin.as_deref(), owner.clone())
            .await
        {
            Ok(diff) => {
                tracing::info!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "gateway.config.commit",
                    event = "commit.finish",
                    persisted_revision = candidate_revision,
                    live_revision = candidate_revision,
                    rollback = false,
                    "gateway config transaction committed"
                );
                Ok(diff)
            }
            Err(commit_error) => {
                tracing::warn!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "gateway.config.commit",
                    event = "rollback.start",
                    persisted_revision = candidate_revision,
                    live_revision = previous_revision,
                    kind = commit_error.kind(),
                    rollback_outcome = "pending",
                    "gateway config reconcile failed; rolling back"
                );
                if let Err(rollback_error) = self.write_config_file(&previous).await {
                    tracing::error!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "gateway.config.commit",
                        event = "rollback.error",
                        phase = "persist",
                        kind = rollback_error.kind(),
                        persisted_revision = candidate_revision,
                        live_revision = previous_revision,
                        rollback_outcome = "failed",
                        "gateway config rollback persist failed"
                    );
                    return Err(ToolError::internal_message(format!(
                        "gateway reconcile failed ({commit_error}); rollback persist failed ({rollback_error})"
                    )));
                }
                if let Err(rollback_error) = self
                    .reload_with_origin_unlocked_transactional(origin.as_deref(), owner)
                    .await
                {
                    tracing::error!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "gateway.config.commit",
                        event = "rollback.error",
                        phase = "reload",
                        kind = rollback_error.kind(),
                        persisted_revision = previous_revision,
                        live_revision = candidate_revision,
                        rollback_outcome = "failed",
                        "gateway config rollback reload failed"
                    );
                    return Err(ToolError::internal_message(format!(
                        "gateway reconcile failed ({commit_error}); rollback reload failed ({rollback_error})"
                    )));
                }
                tracing::warn!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "gateway.config.commit",
                    event = "rollback.finish",
                    persisted_revision = previous_revision,
                    live_revision = previous_revision,
                    rollback = true,
                    rollback_outcome = "restored",
                    "gateway config transaction rolled back"
                );
                Err(commit_error)
            }
        }
    }

    async fn backup_config_before_commit(&self, revision: &str) -> Result<(), ToolError> {
        let source = self.path.clone();
        let backup = config_backup_path(&source);
        let backup_for_log = backup.clone();
        tokio::task::spawn_blocking(move || write_config_backup(&source, &backup))
            .await
            .map_err(|error| {
                ToolError::internal_message(format!("gateway config backup task failed: {error}"))
            })??;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.config.commit",
            event = "backup.finish",
            persisted_revision = revision,
            backup_path = %backup_for_log.display(),
            "gateway config backup completed"
        );
        Ok(())
    }
}

fn write_config_backup(
    source: &std::path::Path,
    backup: &std::path::Path,
) -> Result<(), ToolError> {
    let contents = match std::fs::read(source) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ToolError::internal_message(format!(
                "failed to back up gateway config {} to {}: {error}",
                source.display(),
                backup.display()
            )));
        }
    };
    use std::io::Write as _;
    let parent = backup.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        ToolError::internal_message(format!(
            "failed to create gateway config backup in {}: {error}",
            parent.display()
        ))
    })?;
    crate::gateway::config::set_file_permissions_600(temporary.path()).map_err(|error| {
        ToolError::internal_message(format!(
            "failed to restrict gateway config backup {} before writing: {error}",
            backup.display()
        ))
    })?;
    temporary.write_all(&contents).map_err(|error| {
        ToolError::internal_message(format!(
            "failed to write gateway config backup {}: {error}",
            backup.display()
        ))
    })?;
    temporary.as_file().sync_all().map_err(|error| {
        ToolError::internal_message(format!(
            "failed to sync gateway config backup {}: {error}",
            backup.display()
        ))
    })?;
    temporary.persist(backup).map_err(|error| {
        ToolError::internal_message(format!(
            "failed to persist gateway config backup {}: {}",
            backup.display(),
            error.error
        ))
    })?;
    Ok(())
}

fn config_revision(config: &GatewayConfig) -> String {
    use sha2::{Digest as _, Sha256};

    let encoded = toml::to_string(config).unwrap_or_default();
    let digest = Sha256::digest(encoded.as_bytes());
    hex::encode(&digest[..8])
}

fn mutation_lock_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut lock_path = path.to_path_buf();
    let name = path
        .file_name()
        .map(|name| format!("{}.mutation.lock", name.to_string_lossy()))
        .unwrap_or_else(|| "config.toml.mutation.lock".to_string());
    lock_path.set_file_name(name);
    lock_path
}

fn open_config_mutation_lock(path: &std::path::Path) -> Result<std::fs::File, ToolError> {
    labby_auth::util::open_restricted_lock_file(path).map_err(|error| {
        ToolError::internal_message(format!(
            "failed to open restricted gateway mutation lock {}: {error}",
            path.display()
        ))
    })
}

fn config_backup_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut backup = path.to_path_buf();
    let name = path
        .file_name()
        .map(|name| format!("{}.bak", name.to_string_lossy()))
        .unwrap_or_else(|| "config.toml.bak".to_string());
    backup.set_file_name(name);
    backup
}

#[cfg(all(test, windows))]
mod windows_acl_tests {
    use super::*;

    #[test]
    fn backup_from_permissive_parent_has_private_acl_and_no_temp_residue() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("config.toml");
        let backup = config_backup_path(&source);
        std::fs::write(&source, "secret = 'value'\n").unwrap();
        let loosen = std::process::Command::new("icacls.exe")
            .args([dir.path().as_os_str(), "/grant", "*S-1-1-0:(OI)(CI)(F)"])
            .status()
            .unwrap();
        assert!(loosen.success());

        write_config_backup(&source, &backup).unwrap();
        crate::gateway::config::tests::assert_private_windows_acl(&backup);
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path() != source && entry.path() != backup)
            .count();
        assert_eq!(leftovers, 0, "backup temp file leaked");

        std::fs::remove_file(&backup).unwrap();
        std::fs::create_dir(&backup).unwrap();
        let before = std::fs::read_dir(dir.path()).unwrap().count();
        assert!(write_config_backup(&source, &backup).is_err());
        let after = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(after, before, "failed backup leaked a secret temp file");
    }
}

#[cfg(all(test, unix))]
mod mutation_lock_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn mutation_lock_is_restricted_before_use() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml.mutation.lock");
        let file = open_config_mutation_lock(&path).unwrap();
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }
}
