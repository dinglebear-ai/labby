//! `list_tools` handler body + gateway meta-tool input-schema construction.
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.4`) as an inherent
//! `impl LabMcpServer` method. The `ServerHandler` trait impl in
//! `server.rs` keeps a one-line delegator.
//!
//! The Code Mode tool description has exactly one definition; this module
//! imports it for `list_tools`.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::MetaObject;
use rmcp::model::{ListToolsResult, PaginatedRequestParams};
use rmcp::service::RequestContext;
use serde_json::Value;

#[cfg(feature = "gateway")]
use crate::dispatch::upstream::pool::MAX_UPSTREAM_TOOLS;
#[cfg(feature = "gateway")]
use crate::mcp::call_tool_codemode::CodeModeUpstreamDescription;
#[cfg(feature = "gateway")]
use crate::mcp::catalog::{
    ADD_SERVER_TOOL_NAME, CODE_MODE_READ_TOOL_NAME, CODE_MODE_TOOL_NAME, CODE_MODE_UI_TOOL_NAME,
    GATEWAY_STATUS_TOOL_NAME, MCP_APP_TOOL_NAME,
};
use crate::mcp::catalog::{SERVER_LOGS_TOOL_NAME, ToolCatalogSnapshot};
#[cfg(feature = "gateway")]
use crate::mcp::context::oauth_upstream_subject_for_request;
#[cfg(feature = "gateway")]
use crate::mcp::context::{
    auth_context_from_extensions, code_mode_read_scope_allowed, tool_execute_scope_allowed,
};
#[cfg(feature = "gateway")]
use crate::mcp::handlers_resources::{
    add_server_app_resource_uri_for_tool, add_server_app_skybridge_uri_for_tool,
    code_mode_app_resource_uri_for_tool, code_mode_app_skybridge_uri_for_tool,
    gateway_status_app_resource_uri_for_tool, gateway_status_app_skybridge_uri_for_tool,
};
use crate::mcp::handlers_resources::{
    admin_app_resources_visible, server_logs_app_resource_uri_for_tool,
    server_logs_app_skybridge_uri_for_tool,
};
use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel};
use crate::mcp::pagination::{PageCollector, error_kind as pagination_error_kind};
use crate::mcp::server::LabMcpServer;

impl LabMcpServer {
    pub(crate) async fn list_tools_impl(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_tools",
            subject,
            "dispatch start"
        );
        let page_collector = match PageCollector::new(request) {
            Ok(collector) => collector,
            Err(error) => {
                let elapsed_ms = start.elapsed().as_millis();
                let kind = pagination_error_kind(&error);
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "list_tools",
                    subject,
                    elapsed_ms,
                    kind,
                    "tool list failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "list_tools",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind,
                    },
                )
                .await;
                return Err(error);
            }
        };
        let mut descriptors = Vec::new();
        let mut advertised_names = HashSet::new();
        let mut builtin_tool_count = 0usize;
        let mut upstream_tool_count = 0usize;
        let mut subject_scoped_tool_count = 0usize;
        let mut gateway_tool_count = 0usize;
        let upstream_ui_tool_count = 0usize;
        let mut suppressed_builtin_tool_count = 0usize;
        let mut pool_present = false;
        let mut catalog_upstream_count = 0usize;
        let mut upstream_tool_error_count = 0usize;
        let mut open_upstream_count = 0usize;
        // FU-2 (issue #210, lab-ecxfl): one PeerContract for the whole listing.
        // The three consumers below (visibility, Code Mode upstream
        // descriptions, upstream pool) are audience-independent, so hoisting
        // is behavior-neutral. The clone cost is only real on ProtectedSubset
        // routes — `Root` is a unit variant.
        let peer_contract = self.peer_contract();
        let visibility = peer_contract.code_mode_visibility().await;
        let manager_code_mode_enabled = visibility.exposes_synthetic_tools();
        let process_code_mode_enabled = crate::config::process_code_mode_enabled();
        let hide_raw_tools = visibility.hides_raw_tools();
        let visibility_mode = visibility.mode_label();
        #[cfg(feature = "gateway")]
        let auth = auth_context_from_extensions(&context.extensions);
        let server_logs_app_visible = {
            #[cfg(feature = "gateway")]
            {
                admin_app_resources_visible(auth)
            }
            #[cfg(not(feature = "gateway"))]
            {
                true
            }
        };
        #[cfg(feature = "gateway")]
        let add_server_app_visible =
            admin_app_resources_visible(auth) && self.add_server_app_available_on_mcp().await;
        #[cfg(feature = "gateway")]
        let gateway_status_app_visible =
            admin_app_resources_visible(auth) && self.gateway_status_app_available_on_mcp().await;
        let mut builtin_names = HashSet::new();
        for svc in self.registry.services() {
            // `service_visible_on_mcp` already checks `route_scope.allows_service`.
            if self.service_visible_on_mcp(svc.name).await {
                builtin_names.insert(svc.name.to_string());
                if hide_raw_tools && svc.name != SERVER_LOGS_TOOL_NAME {
                    suppressed_builtin_tool_count += 1;
                } else {
                    advertised_names.insert(svc.name.to_string());
                    descriptors.push(
                        self.registry
                            .permanent_tools()
                            .builtin_service_tool(svc, server_logs_app_visible),
                    );
                    builtin_tool_count += 1;
                }
            }
        }
        // Assemble and deduplicate the complete visible contract before pagination. Offset
        // cursors are only safe when every catalog rebuild produces the same global order.
        #[cfg(feature = "gateway")]
        if visibility.exposes_synthetic_tools()
            && (code_mode_read_scope_allowed(auth) || tool_execute_scope_allowed(auth))
        {
            // ── Gateway Code Mode tool. It takes `{ code, upstreams?, tools? }`
            // and exposes in-sandbox discovery through `codemode.search()` /
            // `codemode.describe()`.
            // See mcp/CLAUDE.md for the exception rationale and
            // dispatch/gateway/dispatch.rs guard.
            let code_mode_upstreams = peer_contract.code_mode_upstreams_for_description().await;
            if code_mode_read_scope_allowed(auth) {
                descriptors.push(
                    self.registry
                        .permanent_tools()
                        .code_mode_read_descriptor(&code_mode_upstreams),
                );
                advertised_names.insert(CODE_MODE_READ_TOOL_NAME.to_string());
                gateway_tool_count += 1;
            }

            if tool_execute_scope_allowed(auth) {
                let descriptor = self
                    .registry
                    .permanent_tools()
                    .code_mode_descriptor(&code_mode_upstreams);
                tracing::info!(
                    surface = "mcp",
                    service = labby_codemode::SERVICE,
                    action = "tool.describe",
                    description_bytes =
                        descriptor.description.as_deref().map(str::len).unwrap_or(0),
                    "registered primary Code Mode description"
                );
                descriptors.push(descriptor);
                advertised_names.insert(CODE_MODE_TOOL_NAME.to_string());
                gateway_tool_count += 1;

                if self.code_mode_app_state.is_enabled() {
                    let codemode_resource_uri =
                        code_mode_app_resource_uri_for_tool(CODE_MODE_UI_TOOL_NAME)
                            .unwrap_or_else(|| "<missing>".to_string());
                    let codemode_skybridge_uri =
                        code_mode_app_skybridge_uri_for_tool(CODE_MODE_UI_TOOL_NAME)
                            .unwrap_or_else(|| "<missing>".to_string());
                    tracing::info!(
                        surface = "mcp",
                        service = labby_codemode::SERVICE,
                        action = "mcp_app.advertise",
                        tool = CODE_MODE_UI_TOOL_NAME,
                        resource_uri = %codemode_resource_uri,
                        skybridge_uri = %codemode_skybridge_uri,
                        "advertised explicit Code Mode MCP app tool"
                    );
                    descriptors.push(
                        self.registry
                            .permanent_tools()
                            .code_mode_ui_tool(&code_mode_upstreams),
                    );
                    advertised_names.insert(CODE_MODE_UI_TOOL_NAME.to_string());
                    gateway_tool_count += 1;
                }

                descriptors.push(self.registry.permanent_tools().mcp_app_tool());
                advertised_names.insert(MCP_APP_TOOL_NAME.to_string());
                gateway_tool_count += 1;
            }
        }

        #[cfg(feature = "gateway")]
        if add_server_app_visible {
            descriptors.push(self.registry.permanent_tools().add_server_tool());
            advertised_names.insert(ADD_SERVER_TOOL_NAME.to_string());
            gateway_tool_count += 1;
        }

        #[cfg(feature = "gateway")]
        if gateway_status_app_visible {
            descriptors.push(self.registry.permanent_tools().gateway_status_tool());
            advertised_names.insert(GATEWAY_STATUS_TOOL_NAME.to_string());
            gateway_tool_count += 1;
        }

        // Merge upstream tools from the already-healthy catalog only. The
        // hidden-raw-tools path must never cold-connect upstreams: a single
        // slow or unhealthy server can otherwise stall the host's tool refresh
        // and make Labby's synthetic Code Mode tool appear to disappear. Code
        // Mode execution/search still performs cold discovery through the
        // gateway manager when the caller asks for upstream catalog data.
        #[cfg(feature = "gateway")]
        if let Some(pool) = peer_contract.current_upstream_pool().await {
            pool_present = true;
            let upstream_status = pool.upstream_status().await;
            catalog_upstream_count = upstream_status.len();
            open_upstream_count = upstream_status
                .iter()
                .filter(|(_, health)| health.is_open())
                .count();
            let upstream_tools = if hide_raw_tools || !self.route_scope.exposes_tools() {
                Vec::new()
            } else {
                pool.healthy_tools_allowed(self.route_scope.allowed_upstreams())
                    .await
            };
            for ut in upstream_tools {
                let tool_name = ut.tool.name.as_ref();
                if builtin_names.contains(tool_name)
                    || !advertised_names.insert(tool_name.to_string())
                {
                    tracing::debug!(
                        surface = "mcp",
                        service = "labby",
                        action = "tool.register",
                        tool = tool_name,
                        "skipping upstream tool that collides with an already advertised tool"
                    );
                    continue;
                }
                descriptors.push(ut.tool);
                upstream_tool_count += 1;
            }
            let oauth_subject =
                oauth_upstream_subject_for_request(auth, self.request_subject(&context));
            if !hide_raw_tools
                && self.route_scope.exposes_tools()
                && let Some(oauth_subject) = oauth_subject.as_ref()
            {
                let configs = self.route_scoped_oauth_upstream_configs().await;
                let subject_tool_limit = MAX_UPSTREAM_TOOLS.saturating_sub(upstream_tool_count);
                for (_, upstream_tools) in pool
                    .cached_subject_scoped_tools_bounded(
                        &configs,
                        oauth_subject.as_ref(),
                        subject_tool_limit,
                    )
                    .await
                {
                    for ut in upstream_tools {
                        let tool_name = ut.name.as_ref();
                        if builtin_names.contains(tool_name)
                            || !advertised_names.insert(tool_name.to_string())
                        {
                            continue;
                        }
                        descriptors.push(ut);
                        subject_scoped_tool_count += 1;
                    }
                }
            }
            for (upstream, _) in &upstream_status {
                if pool.upstream_tool_last_error(upstream).await.is_some() {
                    upstream_tool_error_count += 1;
                }
            }
        }

        if !self.route_scope.exposes_tools() {
            let keep_code_mode = self.route_scope.exposes_code_mode();
            descriptors.retain(|descriptor| {
                keep_code_mode
                    && matches!(
                        descriptor.name.as_ref(),
                        CODE_MODE_TOOL_NAME
                            | CODE_MODE_READ_TOOL_NAME
                            | CODE_MODE_UI_TOOL_NAME
                            | MCP_APP_TOOL_NAME
                    )
            });
        }
        descriptors.sort_by(|left, right| left.name.cmp(&right.name));
        let mut page_collector = page_collector;
        let complete_contract = ToolCatalogSnapshot::from_descriptors(&descriptors);
        let contract_revision = hex::encode(complete_contract.contract_hash);
        if let Err(error) = page_collector.bind_revision(&contract_revision) {
            let elapsed_ms = start.elapsed().as_millis();
            let kind = pagination_error_kind(&error);
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "list_tools",
                subject,
                elapsed_ms,
                kind,
                "tool list failed"
            );
            self.emit_dispatch_notification(
                &context,
                "lab",
                "list_tools",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind,
                },
            )
            .await;
            return Err(error);
        }
        for descriptor in descriptors.iter().cloned() {
            page_collector.accept(descriptor);
            if page_collector.finished() {
                break;
            }
        }
        let (tools, next_cursor) = match page_collector.finish() {
            Ok(page) => page,
            Err(error) => {
                let elapsed_ms = start.elapsed().as_millis();
                let kind = pagination_error_kind(&error);
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "list_tools",
                    subject,
                    elapsed_ms,
                    kind,
                    "tool list failed"
                );
                self.emit_dispatch_notification(
                    &context,
                    "lab",
                    "list_tools",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind,
                    },
                )
                .await;
                return Err(error);
            }
        };
        let page_tool_count = tools.len();
        let has_next_cursor = next_cursor.is_some();
        if !has_next_cursor && self.transport_label != "http" {
            let subject_key = self.request_subject(&context).map(str::to_owned);
            self.last_listed_tool_contract
                .write()
                .await
                .publish(subject_key, complete_contract);
        }

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_tools",
            subject,
            elapsed_ms,
            builtin_tool_count,
            gateway_tool_count,
            upstream_tool_count,
            upstream_ui_tool_count,
            subject_scoped_tool_count,
            suppressed_builtin_tool_count,
            pool_present,
            cold_discovery_skipped = hide_raw_tools,
            oauth_subject_catalog_source = "cached_only",
            upstream_catalog_source = if pool_present {
                "cached"
            } else {
                "not_initialized"
            },
            catalog_upstream_count,
            open_upstream_count,
            upstream_tool_error_count,
            manager_code_mode_enabled,
            process_code_mode_enabled,
            hide_raw_tools,
            visibility_mode,
            page_tool_count,
            has_next_cursor,
            "tool list ok"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "list_tools",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        let mut result = ListToolsResult::with_all_items(tools)
            .with_ttl_ms(0)
            .with_cache_scope(rmcp::model::CacheScope::Private);
        result.next_cursor = next_cursor;
        Ok(result)
    }
}

/// The note appended to the `codemode` descriptor.
///
/// Shared with `PermanentToolRegistry::code_mode_descriptor` so the advertised
/// description and the hashed peer contract can never disagree.
#[cfg(feature = "gateway")]
pub(crate) fn code_mode_app_text_note() -> String {
    format!(
        "This entry point has no static Labby UI, but nested upstream MCP Apps attach dynamically when a called tool returns `_meta.ui`. When advertised, use `{CODE_MODE_UI_TOOL_NAME}` for the visual trace inspector; `{MCP_APP_TOOL_NAME}` can inspect or restore that Labby-owned app surface."
    )
}

/// Description for the optional `codemode_ui` MCP App twin.
#[cfg(feature = "gateway")]
pub(crate) fn code_mode_ui_description(upstreams: &[CodeModeUpstreamDescription]) -> String {
    crate::mcp::call_tool_codemode::code_mode_description_with_suffix(
        upstreams,
        &format!(
            "This explicit UI entry point renders the Code Mode trace inspector. Use `{CODE_MODE_TOOL_NAME}` when nested upstream MCP Apps should become the active result UI."
        ),
    )
}

/// Description for the text-only `mcp_app` control tool.
#[cfg(feature = "gateway")]
pub(crate) const fn mcp_app_tool_description() -> &'static str {
    "Enable, disable, or inspect Labby's Code Mode inspector surface. This controls the explicit codemode_ui tool and discoverable Labby-owned app resources; codemode remains available and can return nested upstream MCP App metadata dynamically."
}

#[cfg(feature = "gateway")]
pub(crate) fn mcp_app_tool_schema() -> Arc<serde_json::Map<String, Value>> {
    static SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> = LazyLock::new(|| {
        let Value::Object(schema) = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["status", "enable", "disable"],
                    "default": "status",
                    "description": "Inspect or change whether the explicit Code Mode MCP App is advertised."
                },
                "target": {
                    "type": "string",
                    "enum": ["codemode"],
                    "default": "codemode",
                    "description": "Lab-owned MCP App target."
                }
            },
            "additionalProperties": false
        }) else {
            unreachable!("MCP App management schema is an object")
        };
        Arc::new(schema)
    });
    Arc::clone(&SCHEMA)
}

#[cfg(feature = "gateway")]
/// Build MCP Apps metadata for the explicit Code Mode UI tool.
pub(crate) fn code_mode_tool_meta(tool_name: &str) -> MetaObject {
    let resource_uri = code_mode_app_resource_uri_for_tool(tool_name)
        .expect("Code Mode tools must have an associated UI resource");
    // Anthropic / MCP Apps (SEP-1724) binding: hosts read `_meta.ui.resourceUri`.
    // OpenAI Apps SDK binding: ChatGPT / Codex hosts bind the widget via
    // `openai/outputTemplate` rather than `_meta.ui`. It points at the skybridge
    // variant of the same widget — identical HTML, served under the
    // `text/html+skybridge` MIME those hosts expect — so the Claude resource
    // stays untouched. The widget self-hydrates from `window.openai.toolOutput`.
    owned_app_tool_meta(
        resource_uri,
        code_mode_app_skybridge_uri_for_tool(tool_name),
    )
}

/// Build MCP Apps metadata for the Server Logs tool.
pub(crate) fn server_logs_tool_meta(tool_name: &str) -> MetaObject {
    let resource_uri = server_logs_app_resource_uri_for_tool(tool_name)
        .expect("server log tools must have an associated UI resource");
    owned_app_tool_meta(
        resource_uri,
        server_logs_app_skybridge_uri_for_tool(tool_name),
    )
}

#[cfg(feature = "gateway")]
/// Build MCP Apps metadata for the synthetic Add Server tool.
pub(crate) fn add_server_tool_meta(tool_name: &str) -> MetaObject {
    let resource_uri = add_server_app_resource_uri_for_tool(tool_name)
        .expect("Add Server tool must have an associated UI resource");
    owned_app_tool_meta(
        resource_uri,
        add_server_app_skybridge_uri_for_tool(tool_name),
    )
}

#[cfg(feature = "gateway")]
/// Build MCP Apps metadata for the synthetic Gateway Status tool.
pub(crate) fn gateway_status_tool_meta(tool_name: &str) -> MetaObject {
    let resource_uri = gateway_status_app_resource_uri_for_tool(tool_name)
        .expect("Gateway Status tool must have an associated UI resource");
    owned_app_tool_meta(
        resource_uri,
        gateway_status_app_skybridge_uri_for_tool(tool_name),
    )
}

/// Bind one tool to its MCP Apps and optional OpenAI skybridge resources.
fn owned_app_tool_meta(resource_uri: String, skybridge_uri: Option<String>) -> MetaObject {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "ui".to_string(),
        serde_json::json!({ "resourceUri": resource_uri }),
    );
    if let Some(skybridge_uri) = skybridge_uri {
        meta.insert(
            "openai/outputTemplate".to_string(),
            serde_json::json!(skybridge_uri),
        );
    }
    MetaObject(meta)
}

#[cfg(feature = "gateway")]
/// Describe the synthetic Add Server callback contract for agent clients.
pub(crate) fn add_server_tool_schema() -> Arc<serde_json::Map<String, Value>> {
    static SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> = LazyLock::new(|| {
        let Value::Object(schema) = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open", "test", "create"],
                    "default": "open",
                    "description": "Open the form, test a proposed server, or create it. Most callers should omit this to open the app."
                },
                "params": {
                    "type": "object",
                    "description": "For test/create, pass a proposed upstream server configuration.",
                    "required": ["spec"],
                    "properties": {
                        "spec": {
                            "type": "object",
                            "required": ["name"],
                            "properties": {
                                "name": {
                                    "type": "string",
                                    "pattern": "^[A-Za-z0-9][A-Za-z0-9._-]*$",
                                    "description": "Unique gateway server name."
                                },
                                "url": {
                                    "type": "string",
                                    "format": "uri",
                                    "description": "HTTP(S) MCP endpoint for a remote server."
                                },
                                "command": {
                                    "type": "string",
                                    "minLength": 1,
                                    "description": "Executable for a local stdio MCP server."
                                },
                                "args": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "default": [],
                                    "description": "Arguments passed to the local stdio command."
                                },
                                "enabled": {
                                    "type": "boolean",
                                    "default": true
                                },
                                "proxy_resources": {
                                    "type": "boolean",
                                    "default": true,
                                    "description": "Expose discovered upstream resources downstream."
                                },
                                "proxy_prompts": {
                                    "type": "boolean",
                                    "default": true,
                                    "description": "Expose discovered upstream prompts downstream."
                                }
                            },
                            "oneOf": [
                                {
                                    "required": ["url"],
                                    "not": { "anyOf": [{ "required": ["command"] }, { "required": ["args"] }] }
                                },
                                {
                                    "required": ["command"],
                                    "not": { "required": ["url"] }
                                }
                            ],
                            "additionalProperties": true
                        }
                    },
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        }) else {
            unreachable!("Add Server schema is an object")
        };
        Arc::new(schema)
    });
    Arc::clone(&SCHEMA)
}

#[cfg(feature = "gateway")]
/// Describe the read-only Gateway Status callback contract.
pub(crate) fn gateway_status_tool_schema() -> Arc<serde_json::Map<String, Value>> {
    static SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> = LazyLock::new(|| {
        let Value::Object(schema) = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open", "refresh"],
                    "default": "open",
                    "description": "Open the status app or refresh its live upstream snapshot."
                },
                "params": {
                    "type": "object",
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        }) else {
            unreachable!("Gateway Status schema is an object")
        };
        Arc::new(schema)
    });
    Arc::clone(&SCHEMA)
}

#[cfg(feature = "gateway")]
pub(crate) fn code_mode_execute_schema() -> Arc<serde_json::Map<String, Value>> {
    static EXECUTE_SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> = LazyLock::new(
        || match serde_json::json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "minLength": 1,
                    "description": "JavaScript async arrow function to execute. Use await callTool(id, params) with JSON-serializable params."
                },
                "upstreams": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional upstream allowlist for this execution."
                },
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tool allowlist for this execution. Accepts raw tool names or <upstream>::<tool> ids."
                }
            },
            "required": ["code"]
        }) {
            Value::Object(map) => Arc::new(map),
            _ => unreachable!("execute schema must be an object"),
        },
    );
    Arc::clone(&EXECUTE_SCHEMA)
}

#[cfg(feature = "gateway")]
pub(crate) fn code_mode_trace_output_schema() -> Arc<serde_json::Map<String, Value>> {
    static TRACE_OUTPUT_SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> = LazyLock::new(
        || match serde_json::json!({
        "type": "object",
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "code_mode_execute_trace" },
                    "call_count": { "type": "integer", "minimum": 0 },
                    "input_tokens": { "type": "integer", "minimum": 0 },
                    "output_tokens": { "type": "integer", "minimum": 0 },
                    "calls": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "namespace": { "type": "string" },
                                "tool": { "type": "string" },
                                "ok": { "type": "boolean" },
                                "elapsed_ms": { "type": "integer", "minimum": 0 },
                                "start_ms": { "type": "integer", "minimum": 0 },
                                "params": {},
                                "error_kind": { "type": "string" },
                                "ui": {
                                    "type": "object",
                                    "properties": {
                                        "resourceUri": {
                                            "type": "string",
                                            "description": "Native MCP UI resource URI returned by the upstream tool for this call."
                                        }
                                    },
                                    "required": ["resourceUri"],
                                    "additionalProperties": true
                                }
                            },
                            "required": ["id", "namespace", "tool", "ok", "elapsed_ms"],
                            "additionalProperties": true
                        }
                    },
                    "result": {},
                    "result_shape": { "type": "object" },
                    "result_shaping": { "type": "object" },
                    "artifacts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" },
                                "absolute_path": { "type": "string" },
                                "content_type": {
                                    "type": "string",
                                    "maxLength": 256,
                                    "pattern": "^[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+$",
                                    "description": "Simple ASCII type/subtype media type for the artifact receipt."
                                },
                                "bytes": { "type": "integer", "minimum": 0 },
                                "sha256": {
                                    "type": "string",
                                    "pattern": "^[a-f0-9]{64}$"
                                }
                            },
                            "required": ["path", "absolute_path", "content_type", "bytes", "sha256"],
                            "additionalProperties": false
                        }
                    },
                    "logs_count": { "type": "integer", "minimum": 0 }
                },
                "required": ["kind", "call_count", "calls", "result_shape", "logs_count"],
                "additionalProperties": true
            }
        ]
        }) {
            Value::Object(map) => Arc::new(map),
            _ => unreachable!("trace output schema must be an object"),
        },
    );
    Arc::clone(&TRACE_OUTPUT_SCHEMA)
}

#[cfg(test)]
#[cfg(feature = "gateway")]
mod tests;
