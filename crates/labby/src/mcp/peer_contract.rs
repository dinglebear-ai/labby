//! Per-peer visible tool contract.
//!
//! `tools/list_changed` is a per-session statement — "the tool list *you* would
//! get from `tools/list` has changed" — but the catalog it is derived from is
//! global. Two sessions can see different contracts from the same gateway
//! state: `McpRouteScope` restricts which upstreams and services a route
//! exposes, and a protected route may set `expose_code_mode = false`, which
//! shows that session raw upstream tools while every other session sees the
//! constant `codemode` tool.
//!
//! Diffing one global projection and broadcasting the result therefore gets it
//! wrong in both directions: sessions are told about changes they cannot see,
//! and — the sharper failure — a raw-exposing route is told *nothing* when its
//! own tool set moves, because the global projection was computed under Code
//! Mode and filtered that movement out.
//!
//! `PeerContract` is the fix: it captures exactly the inputs a session's
//! visible tool list depends on, so the notification fanout can recompute each
//! peer's own contract and notify only the peers whose contract actually moved.
//!
//! It holds cheap clones (two `Arc`s and a small enum), never a `LabMcpServer`
//! — a server holds the peer registry, so storing one inside that registry
//! would close a reference cycle.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::mcp::catalog::{
    CODE_MODE_TOOL_NAME, CODE_MODE_UI_TOOL_NAME, CodeModeAppState, CodeModeVisibility,
    MCP_APP_TOOL_NAME, ToolCatalogSnapshot,
};
use crate::mcp::route_scope::McpRouteScope;
use crate::registry::ToolRegistry;

#[cfg(feature = "gateway")]
use crate::dispatch::gateway::manager::GatewayManager;
#[cfg(feature = "gateway")]
use crate::dispatch::upstream::pool::UpstreamPool;
#[cfg(feature = "gateway")]
use crate::mcp::call_tool_codemode::CodeModeUpstreamDescription;

/// Everything a session's visible tool list is derived from.
///
/// Kept deliberately minimal: `LabMcpServer` has these three fields plus
/// session-local state (logging level, relay id, transport label) that cannot
/// affect which tools are visible.
#[derive(Clone)]
pub(crate) struct PeerContract {
    pub(crate) registry: Arc<ToolRegistry>,
    #[cfg(feature = "gateway")]
    pub(crate) gateway_manager: Option<Arc<GatewayManager>>,
    pub(crate) route_scope: McpRouteScope,
    pub(crate) code_mode_app_state: CodeModeAppState,
}

impl PeerContract {
    /// Which Code Mode regime applies to this session.
    ///
    /// Route scope wins over global config: a protected route with
    /// `expose_code_mode = false` sees raw tools even while the gateway has
    /// Code Mode enabled for everyone else. That asymmetry is the reason the
    /// notification fanout cannot use one global regime for all peers.
    pub(crate) async fn code_mode_visibility(&self) -> CodeModeVisibility {
        #[cfg(feature = "gateway")]
        {
            if !self.route_scope.exposes_code_mode() {
                return CodeModeVisibility::Raw;
            }
            let manager_code_mode_enabled = if let Some(manager) = &self.gateway_manager {
                manager.code_mode_enabled().await
            } else {
                false
            };
            if manager_code_mode_enabled {
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

    /// Enabled upstreams this route can see, with their normalized hints — the
    /// determinants of the rendered `codemode` tool description.
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

    /// The tool-name projection this session would see from `tools/list`.
    ///
    /// This is the single implementation — `LabMcpServer::snapshot_tool_catalog`
    /// delegates here so a session's own view and the fanout's view of that
    /// session can never drift apart.
    pub(crate) async fn visible_tools(&self) -> BTreeSet<String> {
        let visibility = self.code_mode_visibility().await;
        let mut tools = BTreeSet::new();
        if visibility.exposes_synthetic_tools() {
            tools.insert(CODE_MODE_TOOL_NAME.to_string());
            tools.insert(MCP_APP_TOOL_NAME.to_string());
            if self.code_mode_app_state.is_enabled() {
                tools.insert(CODE_MODE_UI_TOOL_NAME.to_string());
            }
        } else {
            for svc in self.registry.services() {
                if !visibility.hides_raw_tools() && self.service_visible_on_mcp(svc.name).await {
                    tools.insert(svc.name.to_string());
                }
            }
        }

        #[cfg(feature = "gateway")]
        if let Some(pool) = self.current_upstream_pool().await {
            let upstream_tool_names = if visibility.hides_raw_tools() {
                pool.healthy_ui_tool_names_allowed(self.route_scope.allowed_upstreams())
                    .await
            } else {
                pool.healthy_tool_names_allowed(self.route_scope.allowed_upstreams())
                    .await
            };
            for tool_name in upstream_tool_names {
                tools.insert(tool_name);
            }
        }

        tools
    }

    /// The session's full visible contract: its tool names plus the
    /// determinants of the `codemode` tool *description*.
    ///
    /// The description embeds this route's enabled upstream namespaces and
    /// their hints, so an operator editing a hint changes what the session sees
    /// without changing any tool name. A name-set-only comparison is blind to
    /// that; folding the determinants in as synthetic tokens keeps one
    /// comparable set. Unlike the gateway-side reconcile approximation these
    /// tokens are route-filtered, so a hint on an upstream this route cannot
    /// see does not notify it.
    pub(crate) async fn visible_contract(&self) -> ToolCatalogSnapshot {
        let mut tools = self.visible_tools().await;
        tools.extend(self.description_tokens().await);
        ToolCatalogSnapshot { tools }
    }

    #[cfg(feature = "gateway")]
    async fn description_tokens(&self) -> BTreeSet<String> {
        // Only the synthetic `codemode` tool carries a description built from
        // upstream state; a raw-exposing route has no such coupling.
        if !self.code_mode_visibility().await.exposes_synthetic_tools() {
            return BTreeSet::new();
        }
        self.code_mode_upstreams_for_description()
            .await
            .into_iter()
            .map(|upstream| {
                // \u{1} cannot appear in an upstream or tool name, so these
                // tokens stay disjoint from the tool names they share a set
                // with. Decoded before logging, never rendered raw.
                format!(
                    "\u{1}ns\u{1}{}\u{1}{}",
                    upstream.name,
                    upstream.hint.unwrap_or_default()
                )
            })
            .collect()
    }

    #[cfg(not(feature = "gateway"))]
    async fn description_tokens(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
}

#[cfg(test)]
mod tests {
    use super::PeerContract;
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
        }
    }

    #[tokio::test]
    async fn contract_without_gateway_exposes_no_description_tokens() {
        // No gateway manager means no Code Mode and no upstream namespaces —
        // the contract must be a plain (here empty) tool-name set rather than
        // carrying stray sentinel tokens.
        let snapshot = contract(McpRouteScope::Root).visible_contract().await;

        assert!(
            !snapshot.tools.iter().any(|tool| tool.starts_with('\u{1}')),
            "no namespace tokens without a gateway"
        );
    }

    #[tokio::test]
    async fn route_scope_is_part_of_the_contract_identity() {
        // Two scopes over identical global state are allowed to produce
        // identical contracts; what matters is that the scope is an input at
        // all, so the fanout can ask "what does *this* peer see".
        let root = contract(McpRouteScope::Root).visible_contract().await;
        let scoped = contract(McpRouteScope::protected_subset(
            "ops",
            ["unifi"],
            ["gateway"],
            false,
        ))
        .visible_contract()
        .await;

        // Root exposes builtin service tools; a subset route that allows only
        // `gateway` cannot expose more than root does.
        assert!(scoped.tools.is_subset(&root.tools));
    }
}
