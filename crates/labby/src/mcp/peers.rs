use rmcp::RoleServer;
use rmcp::service::Peer;
use std::sync::Arc;
use tokio::sync::RwLock;
#[cfg(feature = "gateway")]
use tokio::sync::mpsc;

#[cfg(feature = "gateway")]
use crate::dispatch::gateway::types::GatewayCatalogDiff;

/// MCP-specific peer fanout that forwards catalog-change notifications to all
/// connected `rmcp::Peer<RoleServer>` instances.
///
/// This keeps `rmcp` types out of the dispatch layer while allowing
/// `GatewayManager` to notify peers when the upstream pool changes.
#[derive(Clone, Default)]
pub struct PeerNotifier {
    pub peers: Arc<RwLock<Vec<Peer<RoleServer>>>>,
    /// Live inbound MCP client/session metadata (redacted subject, declared
    /// client name/version, transport, connect time), one entry pushed per
    /// `on_initialized` call. Read by `gateway.clients.list` via
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
    pub async fn run(self, mut rx: mpsc::UnboundedReceiver<GatewayCatalogDiff>) {
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "notifier.start",
            subsystem = "mcp_server",
            phase = "peer_notifier.start",
            "starting MCP peer catalog-change notifier"
        );
        while let Some(diff) = rx.recv().await {
            self.notify_catalog_changes(&diff).await;
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

    #[cfg(feature = "gateway")]
    async fn notify_catalog_changes(&self, diff: &GatewayCatalogDiff) {
        crate::mcp::catalog_notifications::notify_catalog_peers(
            &self.peers,
            crate::mcp::catalog_notifications::CatalogNotificationChanges::new(
                diff.tools_changed,
                diff.resources_changed,
                diff.prompts_changed,
            ),
            "broadcasting catalog change to connected peers",
        )
        .await;
    }
}
