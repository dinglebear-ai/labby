use futures::future::join_all;
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
        let peers = self.peers.read().await.clone();
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "catalog.notify",
            subsystem = "mcp_server",
            phase = "catalog.notify",
            peer_count = peers.len(),
            tools_changed = diff.tools_changed,
            resources_changed = diff.resources_changed,
            prompts_changed = diff.prompts_changed,
            "broadcasting catalog change to connected peers"
        );

        // Notify all peers concurrently so one slow peer cannot stall the
        // fanout. Each peer future is bounded by the configured notification
        // timeout so a hung session times out independently.
        let notification_timeout = crate::config::resolved_catalog_notification_timeout();
        let notify_futures = peers.iter().enumerate().map(|(index, peer)| {
            let peer = peer.clone();
            let diff = diff.clone();
            async move {
                let result = tokio::time::timeout(notification_timeout, async {
                    if diff.tools_changed && peer.notify_tool_list_changed().await.is_err() {
                        tracing::warn!(
                            surface = "mcp",
                            service = "peers",
                            action = "peer.disconnect",
                            peer_index = index,
                            phase = "tools",
                            tools_changed = diff.tools_changed,
                            resources_changed = diff.resources_changed,
                            prompts_changed = diff.prompts_changed,
                            "failed to notify peer about catalog change; pruning stale session"
                        );
                        return false;
                    }
                    if diff.resources_changed && peer.notify_resource_list_changed().await.is_err()
                    {
                        tracing::warn!(
                            surface = "mcp",
                            service = "peers",
                            action = "peer.disconnect",
                            peer_index = index,
                            phase = "resources",
                            tools_changed = diff.tools_changed,
                            resources_changed = diff.resources_changed,
                            prompts_changed = diff.prompts_changed,
                            "failed to notify peer about catalog change; pruning stale session"
                        );
                        return false;
                    }
                    if diff.prompts_changed && peer.notify_prompt_list_changed().await.is_err() {
                        tracing::warn!(
                            surface = "mcp",
                            service = "peers",
                            action = "peer.disconnect",
                            peer_index = index,
                            phase = "prompts",
                            tools_changed = diff.tools_changed,
                            resources_changed = diff.resources_changed,
                            prompts_changed = diff.prompts_changed,
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
                            peer_index = index,
                            timeout_ms = notification_timeout.as_millis(),
                            tools_changed = diff.tools_changed,
                            resources_changed = diff.resources_changed,
                            prompts_changed = diff.prompts_changed,
                            "peer notification timed out; pruning stale session"
                        );
                        false
                    }
                }
            }
        });

        let snapshot_len = peers.len();
        let results = join_all(notify_futures).await;
        let alive: Vec<Peer<RoleServer>> = peers
            .into_iter()
            .zip(results)
            .filter_map(|(peer, ok)| ok.then_some(peer))
            .collect();

        let pruned = snapshot_len.saturating_sub(alive.len());

        let mut guard = self.peers.write().await;
        // Preserve peers that connected after we took the snapshot so they are
        // not incorrectly GC'd — identical to the original serial logic.
        let added_since_snapshot = guard.split_off(snapshot_len);
        *guard = alive;
        guard.extend(added_since_snapshot);
        let total = guard.len();
        if pruned > 0 {
            tracing::info!(
                surface = "mcp",
                service = "peers",
                action = "peer.gc",
                pruned_count = pruned,
                active_count = total,
                "pruned stale MCP peer sessions after catalog notify",
            );
        } else {
            tracing::debug!(
                surface = "mcp",
                service = "peers",
                action = "peer.gc",
                active_count = total,
                "catalog notify complete — all peers alive",
            );
        }
    }
}
