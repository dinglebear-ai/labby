//! Relaying `ClientHandler` for upstream server→client requests.
//!
//! The pool's normal upstream connections are served with the unit handler
//! (`().serve(...)`), which advertises no client capabilities and declines any
//! `elicitation/create`, `sampling/createMessage`, or `roots/list` request a
//! server sends back. That severs the server→client half of MCP: an upstream
//! that needs interactive confirmation (elicitation), an LLM completion
//! (sampling), or the caller's roots cannot reach the agent driving the
//! gateway.
//!
//! [`RelayClientHandler`] is the bridge. Each instance closes over the single
//! downstream `Peer<RoleServer>` — the agent connection whose in-flight
//! `call_tool` opened this upstream connection — and forwards server→client
//! requests straight down to it. The relay therefore only makes sense on a
//! **dedicated, non-multiplexed** upstream connection: one connection per
//! in-flight downstream call, so an upstream elicitation maps unambiguously to
//! the one agent that should answer it. A pooled connection shared by many
//! agents has no single "current" downstream peer to forward to — which is
//! exactly why the existing pool path uses `()` and this is opt-in.
//!
//! ## Capability mirroring
//!
//! `get_info()` advertises to the upstream only the server→client capabilities
//! the downstream agent itself declared (elicitation / sampling / roots). If
//! the agent cannot elicit, the gateway does not claim it can, so a well-behaved
//! upstream will not attempt it. This keeps the proxied capability set honest
//! end to end instead of advertising support the gateway cannot actually honor.
//!
//! ## Live entry point
//!
//! [`UpstreamPool::call_tool_relayed`] opens a dedicated connection via the
//! generic `connect_upstream_with_handler` seam (so HTTP, WebSocket, stdio, and
//! OAuth all reuse the existing transport + process-reaping machinery), invokes
//! one tool with the relay handler installed, and shuts the connection down. The
//! MCP raw-proxy path calls it (behind an opt-in env gate) when the downstream
//! agent advertises elicitation. Cost: one fresh connect per call — the gate
//! keeps that off the default hot path.

use std::sync::Arc;
use std::time::Instant;

use rmcp::ErrorData as McpError;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientInfo, CreateElicitationRequestParams,
    CreateElicitationResult, CreateMessageRequestParams, CreateMessageResult, ListRootsResult,
};
use rmcp::service::{Peer, RequestContext};
use rmcp::{ClientHandler, RoleClient, RoleServer};

use crate::config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::connect::connect_upstream_with_handler;

/// A client handler that relays an upstream server's server→client requests
/// (elicitation, sampling, roots) down to the gateway's downstream agent peer.
///
/// Construct one per dedicated upstream connection with [`RelayClientHandler::new`].
#[derive(Clone)]
pub(crate) struct RelayClientHandler {
    /// The downstream agent connection to forward requests to.
    downstream: Peer<RoleServer>,
    /// Name of the upstream this handler is attached to (for logging only).
    upstream_name: Arc<str>,
}

impl RelayClientHandler {
    pub(crate) fn new(downstream: Peer<RoleServer>, upstream_name: Arc<str>) -> Self {
        Self {
            downstream,
            upstream_name,
        }
    }
}

/// Map a downstream `ServiceError` into the `McpError` returned to the upstream.
///
/// The upstream sees a generic `internal_error`; the underlying cause is logged
/// at the gateway rather than leaked verbatim across the proxy boundary.
fn relay_error(upstream: &str, capability: &str, error: &rmcp::service::ServiceError) -> McpError {
    tracing::warn!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.relay",
        upstream = %upstream,
        capability,
        kind = "upstream_relay_error",
        error = %error,
        "relaying upstream server->client request to downstream agent failed",
    );
    McpError::internal_error(format!("relay of {capability} to downstream agent failed"), None)
}

impl ClientHandler for RelayClientHandler {
    /// Advertise to the upstream exactly the server→client capabilities the
    /// downstream agent declared. Anything the agent cannot do, the gateway
    /// does not claim on its behalf.
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        if let Some(downstream_info) = self.downstream.peer_info() {
            info.capabilities.elicitation = downstream_info.capabilities.elicitation.clone();
            info.capabilities.sampling = downstream_info.capabilities.sampling.clone();
            info.capabilities.roots = downstream_info.capabilities.roots.clone();
        }
        info
    }

    /// Relay an upstream elicitation request to the downstream agent.
    async fn create_elicitation(
        &self,
        params: CreateElicitationRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateElicitationResult, McpError> {
        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.relay",
            upstream = %self.upstream_name,
            capability = "elicitation",
            "relaying upstream elicitation to downstream agent",
        );
        self.downstream
            .create_elicitation(params)
            .await
            .map_err(|e| relay_error(&self.upstream_name, "elicitation", &e))
    }

    /// Relay an upstream sampling request to the downstream agent.
    async fn create_message(
        &self,
        params: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateMessageResult, McpError> {
        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.relay",
            upstream = %self.upstream_name,
            capability = "sampling",
            "relaying upstream sampling request to downstream agent",
        );
        self.downstream
            .create_message(params)
            .await
            .map_err(|e| relay_error(&self.upstream_name, "sampling", &e))
    }

    /// Relay an upstream roots request to the downstream agent.
    async fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> Result<ListRootsResult, McpError> {
        self.downstream
            .list_roots()
            .await
            .map_err(|e| relay_error(&self.upstream_name, "roots", &e))
    }
}

impl UpstreamPool {
    /// Call a single tool on an upstream over a **dedicated, relay-handled**
    /// connection.
    ///
    /// Unlike [`UpstreamPool::call_tool`] (which reuses a pooled, multiplexed
    /// `()` connection), this opens a fresh connection served with a
    /// [`RelayClientHandler`] bound to `downstream`, so any server→client
    /// request the upstream raises mid-call (elicitation/sampling/roots) is
    /// forwarded to that one agent. The connection is shut down before
    /// returning — it exists only for the lifetime of the call, which is what
    /// makes the upstream→agent mapping unambiguous.
    ///
    /// Reuses the generic `connect_upstream_with_handler` seam, so every
    /// transport (HTTP, WebSocket, stdio, OAuth-HTTP) and the stdio
    /// process-reaping guard work unchanged. `subject` is forwarded for
    /// OAuth-scoped upstreams (`None` for the common non-OAuth case).
    ///
    /// Returns `None` only if the dedicated connect fails before a peer exists
    /// — mirroring `call_tool`'s "not connected" signal so the caller's circuit
    /// breaker can react identically.
    pub async fn call_tool_relayed(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        params: CallToolRequestParams,
        downstream: Peer<RoleServer>,
    ) -> Option<Result<CallToolResult, String>> {
        let upstream_name: Arc<str> = Arc::from(config.name.as_str());
        let handler = RelayClientHandler::new(downstream, Arc::clone(&upstream_name));
        let started = Instant::now();

        let (conn, _tools) = match connect_upstream_with_handler(
            config,
            subject,
            self.oauth_client_cache.as_ref(),
            self.runtime_origin.as_deref(),
            self.runtime_owner.as_ref(),
            Some(&self.shared_http_client),
            handler,
        )
        .await
        {
            Ok(pair) => pair,
            Err(error) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Tools,
                    format!("relayed upstream connect failed: {error}"),
                )
                .await;
                return None;
            }
        };

        let timeout = self.request_timeout;
        let outcome = match tokio::time::timeout(timeout, conn.peer.call_tool(params)).await {
            Ok(Ok(result)) => Some(Ok(result)),
            Ok(Err(error)) => Some(Err(format!("relayed upstream call failed: {error}"))),
            Err(_) => Some(Err(format!(
                "relayed upstream call timed out after {}ms",
                timeout.as_millis()
            ))),
        };

        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.relay.call",
            upstream = %config.name,
            subject_scoped = subject.is_some(),
            elapsed_ms = started.elapsed().as_millis(),
            "relayed upstream tool call complete",
        );

        // Tear the dedicated connection down before returning — it is scoped to
        // this single call (and, for stdio, must reap its child).
        conn.shutdown(&config.name, "relay.call.complete").await;
        outcome
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::{
        CallToolRequestParams, CallToolResult, ClientCapabilities, Content,
        CreateElicitationRequestParams, CreateElicitationResult, ElicitationAction,
        ElicitationSchema, ErrorData, PaginatedRequestParams, PrimitiveSchema, ServerCapabilities,
        ServerInfo,
    };
    use rmcp::service::{RequestContext, RunningService};
    use rmcp::{ClientHandler, RoleClient, RoleServer, ServerHandler, ServiceExt};

    use super::super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
    use super::*;

    /// A mock agent (downstream client) that answers any elicitation by
    /// accepting with `{"confirm": true}`. Advertises elicitation support.
    #[derive(Clone)]
    struct AnsweringAgent;

    impl ClientHandler for AnsweringAgent {
        fn get_info(&self) -> ClientInfo {
            let mut info = ClientInfo::default();
            info.capabilities = ClientCapabilities::builder().enable_elicitation().build();
            info
        }

        async fn create_elicitation(
            &self,
            _params: CreateElicitationRequestParams,
            _context: RequestContext<RoleClient>,
        ) -> Result<CreateElicitationResult, McpError> {
            let mut content = serde_json::Map::new();
            content.insert("confirm".to_string(), serde_json::Value::Bool(true));
            Ok(CreateElicitationResult::new(ElicitationAction::Accept)
                .with_content(serde_json::Value::Object(content)))
        }
    }

    /// A trivial downstream-facing server: just enough to hand back a
    /// `Peer<RoleServer>` once the agent connects.
    #[derive(Clone)]
    struct TrivialServer;

    impl ServerHandler for TrivialServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::default()
        }
    }

    /// A mock upstream server whose `call_tool` issues a server→client
    /// elicitation mid-call and reports whether it was accepted.
    #[derive(Clone)]
    struct ElicitingUpstream;

    impl ServerHandler for ElicitingUpstream {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResult, ErrorData> {
            let schema = ElicitationSchema::builder()
                .required_property(
                    "confirm",
                    PrimitiveSchema::Boolean(rmcp::model::BooleanSchema::default()),
                )
                .build()
                .expect("schema builds");
            let params = CreateElicitationRequestParams::FormElicitationParams {
                meta: None,
                message: "confirm the action?".to_string(),
                requested_schema: schema,
            };
            let result = context
                .peer
                .create_elicitation(params)
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
            let confirmed = matches!(result.action, ElicitationAction::Accept);
            Ok(CallToolResult::success(vec![Content::text(format!(
                "confirmed={confirmed}"
            ))]))
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
            Ok(rmcp::model::ListToolsResult::with_all_items(vec![
                rmcp::model::Tool::new(
                    "echo".to_string(),
                    "echoes confirmation".to_string(),
                    Arc::new(serde_json::Map::new()),
                ),
            ]))
        }
    }

    /// End-to-end proof: an upstream elicitation, raised during a tool call, is
    /// relayed through the gateway's [`RelayClientHandler`] to the downstream
    /// agent, answered, and the answer flows back to the upstream — all over a
    /// dedicated connection.
    #[tokio::test]
    async fn upstream_elicitation_is_relayed_to_downstream_agent() {
        // 1. Wire the gateway's downstream side to a mock agent that answers
        //    elicitation. The gateway-server peer is what the relay forwards to.
        let (gw_server_transport, agent_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _agent_task = tokio::spawn(async move {
            let running = AnsweringAgent
                .serve(agent_transport)
                .await
                .expect("agent connects");
            running.waiting().await.expect("agent runs");
        });
        let gw_server = TrivialServer
            .serve(gw_server_transport)
            .await
            .expect("gateway server side connects");
        let downstream = gw_server.peer().clone();

        // 2. Wire the gateway's upstream side to a mock upstream that elicits.
        //    The dedicated connection is served with the relay handler.
        let (upstream_transport, gw_client_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _upstream_task = tokio::spawn(async move {
            let running = ElicitingUpstream
                .serve(upstream_transport)
                .await
                .expect("upstream connects");
            running.waiting().await.expect("upstream runs");
        });
        let gw_client = RelayClientHandler::new(downstream, Arc::from("test-upstream"))
            .serve(gw_client_transport)
            .await
            .expect("relayed upstream connection establishes");
        let upstream_peer = gw_client.peer().clone();

        // 3. Drive a tool call on the upstream. Its handler elicits → relay →
        //    agent → accept → back to the upstream, which reports the outcome.
        let result = upstream_peer
            .call_tool(CallToolRequestParams::new("echo"))
            .await
            .expect("tool call succeeds with relayed elicitation");

        let text = result
            .content
            .iter()
            .find_map(|c| c.as_text().map(|t| t.text.clone()))
            .expect("tool result has text content");
        assert_eq!(
            text, "confirmed=true",
            "the upstream should observe the downstream agent's acceptance"
        );
    }

    /// Without the relay, the unit handler declines elicitation, so the same
    /// upstream tool call reports `confirmed=false`. This pins the behavioral
    /// difference the relay introduces.
    #[tokio::test]
    async fn unit_handler_declines_upstream_elicitation() {
        let (upstream_transport, gw_client_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _upstream_task = tokio::spawn(async move {
            let running = ElicitingUpstream
                .serve(upstream_transport)
                .await
                .expect("upstream connects");
            running.waiting().await.expect("upstream runs");
        });
        let gw_client: RunningService<RoleClient, ()> = ()
            .serve(gw_client_transport)
            .await
            .expect("plain upstream connection establishes");
        let upstream_peer = gw_client.peer().clone();

        let result = upstream_peer
            .call_tool(CallToolRequestParams::new("echo"))
            .await
            .expect("tool call still completes");

        let text = result
            .content
            .iter()
            .find_map(|c| c.as_text().map(|t| t.text.clone()))
            .expect("tool result has text content");
        assert_eq!(
            text, "confirmed=false",
            "the unit handler declines elicitation, so nothing is confirmed"
        );
    }

    /// `call_tool_relayed` returns `None` (the "not connected" signal, mirroring
    /// `call_tool`) when the dedicated connect fails — here because the config
    /// names neither a URL nor a command. Proves the orchestration's
    /// connect-failure path without needing a live transport.
    #[tokio::test]
    async fn call_tool_relayed_returns_none_when_connect_fails() {
        // A downstream agent peer is required by the signature; the connect
        // fails before it is ever used.
        let (gw_server_transport, agent_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _agent_task = tokio::spawn(async move {
            let running = ()
                .serve(agent_transport)
                .await
                .expect("agent connects");
            running.waiting().await.expect("agent runs");
        });
        let gw_server = TrivialServer
            .serve(gw_server_transport)
            .await
            .expect("gateway server side connects");
        let downstream = gw_server.peer().clone();

        let pool = UpstreamPool::new();
        // Neither `url` nor `command` set → connect_upstream_with_handler errors.
        let config = super::super::testsupport::test_upstream_config();

        let result = pool
            .call_tool_relayed(
                &config,
                None,
                CallToolRequestParams::new("anything"),
                downstream,
            )
            .await;

        assert!(
            result.is_none(),
            "a failed dedicated connect should surface as None"
        );
    }
}
