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
use crate::mcp::completion::action_schema;
use crate::mcp::handlers_tools::server_logs_tool_meta;
use crate::mcp::route_scope::McpRouteScope;
use crate::registry::ToolRegistry;

#[cfg(feature = "gateway")]
use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
#[cfg(feature = "gateway")]
use crate::dispatch::gateway::manager::GatewayManager;
#[cfg(feature = "gateway")]
use crate::dispatch::upstream::pool::UpstreamPool;
#[cfg(feature = "gateway")]
use crate::mcp::call_tool_codemode::CodeModeUpstreamDescription;
#[cfg(feature = "gateway")]
use crate::mcp::catalog::{
    ADD_SERVER_TOOL_NAME, CODE_MODE_UI_TOOL_NAME, GATEWAY_STATUS_TOOL_NAME, MCP_APP_TOOL_NAME,
};
#[cfg(feature = "gateway")]
use crate::mcp::handlers_tools::{
    add_server_tool_meta, add_server_tool_schema, code_mode_execute_schema, code_mode_tool_meta,
    code_mode_trace_output_schema, code_mode_ui_description, gateway_status_tool_meta,
    gateway_status_tool_schema, mcp_app_tool_description, mcp_app_tool_schema,
};

/// Request-derived inputs that affect the descriptor set for one peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PeerCatalogAudience {
    pub(crate) code_mode_execute_allowed: bool,
    pub(crate) admin_apps_visible: bool,
    #[cfg(feature = "gateway")]
    pub(crate) oauth_subject: Option<String>,
}

impl Default for PeerCatalogAudience {
    fn default() -> Self {
        Self {
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
    /// text-only `codemode` and the `mcp_app` control tool never depend on it.
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

    pub(crate) async fn service_visible_on_mcp(&self, service: &str) -> bool {
        if !self.route_scope.allows_service(service) {
            return false;
        }
        #[cfg(feature = "gateway")]
        match &self.gateway_manager {
            Some(manager) => manager.surface_enabled_for_service(service, "mcp").await,
            None => true,
        }
        #[cfg(not(feature = "gateway"))]
        true
    }

    #[cfg(feature = "gateway")]
    async fn action_allowed_on_mcp(&self, service: &str, action: &str) -> bool {
        match &self.gateway_manager {
            Some(manager) => {
                manager
                    .mcp_action_allowed_for_service(service, action)
                    .await
            }
            None => true,
        }
    }

    #[cfg(feature = "gateway")]
    async fn add_server_app_available(&self) -> bool {
        self.route_scope.allows_service("gateway")
            && self.gateway_manager.is_some()
            && self.registry.service("gateway").is_some()
            && self.service_visible_on_mcp("gateway").await
            && self.action_allowed_on_mcp("gateway", "gateway.test").await
            && self.action_allowed_on_mcp("gateway", "gateway.add").await
    }

    #[cfg(feature = "gateway")]
    async fn gateway_status_app_available(&self) -> bool {
        self.route_scope.allows_service("gateway")
            && self.gateway_manager.is_some()
            && self.registry.service("gateway").is_some()
            && self.service_visible_on_mcp("gateway").await
            && self.action_allowed_on_mcp("gateway", "gateway.list").await
    }

    /// Enabled upstream namespaces and normalized hints rendered in codemode's
    /// descriptor. Descriptor hashing therefore catches a hint-only edit.
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
        let schema = Arc::new(action_schema());
        let mut descriptors = Vec::new();
        let mut builtin_names = HashSet::new();
        let mut advertised_names = HashSet::new();

        for service in self.registry.services() {
            if self.service_visible_on_mcp(service.name).await {
                builtin_names.insert(service.name.to_string());
                if hide_raw_tools && service.name != SERVER_LOGS_TOOL_NAME {
                    continue;
                }
                let tool = Tool::new(service.name, service.description, Arc::clone(&schema));
                let tool =
                    if service.name == SERVER_LOGS_TOOL_NAME && self.audience.admin_apps_visible {
                        tool.with_meta(server_logs_tool_meta(service.name))
                    } else {
                        tool
                    };
                advertised_names.insert(service.name.to_string());
                descriptors.push(tool);
            }
        }

        #[cfg(feature = "gateway")]
        if visibility.exposes_synthetic_tools() && self.audience.code_mode_execute_allowed {
            let upstreams = self.code_mode_upstreams_for_description().await;
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
                let tool = Tool::new(
                    CODE_MODE_UI_TOOL_NAME,
                    code_mode_ui_description(&upstreams),
                    code_mode_execute_schema(),
                )
                .with_raw_output_schema(code_mode_trace_output_schema())
                .with_meta(code_mode_tool_meta(CODE_MODE_UI_TOOL_NAME));
                advertised_names.insert(CODE_MODE_UI_TOOL_NAME.to_string());
                descriptors.push(tool);
            }

            // Always advertised alongside codemode: the control tool is how a
            // disabled app surface gets restored.
            let tool = Tool::new(
                MCP_APP_TOOL_NAME,
                mcp_app_tool_description(),
                mcp_app_tool_schema(),
            );
            advertised_names.insert(MCP_APP_TOOL_NAME.to_string());
            descriptors.push(tool);
        }

        #[cfg(feature = "gateway")]
        if self.audience.admin_apps_visible && self.add_server_app_available().await {
            let tool = Tool::new(
                ADD_SERVER_TOOL_NAME,
                "Open a responsive form to test and add a remote or local MCP server to the Labby gateway catalog.",
                add_server_tool_schema(),
            )
            .with_meta(add_server_tool_meta(ADD_SERVER_TOOL_NAME));
            advertised_names.insert(ADD_SERVER_TOOL_NAME.to_string());
            descriptors.push(tool);
        }

        #[cfg(feature = "gateway")]
        if self.audience.admin_apps_visible && self.gateway_status_app_available().await {
            let tool = Tool::new(
                GATEWAY_STATUS_TOOL_NAME,
                "Display live connection status, capabilities, and warnings for gateway upstream MCP servers.",
                gateway_status_tool_schema(),
            )
            .with_meta(gateway_status_tool_meta(GATEWAY_STATUS_TOOL_NAME));
            advertised_names.insert(GATEWAY_STATUS_TOOL_NAME.to_string());
            descriptors.push(tool);
        }

        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await {
            let upstream_tools = if hide_raw_tools {
                pool.healthy_ui_tools_allowed(self.route_scope.allowed_upstreams())
                    .await
            } else {
                pool.healthy_tools_allowed(self.route_scope.allowed_upstreams())
                    .await
            };
            for upstream_tool in upstream_tools {
                let name = upstream_tool.tool.name.as_ref();
                if builtin_names.contains(name) || !advertised_names.insert(name.to_string()) {
                    continue;
                }
                descriptors.push(upstream_tool.tool);
            }

            if !hide_raw_tools && let Some(subject) = self.audience.oauth_subject.as_deref() {
                let configs = self.route_scoped_oauth_upstream_configs().await;
                for (_, tools) in pool.subject_scoped_tools(&configs, subject).await {
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
}
