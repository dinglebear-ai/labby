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
//! routing on the other end), prompts, `complete`, and
//! the SEP-2663 task extension (`tasks/get`, `tasks/update`, and
//! `tasks/cancel`) plus the generic `CustomRequest` escape hatch. MRTR
//! `input_required` and task responses are forwarded without collapsing
//! them to complete-only results.
//!
//! Downstream -> daemon notifications (`cancelled`, `progress`, and custom
//! notifications) are forwarded too, so
//! cancelling a call through the bridge actually interrupts it on the real
//! daemon.
//!
//! Daemon -> downstream: `get_info()` mirrors the real daemon's actual
//! `ServerInfo` (fetched from the connection's own `peer_info()`, populated
//! by protocol discovery) instead of hand-declaring a capability
//! subset -- otherwise a downstream client could see a capability set (e.g.
//! `extensions` for mcp-ui) that doesn't match what the daemon it's
//! actually talking to supports. `BridgeClientHandler` advertises the
//! capabilities required for MRTR, and the bridge uses the one-round helpers
//! so `input_required` results are returned to the downstream client unchanged.

use std::borrow::Cow;

use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResponse, CancelTaskParams, CancelTaskRequest,
    CancelledNotificationParam, ClientInfo, ClientNotification, ClientRequest, CompleteRequest,
    CompleteRequestParams, CompleteResult, CustomNotification, CustomRequest, CustomResult,
    DiscoverResult, GetPromptRequest, GetPromptRequestParams, GetPromptResponse, GetTaskParams,
    GetTaskRequest, GetTaskResult, Implementation, InitializeRequestParams, InitializeResult,
    ListPromptsRequest, ListPromptsResult, ListResourceTemplatesRequest,
    ListResourceTemplatesResult, ListResourcesRequest, ListResourcesResult, ListToolsRequest,
    ListToolsResult, PaginatedRequestParams, ProgressNotificationParam, ProtocolVersion,
    ReadResourceRequest, ReadResourceRequestParams, ReadResourceResponse, ServerInfo,
    ServerNotification, ServerResult, SubscriptionFilter, UpdateTaskParams, UpdateTaskRequest,
};
use rmcp::service::{
    NotificationContext, Peer, PeerRequestOptions, RequestContext, RunningService, ServiceError,
    SubscriptionContext,
};
use rmcp::{ClientHandler, ErrorData, RoleClient, RoleServer, ServerHandler};

/// `ClientHandler` for the bridge's outbound connection to the real daemon.
///
/// It advertises form elicitation because the bridge can preserve an MRTR
/// `input_required` result for its downstream client. It does not implement
/// the removed server-initiated request callbacks.
#[derive(Clone)]
pub struct BridgeClientHandler;

impl BridgeClientHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BridgeClientHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientHandler for BridgeClientHandler {
    /// The bridge itself claims no fixed client capabilities. Each forwarded
    /// request carries the current downstream request metadata explicitly, so
    /// task and MRTR support are negotiated per request rather than frozen at
    /// bridge startup.
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

/// Holds the live connection to the real daemon. `_service` keeps the
/// underlying transport worker (and its `BridgeClientHandler`) alive for as
/// long as the bridge runs; `peer` is the actual handle used to forward
/// downstream requests to the daemon.
pub struct BridgeServerHandler {
    _service: RunningService<RoleClient, BridgeClientHandler>,
    peer: Peer<RoleClient>,
}

impl BridgeServerHandler {
    pub fn new(service: RunningService<RoleClient, BridgeClientHandler>) -> Self {
        let peer = service.peer().clone();
        Self {
            _service: service,
            peer,
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

impl BridgeServerHandler {
    /// Forward one downstream request with its current request metadata and
    /// cancellation token. Explicit metadata overrides the bridge connection
    /// defaults, which keeps capability negotiation request-scoped.
    async fn forward_request(
        &self,
        request: ClientRequest,
        context: &RequestContext<RoleServer>,
        action: &str,
    ) -> Result<ServerResult, ErrorData> {
        let options = PeerRequestOptions::no_options().with_meta(context.meta.clone());
        let mut handle = self
            .peer
            .send_cancellable_request(request, options)
            .await
            .map_err(|error| bridge_error(action, error))?;

        tokio::select! {
            biased;
            () = context.ct.cancelled() => {
                handle
                    .cancel(Some("downstream request cancelled".to_string()))
                    .await
                    .map_err(|error| bridge_error(action, error))?;
                Err(ErrorData::internal_error("request cancelled", None))
            }
            response = &mut handle.rx => {
                response
                    .map_err(|_| bridge_error(action, ServiceError::TransportClosed))?
                    .map_err(|error| bridge_error(action, error))
            }
        }
    }
}

impl ServerHandler for BridgeServerHandler {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        tracing::warn!(
            surface = "mcp",
            service = "labby",
            action = "bridge.lifecycle.compat_legacy_initialize",
            subsystem = "mcp_bridge",
            requested_protocol_version = %request.protocol_version,
            client_name = %request.client_info.name,
            client_version = %request.client_info.version,
            "adapting legacy MCP initialize lifecycle on stdio bridge"
        );
        context.peer.set_peer_info(request.clone());
        let mut info = self.get_info();
        info.protocol_version = request.protocol_version;
        Ok(info)
    }

    async fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, ErrorData> {
        Ok(DiscoverResult::from_server_info(
            vec![ProtocolVersion::V_2026_07_28],
            self.get_info(),
        ))
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        // The daemon validates the final filter when its subscription is
        // established; preserving the requested subset keeps the bridge
        // transparent while still letting the downstream sink enforce it.
        Some(requested.clone())
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        let mut upstream = self
            .peer
            .listen(context.accepted().clone())
            .await
            .map_err(|error| bridge_error("subscriptions.listen", error))?;
        let sink = context.sink().clone();

        loop {
            tokio::select! {
                _ = context.cancelled() => {
                    drop(upstream.cancel().await);
                    return Ok(());
                }
                notification = upstream.next() => {
                    match notification.map_err(|error| bridge_error("subscriptions.relay", error))? {
                        Some(notification @ (
                            ServerNotification::ToolListChangedNotification(_)
                            | ServerNotification::ResourceListChangedNotification(_)
                            | ServerNotification::PromptListChangedNotification(_)
                            | ServerNotification::ResourceUpdatedNotification(_)
                        )) => sink.send(notification).await.map_err(|error| {
                            ErrorData::internal_error(format!("failed to relay subscription notification: {error}"), None)
                        })?,
                        Some(_) => {}
                        None => return Ok(()),
                    }
                }
            }
        }
    }

    /// Mirror the real daemon's actual advertised `ServerInfo` -- fetched
    /// from the connection's `peer_info()`, populated by the initialize
    /// handshake when the bridge connected -- rather than hand-declaring a
    /// capability subset that could drift from what the daemon truly
    /// supports (e.g. the `extensions` capability mcp-ui widgets need).
    fn get_info(&self) -> ServerInfo {
        self.peer
            .peer_info()
            .map(|info| {
                let mut server_info = ServerInfo::new(info.capabilities.clone())
                    .with_protocol_version(info.protocol_version.clone())
                    .with_server_info(
                        info.server_info
                            .clone()
                            .unwrap_or_else(Implementation::from_build_env),
                    );
                if let Some(instructions) = info.instructions.clone() {
                    server_info = server_info.with_instructions(instructions);
                }
                server_info.meta = info.meta.clone();
                server_info
            })
            .unwrap_or_else(|| ServerInfo::new(Default::default()))
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let request = request
            .map(ListToolsRequest::with_param)
            .unwrap_or_default();
        match self
            .forward_request(
                ClientRequest::ListToolsRequest(request),
                &context,
                "list_tools",
            )
            .await?
        {
            ServerResult::ListToolsResult(result) => Ok(result),
            _ => Err(unexpected_response("list_tools")),
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        match self
            .forward_request(
                ClientRequest::CallToolRequest(CallToolRequest::new(request)),
                &context,
                "call_tool",
            )
            .await?
        {
            ServerResult::CallToolResult(result) => Ok(CallToolResponse::Complete(result)),
            ServerResult::InputRequiredResult(result) => {
                Ok(CallToolResponse::InputRequired(result))
            }
            ServerResult::CreateTaskResult(result) => Ok(CallToolResponse::Task(result)),
            _ => Err(unexpected_response("call_tool")),
        }
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let request = request
            .map(ListResourcesRequest::with_param)
            .unwrap_or_default();
        match self
            .forward_request(
                ClientRequest::ListResourcesRequest(request),
                &context,
                "list_resources",
            )
            .await?
        {
            ServerResult::ListResourcesResult(result) => Ok(result),
            _ => Err(unexpected_response("list_resources")),
        }
    }

    async fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let request = request
            .map(ListResourceTemplatesRequest::with_param)
            .unwrap_or_default();
        match self
            .forward_request(
                ClientRequest::ListResourceTemplatesRequest(request),
                &context,
                "list_resource_templates",
            )
            .await?
        {
            ServerResult::ListResourceTemplatesResult(result) => Ok(result),
            _ => Err(unexpected_response("list_resource_templates")),
        }
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        match self
            .forward_request(
                ClientRequest::ReadResourceRequest(ReadResourceRequest::new(request)),
                &context,
                "read_resource",
            )
            .await?
        {
            ServerResult::ReadResourceResult(result) => Ok(ReadResourceResponse::Complete(result)),
            ServerResult::InputRequiredResult(result) => {
                Ok(ReadResourceResponse::InputRequired(result))
            }
            _ => Err(unexpected_response("read_resource")),
        }
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let request = request
            .map(ListPromptsRequest::with_param)
            .unwrap_or_default();
        match self
            .forward_request(
                ClientRequest::ListPromptsRequest(request),
                &context,
                "list_prompts",
            )
            .await?
        {
            ServerResult::ListPromptsResult(result) => Ok(result),
            _ => Err(unexpected_response("list_prompts")),
        }
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        match self
            .forward_request(
                ClientRequest::GetPromptRequest(GetPromptRequest::new(request)),
                &context,
                "get_prompt",
            )
            .await?
        {
            ServerResult::GetPromptResult(result) => Ok(GetPromptResponse::Complete(result)),
            ServerResult::InputRequiredResult(result) => {
                Ok(GetPromptResponse::InputRequired(result))
            }
            _ => Err(unexpected_response("get_prompt")),
        }
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        match self
            .forward_request(
                ClientRequest::CompleteRequest(CompleteRequest::new(request)),
                &context,
                "complete",
            )
            .await?
        {
            ServerResult::CompleteResult(result) => Ok(result),
            _ => Err(unexpected_response("complete")),
        }
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        match self
            .forward_request(
                ClientRequest::GetTaskRequest(GetTaskRequest::new(request)),
                &context,
                "get_task",
            )
            .await?
        {
            ServerResult::GetTaskResult(result) => Ok(result),
            _ => Err(unexpected_response("get_task")),
        }
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        match self
            .forward_request(
                ClientRequest::UpdateTaskRequest(UpdateTaskRequest::new(request)),
                &context,
                "update_task",
            )
            .await?
        {
            ServerResult::TaskAckResult(_) | ServerResult::EmptyResult(_) => Ok(()),
            _ => Err(unexpected_response("update_task")),
        }
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        match self
            .forward_request(
                ClientRequest::CancelTaskRequest(CancelTaskRequest::new(request)),
                &context,
                "cancel_task",
            )
            .await?
        {
            ServerResult::TaskAckResult(_) | ServerResult::EmptyResult(_) => Ok(()),
            _ => Err(unexpected_response("cancel_task")),
        }
    }

    /// Generic escape hatch for any method neither side has typed support
    /// for. Forwarded verbatim so a downstream client and the real daemon
    /// can negotiate a custom method through the bridge transparently.
    async fn on_custom_request(
        &self,
        request: CustomRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        match self
            .forward_request(
                ClientRequest::CustomRequest(request),
                &context,
                "custom_request",
            )
            .await?
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
        let bridge_client_service: RunningService<RoleClient, BridgeClientHandler> =
            BridgeClientHandler::new()
                .serve_with_lifecycle(
                    bridge_outbound_transport,
                    ClientLifecycleMode::Discover {
                        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                    },
                )
                .await
                .expect("bridge connects to fake daemon");

        let bridge_handler = BridgeServerHandler::new(bridge_client_service);

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
