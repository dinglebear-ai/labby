#[cfg(test)]
use rmcp::RoleServer;
#[cfg(test)]
use rmcp::service::Peer;
use rmcp::service::SubscriptionSink;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;
#[cfg(feature = "gateway")]
use tokio::sync::mpsc;

#[cfg(feature = "gateway")]
use crate::dispatch::gateway::types::{CatalogChangeEvent, GatewayCatalogDiff};

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
    /// Last contract this peer was notified about. Seeded at registration so
    /// the first diff compares against what the peer actually received in its
    /// initial `tools/list`, not against an empty set.
    pub(crate) last_contract: crate::mcp::catalog::ToolCatalogSnapshot,
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
}

impl RegisteredPeer {
    pub(crate) fn from_subscription(
        sink: SubscriptionSink,
        contract: crate::mcp::peer_contract::PeerContract,
        last_contract: crate::mcp::catalog::ToolCatalogSnapshot,
    ) -> Self {
        Self {
            registration_id: next_registration_id(),
            target: NotificationTarget::Subscription(sink),
            contract,
            last_contract,
        }
    }
}

/// Registry of live sessions, shared by every `LabMcpServer` and the notifier.
pub type PeerRegistry = Arc<RwLock<Vec<RegisteredPeer>>>;

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
                route_scope: crate::mcp::route_scope::McpRouteScope::Root,
                code_mode_app_state: Default::default(),
                audience: crate::mcp::peer_contract::PeerCatalogAudience::default(),
            },
            last_contract,
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
            crate::mcp::catalog::ToolCatalogSnapshot::from_names(std::collections::BTreeSet::new()),
        )
    }
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
}
