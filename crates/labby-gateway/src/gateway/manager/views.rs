//! Read-only inspection surface: `list`, `get`, `status`, `test`, discovered
//! tool/resource/prompt views, surface gating checks, and client config export.

use crate::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::gateway::params::GatewayEnrichmentScope;
use crate::gateway::projection::*;
use crate::gateway::types::{
    GatewayRuntimeView, GatewayToolExposureRowView, GatewayView, McpClientConfigView,
    McpClientTransportType,
};
use crate::gateway::view_models::ServerView;
use crate::upstream::pool::in_process_upstream_name;
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::UpstreamConfig;

use super::GatewayManager;

fn upstream_revision(upstream: &UpstreamConfig) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(upstream).expect("upstream config serializes");
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}
use super::virtual_servers::find_virtual_server;

const WARNING_UNKNOWN_SERVICE: &str = "unknown_service";

fn find_virtual_server_for_service<'a>(
    virtual_servers: &'a [labby_runtime::gateway_config::VirtualServerConfig],
    service: &str,
) -> Option<&'a labby_runtime::gateway_config::VirtualServerConfig> {
    virtual_servers
        .iter()
        .find(|server| server.service == service || server.id == service)
}

/// Effective MCP exposure for a registered built-in service. Absence remains
/// distinct from an explicitly hidden virtual server; non-empty allowlists
/// retain the implicit `help` and `schema` compatibility actions.
pub(super) enum McpServicePolicy<'a> {
    Absent,
    Hidden,
    Unrestricted,
    Allowlisted(&'a [String]),
}

pub(super) fn mcp_service_policy_for_config<'a>(
    virtual_servers: &'a [labby_runtime::gateway_config::VirtualServerConfig],
    service: &str,
) -> McpServicePolicy<'a> {
    let Some(server) = find_virtual_server_for_service(virtual_servers, service) else {
        return McpServicePolicy::Absent;
    };
    if !server.enabled || !server.surfaces.mcp {
        return McpServicePolicy::Hidden;
    }
    match server.mcp_policy.as_ref() {
        Some(policy) if !policy.allowed_actions.is_empty() => {
            McpServicePolicy::Allowlisted(&policy.allowed_actions)
        }
        _ => McpServicePolicy::Unrestricted,
    }
}

impl GatewayManager {
    /// Live inbound MCP client/session list — see
    /// `labby_runtime::client_registry` for the best-effort/pruning caveat.
    /// Empty when no transport has wired `with_client_registry`.
    pub async fn clients(
        &self,
    ) -> Result<Vec<crate::gateway::types::GatewayClientView>, ToolError> {
        Ok(self
            .client_registry
            .list()
            .await
            .into_iter()
            .map(|client| crate::gateway::types::GatewayClientView {
                subject: client.subject_tag,
                client_name: client.client_name,
                client_version: client.client_version,
                transport: client.transport,
                connected_at: client.connected_at,
            })
            .collect())
    }

    pub async fn refresh_gateway_status_catalog(
        &self,
        scope: &GatewayEnrichmentScope,
        name: Option<&str>,
    ) {
        let (cfg, pool) = self.published_config_and_pool().await;
        let allowed_upstreams = match (scope.route_visible_upstreams.as_ref(), name) {
            (Some(allowed), Some(name)) => Some(
                allowed
                    .iter()
                    .filter(|upstream| upstream.as_str() == name)
                    .cloned()
                    .collect(),
            ),
            (Some(allowed), None) => Some(allowed.clone()),
            (None, Some(name)) => Some(std::iter::once(name.to_owned()).collect()),
            (None, None) => None,
        };
        self.refresh_mcp_runtime_catalog_bounded(
            &cfg,
            pool.as_deref(),
            allowed_upstreams.as_ref(),
            scope.oauth_subject.as_deref(),
            "gateway.status.refresh",
        )
        .await;
    }

    pub async fn list(&self) -> Result<Vec<ServerView>, ToolError> {
        let (cfg, pool) = self.published_config_and_pool().await;
        // Inspection must remain side-effect free. Project whatever the runtime
        // has already observed; callers that need fresh discovery use the
        // explicit status refresh or per-upstream test/reload actions.
        let mut views = Vec::with_capacity(cfg.upstream.len() + cfg.virtual_servers.len());
        for upstream in &cfg.upstream {
            views.push(server_view_from_upstream(pool.as_deref(), upstream).await);
        }
        for virtual_server in &cfg.virtual_servers {
            let peer_name = in_process_upstream_name(&virtual_server.service);
            let summary = upstream_summary(pool.as_deref(), &peer_name).await;
            let last_error = operator_visible_upstream_error(match pool.as_deref() {
                Some(pool) => pool.upstream_last_error(&peer_name).await,
                None => None,
            });
            views.push(server_view_from_virtual_server(
                virtual_server,
                summary,
                last_error,
                None,
                self.builtin_service_registry().as_ref(),
            ));
        }
        let unknown_service_count = degraded_server_warning_count(&views, WARNING_UNKNOWN_SERVICE);
        if unknown_service_count > 0 {
            tracing::warn!(
                action = "gateway.list",
                unknown_service_count,
                "gateway list returned degraded rows with unknown services"
            );
        }
        Ok(views)
    }

    pub async fn list_scoped(
        &self,
        scope: &GatewayEnrichmentScope,
    ) -> Result<Vec<ServerView>, ToolError> {
        let mut views = self.list().await?;
        if let Some(visible) = scope.route_visible_upstreams.as_ref() {
            views.retain(|view| view.source == "custom_gateway" && visible.contains(&view.id));
        }
        Ok(views)
    }

    pub async fn get_server(&self, id: &str) -> Result<ServerView, ToolError> {
        let (cfg, pool) = self.published_config_and_pool().await;

        if let Some(upstream) = cfg.upstream.iter().find(|upstream| upstream.name == id) {
            return Ok(server_view_from_upstream(pool.as_deref(), upstream).await);
        }

        let virtual_server = find_virtual_server(&cfg, id)?;
        let peer_name = in_process_upstream_name(&virtual_server.service);
        let summary = upstream_summary(pool.as_deref(), &peer_name).await;
        let last_error = operator_visible_upstream_error(match pool.as_deref() {
            Some(pool) => pool.upstream_last_error(&peer_name).await,
            None => None,
        });
        Ok(server_view_from_virtual_server(
            virtual_server,
            summary,
            last_error,
            None,
            self.builtin_service_registry().as_ref(),
        ))
    }

    pub async fn get(&self, name: &str) -> Result<GatewayView, ToolError> {
        let (cfg, pool) = self.published_config_and_pool().await;
        let code_mode = cfg.code_mode.clone();
        let upstream = cfg
            .upstream
            .iter()
            .find(|u| u.name == name)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("gateway `{name}` not found"),
            })?
            .clone();
        Ok(GatewayView {
            revision: upstream_revision(&upstream),
            config: config_view(&upstream, &code_mode),
            runtime: runtime_view(pool.as_deref(), &upstream.name, None).await,
            enrichment_suggestion: None,
            enrichment_suggestion_error: None,
        })
    }

    pub async fn get_scoped(
        &self,
        name: &str,
        scope: &GatewayEnrichmentScope,
    ) -> Result<GatewayView, ToolError> {
        scope.ensure_visible(name)?;
        self.get(name).await
    }

    pub async fn surface_enabled_for_service(&self, service: &str, surface: &str) -> bool {
        if self.registered_service_meta(service).is_none() {
            return true;
        }

        let cfg = self.config.read().await;
        let Some(virtual_server) = find_virtual_server_for_service(&cfg.virtual_servers, service)
        else {
            return surface != "mcp";
        };

        if !virtual_server.enabled {
            return false;
        }

        match surface {
            "cli" => virtual_server.surfaces.cli,
            "api" => virtual_server.surfaces.api,
            "mcp" => virtual_server.surfaces.mcp,
            "webui" => virtual_server.surfaces.webui,
            _ => false,
        }
    }

    pub async fn allowed_mcp_actions_for_service(&self, service: &str) -> Option<Vec<String>> {
        let cfg = self.config.read().await;
        match mcp_service_policy_for_config(&cfg.virtual_servers, service) {
            McpServicePolicy::Absent => None,
            McpServicePolicy::Hidden => Some(Vec::new()),
            McpServicePolicy::Unrestricted => None,
            McpServicePolicy::Allowlisted(actions) => {
                let mut allowed = vec!["help".to_string(), "schema".to_string()];
                allowed.extend(actions.iter().cloned());
                Some(allowed)
            }
        }
    }

    pub async fn mcp_action_allowed_for_service(&self, service: &str, action: &str) -> bool {
        let cfg = self.config.read().await;
        match mcp_service_policy_for_config(&cfg.virtual_servers, service) {
            McpServicePolicy::Absent => true,
            McpServicePolicy::Hidden => false,
            McpServicePolicy::Unrestricted => true,
            McpServicePolicy::Allowlisted(actions) => {
                matches!(action, "help" | "schema")
                    || actions.iter().any(|allowed| allowed == action)
            }
        }
    }

    pub async fn status(&self, name: Option<&str>) -> Result<Vec<GatewayRuntimeView>, ToolError> {
        self.status_scoped(name, &GatewayEnrichmentScope::default())
            .await
    }

    pub async fn status_scoped(
        &self,
        name: Option<&str>,
        scope: &GatewayEnrichmentScope,
    ) -> Result<Vec<GatewayRuntimeView>, ToolError> {
        if let Some(name) = name {
            scope.ensure_visible(name)?;
        }
        let (cfg, pool) = self.published_config_and_pool().await;
        let upstreams: Vec<UpstreamConfig> = cfg
            .upstream
            .iter()
            .filter(|upstream| {
                name.is_none_or(|needle| needle == upstream.name)
                    && scope
                        .route_visible_upstreams
                        .as_ref()
                        .is_none_or(|visible| visible.contains(&upstream.name))
            })
            .cloned()
            .collect();
        // P-M8: use the cached prompt-ownership snapshot instead of a live
        // prompts/list fan-out on every status poll (mirrors the resources fix
        // for lab-mzm2 — same pattern, same rationale).
        let prompt_owners = match pool.as_deref() {
            Some(p) => Some(p.cached_prompt_ownership_map().await),
            None => None,
        };
        let mut items = Vec::new();
        for upstream in &upstreams {
            items.push(runtime_view(pool.as_deref(), &upstream.name, prompt_owners.as_ref()).await);
        }
        Ok(items)
    }

    pub async fn test(
        &self,
        spec_or_name: Result<&UpstreamConfig, &str>,
    ) -> Result<GatewayRuntimeView, ToolError> {
        let upstream = match spec_or_name {
            Ok(spec) => spec.clone(),
            Err(name) => {
                let cfg = self.config.read().await;
                cfg.upstream
                    .iter()
                    .find(|u| u.name == name)
                    .cloned()
                    .ok_or_else(|| ToolError::Sdk {
                        sdk_kind: "not_found".to_string(),
                        message: format!("gateway `{name}` not found"),
                    })?
            }
        };

        let (request_timeout, relay_timeout) = {
            let cfg = self.config.read().await;
            (cfg.upstream_request_timeout(), cfg.upstream_relay_timeout())
        };
        let pool = self.new_base_pool(request_timeout, relay_timeout);
        let registry = self.builtin_service_registry();
        pool.discover_all_for_subject_ephemeral_with_in_process_peers(
            &[upstream.clone()],
            SHARED_GATEWAY_OAUTH_SUBJECT,
            registry.as_ref(),
        )
        .await;

        let view = runtime_view(Some(&pool), &upstream.name, None).await;
        pool.drain_for_swap("gateway.test.ephemeral").await;
        Ok(view)
    }

    pub async fn client_config(&self, name: &str) -> Result<McpClientConfigView, ToolError> {
        let upstream = self
            .upstream_config(name)
            .await
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("gateway `{name}` not found"),
            })?;

        if let Some(url) = upstream.url.clone() {
            return Ok(McpClientConfigView {
                name: upstream.name,
                r#type: McpClientTransportType::Http,
                url: Some(url),
                command: None,
                args: None,
                env: None,
            });
        }

        let Some(command) = upstream.command.clone() else {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_config".to_string(),
                message: format!("gateway `{name}` has neither url nor command configured"),
            });
        };

        Ok(McpClientConfigView {
            name: upstream.name,
            r#type: McpClientTransportType::Stdio,
            url: None,
            command: Some(command),
            args: (!upstream.args.is_empty()).then_some(upstream.args),
            env: None,
        })
    }

    pub async fn discovered_tools(
        &self,
        name: &str,
    ) -> Result<Vec<GatewayToolExposureRowView>, ToolError> {
        let Some(pool) = self.runtime.current_pool().await else {
            return Ok(Vec::new());
        };

        Ok(pool
            .tool_exposure_rows(name)
            .await
            .into_iter()
            .map(|row| GatewayToolExposureRowView {
                name: row.name,
                description: row.description,
                exposed: row.exposed,
                matched_by: row.matched_by,
            })
            .collect())
    }

    pub async fn discovered_resources(&self, name: &str) -> Result<Vec<String>, ToolError> {
        let Some(pool) = self.runtime.current_pool().await else {
            return Ok(Vec::new());
        };
        // Serve from the cached resource URI snapshot to avoid a live fan-out
        // RPC burst on every admin inspection call (lab-mzm2).
        let all = pool.cached_upstream_resource_uris().await;
        let mut resources: Vec<String> = all
            .into_iter()
            .filter(|(upstream_name, _)| upstream_name == name)
            .flat_map(|(_, uris)| uris)
            .collect();
        resources.sort();
        Ok(resources)
    }

    pub async fn discovered_prompts(&self, name: &str) -> Result<Vec<String>, ToolError> {
        let Some(pool) = self.runtime.current_pool().await else {
            return Ok(Vec::new());
        };
        // Serve from the cached prompt name snapshot to avoid a live fan-out
        // RPC burst on every admin inspection call (lab-mzm2).
        let all = pool.cached_upstream_prompt_names_by_upstream().await;
        let mut prompts: Vec<String> = all
            .into_iter()
            .filter(|(upstream_name, _)| upstream_name == name)
            .flat_map(|(_, names)| names)
            .collect();
        prompts.sort();
        Ok(prompts)
    }

    pub async fn gateway_servers_doc(&self) -> Result<serde_json::Value, ToolError> {
        self.gateway_servers_doc_scoped(&GatewayEnrichmentScope::default())
            .await
    }

    pub async fn gateway_servers_doc_scoped(
        &self,
        scope: &GatewayEnrichmentScope,
    ) -> Result<serde_json::Value, ToolError> {
        let Some(pool) = self.runtime.current_pool().await else {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: "upstream pool not configured".to_string(),
            });
        };
        let mut document = pool
            .gateway_servers_doc_allowed(scope.route_visible_upstreams.as_ref())
            .await;
        let oauth_upstreams: std::collections::BTreeSet<String> = self
            .oauth_upstream_configs()
            .await
            .into_iter()
            .map(|config| config.name)
            .collect();
        if let Some(servers) = document
            .get_mut("servers")
            .and_then(serde_json::Value::as_array_mut)
        {
            for server in servers {
                let Some(name) = server.get("name").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                if oauth_upstreams.contains(name)
                    && let Some(row) = server.as_object_mut()
                {
                    // OAuth tools are intentionally absent from the shared
                    // catalog. A zero/healthy row is therefore misleading:
                    // discovery must use gateway.schema with this request's
                    // verified subject.
                    row.insert("tool_count".to_string(), serde_json::Value::Null);
                    row.insert(
                        "tool_health".to_string(),
                        serde_json::Value::String("not_probed".to_string()),
                    );
                    row.insert(
                        "discovery_mode".to_string(),
                        serde_json::Value::String("request_scoped".to_string()),
                    );
                }
            }
        }
        Ok(document)
    }

    pub async fn gateway_server_schema(&self, name: &str) -> Result<serde_json::Value, ToolError> {
        self.gateway_server_schema_scoped(name, &GatewayEnrichmentScope::default())
            .await
    }

    pub async fn gateway_server_schema_scoped(
        &self,
        name: &str,
        scope: &GatewayEnrichmentScope,
    ) -> Result<serde_json::Value, ToolError> {
        let Some(pool) = self.runtime.current_pool().await else {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: "upstream pool not configured".to_string(),
            });
        };
        scope.ensure_visible(name)?;
        if let Some(config) = self.oauth_upstream_config(name).await {
            let subject = scope
                .oauth_subject
                .as_deref()
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "auth_failed".to_string(),
                    message: format!(
                        "upstream `{name}` requires an authenticated subject for schema discovery"
                    ),
                })?;
            return pool
                .subject_scoped_gateway_server_schema(&config, subject)
                .await;
        }
        pool.gateway_server_schema_allowed(name, scope.route_visible_upstreams.as_ref())
            .await
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("unknown upstream: {name}"),
            })
    }
}
