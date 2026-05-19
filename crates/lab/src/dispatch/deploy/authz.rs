//! Authorization gates for the deploy service.
//!
//! Deploy requires a dedicated token separate from the general MCP bearer:
//! `LAB_DEPLOY_TOKEN` must be set in the environment before any plan/run/
//! rollback action is accepted.
//!
//! Destructive deploy actions (`run`, `rollback`) additionally require live
//! MCP elicitation when called over MCP. A client that does not advertise
//! elicitation cannot bypass the confirmation by simply setting
//! `params.confirm = true` — we refuse it at this layer.

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::env_non_empty;
use serde_json::Value;

tokio::task_local! {
    /// Per-task MCP context set by the surface adapter before calling
    /// `dispatch_with_runner`. The CLI path scopes this to
    /// `McpContext::Cli`; the MCP adapter uses `McpElicited` or
    /// `HeadlessNoElicitation` depending on capability negotiation.
    pub static MCP_CONTEXT: McpContext;
}

/// Where the request came from, for purposes of destructive-action gating.
#[derive(Debug, Clone, Copy)]
pub enum McpContext {
    /// Command invoked from the CLI; operator confirmed via `-y`.
    Cli,
    // Not yet constructed: wired in when the HTTP surface adds bearer-scoped deploy dispatch.
    #[allow(dead_code)]
    HttpWithToken,
    /// MCP call whose client completed an elicitation exchange.
    McpElicited,
    // Not yet constructed: wired in when the MCP surface implements elicitation negotiation.
    #[allow(dead_code)]
    HeadlessNoElicitation,
}

/// Verify `LAB_DEPLOY_TOKEN` is set and non-empty.
///
/// This is the first check every deploy action runs. The MCP HTTP bearer is
/// insufficient — deploy requires a dedicated token that the operator opts
/// into explicitly.
pub fn require_deploy_token() -> Result<(), ToolError> {
    match env_non_empty("LAB_DEPLOY_TOKEN") {
        Some(ref v) if !v.trim().is_empty() => {
            tracing::info!(
                surface = "dispatch",
                service = "deploy",
                action = "authz.require_deploy_token",
                actor = "operator",
                outcome = "success",
                entity_kind = "env_var",
                entity_id = "LAB_DEPLOY_TOKEN",
                "deploy authorization token gate passed",
            );
            Ok(())
        }
        _ => {
            tracing::warn!(
                surface = "dispatch",
                service = "deploy",
                action = "authz.require_deploy_token",
                actor = "operator",
                outcome = "rejected",
                kind = "auth_failed",
                entity_kind = "env_var",
                entity_id = "LAB_DEPLOY_TOKEN",
                "deploy authorization token gate rejected request",
            );
            Err(ToolError::Sdk {
                sdk_kind: "auth_failed".into(),
                message: "LAB_DEPLOY_TOKEN is required for deploy actions".into(),
            })
        }
    }
}

/// Reject the `confirm: true` headless-bypass for destructive deploy actions.
///
/// When the caller supplies `confirm: true` but the MCP client did not
/// complete an elicitation exchange (i.e., context is
/// `HeadlessNoElicitation`), the request is refused. CLI and elicited MCP
/// calls pass through.
pub fn reject_headless_bypass(params: &Value, ctx: McpContext) -> Result<(), ToolError> {
    let confirm_present = params
        .get("confirm")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if confirm_present && matches!(ctx, McpContext::HeadlessNoElicitation) {
        tracing::warn!(
            surface = "dispatch",
            service = "deploy",
            action = "authz.reject_headless_bypass",
            actor = "mcp_client",
            outcome = "rejected",
            kind = "auth_failed",
            entity_kind = "destructive_action",
            entity_id = "deploy",
            mcp_context = ?ctx,
            "deploy destructive action headless confirmation bypass rejected",
        );
        return Err(ToolError::Sdk {
            sdk_kind: "auth_failed".into(),
            message: "deploy destructive actions require live MCP elicitation; \
                      `confirm: true` without an elicitation response is rejected"
                .into(),
        });
    }
    tracing::info!(
        surface = "dispatch",
        service = "deploy",
        action = "authz.reject_headless_bypass",
        actor = "operator",
        outcome = "success",
        entity_kind = "destructive_action",
        entity_id = "deploy",
        mcp_context = ?ctx,
        "deploy destructive action headless bypass gate passed",
    );
    Ok(())
}

/// Read the current MCP context, falling back to `HeadlessNoElicitation` when
/// the task-local has not been scoped.
///
/// Fails closed: an unscoped call is treated as the most restricted context
/// so that a surface that forgets to call `MCP_CONTEXT.scope(...)` before
/// dispatching a destructive action is refused rather than silently granted
/// operator-level trust.
pub fn current_context() -> McpContext {
    MCP_CONTEXT
        .try_with(|c| *c)
        .unwrap_or(McpContext::HeadlessNoElicitation)
}
