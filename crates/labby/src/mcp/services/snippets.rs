//! MCP adapter for snippets-specific request context.
//!
//! Normal snippets operations dispatch through `crate::dispatch::snippets`.
//! Promotion is the MCP-specific exception because it must resolve the live
//! gateway manager source store against the caller actor, admin state, route
//! scope, and route-scoped capability filter.

use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use serde_json::{Map, Value};

use crate::mcp::context::auth_context_from_extensions;
use crate::mcp::envelope::build_error;
use crate::mcp::error::DispatchError;
use crate::mcp::result_format::{estimate_tokens_args, format_dispatch_result};
use crate::mcp::server::LabMcpServer;

impl LabMcpServer {
    pub(crate) async fn call_snippets_promote_impl(
        &self,
        action: &str,
        params: Value,
        args: &Map<String, Value>,
        start: Instant,
        subject: &str,
        actor_key: Option<&str>,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let service = "snippets";
        let Some(manager) = &self.gateway_manager else {
            let envelope = build_error(
                service,
                action,
                "internal_error",
                "gateway manager not wired",
            );
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                envelope.to_string(),
            )]));
        };

        let auth = auth_context_from_extensions(&context.extensions);
        let capability_filter_fingerprint = self
            .route_scope
            .allowed_upstreams()
            .map(|allowed| {
                crate::dispatch::gateway::code_mode::ToolScope::scoped_namespaces(
                    allowed.iter().cloned().collect(),
                    Vec::new(),
                )
                .fingerprint()
            })
            .unwrap_or_else(|| {
                crate::dispatch::gateway::code_mode::ToolScope::default().fingerprint()
            });
        let promotion_context = crate::dispatch::snippets::dispatch::SnippetPromotionContext {
            actor_key: actor_key.map(ToOwned::to_owned),
            is_admin: auth.is_none_or(|auth| auth.scopes.iter().any(|scope| scope == "lab:admin")),
            route_scope: self.route_scope.label(),
            capability_filter_fingerprint,
        };

        let result = crate::dispatch::snippets::dispatch::dispatch_with_manager_and_context(
            manager,
            action,
            params,
            Some(promotion_context),
        )
        .await
        .map_err(|te| anyhow::Error::from(DispatchError::from(te)));
        let elapsed_ms = start.elapsed().as_millis();
        let input_tokens = estimate_tokens_args(args);
        let (result, outcome) = format_dispatch_result(
            result,
            service,
            action,
            elapsed_ms,
            subject,
            actor_key,
            input_tokens,
        );
        self.emit_dispatch_notification(context, service, action, elapsed_ms, outcome)
            .await;
        Ok(result)
    }
}
