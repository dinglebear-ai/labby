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
//! ## What's forwarded
//!
//! Downstream -> daemon: tools, resources (including `ui://` mcp-ui
//! resources -- the bridge has no URI-scheme awareness of its own; it just
//! forwards `read_resource` verbatim and the daemon does its normal `ui://`
//! routing on the other end), prompts, `complete`, `set_level`,
//! `subscribe`/`unsubscribe`. The one thing that genuinely can't be
//! forwarded is task management (`enqueue_task`/`list_tasks`/`get_task_info`/
//! `get_task_result`/`cancel_task`, the SEP-1319 async-task extension) --
//! `rmcp` 2.1's `Peer<RoleClient>` has no client-side method for any of
//! these, so there is nothing to call on the remote connection without
//! hand-rolling raw JSON-RPC outside the typed API. They fall through to
//! `ServerHandler`'s defaults (`method_not_found`).
//!
//! Downstream -> daemon (notifications): `cancelled`/`progress`/
//! `roots_list_changed` are forwarded too, so cancelling a call through the
//! bridge actually interrupts it on the real daemon.
//!
//! Daemon -> downstream: `get_info()` mirrors the real daemon's actual
//! `ServerInfo` (fetched from the connection's own `peer_info()`, populated
//! by the initialize handshake) instead of hand-declaring a capability
//! subset -- otherwise a downstream client could see a capability set (e.g.
//! `extensions` for mcp-ui) that doesn't match what the daemon it's
//! actually talking to supports. `BridgeClientHandler` relays the daemon's
//! server->client requests (elicitation, sampling, roots) down to the one
//! downstream peer connected to this bridge, mirroring
//! `labby-gateway`'s `RelayClientHandler` -- just for a single long-lived
//! connection instead of a per-call dedicated one, since stdio mode serves
//! exactly one downstream session for the whole process lifetime.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, CancelledNotificationParam, ClientInfo,
    CompleteRequestParams, CompleteResult, ElicitRequestParams, ElicitResult,
    GetPromptRequestParams, GetPromptResult, ListPromptsResult, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProgressNotificationParam,
    ReadResourceRequestParams, ReadResourceResult, ServerInfo, SubscribeRequestParams,
    UnsubscribeRequestParams,
};
// rmcp 2.1 deprecates sampling/roots/logging under SEP-2577, but the bridge
// still forwards these legacy server<->client requests for compatibility
// with whatever the daemon and downstream client actually negotiate.
#[allow(deprecated)]
use rmcp::model::{CreateMessageRequestParams, CreateMessageResult, ListRootsResult};
#[allow(deprecated)]
use rmcp::model::SetLevelRequestParams;
use rmcp::service::{NotificationContext, Peer, RequestContext, RunningService};
use rmcp::{ClientHandler, ErrorData, RoleClient, RoleServer, ServerHandler};
use tokio::sync::OnceCell;

/// Shared cell holding the one downstream peer connected to this bridge,
/// populated once that peer's session initializes. Stdio mode serves
/// exactly one downstream session for the process's whole lifetime, so a
/// single cell -- not a per-connection map like the gateway's multiplexed
/// `RelayClientHandler` needs -- is exactly right.
pub type DownstreamCell = Arc<OnceCell<Peer<RoleServer>>>;

/// `ClientHandler` for the bridge's outbound connection to the real daemon.
///
/// Forwards any server->client request the daemon raises mid-call
/// (elicitation, sampling, roots) down to the one downstream peer connected
/// to this bridge. Without this, the connection would use the unit handler
/// `()` and silently decline all three -- severing exactly the half of MCP
/// that lets an upstream tool call (e.g. a `codemode` execution needing
/// human confirmation) actually reach the human on the other end of the
/// bridge.
#[derive(Clone)]
pub struct BridgeClientHandler {
    downstream: DownstreamCell,
}

impl BridgeClientHandler {
    pub fn new(downstream: DownstreamCell) -> Self {
        Self { downstream }
    }
}

fn relay_error(capability: &str, error: impl std::fmt::Display) -> ErrorData {
    tracing::warn!(
        surface = "mcp",
        service = "labby",
        action = "bridge.relay",
        subsystem = "mcp_bridge",
        capability,
        error = %error,
        "relaying the live daemon's server->client request to the downstream peer failed"
    );
    ErrorData::internal_error(
        format!("relay of {capability} to downstream peer failed"),
        None,
    )
}

impl ClientHandler for BridgeClientHandler {
    /// Advertise to the daemon only the server->client capabilities the
    /// downstream peer itself declared -- mirroring
    /// `RelayClientHandler::get_info`. If the downstream peer hasn't
    /// initialized yet (or none is connected), this advertises no
    /// server->client capabilities, so the daemon will not attempt to elicit
    /// against a bridge with nothing to forward to.
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        if let Some(downstream) = self.downstream.get()
            && let Some(downstream_info) = downstream.peer_info()
        {
            info.capabilities.elicitation = downstream_info.capabilities.elicitation.clone();
            info.capabilities.sampling = downstream_info.capabilities.sampling.clone();
            info.capabilities.roots = downstream_info.capabilities.roots.clone();
        }
        info
    }

    async fn create_elicitation(
        &self,
        params: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        let Some(downstream) = self.downstream.get() else {
            return Err(ErrorData::internal_error(
                "no downstream peer connected yet",
                None,
            ));
        };
        downstream
            .create_elicitation(params)
            .await
            .map_err(|e| relay_error("elicitation", e))
    }

    #[allow(deprecated)]
    async fn create_message(
        &self,
        params: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateMessageResult, ErrorData> {
        let Some(downstream) = self.downstream.get() else {
            return Err(ErrorData::internal_error(
                "no downstream peer connected yet",
                None,
            ));
        };
        downstream
            .create_message(params)
            .await
            .map_err(|e| relay_error("sampling", e))
    }

    #[allow(deprecated)]
    async fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> Result<ListRootsResult, ErrorData> {
        let Some(downstream) = self.downstream.get() else {
            return Err(ErrorData::internal_error(
                "no downstream peer connected yet",
                None,
            ));
        };
        downstream
            .list_roots()
            .await
            .map_err(|e| relay_error("roots", e))
    }
}

/// Holds the live connection to the real daemon. `_service` keeps the
/// underlying transport worker (and its `BridgeClientHandler`) alive for as
/// long as the bridge runs; `peer` is the actual handle used to forward
/// downstream requests to the daemon.
pub struct BridgeServerHandler {
    _service: RunningService<RoleClient, BridgeClientHandler>,
    peer: Peer<RoleClient>,
    downstream: DownstreamCell,
}

impl BridgeServerHandler {
    /// `downstream` must be the same cell passed to the `BridgeClientHandler`
    /// used to open `service`, so the daemon's server->client requests and
    /// this handler's own `on_initialized` agree on which peer to relay to.
    pub fn new(
        service: RunningService<RoleClient, BridgeClientHandler>,
        downstream: DownstreamCell,
    ) -> Self {
        let peer = service.peer().clone();
        Self {
            _service: service,
            peer,
            downstream,
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
    /// Mirror the real daemon's actual advertised `ServerInfo` -- fetched
    /// from the connection's `peer_info()`, populated by the initialize
    /// handshake when the bridge connected -- rather than hand-declaring a
    /// capability subset that could drift from what the daemon truly
    /// supports (e.g. the `extensions` capability mcp-ui widgets need).
    fn get_info(&self) -> ServerInfo {
        self.peer
            .peer_info()
            .map(|info| (*info).clone())
            .unwrap_or_default()
    }

    /// Capture the one downstream peer this bridge serves, so
    /// `BridgeClientHandler` has someone to relay the daemon's
    /// elicitation/sampling/roots requests to. Fires once per stdio session
    /// (there is only ever one), right after `initialize` -- well before any
    /// real tool call could raise an elicitation.
    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        if self.downstream.set(context.peer).is_err() {
            tracing::debug!(
                surface = "mcp",
                service = "labby",
                action = "bridge.on_initialized",
                subsystem = "mcp_bridge",
                "downstream peer already set; ignoring duplicate initialized notification"
            );
        }
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
