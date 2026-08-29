use serde::de::DeserializeOwned;
use serde_json::Value;

#[cfg(feature = "skills")]
use futures::{StreamExt, stream};

use crate::dispatch_helpers::{action_schema, handle_builtin, help_payload, require_str, to_json};
#[cfg(feature = "skills")]
use crate::upstream::pool::OperatorSkills;
use labby_runtime::error::ToolError;

use super::SHARED_GATEWAY_OAUTH_SUBJECT;
use super::catalog::ACTIONS;
use super::client::require_gateway_manager;
use super::manager::{GatewayManager, ImportTombstoneSelector};
use super::params::{
    CodeModeSetParams, GatewayAddParams, GatewayClientConfigParams, GatewayDiscoverParams,
    GatewayEnrichApplyParams, GatewayEnrichPreviewParams, GatewayEnrichmentScope,
    GatewayImportParams, GatewayImportTombstoneParams, GatewayMcpCleanupParams,
    GatewayMcpRestartParams, GatewayMcpToggleParams, GatewayNameParams, GatewayOauthNameParams,
    GatewayReloadParams, GatewayStatusParams, GatewayTestParams, GatewayUpdateParams,
    GatewayUpdatePatch, GatewayUsageCallsParams, GatewayUsageMetricsParams, LoadoutNameParams,
    LoadoutPatchParams, LoadoutSpecParams, LoadoutUpdateParams, ProtectedRouteNameParams,
    ProtectedRouteSpecParams, ProtectedRouteUpdateParams, ResourceLeaseCreateParams,
    ResourceLeaseReleaseParams, ResourceLeaseRenewParams, ServiceConfigGetParams,
    ServiceConfigSetParams, VirtualServerMcpPolicyParams, VirtualServerNameParams,
    VirtualServerSurfaceParams,
};
use super::types::{
    DiscoveredServerView, ImportErrorView, ImportSkipReason, ImportSkipView,
    McpClientTransportType, ServiceActionView,
};

fn parse_params<T: DeserializeOwned>(params_value: Value) -> Result<T, ToolError> {
    serde_json::from_value(params_value).map_err(|e| ToolError::InvalidParam {
        message: format!("invalid gateway params: {e}"),
        param: "params".to_string(),
    })
}

fn reject_shared_oauth_subject_override(params: &Value) -> Result<(), ToolError> {
    if params.get("subject").is_some() {
        return Err(ToolError::InvalidParam {
            message: "shared gateway OAuth actions do not accept a subject override".to_string(),
            param: "subject".to_string(),
        });
    }
    Ok(())
}

pub async fn dispatch_with_manager(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    dispatch_with_manager_scoped(
        manager,
        action,
        params_value,
        GatewayEnrichmentScope::default(),
    )
    .await
}

pub async fn dispatch_with_manager_scoped(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
    enrichment_scope: GatewayEnrichmentScope,
) -> Result<Value, ToolError> {
    // Defense-in-depth: built-ins handled here so direct callers of
    // dispatch_with_manager (e.g. HTTP handlers) also get the correct behavior.
    if let Some(result) = handle_builtin(action, &params_value, "gateway", ACTIONS) {
        return result;
    }
    match action {
        "gateway.skills.list" => handle_skills_list(manager, params_value, enrichment_scope).await,
        "gateway.code_mode.get" | "gateway.code_mode.set" => {
            handle_tool_actions(manager, action, params_value).await
        }
        "gateway.discover" => handle_discover(manager, params_value).await,
        "gateway.enrich.preview" => {
            let params: GatewayEnrichPreviewParams = parse_params(params_value)?;
            to_json(
                manager
                    .preview_enrichment_scoped(params, enrichment_scope)
                    .await?,
            )
        }
        "gateway.enrich.apply" => {
            let params: GatewayEnrichApplyParams = parse_params(params_value)?;
            to_json(
                manager
                    .apply_enrichment_scoped(params, enrichment_scope)
                    .await?,
            )
        }
        "gateway.usage.metrics" => {
            let params: GatewayUsageMetricsParams = parse_params(params_value)?;
            to_json(
                manager
                    .usage_metrics_scoped(params, enrichment_scope)
                    .await?,
            )
        }
        "gateway.usage.calls" => {
            let params: GatewayUsageCallsParams = parse_params(params_value)?;
            to_json(manager.usage_calls_scoped(params, enrichment_scope).await?)
        }
        "gateway.import" => handle_import(manager, params_value, enrichment_scope).await,
        "gateway.import_pending.list" => {
            let mut pending = manager.list_pending_imports().await;
            if let Some(visible) = enrichment_scope.route_visible_upstreams.as_ref() {
                pending.retain(|candidate| visible.contains(&candidate.name));
            }
            to_json(pending)
        }
        "gateway.import_pending.approve" => {
            let name = require_str(&params_value, "name")?;
            // Deliberately NOT `ensure_visible`. Approving is a *creation*
            // operation and is unscoped for the same reason `gateway.add` is: the
            // upstream it creates does not exist yet, so a visibility check has
            // nothing to check against. Note this means a pending name that *is*
            // route-visible (`import_pending.list` filters on exactly that) is
            // still approved without a visibility check — the carve-out is on the
            // operation, not on the name. `reject` stays scoped because it only
            // tombstones a row the route can already see.
            //
            // Creation from a subset route is therefore an accepted gap, not an
            // enforced boundary; `docs/runtime/OAUTH.md` says so explicitly. The
            // scope is still threaded through so `approve_pending_import_scoped`
            // suppresses an enrichment suggestion naming a route-hidden upstream.
            to_json(
                manager
                    .approve_pending_import_scoped(name, enrichment_scope)
                    .await?,
            )
        }
        "gateway.import_pending.reject" => {
            let name = require_str(&params_value, "name")?;
            enrichment_scope.ensure_visible(name)?;
            to_json(manager.reject_pending_import(name).await?)
        }
        "gateway.import_tombstones.list"
        | "gateway.import_tombstones.clear"
        | "gateway.import_tombstones.restore" => {
            handle_import_tombstone_actions(manager, action, params_value, enrichment_scope).await
        }
        "gateway.servers" => to_json(
            manager
                .gateway_servers_doc_scoped(&enrichment_scope)
                .await?,
        ),
        "gateway.schema" => {
            // Keep the established missing-parameter envelope stable before
            // typed deserialization; `GatewayNameParams` alone reports a
            // serde-shaped message that differs across dispatch surfaces.
            require_str(&params_value, "name")?;
            let params: GatewayNameParams = parse_params(params_value)?;
            to_json(
                manager
                    .gateway_server_schema_scoped(&params.name, &enrichment_scope)
                    .await?,
            )
        }
        "gateway.list"
        | "gateway.server.get"
        | "gateway.supported_services"
        | "gateway.get"
        | "gateway.test"
        | "gateway.add"
        | "gateway.update"
        | "gateway.remove"
        | "gateway.reload"
        | "gateway.status"
        | "gateway.client_config.get"
        | "gateway.discovered_tools"
        | "gateway.discovered_resources"
        | "gateway.discovered_prompts"
        | "gateway.public_urls.get" => {
            handle_gateway_actions(manager, action, params_value, enrichment_scope).await
        }
        action if action.starts_with("gateway.loadout.") => {
            handle_loadout_actions(manager, action, params_value).await
        }
        action if action.starts_with("gateway.protected_route.") => {
            handle_protected_route_actions(manager, action, params_value).await
        }
        action if action.starts_with("gateway.virtual_server.") => {
            handle_virtual_server_actions(manager, action, params_value).await
        }
        action if action.starts_with("gateway.service_") => {
            handle_service_actions(manager, action, params_value).await
        }
        action if action.starts_with("gateway.oauth.") => {
            handle_oauth_actions(manager, action, params_value, enrichment_scope).await
        }
        action if action.starts_with("gateway.mcp.") => {
            handle_mcp_actions(manager, action, params_value, enrichment_scope).await
        }
        unknown => unknown_action(unknown),
    }
}

const KNOWN_CLIENTS: &[&str] = &[
    "cursor",
    "claude-code",
    "claude-desktop",
    "codex",
    "windsurf",
    "opencode",
    "vscode",
    "gemini",
];

async fn handle_discover(
    manager: &GatewayManager,
    params_value: Value,
) -> Result<Value, ToolError> {
    let params: GatewayDiscoverParams = parse_params(params_value)?;

    for client in &params.clients {
        if !KNOWN_CLIENTS.contains(&client.as_str()) {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "unknown client kind: '{}'. Valid: {}",
                    client,
                    KNOWN_CLIENTS.join(", ")
                ),
                param: "clients".to_string(),
            });
        }
    }

    let home = super::discovery::home_dir().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "cannot determine home directory".to_string(),
    })?;

    let mut discovered = tokio::task::spawn_blocking(move || super::discovery::discover_all(&home))
        .await
        .map_err(|e| ToolError::internal_message(format!("discovery task panicked: {e}")))?;
    if !params.clients.is_empty() {
        let filter: std::collections::HashSet<&str> =
            params.clients.iter().map(String::as_str).collect();
        discovered.retain(|s| filter.contains(s.source_client.as_str()));
    }

    let cfg = manager.current_config().await;
    let existing: std::collections::HashSet<String> =
        cfg.upstream.iter().map(|u| u.name.clone()).collect();

    let views = shape_discovered_views(discovered, &cfg, &existing, &params);

    to_json(views)
}

fn shape_discovered_views(
    discovered: Vec<super::discovery::DiscoveredServer>,
    cfg: &labby_runtime::gateway_config::GatewayConfig,
    existing: &std::collections::HashSet<String>,
    params: &GatewayDiscoverParams,
) -> Vec<DiscoveredServerView> {
    discovered
        .into_iter()
        .filter(|s| params.include_existing || !existing.contains(&s.name))
        .map(|s| {
            let tombstoned = super::manager::discovered_is_tombstoned(cfg, &s);
            let transport = if s.spec.url.is_some() {
                McpClientTransportType::Http
            } else {
                McpClientTransportType::Stdio
            };
            let command_preview = s.spec.command.as_ref().map(|c| {
                c.split_whitespace()
                    .next()
                    .unwrap_or(c.as_str())
                    .to_string()
            });
            DiscoveredServerView {
                name: s.name,
                source_client: s.source_client,
                source_path: s.source_path,
                transport,
                command_preview,
                url_preview: s.spec.url.as_deref().map(redact_url_preview),
                env_key_count: s.env_key_count,
                already_configured: existing.contains(&s.spec.name),
                transport_fingerprint: s
                    .spec
                    .imported_from
                    .as_ref()
                    .and_then(|source| source.transport_fingerprint.clone()),
                tombstoned,
            }
        })
        .collect()
}

async fn handle_import(
    manager: &GatewayManager,
    params_value: Value,
    enrichment_scope: GatewayEnrichmentScope,
) -> Result<Value, ToolError> {
    let params: GatewayImportParams = parse_params(params_value)?;

    if !params.names.is_empty() && params.all {
        return Err(ToolError::InvalidParam {
            message: "gateway.import requires either `all` or `names`, not both".to_string(),
            param: "names".to_string(),
        });
    }

    if params.names.is_empty() && !params.all {
        return Err(ToolError::InvalidParam {
            message: "gateway.import requires either `all: true` or a non-empty `names` list"
                .to_string(),
            param: "names".to_string(),
        });
    }

    for client in &params.clients {
        if !KNOWN_CLIENTS.contains(&client.as_str()) {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "unknown client kind: '{}'. Valid: {}",
                    client,
                    KNOWN_CLIENTS.join(", ")
                ),
                param: "clients".to_string(),
            });
        }
    }

    let home = super::discovery::home_dir().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "cannot determine home directory".to_string(),
    })?;

    let mut discovered = tokio::task::spawn_blocking(move || super::discovery::discover_all(&home))
        .await
        .map_err(|e| ToolError::internal_message(format!("discovery task panicked: {e}")))?;
    if !params.clients.is_empty() {
        let filter: std::collections::HashSet<&str> =
            params.clients.iter().map(String::as_str).collect();
        discovered.retain(|s| filter.contains(s.source_client.as_str()));
    }

    // Reaching here: exactly one of `all=true` or a non-empty `names` list is set.
    // (both-provided is rejected above; neither-provided is rejected above)
    let to_import: Vec<_> = if params.all {
        discovered
    } else {
        let wanted: std::collections::HashSet<&str> =
            params.names.iter().map(String::as_str).collect();
        discovered
            .into_iter()
            .filter(|s| wanted.contains(s.name.as_str()))
            .collect()
    };

    let cfg = manager.current_config().await;
    let (mut result, specs_to_add) =
        super::manager::partition_discovered_for_import(&cfg, to_import);

    if !specs_to_add.is_empty() {
        let outcome = manager
            .batch_add_scoped(specs_to_add, Some("gateway.import"), None, enrichment_scope)
            .await?;

        result.imported.extend(outcome.views);

        for (name, err) in outcome.errors {
            if matches!(err, ToolError::Conflict { .. }) {
                result.skipped.push(ImportSkipView {
                    name,
                    reason: ImportSkipReason::Conflict,
                });
            } else {
                result.errors.push(ImportErrorView {
                    name,
                    message: err.to_string(),
                });
            }
        }
    }

    to_json(result)
}

async fn handle_import_tombstone_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
    enrichment_scope: GatewayEnrichmentScope,
) -> Result<Value, ToolError> {
    match action {
        "gateway.import_tombstones.list" => {
            let mut tombstones = manager.list_import_tombstones().await;
            if let Some(visible) = enrichment_scope.route_visible_upstreams.as_ref() {
                tombstones.retain(|tombstone| visible.contains(&tombstone.name));
            }
            to_json(tombstones)
        }
        "gateway.import_tombstones.clear" => {
            let params: GatewayImportTombstoneParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.name)?;
            to_json(manager.clear_import_tombstone(params.into()).await?)
        }
        "gateway.import_tombstones.restore" => {
            let params: GatewayImportTombstoneParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.name)?;
            let origin = params.origin.clone();
            let owner = params.owner.clone();
            to_json(
                manager
                    .restore_import_tombstone(
                        params.into(),
                        origin.as_deref(),
                        owner.map(Into::into),
                    )
                    .await?,
            )
        }
        unknown => unknown_action(unknown),
    }
}

impl From<GatewayImportTombstoneParams> for ImportTombstoneSelector {
    fn from(value: GatewayImportTombstoneParams) -> Self {
        Self {
            name: value.name,
            source_client: value.source_client,
            source_path: value.source_path,
            server_name: value.server_name,
            transport_fingerprint: value.transport_fingerprint,
        }
    }
}

fn redact_url_preview(raw: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(raw) else {
        return "<redacted>".to_string();
    };
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_query(None);
    parsed.set_fragment(None);
    parsed.to_string()
}

async fn handle_tool_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    match action {
        "gateway.code_mode.get" => to_json(manager.code_mode_config().await),
        "gateway.code_mode.set" => {
            let params: CodeModeSetParams = parse_params(params_value)?;
            let mut next = manager.code_mode_config().await;
            if let Some(enabled) = params.enabled {
                next.enabled = enabled;
            }
            if let Some(trusted_read_only_tools) = params.trusted_read_only_tools {
                // The read-only gate reads the live MCP `readOnlyHint` on each
                // tool descriptor; this allowlist has no production caller.
                // Persisting it would let an operator believe a security control
                // is active when it is inert, so drop the value rather than
                // storing it. The param is still accepted so a get/set round trip
                // from an existing client does not start failing.
                if !trusted_read_only_tools.is_empty() {
                    tracing::warn!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "gateway.code_mode.set",
                        entries = trusted_read_only_tools.len(),
                        "`trusted_read_only_tools` is retired and was discarded; \
                         Code Mode admits read-only tools from the upstream's own \
                         `readOnlyHint` annotation. Remove the setting."
                    );
                }
                next.trusted_read_only_tools.clear();
            }
            if let Some(mcp_ui_enabled) = params.mcp_ui_enabled {
                next.mcp_ui_enabled = mcp_ui_enabled;
            }
            if let Some(trace_params) = params.trace_params {
                next.trace_params = trace_params;
            }
            if let Some(result_shape_policy) = params.result_shape_policy {
                next.result_shape_policy = result_shape_policy;
            }
            if let Some(timeout_ms) = params.timeout_ms {
                next.timeout_ms = timeout_ms;
            }
            if let Some(max_response_bytes) = params.max_response_bytes {
                next.max_response_bytes = max_response_bytes;
            }
            if let Some(max_response_tokens) = params.max_response_tokens {
                next.max_response_tokens = max_response_tokens;
            }
            if let Some(token_estimate_divisor) = params.token_estimate_divisor {
                next.token_estimate_divisor = token_estimate_divisor;
            }
            if let Some(max_log_entries) = params.max_log_entries {
                next.max_log_entries = max_log_entries;
            }
            if let Some(max_log_bytes) = params.max_log_bytes {
                next.max_log_bytes = max_log_bytes;
            }
            to_json(manager.set_code_mode_config(next, None, None).await?)
        }
        unknown => unknown_action(unknown),
    }
}

async fn handle_loadout_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    match action {
        "gateway.loadout.list" => to_json(manager.loadout_list().await),
        "gateway.loadout.list_state" => to_json(manager.loadout_list_state().await?),
        "gateway.loadout.get" => {
            let params: LoadoutNameParams = parse_params(params_value)?;
            to_json(manager.loadout_get(&params.name).await?)
        }
        "gateway.loadout.add" => {
            let params: LoadoutSpecParams = parse_params(params_value)?;
            to_json(manager.loadout_add(params.loadout).await?)
        }
        "gateway.loadout.update" => {
            let params: LoadoutUpdateParams = parse_params(params_value)?;
            to_json(manager.loadout_update(&params.name, params.loadout).await?)
        }
        "gateway.loadout.patch" => {
            let params: LoadoutPatchParams = parse_params(params_value)?;
            to_json(manager.loadout_patch(&params.name, params.patch).await?)
        }
        "gateway.loadout.stage_update" => {
            let params: LoadoutUpdateParams = parse_params(params_value)?;
            manager
                .loadout_stage_update(&params.name, params.loadout)
                .await
        }
        "gateway.loadout.stage_patch" => {
            let params: LoadoutPatchParams = parse_params(params_value)?;
            manager
                .loadout_stage_patch(&params.name, params.patch)
                .await
        }
        "gateway.loadout.stage_remove" => {
            let params: LoadoutNameParams = parse_params(params_value)?;
            manager.loadout_stage_remove(&params.name).await
        }
        "gateway.loadout.remove" => {
            let params: LoadoutNameParams = parse_params(params_value)?;
            to_json(manager.loadout_remove(&params.name).await?)
        }
        unknown => unknown_action(unknown),
    }
}

async fn handle_protected_route_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    match action {
        "gateway.protected_route.list" => to_json(manager.protected_route_list().await),
        "gateway.protected_route.list_state" => {
            to_json(manager.protected_route_list_state().await?)
        }
        "gateway.protected_route.get" => {
            let params: ProtectedRouteNameParams = parse_params(params_value)?;
            to_json(manager.protected_route_get(&params.name).await?)
        }
        "gateway.protected_route.add" => {
            let params: ProtectedRouteSpecParams = parse_params(params_value)?;
            to_json(manager.protected_route_add(params.route).await?)
        }
        "gateway.protected_route.update" => {
            let params: ProtectedRouteUpdateParams = parse_params(params_value)?;
            to_json(
                manager
                    .protected_route_update(&params.name, params.route, params.preserve_project_id)
                    .await?,
            )
        }
        "gateway.protected_route.remove" => {
            let params: ProtectedRouteNameParams = parse_params(params_value)?;
            to_json(manager.protected_route_remove(&params.name).await?)
        }
        "gateway.protected_route.stage_add" => {
            let params: ProtectedRouteSpecParams = parse_params(params_value)?;
            manager.protected_route_stage_add(params.route).await
        }
        "gateway.protected_route.stage_update" => {
            let params: ProtectedRouteUpdateParams = parse_params(params_value)?;
            manager
                .protected_route_stage_update(
                    &params.name,
                    params.route,
                    params.preserve_project_id,
                )
                .await
        }
        "gateway.protected_route.stage_remove" => {
            let params: ProtectedRouteNameParams = parse_params(params_value)?;
            manager.protected_route_stage_remove(&params.name).await
        }
        "gateway.protected_route.test" => {
            let params: ProtectedRouteSpecParams = parse_params(params_value)?;
            to_json(manager.protected_route_test(params.route).await?)
        }
        unknown => unknown_action(unknown),
    }
}

async fn handle_virtual_server_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    match action {
        "gateway.virtual_server.enable" => {
            let params: VirtualServerNameParams = parse_params(params_value)?;
            to_json(manager.enable_virtual_server(&params.id).await?)
        }
        "gateway.virtual_server.disable" => {
            let params: VirtualServerNameParams = parse_params(params_value)?;
            to_json(manager.disable_virtual_server(&params.id).await?)
        }
        "gateway.virtual_server.remove" => {
            let params: VirtualServerNameParams = parse_params(params_value)?;
            to_json(manager.remove_virtual_server(&params.id).await?)
        }
        "gateway.virtual_server.quarantine.list" => {
            to_json(manager.list_quarantined_virtual_servers().await?)
        }
        "gateway.virtual_server.quarantine.restore" => {
            let params: VirtualServerNameParams = parse_params(params_value)?;
            to_json(
                manager
                    .restore_quarantined_virtual_server(&params.id)
                    .await?,
            )
        }
        "gateway.virtual_server.set_surface" => {
            let params: VirtualServerSurfaceParams = parse_params(params_value)?;
            to_json(
                manager
                    .set_virtual_server_surface(&params.id, &params.surface, params.enabled)
                    .await?,
            )
        }
        "gateway.virtual_server.get_mcp_policy" => {
            let params: VirtualServerNameParams = parse_params(params_value)?;
            to_json(manager.get_virtual_server_mcp_policy(&params.id).await?)
        }
        "gateway.virtual_server.set_mcp_policy" => {
            let params: VirtualServerMcpPolicyParams = parse_params(params_value)?;
            let service = manager.service_for_virtual_server_id(&params.id).await?;
            let valid_actions = compiled_service_actions(manager, &service)?;
            for action in &params.allowed_actions {
                if !valid_actions
                    .iter()
                    .any(|candidate| candidate.name == action.as_str())
                {
                    return Err(ToolError::InvalidParam {
                        message: format!("action `{action}` is not valid for service `{service}`"),
                        param: "allowed_actions".to_string(),
                    });
                }
            }
            to_json(
                manager
                    .set_virtual_server_mcp_policy(&params.id, &params.allowed_actions)
                    .await?,
            )
        }
        unknown => unknown_action(unknown),
    }
}

async fn handle_service_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
) -> Result<Value, ToolError> {
    match action {
        "gateway.service_config.get" => {
            let params: ServiceConfigGetParams = parse_params(params_value)?;
            to_json(manager.get_service_config(&params.service).await?)
        }
        "gateway.service_config.set" => {
            let params: ServiceConfigSetParams = parse_params(params_value)?;
            to_json(
                manager
                    .set_service_config(&params.service, &params.values)
                    .await?,
            )
        }
        "gateway.service_actions" => {
            let params: ServiceConfigGetParams = parse_params(params_value)?;
            to_json(compiled_service_actions(manager, &params.service)?)
        }
        unknown => unknown_action(unknown),
    }
}

async fn handle_gateway_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
    enrichment_scope: GatewayEnrichmentScope,
) -> Result<Value, ToolError> {
    match action {
        "gateway.list" => to_json(manager.list_scoped(&enrichment_scope).await?),
        "gateway.server.get" => {
            let params: VirtualServerNameParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.id)?;
            to_json(manager.get_server(&params.id).await?)
        }
        "gateway.supported_services" => {
            let registry = manager.builtin_service_registry();
            to_json(super::service_catalog::supported_services_from_registry(
                registry.as_ref(),
            ))
        }
        "gateway.get" => {
            let params: GatewayNameParams = parse_params(params_value)?;
            to_json(manager.get_scoped(&params.name, &enrichment_scope).await?)
        }
        "gateway.test" => {
            // SECURITY NOTE: When called with a `spec` (unsaved config) for a
            // stdio-backed upstream, this action may **execute a local command**.
            // The `command` field of the spec is passed directly to the child
            // process launcher; there is no sandbox.  Only callers with gateway
            // admin privileges should be able to reach this action, and operators
            // must treat `spec`-mode as equivalent to running the named binary.
            //
            // When called with a `name` (saved config), the command comes from the
            // persisted config file, which is under operator control.  The same
            // execution risk applies — the test action spawns the stdio process
            // and probes it exactly as the gateway would during live operation.
            let params: GatewayTestParams = parse_params(params_value)?;
            match (params.name.as_deref(), params.spec.as_ref()) {
                (Some(name), None) => {
                    enrichment_scope.ensure_visible(name)?;
                    to_json(manager.test(Err(name)).await?)
                }
                (None, Some(spec)) => {
                    if enrichment_scope.route_visible_upstreams.is_some() {
                        return Err(ToolError::Sdk {
                            sdk_kind: "forbidden".to_string(),
                            message: "testing an unsaved gateway spec is unavailable on a protected subset route".to_string(),
                        });
                    }
                    to_json(manager.test(Ok(spec)).await?)
                }
                (Some(_), Some(_)) => Err(ToolError::InvalidParam {
                    message: "gateway.test accepts either `name` or `spec`, not both".to_string(),
                    param: "name".to_string(),
                }),
                (None, None) => Err(ToolError::MissingParam {
                    message: "gateway.test requires either `name` or `spec`".to_string(),
                    param: "name".to_string(),
                }),
            }
        }
        "gateway.add" => {
            let params: GatewayAddParams = parse_params(params_value)?;
            to_json(
                manager
                    .add_scoped(
                        params.spec,
                        params.bearer_token_value,
                        params.origin.as_deref(),
                        params.owner.map(Into::into),
                        enrichment_scope,
                    )
                    .await?,
            )
        }
        "gateway.update" => {
            let params: GatewayUpdateParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.name)?;
            to_json(
                manager
                    .update_checked(
                        &params.name,
                        params.patch,
                        params.bearer_token_value,
                        params.origin.as_deref(),
                        params.owner.map(Into::into),
                        params.expected_revision.as_deref(),
                    )
                    .await?,
            )
        }
        "gateway.remove" => {
            let params: GatewayNameParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.name)?;
            to_json(
                manager
                    .remove_checked(
                        &params.name,
                        params.origin.as_deref(),
                        params.owner.map(Into::into),
                        params.expected_revision.as_deref(),
                    )
                    .await?,
            )
        }
        "gateway.reload" => {
            let params: GatewayReloadParams = parse_params(params_value)?;
            if let (Some(name), Some(expected)) =
                (params.name.as_deref(), params.expected_revision.as_deref())
            {
                let current = manager.get(name).await?;
                if current.revision != expected {
                    return Err(ToolError::Sdk {
                        sdk_kind: "stale_state".into(),
                        message: format!(
                            "gateway revision conflict; current revision is {}",
                            current.revision
                        ),
                    });
                }
            }
            // Bounded below the API router's transport backstop so a slow full
            // rebuild reports "still reconciling" instead of the middleware
            // cancelling the reload mid-flight and discarding the config.
            to_json(
                manager
                    .reload_with_origin_detached(
                        params.origin.as_deref(),
                        params.owner.map(Into::into),
                        std::time::Duration::from_secs(20),
                    )
                    .await?,
            )
        }
        "gateway.status" => {
            let params: GatewayStatusParams = parse_params(params_value)?;
            manager
                .refresh_gateway_status_catalog(&enrichment_scope, params.name.as_deref())
                .await;
            to_json(
                manager
                    .status_scoped(params.name.as_deref(), &enrichment_scope)
                    .await?,
            )
        }
        "gateway.client_config.get" => {
            let params: GatewayClientConfigParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.name)?;
            to_json(manager.client_config(&params.name).await?)
        }
        "gateway.public_urls.get" => {
            let urls = manager.public_urls();
            let effective_mcp_gateway = urls.effective_mcp_gateway().map(str::to_owned);
            to_json(serde_json::json!({
                "app": urls.app,
                "mcp_gateway": urls.mcp_gateway,
                "effective_mcp_gateway": effective_mcp_gateway,
            }))
        }
        "gateway.discovered_tools" => {
            let params: GatewayNameParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.name)?;
            to_json(manager.discovered_tools(&params.name).await?)
        }
        "gateway.discovered_resources" => {
            let params: GatewayNameParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.name)?;
            to_json(manager.discovered_resources(&params.name).await?)
        }
        "gateway.discovered_prompts" => {
            let params: GatewayNameParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.name)?;
            to_json(manager.discovered_prompts(&params.name).await?)
        }
        unknown => unknown_action(unknown),
    }
}

async fn handle_oauth_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
    enrichment_scope: GatewayEnrichmentScope,
) -> Result<Value, ToolError> {
    match action {
        "gateway.oauth.resource_lease.create" => {
            let params: ResourceLeaseCreateParams = parse_params(params_value)?;
            let registry = require_resource_registry(manager)?;
            let lease = registry
                .create_resource_lease(
                    &params.resource,
                    params.scopes,
                    std::time::Duration::from_secs(params.ttl_secs),
                    &params.owner,
                )
                .map_err(resource_registry_error)?;
            to_json(lease)
        }
        "gateway.oauth.resource_lease.renew" => {
            let params: ResourceLeaseRenewParams = parse_params(params_value)?;
            let registry = require_resource_registry(manager)?;
            let lease = registry
                .renew_resource_lease(&params.id, std::time::Duration::from_secs(params.ttl_secs))
                .map_err(resource_registry_error)?;
            to_json(lease)
        }
        "gateway.oauth.resource_lease.release" => {
            let params: ResourceLeaseReleaseParams = parse_params(params_value)?;
            require_resource_registry(manager)?
                .release_resource_lease(&params.id)
                .map_err(resource_registry_error)?;
            to_json(super::types::ResourceLeaseReleaseView { released: true })
        }
        "gateway.oauth.probe" => {
            let url = require_str(&params_value, "url")?;
            to_json(crate::gateway::oauth::probe(manager, url).await?)
        }
        "gateway.oauth.start" => {
            reject_shared_oauth_subject_override(&params_value)?;
            let params: GatewayOauthNameParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.upstream)?;
            to_json(
                crate::gateway::oauth::begin_authorization(
                    manager,
                    &params.upstream,
                    SHARED_GATEWAY_OAUTH_SUBJECT,
                )
                .await?,
            )
        }
        "gateway.oauth.status" => {
            reject_shared_oauth_subject_override(&params_value)?;
            let params: GatewayOauthNameParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.upstream)?;
            to_json(
                crate::gateway::oauth::status(
                    manager,
                    &params.upstream,
                    SHARED_GATEWAY_OAUTH_SUBJECT,
                )
                .await?,
            )
        }
        "gateway.oauth.clear" => {
            reject_shared_oauth_subject_override(&params_value)?;
            let params: GatewayOauthNameParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.upstream)?;
            crate::gateway::oauth::clear(manager, &params.upstream, SHARED_GATEWAY_OAUTH_SUBJECT)
                .await?;
            to_json(serde_json::json!({ "ok": true }))
        }
        "gateway.oauth.google_revoke" => {
            let confirmed = params_value
                .get("confirm")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !confirmed {
                return Err(ToolError::Sdk {
                    sdk_kind: "confirmation_required".to_string(),
                    message: "set confirm=true to revoke the shared Google provider credential"
                        .to_string(),
                });
            }
            let params: GatewayOauthNameParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.upstream)?;
            to_json(crate::gateway::oauth::revoke_google(manager, &params.upstream).await?)
        }
        // Q-H3: poll loop moved from cli/gateway.rs into shared dispatch so all
        // surfaces (CLI, API, MCP) share the same orchestration logic.
        "gateway.oauth.wait" => {
            reject_shared_oauth_subject_override(&params_value)?;
            // Extract timeout_secs before parse_params consumes params_value.
            let timeout_secs: u64 = params_value
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(120);
            let params: GatewayOauthNameParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.upstream)?;
            let timeout = std::time::Duration::from_secs(timeout_secs);
            let authenticated = manager
                .await_upstream_authorization(
                    &params.upstream,
                    SHARED_GATEWAY_OAUTH_SUBJECT,
                    timeout,
                )
                .await?;
            to_json(serde_json::json!({
                "authenticated": authenticated,
                "timed_out": !authenticated,
            }))
        }
        unknown => unknown_action(unknown),
    }
}

fn require_resource_registry(
    manager: &GatewayManager,
) -> Result<labby_auth::resource_registry::ResourceRegistry, ToolError> {
    manager.resource_registry().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "auth_failed".to_string(),
        message: "OAuth resource leases are unavailable because daemon OAuth is not configured"
            .to_string(),
    })
}

fn resource_registry_error(
    error: labby_auth::resource_registry::ResourceRegistryError,
) -> ToolError {
    use labby_auth::resource_registry::ResourceRegistryError;
    match error {
        ResourceRegistryError::LeaseNotFound => ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: error.to_string(),
        },
        ResourceRegistryError::InvalidResource
        | ResourceRegistryError::InvalidScopes
        | ResourceRegistryError::InvalidTtl
        | ResourceRegistryError::InvalidOwner => ToolError::InvalidParam {
            message: error.to_string(),
            param: "params".to_string(),
        },
        ResourceRegistryError::RandomnessUnavailable | ResourceRegistryError::InvalidClock => {
            ToolError::internal_message(error.to_string())
        }
    }
}

async fn handle_mcp_actions(
    manager: &GatewayManager,
    action: &str,
    params_value: Value,
    enrichment_scope: GatewayEnrichmentScope,
) -> Result<Value, ToolError> {
    match action {
        "gateway.mcp.enable" => {
            let params: GatewayNameParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.name)?;
            to_json(
                manager
                    .update(
                        &params.name,
                        GatewayUpdatePatch {
                            enabled: Some(true),
                            ..GatewayUpdatePatch::default()
                        },
                        None,
                        params.origin.as_deref(),
                        params.owner.clone().map(Into::into),
                    )
                    .await?,
            )
        }
        "gateway.mcp.list" => {
            let params: GatewayStatusParams = parse_params(params_value)?;
            to_json(
                manager
                    .mcp_runtime_list(params.name.as_deref(), &enrichment_scope)
                    .await?,
            )
        }
        "gateway.clients.list" => to_json(manager.clients().await?),
        "gateway.mcp.disable" => {
            let params: GatewayMcpToggleParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.name)?;
            let gateway = manager
                .update(
                    &params.name,
                    GatewayUpdatePatch {
                        enabled: Some(false),
                        ..GatewayUpdatePatch::default()
                    },
                    None,
                    params.origin.as_deref(),
                    params.owner.clone().map(Into::into),
                )
                .await?;
            let cleanup = if params.cleanup {
                Some(
                    manager
                        .cleanup_upstream_processes(&params.name, params.aggressive, false)
                        .await?,
                )
            } else {
                None
            };
            to_json(serde_json::json!({
                "gateway": gateway,
                "cleanup": cleanup,
            }))
        }
        "gateway.mcp.restart" => {
            let params: GatewayMcpRestartParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.name)?;
            let upstream =
                manager
                    .upstream_config(&params.name)
                    .await
                    .ok_or_else(|| ToolError::Sdk {
                        sdk_kind: "not_found".to_string(),
                        message: format!("upstream MCP server '{}' was not found", params.name),
                    })?;
            if !upstream.enabled {
                return Err(ToolError::InvalidParam {
                    message: format!(
                        "upstream MCP server '{}' is disabled; enable it before restarting its connection",
                        params.name
                    ),
                    param: "name".to_string(),
                });
            }

            manager
                .update(
                    &params.name,
                    GatewayUpdatePatch {
                        enabled: Some(false),
                        ..GatewayUpdatePatch::default()
                    },
                    None,
                    params.origin.as_deref(),
                    params.owner.clone().map(Into::into),
                )
                .await?;

            let cleanup = manager
                .cleanup_upstream_processes(&params.name, params.aggressive, false)
                .await;
            let gateway = manager
                .update(
                    &params.name,
                    GatewayUpdatePatch {
                        enabled: Some(true),
                        ..GatewayUpdatePatch::default()
                    },
                    None,
                    params.origin.as_deref(),
                    params.owner.map(Into::into),
                )
                .await;

            match (cleanup, gateway) {
                (Ok(cleanup), Ok(gateway)) => to_json(serde_json::json!({
                    "gateway": gateway,
                    "cleanup": cleanup,
                })),
                (Err(error), Ok(_)) | (_, Err(error)) => Err(error),
            }
        }
        "gateway.mcp.cleanup" => {
            let params: GatewayMcpCleanupParams = parse_params(params_value)?;
            enrichment_scope.ensure_visible(&params.name)?;
            to_json(
                manager
                    .cleanup_upstream_processes(&params.name, params.aggressive, params.dry_run)
                    .await?,
            )
        }
        unknown => unknown_action(unknown),
    }
}

fn unknown_action(unknown: &str) -> Result<Value, ToolError> {
    Err(ToolError::UnknownAction {
        message: format!("unknown action '{unknown}'"),
        valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
        hint: None,
    })
}

fn compiled_service_actions(
    manager: &GatewayManager,
    service: &str,
) -> Result<Vec<ServiceActionView>, ToolError> {
    let registry = manager.builtin_service_registry();
    let actions = registry
        .service_actions(service)
        .ok_or_else(|| ToolError::InvalidParam {
            message: format!("unknown service `{service}`"),
            param: "service".to_string(),
        })?;

    Ok(actions
        .iter()
        .map(|action| ServiceActionView {
            name: action.name.to_string(),
            description: action.description.to_string(),
            destructive: action.destructive,
        })
        .collect())
}

/// Public entry point for gateway dispatch.
///
/// Built-in actions (`help`, `schema`) are handled **before** manager
/// resolution so they succeed even when no gateway manager is installed.
/// This matches the shared dispatch contract used by every other service.
pub async fn dispatch(action: &str, params_value: Value) -> Result<Value, ToolError> {
    // Handle catalog-discovery built-ins first — they must not fail when no
    // gateway manager is installed (e.g. during initial setup or test runs
    // that do not wire a manager).  Fixing the dispatch contract here is the
    // minimum required change (see bead lab-l3cm).
    match action {
        "help" => return Ok(help_payload("gateway", ACTIONS)),
        "schema" => {
            let action_name = require_str(&params_value, "action")?;
            return action_schema(ACTIONS, action_name);
        }
        _ => {}
    }
    let manager = require_gateway_manager()?;
    dispatch_with_manager(&manager, action, params_value).await
}

#[cfg(test)]
#[allow(clippy::panic)]
#[path = "dispatch_tests.rs"]
mod tests;

/// Operator view of aggregated upstream skills.
///
/// Deliberately richer than the agent-facing listing: an operator needs to see
/// *why* a catalog looks the way it does — which upstreams opted in, how stale
/// each snapshot is, and what was dropped. Exclusion causes are operator-only;
/// an agent receives a count, never the reasons, because the reasons describe
/// the shape of an operator's configuration.
#[cfg(feature = "skills")]
async fn handle_skills_list(
    manager: &GatewayManager,
    params_value: Value,
    enrichment_scope: GatewayEnrichmentScope,
) -> Result<Value, ToolError> {
    #[derive(serde::Deserialize, Default)]
    #[serde(deny_unknown_fields)]
    struct Params {
        #[serde(default)]
        upstream: Option<String>,
    }

    let params: Params = if params_value.is_null() {
        Params::default()
    } else {
        parse_params(params_value)?
    };

    let started = std::time::Instant::now();
    // Enforce route scope BEFORE the existence probe below: that probe reports
    // `not_found` with discovery advice, which would otherwise confirm to a
    // subset-route caller whether an out-of-scope upstream name exists.
    if let Some(filter) = params.upstream.as_deref() {
        enrichment_scope.ensure_visible(filter)?;
    }
    let cfg = manager.current_config().await;
    if let Some(filter) = params.upstream.as_deref()
        && !cfg.upstream.iter().any(|config| config.name == filter)
    {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!(
                "gateway upstream `{filter}` was not found; run `labby gateway skills list` or `labby gateway list` to discover valid upstream names"
            ),
        });
    }
    tracing::debug!(
        surface = "dispatch",
        service = "gateway",
        action = "gateway.skills.list",
        upstream = params.upstream.as_deref().unwrap_or("*"),
        configured_upstreams = cfg.upstream.len(),
        "gateway skills operator listing start"
    );
    let Some(pool) = manager.current_pool().await else {
        return Err(ToolError::Sdk {
            sdk_kind: "runtime_unavailable".to_string(),
            message: "gateway runtime is unavailable; start or reconnect `labby serve`, then retry `gateway.skills.list`".to_string(),
        });
    };
    let configs = cfg
        .upstream
        .into_iter()
        .filter(|config| {
            enrichment_scope
                .route_visible_upstreams
                .as_ref()
                .is_none_or(|visible| visible.contains(&config.name))
        })
        .filter(|config| {
            params
                .upstream
                .as_ref()
                .is_none_or(|filter| config.name == *filter)
        })
        .collect::<Vec<_>>();
    let concurrency = crate::upstream::pool::upstream_discovery_concurrency(
        cfg.gateway.upstream_discovery_concurrency,
    );
    let mut inspections = stream::iter(configs.into_iter().map(|config| {
        let pool = pool.clone();
        async move {
            let fallback_support = pool
                .cached_upstream_summary(&config.name)
                .await
                .and_then(|summary| summary.supports_skills);
            let inspection = match pool.upstream_skills_operator(&config).await {
                Ok(operator) => {
                    let refresh_error = pool.upstream_skills_last_error(&config.name).await;
                    (
                        operator.supports_skills.or(fallback_support),
                        project_operator_skills(&operator),
                        operator.truncated,
                        operator.age_secs,
                        refresh_error,
                    )
                }
                Err(error) => (
                    fallback_support,
                    OperatorSkillsProjection::default(),
                    false,
                    0,
                    Some(error),
                ),
            };
            (config, inspection)
        }
    }))
    .buffered(concurrency);

    let mut rows = Vec::new();
    while let Some((config, (supports_skills, projection, truncated, age_secs, error))) =
        inspections.next().await
    {
        if let Some(error) = error.as_deref() {
            tracing::warn!(
                surface = "dispatch",
                service = "gateway",
                action = "gateway.skills.list",
                upstream = %config.name,
                trusted = config.proxy_skills,
                supports_skills = ?supports_skills,
                error = %error,
                "gateway skills upstream inspection degraded"
            );
        } else {
            tracing::debug!(
                surface = "dispatch",
                service = "gateway",
                action = "gateway.skills.list",
                upstream = %config.name,
                trusted = config.proxy_skills,
                supports_skills = ?supports_skills,
                discovered = projection.discovered_count,
                exposed = projection.exposed_count,
                rejected = projection.rejected.len(),
                truncated,
                cache_age_secs = age_secs,
                "gateway skills upstream inspection complete"
            );
        }

        rows.push(skills_operator_row(
            &config,
            supports_skills,
            projection,
            truncated,
            age_secs,
            error,
        ));
    }
    tracing::info!(
        surface = "dispatch",
        service = "gateway",
        action = "gateway.skills.list",
        upstream = params.upstream.as_deref().unwrap_or("*"),
        rows = rows.len(),
        elapsed_ms = started.elapsed().as_millis(),
        "gateway skills operator listing finish"
    );
    to_json(rows)
}

#[cfg(feature = "skills")]
#[derive(Default)]
struct OperatorSkillsProjection {
    skills: Vec<Value>,
    rejected: Vec<Value>,
    discovered_count: usize,
    exposed_count: usize,
}

#[cfg(feature = "skills")]
fn project_operator_skills(operator: &OperatorSkills) -> OperatorSkillsProjection {
    let skills = operator
        .skills
        .iter()
        .map(|item| {
            let skill = &item.descriptor;
            serde_json::json!({
                "name": skill.name,
                "identity": skill.id,
                "uri": skill.source_uri,
                "description": skill.description,
                "resource_count": skill.resource_count,
                // `exposed` is the legacy boolean; `exposure` is the structured
                // decision. Both are a documented contract
                // (docs/guides/SKILLS_AND_LOADOUTS.md) — operators need the
                // reason to tell "no pattern matched" from "not advertised".
                "exposed": item.exposure.exposed,
                "exposure": {
                    "status": item.exposure.status(),
                    "reason": item.exposure.reason.as_str(),
                    "matched_pattern": item.exposure.matched_pattern,
                },
            })
        })
        .collect();
    let rejected = operator
        .rejected
        .iter()
        .map(|item| {
            serde_json::json!({
                "uri": item.uri,
                "reason": item.reason,
                "detail": item.detail,
            })
        })
        .collect();
    OperatorSkillsProjection {
        skills,
        rejected,
        discovered_count: operator.discovered_count,
        exposed_count: operator
            .skills
            .iter()
            .filter(|item| item.exposure.exposed)
            .count(),
    }
}

#[cfg(feature = "skills")]
fn skills_operator_row(
    config: &labby_runtime::gateway_config::UpstreamConfig,
    supports_skills: Option<bool>,
    projection: OperatorSkillsProjection,
    truncated: bool,
    age_secs: u64,
    error: Option<String>,
) -> Value {
    let excluded_count = projection.rejected.len();
    serde_json::json!({
        "upstream": config.name,
        "enabled": config.enabled,
        "trusted": config.proxy_skills,
        "supports_skills": supports_skills,
        "exposure_patterns": config.expose_skills,
        "skills": projection.skills,
        "discovered_count": projection.discovered_count,
        "exposed_count": projection.exposed_count,
        "rejected": projection.rejected,
        "excluded_count": excluded_count,
        "truncated": truncated,
        "cache_age_secs": age_secs,
        "error": error,
    })
}

#[cfg(not(feature = "skills"))]
async fn handle_skills_list(
    _manager: &GatewayManager,
    params_value: Value,
    enrichment_scope: GatewayEnrichmentScope,
) -> Result<Value, ToolError> {
    #[derive(serde::Deserialize, Default)]
    #[serde(deny_unknown_fields)]
    struct Params {
        #[serde(default)]
        upstream: Option<String>,
    }

    let params: Params = if params_value.is_null() {
        Params::default()
    } else {
        parse_params(params_value)?
    };
    // Feature slicing must not bypass route isolation. A caller restricted to
    // a subset of upstreams must receive the same non-enumerating error before
    // learning whether this build happens to include the Skills runtime.
    if let Some(filter) = params.upstream.as_deref() {
        enrichment_scope.ensure_visible(filter)?;
    }

    // Not `unknown_action`: that kind's recovery advice is "rediscover", and
    // rediscovery re-advertises this same action, so an agent would loop. The
    // action is real and permanently unavailable in this build.
    Err(ToolError::Sdk {
        sdk_kind: "feature_not_compiled".to_string(),
        message: "this build of Labby was compiled without the `skills` feature; install a release build (which includes Skills) or rebuild with `--features skills`, then retry".to_string(),
    })
}
