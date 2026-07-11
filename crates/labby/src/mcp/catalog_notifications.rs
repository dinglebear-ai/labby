use std::sync::Arc;

use futures::future::join_all;
use rmcp::RoleServer;
use rmcp::service::Peer;
use tokio::sync::RwLock;

use crate::mcp::catalog::CatalogChangeSet;

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
    use std::future::Future;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use rmcp::service::{MaybeSendFuture, NotificationContext};
    use rmcp::{ClientHandler, RoleClient, ServerHandler, ServiceExt};
    use tokio::sync::{Notify, RwLock};

    use super::{CatalogNotificationChanges, notify_catalog_peers};
    use crate::mcp::catalog::CatalogChangeSet;

    #[derive(Clone)]
    struct TestServer {
        peers: Arc<RwLock<Vec<rmcp::service::Peer<rmcp::RoleServer>>>>,
    }

    impl ServerHandler for TestServer {
        fn on_initialized(
            &self,
            context: NotificationContext<rmcp::RoleServer>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            let peers = Arc::clone(&self.peers);
            let peer = context.peer.clone();
            async move {
                peers.write().await.push(peer);
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
        Arc<RwLock<Vec<rmcp::service::Peer<rmcp::RoleServer>>>>,
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
        let server = TestServer {
            peers: Arc::clone(&peers),
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
            while peers.read().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("server peer registered");

        (peers, client, client_service, server_handle)
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
        let (peers, client, client_service, server_handle) = connected_peer_fixture().await;

        notify_catalog_peers(
            &peers,
            CatalogNotificationChanges::new(true, false, false),
            "test notify",
        )
        .await;
        client.wait_for_notifications(1).await;
        assert_eq!(client.tool_count.load(Ordering::SeqCst), 1);
        assert_eq!(client.resource_count.load(Ordering::SeqCst), 0);
        assert_eq!(client.prompt_count.load(Ordering::SeqCst), 0);

        notify_catalog_peers(
            &peers,
            CatalogNotificationChanges::new(false, true, true),
            "test notify",
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
        let (peers, client, client_service, server_handle) = connected_peer_fixture().await;

        notify_catalog_peers(
            &peers,
            CatalogNotificationChanges::new(false, false, false),
            "test notify",
        )
        .await;

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(client.total(), 0);
        assert_eq!(peers.read().await.len(), 1);

        client_service.cancel().await.expect("client cancels");
        server_handle.abort();
    }
}
