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
//! Only forwarded: the operations real MCP clients actually exercise in
//! practice (tools, resources, prompts). `set_level`/`complete`/task
//! management fall through to `ServerHandler`'s defaults rather than being
//! wired through -- they're rarely used and the default behavior (declining
//! or no-op) is a reasonable placeholder; wire them through here if a real
//! client needs them forwarded too.

use rmcp::model::{
    CallToolRequestParams, CallToolResult, GetPromptRequestParams, GetPromptResult,
    ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResult, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::{Peer, RequestContext, RunningService};
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
    fn get_info(&self) -> ServerInfo {
        let builder = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_prompts()
            .enable_completions();
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
}
