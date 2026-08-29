//! Pool bootstrap and reload: swap-and-drain reconciliation, catalog snapshot
//! diffing, and quarantine of virtual servers with unregistered services.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::StreamExt as _;

use tokio::time::Instant;

use crate::gateway::config::load_gateway_config;
use crate::gateway::protected_routes::ProtectedRouteIndex;
use crate::gateway::runtime::runtime_origin_tag;
use crate::gateway::service_registry::GatewayServiceRegistry;
use crate::gateway::types::GatewayCatalogDiff;
use crate::upstream::pool::UpstreamPool;
use crate::upstream::types::UpstreamRuntimeOwner;
use labby_runtime::catalog_notify::{SOURCE_GATEWAY_RELOAD_FULL, SOURCE_GATEWAY_RELOAD_SELECTIVE};
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::GatewayConfig;

use super::GatewayManager;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatewayCatalogSnapshot {
    pub tools: BTreeSet<String>,
    pub resources: BTreeSet<String>,
    pub prompts: BTreeSet<String>,
}

pub fn diff_catalogs(
    before: &GatewayCatalogSnapshot,
    after: &GatewayCatalogSnapshot,
) -> GatewayCatalogDiff {
    GatewayCatalogDiff {
        tools_changed: before.tools != after.tools,
        resources_changed: before.resources != after.resources,
        prompts_changed: before.prompts != after.prompts,
    }
}

/// Process-lifetime count of reconciles where the raw upstream tool set moved
/// but the Code-Mode-visible contract did not — i.e. churn that would have
/// broadcast a spurious `tools/list_changed` before the visible-contract
/// projection landed. A steadily climbing value is the normal, healthy signal
/// that raw upstreams are flapping and clients are being shielded from it.
static SUPPRESSED_RAW_CHURN_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Cap on how many tool names a single delta log line will render. Reconciles
/// after a cold start legitimately add hundreds of tools; the point of the
/// field is diagnosing repeated small deltas, not dumping the catalog.
const MAX_LOGGED_DELTA_NAMES: usize = 20;

/// Prefix marking a synthetic namespace-description determinant inside a Code
/// Mode-visible snapshot. The control character cannot occur in a valid
/// upstream or tool name, keeping these entries disjoint from real tool names.
const NS_TOKEN_PREFIX: &str = "\u{1}ns\u{1}";

/// Stable synthetic tool advertised whenever the gateway is in Code Mode.
/// Keeping it in the projected snapshot makes raw↔Code Mode transitions visible
/// even when the gateway has no connected upstreams.
const CODE_MODE_SYNTHETIC_TOOL_NAME: &str = "codemode";
const CODE_MODE_READ_TOOL_NAME: &str = "codemode_read";
const CODE_MODE_UI_TOOL_NAME: &str = "codemode_ui";
const CODE_MODE_APP_CONTROL_TOOL_NAME: &str = "mcp_app";

/// The rendered delta between two catalog snapshots.
#[derive(Debug, Default, PartialEq, Eq)]
struct CatalogToolDelta {
    added: Vec<String>,
    removed: Vec<String>,
    namespaces_added: Vec<String>,
    namespaces_removed: Vec<String>,
    truncated: usize,
}

impl CatalogToolDelta {
    fn describe(before: &BTreeSet<String>, after: &BTreeSet<String>) -> Self {
        let mut delta = Self::default();
        for (source, tools, namespaces) in [
            (
                after.difference(before),
                &mut delta.added,
                &mut delta.namespaces_added,
            ),
            (
                before.difference(after),
                &mut delta.removed,
                &mut delta.namespaces_removed,
            ),
        ] {
            for entry in source {
                if let Some(rest) = entry.strip_prefix(NS_TOKEN_PREFIX) {
                    let name = rest.split('\u{1}').next().unwrap_or(rest);
                    namespaces.push(name.to_string());
                } else {
                    tools.push(entry.clone());
                }
            }
        }
        delta.truncate_names();
        delta
    }

    fn truncate_names(&mut self) {
        for names in [
            &mut self.added,
            &mut self.removed,
            &mut self.namespaces_added,
            &mut self.namespaces_removed,
        ] {
            if names.len() > MAX_LOGGED_DELTA_NAMES {
                self.truncated += names.len() - MAX_LOGGED_DELTA_NAMES;
                names.truncate(MAX_LOGGED_DELTA_NAMES);
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct VirtualServerMigration {
    quarantined: Vec<String>,
}

impl VirtualServerMigration {
    pub(super) fn changed(&self) -> bool {
        !self.quarantined.is_empty()
    }
}

/// Result of a detached reload: `completed: false` means the reconcile is
/// still running in its owned task and will apply on its own; the flattened
/// catalog diff is present only when the reload finished within the wait
/// budget.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GatewayReloadOutcome {
    pub completed: bool,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub diff: Option<GatewayCatalogDiff>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl GatewayManager {
    pub async fn reload_with_origin(
        &self,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayCatalogDiff, ToolError> {
        let _mutation_guard = self.acquire_config_mutation().await?;
        self.reload_with_origin_unlocked(origin, owner).await
    }

    /// Compare a requested upstream revision and reload while holding the same
    /// mutation lease. The durable config is deliberately re-read after lock
    /// acquisition so a queued reload cannot validate against stale state.
    pub async fn reload_checked(
        &self,
        name: Option<&str>,
        expected_revision: Option<&str>,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayCatalogDiff, ToolError> {
        let _mutation_guard = self.acquire_config_mutation().await?;
        if let (Some(name), Some(expected)) = (name, expected_revision) {
            let current = self.load_config_for_mutation().await?;
            super::config_ops::ensure_upstream_revision(&current, name, Some(expected))?;
        }
        self.reload_with_origin_unlocked(origin, owner).await
    }

    /// Reload in an owned task so the reconcile survives caller cancellation.
    ///
    /// Dispatch surfaces run inside request futures that the HTTP stack is
    /// free to drop (the API router's 30s `TimeoutLayer`, client disconnects).
    /// A full pool rebuild probes every upstream and routinely outlives those
    /// deadlines, so driving `reload_with_origin` directly from the request
    /// future silently discards the pending config at the timeout. Spawning
    /// decouples the reconcile from the request lifetime; the bounded wait
    /// keeps the common fast path returning a real diff.
    pub async fn reload_with_origin_detached(
        &self,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
        wait: std::time::Duration,
    ) -> Result<GatewayReloadOutcome, ToolError> {
        let manager = self.clone();
        let origin = origin.map(str::to_owned);
        let mut task =
            tokio::spawn(async move { manager.reload_with_origin(origin.as_deref(), owner).await });
        match tokio::time::timeout(wait, &mut task).await {
            Ok(Ok(result)) => result.map(|diff| GatewayReloadOutcome {
                completed: true,
                diff: Some(diff),
                note: None,
            }),
            Ok(Err(join_error)) => Err(ToolError::internal_message(format!(
                "gateway reload task failed: {join_error}"
            ))),
            Err(_elapsed) => {
                tokio::spawn(async move {
                    match task.await {
                        Ok(Ok(diff)) => tracing::info!(
                            surface = "dispatch",
                            service = "gateway",
                            action = "gateway.reload",
                            event = "background.finish",
                            tools_changed = diff.tools_changed,
                            resources_changed = diff.resources_changed,
                            prompts_changed = diff.prompts_changed,
                            "backgrounded gateway reload finished"
                        ),
                        Ok(Err(error)) => tracing::warn!(
                            surface = "dispatch",
                            service = "gateway",
                            action = "gateway.reload",
                            event = "background.error",
                            error = %error,
                            "backgrounded gateway reload failed"
                        ),
                        Err(join_error) => tracing::warn!(
                            surface = "dispatch",
                            service = "gateway",
                            action = "gateway.reload",
                            event = "background.panic",
                            error = %join_error,
                            "backgrounded gateway reload task failed"
                        ),
                    }
                });
                Ok(GatewayReloadOutcome {
                    completed: false,
                    diff: None,
                    note: Some(
                        "reload is still reconciling upstreams in the background; \
                         check `gateway list` for the applied state"
                            .to_string(),
                    ),
                })
            }
        }
    }

    pub async fn reload_checked_detached(
        &self,
        name: Option<&str>,
        expected_revision: Option<&str>,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
        wait: std::time::Duration,
    ) -> Result<GatewayReloadOutcome, ToolError> {
        let manager = self.clone();
        let name = name.map(str::to_owned);
        let expected_revision = expected_revision.map(str::to_owned);
        let origin = origin.map(str::to_owned);
        let task = tokio::spawn(async move {
            manager
                .reload_checked(
                    name.as_deref(),
                    expected_revision.as_deref(),
                    origin.as_deref(),
                    owner,
                )
                .await
        });
        match tokio::time::timeout(wait, task).await {
            Ok(Ok(result)) => result.map(|diff| GatewayReloadOutcome {
                completed: true,
                diff: Some(diff),
                note: None,
            }),
            Ok(Err(join_error)) => Err(ToolError::internal_message(format!(
                "gateway reload task failed: {join_error}"
            ))),
            Err(_) => Ok(GatewayReloadOutcome {
                completed: false,
                diff: None,
                note: Some("reload is still reconciling upstreams in the background".into()),
            }),
        }
    }

    pub(super) async fn reload_with_origin_unlocked(
        &self,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayCatalogDiff, ToolError> {
        self.reload_with_origin_unlocked_mode(origin, owner, false)
            .await
    }

    /// Transaction-owned reload path. Config transactions already execute in
    /// an owned task, so per-upstream changes may reconcile the published pool
    /// in place without inheriting caller-cancellation risk. Direct/manual
    /// reloads keep the private replacement-pool path below.
    pub(super) async fn reload_with_origin_unlocked_transactional(
        &self,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayCatalogDiff, ToolError> {
        self.reload_with_origin_unlocked_mode(origin, owner, true)
            .await
    }

    async fn reload_with_origin_unlocked_mode(
        &self,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
        allow_in_place_selective: bool,
    ) -> Result<GatewayCatalogDiff, ToolError> {
        let started = Instant::now();
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "catalog.refresh.start",
            phase = "config.load.start",
            "gateway reconcile"
        );
        let path = self.path.clone();
        let cfg = tokio::task::spawn_blocking(move || load_gateway_config(&path))
            .await
            .map_err(|e| ToolError::internal_message(format!("config read task failed: {e}")))??;
        // Seed the config.toml fallbacks for the small set of pool-internal
        // env-resolved caches (see `pool/helpers.rs` / `pool/stdio_stderr.rs`
        // doc comments) — a no-op after the first successful reload, since
        // those caches are themselves resolved once per process.
        crate::upstream::pool::install_max_response_bytes_default(
            cfg.gateway.upstream_max_response_bytes,
        );
        crate::upstream::pool::install_upstream_stderr_level_default(
            cfg.gateway.upstream_stderr_level.clone(),
        );
        crate::upstream::pool::install_upstream_discovery_concurrency_default(
            cfg.gateway.upstream_discovery_concurrency,
        );
        labby_codemode::install_artifact_config_defaults(
            cfg.code_mode.artifact_retention_runs,
            cfg.code_mode.artifact_max_mib,
            cfg.code_mode.artifact_max_store_mib,
        );
        labby_codemode::install_call_budget_config_defaults(
            cfg.code_mode.max_calls_per_run,
            cfg.code_mode.calltool_result_max_mib,
        );
        let registry = self.builtin_service_registry();
        let (cfg, migration) = quarantine_unregistered_virtual_servers(cfg, registry.as_ref());
        if migration.changed() {
            tracing::warn!(
                action = "gateway.config.migrate",
                stale_virtual_server_count = migration.quarantined.len(),
                stale_virtual_servers = ?migration.quarantined,
                "quarantined virtual servers with unregistered backing services"
            );
            self.persist_config(cfg.clone()).await?;
        }
        let runtime_cfg = {
            let current = self.config.read().await;
            super::config_transaction::runtime_config_for_desired(&current, &cfg)
        };
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "catalog.config.loaded",
            phase = "config.load.finish",
            upstream_count = cfg.upstream.len(),
            virtual_server_count = cfg.virtual_servers.len(),
            quarantined_virtual_server_count = cfg.quarantined_virtual_servers.len(),
            "gateway reconcile"
        );
        self.reconcile_upstream_oauth_managers(&cfg);

        // Project catalog snapshots through the visible Code Mode contract so
        // `tools/list_changed` fires on real client-facing changes only. The
        // "before" snapshot uses the currently-active regime; the "after"
        // snapshot uses the incoming config. When Code Mode itself toggles this
        // correctly captures the raw↔`codemode` transition as a real change.
        // The synthetic tool names are stable, while enabled upstream namespace
        // names and normalized operator hints are intentionally rendered into
        // their descriptions. Snapshot those determinants so hint/name changes
        // publish a real tools/list_changed notification.
        let (old_code_mode_enabled, old_mcp_ui_enabled, old_ns_tokens, lab_owned_surface_changed) = {
            let current = self.config.read().await;
            (
                current.code_mode.enabled,
                current.code_mode.mcp_ui_enabled,
                code_mode_namespace_tokens(&current),
                current.code_mode.enabled != runtime_cfg.code_mode.enabled
                    || current.code_mode.mcp_ui_enabled != runtime_cfg.code_mode.mcp_ui_enabled
                    || current.mcp_apps != runtime_cfg.mcp_apps
                    || current.virtual_servers != runtime_cfg.virtual_servers
                    || current.protected_mcp_routes != runtime_cfg.protected_mcp_routes,
            )
        };
        let new_code_mode_enabled = runtime_cfg.code_mode.enabled;
        let new_mcp_ui_enabled = runtime_cfg.code_mode.mcp_ui_enabled;
        let new_ns_tokens = code_mode_namespace_tokens(&runtime_cfg);

        let (previous_cfg, pool_settings_unchanged, changed_upstreams) = {
            let current = self.config.read().await;
            let changed_upstreams = upstream_changed_names(&current, &cfg);
            (
                current.clone(),
                pool_settings_fingerprint(&current) == pool_settings_fingerprint(&cfg),
                changed_upstreams,
            )
        };
        let existing_pool = self.runtime.current_pool().await;
        if pool_settings_unchanged && existing_pool.is_some() && changed_upstreams.is_empty() {
            self.reconcile_runtime_state(&cfg, existing_pool.as_deref())
                .await?;
            let _publication = self.publication_barrier.write().await;
            self.store
                .set_process_code_mode_enabled(runtime_cfg.code_mode.enabled);
            self.code_mode_app_state
                .set_enabled(runtime_cfg.code_mode.mcp_ui_enabled);
            *self.protected_route_index.write().await =
                ProtectedRouteIndex::from_routes(&runtime_cfg.protected_mcp_routes);
            *self.config.write().await = runtime_cfg;
            self.advance_runtime_config_generation();
            let diff = GatewayCatalogDiff {
                tools_changed: lab_owned_surface_changed,
                resources_changed: lab_owned_surface_changed,
                ..GatewayCatalogDiff::default()
            };
            self.notify_catalog_changes(&diff, SOURCE_GATEWAY_RELOAD_SELECTIVE);
            tracing::info!(
                surface = "dispatch",
                service = "gateway",
                action = "gateway.reload",
                event = "catalog.refresh.finish",
                phase = "finish",
                pool_rebuild_skipped = true,
                lab_owned_surface_changed,
                tools_changed = diff.tools_changed,
                elapsed_ms = started.elapsed().as_millis(),
                "gateway reconcile (upstream runtime inputs unchanged; live pool preserved)"
            );
            return Ok(diff);
        }

        if allow_in_place_selective
            && pool_settings_unchanged
            && !changed_upstreams.is_empty()
            && let Some(pool) = existing_pool.as_ref()
        {
            let before = snapshot_from_pool(
                Some(Arc::clone(pool)),
                old_code_mode_enabled,
                old_mcp_ui_enabled,
                &old_ns_tokens,
            )
            .await;
            // Publish the new config and perform only the in-memory changed-name
            // eviction/seed while holding the publication writer. Never wait on
            // upstream I/O under this lock: several MCP readers retain the live
            // pool Arc directly, so a network probe here would stretch a tiny
            // config/runtime convergence window into the full upstream timeout.
            {
                let _publication = self.publication_barrier.write().await;
                self.store
                    .set_process_code_mode_enabled(cfg.code_mode.enabled);
                self.code_mode_app_state
                    .set_enabled(cfg.code_mode.mcp_ui_enabled);
                *self.protected_route_index.write().await =
                    ProtectedRouteIndex::from_routes(&cfg.protected_mcp_routes);
                *self.config.write().await = cfg.clone();
                self.advance_runtime_config_generation();
                pool.reconcile_lazy_upstreams(
                    &cfg.upstream,
                    &changed_upstreams,
                    "gateway.reload.transactional_selective",
                )
                .await;
            }

            // Only the changed upstreams are cold-probed, after the candidate
            // revision is published. During this wait the pool/config pair is a
            // valid lazy-runtime state rather than a private candidate leaking
            // behind the old config revision.
            probe_reload_upstreams(
                Arc::clone(pool),
                &cfg,
                Some(&changed_upstreams),
                owner.as_ref(),
            )
            .await;
            let after = snapshot_from_pool(
                Some(Arc::clone(pool)),
                new_code_mode_enabled,
                new_mcp_ui_enabled,
                &new_ns_tokens,
            )
            .await;

            // Runtime-state persistence is the only fallible step after the
            // in-place reconcile. If it fails, restore both the published config
            // and the changed pool entries before surfacing the error; otherwise
            // the outer disk rollback would see the old config already live and
            // incorrectly treat the candidate-mutated pool as a no-op reload.
            if let Err(error) = self
                .reconcile_runtime_state(&cfg, Some(pool.as_ref()))
                .await
            {
                tracing::warn!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "gateway.reload",
                    event = "selective.rollback.start",
                    phase = "runtime_state",
                    error = %error,
                    changed_upstream_count = changed_upstreams.len(),
                    "transactional selective reconcile failed; restoring prior live revision"
                );
                {
                    let _publication = self.publication_barrier.write().await;
                    self.store
                        .set_process_code_mode_enabled(previous_cfg.code_mode.enabled);
                    self.code_mode_app_state
                        .set_enabled(previous_cfg.code_mode.mcp_ui_enabled);
                    *self.protected_route_index.write().await =
                        ProtectedRouteIndex::from_routes(&previous_cfg.protected_mcp_routes);
                    *self.config.write().await = previous_cfg.clone();
                    self.advance_runtime_config_generation();
                    pool.reconcile_lazy_upstreams(
                        &previous_cfg.upstream,
                        &changed_upstreams,
                        "gateway.reload.transactional_selective.rollback",
                    )
                    .await;
                }
                // Leave restored upstreams lazy. Rollback must not depend on
                // network availability; the next real request can reconnect the
                // previous runtime on demand.
                if let Err(rollback_error) = self
                    .reconcile_runtime_state(&previous_cfg, Some(pool.as_ref()))
                    .await
                {
                    tracing::error!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "gateway.reload",
                        event = "selective.rollback.runtime_state_error",
                        error = %rollback_error,
                        "prior live revision restored but runtime-state persistence still failed"
                    );
                }
                tracing::warn!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "gateway.reload",
                    event = "selective.rollback.finish",
                    changed_upstream_count = changed_upstreams.len(),
                    "transactional selective live revision restored"
                );
                return Err(error);
            }

            let observed = ReconcileCatalogObservation::observe(
                &before,
                &after,
                new_code_mode_enabled,
                lab_owned_surface_changed,
            );
            let diff = observed.diff.clone();
            self.notify_catalog_changes(&diff, SOURCE_GATEWAY_RELOAD_SELECTIVE);
            tracing::info!(
                surface = "dispatch",
                service = "gateway",
                action = "gateway.reload",
                event = "catalog.refresh.finish",
                phase = "finish",
                source = SOURCE_GATEWAY_RELOAD_SELECTIVE,
                pool_rebuild_skipped = true,
                transactional_selective = true,
                changed_upstream_count = changed_upstreams.len(),
                lab_owned_surface_changed,
                projection = observed.projection,
                tools_changed = diff.tools_changed,
                resources_changed = diff.resources_changed,
                prompts_changed = diff.prompts_changed,
                tools_added = ?observed.delta.added,
                tools_removed = ?observed.delta.removed,
                namespaces_added = ?observed.delta.namespaces_added,
                namespaces_removed = ?observed.delta.namespaces_removed,
                delta_truncated_count = observed.delta.truncated,
                raw_tools_changed = observed.raw_tools_changed,
                suppressed_raw_churn = observed.suppressed_raw_churn,
                suppressed_raw_churn_total = observed.suppressed_raw_churn_total,
                elapsed_ms = started.elapsed().as_millis(),
                "gateway reconcile (transactional per-upstream selective)"
            );
            return Ok(diff);
        }

        let old_pool = existing_pool;
        let before = snapshot_from_pool(
            old_pool.clone(),
            old_code_mode_enabled,
            old_mcp_ui_enabled,
            &old_ns_tokens,
        )
        .await;
        let old_pool_present = old_pool.is_some();
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "pool.seed.start",
            operation = "lazy_runtime_seed",
            phase = "pool.build.start",
            upstream_count = cfg.upstream.len(),
            "gateway reconcile"
        );
        let fresh_pool = {
            let base_pool =
                self.new_base_pool(cfg.upstream_request_timeout(), cfg.upstream_relay_timeout());
            let pool = Arc::new(
                base_pool
                    .with_runtime_origin(runtime_origin_tag(origin))
                    .with_runtime_owner(owner),
            );
            pool.seed_lazy_upstreams(&cfg.upstream).await;
            Some(pool)
        };
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "pool.seed.finish",
            operation = "lazy_runtime_seed",
            phase = "pool.build.finish",
            elapsed_ms = started.elapsed().as_millis(),
            "gateway reconcile"
        );

        // Full/manual reloads retain the private replacement-pool semantics,
        // including eager discovery of every enabled upstream so raw-mode
        // catalog diffs remain exact. Transactional mutations take the
        // selective branch above and probe only changed upstreams.
        if let Some(ref pool) = fresh_pool {
            probe_reload_upstreams(Arc::clone(pool), &cfg, None, None).await;
        }

        let after = snapshot_from_pool(
            fresh_pool.clone(),
            new_code_mode_enabled,
            new_mcp_ui_enabled,
            &new_ns_tokens,
        )
        .await;
        // Runtime-state persistence is fallible. Complete it while the fresh
        // pool is still private so failure cannot publish a pool/config pair
        // that the caller subsequently rolls back on disk.
        self.reconcile_runtime_state(&cfg, fresh_pool.as_deref())
            .await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "pool.swap",
            phase = "pool.swap",
            old_pool_present,
            "gateway reconcile"
        );
        // Serialize publication with credential invalidation. A revocation
        // that wins first leaves no subject-authenticated state in this cold
        // pool; one that wins second observes this pool as current and drains
        // it. This prevents revocation from targeting the old pool while a
        // replacement is concurrently published behind it.
        let oauth_barrier = self
            .oauth_client_cache
            .as_ref()
            .map(|cache| cache.invalidation_barrier());
        let _oauth_publication_guard = match oauth_barrier {
            Some(barrier) => Some(barrier.write_owned().await),
            None => None,
        };
        let _publication = self.publication_barrier.write().await;
        self.store
            .set_process_code_mode_enabled(runtime_cfg.code_mode.enabled);
        self.code_mode_app_state
            .set_enabled(runtime_cfg.code_mode.mcp_ui_enabled);
        self.runtime.swap(fresh_pool).await;
        // Keep the old pool serving throughout build/probe and publish the
        // replacement before draining. A dropped/timeout-cancelled reload can
        // therefore never leave `runtime` as None. Drain in an owned task so
        // cancellation immediately after the atomic swap cannot leak children.
        if let Some(old_pool) = old_pool {
            tokio::spawn(async move {
                tracing::info!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "gateway.reload",
                    event = "old_pool.drain.start",
                    phase = "pool.drain.start",
                    "gateway old upstream pool drain start"
                );
                old_pool.drain_for_swap("gateway.reload.after_swap").await;
                tracing::info!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "gateway.reload",
                    event = "old_pool.drain.finish",
                    phase = "pool.drain.finish",
                    "gateway old upstream pool drain finish"
                );
            });
        }
        *self.protected_route_index.write().await =
            ProtectedRouteIndex::from_routes(&runtime_cfg.protected_mcp_routes);
        *self.config.write().await = runtime_cfg;
        self.advance_runtime_config_generation();
        let observed = ReconcileCatalogObservation::observe(
            &before,
            &after,
            new_code_mode_enabled,
            lab_owned_surface_changed,
        );
        let diff = observed.diff.clone();
        self.notify_catalog_changes(&diff, SOURCE_GATEWAY_RELOAD_FULL);
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "catalog.refresh.finish",
            phase = "finish",
            source = SOURCE_GATEWAY_RELOAD_FULL,
            lab_owned_surface_changed,
            projection = observed.projection,
            tools_changed = diff.tools_changed,
            resources_changed = diff.resources_changed,
            prompts_changed = diff.prompts_changed,
            tools_added = ?observed.delta.added,
            tools_removed = ?observed.delta.removed,
            namespaces_added = ?observed.delta.namespaces_added,
            namespaces_removed = ?observed.delta.namespaces_removed,
            delta_truncated_count = observed.delta.truncated,
            raw_tools_changed = observed.raw_tools_changed,
            suppressed_raw_churn = observed.suppressed_raw_churn,
            suppressed_raw_churn_total = observed.suppressed_raw_churn_total,
            before_tool_count = before.visible.tools.len(),
            after_tool_count = after.visible.tools.len(),
            before_raw_tool_count = before.raw_tools.len(),
            after_raw_tool_count = after.raw_tools.len(),
            before_resource_count = before.visible.resources.len(),
            after_resource_count = after.visible.resources.len(),
            before_prompt_count = before.visible.prompts.len(),
            after_prompt_count = after.visible.prompts.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "gateway reconcile"
        );
        Ok(diff)
    }

    /// `source` is the emitting site, carried through to the MCP peer fanout so
    /// every `tools/list_changed` is attributable. Use a label from
    /// `labby_runtime::catalog_notify`.
    pub(super) fn notify_catalog_changes(&self, diff: &GatewayCatalogDiff, source: &'static str) {
        if !diff.tools_changed && !diff.resources_changed && !diff.prompts_changed {
            return;
        }

        if let Some(notifier) = &self.notifier {
            notifier.notify_catalog_changes(diff, source);
        }
    }
}

async fn probe_reload_upstreams(
    pool: Arc<UpstreamPool>,
    cfg: &GatewayConfig,
    allowed_names: Option<&HashSet<String>>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
) {
    let concurrency = crate::upstream::pool::upstream_discovery_concurrency(
        cfg.gateway.upstream_discovery_concurrency,
    );
    let enabled = cfg
        .upstream
        .iter()
        .filter(|upstream| upstream.enabled)
        .filter(|upstream| allowed_names.is_none_or(|names| names.contains(upstream.name.as_str())))
        .cloned()
        .collect::<Vec<_>>();
    let resource_names = enabled
        .iter()
        .filter(|upstream| upstream.proxy_resources)
        .map(|upstream| upstream.name.clone())
        .collect::<BTreeSet<_>>();

    futures::stream::iter(enabled)
        .map(|upstream| {
            let pool = Arc::clone(&pool);
            async move {
                let name = upstream.name.clone();
                match pool
                    .ensure_tools_for_upstream(&upstream, None, runtime_owner)
                    .await
                {
                    Ok(true) => tracing::info!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "gateway.reload",
                        event = "upstream.probe.connected",
                        upstream = %name,
                        "upstream probed and connected on reload"
                    ),
                    Ok(false) => tracing::debug!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "gateway.reload",
                        event = "upstream.probe.cached",
                        upstream = %name,
                        "upstream already healthy; probe skipped"
                    ),
                    Err(error) => tracing::warn!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "gateway.reload",
                        event = "upstream.probe.error",
                        upstream = %name,
                        error = %error,
                        "upstream probe failed on reload"
                    ),
                }
            }
        })
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;

    // Resource ownership must be populated after tool discovery because only
    // connected peers can answer resources/list. A transactional selective
    // reconcile limits that fan-out to the upstreams that actually changed.
    match allowed_names {
        Some(_) if !resource_names.is_empty() => {
            pool.list_upstream_resources_allowed(Some(&resource_names))
                .await;
        }
        Some(_) => {}
        None => {
            pool.list_upstream_resources().await;
        }
    }
}

pub(super) fn quarantine_unregistered_virtual_servers(
    mut cfg: GatewayConfig,
    registry: &dyn GatewayServiceRegistry,
) -> (GatewayConfig, VirtualServerMigration) {
    let mut migration = VirtualServerMigration::default();
    let mut active = Vec::with_capacity(cfg.virtual_servers.len());

    for virtual_server in std::mem::take(&mut cfg.virtual_servers) {
        if registry.contains_service(&virtual_server.service) {
            active.push(virtual_server);
            continue;
        }

        migration.quarantined.push(virtual_server.id.clone());
        let already_quarantined = cfg
            .quarantined_virtual_servers
            .iter()
            .any(|existing| existing.id == virtual_server.id);
        if !already_quarantined {
            cfg.quarantined_virtual_servers.push(virtual_server);
        }
    }

    cfg.virtual_servers = active;
    (cfg, migration)
}

/// Fingerprint of pool-wide settings that require rebuilding the whole pool.
///
/// Per-upstream add/update/remove is handled by selective reconciliation. These
/// settings apply to the pool object itself, so changing them still forces a
/// full swap-and-drain.
fn pool_settings_fingerprint(cfg: &GatewayConfig) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&cfg.gateway).unwrap_or_default());
    hasher.update([0u8]);
    hasher.update(serde_json::to_vec(&cfg.code_mode).unwrap_or_default());
    hasher.update([0u8]);
    hasher.update(cfg.upstream_request_timeout().as_millis().to_le_bytes());
    hasher.update([0u8]);
    hasher.update(cfg.upstream_relay_timeout().as_millis().to_le_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn upstream_changed_names(current: &GatewayConfig, next: &GatewayConfig) -> HashSet<String> {
    let current = upstream_fingerprint_map(current);
    let next = upstream_fingerprint_map(next);
    current
        .keys()
        .chain(next.keys())
        .filter(|name| current.get(*name) != next.get(*name))
        .cloned()
        .collect()
}

fn upstream_fingerprint_map(cfg: &GatewayConfig) -> BTreeMap<String, String> {
    cfg.upstream
        .iter()
        .map(|upstream| {
            (
                upstream.name.clone(),
                crate::gateway::code_mode::catalog_cache::fingerprint(upstream),
            )
        })
        .collect()
}

/// Tokens mirroring the global config-derived determinants of each peer's
/// route-scoped Code Mode description. Route scope itself is tracked by
/// `lab_owned_surface_changed`; this global superset catches upstream
/// add/remove/enable and normalized hint edits without reacting to runtime
/// health or discovered-tool churn.
fn code_mode_namespace_tokens(cfg: &GatewayConfig) -> BTreeSet<String> {
    cfg.upstream
        .iter()
        .filter(|upstream| upstream.enabled)
        .map(|upstream| {
            let hint = upstream
                .code_mode_hint
                .as_deref()
                .and_then(labby_runtime::gateway_config::normalize_code_mode_hint)
                .unwrap_or_default();
            format!("{NS_TOKEN_PREFIX}{}\u{1}{hint}", upstream.name)
        })
        .collect()
}

/// Snapshot the pool's catalog for `tools/list_changed` change detection.
///
/// `code_mode_enabled` selects the **externally visible** tool projection so
/// the diff reflects the client-facing contract, not raw internal pool state.
/// When Code Mode is enabled the MCP surface hides every raw upstream tool
/// behind the `codemode` tool. Diffing raw
/// `healthy_tools()` in that mode makes ordinary upstream churn — an upstream
/// becoming healthy, discovering tools, or being added — flip `tools_changed`
/// and emit a spurious `tools/list_changed`, even though the visible contract
/// (the Lab-owned Code Mode tools and their configured namespace descriptions)
/// never moved. That notification churn is what makes
/// clients discard and rebuild the canonical `codemode` binding. So under Code
/// Mode we snapshot the Lab-owned tools (`codemode_read`, `codemode`,
/// `mcp_app`, and optional `codemode_ui`) plus synthetic tokens for the enabled
/// upstream names and normalized hints rendered into their descriptions.
/// Runtime health, discovered tools, and UI callbacks do not alter those
/// descriptors. The projected set is unchanged during ordinary upstream churn,
/// while configuration changes to names or hints remain observable, and a
/// raw↔Code Mode regime transition observable even with an empty pool.
///
/// The raw tool set is captured alongside the visible one so the reconcile log
/// can report *suppressed* churn — raw upstream movement that correctly did not
/// notify. Without that field the fix is invisible in production: a quiet log
/// looks identical whether nothing happened or everything was filtered.
async fn snapshot_from_pool(
    pool: Option<Arc<UpstreamPool>>,
    code_mode_enabled: bool,
    mcp_ui_enabled: bool,
    namespace_tokens: &BTreeSet<String>,
) -> ProjectedCatalogSnapshot {
    let raw_tools: BTreeSet<String> = match pool.as_ref() {
        Some(pool) => pool
            .healthy_tools()
            .await
            .into_iter()
            .map(|tool| tool.tool.name.to_string())
            .collect(),
        None => BTreeSet::new(),
    };

    let tools = if code_mode_enabled {
        let mut tools = BTreeSet::from([
            CODE_MODE_READ_TOOL_NAME.to_string(),
            CODE_MODE_SYNTHETIC_TOOL_NAME.to_string(),
            CODE_MODE_APP_CONTROL_TOOL_NAME.to_string(),
        ]);
        if mcp_ui_enabled {
            tools.insert(CODE_MODE_UI_TOOL_NAME.to_string());
        }
        tools.extend(namespace_tokens.iter().cloned());
        tools
    } else {
        raw_tools.clone()
    };

    let (resources, prompts) = match pool.as_ref() {
        Some(pool) => (
            pool.routable_upstream_names(crate::upstream::types::UpstreamCapability::Resources)
                .await
                .into_iter()
                .collect(),
            pool.routable_upstream_names(crate::upstream::types::UpstreamCapability::Prompts)
                .await
                .into_iter()
                .collect(),
        ),
        None => (BTreeSet::new(), BTreeSet::new()),
    };

    ProjectedCatalogSnapshot {
        visible: GatewayCatalogSnapshot {
            tools,
            resources,
            prompts,
        },
        raw_tools,
    }
}

/// A reconcile snapshot in both projections: the client-visible contract that
/// drives `tools/list_changed`, and the raw upstream tool set behind it.
#[derive(Debug, Default)]
struct ProjectedCatalogSnapshot {
    visible: GatewayCatalogSnapshot,
    raw_tools: BTreeSet<String>,
}

/// Everything the reconcile finish log needs to say about a catalog transition:
/// what the client will be told, what actually moved underneath, and whether
/// raw churn was correctly withheld.
struct ReconcileCatalogObservation {
    diff: GatewayCatalogDiff,
    delta: CatalogToolDelta,
    projection: &'static str,
    raw_tools_changed: bool,
    suppressed_raw_churn: bool,
    suppressed_raw_churn_total: u64,
}

impl ReconcileCatalogObservation {
    /// Compares both projections and, as a side effect, advances the
    /// process-lifetime suppressed-churn counter when this reconcile withheld a
    /// notification. Call exactly once per reconcile.
    fn observe(
        before: &ProjectedCatalogSnapshot,
        after: &ProjectedCatalogSnapshot,
        code_mode_enabled: bool,
        force_tools_changed: bool,
    ) -> Self {
        let mut diff = diff_catalogs(&before.visible, &after.visible);
        diff.tools_changed |= force_tools_changed;
        let raw_tools_changed = before.raw_tools != after.raw_tools;
        let suppressed_raw_churn = raw_tools_changed && !diff.tools_changed;
        // `fetch_add` returns the previous value; report the post-increment
        // total so the log reads as "this is the Nth suppression".
        let suppressed_raw_churn_total = if suppressed_raw_churn {
            SUPPRESSED_RAW_CHURN_TOTAL.fetch_add(1, Ordering::Relaxed) + 1
        } else {
            SUPPRESSED_RAW_CHURN_TOTAL.load(Ordering::Relaxed)
        };

        Self {
            delta: CatalogToolDelta::describe(&before.visible.tools, &after.visible.tools),
            diff,
            projection: if code_mode_enabled {
                "code_mode_visible"
            } else {
                "raw"
            },
            raw_tools_changed,
            suppressed_raw_churn,
            suppressed_raw_churn_total,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        CatalogToolDelta, GatewayCatalogSnapshot, MAX_LOGGED_DELTA_NAMES, NS_TOKEN_PREFIX,
        ProjectedCatalogSnapshot, ReconcileCatalogObservation, diff_catalogs, snapshot_from_pool,
    };

    fn snapshot(tools: BTreeSet<String>) -> GatewayCatalogSnapshot {
        GatewayCatalogSnapshot {
            tools,
            resources: BTreeSet::new(),
            prompts: BTreeSet::new(),
        }
    }

    fn names(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn projected(visible: &[&str], raw: &[&str]) -> ProjectedCatalogSnapshot {
        ProjectedCatalogSnapshot {
            visible: snapshot(names(visible)),
            raw_tools: names(raw),
        }
    }

    #[tokio::test]
    async fn code_mode_projection_includes_lab_owned_tools_and_description_determinants() {
        let namespace_tokens = names(&[&format!(
            "{NS_TOKEN_PREFIX}apps\u{1}Search connected application data"
        )]);
        let snapshot = snapshot_from_pool(None, true, true, &namespace_tokens).await;

        assert!(snapshot.visible.tools.contains("codemode"));
        assert!(snapshot.visible.tools.contains("codemode_read"));
        assert!(snapshot.visible.tools.contains("codemode_ui"));
        assert!(snapshot.visible.tools.contains("mcp_app"));
        assert!(snapshot.visible.tools.is_superset(&namespace_tokens));
        assert!(snapshot.raw_tools.is_empty());

        let without_ui = snapshot_from_pool(None, true, false, &namespace_tokens).await;
        assert!(!without_ui.visible.tools.contains("codemode_ui"));
        assert!(without_ui.visible.tools.is_superset(&namespace_tokens));
        assert!(diff_catalogs(&without_ui.visible, &snapshot.visible).tools_changed);

        let raw = snapshot_from_pool(None, false, false, &namespace_tokens).await;
        assert!(diff_catalogs(&raw.visible, &snapshot.visible).tools_changed);
    }

    #[test]
    fn namespace_description_changes_are_logged_and_notify_clients() {
        let before = names(&[&format!("{NS_TOKEN_PREFIX}apps\u{1}old hint")]);
        let after = names(&[&format!("{NS_TOKEN_PREFIX}apps\u{1}new hint")]);

        let delta = CatalogToolDelta::describe(&before, &after);

        assert_eq!(delta.namespaces_added, ["apps"]);
        assert_eq!(delta.namespaces_removed, ["apps"]);
        assert!(diff_catalogs(&snapshot(before), &snapshot(after)).tools_changed);
    }

    #[test]
    fn delta_caps_logged_names_and_reports_the_remainder() {
        // A cold-start reconcile legitimately adds hundreds of tools; the log
        // line must stay bounded while still saying how much it dropped.
        let before = BTreeSet::new();
        let after: BTreeSet<String> = (0..MAX_LOGGED_DELTA_NAMES + 7)
            .map(|index| format!("tool_{index:03}"))
            .collect();

        let delta = CatalogToolDelta::describe(&before, &after);

        assert_eq!(delta.added.len(), MAX_LOGGED_DELTA_NAMES);
        assert_eq!(delta.truncated, 7);
    }

    #[test]
    fn observation_flags_suppressed_raw_churn_under_code_mode() {
        // The incident shape: an upstream comes online and discovers tools, but
        // the Code-Mode-visible contract is unmoved. No notification is due —
        // and the reconcile log must still say the raw set moved, otherwise the
        // suppression is invisible in production.
        let before = projected(&["codemode"], &["youtube_search_ui"]);
        let after = projected(&["codemode"], &["youtube_search_ui", "search"]);

        let observed = ReconcileCatalogObservation::observe(&before, &after, true, false);

        assert!(!observed.diff.tools_changed, "no visible change; no notify");
        assert!(observed.raw_tools_changed);
        assert!(observed.suppressed_raw_churn);
        assert!(observed.suppressed_raw_churn_total >= 1);
        assert_eq!(observed.projection, "code_mode_visible");
    }

    #[test]
    fn observation_does_not_flag_suppression_for_a_real_visible_change() {
        let before = projected(&["youtube_search_ui"], &["youtube_search_ui"]);
        let after = projected(
            &["youtube_search_ui", "podcast_ui"],
            &["youtube_search_ui", "podcast_ui"],
        );

        let observed = ReconcileCatalogObservation::observe(&before, &after, true, false);

        assert!(observed.diff.tools_changed);
        assert!(observed.raw_tools_changed);
        assert!(
            !observed.suppressed_raw_churn,
            "a notified change is not a suppression"
        );
        assert_eq!(observed.delta.added, vec!["podcast_ui".to_string()]);
    }

    #[test]
    fn observation_outside_code_mode_reports_the_raw_projection() {
        let before = projected(&["search"], &["search"]);
        let after = projected(&["search", "download"], &["search", "download"]);

        let observed = ReconcileCatalogObservation::observe(&before, &after, false, false);

        assert_eq!(observed.projection, "raw");
        assert!(observed.diff.tools_changed);
        assert!(
            !observed.suppressed_raw_churn,
            "raw churn is a visible change when Code Mode is off, never suppressed"
        );
    }

    #[test]
    fn lab_owned_policy_change_forces_peer_recomputation_without_raw_churn() {
        let before = projected(&["codemode"], &["search"]);
        let after = projected(&["codemode"], &["search"]);

        let observed = ReconcileCatalogObservation::observe(&before, &after, true, true);

        assert!(observed.diff.tools_changed);
        assert!(!observed.raw_tools_changed);
        assert!(!observed.suppressed_raw_churn);
    }
}
