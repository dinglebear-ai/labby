use futures::future::join_all;

use crate::mcp::catalog::{CatalogChangeSet, ToolCatalogSnapshot};
use crate::mcp::peers::{PeerRegistry, RegisteredPeer};

#[cfg(feature = "gateway")]
use crate::dispatch::gateway::types::GatewayCatalogDiff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CatalogNotificationChanges {
    pub(crate) tools_changed: bool,
    pub(crate) resources_changed: bool,
    pub(crate) prompts_changed: bool,
}

impl CatalogNotificationChanges {
    pub(crate) const fn new(
        tools_changed: bool,
        resources_changed: bool,
        prompts_changed: bool,
    ) -> Self {
        Self {
            tools_changed,
            resources_changed,
            prompts_changed,
        }
    }

    pub(crate) const fn any(self) -> bool {
        self.tools_changed || self.resources_changed || self.prompts_changed
    }

    /// Union of two triggers. Coalescing must never drop a kind because a
    /// later trigger in the batch only moved a different one.
    pub(crate) const fn merged_with(self, other: Self) -> Self {
        Self {
            tools_changed: self.tools_changed || other.tools_changed,
            resources_changed: self.resources_changed || other.resources_changed,
            prompts_changed: self.prompts_changed || other.prompts_changed,
        }
    }
}

impl From<CatalogChangeSet> for CatalogNotificationChanges {
    fn from(changes: CatalogChangeSet) -> Self {
        Self::new(
            changes.tools_changed,
            changes.resources_changed,
            changes.prompts_changed,
        )
    }
}

#[cfg(feature = "gateway")]
impl From<&GatewayCatalogDiff> for CatalogNotificationChanges {
    fn from(diff: &GatewayCatalogDiff) -> Self {
        Self::new(
            diff.tools_changed,
            diff.resources_changed,
            diff.prompts_changed,
        )
    }
}

/// Fan a catalog change out to every connected peer.
///
/// `source` attributes the emission to a call site (see
/// `labby_runtime::catalog_notify`); it is the field that makes notification
/// churn diagnosable, so callers must pass a real label rather than a
/// convenient constant.
///
/// This is the single choke point every emitter funnels through, which is why
/// churn is recorded here and nowhere else — recording per emitter would count
/// a diff once per peer.
pub(crate) async fn notify_catalog_peers(
    peers: &PeerRegistry,
    changes: CatalogNotificationChanges,
    source: &'static str,
) {
    if !changes.any() {
        return;
    }

    let peer_snapshot = peers.read().await.clone();
    let peer_count = peer_snapshot.len();
    let evaluated = evaluate_peers(peer_snapshot, changes).await;
    let peers_notified = evaluated
        .iter()
        .filter(|evaluated| evaluated.changes.any())
        .count();
    // Nothing to say to anyone: the trigger did not move any peer's visible
    // contract. This is the healthy outcome for raw upstream churn under Code
    // Mode, and it must not count as a notification.
    if peers_notified == 0 {
        tracing::debug!(
            surface = "mcp",
            service = "peers",
            action = "catalog.notify.skipped",
            subsystem = "mcp_server",
            source,
            peer_count,
            "catalog change moved no peer's visible contract; nothing broadcast"
        );
        return;
    }
    let churn = crate::mcp::catalog_churn::record_notification();
    // `during_tool_call` is the field to reach for first when a client reports
    // "the tool disappeared mid-turn": a notification emitted while a call is
    // open invalidates the binding that call is using.
    let during_tool_call = churn.in_flight_tool_calls > 0;
    tracing::info!(
        surface = "mcp",
        service = "peers",
        action = "catalog.notify",
        subsystem = "mcp_server",
        phase = "catalog.notify",
        source,
        peer_count,
        peers_notified,
        peers_skipped = peer_count.saturating_sub(peers_notified),
        tools_changed = changes.tools_changed,
        resources_changed = changes.resources_changed,
        prompts_changed = changes.prompts_changed,
        notify_total = churn.total,
        since_last_ms = churn.since_last_ms,
        window_count = churn.window_count,
        window_secs = churn.window_secs,
        in_flight_tool_calls = churn.in_flight_tool_calls,
        during_tool_call,
        "notifying MCP peers about catalog change"
    );
    if churn.is_churning() {
        tracing::warn!(
            surface = "mcp",
            service = "peers",
            action = "catalog.notify.churn",
            subsystem = "mcp_server",
            phase = "catalog.notify",
            source,
            peer_count,
            peers_notified,
            notify_total = churn.total,
            since_last_ms = churn.since_last_ms,
            window_count = churn.window_count,
            window_secs = churn.window_secs,
            threshold = churn.threshold,
            in_flight_tool_calls = churn.in_flight_tool_calls,
            during_tool_call,
            "catalog notification churn: clients are rebuilding their tool bindings repeatedly"
        );
    }

    let notification_timeout = crate::config::resolved_catalog_notification_timeout();
    let notify_futures = evaluated.iter().enumerate().map(|(peer_index, evaluated)| {
        let target = evaluated.registered.target.clone();
        let changes = evaluated.changes;
        async move {
            let result = tokio::time::timeout(notification_timeout, async {
                if changes.tools_changed && target.notify_tool_list_changed().await.is_err() {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        phase = "tools",
                        tools_changed = changes.tools_changed,
                        resources_changed = changes.resources_changed,
                        prompts_changed = changes.prompts_changed,
                        "failed to notify peer about catalog change; pruning stale session"
                    );
                    return false;
                }
                if changes.resources_changed && target.notify_resource_list_changed().await.is_err()
                {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        phase = "resources",
                        tools_changed = changes.tools_changed,
                        resources_changed = changes.resources_changed,
                        prompts_changed = changes.prompts_changed,
                        "failed to notify peer about catalog change; pruning stale session"
                    );
                    return false;
                }
                if changes.prompts_changed && target.notify_prompt_list_changed().await.is_err() {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        phase = "prompts",
                        tools_changed = changes.tools_changed,
                        resources_changed = changes.resources_changed,
                        prompts_changed = changes.prompts_changed,
                        "failed to notify peer about catalog change; pruning stale session"
                    );
                    return false;
                }
                true
            })
            .await;

            match result {
                Ok(alive) => alive,
                Err(_elapsed) => {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        timeout_ms = notification_timeout.as_millis(),
                        tools_changed = changes.tools_changed,
                        resources_changed = changes.resources_changed,
                        prompts_changed = changes.prompts_changed,
                        "peer notification timed out; pruning stale session"
                    );
                    false
                }
            }
        }
    });

    let results = join_all(notify_futures).await;
    // A peer that was successfully told about its new contract has now been
    // told; record that so the next fanout diffs against what it actually
    // received. A peer that failed is being pruned, so its bookkeeping is moot.
    let outcomes: Vec<_> = evaluated.into_iter().zip(results).collect();

    let mut guard = peers.write().await;
    let mut pruned = 0;
    for (evaluated, ok) in &outcomes {
        if !ok {
            continue;
        }
        let registered = evaluated.clone().into_published();
        let registration_id = registered.registration_id;
        if let Some(index) = guard
            .iter()
            .position(|current| current.registration_id == registration_id)
        {
            guard[index] = registered;
        }
    }
    for (evaluated, ok) in outcomes {
        if ok {
            continue;
        }
        if let Some(index) = guard
            .iter()
            .position(|current| current.registration_id == evaluated.registered.registration_id)
        {
            guard.remove(index);
            pruned += 1;
        }
    }
    tracing::info!(
        surface = "mcp",
        service = "peers",
        action = "peer.gc",
        pruned_count = pruned,
        active_count = guard.len(),
        "MCP peer catalog-change notification complete"
    );
}

/// Forward one normalized resource update to subscriptions that accepted the
/// exact URI and whose protected route exposes the owning upstream.
pub(crate) async fn notify_resource_update_peers(peers: &PeerRegistry, upstream: &str, uri: &str) {
    let snapshot = peers.read().await.clone();
    let notification_timeout = crate::config::resolved_catalog_notification_timeout();
    let deliveries = snapshot.into_iter().filter_map(|registered| {
        (registered.contract.route_scope.allows_upstream(upstream)
            && registered.target.wants_resource_update(uri))
        .then_some(registered)
    });

    let outcomes = join_all(deliveries.map(|registered| {
        let target = registered.target.clone();
        async move {
            let ok =
                tokio::time::timeout(notification_timeout, target.notify_resource_updated(uri))
                    .await
                    .is_ok_and(|result| result.is_ok());
            (registered.registration_id, ok)
        }
    }))
    .await;

    if outcomes.iter().all(|(_, ok)| *ok) {
        return;
    }
    let failed = outcomes
        .into_iter()
        .filter_map(|(registration_id, ok)| (!ok).then_some(registration_id))
        .collect::<std::collections::HashSet<_>>();
    peers
        .write()
        .await
        .retain(|registered| !failed.contains(&registered.registration_id));
}

/// One peer's evaluated outcome for a single fanout: what it will be told, and
/// the contract to remember if that succeeds.
#[derive(Clone)]
struct EvaluatedPeer {
    registered: RegisteredPeer,
    changes: CatalogNotificationChanges,
    /// Freshly computed contract, present only when tools were re-evaluated.
    next_contract: Option<ToolCatalogSnapshot>,
}

impl EvaluatedPeer {
    fn into_published(self) -> RegisteredPeer {
        let mut registered = self.registered;
        if let Some(next) = self.next_contract {
            registered.last_contract = next;
        }
        registered
    }
}

/// Re-derive each peer's visible contract and decide what that peer is owed.
///
/// `changes.tools_changed` arrives as a *hint* — "something happened that could
/// move a tool list" — not a verdict. The verdict is per peer, because two
/// sessions can see different contracts from the same gateway state (see
/// `peer_contract.rs`). Resources and prompts are still global signals and are
/// forwarded to every peer unchanged.
///
/// Contracts are computed off the registry lock: `visible_contract()` takes
/// gateway config and pool locks, and holding the peer registry across that
/// would serialize notification against session registration for no benefit.
async fn evaluate_peers(
    peer_snapshot: Vec<RegisteredPeer>,
    changes: CatalogNotificationChanges,
) -> Vec<EvaluatedPeer> {
    let mut evaluated = Vec::with_capacity(peer_snapshot.len());
    for registered in peer_snapshot {
        let next_contract = if changes.tools_changed {
            Some(registered.contract.visible_contract().await)
        } else {
            None
        };
        let tools_changed = next_contract
            .as_ref()
            .is_some_and(|next| *next != registered.last_contract)
            && registered.target.wants_tool_list_changed();
        let resources_changed =
            changes.resources_changed && registered.target.wants_resource_list_changed();
        let prompts_changed =
            changes.prompts_changed && registered.target.wants_prompt_list_changed();
        evaluated.push(EvaluatedPeer {
            registered,
            changes: CatalogNotificationChanges::new(
                tools_changed,
                resources_changed,
                prompts_changed,
            ),
            next_contract,
        });
    }
    evaluated
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, MutexGuard};
    use std::time::Duration;

    use rmcp::service::{MaybeSendFuture, NotificationContext};
    use rmcp::{ClientHandler, RoleClient, ServerHandler, ServiceExt};
    use tokio::sync::{Notify, RwLock};

    use super::{CatalogNotificationChanges, notify_catalog_peers};
    use crate::mcp::catalog::CatalogChangeSet;
    use crate::mcp::catalog_churn::InFlightToolCall;
    use crate::mcp::catalog_coalesce::{reset_for_test, schedule_catalog_notification};
    use crate::mcp::peers::{PeerRegistry, RegisteredPeer, prune_closed_peers};

    fn serial_catalog() -> MutexGuard<'static, ()> {
        crate::test_support::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[derive(Clone)]
    struct TestServer {
        peers: PeerRegistry,
        /// Whether this session registers as "my contract has moved since I was
        /// last notified" (true) or "already in sync" (false).
        stale: bool,
    }

    impl ServerHandler for TestServer {
        fn on_initialized(
            &self,
            context: NotificationContext<rmcp::RoleServer>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            let peers = Arc::clone(&self.peers);
            let peer = context.peer.clone();
            let stale = self.stale;
            async move {
                let registered = if stale {
                    RegisteredPeer::stale_for_test(peer)
                } else {
                    RegisteredPeer::current_for_test(peer)
                };
                peers.write().await.push(registered);
            }
        }
    }

    #[derive(Clone, Default)]
    struct TestClient {
        tool_count: Arc<AtomicUsize>,
        resource_count: Arc<AtomicUsize>,
        prompt_count: Arc<AtomicUsize>,
        notify: Arc<Notify>,
    }

    impl TestClient {
        async fn wait_for_notifications(&self, expected_total: usize) {
            tokio::time::timeout(Duration::from_secs(5), async {
                while self.total() < expected_total {
                    self.notify.notified().await;
                }
            })
            .await
            .expect("timed out waiting for catalog notification");
        }

        fn total(&self) -> usize {
            self.tool_count.load(Ordering::SeqCst)
                + self.resource_count.load(Ordering::SeqCst)
                + self.prompt_count.load(Ordering::SeqCst)
        }
    }

    impl ClientHandler for TestClient {
        fn on_tool_list_changed(
            &self,
            _context: NotificationContext<RoleClient>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            self.tool_count.fetch_add(1, Ordering::SeqCst);
            self.notify.notify_one();
            std::future::ready(())
        }

        fn on_resource_list_changed(
            &self,
            _context: NotificationContext<RoleClient>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            self.resource_count.fetch_add(1, Ordering::SeqCst);
            self.notify.notify_one();
            std::future::ready(())
        }

        fn on_prompt_list_changed(
            &self,
            _context: NotificationContext<RoleClient>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            self.prompt_count.fetch_add(1, Ordering::SeqCst);
            self.notify.notify_one();
            std::future::ready(())
        }
    }

    async fn connected_peer_fixture() -> (
        PeerRegistry,
        TestClient,
        rmcp::service::RunningService<RoleClient, TestClient>,
        tokio::task::JoinHandle<
            Result<
                rmcp::service::RunningService<rmcp::RoleServer, TestServer>,
                rmcp::service::ServerInitializeError,
            >,
        >,
    ) {
        let peers = Arc::new(RwLock::new(Vec::new()));
        let (client, client_service, server_handle) = connect_peer(&peers, true).await;
        (peers, client, client_service, server_handle)
    }

    /// Attach one more session to an existing registry, registering it as
    /// stale (owed a notification) or already in sync.
    async fn connect_peer(
        peers: &PeerRegistry,
        stale: bool,
    ) -> (
        TestClient,
        rmcp::service::RunningService<RoleClient, TestClient>,
        tokio::task::JoinHandle<
            Result<
                rmcp::service::RunningService<rmcp::RoleServer, TestServer>,
                rmcp::service::ServerInitializeError,
            >,
        >,
    ) {
        let expected_peer_count = peers.read().await.len() + 1;
        let server = TestServer {
            peers: Arc::clone(peers),
            stale,
        };
        let client = TestClient::default();
        let (server_transport, client_transport) = tokio::io::duplex(4096);
        let server_handle = tokio::spawn(async move { server.serve(server_transport).await });
        let client_service = client
            .clone()
            .serve(client_transport)
            .await
            .expect("client starts");

        tokio::time::timeout(Duration::from_secs(5), async {
            while peers.read().await.len() < expected_peer_count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("server peer registered");

        (client, client_service, server_handle)
    }

    #[test]
    fn catalog_notification_changes_reports_any_changed_kind() {
        assert!(!CatalogNotificationChanges::new(false, false, false).any());
        assert!(CatalogNotificationChanges::new(true, false, false).any());
        assert!(CatalogNotificationChanges::new(false, true, false).any());
        assert!(CatalogNotificationChanges::new(false, false, true).any());
    }

    #[test]
    fn catalog_notification_changes_preserves_catalog_change_set_fields() {
        let changes = CatalogNotificationChanges::from(CatalogChangeSet {
            tools_changed: false,
            resources_changed: true,
            prompts_changed: true,
        });

        assert_eq!(changes, CatalogNotificationChanges::new(false, true, true));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn catalog_notification_changes_preserves_gateway_diff_fields() {
        let changes = CatalogNotificationChanges::from(
            &crate::dispatch::gateway::types::GatewayCatalogDiff {
                tools_changed: true,
                resources_changed: false,
                prompts_changed: true,
            },
        );

        assert_eq!(changes, CatalogNotificationChanges::new(true, false, true));
    }

    #[tokio::test]
    async fn notify_catalog_peers_sends_only_changed_kinds() {
        let _catalog_lock = serial_catalog();
        let (peers, client, client_service, server_handle) = connected_peer_fixture().await;

        notify_catalog_peers(
            &peers,
            CatalogNotificationChanges::new(true, false, false),
            labby_runtime::catalog_notify::SOURCE_MCP_CALL_UPSTREAM,
        )
        .await;
        client.wait_for_notifications(1).await;
        assert_eq!(client.tool_count.load(Ordering::SeqCst), 1);
        assert_eq!(client.resource_count.load(Ordering::SeqCst), 0);
        assert_eq!(client.prompt_count.load(Ordering::SeqCst), 0);

        notify_catalog_peers(
            &peers,
            CatalogNotificationChanges::new(false, true, true),
            labby_runtime::catalog_notify::SOURCE_MCP_CALL_UPSTREAM,
        )
        .await;
        client.wait_for_notifications(3).await;
        assert_eq!(client.tool_count.load(Ordering::SeqCst), 1);
        assert_eq!(client.resource_count.load(Ordering::SeqCst), 1);
        assert_eq!(client.prompt_count.load(Ordering::SeqCst), 1);

        client_service.cancel().await.expect("client cancels");
        server_handle.abort();
    }

    #[tokio::test]
    async fn notify_catalog_peers_all_false_is_noop() {
        let _catalog_lock = serial_catalog();
        let (peers, client, client_service, server_handle) = connected_peer_fixture().await;

        notify_catalog_peers(
            &peers,
            CatalogNotificationChanges::new(false, false, false),
            labby_runtime::catalog_notify::SOURCE_MCP_CALL_UPSTREAM,
        )
        .await;

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(client.total(), 0);
        assert_eq!(peers.read().await.len(), 1);

        client_service.cancel().await.expect("client cancels");
        server_handle.abort();
    }

    /// A relisted upstream whose visible tools are unchanged must remain quiet
    /// for that peer even though resources and prompts carry global signals.
    #[tokio::test]
    async fn unchanged_upstream_tool_signal_remains_suppressed_for_a_current_peer() {
        let _catalog_lock = serial_catalog();
        let peers = Arc::new(RwLock::new(Vec::new()));
        let (client, client_service, server_handle) = connect_peer(&peers, false).await;

        notify_catalog_peers(
            &peers,
            CatalogNotificationChanges::new(true, true, true),
            "upstream.subscription",
        )
        .await;

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            (
                client.tool_count.load(Ordering::SeqCst),
                client.resource_count.load(Ordering::SeqCst),
                client.prompt_count.load(Ordering::SeqCst),
            ),
            (0, 1, 1),
            "an unchanged visible tool contract must not trigger a rebuild"
        );

        client_service.cancel().await.expect("client cancels");
        server_handle.abort();
    }

    /// Regression: one trigger, two sessions, only the one whose contract
    /// actually moved is notified.
    ///
    /// A `tools_changed` trigger is a hint that something *might* have moved,
    /// not a verdict for every peer — two sessions can hold different contracts
    /// over the same gateway state (see `peer_contract.rs`). Broadcasting the
    /// hint verbatim is what made unrelated sessions rebuild their bindings.
    #[tokio::test]
    async fn notify_catalog_peers_notifies_only_peers_whose_contract_moved() {
        let _catalog_lock = serial_catalog();
        let peers = Arc::new(RwLock::new(Vec::new()));
        let (moved_client, moved_service, moved_handle) = connect_peer(&peers, true).await;
        let (unchanged_client, unchanged_service, unchanged_handle) =
            connect_peer(&peers, false).await;

        notify_catalog_peers(
            &peers,
            CatalogNotificationChanges::new(true, false, false),
            labby_runtime::catalog_notify::SOURCE_GATEWAY_RELOAD_FULL,
        )
        .await;

        moved_client.wait_for_notifications(1).await;
        assert_eq!(moved_client.tool_count.load(Ordering::SeqCst), 1);
        // Give a stray broadcast a chance to land before asserting its absence.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            unchanged_client.total(),
            0,
            "a peer whose visible contract did not move must not be told to rebuild"
        );

        // The notified peer's contract is now recorded as published, so an
        // identical trigger is a no-op for everyone.
        notify_catalog_peers(
            &peers,
            CatalogNotificationChanges::new(true, false, false),
            labby_runtime::catalog_notify::SOURCE_GATEWAY_RELOAD_FULL,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            moved_client.tool_count.load(Ordering::SeqCst),
            1,
            "publishing a contract must stop it from re-notifying every trigger"
        );
        assert_eq!(unchanged_client.total(), 0);

        moved_service.cancel().await.expect("client cancels");
        unchanged_service.cancel().await.expect("client cancels");
        moved_handle.abort();
        unchanged_handle.abort();
    }

    /// A4: a burst of triggers for one net visible change must reach the client
    /// as a single `tools/list_changed`, not one per trigger.
    #[tokio::test]
    async fn scheduled_notifications_coalesce_into_one_delivery() {
        let _catalog_lock = serial_catalog();
        reset_for_test();
        crate::mcp::catalog_churn::reset_for_test();
        let peers = Arc::new(RwLock::new(Vec::new()));
        let (client, client_service, server_handle) = connect_peer(&peers, true).await;

        // Five emitters firing in quick succession — a reconcile plus its
        // follow-on triggers, the shape that produced duplicate notifications.
        for source in [
            labby_runtime::catalog_notify::SOURCE_GATEWAY_RELOAD_FULL,
            labby_runtime::catalog_notify::SOURCE_GATEWAY_RELOAD_SELECTIVE,
            labby_runtime::catalog_notify::SOURCE_GATEWAY_ENRICH_HINT,
            labby_runtime::catalog_notify::SOURCE_MCP_CALL_CODEMODE,
            labby_runtime::catalog_notify::SOURCE_MCP_CALL_UPSTREAM,
        ] {
            schedule_catalog_notification(
                &peers,
                CatalogNotificationChanges::new(true, false, false),
                source,
            );
        }

        client.wait_for_notifications(1).await;
        // Give any second delivery the chance to arrive before asserting it didn't.
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            client.tool_count.load(Ordering::SeqCst),
            1,
            "five triggers for one net change must coalesce into one notification"
        );

        client_service.cancel().await.expect("client cancels");
        server_handle.abort();
    }

    /// A4: a notification must not land while a tool call is open — that is
    /// what invalidates the binding the caller is mid-way through using.
    #[tokio::test]
    async fn scheduled_notification_defers_until_the_turn_closes() {
        let _catalog_lock = serial_catalog();
        reset_for_test();
        crate::mcp::catalog_churn::reset_for_test();
        let peers = Arc::new(RwLock::new(Vec::new()));
        let (client, client_service, server_handle) = connect_peer(&peers, true).await;

        let call = InFlightToolCall::enter();
        schedule_catalog_notification(
            &peers,
            CatalogNotificationChanges::new(true, false, false),
            labby_runtime::catalog_notify::SOURCE_MCP_CALL_CODEMODE,
        );

        // Well past the coalesce window: without deferral this would already
        // have been delivered into the open turn.
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(
            client.tool_count.load(Ordering::SeqCst),
            0,
            "must not notify while a tool call is in flight"
        );

        drop(call);
        client.wait_for_notifications(1).await;
        assert_eq!(
            client.tool_count.load(Ordering::SeqCst),
            1,
            "the notification must be delivered once the turn closes, not dropped"
        );

        client_service.cancel().await.expect("client cancels");
        server_handle.abort();
    }

    /// Regression: dead sessions must not accumulate. Pruning used to happen
    /// only as a side effect of the notification fanout, so once the gateway
    /// stopped emitting spurious notifications the registry only ever grew.
    #[tokio::test]
    async fn closed_peers_are_pruned_and_live_ones_are_kept() {
        let _catalog_lock = serial_catalog();
        let peers = Arc::new(RwLock::new(Vec::new()));
        let (_doomed_client, doomed_service, doomed_handle) = connect_peer(&peers, true).await;
        let (_live_client, live_service, live_handle) = connect_peer(&peers, true).await;
        assert_eq!(peers.read().await.len(), 2);

        // Nothing is closed yet: a sweep must not evict a live-but-idle
        // session, which would silently cost it every future notification.
        assert_eq!(prune_closed_peers(&peers).await, 0);
        assert_eq!(peers.read().await.len(), 2);

        doomed_service.cancel().await.expect("client cancels");
        doomed_handle.abort();
        // Let the server side observe the closed transport.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let pruned = prune_closed_peers(&peers).await;
        assert_eq!(pruned, 1, "the closed session must be dropped");
        assert_eq!(
            peers.read().await.len(),
            1,
            "the live session must survive the sweep"
        );

        live_service.cancel().await.expect("client cancels");
        live_handle.abort();
    }
}
