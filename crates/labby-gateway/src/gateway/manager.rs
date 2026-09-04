//! `GatewayManager` — the shared orchestration object behind every gateway
//! surface (CLI, MCP, HTTP).
//!
//! This file owns the struct definition and its fields; all method bodies live
//! in the `manager/` child modules as additional `impl GatewayManager` blocks:
//!
//! | Module | Responsibilities |
//! |--------|-----------------|
//! | `core` | `new()`, `with_*` builders, `from_config` factory, accessors |
//! | `config_ops` | upstream add/update/remove, service env config, code-mode config |
//! | `pool_lifecycle` | reload + swap-and-drain, `GatewayCatalogSnapshot`/`diff_catalogs` |
//! | `code_mode_runtime` | catalog refresh, render cache, runtime readiness |
//! | `code_mode_resolve` | `resolve_*_tool`, `ToolExecuteSelector` |
//! | `persist` | env-file path + bearer-token persistence |
//! | `imports` | discovery import orchestration + tombstones |
//! | `import_matchers` | pure import/tombstone matching helpers |
//! | `virtual_servers` | virtual-server CRUD + quarantine restore |
//! | `protected_routes` | protected MCP route CRUD + live resolver |
//! | `oauth_resources` | upstream OAuth manager/cache reconciliation |
//! | `views` | `list`/`get`/`status`/`test` and discovery inspection views |

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use arc_swap::ArcSwap;
use tokio::sync::{Mutex, RwLock};
use tokio::time::Instant;

use labby_auth::upstream::cache::OauthClientCache;
use labby_auth::upstream::encryption::EncryptionKey;
use labby_auth::upstream::manager::UpstreamOauthManager;
use labby_runtime::CodeModeAppState;
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::GatewayConfig;

use crate::upstream::pool::{HeaderRecoveryMetricsStore, InProcessConnector};

use super::code_mode::{CodeModeHistory, CodeModeSourceStore};
use super::config_store::GatewayConfigStore;
use super::protected_routes::ProtectedRouteIndex;
pub use super::runtime::GatewayRuntimeHandle;
use super::service_registry::PublishedServiceRegistryState;
use super::types::CatalogChangeNotifier;

#[derive(Clone)]
pub(super) struct OauthStatusDiscoverySnapshot {
    pub(super) completed_at: Instant,
    pub(super) summary: Option<crate::upstream::pool::UpstreamCachedSummary>,
    pub(super) tool_error: Option<String>,
    pub(super) error: Option<String>,
}

mod code_mode_discovery;
mod code_mode_resolve;
mod code_mode_runtime;
mod config_ops;
mod config_transaction;
mod core;
mod enrichment;
mod import_matchers;
mod imports;
mod loadouts;
mod oauth_resources;
mod persist;
mod pool_lifecycle;
mod protected_routes;
mod publication;
#[cfg(test)]
mod tests;
mod usage;
mod views;
mod virtual_servers;

// `BatchAddOutcome`, `GatewayCatalogSnapshot`, and `diff_catalogs` keep the
// monolith's public `manager::` paths; they currently have no callers outside
// the manager tree, so the re-exports are allowed to be unused in non-test
// builds (the test suite imports them through these paths).
pub use self::code_mode_resolve::CallbackToolLookup;
#[allow(unused_imports)]
pub use self::config_ops::BatchAddOutcome;
pub use self::core::{GatewayManagerConfig, GatewayOauthConfig};
pub use self::core::{
    PublishedPromptCallError, PublishedResourceReadError, PublishedToolCallError,
};
pub use self::import_matchers::ImportTombstoneSelector;
pub(crate) use self::import_matchers::{discovered_is_tombstoned, partition_discovered_for_import};
#[allow(unused_imports)]
pub use self::pool_lifecycle::{GatewayCatalogSnapshot, GatewayReloadOutcome, diff_catalogs};
pub use self::publication::{
    BootstrapPolicyLeaseError, LoadoutMcpCatalogPublicationError,
    LoadoutPromptCatalogPublicationError, LoadoutResourceCatalogPublicationError,
    LoadoutResourceTemplateCatalogPublicationError, LoadoutServiceCatalogPublicationError,
    LoadoutToolCatalogPublicationError, ProjectRoutePublicationError,
    PublishedBootstrapPolicyLease, PublishedLoadoutMcpCatalogSnapshot,
    PublishedLoadoutPromptCatalogSnapshot, PublishedLoadoutResourceCatalogSnapshot,
    PublishedLoadoutResourceTemplateCatalogSnapshot, PublishedLoadoutService,
    PublishedLoadoutServiceCatalogSnapshot, PublishedLoadoutToolCatalogSnapshot,
    PublishedProjectRouteSnapshot,
};
pub use self::publication::{GatewayRuntimeConfigGeneration, PublishedRuntimeLoadoutSnapshot};
pub use super::service_registry::{
    PublishedServiceRegistrySnapshot, ServiceRegistryPublicationError,
    ServiceRegistryPublicationGeneration,
};
pub use crate::gateway::runtime::PoolPublicationGeneration;

#[derive(Clone)]
pub struct GatewayManager {
    pub(super) path: PathBuf,
    /// Host-owned persistence + environment seam.
    ///
    /// Owns `config.toml` rendering (with foreign-key preservation), the `.env`
    /// credential file helpers, the process-wide Code Mode flag, and public-URL
    /// resolution — all of which depend on the host's full `LabConfig` and are
    /// shared with non-gateway Labby code, so they cannot live in `labby-gateway`.
    pub(super) store: Arc<dyn GatewayConfigStore>,
    pub(super) runtime: GatewayRuntimeHandle,
    pub(super) config: Arc<RwLock<GatewayConfig>>,
    /// Serializes the short publication window spanning the live pool,
    /// config snapshot, protected-route index, and Code Mode flags. Readers
    /// that combine those components take a read lease and clone a coherent
    /// revision before doing slow I/O.
    pub(super) publication_barrier: Arc<RwLock<()>>,
    /// Opaque process-local identity for the currently published runtime
    /// configuration revision. Advanced under `publication_barrier` for every
    /// live config publication, including rollback and ABA.
    pub(super) runtime_config_generation: Arc<AtomicU64>,
    pub(super) config_mutation: Arc<Mutex<()>>,
    /// Scope-keyed single-flight and terminal-failure state for full-fleet MCP discovery.
    pub(super) mcp_catalog_refresh_inflight: Arc<Mutex<std::collections::HashSet<String>>>,
    pub(super) mcp_catalog_refresh_failures: Arc<Mutex<std::collections::HashSet<String>>>,
    pub(super) code_mode_app_state: CodeModeAppState,
    lazy_pool_init: Arc<Mutex<()>>,
    notifier: Option<CatalogChangeNotifier>,
    pub(super) oauth_client_cache: Option<OauthClientCache>,
    pub(super) upstream_oauth_managers: Option<Arc<dashmap::DashMap<String, UpstreamOauthManager>>>,
    pub(super) oauth_status_discovery_cache:
        Arc<Mutex<std::collections::HashMap<(String, String), OauthStatusDiscoverySnapshot>>>,
    pub(super) oauth_status_discovery_locks:
        Arc<dashmap::DashMap<(String, String), Arc<Mutex<()>>>>,
    builtin_service_registry: Arc<ArcSwap<PublishedServiceRegistryState>>,
    /// Serializes synchronous registry projection and publication so concurrent
    /// setters cannot install generations out of allocation order.
    builtin_service_registry_publication: Arc<std::sync::Mutex<()>>,
    pub(super) oauth_sqlite: Option<labby_auth::sqlite::SqliteStore>,
    pub(super) oauth_key: Option<EncryptionKey>,
    pub(super) oauth_redirect_uri: Option<Arc<String>>,
    pub(super) resource_registry: Option<labby_auth::resource_registry::ResourceRegistry>,
    pub(super) usage_store: Option<Arc<crate::usage::UsageStore>>,
    /// Process-lifetime SEP-2243 recovery counters shared by every pool
    /// generation built by this manager.
    pub(super) header_recovery_metrics_store: HeaderRecoveryMetricsStore,
    /// Durable append-only journal for `codemode.step` boundaries. `None`
    /// disables journaling (pure no-op path). Owned as an `Arc` so every `Clone`
    /// of the manager shares one store.
    pub(super) step_journal: Option<Arc<crate::codemode_journal::StepJournalStore>>,
    /// Per-execution in-memory buffers of journal rows, keyed by `execution_id`.
    /// `record_step` pushes here (nanoseconds, no I/O); the single bulk flush at
    /// the run boundary drains one execution's buffer. Keyed per execution — not
    /// a single shared "current execution" scalar — so concurrent runs never
    /// cross-contaminate.
    pub(super) step_buffers: Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, Vec<crate::codemode_journal::StepJournalRow>>,
        >,
    >,
    protected_route_index: Arc<RwLock<ProtectedRouteIndex>>,
    code_mode_history: Arc<Mutex<CodeModeHistory>>,
    code_mode_source_store: Arc<Mutex<CodeModeSourceStore>>,
    /// Optional connector for in-process (built-in) service peers.
    /// Propagated to each pool the manager creates so built-in services are
    /// reachable without an external HTTP/stdio connection.
    in_process_connector: Option<InProcessConnector>,
    /// Wall-clock TTL guard for `refresh_code_mode_catalog`. Tracks the
    /// last time a full reprobe completed; back-to-back calls within the
    /// freshness window skip the reprobe and return immediately.
    pub(super) code_mode_refresh_deadline: Arc<Mutex<Option<Instant>>>,
    /// Single-flight guard: only one concurrent `refresh_code_mode_catalog`
    /// runs at a time. Subsequent callers that arrive while a refresh is in
    /// progress wait for it to finish rather than spawning a second reprobe.
    pub(super) code_mode_refresh_inflight: Arc<Mutex<()>>,
    /// Cached rendered Code Mode discovery catalog, keyed by a fingerprint of
    /// the live healthy tool list. Avoids regenerating `ToolDescriptor`
    /// structs (including TS `.signature`/`.dts` via `generate_tool_types`),
    /// the serialized JSON blob, and the JS proxy string on every lookup when
    /// the upstream catalog has not changed between calls.
    pub(super) code_mode_catalog_render_cache:
        Arc<Mutex<Option<crate::gateway::code_mode::CatalogRenderCache>>>,
    /// Weak keyed build flights prevent duplicate cold render construction.
    /// Weak values disappear after callers finish, bounding retained state.
    pub(super) code_mode_catalog_render_flights: Arc<
        Mutex<
            std::collections::HashMap<
                String,
                std::sync::Weak<crate::gateway::code_mode::CatalogRenderFlight>,
            >,
        >,
    >,
    /// Cached Code Mode catalog embedding vectors, keyed separately by the
    /// visible `(id, description)` ranking corpus. `RwLock` (not
    /// `Mutex`), matching the `config: Arc<RwLock<GatewayConfig>>` precedent
    /// above — this is a read-heavy cache; writes only happen on a
    /// ranking-corpus change or the very first embed. Safety/schema-only
    /// render changes therefore do not force an identical TEI batch.
    ///
    /// `ensure_embeddings_for_fingerprint` holds the write lock across the
    /// full check-then-embed-then-store sequence (not just the store) as a
    /// single-flight guard: concurrent calls against the same cold
    /// fingerprint serialize onto one TEI batch call instead of firing N
    /// redundant ones.
    pub(super) code_mode_embedding_cache:
        Arc<RwLock<Option<crate::gateway::code_mode::CatalogEmbeddingCache>>>,
    /// Fail-open cooldown gate for the TEI semantic-search embedding
    /// service. `Some(instant)` = a call failed at `instant`; calls made
    /// before `instant + 30s` skip TEI entirely (falling back to
    /// lexical-only) rather than retrying a known-down service on every
    /// search. `None` = healthy (or never tried).
    pub(super) semantic_search_last_failure: Arc<RwLock<Option<Instant>>>,
    /// Cached snippet metadata for Code Mode discovery. Snippet executable
    /// source is never stored here; `codemode.run()` resolves source lazily.
    pub(super) code_mode_snippet_metadata_cache:
        Arc<Mutex<Option<crate::gateway::code_mode::SnippetMetadataCache>>>,
    /// Shared, long-lived warm-runner pool for Code Mode (Perf H1). Pools the
    /// runner OS process across executions (fresh `javy::Runtime` per run) to
    /// amortize fork/startup. Wrapped in `Arc` so the `Clone` manager shares one
    /// pool; configured from the environment at construction (kill switch:
    /// `LABBY_CODE_MODE_POOL_SIZE=0` → spawn-per-execution fallback).
    pub(super) code_mode_runner_pool: Arc<crate::gateway::code_mode::RunnerPool>,
    /// Loaded OpenAPI specs for the Code Mode `openapi` local provider. Built at
    /// `labby serve` startup (`with_openapi`); an empty registry means no specs
    /// are configured/loaded (the shim is then never emitted). Cheap `Arc` clone.
    pub(super) openapi_registry: labby_openapi::OpenApiRegistry,
    /// Hardened `reqwest` client for `openapi` dispatch. Cheap `Arc` clone.
    pub(super) openapi_http_client: reqwest::Client,
    /// Optional private Unraid Core provider. It augments Code Mode only and
    /// is never registered as an MCP upstream.
    pub(super) core_provider_client: Option<crate::core_provider::CoreProviderClient>,
    /// Live inbound MCP client/session registry, populated by `labby`'s MCP
    /// transport layer (`rmcp`-dependent, cannot live in this crate) and read
    /// by `gateway.clients.list`. `Default` (empty, no-op) when not wired —
    /// see `with_client_registry`.
    pub(super) client_registry: labby_runtime::client_registry::ClientRegistryHandle,
}

pub(crate) struct ConfigMutationGuard {
    _local: tokio::sync::OwnedMutexGuard<()>,
    release: Option<std::sync::mpsc::Sender<()>>,
}

impl Drop for ConfigMutationGuard {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

impl GatewayManager {
    /// Persist a gateway config without replacing the live in-memory snapshot.
    ///
    /// Reconcile paths use this when they need the current snapshot to remain the
    /// "before" side of catalog and upstream-diff calculations until reload has
    /// successfully installed the newly persisted config.
    pub(super) async fn write_config_file(&self, cfg: &GatewayConfig) -> Result<(), ToolError> {
        tracing::info!(
            action = "gateway.config.write",
            phase = "start",
            upstream_count = cfg.upstream.len(),
            virtual_server_count = cfg.virtual_servers.len(),
            "gateway reconcile"
        );
        // Persistence (TOML render with foreign-key preservation + atomic write)
        // is owned by the host through the `GatewayConfigStore` seam, reusing the
        // existing `write_gateway_config`/`render_gateway_config` toml_edit logic
        // verbatim.
        let store = Arc::clone(&self.store);
        let cfg_for_persist = cfg.clone();
        tokio::task::spawn_blocking(move || store.persist(&cfg_for_persist))
            .await
            .map_err(|err| {
                ToolError::internal_message(format!("gateway config write task failed: {err}"))
            })??;
        tracing::info!(
            action = "gateway.config.write",
            phase = "finish",
            "gateway reconcile"
        );
        Ok(())
    }

    pub(super) async fn persist_config(&self, cfg: GatewayConfig) -> Result<(), ToolError> {
        let runtime_cfg = {
            let current = self.config.read().await;
            config_transaction::runtime_config_for_desired(&current, &cfg)
        };
        self.write_config_file(&cfg).await?;
        let _publication = self.publication_barrier.write().await;
        self.store
            .set_process_code_mode_enabled(runtime_cfg.code_mode.enabled);
        self.code_mode_app_state
            .set_enabled(runtime_cfg.code_mode.mcp_ui_enabled);
        *self.protected_route_index.write().await =
            ProtectedRouteIndex::from_routes(&runtime_cfg.protected_mcp_routes);
        *self.config.write().await = runtime_cfg;
        self.advance_runtime_config_generation();
        Ok(())
    }
}
