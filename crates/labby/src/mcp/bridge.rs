//! `BridgeServerHandler` — a `ServerHandler` that is itself an MCP client of
//! the real, canonical `labby serve` daemon.
//!
//! Every data-plane request this handler receives over its own transport
//! (stdio, in practice -- see `cli/serve.rs`) is forwarded verbatim to the
//! live daemon via `crate::live_gateway`, and the daemon's response is
//! returned as-is. This process builds no `GatewayManager`, no upstream
//! pool, and no local OAuth state of its own -- it's a thin pipe, not a
//! second independent gateway instance. That's what keeps a locally
//! stdio-spawned `labby` from silently diverging from the one true running
//! daemon: config, upstream connections, and OAuth refresh state all live in
//! exactly one place.
//!
//! Everything `Peer<RoleClient>` exposes a matching method for is forwarded:
//! tools, resources, prompts, `complete`, `set_level`, `subscribe`/
//! `unsubscribe`, and the `cancelled`/`progress`/`roots_list_changed`
//! notifications in both directions. The one thing that genuinely can't be
//! forwarded is task management (`enqueue_task`/`list_tasks`/`get_task_info`/
//! `get_task_result`/`cancel_task`, the SEP-1319 async-task extension) --
//! `rmcp` 2.1's `Peer<RoleClient>` has no client-side method for any of
//! these, so there is nothing to call on the remote connection without
//! hand-rolling raw JSON-RPC outside the typed API. They fall through to
//! `ServerHandler`'s defaults (`method_not_found`), which is what a client
//! would see against an `rmcp` server that doesn't implement tasks anyway.

use rmcp::model::{
    CallToolRequestParams, CallToolResult, CancelledNotificationParam, CompleteRequestParams,
    CompleteResult, GetPromptRequestParams, GetPromptResult, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProgressNotificationParam, ReadResourceRequestParams, ReadResourceResult, ServerCapabilities,
    ServerInfo, SubscribeRequestParams, UnsubscribeRequestParams,
};
#[allow(deprecated)]
use rmcp::model::SetLevelRequestParams;
use rmcp::service::{NotificationContext, Peer, RequestContext, RunningService};
use rmcp::{ErrorData, RoleClient, RoleServer, ServerHandler};

/// Holds the live connection to the real daemon. `_service` keeps the
/// underlying transport worker alive for as long as the bridge runs; `peer`
/// is the actual handle used to forward calls.
pub struct BridgeServerHandler {
    _service: RunningService<RoleClient, ()>,
    peer: Peer<RoleClient>,
}

impl BridgeServerHandler {
    pub fn new(service: RunningService<RoleClient, ()>) -> Self {
        let peer = service.peer().clone();
        Self {
            _service: service,
            peer,
        }
    }
}

fn bridge_error(action: &str, error: impl std::fmt::Display) -> ErrorData {
    tracing::warn!(
        surface = "mcp",
        service = "labby",
        action = format!("bridge.{action}"),
        subsystem = "mcp_bridge",
        error = %error,
        "bridged request to live daemon failed"
    );
    ErrorData::internal_error(format!("live daemon request failed: {error}"), None)
}

impl ServerHandler for BridgeServerHandler {
    #[allow(deprecated)]
    fn get_info(&self) -> ServerInfo {
        let builder = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_resources_subscribe()
            .enable_prompts()
            .enable_completions()
            .enable_logging();
        ServerInfo::new(builder.build())
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.peer
            .list_tools(request)
            .await
            .map_err(|e| bridge_error("list_tools", e))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.peer
            .call_tool(request)
            .await
            .map_err(|e| bridge_error("call_tool", e))
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.peer
            .list_resources(request)
            .await
            .map_err(|e| bridge_error("list_resources", e))
    }

    async fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        self.peer
            .list_resource_templates(request)
            .await
            .map_err(|e| bridge_error("list_resource_templates", e))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        self.peer
            .read_resource(request)
            .await
            .map_err(|e| bridge_error("read_resource", e))
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        self.peer
            .list_prompts(request)
            .await
            .map_err(|e| bridge_error("list_prompts", e))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, ErrorData> {
        self.peer
            .get_prompt(request)
            .await
            .map_err(|e| bridge_error("get_prompt", e))
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        self.peer
            .complete(request)
            .await
            .map_err(|e| bridge_error("complete", e))
    }

    #[allow(deprecated)]
    async fn set_level(
        &self,
        request: SetLevelRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.peer
            .set_level(request)
            .await
            .map_err(|e| bridge_error("set_level", e))
    }

    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.peer
            .subscribe(request)
            .await
            .map_err(|e| bridge_error("subscribe", e))
    }

    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.peer
            .unsubscribe(request)
            .await
            .map_err(|e| bridge_error("unsubscribe", e))
    }

    /// Forward a downstream cancellation onto the real connection so an
    /// in-flight remote call (e.g. a long-running `codemode` execution)
    /// actually gets interrupted, instead of running to completion
    /// unaffected while the caller thinks they cancelled it.
    async fn on_cancelled(
        &self,
        notification: CancelledNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) {
        if let Err(error) = self.peer.notify_cancelled(notification).await {
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "bridge.notify_cancelled",
                subsystem = "mcp_bridge",
                error = %error,
                "failed to forward cancellation to live daemon"
            );
        }
    }

    async fn on_progress(
        &self,
        notification: ProgressNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) {
        if let Err(error) = self.peer.notify_progress(notification).await {
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "bridge.notify_progress",
                subsystem = "mcp_bridge",
                error = %error,
                "failed to forward progress notification to live daemon"
            );
        }
    }

    async fn on_roots_list_changed(&self, _context: NotificationContext<RoleServer>) {
        if let Err(error) = self.peer.notify_roots_list_changed().await {
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "bridge.notify_roots_list_changed",
                subsystem = "mcp_bridge",
                error = %error,
                "failed to forward roots-list-changed notification to live daemon"
            );
        }
    }
}
