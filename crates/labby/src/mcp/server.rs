//! `LabMcpServer` — the MCP `ServerHandler` implementation.
//!
//! Extracted from `cli/serve.rs` so that both the stdio and HTTP transports
//! can share the same handler logic.

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Instant;

use axum::http;
#[cfg(feature = "gateway")]
use rmcp::model::ExtensionCapabilities;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CompleteRequestParams, CompleteResult,
    DiscoverResult, GetPromptRequestParams, GetPromptResponse, InitializeRequestParams,
    InitializeResult, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams,
    ReadResourceResponse, ServerCapabilities, ServerInfo, SubscriptionFilter,
};
use rmcp::service::{RequestContext, SubscriptionContext};
use rmcp::{ErrorData, RoleServer, ServerHandler};

#[cfg(feature = "gateway")]
use crate::dispatch::gateway::manager::GatewayManager;
use crate::mcp::completion::{complete_prompt_arg, completion_info};
#[cfg(feature = "gateway")]
use crate::mcp::context::subject_from_extensions;
use crate::mcp::logging::DispatchLogOutcome;
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
    /// Active subscription sinks for list-changed notifications.
    pub peers: crate::mcp::peers::PeerRegistry,
    /// Gateway-wide switch for the explicit Code Mode MCP App surface.
    pub(crate) code_mode_app_state: crate::mcp::catalog::CodeModeAppState,
    /// Observed inbound MCP client registry — shared with `GatewayManager`
    /// via `with_client_registry` so `gateway.clients.list` can read it.
    #[cfg(feature = "gateway")]
    pub client_registry: labby_runtime::client_registry::ClientRegistryHandle,
    /// This route's transport, recorded verbatim into
    /// `ConnectedClient::transport` during discovery. One of `"stdio"`,
    /// `"http"`, `"in-process"` (built-in service peers), or `"test"`.
    pub(crate) transport_label: &'static str,
    /// Negotiated RMCP logging threshold for this server route.
    pub logging_level: Arc<AtomicU8>,
    /// Visibility and dispatch constraints for this MCP route.
    pub(crate) route_scope: McpRouteScope,
    /// Unique id for this route's downstream agent connection. Used as the
    /// second half of the upstream relay cache key so a cached relay connection
    /// is bound to exactly this agent (see `dispatch/upstream/pool/relay.rs`).
    pub(crate) relay_session_id: u64,
    #[cfg(test)]
    pub(crate) code_mode_widget_callbacks_enabled_for_test: bool,
}

#[cfg(feature = "gateway")]
pub fn verify_upstream_subject_resolution_support() -> anyhow::Result<()> {
    let (parts, _) = http::Request::new(()).into_parts();
    let auth = labby_auth::auth_context::AuthContext {
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
         http::request::Parts/AuthContext. The current runtime expects rmcp 3 request \
         extension propagation. Wire the tokio::task_local fallback or pin \
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

#[cfg(feature = "gateway")]
fn mcp_extensions() -> ExtensionCapabilities {
    let mut extensions = ExtensionCapabilities::new();
    extensions.extend(mcp_apps_ui_extension());
    extensions
}

/// Build the `ConnectedClient` record for `server/discover` — pulled out of
/// the `ServerHandler` impl so redaction can be unit tested directly against
/// a fabricated `Extensions`/`AuthContext` without standing up a full
/// `NotificationContext<RoleServer>`.
///
/// The redaction step is the whole point of this function existing
/// separately: `subject_from_extensions` returns the raw authenticated
/// subject, and it must never reach `labby_runtime::client_registry`
/// unredacted. `connected_at` is threaded in rather than read here so this
/// stays pure and testable (`jiff::Timestamp::now()` at the one real call
/// site in `discover`).
#[cfg(feature = "gateway")]
fn connected_client_from_discovery(
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
            action = "lifecycle.compat_legacy_initialize",
            subsystem = "mcp_server",
            requested_protocol_version = %request.protocol_version,
            client_name = %request.client_info.name,
            client_version = %request.client_info.version,
            "adapting legacy MCP initialize lifecycle to the stateless server"
        );
        context.peer.set_peer_info(request.clone());
        let mut info = self.get_info();
        // The legacy wire protocol requires echoing the negotiated version in
        // initialize/result. This is an edge adapter only; internal handling
        // remains stateless and all modern clients use server/discover.
        info.protocol_version = request.protocol_version;
        Ok(info)
    }

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
            .enable_completions();
        #[cfg(feature = "gateway")]
        let capabilities = builder.enable_extensions_with(mcp_extensions()).build();
        #[cfg(not(feature = "gateway"))]
        let capabilities = builder.build();
        let mut info = ServerInfo::new(capabilities);
        info.server_info = rmcp::model::Implementation::new("labby", env!("CARGO_PKG_VERSION"));
        info
    }

    async fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, ErrorData> {
        #[cfg(feature = "gateway")]
        {
            let client_info = context.client_info();
            let connected_client = connected_client_from_discovery(
                client_info,
                &context.extensions,
                self.transport_label,
                jiff::Timestamp::now().to_string(),
            );
            self.client_registry.push(connected_client).await;
        }

        Ok(DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            self.get_info(),
        ))
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(requested.clone())
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        // Seed the subscription's last-published contract with what this route
        // currently exposes. A later catalog trigger only notifies this stream
        // when its own visible contract actually moves.
        let contract = self.peer_contract_for_request(context.request_context());
        let last_contract = contract.visible_contract().await;
        let route_scope_label = self.route_scope.label();
        let pruned_peer_count = crate::mcp::peers::prune_closed_peers(&self.peers).await;
        let mut peers = self.peers.write().await;
        let registered = crate::mcp::peers::RegisteredPeer::from_subscription(
            context.sink().clone(),
            contract,
            last_contract,
        );
        let registration_id = registered.registration_id;
        peers.push(registered);
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "peer.connect",
            subsystem = "mcp_server",
            phase = "subscription.listen",
            peer_count = peers.len(),
            pruned_peer_count,
            route_scope = route_scope_label,
            "mcp notification subscription connected"
        );
        drop(peers);

        context.cancelled().await;

        let mut peers = self.peers.write().await;
        peers.retain(|registered| registered.registration_id != registration_id);
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "peer.disconnect",
            subsystem = "mcp_server",
            phase = "subscription.closed",
            peer_count = peers.len(),
            route_scope = route_scope_label,
            "mcp notification subscription disconnected"
        );
        Ok(())
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
    ) -> Result<GetPromptResponse, ErrorData> {
        self.get_prompt_impl(request, context).await.map(Into::into)
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.list_resources_impl(request, context).await
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::with_all_items(Vec::new())
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.read_resource_impl(request, context)
            .await
            .map(|result| {
                result
                    .with_ttl_ms(0)
                    .with_cache_scope(CacheScope::Private)
                    .into()
            })
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
    ) -> Result<CallToolResponse, ErrorData> {
        self.call_tool_response_impl(request, context).await
    }
}

use crate::mcp::catalog::CatalogChangeSet;

impl LabMcpServer {
    /// `source` attributes the emission — see `labby_runtime::catalog_notify`.
    /// Per-call sites pass their own label so a notification triggered by a
    /// tool call is never confused with a gateway reconcile.
    pub(crate) async fn notify_catalog_changes(
        &self,
        changes: CatalogChangeSet,
        source: &'static str,
    ) {
        // Scheduled, not sent: this runs at the tail of a tool call, and the
        // caller's turn is still open. Delivering here would invalidate the
        // binding that call is using. See `catalog_coalesce`.
        crate::mcp::catalog_coalesce::schedule_catalog_notification(
            &self.peers,
            changes.into(),
            source,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::LabMcpServer;
    #[cfg(feature = "gateway")]
    use super::verify_upstream_subject_resolution_support;
    use crate::mcp::catalog_notifications::{CatalogNotificationChanges, notify_catalog_peers};
    use crate::mcp::logging::logging_level_rank;
    use crate::registry::ToolRegistry;
    use rmcp::ServerHandler;
    use rmcp::ServiceExt;
    use rmcp::model::{ProtocolVersion, ServerNotification, SubscriptionFilter};
    use rmcp::service::{ClientLifecycleMode, ClientServiceExt};

    fn stateless_test_server(peers: crate::mcp::peers::PeerRegistry) -> LabMcpServer {
        LabMcpServer {
            registry: std::sync::Arc::new(ToolRegistry::new()),
            #[cfg(feature = "gateway")]
            gateway_manager: None,
            peers,
            code_mode_app_state: Default::default(),
            #[cfg(feature = "gateway")]
            client_registry: Default::default(),
            transport_label: "test",
            logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                logging_level_rank(crate::mcp::logging::LoggingLevel::Info),
            )),
            route_scope: crate::mcp::route_scope::McpRouteScope::Root,
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        }
    }

    #[test]
    fn server_capabilities_advertise_list_changed_support() {
        let server =
            stateless_test_server(std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())));

        let info = server.get_info();
        assert_eq!(info.server_info.name, "labby");
        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
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
            info.capabilities.logging.is_none(),
            "2026-07-28 removes logging/setLevel and must not advertise legacy logging"
        );
        assert!(
            info.capabilities.completions.is_some(),
            "RMCP completion capability must be advertised"
        );
        if let Some(extensions) = info.capabilities.extensions.as_ref() {
            for invented_auth_extension in [
                "io.modelcontextprotocol/enterprise-managed-authorization",
                "io.modelcontextprotocol/oauth-client-credentials",
                "io.modelcontextprotocol/client-id-metadata-document",
            ] {
                assert!(
                    !extensions.contains_key(invented_auth_extension),
                    "OAuth extensions are discovered through authorization metadata, not MCP initialize capabilities"
                );
            }
        }

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

    #[tokio::test]
    async fn resource_templates_include_required_rc_cache_metadata() {
        let server =
            stateless_test_server(std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())));
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let context = rmcp::service::RequestContext::new(
            rmcp::model::NumberOrString::Number(1),
            running.peer().clone(),
        );

        let result = running
            .service()
            .list_resource_templates(None, context)
            .await
            .expect("resource templates");

        assert_eq!(result.ttl_ms, Some(0));
        assert_eq!(result.cache_scope, Some(rmcp::model::CacheScope::Private));
        let wire = serde_json::to_value(result).expect("serialize resource templates");
        assert_eq!(wire["resultType"], "complete");
        assert_eq!(wire["ttlMs"], 0);
        assert_eq!(wire["cacheScope"], "private");
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn upstream_subject_resolution_self_test_passes_for_plan_a() {
        verify_upstream_subject_resolution_support().expect("self-test");
    }

    #[cfg(feature = "gateway")]
    mod connected_client_from_discovery_tests {
        use axum::http;
        use rmcp::model::Implementation;

        use super::super::connected_client_from_discovery;

        // Same `Extensions` fabrication as `verify_upstream_subject_resolution_support`
        // above — an `http::request::Parts` carrying an `AuthContext`, wrapped in
        // `rmcp::model::Extensions`.
        fn extensions_with_subject(subject: &str) -> rmcp::model::Extensions {
            let (mut parts, _) = http::Request::new(()).into_parts();
            parts
                .extensions
                .insert(labby_auth::auth_context::AuthContext {
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
            let client = connected_client_from_discovery(
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
            let a = connected_client_from_discovery(
                None,
                &extensions_with_subject("same-subject"),
                "http",
                "2026-01-01T00:00:00Z".to_string(),
            );
            let b = connected_client_from_discovery(
                None,
                &extensions_with_subject("same-subject"),
                "http",
                "2026-01-01T00:00:00Z".to_string(),
            );

            assert_eq!(a.subject_tag, b.subject_tag);
        }

        #[test]
        fn distinct_subjects_redact_to_distinct_tags() {
            let a = connected_client_from_discovery(
                None,
                &extensions_with_subject("alice"),
                "http",
                "2026-01-01T00:00:00Z".to_string(),
            );
            let b = connected_client_from_discovery(
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
            let client = connected_client_from_discovery(
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
            let client = connected_client_from_discovery(
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

    #[tokio::test]
    async fn stateless_subscription_receives_catalog_notifications() {
        let peers = std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let server = stateless_test_server(std::sync::Arc::clone(&peers));
        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let server_handle = tokio::spawn(async move {
            let running = server.serve(server_transport).await.expect("server starts");
            running.waiting().await
        });
        let client_service = ()
            .serve_with_lifecycle(
                client_transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .expect("stateless client discovers server");

        let mut subscription = client_service
            .peer()
            .listen(
                SubscriptionFilter::builder()
                    .resources_list_changed()
                    .build(),
            )
            .await
            .expect("subscription is acknowledged");
        assert_eq!(peers.read().await.len(), 1);

        notify_catalog_peers(
            &peers,
            CatalogNotificationChanges::new(false, true, false),
            labby_runtime::catalog_notify::SOURCE_MCP_CALL_UPSTREAM,
        )
        .await;
        let notification = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("catalog notification timed out")
            .expect("subscription remains healthy")
            .expect("catalog notification exists");
        assert!(matches!(
            notification,
            ServerNotification::ResourceListChangedNotification(_)
        ));

        subscription.cancel().await.expect("subscription cancels");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !peers.read().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled subscription is removed");
        client_service.cancel().await.expect("client cancels");
        server_handle.abort();
    }
}
