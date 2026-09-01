//! `UpstreamPool` — manages connections to upstream MCP servers.
//!
//! Connects to configured upstreams via HTTP (`StreamableHttpClientTransport`)
//! or stdio (child process), discovers their tools, and caches schemas.

use std::collections::{BTreeSet, HashMap};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use dashmap::DashMap;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use futures::future::BoxFuture;
use rmcp::RoleClient;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use labby_auth::upstream::cache::OauthClientCache;
#[cfg(test)]
use labby_runtime::gateway_config::UpstreamConfig;

use crate::registry::InProcessService;

use super::types::{UpstreamRuntimeMetadata, UpstreamRuntimeOwner};

#[cfg(test)]
// `panic!` is how tests assert; `panic = "warn"` targets production paths.
#[allow(clippy::panic)]
mod annotation_passthrough_tests;
mod cache_repair;
mod capability;
mod capability_call;
mod catalog_pagination;
mod catalog_publication;
mod checked_call;
mod completion;
mod connect;
mod connect_stdio;
#[cfg(test)]
mod connect_tests;
#[cfg(all(test, unix))]
mod connect_unix_tests;
mod connection;
mod discover;
mod ensure;
pub mod entries;
mod health;
mod helpers;
mod http_cancellation;
mod incarnation;
mod legacy_client;
mod lifecycle;
mod lifecycle_compat;
mod logging;
mod notifications;
#[cfg(test)]
mod notifications_tests;
mod oauth_invalidation;
#[cfg(test)]
mod pooled_cancellation_tests;
mod probe;
mod prompts_exposure;
#[cfg(test)]
mod prompts_exposure_tests;
mod prompts_get;
mod prompts_list;
mod registration;
mod relay;
mod relay_cache;
mod relay_cancellation;
#[cfg(test)]
mod relay_cancellation_tests;
#[cfg(test)]
mod resources_exposure_tests;
mod resources_list;
mod resources_read;
mod skills;
mod skills_exposure;
#[cfg(all(test, feature = "skills"))]
pub(crate) use skills::OperatorSkillRejection;
#[cfg(feature = "skills")]
pub(crate) use skills::OperatorSkills;
mod skills_cache;
mod skills_list;
#[cfg(feature = "skills")]
mod skills_provider;
#[cfg(feature = "skills")]
pub use skills_provider::SepSkillProvider;
mod skills_tests;
mod spawn_lock;
mod stdio_stderr;
mod stdio_transport;
mod subscription_schedule;
mod task_route;
mod tasks;
#[cfg(any(test, feature = "testkit"))]
mod testsupport;
mod tool_call_cancel;
mod tools;
mod tools_call;
mod tools_call_exact;
#[cfg(test)]
// `panic!` is how tests assert; `panic = "warn"` targets production paths.
#[allow(clippy::panic)]
mod tools_exposure_tests;
mod usage_record;
mod validate;

pub use capability_call::CapabilityCallError;
pub(crate) use checked_call::CheckedToolCallError;
pub(crate) use connect_stdio::connect_direct_stdio;
use helpers::{DEFAULT_RELAY_TIMEOUT, DEFAULT_REQUEST_TIMEOUT};
pub use helpers::{
    UpstreamCachedSummary, in_process_upstream_name, redact_resource_uri_for_logging,
    upstream_destructive_from_annotations, upstream_discovery_concurrency,
};
pub(crate) use helpers::{
    install_max_response_bytes_default, install_upstream_discovery_concurrency_default,
};
pub use notifications::UpstreamNotificationEvent;
pub use oauth_invalidation::OAuthSessionInvalidation;
pub(crate) use prompts_get::ExactPromptCallError;
pub use prompts_list::ListedUpstreamPrompt;
pub use resources_list::{ListedUpstreamResource, ListedUpstreamResourceTemplate};
pub(crate) use resources_read::ExactResourceReadError;
pub(crate) use stdio_stderr::install_upstream_stderr_level_default;
pub use task_route::TaskRouteAuthorization;
pub use tools::{
    MAX_UPSTREAM_TOOLS, tool_is_mcp_app_host_visible_for_config,
    upstream_has_mcp_app_ui_owner_for_config,
};
// Catalog size caps are used by pool child modules directly via `super::tools::*`.
// No external consumer references them through this path, so no `pub use` needed.

/// A cached subject-scoped connection entry.  Holds the live peer plus the
/// tool list that was discovered when the connection was opened.  Protected
/// by the `subject_connect_locks` single-flight gate so only one connect
/// runs per `(upstream, subject)` key at a time.
///
/// See `connection.rs:acquire_or_connect_subject` for the full cache logic
/// (P-C1 fix).
pub(super) struct SubjectScopedConnection {
    /// The full upstream connection (keeps the running service + server task alive).
    pub(super) _connection: UpstreamConnection,
    /// Cloned peer handle — pre-cloned so `acquire_or_connect_subject` can
    /// return it on the cache-hit fast path without re-cloning under write lock.
    pub(super) peer: rmcp::service::Peer<RoleClient>,
    /// Tool list discovered at connect time (avoids a round-trip on
    /// every owner-lookup call).
    pub(super) tools: Vec<rmcp::model::Tool>,
    /// Wall-clock instant when this entry was last used.
    pub(super) last_used: Instant,
}

/// Cumulative SEP-2243 recovery counters for one upstream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct HeaderRecoveryMetrics {
    pub(crate) mismatch_detected: u64,
    pub(crate) schema_refreshes: u64,
    pub(crate) retry_successes: u64,
    pub(crate) retry_failures: u64,
}

#[derive(Debug, Default)]
struct HeaderRecoveryCounters {
    mismatch_detected: AtomicU64,
    schema_refreshes: AtomicU64,
    retry_successes: AtomicU64,
    retry_failures: AtomicU64,
}

impl HeaderRecoveryCounters {
    fn snapshot(&self) -> HeaderRecoveryMetrics {
        HeaderRecoveryMetrics {
            mismatch_detected: self.mismatch_detected.load(Ordering::Relaxed),
            schema_refreshes: self.schema_refreshes.load(Ordering::Relaxed),
            retry_successes: self.retry_successes.load(Ordering::Relaxed),
            retry_failures: self.retry_failures.load(Ordering::Relaxed),
        }
    }
}

/// Shared process-lifetime store for per-upstream SEP-2243 recovery counters.
///
/// `GatewayManager` owns one clone and injects it into every replacement pool so
/// gateway reloads cannot erase recovery history. Standalone/test pools get an
/// independent store by default.
#[derive(Debug, Clone, Default)]
pub(crate) struct HeaderRecoveryMetricsStore {
    counters: Arc<DashMap<String, Arc<HeaderRecoveryCounters>>>,
}

impl HeaderRecoveryMetricsStore {
    fn increment_saturating(counter: &AtomicU64) -> u64 {
        match counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            Some(value.saturating_add(1))
        }) {
            Ok(previous) | Err(previous) => previous.saturating_add(1),
        }
    }

    fn counters_for(&self, upstream_name: &str) -> Arc<HeaderRecoveryCounters> {
        Arc::clone(
            self.counters
                .entry(upstream_name.to_string())
                .or_insert_with(|| Arc::new(HeaderRecoveryCounters::default()))
                .value(),
        )
    }

    fn record_mismatch(&self, upstream_name: &str) -> u64 {
        Self::increment_saturating(&self.counters_for(upstream_name).mismatch_detected)
    }

    fn record_refresh(&self, upstream_name: &str) -> u64 {
        Self::increment_saturating(&self.counters_for(upstream_name).schema_refreshes)
    }

    fn record_retry_success(&self, upstream_name: &str) -> u64 {
        Self::increment_saturating(&self.counters_for(upstream_name).retry_successes)
    }

    fn record_retry_failure(&self, upstream_name: &str) -> u64 {
        Self::increment_saturating(&self.counters_for(upstream_name).retry_failures)
    }

    #[must_use]
    fn snapshot(&self, upstream_name: &str) -> HeaderRecoveryMetrics {
        self.counters
            .get(upstream_name)
            .map_or_else(HeaderRecoveryMetrics::default, |entry| entry.snapshot())
    }

    #[cfg(test)]
    pub(crate) fn record_mismatch_for_test(&self, upstream_name: &str) -> u64 {
        self.record_mismatch(upstream_name)
    }
}

/// Upstream connection pool — holds live connections and discovered tool catalogs.
#[derive(Clone)]
pub struct UpstreamPool {
    /// Immutable process-local identity for this published pool generation.
    revision: u64,
    /// Keeps a checked invocation on one live pool while reload drains wait.
    invocation_barrier: Arc<RwLock<()>>,
    /// Discovered upstream state, keyed by upstream name.
    catalog: Arc<RwLock<catalog_publication::CatalogState>>,
    /// Live client connections, keyed by upstream name.
    /// Each is an `Arc<Peer<RoleClient>>` that can `call_tool` / `list_tools`.
    connections: Arc<RwLock<HashMap<String, UpstreamConnection>>>,
    /// Linearizes generic connection/catalog-entry identity publication.
    connection_catalog_binding: Arc<Mutex<()>>,
    /// OAuth subject provenance for entries in the generic connection map.
    /// Absence means the generic peer was connected without upstream OAuth.
    generic_oauth_subjects: Arc<RwLock<HashMap<String, String>>>,
    /// Names of upstreams that have `proxy_resources=true`.
    resource_upstreams: Arc<RwLock<Vec<String>>>,
    /// Normalized notification events produced by upstream subscription streams.
    notification_tx: tokio::sync::broadcast::Sender<UpstreamNotificationEvent>,
    /// Cancellation tokens for one active subscriptions/listen stream per upstream.
    subscription_tasks: Arc<RwLock<HashMap<String, Arc<CancellationToken>>>>,
    /// Upstreams already queued for a background subscription reconcile.
    subscription_refresh_pending: Arc<Mutex<BTreeSet<String>>>,
    /// Cancels queued/in-flight subscription reconcile batches during pool drain.
    subscription_reconcile_cancel: CancellationToken,
    /// Native resource URIs acknowledged by each upstream subscription.
    subscription_resources: Arc<RwLock<HashMap<String, BTreeSet<String>>>>,
    /// Lock-free gateway-facing snapshot used by synchronous subscription negotiation.
    subscribable_resource_uris: Arc<ArcSwap<BTreeSet<String>>>,
    /// Per-upstream OAuth managers, keyed by upstream name.
    /// `None` when the server was started without OAuth support.
    oauth_client_cache: Option<OauthClientCache>,
    /// Shared with `OauthClientCache`: readers cover the entire authenticated
    /// connect-and-publish path; credential mutation takes the sole writer.
    oauth_invalidation_barrier: Arc<RwLock<()>>,
    /// Background reprobe task cancellation tokens, keyed by upstream name.
    probe_tasks: Arc<RwLock<HashMap<String, CancellationToken>>>,
    /// Shared fleet-wide gate for periodic reprobes. Per-upstream tasks retain
    /// independent schedules, but only a bounded number may probe concurrently.
    reprobe_semaphore: Arc<tokio::sync::Semaphore>,
    /// Shared gate for cold subject-scoped catalog fan-outs. Unlike the
    /// per-upstream call bulkheads, this bounds simultaneous OAuth connection
    /// acquisition across the entire fleet and across concurrent callers.
    catalog_fanout_semaphore: Arc<tokio::sync::Semaphore>,
    /// Per-upstream RPC bulkheads. Each upstream receives an independent
    /// semaphore so one slow or reconnecting peer cannot absorb unbounded
    /// concurrent retries.
    call_semaphores: Arc<RwLock<HashMap<String, Arc<tokio::sync::Semaphore>>>>,
    /// Permit count used when lazily creating a per-upstream bulkhead.
    call_concurrency: usize,
    /// Per-upstream lazy connection gates to prevent duplicate cold starts.
    lazy_connect_locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
    /// Cached upstream skill catalogs, keyed by `(upstream, subject)`.
    ///
    /// Sharded per authorization context unconditionally — see
    /// `skills_cache.rs` for why a declared `cacheScope` never widens this.
    skills_cache: Arc<RwLock<HashMap<skills_cache::SkillsCacheKey, skills_cache::CachedSkills>>>,
    /// Per-key single-flight guards for skill-catalog fetches.
    skills_fetch_locks: Arc<skills_cache::SkillsFetchLocks>,
    /// Per-`(upstream, subject)` cached connections for the OAuth / subject-scoped
    /// proxy path.  Reused across calls for the same subject so we pay TLS +
    /// `initialize` + `tools/list` only once per idle-TTL window (P-C1 fix).
    ///
    /// Keyed by `(upstream_name, subject)`.
    subject_connections: Arc<RwLock<HashMap<(String, String), SubjectScopedConnection>>>,
    /// Per-`(upstream, subject)` single-flight locks so concurrent first-requests
    /// for the same key do not open duplicate OAuth connections (mirrors the
    /// `lazy_connect_locks` gate used by the normal pool path).
    subject_connect_locks: Arc<RwLock<HashMap<(String, String), Arc<Mutex<()>>>>>,
    /// Per-`(upstream, downstream-session, oauth-subject)` cached **relay**
    /// connections.
    ///
    /// Distinct from `subject_connections` because the cached connection is
    /// served with a `RelayClientHandler` bound to one specific downstream
    /// agent peer (`UpstreamConnection<RelayClientHandler>`, a different type).
    /// The session component of the key guarantees a cached relay connection is
    /// never reused across agents; the `Option<String>` subject component
    /// guarantees it is never reused across OAuth identities within a session,
    /// so a connection authenticated as subject A can never serve a call made
    /// as subject B (`None` = the non-OAuth/raw proxy path). The capability
    /// fingerprint is also part of the key so a newly negotiated snapshot
    /// cannot drop a connection still serving an older in-flight request.
    /// See `pool/relay.rs`.
    relay_connections:
        Arc<RwLock<HashMap<relay_cache::RelayCacheKey, relay_cache::RelayCachedConnection>>>,
    /// Single-flight locks for the relay-connection cache, mirroring
    /// `subject_connect_locks`. Keyed identically to `relay_connections`.
    relay_connect_locks: Arc<RwLock<HashMap<relay_cache::RelayCacheKey, Arc<Mutex<()>>>>>,
    /// Gateway-owned task handles and the relay connections that created them.
    /// Shared across stateless HTTP requests through the pool.
    task_routes: Arc<RwLock<HashMap<String, tasks::TaskRoute>>>,
    /// Cancellation token for the background subject-connection sweep task.
    /// `None` until the first subject-scoped connect arms it; cancelled and
    /// cleared on `drain_for_swap` (P-H2). Mirrors the `probe_tasks` lifecycle.
    subject_sweep_task: Arc<RwLock<Option<CancellationToken>>>,
    /// Request/session identity stamped onto spawned stdio upstreams.
    runtime_origin: Option<String>,
    /// Structured owner metadata stamped onto spawned stdio upstreams.
    runtime_owner: Option<UpstreamRuntimeOwner>,
    /// Maximum time to wait for an upstream tool/resource/prompt response.
    request_timeout: Duration,
    /// Maximum time to wait for one *relayed* upstream tool call. Longer than
    /// `request_timeout` because a relayed call blocks on a human answering an
    /// elicitation forwarded from the upstream — see `pool/relay.rs`.
    relay_timeout: Duration,
    /// Optional connector for in-process (built-in) service peers.
    /// When set, built-in lab services are reachable via the upstream pool.
    in_process_connector: Option<InProcessConnector>,
    /// Single-flight guard + last-attempt timestamp for
    /// `ensure_in_process_service_peers`: concurrent catalog builds serialize
    /// here (only one registers; the rest re-check and find entries present),
    /// and a persistently failing peer is retried at most once per cooldown
    /// window instead of on every catalog build. `Arc`-shared so pool clones
    /// serialize on the same guard.
    in_process_ensure_state: Arc<Mutex<Option<Instant>>>,
    /// Shared `reqwest::Client` used for ALL non-OAuth HTTP upstream connections.
    ///
    /// `reqwest::Client` is internally `Arc`-wrapped and holds a connection pool:
    /// sharing it means TLS sessions and keep-alive connections are reused across
    /// upstreams rather than rebuilt on every `connect_http_upstream` call (P-M10).
    pub(super) shared_http_client: Arc<reqwest::Client>,
    /// Optional call-usage recorder. `None` (the default) disables telemetry
    /// capture entirely — most tests and any pool built without an explicit
    /// `.with_usage_store(...)` call never touch SQLite.
    pub(super) usage_store: Option<Arc<crate::usage::UsageStore>>,
    /// Shared per-upstream SEP-2243 recovery metrics. Gateway-managed pools
    /// inherit one process-lifetime store across pool replacement.
    header_recovery_metrics_store: HeaderRecoveryMetricsStore,
}

/// Type-erased-over-lifecycle running client service.
///
/// The handler type remains stable for pool and relay caches, while a legacy
/// fallback can wrap it to override the initialize protocol version without
/// discarding server-to-client callbacks.
pub(super) enum UpstreamClientService<H>
where
    H: rmcp::ClientHandler,
{
    Direct(rmcp::service::RunningService<RoleClient, H>),
    Versioned(rmcp::service::RunningService<RoleClient, legacy_client::VersionedClientHandler<H>>),
}

impl<H> From<rmcp::service::RunningService<RoleClient, H>> for UpstreamClientService<H>
where
    H: rmcp::ClientHandler,
{
    fn from(service: rmcp::service::RunningService<RoleClient, H>) -> Self {
        Self::Direct(service)
    }
}

impl<H> UpstreamClientService<H>
where
    H: rmcp::ClientHandler,
{
    pub(super) fn peer(&self) -> &rmcp::service::Peer<RoleClient> {
        match self {
            Self::Direct(service) => service.peer(),
            Self::Versioned(service) => service.peer(),
        }
    }

    pub(super) fn service(&self) -> &H {
        match self {
            Self::Direct(service) => service.service(),
            Self::Versioned(service) => service.service().inner(),
        }
    }

    pub(super) async fn close_with_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<rmcp::service::QuitReason>, tokio::task::JoinError> {
        match self {
            Self::Direct(service) => service.close_with_timeout(timeout).await,
            Self::Versioned(service) => service.close_with_timeout(timeout).await,
        }
    }
}

/// A live connection to an upstream MCP server.
///
/// Generic over the client handler `H` (default `()`). Almost every connection
/// uses the unit handler `()` — which declines server→client requests — and is
/// stored in the pool maps as `UpstreamConnection<()>`. The relay path
/// (`pool/relay.rs`) constructs an `UpstreamConnection<RelayClientHandler>` for
/// a dedicated, ephemeral connection that forwards elicitation/sampling/roots to
/// the downstream agent. Only the `serve()` handler differs; every field below
/// (peer ops, process reaping, shutdown) is handler-agnostic.
pub struct UpstreamConnection<H = ()>
where
    H: rmcp::ClientHandler,
{
    /// The running client service handle — kept alive to maintain the connection.
    _client_service: UpstreamClientService<H>,
    /// Background task holding an in-process server alive when applicable.
    _server_task: Option<tokio::task::JoinHandle<()>>,
    /// The peer handle for making requests.
    pub(crate) peer: rmcp::service::Peer<RoleClient>,
    /// Runtime metadata for process-backed upstreams.
    pub(crate) runtime: UpstreamRuntimeMetadata,
    incarnation: Option<incarnation::ConnectionIncarnation>,
}

impl<H> UpstreamConnection<H>
where
    H: rmcp::ClientHandler,
{
    /// Build a live upstream connection from its rmcp handles.
    ///
    /// The running service and optional server task are intentionally retained
    /// as private keepalive fields for the connection's lifetime.
    #[must_use]
    pub fn new(
        client_service: rmcp::service::RunningService<RoleClient, H>,
        server_task: Option<tokio::task::JoinHandle<()>>,
        peer: rmcp::service::Peer<RoleClient>,
        runtime: UpstreamRuntimeMetadata,
    ) -> Self {
        Self::new_with_client_service(client_service.into(), server_task, peer, runtime)
    }

    #[must_use]
    pub(super) fn new_with_client_service(
        client_service: UpstreamClientService<H>,
        server_task: Option<tokio::task::JoinHandle<()>>,
        peer: rmcp::service::Peer<RoleClient>,
        runtime: UpstreamRuntimeMetadata,
    ) -> Self {
        Self {
            _client_service: client_service,
            _server_task: server_task,
            peer,
            runtime,
            incarnation: None,
        }
    }
}

static NEXT_POOL_REVISION: AtomicU64 = AtomicU64::new(1);

pub struct InProcessRegistration {
    pub connection: Option<UpstreamConnection>,
    pub tools: Vec<rmcp::model::Tool>,
    pub entry_name: Arc<str>,
    pub upstream_name: String,
}

pub type InProcessConnector = Arc<
    dyn Fn(Box<dyn InProcessService>) -> BoxFuture<'static, anyhow::Result<InProcessRegistration>>
        + Send
        + Sync,
>;

#[cfg(test)]
type TestUpstreamConnector = Arc<
    dyn Fn(
            UpstreamConfig,
        ) -> BoxFuture<
            'static,
            anyhow::Result<(Option<UpstreamConnection>, Vec<rmcp::model::Tool>)>,
        > + Send
        + Sync,
>;

impl UpstreamPool {
    #[must_use]
    pub fn revision_label(&self) -> String {
        format!("pool:{}", self.revision)
    }

    /// Create a new empty pool.
    #[must_use]
    pub fn new() -> Self {
        // reqwest is built workspace-wide with "rustls-no-provider" (root
        // Cargo.toml) so it never silently pulls in aws-lc-rs; a rustls
        // crypto provider must be installed before the first TLS-capable
        // client is built. `crates/labby/src/entrypoint.rs::run()` does this
        // for the real binary; test binaries never go through it, so this
        // call is also needed here. Idempotent -- Err just means a provider
        // (this one) is already installed, safe to ignore.
        drop(rustls::crypto::ring::default_provider().install_default());
        // Build a shared reqwest::Client that lives for the pool's lifetime.
        // All non-OAuth HTTP connects reuse this client so TLS sessions and
        // keep-alive connections are pooled across upstreams (P-M10).
        let shared_http_client = Arc::new(
            reqwest::Client::builder()
                .timeout(DEFAULT_REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default(),
        );
        let (notification_tx, _notification_rx) = Self::notification_channel();
        Self {
            revision: NEXT_POOL_REVISION.fetch_add(1, Ordering::Relaxed),
            invocation_barrier: Arc::new(RwLock::new(())),
            catalog: Arc::new(RwLock::new(HashMap::new())),
            connections: Arc::new(RwLock::new(HashMap::new())),
            connection_catalog_binding: Arc::new(Mutex::new(())),
            generic_oauth_subjects: Arc::new(RwLock::new(HashMap::new())),
            resource_upstreams: Arc::new(RwLock::new(Vec::new())),
            notification_tx,
            subscription_tasks: Arc::new(RwLock::new(HashMap::new())),
            subscription_refresh_pending: Arc::new(Mutex::new(BTreeSet::new())),
            subscription_reconcile_cancel: CancellationToken::new(),
            subscription_resources: Arc::new(RwLock::new(HashMap::new())),
            subscribable_resource_uris: Arc::new(ArcSwap::from_pointee(BTreeSet::new())),
            oauth_client_cache: None,
            oauth_invalidation_barrier: Arc::new(RwLock::new(())),
            probe_tasks: Arc::new(RwLock::new(HashMap::new())),
            reprobe_semaphore: Arc::new(tokio::sync::Semaphore::new(
                upstream_discovery_concurrency(None),
            )),
            catalog_fanout_semaphore: Arc::new(tokio::sync::Semaphore::new(
                upstream_discovery_concurrency(None),
            )),
            call_semaphores: Arc::new(RwLock::new(HashMap::new())),
            call_concurrency: helpers::upstream_call_concurrency(),
            lazy_connect_locks: Arc::new(RwLock::new(HashMap::new())),
            skills_cache: Arc::new(RwLock::new(HashMap::new())),
            skills_fetch_locks: Arc::new(skills_cache::SkillsFetchLocks::default()),
            subject_connections: Arc::new(RwLock::new(HashMap::new())),
            subject_connect_locks: Arc::new(RwLock::new(HashMap::new())),
            relay_connections: Arc::new(RwLock::new(HashMap::new())),
            relay_connect_locks: Arc::new(RwLock::new(HashMap::new())),
            task_routes: Arc::new(RwLock::new(HashMap::new())),
            subject_sweep_task: Arc::new(RwLock::new(None)),
            runtime_origin: None,
            runtime_owner: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            relay_timeout: DEFAULT_RELAY_TIMEOUT,
            in_process_connector: None,
            in_process_ensure_state: Arc::new(Mutex::new(None)),
            shared_http_client,
            usage_store: None,
            header_recovery_metrics_store: HeaderRecoveryMetricsStore::default(),
        }
    }

    /// Attach a connector for in-process (built-in) service peers.
    ///
    /// When provided, built-in lab services are registered as in-process
    /// upstream peers rather than external HTTP/stdio connections.
    #[must_use]
    pub fn with_in_process_connector(mut self, connector: InProcessConnector) -> Self {
        self.in_process_connector = Some(connector);
        self
    }

    /// Attach the per-`(upstream, subject)` OAuth client cache so the pool can
    /// authenticate OAuth-protected upstreams.
    ///
    /// Must be called before `discover_all` for OAuth upstreams to connect successfully.
    #[must_use]
    pub fn with_oauth_client_cache(mut self, cache: OauthClientCache) -> Self {
        self.oauth_invalidation_barrier = cache.invalidation_barrier();
        self.oauth_client_cache = Some(cache);
        self
    }

    #[must_use]
    pub fn with_runtime_origin(mut self, origin: Option<String>) -> Self {
        self.runtime_origin = origin;
        self
    }

    #[must_use]
    pub fn with_runtime_owner(mut self, owner: Option<UpstreamRuntimeOwner>) -> Self {
        self.runtime_owner = owner;
        self
    }

    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Override the per-upstream RPC concurrency bulkhead. Existing lazily
    /// created semaphores are discarded so the new limit applies consistently.
    #[must_use]
    pub fn with_upstream_call_concurrency(mut self, limit: usize) -> Self {
        self.call_concurrency = limit.clamp(1, 128);
        self.call_semaphores = Arc::new(RwLock::new(HashMap::new()));
        self
    }

    pub(super) async fn acquire_catalog_fanout_permit(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
        Arc::clone(&self.catalog_fanout_semaphore)
            .acquire_owned()
            .await
            .map_err(|_| "subject catalog concurrency gate was closed".to_string())
    }

    pub(super) async fn acquire_upstream_call_permit(
        &self,
        upstream_name: &str,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
        let semaphore = if let Some(existing) = self.call_semaphores.read().await.get(upstream_name)
        {
            Arc::clone(existing)
        } else {
            let mut semaphores = self.call_semaphores.write().await;
            Arc::clone(
                semaphores
                    .entry(upstream_name.to_string())
                    .or_insert_with(|| {
                        Arc::new(tokio::sync::Semaphore::new(self.call_concurrency))
                    }),
            )
        };
        semaphore
            .acquire_owned()
            .await
            .map_err(|_| format!("upstream `{upstream_name}` concurrency gate was closed"))
    }

    /// Return the exact stdio child generation backing a pooled or
    /// subject-scoped request. HTTP and in-process connections return `None`.
    pub(super) async fn connection_generation(
        &self,
        upstream_name: &str,
        subject: Option<&str>,
    ) -> Option<u64> {
        if let Some(subject) = subject {
            return self
                .subject_connections
                .read()
                .await
                .get(&(upstream_name.to_string(), subject.to_string()))
                .and_then(|entry| entry._connection.runtime.generation);
        }
        self.connections
            .read()
            .await
            .get(upstream_name)
            .and_then(|connection| connection.runtime.generation)
    }

    /// Set the deadline for relayed upstream tool calls (the elicitation-relay
    /// path). Defaults to `DEFAULT_RELAY_TIMEOUT` (5 minutes) so a human has
    /// time to answer an elicitation without the call timing out.
    #[must_use]
    pub fn with_relay_timeout(mut self, timeout: Duration) -> Self {
        self.relay_timeout = timeout;
        self
    }

    /// Attach a call-usage recorder. `None` explicitly disables capture even
    /// if the caller previously wired one — used by tests that want a clean
    /// pool without reconstructing it.
    #[must_use]
    pub fn with_usage_store(mut self, store: Option<Arc<crate::usage::UsageStore>>) -> Self {
        self.usage_store = store;
        self
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn usage_row_count_for_tests(&self) -> i64 {
        let Some(store) = self.usage_store.as_ref() else {
            return 0;
        };
        store
            .with_conn(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM upstream_calls", [], |row| row.get(0))
                    .map_err(crate::usage::store::sqlite_error)
            })
            .await
            .expect("test usage count")
    }

    /// Configured wall-clock request budget used by surface adapters that must
    /// compose multiple upstream passes under one absolute deadline.
    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    /// Whether a call-usage recorder is wired for this pool. `usage_store`
    /// itself is `pub(super)` (only visible to `pool/` descendant modules,
    /// e.g. `pool/usage_record.rs`); this accessor is the sanctioned way for
    /// code outside the `upstream` module tree (e.g. gateway manager tests)
    /// to observe whether telemetry capture is enabled.
    #[must_use]
    pub fn usage_store_is_wired(&self) -> bool {
        self.usage_store.is_some()
    }

    /// Reuse an externally owned SEP-2243 metrics store. GatewayManager uses
    /// this so every pool generation contributes to one process-lifetime view.
    #[must_use]
    pub(crate) fn with_header_recovery_metrics_store(
        mut self,
        store: HeaderRecoveryMetricsStore,
    ) -> Self {
        self.header_recovery_metrics_store = store;
        self
    }

    pub(super) fn record_header_mismatch_detected(&self, upstream_name: &str) -> u64 {
        self.header_recovery_metrics_store
            .record_mismatch(upstream_name)
    }

    pub(super) fn record_header_schema_refresh(&self, upstream_name: &str) -> u64 {
        self.header_recovery_metrics_store
            .record_refresh(upstream_name)
    }

    pub(super) fn record_header_schema_retry_success(&self, upstream_name: &str) -> u64 {
        self.header_recovery_metrics_store
            .record_retry_success(upstream_name)
    }

    pub(super) fn record_header_schema_retry_failure(&self, upstream_name: &str) -> u64 {
        self.header_recovery_metrics_store
            .record_retry_failure(upstream_name)
    }

    /// Snapshot cumulative SEP-2243 recovery activity for one upstream.
    #[must_use]
    pub(crate) fn header_recovery_metrics(&self, upstream_name: &str) -> HeaderRecoveryMetrics {
        self.header_recovery_metrics_store.snapshot(upstream_name)
    }

    #[cfg(any(test, feature = "testkit"))]
    pub fn header_recovery_is_empty_for_tests(&self, upstream_name: &str) -> bool {
        self.header_recovery_metrics(upstream_name) == HeaderRecoveryMetrics::default()
    }
}

impl Default for UpstreamPool {
    fn default() -> Self {
        Self::new()
    }
}
