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

/// Prefix marking a synthetic namespace token inside a Code-Mode-visible
/// snapshot. `\u{1}` cannot appear in an upstream name or a real tool name, so
/// tokens stay disjoint from tool names in the same `BTreeSet`.
const NS_TOKEN_PREFIX: &str = "\u{1}ns\u{1}";

/// The rendered delta between two catalog snapshots, split so operators read
/// tool names and namespace tokens as the different things they are.
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
                // A namespace token is `\u{1}ns\u{1}<name>\u{1}<hint>`; render
                // only the namespace name — the hint is operator-authored prose
                // that would swamp the line, and its *presence* in the delta is
                // the signal.
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
        let _mutation_guard = self.config_mutation.lock().await;
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

    pub(super) async fn reload_with_origin_unlocked(
        &self,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
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
        // The namespace tokens mirror the config-derived determinants of the
        // visible `codemode` tool description, so upstream add/remove/enable and
        // hint edits still notify while raw tool health/discovery churn does not.
        let (old_code_mode_enabled, old_ns_tokens) = {
            let current = self.config.read().await;
            (
                current.code_mode.enabled,
                code_mode_namespace_tokens(&current),
            )
        };
        let new_code_mode_enabled = cfg.code_mode.enabled;
        let new_ns_tokens = code_mode_namespace_tokens(&cfg);

        let (pool_settings_unchanged, changed_upstreams, changed_upstreams_add_only) = {
            let current = self.config.read().await;
            let changed_upstreams = upstream_changed_names(&current, &cfg);
            let changed_upstreams_add_only =
                upstream_changes_are_add_only(&current, &changed_upstreams);
            (
                pool_settings_fingerprint(&current) == pool_settings_fingerprint(&cfg),
                changed_upstreams,
                changed_upstreams_add_only,
            )
        };
        let existing_pool = self.runtime.current_pool().await;
        if pool_settings_unchanged && existing_pool.is_some() && changed_upstreams.is_empty() {
            *self.protected_route_index.write().await =
                ProtectedRouteIndex::from_routes(&cfg.protected_mcp_routes);
            let current_pool = existing_pool;
            *self.config.write().await = cfg;
            let current_cfg = self.config.read().await.clone();
            self.reconcile_runtime_state(&current_cfg, current_pool.as_deref())
                .await?;
            let diff = GatewayCatalogDiff::default();
            tracing::info!(
                surface = "dispatch",
                service = "gateway",
                action = "gateway.reload",
                event = "catalog.refresh.finish",
                phase = "finish",
                pool_rebuild_skipped = true,
                elapsed_ms = started.elapsed().as_millis(),
                "gateway reconcile (upstream runtime inputs unchanged; live pool preserved)"
            );
            return Ok(diff);
        }
        let expected_runtime_origin = runtime_origin_tag(origin);
        let pool_runtime_identity_matches = existing_pool.as_ref().is_some_and(|pool| {
            pool.runtime_identity_matches(&expected_runtime_origin, owner.as_ref())
        });
        if pool_settings_unchanged
            && changed_upstreams_add_only
            && pool_runtime_identity_matches
            && let Some(current_pool) = existing_pool.clone()
        {
            let before = snapshot_from_pool(
                Some(Arc::clone(&current_pool)),
                old_code_mode_enabled,
                &old_ns_tokens,
            )
            .await;
            current_pool
                .reconcile_lazy_upstreams(
                    &cfg.upstream,
                    &changed_upstreams,
                    "gateway.reload.selective_reconcile",
                )
                .await;
            let after = snapshot_from_pool(
                Some(Arc::clone(&current_pool)),
                new_code_mode_enabled,
                &new_ns_tokens,
            )
            .await;
            *self.protected_route_index.write().await =
                ProtectedRouteIndex::from_routes(&cfg.protected_mcp_routes);
            *self.config.write().await = cfg;
            let current_cfg = self.config.read().await.clone();
            self.reconcile_runtime_state(&current_cfg, Some(current_pool.as_ref()))
                .await?;
            let observed =
                ReconcileCatalogObservation::observe(&before, &after, new_code_mode_enabled);
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
                selectively_reconciled_upstream_count = changed_upstreams.len(),
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
                "gateway reconcile (upstream changes selectively reconciled; live pool preserved)"
            );
            return Ok(diff);
        }

        let old_pool = existing_pool;
        let before =
            snapshot_from_pool(old_pool.clone(), old_code_mode_enabled, &old_ns_tokens).await;
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
        self.store
            .set_process_code_mode_enabled(cfg.code_mode.enabled);
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

        // Eagerly probe all upstreams so the after-snapshot reflects real tool
        // counts. seed_lazy_upstreams() only creates skeleton entries with empty
        // tool maps; without this the diff always reports tools_changed: ✗ even
        // when new upstreams were added, because both before and after snapshots
        // are empty (discovery is lazy and only triggered on the first list_tools
        // call). Bounded by LABBY_UPSTREAM_DISCOVERY_CONCURRENCY (default 3) to
        // match the refresh path in code_mode_runtime.rs.
        if let Some(ref pool) = fresh_pool {
            let concurrency = crate::upstream::pool::upstream_discovery_concurrency(
                cfg.gateway.upstream_discovery_concurrency,
            );
            let pool_arc = Arc::clone(pool);
            let enabled: Vec<_> = cfg.upstream.iter().filter(|u| u.enabled).cloned().collect();
            // Step 1: connect all upstreams and discover tools.
            futures::stream::iter(enabled)
                .map(|upstream| {
                    let pool = Arc::clone(&pool_arc);
                    async move {
                        let name = upstream.name.clone();
                        match pool.ensure_tools_for_upstream(&upstream, None, None).await {
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
                            Err(e) => tracing::warn!(
                                surface = "dispatch",
                                service = "gateway",
                                action = "gateway.reload",
                                event = "upstream.probe.error",
                                upstream = %name,
                                error = %e,
                                "upstream probe failed on reload"
                            ),
                        }
                    }
                })
                .buffer_unordered(concurrency)
                .collect::<Vec<_>>()
                .await;
            // Step 2: list resources for proxy_resources upstreams. This populates
            // entry.resource_uris so read_upstream_ui_resource can reverse-lookup
            // the owner of ui:// URIs (e.g. youtube_search_ui's MCP App widget).
            // Must run after tool discovery since list_upstream_resources only
            // contacts already-connected peers.
            pool_arc.list_upstream_resources().await;
        }

        let after =
            snapshot_from_pool(fresh_pool.clone(), new_code_mode_enabled, &new_ns_tokens).await;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "pool.swap",
            phase = "pool.swap",
            old_pool_present,
            "gateway reconcile"
        );
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
            ProtectedRouteIndex::from_routes(&cfg.protected_mcp_routes);
        *self.config.write().await = cfg;
        let current_cfg = self.config.read().await.clone();
        let current_pool = self.runtime.current_pool().await;
        self.reconcile_runtime_state(&current_cfg, current_pool.as_deref())
            .await?;
        let observed = ReconcileCatalogObservation::observe(&before, &after, new_code_mode_enabled);
        let diff = observed.diff.clone();
        self.notify_catalog_changes(&diff, SOURCE_GATEWAY_RELOAD_FULL);
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "catalog.refresh.finish",
            phase = "finish",
            source = SOURCE_GATEWAY_RELOAD_FULL,
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

fn upstream_changes_are_add_only(current: &GatewayConfig, changed_names: &HashSet<String>) -> bool {
    !changed_names.is_empty()
        && changed_names.iter().all(|name| {
            current
                .upstream
                .iter()
                .all(|upstream| upstream.name != *name)
        })
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

/// Tokens mirroring the config-derived determinants of the visible `codemode`
/// tool description — the "Available upstream namespaces" section rendered in
/// `mcp/handlers_tools.rs::code_mode_upstreams_for_description`: each enabled
/// upstream's namespace name plus its normalized Code Mode hint. Changing any of
/// these changes the visible `codemode` tool descriptor and so is a real
/// `tools/list` change; raw upstream tool health/discovery churn is not. Route
/// scope (per-session) cannot be applied here, so this is the global superset —
/// a conservative over-approximation that can only ever over-notify on rare
/// operator config edits, never under-notify.
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
            // `NS_TOKEN_PREFIX` uses \u{1}, a control char that cannot appear in
            // an upstream name or a real tool name, keeping these tokens
            // disjoint from UI-tool names.
            format!("{NS_TOKEN_PREFIX}{}\u{1}{hint}", upstream.name)
        })
        .collect()
}

/// Snapshot the pool's catalog for `tools/list_changed` change detection.
///
/// `code_mode_enabled` selects the **externally visible** tool projection so
/// the diff reflects the client-facing contract, not raw internal pool state.
/// When Code Mode is enabled the MCP surface hides every raw upstream tool
/// behind the constant `codemode` tool and exposes only MCP-App UI tools
/// individually (see `mcp/catalog.rs::snapshot_tool_catalog`). Diffing raw
/// `healthy_tools()` in that mode makes ordinary upstream churn — an upstream
/// becoming healthy, discovering tools, or being added — flip `tools_changed`
/// and emit a spurious `tools/list_changed`, even though the visible contract
/// (`codemode` + UI tools) never moved. That notification churn is what makes
/// clients discard and rebuild the canonical `codemode` binding. So under Code
/// Mode we snapshot the UI-tool names plus `ns_tokens` (the config-derived
/// determinants of the visible `codemode` tool description); `codemode` itself
/// is constant and does not affect the diff.
///
/// The raw tool set is captured alongside the visible one so the reconcile log
/// can report *suppressed* churn — raw upstream movement that correctly did not
/// notify. Without that field the fix is invisible in production: a quiet log
/// looks identical whether nothing happened or everything was filtered.
async fn snapshot_from_pool(
    pool: Option<Arc<UpstreamPool>>,
    code_mode_enabled: bool,
    ns_tokens: &BTreeSet<String>,
) -> ProjectedCatalogSnapshot {
    let Some(pool) = pool else {
        return ProjectedCatalogSnapshot::default();
    };

    let raw_tools: BTreeSet<String> = pool
        .healthy_tools()
        .await
        .into_iter()
        .map(|tool| tool.tool.name.to_string())
        .collect();

    let tools = if code_mode_enabled {
        let mut tools: BTreeSet<String> = pool
            .healthy_ui_tool_names_allowed(None)
            .await
            .into_iter()
            .collect();
        tools.extend(ns_tokens.iter().cloned());
        tools
    } else {
        raw_tools.clone()
    };

    ProjectedCatalogSnapshot {
        visible: GatewayCatalogSnapshot {
            tools,
            resources: pool
                .routable_upstream_names(crate::upstream::types::UpstreamCapability::Resources)
                .await
                .into_iter()
                .collect(),
            prompts: pool
                .routable_upstream_names(crate::upstream::types::UpstreamCapability::Prompts)
                .await
                .into_iter()
                .collect(),
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
    ) -> Self {
        let diff = diff_catalogs(&before.visible, &after.visible);
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
    use std::collections::{BTreeMap, BTreeSet};

    use labby_runtime::gateway_config::{GatewayConfig, UpstreamConfig};

    use super::{
        CatalogToolDelta, GatewayCatalogSnapshot, MAX_LOGGED_DELTA_NAMES, ProjectedCatalogSnapshot,
        ReconcileCatalogObservation, code_mode_namespace_tokens, diff_catalogs,
    };

    fn upstream(name: &str, enabled: bool, hint: Option<&str>) -> UpstreamConfig {
        UpstreamConfig {
            enabled,
            name: name.to_string(),
            url: Some("http://127.0.0.1:9/mcp".to_string()),
            bearer_token_env: None,
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            code_mode_hint: hint.map(str::to_string),
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }
    }

    fn config(upstreams: Vec<UpstreamConfig>) -> GatewayConfig {
        GatewayConfig {
            upstream: upstreams,
            ..GatewayConfig::default()
        }
    }

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

    #[test]
    fn namespace_tokens_track_enabled_upstreams_and_hints() {
        let tokens = code_mode_namespace_tokens(&config(vec![
            upstream("github", true, Some("search repositories")),
            upstream("rustarr", true, None),
            upstream("disabled", false, Some("ignored")),
        ]));

        // Disabled upstreams never reach the visible description.
        assert_eq!(tokens.len(), 2);
        assert!(
            tokens
                .iter()
                .any(|t| t.contains("github") && t.contains("search repositories"))
        );
        assert!(tokens.iter().any(|t| t.contains("rustarr")));
        assert!(!tokens.iter().any(|t| t.contains("disabled")));
    }

    #[test]
    fn adding_an_upstream_changes_the_code_mode_visible_snapshot() {
        // A newly added upstream — even one that brings only raw (non-UI) tools —
        // adds a `codemode` description namespace, a real visible change that must
        // still notify under Code Mode.
        let before = code_mode_namespace_tokens(&config(vec![upstream("github", true, None)]));
        let after = code_mode_namespace_tokens(&config(vec![
            upstream("github", true, None),
            upstream("rustarr", true, None),
        ]));

        assert!(diff_catalogs(&snapshot(before), &snapshot(after)).tools_changed);
    }

    #[test]
    fn editing_a_hint_changes_the_code_mode_visible_snapshot() {
        let before = code_mode_namespace_tokens(&config(vec![upstream("github", true, None)]));
        let after = code_mode_namespace_tokens(&config(vec![upstream(
            "github",
            true,
            Some("search repositories"),
        )]));

        assert!(diff_catalogs(&snapshot(before), &snapshot(after)).tools_changed);
    }

    #[test]
    fn identical_config_yields_no_code_mode_visible_change() {
        // Raw tool health/discovery churn leaves the namespace tokens identical,
        // so the Code-Mode reconcile diff reports no change — no `tools/list_changed`.
        let tokens = code_mode_namespace_tokens(&config(vec![
            upstream("github", true, Some("search repositories")),
            upstream("rustarr", true, None),
        ]));

        assert!(!diff_catalogs(&snapshot(tokens.clone()), &snapshot(tokens)).tools_changed);
    }

    #[test]
    fn delta_separates_namespace_tokens_from_tool_names() {
        let before = code_mode_namespace_tokens(&config(vec![upstream("github", true, None)]));
        let mut after = code_mode_namespace_tokens(&config(vec![
            upstream("github", true, None),
            upstream("rustarr", true, Some("manage media")),
        ]));
        after.insert("youtube_search_ui".to_string());

        let delta = CatalogToolDelta::describe(&before, &after);

        // The namespace token must render as a bare upstream name, not the raw
        // `\u{1}`-delimited sentinel, and must not be mistaken for a tool.
        assert_eq!(delta.namespaces_added, vec!["rustarr".to_string()]);
        assert_eq!(delta.added, vec!["youtube_search_ui".to_string()]);
        assert!(delta.removed.is_empty());
        assert!(delta.namespaces_removed.is_empty());
        assert_eq!(delta.truncated, 0);
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
        let before = projected(&["youtube_search_ui"], &["youtube_search_ui"]);
        let after = projected(&["youtube_search_ui"], &["youtube_search_ui", "search"]);

        let observed = ReconcileCatalogObservation::observe(&before, &after, true);

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

        let observed = ReconcileCatalogObservation::observe(&before, &after, true);

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

        let observed = ReconcileCatalogObservation::observe(&before, &after, false);

        assert_eq!(observed.projection, "raw");
        assert!(observed.diff.tools_changed);
        assert!(
            !observed.suppressed_raw_churn,
            "raw churn is a visible change when Code Mode is off, never suppressed"
        );
    }
}
