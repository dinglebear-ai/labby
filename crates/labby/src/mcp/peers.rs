use std::collections::{BTreeSet, VecDeque};
use std::ops::Deref;
use std::time::Duration;

#[cfg(test)]
use rmcp::RoleServer;
#[cfg(test)]
use rmcp::service::Peer;
use rmcp::service::SubscriptionSink;
#[cfg(feature = "gateway")]
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "gateway")]
use tokio::sync::mpsc;
use tokio::sync::{Mutex, RwLock};
use tokio::time::Instant;

#[cfg(feature = "gateway")]
use crate::dispatch::gateway::types::{CatalogChangeEvent, GatewayCatalogDiff};

#[cfg(feature = "gateway")]
fn lag_reconciliation_diff() -> GatewayCatalogDiff {
    GatewayCatalogDiff {
        tools_changed: true,
        resources_changed: true,
        prompts_changed: true,
    }
}

#[cfg(feature = "gateway")]
async fn bounded_lag_reconciliation<F, T>(
    timeout: Duration,
    reconcile: F,
) -> Result<T, tokio::time::error::Elapsed>
where
    F: Future<Output = T>,
{
    tokio::time::timeout(timeout, reconcile).await
}

/// A connected peer plus what it can see and what it was last told.
///
/// `tools/list_changed` is a claim about one session's tool list, so the
/// fanout has to be able to answer "did *this* peer's contract move" — which
/// needs the peer's own scope (`contract`) and the contract it last published
/// (`last_contract`). Broadcasting one global diff instead is what let a
/// raw-exposing protected route miss its own changes.
#[derive(Clone)]
pub struct RegisteredPeer {
    /// Server-global identity. JSON-RPC request IDs are client-local, so they
    /// cannot identify a registry entry shared by all transports.
    pub(crate) registration_id: u64,
    pub(crate) target: NotificationTarget,
    pub(crate) contract: crate::mcp::peer_contract::PeerContract,
    /// Last complete contract this peer actually received from `tools/list`,
    /// or `None` when it subscribed before completing a listing.
    pub(crate) last_contract: Option<crate::mcp::catalog::ToolCatalogSnapshot>,
}

static PEER_REGISTRATION_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_registration_id() -> u64 {
    PEER_REGISTRATION_COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone)]
pub(crate) enum NotificationTarget {
    #[cfg(test)]
    LegacyPeer(Peer<RoleServer>),
    Subscription(SubscriptionSink),
}

impl NotificationTarget {
    pub(crate) fn wants_tool_list_changed(&self) -> bool {
        match self {
            #[cfg(test)]
            Self::LegacyPeer(_) => true,
            Self::Subscription(sink) => sink.accepted().tools_list_changed == Some(true),
        }
    }

    pub(crate) fn wants_resource_list_changed(&self) -> bool {
        match self {
            #[cfg(test)]
            Self::LegacyPeer(_) => true,
            Self::Subscription(sink) => sink.accepted().resources_list_changed == Some(true),
        }
    }

    pub(crate) fn wants_prompt_list_changed(&self) -> bool {
        match self {
            #[cfg(test)]
            Self::LegacyPeer(_) => true,
            Self::Subscription(sink) => sink.accepted().prompts_list_changed == Some(true),
        }
    }

    pub(crate) fn wants_resource_update(&self, uri: &str) -> bool {
        match self {
            #[cfg(test)]
            Self::LegacyPeer(_) => true,
            Self::Subscription(sink) => sink
                .accepted()
                .resource_subscriptions
                .as_ref()
                .is_some_and(|uris| uris.iter().any(|accepted| accepted == uri)),
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        match self {
            #[cfg(test)]
            Self::LegacyPeer(peer) => peer.is_transport_closed(),
            // Subscriptions are removed when `listen` returns. A failed send is
            // also pruned by the fanout path.
            Self::Subscription(_) => false,
        }
    }

    pub(crate) async fn notify_tool_list_changed(&self) -> Result<(), ()> {
        match self {
            #[cfg(test)]
            Self::LegacyPeer(peer) => peer.notify_tool_list_changed().await.map_err(|_| ()),
            Self::Subscription(sink) => sink.notify_tool_list_changed().await.map_err(|_| ()),
        }
    }

    pub(crate) async fn notify_resource_list_changed(&self) -> Result<(), ()> {
        match self {
            #[cfg(test)]
            Self::LegacyPeer(peer) => peer.notify_resource_list_changed().await.map_err(|_| ()),
            Self::Subscription(sink) => sink.notify_resource_list_changed().await.map_err(|_| ()),
        }
    }

    pub(crate) async fn notify_prompt_list_changed(&self) -> Result<(), ()> {
        match self {
            #[cfg(test)]
            Self::LegacyPeer(peer) => peer.notify_prompt_list_changed().await.map_err(|_| ()),
            Self::Subscription(sink) => sink.notify_prompt_list_changed().await.map_err(|_| ()),
        }
    }

    pub(crate) async fn notify_resource_updated(&self, uri: &str) -> Result<(), ()> {
        match self {
            #[cfg(test)]
            Self::LegacyPeer(peer) => peer
                .notify_resource_updated(rmcp::model::ResourceUpdatedNotificationParam::new(uri))
                .await
                .map_err(|_| ()),
            Self::Subscription(sink) => sink.notify_resource_updated(uri).await.map_err(|_| ()),
        }
    }
}

impl RegisteredPeer {
    pub(crate) fn from_subscription(
        sink: SubscriptionSink,
        contract: crate::mcp::peer_contract::PeerContract,
        last_contract: Option<crate::mcp::catalog::ToolCatalogSnapshot>,
    ) -> Self {
        Self {
            registration_id: next_registration_id(),
            target: NotificationTarget::Subscription(sink),
            contract,
            last_contract,
        }
    }
}

const RESOURCE_UPDATE_REPLAY_WINDOW: Duration = Duration::from_secs(2);
const MAX_RECENT_RESOURCE_UPDATES: usize = 256;

#[derive(Clone)]
struct RecentResourceUpdate {
    observed_at: Instant,
    upstream: String,
    uri: String,
}

/// Registry of live sessions, plus the tiny edge-event journal needed to
/// bridge rmcp's subscription-ack -> handler-registration scheduling gap.
///
/// `subscriptions/acknowledged` is emitted before `ServerHandler::listen`
/// runs. Resource updates are edge-triggered rather than derivable catalog
/// state, so the notifier records them before fanout and a newly registered
/// peer replays matching updates from this short bounded window.
#[derive(Default)]
pub struct PeerRegistryState {
    peers: RwLock<Vec<RegisteredPeer>>,
    recent_resource_updates: Mutex<VecDeque<RecentResourceUpdate>>,
}

impl Deref for PeerRegistryState {
    type Target = RwLock<Vec<RegisteredPeer>>;

    fn deref(&self) -> &Self::Target {
        &self.peers
    }
}

impl PeerRegistryState {
    pub(crate) async fn record_resource_update(&self, upstream: &str, uri: &str) {
        let now = Instant::now();
        let mut updates = self.recent_resource_updates.lock().await;
        prune_recent_resource_updates(&mut updates, now);
        updates.push_back(RecentResourceUpdate {
            observed_at: now,
            upstream: upstream.to_string(),
            uri: uri.to_string(),
        });
        while updates.len() > MAX_RECENT_RESOURCE_UPDATES {
            updates.pop_front();
        }
    }

    pub(crate) async fn recent_resource_updates_for(
        &self,
        registered: &RegisteredPeer,
    ) -> Vec<(String, String)> {
        let now = Instant::now();
        let mut updates = self.recent_resource_updates.lock().await;
        prune_recent_resource_updates(&mut updates, now);
        let mut seen = BTreeSet::new();
        let mut matched = updates
            .iter()
            .rev()
            .filter(|update| {
                registered
                    .contract
                    .route_scope
                    .allows_upstream(&update.upstream)
                    && registered.target.wants_resource_update(&update.uri)
                    && seen.insert((update.upstream.clone(), update.uri.clone()))
            })
            .map(|update| (update.upstream.clone(), update.uri.clone()))
            .collect::<Vec<_>>();
        matched.reverse();
        matched
    }

    #[cfg(test)]
    async fn recent_resource_update_count(&self) -> usize {
        self.recent_resource_updates.lock().await.len()
    }
}

fn prune_recent_resource_updates(updates: &mut VecDeque<RecentResourceUpdate>, now: Instant) {
    while updates.front().is_some_and(|update| {
        now.saturating_duration_since(update.observed_at) > RESOURCE_UPDATE_REPLAY_WINDOW
    }) {
        updates.pop_front();
    }
}

/// Registry of live sessions, shared by every `LabMcpServer` and the notifier.
pub type PeerRegistry = Arc<PeerRegistryState>;

/// Drop peers whose transport has definitively closed, returning how many went.
///
/// Peers were previously removed only inside the notification fanout, as a side
/// effect of a send failing. That made pruning depend on notifications being
/// emitted — which was fine when the gateway emitted spurious ones constantly,
/// and stopped being fine once that was fixed: with notifications correctly
/// rare, the registry only ever grew (119 dead sessions accumulated in about a
/// day of real use).
///
/// Only `is_transport_closed()` peers are dropped. A live-but-idle session must
/// never be evicted — that would silently cost it every future notification,
/// which is a worse failure than briefly holding a dead entry.
pub(crate) async fn prune_closed_peers(peers: &PeerRegistry) -> usize {
    let mut guard = peers.write().await;
    let before = guard.len();
    guard.retain(|registered| !registered.target.is_closed());
    before - guard.len()
}

#[cfg(test)]
impl RegisteredPeer {
    /// Register a gateway-less peer whose live contract is empty, with
    /// `last_contract` supplied by the caller. A non-empty `last_contract`
    /// therefore re-derives to a *changed* contract and the peer is owed a
    /// notification; an empty one re-derives unchanged and the peer is not.
    /// Lets fanout tests drive both outcomes without standing up a gateway.
    pub(crate) fn with_last_contract_for_test(
        peer: Peer<RoleServer>,
        last_contract: crate::mcp::catalog::ToolCatalogSnapshot,
    ) -> Self {
        Self {
            registration_id: next_registration_id(),
            target: NotificationTarget::LegacyPeer(peer),
            contract: crate::mcp::peer_contract::PeerContract {
                registry: Arc::new(crate::registry::ToolRegistry::default()),
                #[cfg(feature = "gateway")]
                gateway_manager: None,
                // Keep the empty-contract fixture independent of process-wide
                // Code Mode changes made by other tests. Root scope can expose
                // the synthetic Code Mode tools even without a gateway manager.
                route_scope: crate::mcp::route_scope::McpRouteScope::protected_subset(
                    "catalog-notification-test",
                    std::iter::empty::<&str>(),
                    std::iter::empty::<&str>(),
                    false,
                ),
                code_mode_app_state: Default::default(),
                audience: crate::mcp::peer_contract::PeerCatalogAudience::default(),
            },
            last_contract: Some(last_contract),
        }
    }

    /// A peer whose contract has moved since it was last notified.
    pub(crate) fn stale_for_test(peer: Peer<RoleServer>) -> Self {
        Self::with_last_contract_for_test(
            peer,
            crate::mcp::catalog::ToolCatalogSnapshot::from_names(
                std::iter::once("stale-tool-from-a-previous-contract".to_string()).collect(),
            ),
        )
    }

    /// A peer already in sync with what it would see now.
    pub(crate) fn current_for_test(peer: Peer<RoleServer>) -> Self {
        Self::with_last_contract_for_test(
            peer,
            crate::mcp::catalog::ToolCatalogSnapshot::from_names(BTreeSet::new()),
        )
    }
}

#[cfg(test)]
#[tokio::test]
async fn resource_update_journal_is_bounded() {
    let peers: PeerRegistry = Default::default();
    for index in 0..(MAX_RECENT_RESOURCE_UPDATES + 2) {
        peers
            .record_resource_update("leaf", &format!("fixture://resource/{index}"))
            .await;
    }
    assert_eq!(
        peers.recent_resource_update_count().await,
        MAX_RECENT_RESOURCE_UPDATES
    );
}

#[cfg(test)]
#[test]
fn resource_update_journal_prunes_expired_entries() {
    let now = Instant::now();
    let mut updates = VecDeque::from([
        RecentResourceUpdate {
            observed_at: now - RESOURCE_UPDATE_REPLAY_WINDOW - Duration::from_millis(1),
            upstream: "leaf".to_string(),
            uri: "fixture://expired".to_string(),
        },
        RecentResourceUpdate {
            observed_at: now,
            upstream: "leaf".to_string(),
            uri: "fixture://fresh".to_string(),
        },
    ]);

    prune_recent_resource_updates(&mut updates, now);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].uri, "fixture://fresh");
}

#[cfg(test)]
#[test]
fn catalog_notification_fixture_never_exposes_code_mode() {
    let scope = crate::mcp::route_scope::McpRouteScope::protected_subset(
        "catalog-notification-test",
        std::iter::empty::<&str>(),
        std::iter::empty::<&str>(),
        false,
    );

    assert!(!scope.exposes_code_mode());
    assert!(!scope.allows_service("gateway"));
    assert!(!scope.allows_upstream("fixture"));
}

/// MCP-specific peer fanout that forwards catalog-change notifications to
/// active subscriptions whose visible contract actually changed.
///
/// This keeps `rmcp` types out of the dispatch layer while allowing
/// `GatewayManager` to notify peers when the upstream pool changes.
#[derive(Clone, Default)]
pub struct PeerNotifier {
    pub peers: PeerRegistry,
    pub(crate) code_mode_app_state: crate::mcp::catalog::CodeModeAppState,
    /// Observed inbound MCP client metadata (redacted subject, declared
    /// client name/version, transport, connect time), one entry pushed per
    /// successful discovery. Read by `gateway.clients.list` via
    /// `GatewayManager::with_client_registry`. Not index-paired with `peers`
    /// and not pruned on disconnect — see
    /// `labby_runtime::client_registry` module docs for the best-effort
    /// caveat; this deliberately does not reuse `peers`' pruning dance
    /// (would require keeping two Vecs in lockstep under concurrent
    /// connects, which is real complexity for a first pass — see bead
    /// lab-av018 follow-up).
    #[cfg(feature = "gateway")]
    pub client_registry: labby_runtime::client_registry::ClientRegistryHandle,
}

impl PeerNotifier {
    #[cfg(feature = "gateway")]
    pub async fn run_upstream_notifications(
        self,
        runtime: crate::dispatch::gateway::manager::GatewayRuntimeHandle,
    ) {
        use crate::dispatch::upstream::pool::UpstreamNotificationEvent;

        let mut pool_changes = runtime.subscribe_pool_changes();
        loop {
            let Some(pool) = runtime.current_pool().await else {
                if pool_changes.changed().await.is_err() {
                    break;
                }
                continue;
            };
            let mut notifications = pool.subscribe_notifications();
            loop {
                tokio::select! {
                    changed = pool_changes.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        break;
                    }
                    event = notifications.recv() => {
                        match event {
                            Ok(UpstreamNotificationEvent::ToolListChanged { upstream }) => {
                                // Re-list the exact named upstream before peers recompute their
                                // visible contracts. Both the shared subscription stream and
                                // request-scoped relay clients publish through this event bus.
                                pool.refresh_tools_after_list_changed(&upstream).await;
                                self.notify_catalog_changes(
                                    &GatewayCatalogDiff {
                                        tools_changed: true,
                                        resources_changed: false,
                                        prompts_changed: false,
                                    },
                                    labby_runtime::catalog_notify::SOURCE_UPSTREAM_SUBSCRIPTION,
                                ).await;
                            }
                            Ok(UpstreamNotificationEvent::PromptListChanged { upstream }) => {
                                self.notify_upstream_catalog_change(
                                    false, false, true, upstream,
                                );
                            }
                            Ok(UpstreamNotificationEvent::ResourceListChanged { upstream }) => {
                                self.notify_upstream_catalog_change(
                                    false, true, false, upstream,
                                );
                            }
                            Ok(UpstreamNotificationEvent::ResourceUpdated { upstream, uri }) => {
                                // Journal first. rmcp sends subscriptions/acknowledged before
                                // invoking LabMcpServer::listen, so a newly acknowledged peer
                                // may not be in the registry yet. listen replays this bounded
                                // edge-event journal after registration.
                                self.peers.record_resource_update(&upstream, &uri).await;
                                crate::mcp::catalog_notifications::notify_resource_update_peers(
                                    &self.peers,
                                    &upstream,
                                    &uri,
                                ).await;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                self.reconcile_after_notification_lag(&pool, skipped).await;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            Ok(_) => {}
                        }
                    }
                }
            }
        }
    }

    #[cfg(feature = "gateway")]
    pub async fn run(self, mut rx: mpsc::UnboundedReceiver<CatalogChangeEvent>) {
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "notifier.start",
            subsystem = "mcp_server",
            phase = "peer_notifier.start",
            "starting MCP peer catalog-change notifier"
        );
        while let Some(event) = rx.recv().await {
            self.notify_catalog_changes(&event.diff, event.source).await;
        }
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "notifier.stop",
            subsystem = "mcp_server",
            phase = "peer_notifier.stop",
            "MCP peer catalog-change notifier stopped"
        );
    }

    /// Hands the diff to the coalescer rather than fanning out immediately: a
    /// reconcile commonly produces several triggers for one net visible change,
    /// and a notification delivered into an open turn invalidates the binding
    /// that turn is using. See `catalog_coalesce`.
    #[cfg(feature = "gateway")]
    async fn notify_catalog_changes(&self, diff: &GatewayCatalogDiff, source: &'static str) {
        crate::mcp::catalog_coalesce::schedule_catalog_notification(
            &self.peers,
            diff.into(),
            source,
        );
    }

    #[cfg(feature = "gateway")]
    fn notify_upstream_catalog_change(
        &self,
        tools_changed: bool,
        resources_changed: bool,
        prompts_changed: bool,
        upstream: String,
    ) {
        crate::mcp::catalog_coalesce::schedule_catalog_notification(
            &self.peers,
            crate::mcp::catalog_notifications::CatalogNotificationChanges::new(
                tools_changed,
                resources_changed,
                prompts_changed,
            )
            .for_upstream(upstream),
            labby_runtime::catalog_notify::SOURCE_UPSTREAM_SUBSCRIPTION,
        );
    }

    #[cfg(feature = "gateway")]
    async fn reconcile_after_notification_lag(
        &self,
        pool: &crate::dispatch::upstream::pool::UpstreamPool,
        skipped: u64,
    ) {
        use futures::StreamExt;

        const RECONCILE_TIMEOUT: Duration = Duration::from_secs(30);
        let started = std::time::Instant::now();
        tracing::warn!(
            surface = "mcp",
            service = "peers",
            action = "catalog.reconcile.start",
            phase = "broadcast_lag",
            skipped,
            timeout_ms = RECONCILE_TIMEOUT.as_millis(),
            "upstream notification relay lagged; reconciling authoritative catalogs"
        );

        let reconcile = async {
            let tool_names = pool
                .routable_upstream_names(
                    crate::dispatch::upstream::types::UpstreamCapability::Tools,
                )
                .await;
            let tool_refreshes =
                futures::stream::iter(tool_names)
                    .map(|upstream| async move {
                        pool.refresh_tools_after_list_changed(&upstream).await
                    })
                    .buffer_unordered(8)
                    .collect::<Vec<_>>();
            let (resources, prompts, tool_results) = tokio::join!(
                pool.list_upstream_resources(),
                pool.list_upstream_prompts(&[]),
                tool_refreshes,
            );
            let refreshed_tools = tool_results
                .into_iter()
                .filter(|refreshed| *refreshed)
                .count();
            (refreshed_tools, resources.len(), prompts.len())
        };

        match bounded_lag_reconciliation(RECONCILE_TIMEOUT, reconcile).await {
            Ok((refreshed_tools, resource_count, prompt_count)) => {
                tracing::info!(
                    surface = "mcp",
                    service = "peers",
                    action = "catalog.reconcile.finish",
                    phase = "broadcast_lag",
                    outcome = "success",
                    skipped,
                    refreshed_tools,
                    resource_count,
                    prompt_count,
                    elapsed_ms = started.elapsed().as_millis(),
                    "reconciled authoritative catalogs after notification lag"
                );
                self.notify_catalog_changes(
                    &lag_reconciliation_diff(),
                    labby_runtime::catalog_notify::SOURCE_UPSTREAM_NOTIFICATION_LAG,
                )
                .await;
            }
            Err(_) => {
                tracing::warn!(
                    surface = "mcp",
                    service = "peers",
                    action = "catalog.reconcile.finish",
                    phase = "broadcast_lag",
                    outcome = "timeout_partial_unknown",
                    convergence_scheduled = true,
                    skipped,
                    timeout_ms = RECONCILE_TIMEOUT.as_millis(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "catalog reconciliation timed out after notification lag; caches may be partially refreshed"
                );
                // The refresh futures can update one cache before another one
                // reaches the outer deadline. Always signal all catalog kinds
                // after timeout so no partial mutation remains unpublished.
                self.notify_catalog_changes(
                    &lag_reconciliation_diff(),
                    labby_runtime::catalog_notify::SOURCE_UPSTREAM_NOTIFICATION_LAG,
                )
                .await;
            }
        }
    }
}

#[cfg(all(test, feature = "gateway"))]
mod lag_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{bounded_lag_reconciliation, lag_reconciliation_diff};

    #[tokio::test]
    async fn forced_broadcast_lag_requires_reconciliation_of_every_catalog_class() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(1);
        tx.send("first").expect("receiver exists");
        tx.send("contract-changing-final-event")
            .expect("receiver exists");

        let error = rx.recv().await.expect_err("receiver must report lag");
        assert!(matches!(
            error,
            tokio::sync::broadcast::error::RecvError::Lagged(1)
        ));
        let recovery = lag_reconciliation_diff();
        assert!(recovery.tools_changed);
        assert!(recovery.resources_changed);
        assert!(recovery.prompts_changed);
        assert_eq!(
            rx.recv().await.expect("latest event remains available"),
            "contract-changing-final-event"
        );
    }

    #[tokio::test]
    async fn timeout_after_partial_refresh_still_requires_all_catalog_convergence() {
        let cache_mutated = Arc::new(AtomicBool::new(false));
        let mutation = Arc::clone(&cache_mutated);
        let result = bounded_lag_reconciliation(std::time::Duration::from_millis(10), async move {
            mutation.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
        })
        .await;

        assert!(result.is_err(), "synthetic partial refresh must time out");
        assert!(
            cache_mutated.load(Ordering::SeqCst),
            "one authoritative cache changed before the timeout"
        );
        let convergence = lag_reconciliation_diff();
        assert!(convergence.tools_changed);
        assert!(convergence.resources_changed);
        assert!(convergence.prompts_changed);
    }
}
