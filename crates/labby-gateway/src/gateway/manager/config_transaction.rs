//! Serialized, rollback-capable gateway configuration transactions.

use std::fs::OpenOptions;
use std::sync::Arc;
use std::time::Instant;

use fd_lock::RwLock;
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::GatewayConfig;
use tokio::sync::oneshot;

use crate::gateway::config::load_gateway_config;
use crate::upstream::types::UpstreamRuntimeOwner;

use super::{ConfigMutationGuard, GatewayManager};

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
                let file = OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&path)
                    .map_err(|error| {
                        ToolError::internal_message(format!(
                            "failed to open gateway mutation lock {}: {error}",
                            path.display()
                        ))
                    })?;
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
            .reload_with_origin_unlocked(origin.as_deref(), owner.clone())
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
                    .reload_with_origin_unlocked(origin.as_deref(), owner)
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
        tokio::task::spawn_blocking(move || match std::fs::read(&source) {
            Ok(contents) => {
                use std::io::Write as _;
                let parent = backup.parent().unwrap_or_else(|| std::path::Path::new("."));
                let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
                    ToolError::internal_message(format!(
                        "failed to create gateway config backup in {}: {error}",
                        parent.display()
                    ))
                })?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    temporary
                        .as_file()
                        .set_permissions(std::fs::Permissions::from_mode(0o600))
                        .map_err(|error| {
                            ToolError::internal_message(format!(
                                "failed to restrict gateway config backup {}: {error}",
                                backup.display()
                            ))
                        })?;
                }
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
                temporary.persist(&backup).map_err(|error| {
                    ToolError::internal_message(format!(
                        "failed to persist gateway config backup {}: {}",
                        backup.display(),
                        error.error
                    ))
                })?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(ToolError::internal_message(format!(
                "failed to back up gateway config {} to {}: {error}",
                source.display(),
                backup.display()
            ))),
        })
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

fn config_backup_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut backup = path.to_path_buf();
    let name = path
        .file_name()
        .map(|name| format!("{}.bak", name.to_string_lossy()))
        .unwrap_or_else(|| "config.toml.bak".to_string());
    backup.set_file_name(name);
    backup
}
