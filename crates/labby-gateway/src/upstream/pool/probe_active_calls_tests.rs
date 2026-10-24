//! A catalog timeout must not disconnect an unrelated in-flight tool effect.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use tokio::sync::Notify;

use super::UpstreamPool;
use super::testsupport::{catalog_pool_with_server, named_test_upstream_config, test_tool};

#[derive(Clone, Default)]
struct BlockedCatalogServer {
    block_catalog: Arc<AtomicBool>,
    call_started: Arc<Notify>,
    finish_call: Arc<Notify>,
}

impl ServerHandler for BlockedCatalogServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if self.block_catalog.load(Ordering::SeqCst) {
            std::future::pending().await
        } else {
            Ok(ListToolsResult::with_all_items(vec![test_tool("mutation")]))
        }
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.call_started.notify_one();
        self.finish_call.notified().await;
        Ok(CallToolResult::success(Vec::new()).into())
    }
}

#[tokio::test(start_paused = true)]
async fn failed_heartbeat_preserves_active_tool_and_exact_connection() {
    let server = BlockedCatalogServer::default();
    let pool = catalog_pool_with_server("active-effect", server.clone()).await;
    let original = pool
        .observe_connection_catalog_entry("active-effect")
        .await
        .unwrap()
        .incarnation();
    let call = tokio::spawn({
        let pool = Arc::clone(&pool);
        async move {
            pool.call_tool("active-effect", CallToolRequestParams::new("mutation"))
                .await
        }
    });
    server.call_started.notified().await;
    server.block_catalog.store(true, Ordering::SeqCst);

    let result = pool
        .reprobe_tools_for_upstream(&named_test_upstream_config("active-effect"))
        .await;
    assert!(
        matches!(result, Ok(false)),
        "busy recovery must defer: {result:?}"
    );
    assert!(!call.is_finished(), "a heartbeat cancelled the tool effect");
    assert!(
        pool.observe_connection_catalog_entry("active-effect")
            .await
            .is_some_and(|current| current.incarnation() == original),
        "the active connection was removed or replaced"
    );
    server.finish_call.notify_one();
    assert!(
        call.await.unwrap().expect("routed tool").is_ok(),
        "the original tool must complete"
    );
    pool.drain_for_swap("test.active-effect.complete").await;
}

#[tokio::test(start_paused = true)]
async fn unrelated_upstream_call_does_not_block_idle_peer_recovery() {
    let server = BlockedCatalogServer::default();
    let pool = catalog_pool_with_server("idle-failed-peer", server.clone()).await;
    let other = pool
        .acquire_upstream_call_permit("other-peer")
        .await
        .unwrap();
    server.block_catalog.store(true, Ordering::SeqCst);

    // This fixture reconnects through `true`, which cannot speak MCP. The
    // existing typed reconnect failure proves recovery was attempted.
    assert!(
        pool.reprobe_tools_for_upstream(&named_test_upstream_config("idle-failed-peer"))
            .await
            .is_err(),
        "unrelated work must not suppress this idle peer's recovery"
    );
    assert!(
        pool.observe_connection_catalog_entry("idle-failed-peer")
            .await
            .is_none(),
        "the idle failed connection must follow the existing replacement path"
    );
    drop(other);
    pool.drain_for_swap("test.idle-peer.complete").await;
}

#[tokio::test]
async fn reconnect_fence_excludes_racing_dispatch_only_for_its_upstream() {
    let pool = UpstreamPool::new().with_upstream_call_concurrency(2);
    let fence = pool
        .try_acquire_upstream_reconnect_permit("replacing-peer")
        .await
        .unwrap()
        .expect("idle peer");
    let incoming = pool.acquire_upstream_call_permit("replacing-peer");
    tokio::pin!(incoming);
    assert!(futures::poll!(&mut incoming).is_pending());
    let independent = pool
        .acquire_upstream_call_permit("other-peer")
        .await
        .unwrap();
    drop(fence);
    let active = incoming.await.unwrap();
    assert!(
        pool.try_acquire_upstream_reconnect_permit("replacing-peer")
            .await
            .unwrap()
            .is_none(),
        "one active call must prevent replacement even when another slot is free"
    );
    drop(active);
    drop(independent);
    assert!(
        pool.try_acquire_upstream_reconnect_permit("replacing-peer")
            .await
            .unwrap()
            .is_some()
    );
}
