//! `LabMcpServer` — the MCP `ServerHandler` implementation.
//!
//! Extracted from `cli/serve.rs` so that both the stdio and HTTP transports
//! can share the same handler logic.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Instant;

use axum::http;
use futures::future::join_all;
#[cfg(feature = "gateway")]
use rmcp::model::ExtensionCapabilities;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, CompleteRequestParams, CompleteResult,
    GetPromptRequestParams, GetPromptResult, ListPromptsResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResult,
    ServerCapabilities, ServerInfo,
};
// rmcp 2.1 deprecates legacy logging under SEP-2577; the ServerHandler trait
// still requires this request type for clients using the old logging flow.
#[allow(deprecated)]
use rmcp::model::SetLevelRequestParams;
use rmcp::service::{NotificationContext, Peer, RequestContext};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use tokio::sync::RwLock;

#[cfg(feature = "gateway")]
use crate::dispatch::gateway::manager::GatewayManager;
use crate::mcp::completion::{complete_prompt_arg, completion_info};
#[cfg(feature = "gateway")]
use crate::mcp::context::subject_from_extensions;
use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel, logging_level_rank};
use crate::mcp::route_scope::McpRouteScope;
use crate::registry::ToolRegistry;

/// Process-global counter minting a unique `relay_session_id` per
/// `LabMcpServer` instance. Each transport session (HTTP factory invocation or
/// the single stdio server) builds one `LabMcpServer`, so the id is stable for
/// a session's lifetime and unique across sessions — exactly the key the
/// upstream relay cache needs to bind a cached connection to one downstream
/// agent without ever reusing it across agents.
static RELAY_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Mint the next unique relay-session id. Called once per `LabMcpServer`.
pub(crate) fn next_relay_session_id() -> u64 {
    RELAY_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// MCP server handler — one tool per registered service.
pub struct LabMcpServer {
    pub registry: Arc<ToolRegistry>,
    /// Shared gateway manager used to resolve the current live upstream pool.
    #[cfg(feature = "gateway")]
    pub gateway_manager: Option<Arc<GatewayManager>>,
    /// Connected peers for list-changed notifications.
    pub peers: Arc<RwLock<Vec<Peer<RoleServer>>>>,
    /// Live inbound MCP client/session registry — shared with `GatewayManager`
    /// via `with_client_registry` so `gateway.clients.list` can read it.
    #[cfg(feature = "gateway")]
    pub client_registry: labby_runtime::client_registry::ClientRegistryHandle,
    /// This session's transport, recorded verbatim into
    /// `ConnectedClient::transport` on `on_initialized`. One of `"stdio"`,
    /// `"http"`, `"in-process"` (built-in service peers), or `"test"`.
    pub(crate) transport_label: &'static str,
    /// Negotiated RMCP logging threshold for this server/session.
    pub logging_level: Arc<AtomicU8>,
    /// Visibility and dispatch constraints for this MCP route/session.
    pub(crate) route_scope: McpRouteScope,
    /// Unique id for this session's downstream agent connection. Used as the
    /// second half of the upstream relay cache key so a cached relay connection
    /// is bound to exactly this agent (see `dispatch/upstream/pool/relay.rs`).
    pub(crate) relay_session_id: u64,
    #[cfg(test)]
    pub(crate) code_mode_widget_callbacks_enabled_for_test: bool,
}

#[cfg(feature = "gateway")]
pub fn verify_upstream_subject_resolution_support() -> anyhow::Result<()> {
    let (parts, _) = http::Request::new(()).into_parts();
    let auth = crate::api::oauth::AuthContext {
        sub: "startup-self-test".to_string(),
        actor_key: None,
        scopes: Vec::new(),
        issuer: "https://lab.example.com".to_string(),
        via_session: false,
        csrf_token: None,
        email: None,
    };

    let mut extensions = rmcp::model::Extensions::new();
    let mut parts = parts;
    parts.extensions.insert(auth);
    extensions.insert(parts);

    if subject_from_extensions(&extensions) == Some("startup-self-test") {
        return Ok(());
    }

    anyhow::bail!(
        "rmcp subject extraction self-test failed: RequestContext.extensions did not yield \
         http::request::Parts/AuthContext. The current runtime expects rmcp 1.4 request \
         extension propagation (Plan A). Wire the tokio::task_local fallback (Plan B) or pin \
         a compatible rmcp version before starting."
    );
}

/// Advertise the MCP Apps UI extension (`io.modelcontextprotocol/ui`, SEP-1724)
/// so hosts like Claude.ai know to render the Code Mode inspector widgets served
/// at `ui://lab/code-mode/{search,execute,history}`. The `mimeTypes` value mirrors
/// the MIME the widget resources are published with (`text/html;profile=mcp-app`).
#[cfg(feature = "gateway")]
fn mcp_apps_ui_extension() -> ExtensionCapabilities {
    let mut extensions = ExtensionCapabilities::new();
    let mut ui_ext = serde_json::Map::new();
    ui_ext.insert(
        "mimeTypes".to_string(),
        serde_json::json!([crate::mcp::handlers_resources::CODE_MODE_APP_MIME]),
    );
    extensions.insert("io.modelcontextprotocol/ui".to_string(), ui_ext);
    extensions
}

/// Build the `ConnectedClient` record for `on_initialized` — pulled out of
/// the `ServerHandler` impl so redaction can be unit tested directly against
/// a fabricated `Extensions`/`AuthContext` without standing up a full
/// `NotificationContext<RoleServer>`.
///
/// The redaction step is the whole point of this function existing
/// separately: `subject_from_extensions` returns the raw authenticated
/// subject, and it must never reach `labby_runtime::client_registry`
/// unredacted. `connected_at` is threaded in rather than read here so this
/// stays pure and testable (`jiff::Timestamp::now()` at the one real call
/// site in `on_initialized`).
#[cfg(feature = "gateway")]
fn connected_client_from_handshake(
    client_info: Option<rmcp::model::Implementation>,
    extensions: &rmcp::model::Extensions,
    transport_label: &str,
    connected_at: String,
) -> labby_runtime::client_registry::ConnectedClient {
    let subject_tag =
        subject_from_extensions(extensions).map(crate::mcp::context::redact_subject_for_logging);
    labby_runtime::client_registry::ConnectedClient {
        subject_tag,
        client_name: client_info.as_ref().map(|info| info.name.clone()),
        client_version: client_info.as_ref().map(|info| info.version.clone()),
        transport: transport_label.to_string(),
        connected_at,
    }
}

impl ServerHandler for LabMcpServer {
    #[allow(deprecated)]
    fn get_info(&self) -> ServerInfo {
        #[cfg(feature = "gateway")]
        let gateway_manager_configured = self.gateway_manager.is_some();
        #[cfg(not(feature = "gateway"))]
        let gateway_manager_configured = false;
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "server.info",
            subsystem = "mcp_server",
            phase = "server.info",
            builtin_service_count = self.registry.services().len(),
            gateway_manager_configured,
            "advertising MCP server capabilities"
        );
        let builder = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .enable_resources()
            .enable_resources_list_changed()
            .enable_prompts()
            .enable_prompts_list_changed()
            .enable_logging()
            .enable_completions();
        #[cfg(feature = "gateway")]
        let builder = builder.enable_extensions_with(mcp_apps_ui_extension());
        ServerInfo::new(builder.build())
    }

    #[allow(deprecated)]
    async fn set_level(
        &self,
        request: SetLevelRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.logging_level.store(
            logging_level_rank(LoggingLevel::from_rmcp(request.level)),
            Ordering::Release,
        );
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "logging.setLevel",
            level = ?request.level,
            "rmcp logging level updated"
        );
        Ok(())
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        #[cfg(feature = "gateway")]
        {
            let client_info = context
                .peer
                .peer_info()
                .map(|info| info.client_info.clone());
            let connected_client = connected_client_from_handshake(
                client_info,
                &context.extensions,
                self.transport_label,
                jiff::Timestamp::now().to_string(),
            );
            self.client_registry.push(connected_client).await;
        }
        let mut peers = self.peers.write().await;
        peers.push(context.peer);
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "peer.connect",
            subsystem = "mcp_server",
            phase = "session.initialized",
            peer_count = peers.len(),
            "mcp session connected"
        );
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        let reference_type = request.r#ref.reference_type();
        let prompt = request.r#ref.as_prompt_name().map(str::to_string);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "completion.complete",
            subject,
            reference_type,
            prompt = prompt.as_deref().unwrap_or(""),
            argument = %request.argument.name,
            "dispatch start"
        );

        let completion = match prompt.as_deref() {
            Some(prompt_name) => complete_prompt_arg(
                &self.registry,
                prompt_name,
                &request.argument.name,
                &request.argument.value,
            ),
            None => completion_info(Vec::new()),
        };

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "completion.complete",
            subject,
            reference_type,
            prompt = prompt.as_deref().unwrap_or(""),
            argument = %request.argument.name,
            result_count = completion.values.len(),
            elapsed_ms,
            "completion ok"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "completion.complete",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        Ok(CompleteResult::new(completion))
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        self.list_prompts_impl(request, context).await
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, ErrorData> {
        self.get_prompt_impl(request, context).await
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.list_resources_impl(request, context).await
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        self.read_resource_impl(request, context).await
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.list_tools_impl(request, context).await
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.call_tool_impl(request, context).await
    }
}

use crate::mcp::catalog::CatalogChangeSet;

impl LabMcpServer {
    pub(crate) async fn notify_catalog_changes(&self, changes: CatalogChangeSet) {
        if !changes.any() {
            return;
        }

        let peers = self.peers.read().await.clone();
        let peer_count = peers.len();
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
            "notifying MCP peers about catalog change"
        );

        let notification_timeout = crate::config::resolved_catalog_notification_timeout();
        let notify_futures = peers.iter().enumerate().map(|(peer_index, peer)| {
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
                            "failed to notify peer about tool catalog change; pruning stale session"
                        );
                        return false;
                    }
                    if changes.resources_changed
                        && peer.notify_resource_list_changed().await.is_err()
                    {
                        tracing::warn!(
                            surface = "mcp",
                            service = "peers",
                            action = "peer.disconnect",
                            peer_index,
                            phase = "resources",
                            "failed to notify peer about resource catalog change; pruning stale session"
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
                            "failed to notify peer about prompt catalog change; pruning stale session"
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
        let alive: Vec<_> = peers
            .into_iter()
            .zip(results)
            .filter_map(|(peer, ok)| ok.then_some(peer))
            .collect();
        let mut guard = self.peers.write().await;
        let added_since_snapshot = if guard.len() > peer_count {
            guard.split_off(peer_count)
        } else {
            Vec::new()
        };
        let alive_count = alive.len();
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
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "gateway")]
    use super::verify_upstream_subject_resolution_support;
    use super::{LabMcpServer, logging_level_rank};
    use crate::registry::ToolRegistry;
    use rmcp::ServerHandler;

    #[test]
    fn server_capabilities_advertise_list_changed_support() {
        let server = LabMcpServer {
            registry: std::sync::Arc::new(ToolRegistry::new()),
            #[cfg(feature = "gateway")]
            gateway_manager: None,
            peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
            #[cfg(feature = "gateway")]
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                logging_level_rank(crate::mcp::logging::LoggingLevel::Info),
            )),
            route_scope: crate::mcp::route_scope::McpRouteScope::Root,
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        };

        let info = server.get_info();
        assert_eq!(
            info.capabilities.tools.and_then(|c| c.list_changed),
            Some(true)
        );
        assert_eq!(
            info.capabilities.resources.and_then(|c| c.list_changed),
            Some(true)
        );
        assert_eq!(
            info.capabilities.prompts.and_then(|c| c.list_changed),
            Some(true)
        );
        assert!(
            info.capabilities.logging.is_some(),
            "RMCP logging capability must be advertised"
        );
        assert!(
            info.capabilities.completions.is_some(),
            "RMCP completion capability must be advertised"
        );

        #[cfg(feature = "gateway")]
        {
            // MCP Apps UI extension (SEP-1724) must be advertised so hosts render
            // the Code Mode inspector widgets.
            let extensions = info
                .capabilities
                .extensions
                .expect("MCP Apps UI extension capability must be advertised");
            let ui_ext = extensions
                .get("io.modelcontextprotocol/ui")
                .expect("io.modelcontextprotocol/ui extension must be present");
            assert_eq!(
                ui_ext.get("mimeTypes"),
                Some(&serde_json::json!(["text/html;profile=mcp-app"])),
                "UI extension must advertise the mcp-app widget MIME type"
            );
        }
        #[cfg(not(feature = "gateway"))]
        assert!(
            info.capabilities.extensions.is_none(),
            "no-gateway builds must not advertise MCP Apps UI"
        );
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn upstream_subject_resolution_self_test_passes_for_plan_a() {
        verify_upstream_subject_resolution_support().expect("self-test");
    }

    #[cfg(feature = "gateway")]
    mod connected_client_from_handshake_tests {
        use axum::http;
        use rmcp::model::Implementation;

        use super::super::connected_client_from_handshake;

        // Same `Extensions` fabrication as `verify_upstream_subject_resolution_support`
        // above — an `http::request::Parts` carrying an `AuthContext`, wrapped in
        // `rmcp::model::Extensions`.
        fn extensions_with_subject(subject: &str) -> rmcp::model::Extensions {
            let (mut parts, _) = http::Request::new(()).into_parts();
            parts.extensions.insert(crate::api::oauth::AuthContext {
                sub: subject.to_string(),
                actor_key: None,
                scopes: Vec::new(),
                issuer: "https://lab.example.com".to_string(),
                via_session: false,
                csrf_token: None,
                email: None,
            });
            let mut extensions = rmcp::model::Extensions::new();
            extensions.insert(parts);
            extensions
        }

        #[test]
        fn never_stores_the_raw_authenticated_subject() {
            let extensions = extensions_with_subject("jacob@example.com");
            let client = connected_client_from_handshake(
                Some(Implementation::new("claude-code", "2.4.1")),
                &extensions,
                "stdio",
                "2026-01-01T00:00:00Z".to_string(),
            );

            let tag = client.subject_tag.expect("subject_tag must be set");
            assert_ne!(tag, "jacob@example.com", "raw subject must never be stored");
            assert!(
                tag.starts_with("sub:"),
                "expected a redacted `sub:` tag, got {tag:?}"
            );
        }

        #[test]
        fn redaction_is_deterministic_for_the_same_subject() {
            let a = connected_client_from_handshake(
                None,
                &extensions_with_subject("same-subject"),
                "http",
                "2026-01-01T00:00:00Z".to_string(),
            );
            let b = connected_client_from_handshake(
                None,
                &extensions_with_subject("same-subject"),
                "http",
                "2026-01-01T00:00:00Z".to_string(),
            );

            assert_eq!(a.subject_tag, b.subject_tag);
        }

        #[test]
        fn distinct_subjects_redact_to_distinct_tags() {
            let a = connected_client_from_handshake(
                None,
                &extensions_with_subject("alice"),
                "http",
                "2026-01-01T00:00:00Z".to_string(),
            );
            let b = connected_client_from_handshake(
                None,
                &extensions_with_subject("bob"),
                "http",
                "2026-01-01T00:00:00Z".to_string(),
            );

            assert_ne!(a.subject_tag, b.subject_tag);
        }

        #[test]
        fn no_auth_context_yields_no_subject_tag() {
            let extensions = rmcp::model::Extensions::new();
            let client = connected_client_from_handshake(
                None,
                &extensions,
                "in-process",
                "2026-01-01T00:00:00Z".to_string(),
            );

            assert_eq!(client.subject_tag, None);
        }

        #[test]
        fn client_info_and_transport_pass_through_unmodified() {
            let extensions = rmcp::model::Extensions::new();
            let client = connected_client_from_handshake(
                Some(Implementation::new("codex-cli", "0.9.2")),
                &extensions,
                "stdio",
                "2026-01-01T00:00:00Z".to_string(),
            );

            assert_eq!(client.client_name.as_deref(), Some("codex-cli"));
            assert_eq!(client.client_version.as_deref(), Some("0.9.2"));
            assert_eq!(client.transport, "stdio");
            assert_eq!(client.connected_at, "2026-01-01T00:00:00Z");
        }
    }
}
