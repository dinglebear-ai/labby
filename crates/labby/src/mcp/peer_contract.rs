//! Per-peer client-visible tool contract.
//!
//! A tools/list_changed notification is a statement about one subscription's
//! post-filter descriptor set. PeerContract retains the durable inputs needed
//! to rebuild that set after gateway state changes: route scope, caller class,
//! registry composition, and the current gateway manager.

use std::collections::HashSet;
use std::sync::Arc;

use rmcp::model::Tool;

use crate::mcp::catalog::{
    CodeModeAppState, CodeModeVisibility, SERVER_LOGS_TOOL_NAME, ToolCatalogSnapshot,
};
use crate::mcp::route_scope::McpRouteScope;
use crate::registry::ToolRegistry;

#[cfg(feature = "gateway")]
use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
#[cfg(feature = "gateway")]
use crate::dispatch::gateway::manager::GatewayManager;
#[cfg(feature = "gateway")]
use crate::dispatch::upstream::pool::MAX_UPSTREAM_TOOLS;
#[cfg(feature = "gateway")]
use crate::dispatch::upstream::pool::UpstreamPool;
#[cfg(feature = "gateway")]
use crate::mcp::call_tool_codemode::CodeModeUpstreamDescription;
#[cfg(feature = "gateway")]
use crate::mcp::catalog::{
    ADD_SERVER_TOOL_NAME, CODE_MODE_READ_TOOL_NAME, CODE_MODE_UI_TOOL_NAME,
    GATEWAY_STATUS_TOOL_NAME, MCP_APP_TOOL_NAME,
};

// ── FR-2a (issue #210, lab-41e7m.5): single audience-free authorization gates ──
//
// One implementation of the MCP visibility/authorization predicates,
// consumed by both `LabMcpServer` (catalog.rs, live-request path) and
// `PeerContract` (stored-subscription path). Two hard constraints, from the
// 2026-08-05 engineering review:
//
// - AUDIENCE-FREE. `audience.admin_apps_visible` (or the request's
//   `admin_app_resources_visible`) must stay at the call sites: folding it in
//   here would silently grant admin apps to unprivileged callers, because
//   `catalog.rs` supplies `PeerCatalogAudience::default()` with
//   `admin_apps_visible: true`.
// - FREE FUNCTIONS over borrowed fields, not methods reached via
//   `self.peer_contract()` — constructing a `PeerContract` clones a deep
//   `McpRouteScope` on every builtin dispatch.

/// Whether `service` is exposed on the MCP surface for this route scope.
#[cfg(feature = "gateway")]
pub(crate) async fn mcp_service_visible(
    route_scope: &McpRouteScope,
    gateway_manager: Option<&GatewayManager>,
    service: &str,
) -> bool {
    if !route_scope.allows_service(service) {
        return false;
    }
    match gateway_manager {
        Some(manager) => manager.surface_enabled_for_service(service, "mcp").await,
        None => true,
    }
}

/// Whether `service.action` is allowed on the MCP surface.
#[cfg(feature = "gateway")]
pub(crate) async fn mcp_action_allowed(
    gateway_manager: Option<&GatewayManager>,
    service: &str,
    action: &str,
) -> bool {
    match gateway_manager {
        Some(manager) => {
            manager
                .mcp_action_allowed_for_service(service, action)
                .await
        }
        None => true,
    }
}

/// Current gateway-wide visibility for Labby-owned MCP Apps other than Code Mode.
#[cfg(feature = "gateway")]
pub(crate) async fn mcp_apps_config(
    gateway_manager: Option<&GatewayManager>,
) -> labby_runtime::gateway_config::McpAppsConfig {
    match gateway_manager {
        Some(manager) => manager.mcp_apps_config().await,
        None => labby_runtime::gateway_config::McpAppsConfig::default(),
    }
}

/// Whether the current route can safely advertise and execute Add Server.
#[cfg(feature = "gateway")]
pub(crate) async fn add_server_app_available(
    route_scope: &McpRouteScope,
    gateway_manager: Option<&GatewayManager>,
    registry: &ToolRegistry,
) -> bool {
    if !mcp_apps_config(gateway_manager).await.add_server {
        return false;
    }
    admin_gateway_app_available(
        route_scope,
        gateway_manager,
        registry,
        &["gateway.test", "gateway.add"],
    )
    .await
}

/// Whether the current route can safely advertise live gateway status.
#[cfg(feature = "gateway")]
pub(crate) async fn gateway_status_app_available(
    route_scope: &McpRouteScope,
    gateway_manager: Option<&GatewayManager>,
    registry: &ToolRegistry,
) -> bool {
    if !mcp_apps_config(gateway_manager).await.gateway_status {
        return false;
    }
    admin_gateway_app_available(route_scope, gateway_manager, registry, &["gateway.list"]).await
}

#[cfg(feature = "gateway")]
async fn admin_gateway_app_available(
    route_scope: &McpRouteScope,
    gateway_manager: Option<&GatewayManager>,
    registry: &ToolRegistry,
    required_actions: &[&str],
) -> bool {
    if !(route_scope.allows_service("gateway")
        && gateway_manager.is_some()
        && registry.service("gateway").is_some()
        && mcp_service_visible(route_scope, gateway_manager, "gateway").await)
    {
        return false;
    }
    for action in required_actions {
        if !mcp_action_allowed(gateway_manager, "gateway", action).await {
            return false;
        }
    }
    true
}

/// Request-derived inputs that affect the descriptor set for one peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PeerCatalogAudience {
    pub(crate) code_mode_read_allowed: bool,
    pub(crate) code_mode_execute_allowed: bool,
    pub(crate) admin_apps_visible: bool,
    #[cfg(feature = "gateway")]
    pub(crate) oauth_subject: Option<String>,
}

impl Default for PeerCatalogAudience {
    fn default() -> Self {
        Self {
            code_mode_read_allowed: true,
            code_mode_execute_allowed: true,
            admin_apps_visible: true,
            #[cfg(feature = "gateway")]
            oauth_subject: Some(SHARED_GATEWAY_OAUTH_SUBJECT.to_string()),
        }
    }
}

/// Everything a subscription's visible tool descriptors derive from.
#[derive(Clone)]
pub(crate) struct PeerContract {
    pub(crate) registry: Arc<ToolRegistry>,
    #[cfg(feature = "gateway")]
    pub(crate) gateway_manager: Option<Arc<GatewayManager>>,
    pub(crate) route_scope: McpRouteScope,
    /// Whether the optional `codemode_ui` MCP App surface is advertised. The
    /// text-only `codemode` executor and always-on `mcp_app` manager never
    /// depend on the inspector switch.
    pub(crate) code_mode_app_state: CodeModeAppState,
    pub(crate) audience: PeerCatalogAudience,
}

impl PeerContract {
    /// Which Code Mode regime applies to this peer.
    pub(crate) async fn code_mode_visibility(&self) -> CodeModeVisibility {
        #[cfg(feature = "gateway")]
        {
            if !self.route_scope.exposes_code_mode() {
                return CodeModeVisibility::Raw;
            }
            let enabled = if let Some(manager) = &self.gateway_manager {
                manager.code_mode_enabled().await
            } else {
                false
            };
            if enabled {
                return CodeModeVisibility::RootSynthetic;
            }
            if self.gateway_manager.is_none() && crate::config::process_code_mode_enabled() {
                return CodeModeVisibility::InProcessPeer;
            }
        }
        CodeModeVisibility::Raw
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn current_upstream_pool(&self) -> Option<Arc<UpstreamPool>> {
        match &self.gateway_manager {
            Some(manager) => manager.current_pool().await,
            None => None,
        }
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn mcp_apps_config(&self) -> labby_runtime::gateway_config::McpAppsConfig {
        mcp_apps_config(self.gateway_manager.as_deref()).await
    }

    pub(crate) async fn service_visible_on_mcp(&self, service: &str) -> bool {
        #[cfg(feature = "gateway")]
        {
            mcp_service_visible(&self.route_scope, self.gateway_manager.as_deref(), service).await
        }
        #[cfg(not(feature = "gateway"))]
        {
            self.route_scope.allows_service(service)
        }
    }

    #[cfg(feature = "gateway")]
    async fn add_server_app_available(&self) -> bool {
        add_server_app_available(
            &self.route_scope,
            self.gateway_manager.as_deref(),
            &self.registry,
        )
        .await
    }

    #[cfg(feature = "gateway")]
    async fn gateway_status_app_available(&self) -> bool {
        gateway_status_app_available(
            &self.route_scope,
            self.gateway_manager.as_deref(),
            &self.registry,
        )
        .await
    }

    /// Enabled upstream namespaces and normalized hints rendered in Code
    /// Mode's descriptors. Descriptor hashing therefore catches a hint-only
    /// edit and subscription fanout can publish the new contract.
    #[cfg(feature = "gateway")]
    pub(crate) async fn code_mode_upstreams_for_description(
        &self,
    ) -> Vec<CodeModeUpstreamDescription> {
        let Some(manager) = &self.gateway_manager else {
            return Vec::new();
        };
        let mut upstreams = manager
            .current_config()
            .await
            .upstream
            .into_iter()
            .filter(|upstream| upstream.enabled)
            .filter(|upstream| self.route_scope.allows_upstream(&upstream.name))
            .map(|upstream| CodeModeUpstreamDescription {
                name: upstream.name,
                hint: upstream
                    .code_mode_hint
                    .as_deref()
                    .and_then(labby_runtime::gateway_config::normalize_code_mode_hint),
            })
            .collect::<Vec<_>>();
        upstreams.sort_by(|a, b| a.name.cmp(&b.name));
        upstreams.dedup_by(|a, b| a.name == b.name);
        upstreams
    }

    #[cfg(feature = "gateway")]
    async fn route_scoped_oauth_upstream_configs(&self) -> Vec<crate::config::UpstreamConfig> {
        let Some(manager) = &self.gateway_manager else {
            return Vec::new();
        };
        let mut configs = manager.oauth_upstream_configs().await;
        configs.retain(|config| self.route_scope.allows_upstream(&config.name));
        configs
    }

    /// Exact, unpaginated descriptor set this peer would receive from
    /// tools/list, before cursor slicing. Collision and visibility rules mirror
    /// the handler; only request telemetry and pagination are omitted.
    pub(crate) async fn visible_tool_descriptors(&self) -> Vec<Tool> {
        let visibility = self.code_mode_visibility().await;
        let hide_raw_tools = visibility.hides_raw_tools();
        let mut descriptors = Vec::new();
        let mut builtin_names = HashSet::new();
        let mut advertised_names = HashSet::new();
        let server_logs_app_visible = {
            #[cfg(feature = "gateway")]
            {
                self.audience.admin_apps_visible && self.mcp_apps_config().await.server_logs
            }
            #[cfg(not(feature = "gateway"))]
            {
                self.audience.admin_apps_visible
            }
        };

        for service in self.registry.services() {
            if self.service_visible_on_mcp(service.name).await {
                builtin_names.insert(service.name.to_string());
                if hide_raw_tools && service.name != SERVER_LOGS_TOOL_NAME {
                    continue;
                }
                advertised_names.insert(service.name.to_string());
                descriptors.push(
                    self.registry
                        .permanent_tools()
                        .builtin_service_tool(service, server_logs_app_visible),
                );
            }
        }

        #[cfg(feature = "gateway")]
        if visibility.exposes_synthetic_tools()
            && (self.audience.code_mode_read_allowed || self.audience.code_mode_execute_allowed)
        {
            let upstreams = self.code_mode_upstreams_for_description().await;
            if self.audience.code_mode_read_allowed {
                let tool = self
                    .registry
                    .permanent_tools()
                    .code_mode_read_descriptor(&upstreams);
                advertised_names.insert(CODE_MODE_READ_TOOL_NAME.to_string());
                descriptors.push(tool);
            }

            if self.audience.code_mode_execute_allowed {
                // `codemode` is permanently text-only; the MCP App metadata lives on
                // the optional `codemode_ui` twin so disabling the app cannot remove
                // the execution entry point. Mirrors handlers_tools::list_tools.
                let tool = self
                    .registry
                    .permanent_tools()
                    .code_mode_descriptor(&upstreams);
                advertised_names.insert(tool.name.as_ref().to_string());
                descriptors.push(tool);

                if self.code_mode_app_state.is_enabled() {
                    let tool = self
                        .registry
                        .permanent_tools()
                        .code_mode_ui_tool(&upstreams);
                    advertised_names.insert(CODE_MODE_UI_TOOL_NAME.to_string());
                    descriptors.push(tool);
                }
            }
        }

        #[cfg(feature = "gateway")]
        if self.route_scope.is_root() && self.audience.code_mode_execute_allowed {
            let tool = self.registry.permanent_tools().mcp_app_tool();
            advertised_names.insert(MCP_APP_TOOL_NAME.to_string());
            descriptors.push(tool);
        }

        #[cfg(feature = "gateway")]
        if self.audience.admin_apps_visible && self.add_server_app_available().await {
            let tool = self.registry.permanent_tools().add_server_tool();
            advertised_names.insert(ADD_SERVER_TOOL_NAME.to_string());
            descriptors.push(tool);
        }

        #[cfg(feature = "gateway")]
        if self.audience.admin_apps_visible && self.gateway_status_app_available().await {
            let tool = self.registry.permanent_tools().gateway_status_tool();
            advertised_names.insert(GATEWAY_STATUS_TOOL_NAME.to_string());
            descriptors.push(tool);
        }

        #[cfg(feature = "gateway")]
        if !hide_raw_tools && let Some(pool) = self.current_upstream_pool().await {
            let mut upstream_tool_count = 0usize;
            let upstream_tools = pool
                .healthy_tools_allowed(self.route_scope.allowed_upstreams())
                .await;
            for upstream_tool in upstream_tools {
                let name = upstream_tool.tool.name.as_ref();
                if builtin_names.contains(name) || !advertised_names.insert(name.to_string()) {
                    continue;
                }
                descriptors.push(upstream_tool.tool);
                upstream_tool_count += 1;
            }

            if let Some(subject) = self.audience.oauth_subject.as_deref() {
                let configs = self.route_scoped_oauth_upstream_configs().await;
                let subject_tool_limit = MAX_UPSTREAM_TOOLS.saturating_sub(upstream_tool_count);
                for (_, tools) in pool
                    .cached_subject_scoped_tools_bounded(&configs, subject, subject_tool_limit)
                    .await
                {
                    for tool in tools {
                        let name = tool.name.as_ref();
                        if builtin_names.contains(name)
                            || !advertised_names.insert(name.to_string())
                        {
                            continue;
                        }
                        descriptors.push(tool);
                    }
                }
            }
        }

        descriptors.sort_by(|left, right| left.name.cmp(&right.name));
        descriptors
    }

    pub(crate) async fn visible_contract(&self) -> ToolCatalogSnapshot {
        let descriptors = self.visible_tool_descriptors().await;
        ToolCatalogSnapshot::from_descriptors(&descriptors)
    }
}

#[cfg(test)]
mod tests {
    use super::{PeerCatalogAudience, PeerContract};
    use crate::mcp::route_scope::McpRouteScope;
    use crate::registry::ToolRegistry;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn contract(route_scope: McpRouteScope) -> PeerContract {
        PeerContract {
            registry: Arc::new(ToolRegistry::default()),
            #[cfg(feature = "gateway")]
            gateway_manager: None,
            route_scope,
            code_mode_app_state: Default::default(),
            audience: PeerCatalogAudience::default(),
        }
    }

    #[tokio::test]
    async fn restricted_contract_without_gateway_is_a_real_descriptor_hash() {
        let snapshot = contract(McpRouteScope::protected_subset(
            "descriptor-hash-test",
            std::iter::empty::<&str>(),
            std::iter::empty::<&str>(),
            false,
        ))
        .visible_contract()
        .await;
        assert_eq!(snapshot.tools.len(), 0);
        assert_ne!(snapshot.contract_hash, [0; 32]);
    }

    #[tokio::test]
    async fn route_scope_is_part_of_descriptor_collection() {
        let root = contract(McpRouteScope::Root).visible_contract().await;
        let scoped = contract(McpRouteScope::protected_subset(
            "ops",
            ["unifi"],
            ["gateway"],
            false,
        ))
        .visible_contract()
        .await;
        assert!(scoped.tools.is_subset(&root.tools));
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn cold_oauth_peer_contract_does_not_connect_upstream() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let upstream = crate::config::UpstreamConfig {
            enabled: true,
            name: "cold-oauth-contract".to_string(),
            url: Some(format!("http://{address}/mcp")),
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            proxy_skills: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            expose_skills: None,
            code_mode_hint: None,
            oauth: Some(crate::config::UpstreamOauthConfig {
                mode: crate::config::UpstreamOauthMode::AuthorizationCodePkce,
                registration: crate::config::UpstreamOauthRegistration::Preregistered {
                    client_id: "client-id".to_string(),
                    client_secret_env: None,
                },
                scopes: None,
                credential: Default::default(),
                prefer_client_metadata_document: None,
            }),
            imported_from: None,
            priority: 1.0,
        };
        let pool = Arc::new(crate::dispatch::upstream::pool::UpstreamPool::new());
        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        runtime.swap(Some(pool)).await;
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                std::path::PathBuf::from("config.toml"),
                runtime,
            ),
        );
        manager
            .seed_config_unchecked_for_tests(
                crate::config::LabConfig {
                    upstream: vec![upstream],
                    ..crate::config::LabConfig::default()
                }
                .to_gateway_config(),
            )
            .await;
        let peer_contract = PeerContract {
            registry: Arc::new(ToolRegistry::default()),
            gateway_manager: Some(manager),
            route_scope: McpRouteScope::Root,
            code_mode_app_state: Default::default(),
            audience: PeerCatalogAudience {
                code_mode_read_allowed: false,
                code_mode_execute_allowed: false,
                admin_apps_visible: false,
                oauth_subject: Some("reader".to_string()),
            },
        };

        tokio::time::timeout(
            std::time::Duration::from_millis(250),
            peer_contract.visible_contract(),
        )
        .await
        .expect("peer contract must return from cached state");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
                .await
                .is_err(),
            "peer contract rebuild must not cold-connect an OAuth upstream"
        );
    }
}
