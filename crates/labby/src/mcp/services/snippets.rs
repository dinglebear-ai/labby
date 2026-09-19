//! MCP adapter for snippets-specific request context.
//!
//! Saved snippet execution and promotion must preserve the MCP route authority.
//! This adapter threads the live gateway manager plus the route-derived Code Mode
//! capability scope into dispatch so snippets can only narrow caller authority.

use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use serde_json::{Map, Value};

use crate::mcp::context::auth_context_from_extensions;
use crate::mcp::envelope::build_error;
use crate::mcp::error::DispatchError;
use crate::mcp::result_format::{
    error_result_from_envelope, estimate_tokens_args, format_dispatch_result,
};
use crate::mcp::server::LabMcpServer;

fn execution_scope_for_route(
    route_scope: &crate::mcp::route_scope::McpRouteScope,
) -> crate::dispatch::gateway::code_mode::ToolScope {
    route_scope
        .allowed_upstreams()
        .map(|allowed| {
            crate::dispatch::gateway::code_mode::ToolScope::scoped_namespaces(
                allowed.iter().cloned().collect(),
                Vec::new(),
            )
        })
        .unwrap_or_default()
}

fn execution_caller_for_mcp(
    scopes: Option<&[String]>,
    sub: Option<String>,
    host_provider: Option<(&str, &str)>,
) -> crate::dispatch::gateway::code_mode::CodeModeCaller {
    let Some(scopes) = scopes else {
        return crate::dispatch::gateway::code_mode::CodeModeCaller::TrustedLocal;
    };
    let capabilities = super::super::call_tool_codemode::code_mode_capabilities_for_scopes(scopes);
    match host_provider {
        Some((provider_token, provider_request_id)) => {
            crate::dispatch::gateway::code_mode::CodeModeCaller::ScopedHostProvider {
                capabilities,
                sub,
                provider_token: provider_token.to_string(),
                provider_request_id: provider_request_id.to_string(),
            }
        }
        None => crate::dispatch::gateway::code_mode::CodeModeCaller::Scoped { capabilities, sub },
    }
}

impl LabMcpServer {
    pub(crate) async fn call_snippets_contextual_impl(
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
            return Ok(error_result_from_envelope(envelope));
        };

        let auth = auth_context_from_extensions(&context.extensions);
        let execution_scope = execution_scope_for_route(&self.route_scope);
        let capability_filter_fingerprint = execution_scope.fingerprint();
        let sub = self
            .route_oauth_subject(
                self.request_subject(context)
                    .map(std::borrow::Cow::Borrowed),
            )
            .map(std::borrow::Cow::into_owned);
        let host_provider = self
            .request_host_provider_token(context)
            .zip(self.request_host_provider_request_id(context));
        let execution_caller =
            execution_caller_for_mcp(auth.map(|auth| auth.scopes.as_slice()), sub, host_provider);
        let dispatch_context = crate::dispatch::snippets::dispatch::SnippetDispatchContext {
            actor_key: actor_key.map(ToOwned::to_owned),
            is_admin: auth.is_none_or(|auth| auth.scopes.iter().any(|scope| scope == "lab:admin")),
            route_scope: self.route_scope.label(),
            capability_filter_fingerprint,
            execution_scope,
            execution_caller,
            execution_surface: crate::dispatch::gateway::code_mode::CodeModeSurface::Mcp,
        };

        let result = crate::dispatch::snippets::dispatch::dispatch_with_manager_and_context(
            manager,
            action,
            params,
            Some(dispatch_context),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::route_scope::McpRouteScope;

    #[test]
    fn protected_route_becomes_a_namespace_scoped_snippet_authority() {
        let route = McpRouteScope::protected_subset("team-route", ["alpha"], ["snippets"], true);

        let scope = execution_scope_for_route(&route);

        assert!(scope.is_scoped());
        assert!(scope.allows("alpha", "tool1"));
        assert!(!scope.allows("beta", "tool2"));
    }

    #[test]
    fn root_route_keeps_trusted_local_snippet_authority_unscoped() {
        let scope = execution_scope_for_route(&McpRouteScope::Root);

        assert!(!scope.is_scoped());
        assert!(scope.allows("alpha", "tool1"));
        assert!(scope.allows("beta", "tool2"));
    }

    #[test]
    fn authenticated_mcp_snippet_caller_preserves_identity_instead_of_becoming_trusted_local() {
        let scopes = vec!["lab:admin".to_string()];
        let caller = execution_caller_for_mcp(Some(&scopes), Some("alice".to_string()), None);

        assert!(!matches!(
            caller,
            crate::dispatch::gateway::code_mode::CodeModeCaller::TrustedLocal
        ));
        assert_eq!(caller.subject(), Some("alice"));
        assert!(caller.is_admin());
    }

    #[test]
    fn authenticated_mcp_snippet_caller_preserves_host_provider_context() {
        let scopes = vec!["lab:admin".to_string()];
        let caller = execution_caller_for_mcp(
            Some(&scopes),
            Some("alice".to_string()),
            Some(("provider-secret", "request-7")),
        );

        assert_eq!(caller.subject(), Some("alice"));
        assert_eq!(caller.host_provider_token(), Some("provider-secret"));
    }
}
