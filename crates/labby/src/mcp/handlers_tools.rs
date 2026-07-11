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
#[cfg(feature = "gateway")]
use rmcp::model::Meta;
use rmcp::model::{ListToolsResult, PaginatedRequestParams, Tool};
use rmcp::service::RequestContext;
use serde_json::Value;

#[cfg(feature = "gateway")]
use crate::mcp::call_tool_codemode::{CodeModeUpstreamDescription, code_mode_description};
#[cfg(feature = "gateway")]
use crate::mcp::catalog::CODE_MODE_TOOL_NAME;
use crate::mcp::completion::action_schema;
#[cfg(feature = "gateway")]
use crate::mcp::context::auth_context_from_extensions;
#[cfg(feature = "gateway")]
use crate::mcp::context::oauth_upstream_subject_for_request;
#[cfg(feature = "gateway")]
use crate::mcp::handlers_resources::{
    code_mode_app_resource_uri_for_tool, code_mode_app_skybridge_uri_for_tool,
};
use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel};
use crate::mcp::pagination::{PageCollector, error_kind as pagination_error_kind};
use crate::mcp::server::LabMcpServer;

static ACTION_SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> =
    LazyLock::new(|| Arc::new(action_schema()));

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
        let schema = Arc::clone(&ACTION_SCHEMA);
        let mut tools = match PageCollector::new(request) {
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
        let mut advertised_names = HashSet::new();
        let mut builtin_tool_count = 0usize;
        let mut upstream_tool_count = 0usize;
        let mut subject_scoped_tool_count = 0usize;
        let mut gateway_tool_count = 0usize;
        let mut upstream_ui_tool_count = 0usize;
        let mut suppressed_builtin_tool_count = 0usize;
        let mut pool_present = false;
        let mut catalog_upstream_count = 0usize;
        let mut upstream_tool_error_count = 0usize;
        let mut open_upstream_count = 0usize;
        let visibility = self.code_mode_visibility().await;
        let manager_code_mode_enabled = visibility.exposes_synthetic_tools();
        let process_code_mode_enabled = crate::config::process_code_mode_enabled();
        let hide_raw_tools = visibility.hides_raw_tools();
        let visibility_mode = visibility.mode_label();
        #[cfg(feature = "gateway")]
        let auth = auth_context_from_extensions(&context.extensions);
        let mut builtin_names = Vec::new();
        for svc in self.registry.services() {
            if self.route_scope.allows_service(svc.name)
                && self.service_visible_on_mcp(svc.name).await
            {
                builtin_names.push(svc.name);
                if hide_raw_tools {
                    suppressed_builtin_tool_count += 1;
                } else {
                    advertised_names.insert(svc.name.to_string());
                    tools.accept(Tool::new(svc.name, svc.description, Arc::clone(&schema)));
                    builtin_tool_count += 1;
                    if tools.finished() {
                        break;
                    }
                }
            }
        }
        #[cfg(feature = "gateway")]
        if !tools.finished() && visibility.exposes_synthetic_tools() {
            // ── Gateway Code Mode tool. It takes `{ code, upstreams?, tools? }`
            // and exposes in-sandbox discovery through `codemode.search()` /
            // `codemode.describe()`.
            // See mcp/CLAUDE.md for the exception rationale and
            // dispatch/gateway/dispatch.rs guard.
            let trace_output_schema = code_mode_trace_output_schema();
            let execute_schema = code_mode_execute_schema();
            let code_mode_upstreams = self.code_mode_upstreams_for_description().await;
            let code_mode_description = code_mode_description(&code_mode_upstreams);
            tracing::info!(
                surface = "mcp",
                service = labby_codemode::SERVICE,
                action = "tool.describe",
                description_bytes = code_mode_description.len(),
                upstream_count = code_mode_upstreams.len(),
                "registered primary Code Mode description"
            );
            let codemode_resource_uri = code_mode_app_resource_uri_for_tool(CODE_MODE_TOOL_NAME)
                .unwrap_or_else(|| "<missing>".to_string());
            let codemode_skybridge_uri = code_mode_app_skybridge_uri_for_tool(CODE_MODE_TOOL_NAME)
                .unwrap_or_else(|| "<missing>".to_string());
            tracing::info!(
                surface = "mcp",
                service = labby_codemode::SERVICE,
                action = "mcp_app.advertise",
                resource_uri = %codemode_resource_uri,
                skybridge_uri = %codemode_skybridge_uri,
                "advertised primary Code Mode MCP app metadata"
            );
            tools.accept(
                Tool::new(
                    CODE_MODE_TOOL_NAME,
                    code_mode_description,
                    Arc::clone(&execute_schema),
                )
                .with_raw_output_schema(Arc::clone(&trace_output_schema))
                .with_meta(code_mode_tool_meta(CODE_MODE_TOOL_NAME)),
            );
            advertised_names.insert(CODE_MODE_TOOL_NAME.to_string());
            gateway_tool_count += 1;
        }

        // Merge upstream tools from the already-healthy catalog only. The
        // hidden-raw-tools path must never cold-connect upstreams: a single
        // slow or unhealthy server can otherwise stall the host's tool refresh
        // and make Labby's synthetic Code Mode tool appear to disappear. Code
        // Mode execution/search still performs cold discovery through the
        // gateway manager when the caller asks for upstream catalog data.
        #[cfg(feature = "gateway")]
        if !tools.finished()
            && let Some(pool) = self.current_upstream_pool().await
        {
            pool_present = true;
            let upstream_status = pool.upstream_status().await;
            catalog_upstream_count = upstream_status.len();
            open_upstream_count = upstream_status
                .iter()
                .filter(|(_, health)| health.is_open())
                .count();
            let upstream_tools = if hide_raw_tools {
                pool.healthy_ui_tools_allowed(self.route_scope.allowed_upstreams())
                    .await
            } else {
                pool.healthy_tools_allowed(self.route_scope.allowed_upstreams())
                    .await
            };
            for ut in upstream_tools {
                let tool_name = ut.tool.name.as_ref();
                if builtin_names.contains(&tool_name)
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
                tools.accept(ut.tool);
                if hide_raw_tools {
                    upstream_ui_tool_count += 1;
                } else {
                    upstream_tool_count += 1;
                }
                if tools.finished() {
                    break;
                }
            }
            let oauth_subject =
                oauth_upstream_subject_for_request(auth, self.request_subject(&context));
            if !tools.finished()
                && !hide_raw_tools
                && let Some(oauth_subject) = oauth_subject.as_ref()
            {
                let configs = self.route_scoped_oauth_upstream_configs().await;
                for (_, upstream_tools) in pool
                    .subject_scoped_tools(&configs, oauth_subject.as_ref())
                    .await
                {
                    for ut in upstream_tools {
                        let tool_name = ut.name.as_ref();
                        if builtin_names.contains(&tool_name)
                            || !advertised_names.insert(tool_name.to_string())
                        {
                            continue;
                        }
                        tools.accept(ut);
                        subject_scoped_tool_count += 1;
                        if tools.finished() {
                            break;
                        }
                    }
                    if tools.finished() {
                        break;
                    }
                }
            }
            for (upstream, _) in &upstream_status {
                if pool.upstream_tool_last_error(upstream).await.is_some() {
                    upstream_tool_error_count += 1;
                }
            }
        }

        let (tools, next_cursor) = match tools.finish() {
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

        let mut result = ListToolsResult::with_all_items(tools);
        result.next_cursor = next_cursor;
        Ok(result)
    }

    #[cfg(feature = "gateway")]
    async fn code_mode_upstreams_for_description(&self) -> Vec<CodeModeUpstreamDescription> {
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
}

#[cfg(feature = "gateway")]
fn code_mode_tool_meta(tool_name: &str) -> Meta {
    let resource_uri = code_mode_app_resource_uri_for_tool(tool_name)
        .expect("Code Mode tools must have an associated UI resource");
    let mut meta = serde_json::Map::new();
    // Anthropic / MCP Apps (SEP-1724) binding: hosts read `_meta.ui.resourceUri`.
    meta.insert(
        "ui".to_string(),
        serde_json::json!({
            "resourceUri": resource_uri,
        }),
    );
    // OpenAI Apps SDK binding: ChatGPT / Codex hosts bind the widget via
    // `openai/outputTemplate` rather than `_meta.ui`. It points at the skybridge
    // variant of the same widget — identical HTML, served under the
    // `text/html+skybridge` MIME those hosts expect — so the Claude resource
    // stays untouched. The widget self-hydrates from `window.openai.toolOutput`.
    if let Some(skybridge_uri) = code_mode_app_skybridge_uri_for_tool(tool_name) {
        meta.insert(
            "openai/outputTemplate".to_string(),
            serde_json::json!(skybridge_uri),
        );
    }
    Meta(meta)
}

#[cfg(feature = "gateway")]
fn code_mode_execute_schema() -> Arc<serde_json::Map<String, Value>> {
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
fn code_mode_trace_output_schema() -> Arc<serde_json::Map<String, Value>> {
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
                    "calls": { "type": "array", "items": { "type": "object" } },
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
