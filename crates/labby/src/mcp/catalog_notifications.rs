use std::sync::Arc;

use futures::future::join_all;
use rmcp::RoleServer;
use rmcp::service::Peer;
use tokio::sync::RwLock;

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
}

pub(crate) async fn notify_catalog_peers(
    peers: &Arc<RwLock<Vec<Peer<RoleServer>>>>,
    changes: CatalogNotificationChanges,
    log_message: &'static str,
) {
    if !changes.any() {
        return;
    }

    let peer_snapshot = peers.read().await.clone();
    let peer_count = peer_snapshot.len();
    tracing::info!(
        surface = "mcp",
        service = "peers",
        action = "catalog.notify",
        subsystem = "mcp_server",
        phase = "catalog.notify",
        peer_count,
        tools_changed = changes.tools_changed,
        resources_changed = changes.resources_changed,
        prompts_changed = changes.prompts_changed,
        "{log_message}"
    );

    let notification_timeout = crate::config::resolved_catalog_notification_timeout();
    let notify_futures = peer_snapshot.iter().enumerate().map(|(peer_index, peer)| {
        let peer = peer.clone();
        async move {
            let result = tokio::time::timeout(notification_timeout, async {
                if changes.tools_changed && peer.notify_tool_list_changed().await.is_err() {
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
                if changes.resources_changed && peer.notify_resource_list_changed().await.is_err() {
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
                if changes.prompts_changed && peer.notify_prompt_list_changed().await.is_err() {
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
    let alive: Vec<_> = peer_snapshot
        .into_iter()
        .zip(results)
        .filter_map(|(peer, ok)| ok.then_some(peer))
        .collect();

    let alive_count = alive.len();
    let mut guard = peers.write().await;
    let added_since_snapshot = if guard.len() > peer_count {
        guard.split_off(peer_count)
    } else {
        Vec::new()
    };
    *guard = alive;
    guard.extend(added_since_snapshot);
    let pruned = peer_count.saturating_sub(alive_count);
    tracing::info!(
        surface = "mcp",
        service = "peers",
        action = "peer.gc",
        pruned_count = pruned,
        active_count = guard.len(),
        "MCP peer catalog-change notification complete"
    );
}

#[cfg(test)]
mod tests {
    use super::CatalogNotificationChanges;

    #[test]
    fn catalog_notification_changes_reports_any_changed_kind() {
        assert!(!CatalogNotificationChanges::new(false, false, false).any());
        assert!(CatalogNotificationChanges::new(true, false, false).any());
        assert!(CatalogNotificationChanges::new(false, true, false).any());
        assert!(CatalogNotificationChanges::new(false, false, true).any());
    }
}
