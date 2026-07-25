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
//! routing on the other end), prompts, `complete`, `set_level`, and
//! the SEP-2663 task extension (`tasks/get`, `tasks/update`, and
//! `tasks/cancel`) plus the generic `CustomRequest` escape hatch. MRTR
//! `input_required` and task responses are forwarded without collapsing
//! them to complete-only results.
//!
//! Downstream -> daemon notifications (`cancelled`, `progress`,
//! `roots_list_changed`, and custom notifications) are forwarded too, so
//! cancelling a call through the bridge actually interrupts it on the real
//! daemon.
//!
//! Daemon -> downstream: `get_info()` mirrors the real daemon's actual
//! `ServerInfo` (fetched from the connection's own `peer_info()`, populated
//! by protocol discovery) instead of hand-declaring a capability
//! subset -- otherwise a downstream client could see a capability set (e.g.
//! `extensions` for mcp-ui) that doesn't match what the daemon it's
//! actually talking to supports. `BridgeClientHandler` relays the daemon's
//! server->client requests (elicitation, sampling, roots) down to the one
//! downstream peer connected to this bridge, mirroring
//! `labby-gateway`'s `RelayClientHandler` -- just for a single long-lived
//! connection instead of a per-call dedicated one, since stdio mode serves
//! exactly one downstream session for the whole process lifetime.

use std::borrow::Cow;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CancelTaskParams, CancelledNotificationParam,
    ClientInfo, ClientNotification, ClientRequest, CompleteRequestParams, CompleteResult,
    CustomNotification, CustomRequest, CustomResult, DiscoverResult, ElicitRequestParams,
    ElicitResult, GetPromptRequestParams, GetPromptResponse, GetTaskParams, GetTaskResult,
    InitializeRequestParams, InitializeResult, ListPromptsResult, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, PingRequest,
    ProgressNotificationParam, ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse,
    ServerInfo, ServerResult, UpdateTaskParams,
};
// rmcp deprecates sampling/roots/logging under SEP-2577, but the bridge
// still forwards these legacy server<->client requests for compatibility
// with whatever the daemon and downstream client actually negotiate.
#[allow(deprecated)]
use rmcp::model::SetLevelRequestParams;
#[allow(deprecated)]
use rmcp::model::{CreateMessageRequestParams, CreateMessageResult, ListRootsResult};
use rmcp::service::{NotificationContext, Peer, RequestContext, RunningService, ServiceError};
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
        info.capabilities = rmcp::model::ClientCapabilities::builder()
            .enable_tasks()
            .build();
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
    /// this handler's lifecycle callback agree on which peer to relay to.
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

fn bridge_error(action: &str, error: ServiceError) -> ErrorData {
    if let ServiceError::McpError(error) = error {
        tracing::warn!(
            surface = "mcp",
            service = "labby",
            action = format!("bridge.{action}"),
            subsystem = "mcp_bridge",
            error_code = error.code.0,
            "bridged request to live daemon failed"
        );
        return error;
    }

    tracing::warn!(
        surface = "mcp",
        service = "labby",
        action = format!("bridge.{action}"),
        subsystem = "mcp_bridge",
        error_kind = "bridge_transport_error",
        "bridged request to live daemon failed"
    );
    ErrorData::internal_error("live daemon request failed", None)
}

/// The live daemon replied to a raw `send_request` with a `ServerResult`
/// variant other than the one the wire method promises -- e.g. anything but
/// `CreateTaskResult` for a task-mode `tools/call`. Not expected in
/// practice; only reachable if the daemon itself violates the SEP-1319
/// contract.
fn unexpected_response(action: &str) -> ErrorData {
    tracing::warn!(
        surface = "mcp",
        service = "labby",
        action = format!("bridge.{action}"),
        subsystem = "mcp_bridge",
        error_kind = "unexpected_response",
        "live daemon returned an unexpected result type"
    );
    ErrorData::internal_error("live daemon returned an unexpected result type", None)
}

impl ServerHandler for BridgeServerHandler {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        Err(ErrorData::new(
            rmcp::model::ErrorCode::METHOD_NOT_FOUND,
            "legacy initialize lifecycle is not supported; use server/discover",
            None,
        ))
    }

    async fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, ErrorData> {
        drop(self.downstream.set(context.peer));
        Ok(DiscoverResult::from_server_info(
            vec![ProtocolVersion::V_2026_07_28],
            self.get_info(),
        ))
    }

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
    ) -> Result<CallToolResponse, ErrorData> {
        self.peer
            .call_tool_once(request)
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
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.peer
            .read_resource_once(request)
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
    ) -> Result<GetPromptResponse, ErrorData> {
        self.peer
            .get_prompt_once(request)
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

    /// `Peer<RoleClient>` has no typed `ping` shortcut, so build the
    /// `PingRequest` and match the raw `ServerResult` by hand -- the same
    /// thing the typed convenience methods do internally.
    async fn ping(&self, _context: RequestContext<RoleServer>) -> Result<(), ErrorData> {
        match self
            .peer
            .send_request(ClientRequest::PingRequest(PingRequest::default()))
            .await
            .map_err(|e| bridge_error("ping", e))?
        {
            ServerResult::EmptyResult(_) => Ok(()),
            _ => Err(unexpected_response("ping")),
        }
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        self.peer
            .get_task(request)
            .await
            .map_err(|e| bridge_error("get_task", e))
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.peer
            .update_task(request)
            .await
            .map_err(|e| bridge_error("update_task", e))
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.peer
            .cancel_task(request)
            .await
            .map_err(|e| bridge_error("cancel_task", e))
    }

    /// Generic escape hatch for any method neither side has typed support
    /// for. Forwarded verbatim so a downstream client and the real daemon
    /// can negotiate a custom method through the bridge transparently.
    async fn on_custom_request(
        &self,
        request: CustomRequest,
        _context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        match self
            .peer
            .send_request(ClientRequest::CustomRequest(request))
            .await
            .map_err(|e| bridge_error("custom_request", e))?
        {
            ServerResult::CustomResult(result) => Ok(result),
            _ => Err(unexpected_response("custom_request")),
        }
    }

    async fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _context: NotificationContext<RoleServer>,
    ) {
        if let Err(error) = self
            .peer
            .send_notification(ClientNotification::CustomNotification(notification))
            .await
        {
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "bridge.on_custom_notification",
                subsystem = "mcp_bridge",
                error = %error,
                "failed to forward custom notification to live daemon"
            );
        }
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

#[cfg(test)]
mod tests {
    //! End-to-end proof that `BridgeServerHandler`/`BridgeClientHandler`
    //! actually forward across two independent in-memory connections, rather
    //! than short-circuiting.
    //!
    //! Topology (mirrors `labby-gateway`'s `RelayClientHandler` tests in
    //! `crates/labby-gateway/src/upstream/pool/relay.rs`, and the
    //! `connect_service` wiring in `crate::live_gateway`):
    //!
    //! ```text
    //! TestClient --(duplex #2)--> BridgeServerHandler --(duplex #1)--> FakeDaemonHandler
    //! ```
    //!
    //! `duplex #1` is the bridge's outbound connection to the "live daemon",
    //! served with `BridgeClientHandler` as the `ClientHandler`. `duplex #2`
    //! is the bridge's own inbound transport, served with
    //! `BridgeServerHandler`. A bare test-only `ClientHandler` drives that
    //! second connection to exercise every forwarded request/response path.
    use std::sync::Arc;

    use rmcp::model::{
        CancelTaskParams, CancelTaskRequest, ClientCapabilities, CustomRequest, DetailedTask,
        ErrorData as McpError, GetTaskParams, GetTaskRequest, ServerCapabilities, ServerInfo, Task,
        TaskPayload, TaskStatus,
    };
    use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RequestContext, RunningService};
    use rmcp::{ClientHandler, RoleClient, RoleServer, ServerHandler, ServiceExt};
    use serde_json::json;

    use super::*;

    const IN_PROCESS_PEER_BUFFER_BYTES: usize = 256 * 1024;

    /// Canonical fake task id, asserted verbatim end-to-end to prove the
    /// data actually crossed both hops rather than being stubbed locally.
    const FAKE_TASK_ID: &str = "fake-task-42";

    #[test]
    fn bridge_error_preserves_daemon_mcp_error_code_message_and_data() {
        let daemon_error = ErrorData::invalid_params(
            "missing required parameter `query`",
            Some(json!({
                "kind": "missing_param",
                "param": "query"
            })),
        );

        let bridged = bridge_error("call_tool", ServiceError::McpError(daemon_error.clone()));

        assert_eq!(bridged.code, daemon_error.code);
        assert_eq!(bridged.message, daemon_error.message);
        assert_eq!(bridged.data, daemon_error.data);
    }

    fn fake_task() -> Task {
        Task::new(
            FAKE_TASK_ID.to_string(),
            TaskStatus::Working,
            "2026-01-01T00:00:00Z".to_string(),
            "2026-01-01T00:00:01Z".to_string(),
        )
        .with_status_message("doing the fake thing")
    }

    /// Minimal fake "live daemon" `ServerHandler`. Answers every forwarded
    /// method deterministically so the tests can assert exact round-trip
    /// fidelity through the bridge instead of a no-op stub.
    #[derive(Clone)]
    struct FakeDaemonHandler;

    impl ServerHandler for FakeDaemonHandler {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_tasks()
                    .build(),
            )
        }

        async fn ping(&self, _context: RequestContext<RoleServer>) -> Result<(), McpError> {
            Ok(())
        }

        async fn get_task(
            &self,
            request: GetTaskParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetTaskResult, McpError> {
            // Prove the request itself made it through: echo the requested
            // task id back in the status message instead of always
            // returning the same canned task.
            let mut task = fake_task();
            task.status_message = Some(format!("info-for:{}", request.task_id));
            Ok(GetTaskResult::new(DetailedTask::new(
                task,
                TaskPayload::Working,
            )))
        }

        async fn cancel_task(
            &self,
            request: CancelTaskParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<(), McpError> {
            if request.task_id == FAKE_TASK_ID {
                Ok(())
            } else {
                Err(McpError::invalid_params("unexpected task id", None))
            }
        }

        async fn on_custom_request(
            &self,
            request: CustomRequest,
            _context: RequestContext<RoleServer>,
        ) -> Result<CustomResult, McpError> {
            Ok(CustomResult::new(serde_json::json!({
                "echoed_method": request.method,
                "echoed_params": request.params,
            })))
        }
    }

    /// Bare test-only `ClientHandler` for the downstream (bridge-facing)
    /// side. The tests only ever *call* methods on the bridge, never receive
    /// server->client requests, so no elicitation/sampling/roots relay is
    /// needed here -- that half is covered by `BridgeClientHandler`'s own
    /// unit-level behavior and by `labby-gateway`'s `RelayClientHandler`
    /// tests for the analogous relay path.
    #[derive(Clone)]
    struct TestDownstreamClient;

    impl ClientHandler for TestDownstreamClient {
        fn get_info(&self) -> ClientInfo {
            let mut info = ClientInfo::default();
            info.capabilities = ClientCapabilities::builder().enable_tasks().build();
            info
        }
    }

    /// A live bridge topology: the test client's peer for driving requests,
    /// plus the two `RunningService`s that must stay alive for the duration
    /// of the test. Dropping either tears down its transport (the bridge's
    /// connection to the fake daemon is kept alive internally by
    /// `BridgeServerHandler::_service`, so it doesn't need a separate
    /// binding here).
    struct BridgeHarness {
        peer: Peer<RoleClient>,
        _client_service: RunningService<RoleClient, TestDownstreamClient>,
        _bridge_service: RunningService<RoleServer, BridgeServerHandler>,
    }

    /// Wires up the full two-hop bridge topology:
    /// test client -> `BridgeServerHandler` -> `BridgeClientHandler` -> fake daemon.
    async fn wire_bridge() -> BridgeHarness {
        // Hop 1: bridge -> fake daemon, served with `BridgeClientHandler` so
        // the daemon's server->client requests would be relayed (unused by
        // these tests, but this is the real production wiring shape from
        // `live_gateway::connect_service`).
        let (daemon_transport, bridge_outbound_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        tokio::spawn(async move {
            if let Ok(running) = FakeDaemonHandler.serve(daemon_transport).await {
                running.waiting().await.ok();
            }
        });
        let downstream_cell: DownstreamCell = Arc::new(OnceCell::new());
        let bridge_client_service: RunningService<RoleClient, BridgeClientHandler> =
            BridgeClientHandler::new(downstream_cell.clone())
                .serve_with_lifecycle(
                    bridge_outbound_transport,
                    ClientLifecycleMode::Discover {
                        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                    },
                )
                .await
                .expect("bridge connects to fake daemon");

        let bridge_handler = BridgeServerHandler::new(bridge_client_service, downstream_cell);

        // Hop 2: test client -> bridge, served with the bridge's own
        // `ServerHandler` impl over its own independent in-memory transport.
        // Both `serve()` calls perform the `initialize` handshake with each
        // other over the same duplex pair, so they must run concurrently --
        // awaiting one before starting the other deadlocks forever waiting
        // for a response nobody has sent yet.
        let (bridge_inbound_transport, client_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let (bridge_service, client_service) = tokio::join!(
            bridge_handler.serve(bridge_inbound_transport),
            TestDownstreamClient.serve_with_lifecycle(
                client_transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            ),
        );
        let bridge_service: RunningService<RoleServer, BridgeServerHandler> =
            bridge_service.expect("test client connects to bridge");
        let client_service: RunningService<RoleClient, TestDownstreamClient> =
            client_service.expect("test client transport connects");
        let peer = client_service.peer().clone();

        BridgeHarness {
            peer,
            _client_service: client_service,
            _bridge_service: bridge_service,
        }
    }

    #[tokio::test]
    async fn get_task_reaches_the_daemon_with_the_requested_id() {
        let harness = wire_bridge().await;

        let result = match harness
            .peer
            .send_request(ClientRequest::GetTaskRequest(GetTaskRequest::new(
                GetTaskParams::new(FAKE_TASK_ID),
            )))
            .await
            .expect("get_task round-trips through the bridge")
        {
            ServerResult::GetTaskResult(result) => result,
            other => panic!("expected GetTaskResult, got {other:?}"),
        };

        assert_eq!(result.task.task.task_id, FAKE_TASK_ID);
        // The daemon's fake handler stamps the requested task id into the
        // status message, proving the request params -- not just the
        // response -- crossed the bridge intact.
        assert_eq!(
            result.task.task.status_message.as_deref(),
            Some(format!("info-for:{FAKE_TASK_ID}").as_str())
        );
    }

    #[tokio::test]
    async fn cancel_task_reaches_the_daemon_and_returns_acknowledgement() {
        let harness = wire_bridge().await;

        match harness
            .peer
            .send_request(ClientRequest::CancelTaskRequest(CancelTaskRequest::new(
                CancelTaskParams::new(FAKE_TASK_ID),
            )))
            .await
            .expect("cancel_task round-trips through the bridge")
        {
            ServerResult::TaskAckResult(_) | ServerResult::EmptyResult(_) => {}
            other => panic!("expected task acknowledgement, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn custom_request_round_trips_method_and_params_through_the_daemon() {
        let harness = wire_bridge().await;

        let response = harness
            .peer
            .send_request(ClientRequest::CustomRequest(CustomRequest::new(
                "x-lab/probe",
                Some(serde_json::json!({"hello": "world"})),
            )))
            .await
            .expect("custom request round-trips through the bridge");

        let CustomResult(value) = match response {
            ServerResult::CustomResult(result) => result,
            other => panic!("expected CustomResult, got {other:?}"),
        };

        assert_eq!(value["echoed_method"], "x-lab/probe");
        assert_eq!(value["echoed_params"]["hello"], "world");
    }
}
