//! Code Mode runtime readiness and catalog freshness: upstream warm-up,
//! single-flight catalog reprobe with TTL coalescing, and the rendered-catalog
//! cache used by the `search` surface.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt as _;
use tokio::time::Instant;

use crate::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::gateway::code_mode::{
    CodeModeExecutionSource, CodeModeHistoryEntry, CodeModeSourceLookup,
};
use crate::upstream::pool::{ToolCatalogGeneration, UpstreamPool};
use crate::upstream::types::{UpstreamRuntimeOwner, UpstreamTool};
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::{CodeModeConfig, GatewayConfig, UpstreamConfig};

use super::{CodeModeEmbeddingFlight, CodeModeRefreshFlight, CodeModeRefreshKey, GatewayManager};

/// How long a waiter may reuse the successful refresh it waited behind.
const CATALOG_REFRESH_TTL: std::time::Duration = std::time::Duration::from_secs(30);
/// Limit the request-path OAuth probe; missing subjects continue warming in
/// the background when a public catalog is already usable.
const CODE_MODE_SUBJECT_ENUMERATION_BUDGET: std::time::Duration = std::time::Duration::from_secs(1);

#[cfg(not(test))]
const CATALOG_CACHE_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
#[cfg(test)]
const CATALOG_CACHE_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

/// Cooldown after a TEI failure before the next attempt is tried. Hardcoded
/// per the plan's YAGNI cut — long enough that a flapping/restarting TEI
/// container isn't hit on every search call, short enough that recovery is
/// picked up within one working session.
const SEMANTIC_SEARCH_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);

static CODE_MODE_WARM_UP_IN_FLIGHT: OnceLock<tokio::sync::Mutex<BTreeSet<String>>> =
    OnceLock::new();
#[cfg(test)]
const CODE_MODE_WARM_UP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

struct CodeModeWarmUpAdmission(Arc<AtomicUsize>);

impl CodeModeWarmUpAdmission {
    fn try_acquire(active: &Arc<AtomicUsize>, limit: usize) -> Option<Self> {
        active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < limit).then_some(current + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(active)))
    }
}

impl Drop for CodeModeWarmUpAdmission {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) fn code_mode_warm_up_timeout(
    upstream: &UpstreamConfig,
    request_timeout: std::time::Duration,
) -> std::time::Duration {
    crate::upstream::pool::upstream_discovery_timeout(upstream, request_timeout)
        .saturating_add(std::time::Duration::from_secs(5))
}

fn merge_visible_catalog_tools(
    global: Vec<UpstreamTool>,
    subject_scoped: Vec<UpstreamTool>,
) -> Vec<UpstreamTool> {
    let mut by_identity = BTreeMap::new();
    for tool in global.into_iter().chain(subject_scoped) {
        by_identity
            .entry((tool.upstream_name.to_string(), tool.tool.name.to_string()))
            .or_insert(tool);
    }
    by_identity
        .into_values()
        .take(crate::upstream::pool::MAX_UPSTREAM_TOOLS)
        .collect()
}

async fn healthy_tools_with_generation(
    pool: &UpstreamPool,
    allowed: Option<&BTreeSet<String>>,
) -> (Vec<UpstreamTool>, Option<ToolCatalogGeneration>) {
    let before = pool
        .published_tool_catalog()
        .await
        .ok()
        .map(|snapshot| snapshot.generation());
    let tools = pool.healthy_tools_allowed(allowed).await;
    let after = pool
        .published_tool_catalog()
        .await
        .ok()
        .map(|snapshot| snapshot.generation());
    let generation = if before == after { before } else { None };
    (tools, generation)
}

#[derive(Debug, Clone)]
struct CodeModeReprobeFailure {
    upstream: String,
    message: String,
}

fn upstream_allowed(upstream: &str, allowed_upstreams: Option<&BTreeSet<String>>) -> bool {
    allowed_upstreams.is_none_or(|allowed| allowed.contains(upstream))
}

/// Whether the catalog holds no tools from a REAL upstream.
///
/// The all-upstreams-down hard error is gated on the healthy tool set being
/// empty. FU-1 plants synthetic `__in_process__*` builtin peers into that same
/// catalog, so a plain `is_empty()` check silently went dead the moment one
/// builtin registered — turning "every upstream you configured is
/// unreachable" into a normal-looking `Ok` with a builtin-only catalog, on
/// both the cold-connect and the warm branch. Excluding the synthetic entries
/// keeps the error contract intact for real upstreams while still letting the
/// builtin catalog serve (which is the point of registering before the
/// refresh).
async fn no_real_upstream_tools(
    pool: &UpstreamPool,
    allowed_upstreams: Option<&BTreeSet<String>>,
) -> bool {
    all_tools_are_in_process(&pool.healthy_tools_allowed(allowed_upstreams).await)
}

/// Shared by both emptiness guards so the synthetic-peer exclusion cannot
/// drift between the warm and cold-connect branches.
fn all_tools_are_in_process(tools: &[UpstreamTool]) -> bool {
    tools.iter().all(|tool| {
        tool.upstream_name
            .starts_with(labby_runtime::gateway_config::IN_PROCESS_UPSTREAM_PREFIX)
    })
}

/// Wall-clock a Code Mode catalog build may spend contacting upstreams:
/// half the configured execution timeout, so proxy generation leaves the
/// sandbox roughly the other half (less the broker's response reserve and
/// catalog rendering). Shared by one-shot CLI cold-connects and the long-lived
/// MCP refresh path so neither surface can consume the whole execution budget.
fn catalog_connect_budget(code_mode: &CodeModeConfig) -> std::time::Duration {
    std::time::Duration::from_millis(code_mode.timeout_ms) / 2
}

/// Restore configuration order after concurrent probes settle in arbitrary
/// order, so the rendered proxy is stable across runs. (The live catalog path
/// orders alphabetically instead; both orders are deterministic.)
fn sort_tools_by_config_order(tools: &mut [UpstreamTool], upstreams: &[UpstreamConfig]) {
    let position: std::collections::HashMap<&str, usize> = upstreams
        .iter()
        .enumerate()
        .map(|(index, upstream)| (upstream.name.as_str(), index))
        .collect();
    tools.sort_by_key(|tool| {
        position
            .get(tool.upstream_name.as_ref())
            .copied()
            .unwrap_or(usize::MAX)
    });
}

impl GatewayManager {
    pub(crate) async fn catalog_render_flight(
        &self,
        fingerprint: &str,
    ) -> Arc<crate::gateway::code_mode::CatalogRenderFlight> {
        let mut flights = self.code_mode_catalog_render_flights.lock().await;
        flights.retain(|_, flight| flight.strong_count() > 0);
        if let Some(flight) = flights.get(fingerprint).and_then(std::sync::Weak::upgrade) {
            return flight;
        }
        let flight = Arc::new(crate::gateway::code_mode::CatalogRenderFlight::default());
        flights.insert(fingerprint.to_string(), Arc::downgrade(&flight));
        flight
    }

    pub async fn code_mode_config(&self) -> CodeModeConfig {
        self.config.read().await.code_mode.clone()
    }

    /// Location of the one-shot CLI catalog cache: the product path unless a
    /// test injected an isolated file.
    pub(crate) fn code_mode_catalog_cache_path(&self) -> PathBuf {
        #[cfg(test)]
        if let Some(path) = &self.code_mode_catalog_cache_path {
            return path.clone();
        }
        crate::gateway::code_mode::catalog_cache::cache_path()
    }

    #[cfg(test)]
    pub(crate) fn set_code_mode_catalog_cache_path_for_tests(&mut self, path: PathBuf) {
        self.code_mode_catalog_cache_path = Some(path);
    }

    /// Shared, long-lived Code Mode warm-runner pool (Perf H1).
    ///
    /// The broker checks out a runner from this pool per execution. The pool is
    /// `Arc`-shared across every `Clone` of the manager so a single set of
    /// long-lived runner processes serves all surfaces.
    pub(crate) fn code_mode_runner_pool(&self) -> &Arc<crate::gateway::code_mode::RunnerPool> {
        &self.code_mode_runner_pool
    }

    /// Drain the Code Mode runner pool before the hosting runtime exits.
    pub async fn shutdown_code_mode_runner_pool(&self) {
        self.code_mode_runner_pool.shutdown().await;
    }

    pub async fn record_code_mode_history(&self, entry: CodeModeHistoryEntry) {
        self.code_mode_history.lock().await.push(entry);
    }

    pub async fn record_code_mode_source(&self, source: CodeModeExecutionSource) {
        self.code_mode_source_store.lock().await.push(source);
    }

    pub async fn resolve_code_mode_source(
        &self,
        execution_id: &str,
        lookup: &CodeModeSourceLookup,
    ) -> Result<CodeModeExecutionSource, ToolError> {
        self.code_mode_source_store
            .lock()
            .await
            .resolve(execution_id, lookup)
    }

    pub async fn code_mode_history_snapshot(&self) -> Vec<CodeModeHistoryEntry> {
        self.code_mode_history.lock().await.snapshot()
    }

    pub async fn code_mode_history_snapshot_for_route_scope(
        &self,
        route_scope: Option<&str>,
    ) -> Vec<CodeModeHistoryEntry> {
        self.code_mode_history
            .lock()
            .await
            .snapshot_for_route_scope(route_scope)
    }

    pub async fn code_mode_enabled(&self) -> bool {
        self.config.read().await.code_mode.enabled
    }

    /// Ensure the upstream pool is warm and every enabled upstream has its tool
    /// list connected. Cloudflare-parity: there is no vector/lexical code-mode
    /// index to build — the `search` tool runs the caller's JS over the live
    /// catalog. When `wait_for_refresh` is set, connect upstreams synchronously
    /// so the first cold call sees a populated catalog; otherwise fire-and-forget.
    #[allow(dead_code)]
    pub async fn ensure_search_runtime_ready(
        &self,
        wait_for_refresh: bool,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<(), ToolError> {
        self.ensure_search_runtime_ready_allowed(wait_for_refresh, owner, oauth_subject, None)
            .await
    }

    async fn ensure_search_runtime_ready_allowed(
        &self,
        wait_for_refresh: bool,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
        allowed_upstreams: Option<&BTreeSet<String>>,
    ) -> Result<(), ToolError> {
        let cfg = self.config.read().await.clone();
        if !cfg.code_mode.enabled {
            return Ok(());
        }

        let pool = self.ensure_lazy_upstream_pool(owner).await;
        if wait_for_refresh {
            let mut failures = Vec::new();
            for upstream in cfg
                .upstream
                .iter()
                .filter(|u| u.enabled && upstream_allowed(&u.name, allowed_upstreams))
            {
                if upstream.oauth.is_some() && oauth_subject.is_none() {
                    continue;
                }
                let subject = upstream.oauth.as_ref().and(oauth_subject);
                if let Err(err) = pool
                    .ensure_tools_for_upstream(upstream, subject, owner)
                    .await
                {
                    failures.push(CodeModeReprobeFailure {
                        upstream: upstream.name.clone(),
                        message: err.to_string(),
                    });
                }
            }
            if !failures.is_empty() && no_real_upstream_tools(&pool, allowed_upstreams).await {
                let details = failures
                    .iter()
                    .map(|failure| format!("{}: {}", failure.upstream, failure.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(ToolError::Sdk {
                    sdk_kind: "upstream_connect_error".to_string(),
                    message: format!("failed to connect upstreams for code mode: {details}"),
                });
            }
        } else {
            self.spawn_code_mode_upstream_connections(
                pool,
                &cfg,
                owner,
                oauth_subject,
                allowed_upstreams,
            )
            .await;
        }
        Ok(())
    }

    pub async fn ensure_upstream_tool_runtime_ready(
        &self,
        upstream_name: &str,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<(), ToolError> {
        let cfg = self.config.read().await.clone();
        let Some(upstream) = cfg
            .upstream
            .iter()
            .find(|candidate| candidate.name == upstream_name)
        else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_upstream".to_string(),
                message: format!("unknown upstream `{upstream_name}`"),
            });
        };

        let pool = self.ensure_lazy_upstream_pool(owner).await;

        let subject = upstream.oauth.as_ref().and(oauth_subject);
        pool.ensure_tools_for_upstream(upstream, subject, owner)
            .await
            .map_err(|err| {
                let recovery = if subject.is_some_and(|subject| subject != SHARED_GATEWAY_OAUTH_SUBJECT)
                    && upstream.oauth.as_ref().is_some_and(|oauth| !oauth.credential.is_google_provider())
                {
                    " If credentials are missing, use the native gateway action gateway.oauth.authorize with this upstream name from a lab-scoped connector for the same account (lab:read alone cannot create credentials), open its authorization_url in a browser signed into the same Labby account, then retry. Do not repeat shared gateway authorization or request admin scope for a restricted connector."
                } else { "" };
                ToolError::Sdk {
                    sdk_kind: "upstream_connect_error".to_string(),
                    message: format!("failed to connect upstream `{upstream_name}`: {err}{recovery}"),
                }
            })?;
        Ok(())
    }

    async fn ensure_lazy_upstream_pool(
        &self,
        owner: Option<&UpstreamRuntimeOwner>,
    ) -> Arc<UpstreamPool> {
        // Published pools are owned by configuration reconciliation. A request
        // snapshot must never reseed them or overwrite their recovery policy.
        if let Some(pool) = self.runtime.current_pool().await {
            return pool;
        }

        let _init_guard = self.lazy_pool_init.lock().await;
        let _publication = self.publication_barrier.write().await;
        if let Some(pool) = self.runtime.current_pool_sync() {
            return pool;
        }
        // Read current configuration inside the publication boundary, after
        // waiting for any reload, rather than using the caller's older snapshot.
        let cfg = self.config.read().await.clone();
        let base_pool = self
            .new_base_pool(
                cfg.upstream_request_timeout(),
                cfg.upstream_relay_timeout(),
                cfg.gateway.auto_reconnect,
            )
            .with_runtime_owner(Some(owner.cloned().unwrap_or_else(|| {
                UpstreamRuntimeOwner {
                    surface: "dispatch".to_string(),
                    subject: Some(SHARED_GATEWAY_OAUTH_SUBJECT.to_string()),
                    request_id: None,
                    session_id: None,
                    client_name: None,
                    raw: None,
                }
            })));
        let pool = Arc::new(base_pool);
        pool.seed_lazy_upstreams(&cfg.upstream).await;
        self.runtime.swap(Some(Arc::clone(&pool))).await;
        pool.ensure_recovery_tasks(&cfg.upstream).await;
        pool
    }

    #[allow(dead_code)]
    pub async fn code_mode_catalog_tools(
        &self,
        allow_cold_connect: bool,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<Vec<UpstreamTool>, ToolError> {
        self.code_mode_catalog_tools_allowed(allow_cold_connect, owner, oauth_subject, None)
            .await
    }

    pub async fn code_mode_catalog_tools_allowed(
        &self,
        allow_cold_connect: bool,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
        allowed_upstreams: Option<&BTreeSet<String>>,
    ) -> Result<Vec<UpstreamTool>, ToolError> {
        self.code_mode_catalog_tools_allowed_with_generation(
            allow_cold_connect,
            owner,
            oauth_subject,
            allowed_upstreams,
        )
        .await
        .map(|(tools, _)| tools)
    }

    pub(crate) async fn code_mode_catalog_tools_allowed_with_generation(
        &self,
        allow_cold_connect: bool,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
        allowed_upstreams: Option<&BTreeSet<String>>,
    ) -> Result<(Vec<UpstreamTool>, Option<ToolCatalogGeneration>), ToolError> {
        // FU-1 (issue #210, lab-48z4k): builtin services join the Code Mode
        // catalog as in-process upstream peers so schema and capability
        // arrive together. Root scope only — a ProtectedSubset route's
        // allowlist should never contain the synthetic `__in_process__*`
        // names, and the downstream `upstream_allowed` filter keeps protected
        // routes builtin-free. Gated on the config flag so a gateway with
        // Code Mode disabled never plants synthetic entries into the shared
        // pool. Runs BEFORE the upstream refresh so an all-upstreams-down
        // gateway still serves the builtin catalog: the refresh's hard-error
        // path fires only when the healthy tool set is empty, and the builtin
        // `gateway` tool is most needed exactly when every upstream is broken.
        if allowed_upstreams.is_none() {
            let cfg = self.config.read().await.clone();
            if cfg.code_mode.enabled {
                let pool = self.ensure_lazy_upstream_pool(owner).await;
                let registry = self.builtin_service_registry();
                pool.ensure_in_process_service_peers(registry.as_ref())
                    .await;
            }
        }
        // The long-lived pool already reprobes configured upstreams in the
        // background when auto-reconnect is enabled. Once a real tool is
        // available, a request can use that catalog instead of waiting for an
        // unrelated slow upstream to consume the full refresh budget.
        let mut warm_global = None;
        let has_warm_catalog = if allow_cold_connect {
            let auto_reconnect = self.config.read().await.gateway.auto_reconnect;
            if auto_reconnect {
                if let Some(pool) = self.current_pool().await {
                    let (tools, generation) =
                        healthy_tools_with_generation(&pool, allowed_upstreams).await;
                    if all_tools_are_in_process(&tools) {
                        false
                    } else {
                        warm_global = Some((pool, tools, generation));
                        true
                    }
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        let mut refresh_timed_out = false;
        if allow_cold_connect && !has_warm_catalog {
            let budget = {
                let cfg = self.config.read().await;
                catalog_connect_budget(&cfg.code_mode)
            };
            match tokio::time::timeout(
                budget,
                self.refresh_code_mode_catalog_allowed(owner, oauth_subject, allowed_upstreams),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => {
                    refresh_timed_out = true;
                    tracing::warn!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "code_mode.refresh_catalog",
                        budget_ms = budget.as_millis(),
                        "Code Mode catalog refresh exceeded its wall-clock budget; using already healthy upstreams"
                    );
                }
            }
        } else if !allow_cold_connect {
            self.ensure_search_runtime_ready_allowed(
                false,
                owner,
                oauth_subject,
                allowed_upstreams,
            )
            .await?;
        }
        let Some(pool) = self.current_pool().await else {
            return Ok((Vec::new(), None));
        };
        let (global, generation) = match warm_global {
            Some((warm_pool, tools, generation)) if Arc::ptr_eq(&warm_pool, &pool) => {
                (tools, generation)
            }
            _ => healthy_tools_with_generation(&pool, allowed_upstreams).await,
        };
        if has_warm_catalog {
            self.schedule_code_mode_catalog_cache_sync(Arc::clone(&pool))
                .await;
        }
        let subject_scoped = if let Some(subject) = oauth_subject {
            let cfg = self.config.read().await;
            if has_warm_catalog {
                self.spawn_code_mode_upstream_connections(
                    Arc::clone(&pool),
                    &cfg,
                    owner,
                    Some(subject),
                    allowed_upstreams,
                )
                .await;
            }
            pool.subject_scoped_upstream_tools_allowed_with_deadline(
                &cfg.upstream,
                subject,
                allowed_upstreams,
                crate::upstream::pool::MAX_UPSTREAM_TOOLS,
                CODE_MODE_SUBJECT_ENUMERATION_BUDGET,
            )
            .await
        } else {
            Vec::new()
        };
        let visible = merge_visible_catalog_tools(global, subject_scoped);
        if refresh_timed_out && all_tools_are_in_process(&visible) {
            return Err(ToolError::Sdk {
                sdk_kind: "upstream_connect_error".to_string(),
                message: "Code Mode catalog refresh timed out before any real upstream was usable"
                    .to_string(),
            });
        }
        Ok((
            visible,
            if oauth_subject.is_none() {
                generation
            } else {
                None
            },
        ))
    }

    /// Background recovery keeps the live pool current; publish its healthy
    /// non-OAuth tools to the separate one-shot CLI cache at a bounded rate.
    /// The old synchronous full-fleet refresh performed this write as a side
    /// effect, so the warm fast path needs an independent publication path.
    async fn schedule_code_mode_catalog_cache_sync(&self, pool: Arc<UpstreamPool>) {
        let mut next = self.code_mode_cache_sync_after.lock().await;
        let now = Instant::now();
        let pool_identity = Arc::as_ptr(&pool) as usize;
        if next.is_some_and(|(identity, deadline)| identity == pool_identity && now < deadline) {
            return;
        }
        *next = Some((pool_identity, now + CATALOG_CACHE_SYNC_INTERVAL));
        drop(next);

        let manager = self.clone();
        tokio::spawn(async move {
            if !manager
                .current_pool()
                .await
                .is_some_and(|current| Arc::ptr_eq(&current, &pool))
            {
                return;
            }
            let cfg = manager.config.read().await.clone();
            let mut updates = Vec::new();
            for upstream in cfg
                .upstream
                .iter()
                .filter(|u| u.enabled && u.oauth.is_none())
            {
                let tools = pool.healthy_tools_for_upstream(&upstream.name).await;
                if !tools.is_empty() {
                    updates.push(
                        crate::gateway::code_mode::catalog_cache::CatalogCacheUpdate {
                            upstream_name: upstream.name.clone(),
                            fingerprint: crate::gateway::code_mode::catalog_cache::fingerprint(
                                upstream,
                            ),
                            tools,
                        },
                    );
                }
            }
            if updates.is_empty()
                || !manager
                    .current_pool()
                    .await
                    .is_some_and(|current| Arc::ptr_eq(&current, &pool))
            {
                return;
            }
            crate::gateway::code_mode::catalog_cache::merge_and_store(
                manager.code_mode_catalog_cache_path(),
                updates,
                Vec::new(),
            )
            .await;
        });
    }

    /// One-shot CLI variant of `code_mode_catalog_tools`: serve the codemode
    /// proxy catalog from the on-disk cache, connecting only upstreams whose
    /// cache entry is missing, stale, or fingerprint-mismatched. OAuth upstreams
    /// are subject-scoped, so they are probed on every run (when a subject is
    /// present, which the gateway host always supplies) and never cached.
    ///
    /// A one-shot `labby code run` must not connect the full upstream
    /// fleet per invocation just to generate the `codemode.*` proxy. Tool calls
    /// still resolve live (`resolve_code_mode_upstream_tool` ensures the target
    /// upstream), so a stale cache can only mis-shape the proxy — `callTool`
    /// remains the always-fresh escape hatch. The same path serves
    /// `snippets.exec`, whose executions run on the CLI surface.
    ///
    /// Uncached upstreams are probed concurrently (bounded by
    /// `upstream_discovery_concurrency()`) under a wall-clock budget of half the
    /// configured Code Mode timeout, so proxy generation leaves the sandbox
    /// roughly the other half. This runs inside the broker's proxy-generation
    /// deadline, and a per-upstream discovery timeout is as long as or longer
    /// than that whole deadline (30s HTTP, 60s stdio by default against a 30s
    /// Code Mode timeout): a serial pass let one stalled stdio child spend the
    /// entire budget before any other upstream was reached, and a pass cut off
    /// by the deadline persisted nothing, so every later run was just as cold.
    ///
    /// Partial is loud and never empty: upstreams that fail, are still
    /// connecting, or were never attempted when the budget ends are omitted
    /// from the proxy and are named in a warning; a pass that would leave the
    /// catalog with nothing from cache and nothing connected is an error
    /// instead. Every upstream that did complete is persisted even when the
    /// budget cut the pass short.
    ///
    /// An upstream whose probe genuinely *failed* is additionally recorded as a
    /// short-lived negative cache entry and skipped while that window holds, so
    /// a fleet of dead upstreams stops re-consuming the cold-connect budget on
    /// every invocation. Being cut off by the budget is not a failure and is
    /// never suppressed. Suppression expires on its own and any config edit
    /// clears it, so a recovered upstream returns without operator action.
    #[allow(dead_code)]
    pub async fn code_mode_catalog_tools_cached(
        &self,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<Vec<UpstreamTool>, ToolError> {
        self.code_mode_catalog_tools_cached_allowed(owner, oauth_subject, None)
            .await
    }

    pub async fn code_mode_catalog_tools_cached_allowed(
        &self,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
        allowed_upstreams: Option<&BTreeSet<String>>,
    ) -> Result<Vec<UpstreamTool>, ToolError> {
        use crate::gateway::code_mode::catalog_cache;

        let cfg = self.config.read().await.clone();
        if !cfg.code_mode.enabled {
            return Ok(Vec::new());
        }
        if allowed_upstreams.is_some_and(BTreeSet::is_empty) {
            return Ok(Vec::new());
        }

        let cache_path = self.code_mode_catalog_cache_path();
        let cache = catalog_cache::CatalogCache::load_from(&cache_path);
        let mut tools = Vec::new();
        let mut cache_hits = 0usize;
        // Upstreams skipped outright because a recent failure is still
        // suppressed. Reported like any other omission so a partial catalog
        // never goes unexplained.
        let mut suppressed: Vec<String> = Vec::new();
        // Upstreams that need a live probe, carrying the fingerprint their
        // fresh tools are stored under (`None` for subject-scoped OAuth probes,
        // which are never cached).
        let mut pending: Vec<(UpstreamConfig, Option<String>)> = Vec::new();
        for upstream in cfg
            .upstream
            .iter()
            .filter(|u| u.enabled && upstream_allowed(&u.name, allowed_upstreams))
        {
            if upstream.oauth.is_some() {
                if oauth_subject.is_some() {
                    pending.push((upstream.clone(), None));
                }
                continue;
            }
            let fingerprint = catalog_cache::fingerprint(upstream);
            if let Some(cached) = cache.fresh_tools(&upstream.name, &fingerprint) {
                cache_hits += 1;
                tools.extend(cached);
                continue;
            }
            if cache.probe_suppressed(&upstream.name, &fingerprint) {
                suppressed.push(upstream.name.clone());
                continue;
            }
            pending.push((upstream.clone(), Some(fingerprint)));
        }
        if pending.is_empty() {
            if !suppressed.is_empty() {
                if cache_hits == 0 {
                    tracing::warn!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "code_mode.catalog_cache",
                        suppressed_upstreams = ?suppressed,
                        "one-shot Code Mode catalog has no usable upstreams"
                    );
                    return Err(ToolError::Sdk {
                        sdk_kind: "upstream_connect_error".to_string(),
                        message: format!(
                            "no Code Mode upstream connected for the one-shot catalog: \
                             all upstreams are suppressed by recent probe failures: {}",
                            suppressed.join(", ")
                        ),
                    });
                }
                warn_suppressed(&suppressed);
            }
            sort_tools_by_config_order(&mut tools, &cfg.upstream);
            return Ok(tools);
        }

        // Rotate cold admissions across CLI invocations. Budget cancellation
        // records progress, not a failure or a suppression of slow upstreams.
        if let Some(cursor) = cache.probe_cursor.as_deref()
            && let Some(last) = cfg.upstream.iter().position(|entry| entry.name == cursor)
        {
            pending.sort_by_key(|(entry, _)| {
                let index = cfg
                    .upstream
                    .iter()
                    .position(|candidate| candidate.name == entry.name)
                    .unwrap_or(0);
                (index + cfg.upstream.len() - (last + 1) % cfg.upstream.len()) % cfg.upstream.len()
            });
        }
        let pending_order = pending
            .iter()
            .map(|(entry, _)| entry.name.clone())
            .collect::<Vec<_>>();
        let pool = self.ensure_lazy_upstream_pool(owner).await;
        let concurrency = crate::upstream::pool::upstream_discovery_concurrency(
            cfg.gateway.upstream_discovery_concurrency,
        );
        let budget = catalog_connect_budget(&cfg.code_mode);
        let deadline = Instant::now() + budget;
        let fingerprints: BTreeMap<String, Option<String>> = pending
            .iter()
            .map(|(upstream, fingerprint)| (upstream.name.clone(), fingerprint.clone()))
            .collect();
        let mut outstanding: BTreeSet<String> = fingerprints.keys().cloned().collect();
        // `buffer_unordered` polls at most `concurrency` probes at once, so at
        // the deadline the rest were never attempted; report them as such.
        let started: Arc<std::sync::Mutex<BTreeSet<String>>> = Arc::default();
        let owner_cloned = owner.cloned();
        let oauth_subject_cloned = oauth_subject.map(ToOwned::to_owned);
        let mut probes = Box::pin(
            futures::stream::iter(pending)
                .map(|(upstream, fingerprint)| {
                    let pool = Arc::clone(&pool);
                    let owner = owner_cloned.clone();
                    let oauth_subject = oauth_subject_cloned.clone();
                    let started = Arc::clone(&started);
                    async move {
                        started
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(upstream.name.clone());
                        let subject = upstream.oauth.as_ref().and(oauth_subject.as_deref());
                        let outcome = pool
                            .ensure_tools_for_upstream(&upstream, subject, owner.as_ref())
                            .await;
                        let live = match (&outcome, subject) {
                            (Err(_), _) => Vec::new(),
                            (Ok(_), Some(subject)) => {
                                pool.subject_scoped_upstream_tools_allowed(
                                    std::slice::from_ref(&upstream),
                                    subject,
                                    None,
                                )
                                .await
                            }
                            (Ok(_), None) => pool.healthy_tools_for_upstream(&upstream.name).await,
                        };
                        (upstream, fingerprint, outcome.map(|_| live))
                    }
                })
                .buffer_unordered(concurrency),
        );

        let mut updates = Vec::new();
        let mut connected = 0usize;
        let mut failures = Vec::new();
        let mut failed_upstream_names = Vec::new();
        let mut failed_probes = Vec::new();
        let budget_exhausted = loop {
            match tokio::time::timeout_at(deadline, probes.next()).await {
                Ok(Some((upstream, fingerprint, Ok(live)))) => {
                    outstanding.remove(&upstream.name);
                    connected += 1;
                    if let Some(fingerprint) = fingerprint {
                        updates.push(catalog_cache::CatalogCacheUpdate {
                            upstream_name: upstream.name.clone(),
                            fingerprint,
                            tools: live.clone(),
                        });
                    }
                    tools.extend(live);
                }
                Ok(Some((upstream, _, Err(error)))) => {
                    outstanding.remove(&upstream.name);
                    tracing::debug!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "code_mode.catalog_cache",
                        upstream = %upstream.name,
                        error = %error,
                        "upstream connect failed; omitting from codemode proxy and \
                         suppressing retries briefly"
                    );
                    failed_upstream_names.push(upstream.name.clone());
                    failures.push(format!("{}: {error}", upstream.name));
                    // Only this arm is a real failure. The budget-exhausted
                    // paths below are not, and must not be suppressed.
                    if let Some(Some(fingerprint)) = fingerprints.get(&upstream.name) {
                        failed_probes.push(catalog_cache::CatalogCacheFailure {
                            upstream_name: upstream.name.clone(),
                            fingerprint: fingerprint.clone(),
                        });
                    }
                }
                Ok(None) => break false,
                Err(_elapsed) => break true,
            }
        };
        // Dropping the stream cancels in-flight connects the same way the
        // per-upstream discovery timeout does; the stdio process-group guard
        // reaps any child that was still starting.
        drop(probes);

        let mut in_flight = Vec::new();
        let mut not_attempted = Vec::new();
        if budget_exhausted {
            let started = started
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            for name in outstanding {
                // A connect also refreshes the upstream's resource and prompt
                // caches after its tools are installed, so a probe cut off in
                // that tail is still a connected upstream: keep its tools.
                if let Some(Some(fingerprint)) = fingerprints.get(&name) {
                    let live = pool.healthy_tools_for_upstream(&name).await;
                    if !live.is_empty() {
                        connected += 1;
                        updates.push(catalog_cache::CatalogCacheUpdate {
                            upstream_name: name.clone(),
                            fingerprint: fingerprint.clone(),
                            tools: live.clone(),
                        });
                        tools.extend(live);
                        continue;
                    }
                }
                if started.contains(&name) {
                    in_flight.push(name);
                } else {
                    not_attempted.push(name);
                }
            }
        }
        let probe_cursor = {
            let started = started
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            pending_order
                .into_iter()
                .rev()
                .find(|name| started.contains(name))
        };
        catalog_cache::merge_and_store_with_cursor(
            cache_path,
            updates,
            failed_probes,
            probe_cursor,
        )
        .await;

        // Partial means partial, not empty: with nothing served from cache and
        // nothing connected, the proxy would offer no upstream helpers at all,
        // and a silent empty catalog is exactly what the broker's fail-closed
        // contract forbids.
        if cache_hits == 0 && connected == 0 {
            let mut details = failures;
            if !suppressed.is_empty() {
                details.push(format!(
                    "suppressed by recent probe failures: {}",
                    suppressed.join(", ")
                ));
            }
            if !in_flight.is_empty() {
                details.push(format!(
                    "still connecting when the {}ms cold-connect budget ended: {}",
                    budget.as_millis(),
                    in_flight.join(", ")
                ));
            }
            if !not_attempted.is_empty() {
                details.push(format!(
                    "not attempted within the cold-connect budget: {}",
                    not_attempted.join(", ")
                ));
            }
            tracing::warn!(
                surface = "dispatch",
                service = "gateway",
                action = "code_mode.catalog_cache",
                failed_upstreams = ?failed_upstream_names,
                suppressed_upstreams = ?suppressed,
                in_flight_upstreams = ?in_flight,
                not_attempted_upstreams = ?not_attempted,
                budget_ms = budget.as_millis(),
                "one-shot Code Mode catalog has no usable upstreams"
            );
            return Err(ToolError::Sdk {
                sdk_kind: "upstream_connect_error".to_string(),
                message: format!(
                    "no Code Mode upstream connected for the one-shot catalog: {}",
                    details.join("; ")
                ),
            });
        }
        if !failed_upstream_names.is_empty() {
            tracing::warn!(
                surface = "dispatch",
                service = "gateway",
                action = "code_mode.catalog_cache",
                failed_upstreams = ?failed_upstream_names,
                "one-shot Code Mode catalog is partial because upstream probes failed"
            );
        }
        if !suppressed.is_empty() {
            warn_suppressed(&suppressed);
        }
        if !in_flight.is_empty() || !not_attempted.is_empty() {
            tracing::info!(
                surface = "dispatch",
                service = "gateway",
                action = "code_mode.catalog_cache",
                budget_ms = budget.as_millis(),
                in_flight_upstreams = ?in_flight,
                not_attempted_upstreams = ?not_attempted,
                "cold-connect budget exhausted; omitting unfinished upstreams from codemode proxy (not cached)"
            );
        }
        sort_tools_by_config_order(&mut tools, &cfg.upstream);
        Ok(tools)
    }

    /// Refresh the transient Code Mode catalog from live upstream metadata.
    ///
    /// This is intentionally a manager-level policy for cold catalogs and
    /// installations without background recovery. When auto-reconnect is on,
    /// the pool's periodic probes keep a warm catalog current without making
    /// each Code Mode request wait for the entire upstream fleet.
    ///
    /// **P-H1 improvements:**
    /// - Single-flight + TTL coalescing: while one refresh is in flight, a
    ///   concurrent caller that arrives within `CATALOG_REFRESH_TTL` of the last
    ///   completed refresh skips its own reprobe and rides on the in-flight one.
    ///   This bounds the cost of bursty back-to-back `search` calls **without**
    ///   delaying a cold caller. The TTL suppresses redundant concurrent work;
    ///   the warm catalog path relies on the pool's background probes instead.
    /// - Parallel reprobe: all enabled upstreams are probed concurrently, bounded by
    ///   `upstream_discovery_concurrency()` (default 3, env `LABBY_UPSTREAM_DISCOVERY_CONCURRENCY`).
    #[allow(dead_code)]
    pub async fn refresh_code_mode_catalog(
        &self,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<(), ToolError> {
        self.refresh_code_mode_catalog_allowed(owner, oauth_subject, None)
            .await
    }

    pub(crate) async fn refresh_code_mode_catalog_allowed(
        &self,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
        allowed_upstreams: Option<&BTreeSet<String>>,
    ) -> Result<(), ToolError> {
        let started = Instant::now();
        let cfg = self.config.read().await.clone();
        if !cfg.code_mode.enabled {
            return Ok(());
        }

        let pool = self.ensure_lazy_upstream_pool(owner).await;
        let key = CodeModeRefreshKey {
            pool_identity: Arc::as_ptr(&pool) as usize,
            oauth_subject: oauth_subject.map(ToOwned::to_owned),
            allowed_upstreams: allowed_upstreams.map(|scope| scope.iter().cloned().collect()),
        };
        let flight = {
            let mut flights = self.code_mode_refresh_flights.lock().await;
            flights.retain(|_, value| value.strong_count() > 0);
            flights
                .get(&key)
                .and_then(std::sync::Weak::upgrade)
                .unwrap_or_else(|| {
                    let flight = Arc::new(CodeModeRefreshFlight::default());
                    flights.insert(key, Arc::downgrade(&flight));
                    flight
                })
        };

        // --- Scope-keyed single-flight + TTL coalescing ---
        // Only callers that actually waited for another refresh may reuse its
        // successful result. A lone later caller still reprobes.
        let _inflight_guard = match flight.in_flight.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                let guard = flight.in_flight.lock().await;
                let within_ttl = {
                    let deadline_guard = flight.deadline.lock().await;
                    deadline_guard.is_some_and(|deadline| Instant::now() < deadline)
                };
                if within_ttl {
                    tracing::debug!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "code_mode.refresh_catalog",
                        "concurrent refresh in flight within TTL, coalescing"
                    );
                    return Ok(());
                }
                guard
            }
        };
        *flight.deadline.lock().await = None;

        let concurrency = crate::upstream::pool::upstream_discovery_concurrency(
            cfg.gateway.upstream_discovery_concurrency,
        );

        // Clone context for async move blocks.
        let owner_cloned = owner.cloned();
        let oauth_subject_cloned = oauth_subject.map(ToOwned::to_owned);
        let pool_arc = Arc::clone(&pool);

        // Parallel reprobe — all enabled upstreams concurrently, bounded by concurrency.
        let enabled_upstreams: Vec<_> = cfg
            .upstream
            .iter()
            .filter(|u| {
                u.enabled
                    && upstream_allowed(&u.name, allowed_upstreams)
                    && (u.oauth.is_none() || oauth_subject.is_some())
            })
            .cloned()
            .collect();
        let has_probes = !enabled_upstreams.is_empty();

        let mut probes = Box::pin(
            futures::stream::iter(enabled_upstreams)
                .map(|upstream| {
                    let pool = Arc::clone(&pool_arc);
                    let owner = owner_cloned.clone();
                    let oauth_subject = oauth_subject_cloned.clone();
                    async move {
                        let subject = upstream.oauth.as_ref().and(oauth_subject.as_deref());
                        let outcome = if upstream.oauth.is_some() {
                            pool.ensure_tools_for_upstream(&upstream, subject, owner.as_ref())
                                .await
                        } else {
                            pool.reprobe_tools_for_upstream_as(&upstream, None, owner.as_ref())
                                .await
                        };
                        (upstream, outcome)
                    }
                })
                .buffer_unordered(concurrency),
        );

        let mut failures = Vec::new();
        let mut cache_updates = Vec::new();
        // Leave time for completed probes to be published even when another
        // upstream hangs. Dropping the remaining futures cancels only those
        // probes; successful results are retained below.
        let deadline = started
            + catalog_connect_budget(&cfg.code_mode)
                .saturating_sub(std::time::Duration::from_millis(250));
        let mut timed_out = false;
        loop {
            let next = if has_probes {
                match tokio::time::timeout_at(deadline, probes.next()).await {
                    Ok(next) => next,
                    Err(_) => {
                        timed_out = true;
                        break;
                    }
                }
            } else {
                probes.next().await
            };
            let Some((upstream, outcome)) = next else {
                break;
            };
            match outcome {
                Ok(_) => {
                    // Keep the one-shot CLI catalog cache warm from the
                    // long-lived surface so `code run` rarely has to
                    // cold-connect upstreams for proxy generation.
                    if upstream.oauth.is_none() {
                        cache_updates.push(
                            crate::gateway::code_mode::catalog_cache::CatalogCacheUpdate {
                                upstream_name: upstream.name.clone(),
                                fingerprint: crate::gateway::code_mode::catalog_cache::fingerprint(
                                    &upstream,
                                ),
                                tools: pool.healthy_tools_for_upstream(&upstream.name).await,
                            },
                        );
                    }
                }
                Err(err) => {
                    failures.push(CodeModeReprobeFailure {
                        upstream: upstream.name.clone(),
                        message: err.to_string(),
                    });
                }
            }
        }
        drop(probes);
        // No negative entries from this path: the long-lived MCP surface keeps
        // its own reprobe state and re-probes per call, so cross-invocation
        // suppression here would only mask upstream recovery. The negative cache
        // exists for one-shot CLI runs, which have no such in-process state.
        let cache_path = self.code_mode_catalog_cache_path();
        let cache_write = tokio::spawn(async move {
            crate::gateway::code_mode::catalog_cache::merge_and_store(
                cache_path,
                cache_updates,
                Vec::new(),
            )
            .await;
        });
        if let Err(err) = cache_write.await {
            tracing::warn!(error = %err, "Code Mode catalog cache publication task failed");
        }

        // origin/main widened this to include subject-scoped tools; #210 excludes
        // the synthetic in-process builtin peers. Both matter: the error must
        // still fire when every REAL upstream is unreachable, whether the
        // caller's tools come from the shared pool or an OAuth subject scope.
        let mut available = pool.healthy_tools_allowed(allowed_upstreams).await;
        if let Some(subject) = oauth_subject {
            available.extend(
                pool.subject_scoped_upstream_tools_allowed_with_deadline(
                    &cfg.upstream,
                    subject,
                    allowed_upstreams,
                    crate::upstream::pool::MAX_UPSTREAM_TOOLS,
                    CODE_MODE_SUBJECT_ENUMERATION_BUDGET,
                )
                .await,
            );
        }
        if (timed_out || !failures.is_empty()) && all_tools_are_in_process(&available) {
            let details = failures
                .iter()
                .map(|failure| format!("{}: {}", failure.upstream, failure.message))
                .collect::<Vec<_>>()
                .join("; ");
            let message = if timed_out {
                "Code Mode catalog refresh timed out before any real upstream was usable"
                    .to_string()
            } else {
                format!("failed to refresh Code Mode catalog: {details}")
            };
            return Err(ToolError::Sdk {
                sdk_kind: "upstream_connect_error".to_string(),
                message,
            });
        }

        // Stamp the TTL deadline so a *concurrent* caller that arrives while a
        // later refresh is in flight can coalesce within the freshness window.
        {
            let mut deadline_guard = flight.deadline.lock().await;
            *deadline_guard = Some(Instant::now() + CATALOG_REFRESH_TTL);
        }

        Ok(())
    }

    /// Store a freshly rendered catalog in the manager-level render cache.
    ///
    /// Called by Code Mode catalog discovery after a cache miss so subsequent
    /// lookups within the same healthy-tool fingerprint skip `generate_tool_types`
    /// per entry.
    pub(crate) async fn store_catalog_render_cache(
        &self,
        cache: crate::gateway::code_mode::CatalogRenderCache,
    ) {
        let mut guard = self.code_mode_catalog_render_cache.lock().await;
        *guard = Some(cache);
    }

    /// Return the cached catalog embedding vectors if the fingerprint still
    /// matches.
    ///
    /// Production code goes through `ensure_embeddings_for_fingerprint`
    /// (which serves warm hits itself); this read-only accessor exists for
    /// tests asserting cache state without triggering an embed.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn cached_embeddings(
        &self,
        fingerprint: &str,
    ) -> Option<Vec<(String, Vec<f32>)>> {
        let guard = self.code_mode_embedding_cache.read().await;
        guard.as_ref().and_then(|cache| {
            if cache.fingerprint == fingerprint {
                Some(cache.vectors.as_ref().clone())
            } else {
                None
            }
        })
    }

    /// Single-flight per ranking corpus. The cache lock is never held during
    /// TEI I/O, and warm hits share vectors without cloning their contents.
    pub(crate) async fn ensure_embeddings_for_fingerprint(
        &self,
        fingerprint: &str,
        entries: &[crate::gateway::code_mode::CatalogDescriptor],
    ) -> Arc<Vec<(String, Vec<f32>)>> {
        let config = self.code_mode_config().await.semantic_search;
        if !config.is_configured() || entries.is_empty() {
            return Arc::new(Vec::new());
        }
        if let Some(vectors) = self.cached_embeddings_shared(fingerprint).await {
            return vectors;
        }
        let flight = {
            let mut flights = self.code_mode_embedding_flights.lock().await;
            flights.retain(|_, value| value.strong_count() > 0);
            flights
                .get(fingerprint)
                .and_then(std::sync::Weak::upgrade)
                .unwrap_or_else(|| {
                    let flight = Arc::new(CodeModeEmbeddingFlight::default());
                    flights.insert(fingerprint.to_string(), Arc::downgrade(&flight));
                    flight
                })
        };
        let _build_guard = flight.build.lock().await;
        if let Some(vectors) = self.cached_embeddings_shared(fingerprint).await {
            return vectors;
        }
        if !self.semantic_search_available_locked().await {
            return Arc::new(Vec::new());
        }
        let ids: Vec<String> = entries.iter().map(|e| e.id.clone()).collect();
        let texts: Vec<String> = entries.iter().map(|e| e.description.clone()).collect();
        let tei_url = config
            .tei_url
            .as_deref()
            .expect("is_configured() guarantees tei_url is Some");
        match crate::gateway::code_mode::embeddings::embed_via_tei(tei_url, &texts).await {
            Ok(vectors) if vectors.len() == ids.len() => {
                self.record_semantic_search_recovery().await;
                let pairs = Arc::new(ids.into_iter().zip(vectors).collect());
                *self.code_mode_embedding_cache.write().await =
                    Some(crate::gateway::code_mode::CatalogEmbeddingCache {
                        fingerprint: fingerprint.to_string(),
                        vectors: Arc::clone(&pairs),
                    });
                pairs
            }
            Ok(_) => Arc::new(Vec::new()),
            Err(err) => {
                self.record_semantic_search_failure(&err.to_string()).await;
                Arc::new(Vec::new())
            }
        }
    }

    async fn cached_embeddings_shared(
        &self,
        fingerprint: &str,
    ) -> Option<Arc<Vec<(String, Vec<f32>)>>> {
        let guard = self.code_mode_embedding_cache.read().await;
        guard.as_ref().and_then(|cache| {
            (cache.fingerprint == fingerprint).then(|| Arc::clone(&cache.vectors))
        })
    }

    /// True when the semantic search cooldown has elapsed (or no failure has
    /// been recorded yet) — i.e. it is safe to attempt a TEI call.
    async fn semantic_search_available_locked(&self) -> bool {
        let guard = self.semantic_search_last_failure.read().await;
        match *guard {
            None => true,
            Some(last_failure) => last_failure.elapsed() >= SEMANTIC_SEARCH_COOLDOWN,
        }
    }

    /// Public cooldown check for callers that are NOT already holding the
    /// embedding-cache lock (e.g. a `semantic_rank` call that skips catalog
    /// warming entirely because the cache is already warm).
    pub(crate) async fn semantic_search_available(&self) -> bool {
        self.semantic_search_available_locked().await
    }

    /// Record a TEI failure, starting/refreshing the cooldown window. This is a
    /// recovered optional-dependency degradation, so log the healthy→failing
    /// transition at INFO; repeated failures during an active cooldown stay
    /// silent and normal CLI output is not polluted by a fallback that worked.
    pub(crate) async fn record_semantic_search_failure(&self, reason: &str) {
        let mut guard = self.semantic_search_last_failure.write().await;
        let was_healthy = guard.is_none();
        *guard = Some(Instant::now());
        drop(guard);
        if was_healthy {
            tracing::info!(
                surface = "dispatch",
                service = "code_mode",
                action = "semantic_search",
                kind = "tei_unavailable",
                reason,
                "Code Mode semantic search TEI call failed; falling back to lexical-only search until cooldown elapses"
            );
        }
    }

    /// Clear the failure cooldown after a successful TEI call. Logs
    /// `tracing::info!` only on the failing→healthy transition.
    pub(crate) async fn record_semantic_search_recovery(&self) {
        let mut guard = self.semantic_search_last_failure.write().await;
        let was_failing = guard.is_some();
        *guard = None;
        drop(guard);
        if was_failing {
            tracing::info!(
                surface = "dispatch",
                service = "code_mode",
                action = "semantic_search",
                kind = "tei_recovered",
                "Code Mode semantic search TEI call succeeded again; resuming semantic blend"
            );
        }
    }

    /// Return the cached catalog render if the fingerprint still matches.
    ///
    /// Returns `Some((entries, catalog_json, serialized_size))` on a hit,
    /// `None` on a miss (caller must rebuild and call `store_catalog_render_cache`).
    /// `entries`/`catalog_json` are `Arc`-wrapped, so a hit clones cheaply
    /// (refcount bump) regardless of how many times this is called for the
    /// same fingerprint within one execution — see `CatalogRenderCache`'s doc
    /// comment for why that matters now that `describe()` calls this per
    /// invocation, not just once at execution start.
    pub(crate) async fn cached_catalog_render(
        &self,
        fingerprint: &str,
    ) -> Option<(
        Arc<[crate::gateway::code_mode::CatalogDescriptor]>,
        Arc<str>,
        usize,
    )> {
        let guard = self.code_mode_catalog_render_cache.lock().await;
        guard.as_ref().and_then(|cache| {
            if cache.fingerprint == fingerprint {
                Some((
                    Arc::clone(&cache.entries),
                    Arc::clone(&cache.catalog_json),
                    cache.serialized_size,
                ))
            } else {
                None
            }
        })
    }

    pub(crate) async fn cached_snippet_metadata(
        &self,
        fingerprint: &str,
    ) -> Option<Vec<labby_codemode::snippet::store::SnippetInfo>> {
        let guard = self.code_mode_snippet_metadata_cache.lock().await;
        guard
            .as_ref()
            .and_then(|cache| (cache.fingerprint == fingerprint).then(|| cache.entries.clone()))
    }

    pub(crate) async fn store_snippet_metadata_cache(
        &self,
        cache: crate::gateway::code_mode::SnippetMetadataCache,
    ) {
        let mut guard = self.code_mode_snippet_metadata_cache.lock().await;
        *guard = Some(cache);
    }

    /// Fire-and-forget: spawn per-upstream connection tasks for exclusive code mode.
    ///
    /// Unlike `refresh_code_mode_indexes_if_stale` this does NOT build vector
    /// search indexes.  It only ensures each enabled upstream has its tool list
    /// in the pool so `healthy_tools()` is non-empty.
    async fn spawn_code_mode_upstream_connections(
        &self,
        pool: Arc<UpstreamPool>,
        cfg: &GatewayConfig,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
        allowed_upstreams: Option<&BTreeSet<String>>,
    ) {
        let owner = owner.cloned();
        let oauth_subject = oauth_subject.map(ToOwned::to_owned);
        let per_request_limit = crate::upstream::pool::upstream_discovery_concurrency(
            cfg.gateway.upstream_discovery_concurrency,
        );
        let mut admitted = 0;
        for upstream in cfg
            .upstream
            .iter()
            .filter(|u| u.enabled && upstream_allowed(&u.name, allowed_upstreams))
        {
            if upstream.oauth.is_some() && oauth_subject.is_none() {
                continue;
            }
            let already_warm = if upstream.oauth.is_some() {
                pool.has_cached_subject_tools_for_upstream(
                    &upstream.name,
                    oauth_subject
                        .as_deref()
                        .expect("OAuth subject checked above"),
                )
                .await
            } else {
                pool.has_healthy_tools_for_upstream(&upstream.name).await
            };
            if already_warm {
                continue;
            }
            if admitted >= per_request_limit {
                continue;
            }
            let warm_up_key = format!(
                "{:p}:{}:{}",
                Arc::as_ptr(&pool),
                upstream.name,
                oauth_subject.as_deref().unwrap_or("")
            );
            let Some(warm_up_admission) = CodeModeWarmUpAdmission::try_acquire(
                &self.code_mode_warm_up_active,
                per_request_limit,
            ) else {
                // Opportunistic warm-up can be retried by a later request.
                // Never queue a task beyond this request's lifetime.
                continue;
            };
            {
                let mut in_flight = CODE_MODE_WARM_UP_IN_FLIGHT
                    .get_or_init(|| tokio::sync::Mutex::new(BTreeSet::new()))
                    .lock()
                    .await;
                if !in_flight.insert(warm_up_key.clone()) {
                    continue;
                }
            }
            admitted += 1;
            let pool = Arc::clone(&pool);
            let upstream = upstream.clone();
            let owner = owner.clone();
            let oauth_subject = oauth_subject.clone();
            #[cfg(not(test))]
            let warm_up_timeout = code_mode_warm_up_timeout(&upstream, pool.request_timeout());
            #[cfg(test)]
            let warm_up_timeout = CODE_MODE_WARM_UP_TIMEOUT;
            #[cfg(test)]
            self.code_mode_warm_up_task_spawns
                .fetch_add(1, Ordering::Relaxed);
            tokio::spawn(async move {
                let _warm_up_admission = warm_up_admission;
                // `ensure_tools_for_upstream` skips the upstream internally
                // when it already has healthy tools.
                let subject = upstream.oauth.as_ref().and(oauth_subject.as_deref());
                match tokio::time::timeout(
                    warm_up_timeout,
                    pool.ensure_tools_for_upstream(&upstream, subject, owner.as_ref()),
                )
                .await
                {
                    Ok(Ok(_)) => tracing::debug!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "code_mode.warm_upstream",
                        upstream = %upstream.name,
                        "code_mode upstream connected"
                    ),
                    Ok(Err(err)) => tracing::warn!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "code_mode.warm_upstream",
                        upstream = %upstream.name,
                        error = %err,
                        "code_mode upstream connection failed during warm-up"
                    ),
                    Err(_) => tracing::warn!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "code_mode.warm_upstream",
                        upstream = %upstream.name,
                        timeout_seconds = warm_up_timeout.as_secs(),
                        "code_mode upstream connection timed out during warm-up"
                    ),
                }
                CODE_MODE_WARM_UP_IN_FLIGHT
                    .get_or_init(|| tokio::sync::Mutex::new(BTreeSet::new()))
                    .lock()
                    .await
                    .remove(&warm_up_key);
            });
        }
    }
}

/// Report upstreams skipped because a recent probe failure is still suppressed.
///
/// Separate from the budget-exhaustion warning: those upstreams may be perfectly
/// healthy and merely slow, while these are known to have failed.
fn warn_suppressed(suppressed: &[String]) {
    tracing::info!(
        surface = "dispatch",
        service = "gateway",
        action = "code_mode.catalog_cache",
        suppressed_upstreams = ?suppressed,
        "omitting upstreams from codemode proxy while their recent probe failures are suppressed"
    );
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
mod catalog_merge_tests {
    use super::*;

    fn tool(upstream: &str, name: &str) -> UpstreamTool {
        UpstreamTool {
            tool: rmcp::model::Tool::new(name.to_string(), "", Arc::new(serde_json::Map::new())),
            input_schema: None,
            output_schema: None,
            upstream_name: Arc::from(upstream),
            destructive: false,
        }
    }

    #[test]
    fn combined_catalog_keeps_same_named_tools_from_distinct_upstreams() {
        let merged = merge_visible_catalog_tools(
            vec![tool("public", "search")],
            vec![tool("private", "search")],
        );
        let identities = merged
            .iter()
            .map(|tool| (tool.upstream_name.as_ref(), tool.tool.name.as_ref()))
            .collect::<Vec<_>>();
        assert_eq!(
            identities,
            vec![("private", "search"), ("public", "search")]
        );
    }

    #[test]
    fn combined_catalog_caps_after_deterministic_cross_scope_merge() {
        let global = (0..crate::upstream::pool::MAX_UPSTREAM_TOOLS)
            .map(|index| tool("z-global", &format!("tool_{index:04}")))
            .collect();
        let merged = merge_visible_catalog_tools(global, vec![tool("a-subject", "private")]);

        assert_eq!(merged.len(), crate::upstream::pool::MAX_UPSTREAM_TOOLS);
        assert_eq!(merged[0].upstream_name.as_ref(), "a-subject");
        assert!(
            merged
                .iter()
                .any(|tool| tool.tool.name.as_ref() == "private")
        );
    }
}
