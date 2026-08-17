//! API-private orchestration for the authenticated root-admin tool browser.

use labby_codemode::host::CodeModeHost;
use labby_codemode::{
    CodeModeCaller, CodeModeCallerCapabilities, CodeModeDescribeResponse, CodeModeSearchResponse,
    CodeModeSurface, ToolScope, describe_visible_tool, search_visible_tools,
};
use labby_runtime::error::ToolError;

use super::GatewayManager;

/// Proof-shaped context constructed only after the product API authenticates
/// an exact `lab:admin` scope. It is deliberately not serializable.
#[derive(Debug, Clone)]
pub struct AdminToolBrowserContext {
    subject: Option<String>,
}

impl AdminToolBrowserContext {
    /// Construct after the adapter has authenticated an administrator.
    #[must_use]
    pub fn from_authenticated_admin(subject: Option<String>) -> Self {
        Self { subject }
    }
}

impl GatewayManager {
    pub async fn search_admin_tools(
        &self,
        context: AdminToolBrowserContext,
        query: &str,
        limit: usize,
    ) -> Result<CodeModeSearchResponse, ToolError> {
        let (render, scope) = self.admin_tool_browser_render(context).await?;
        search_visible_tools(&render.entries, &scope, query, limit)
    }

    pub async fn describe_admin_tool(
        &self,
        context: AdminToolBrowserContext,
        target: &str,
    ) -> Result<CodeModeDescribeResponse, ToolError> {
        let (render, scope) = self.admin_tool_browser_render(context).await?;
        describe_visible_tool(&render.entries, &scope, target)
    }

    async fn admin_tool_browser_render(
        &self,
        context: AdminToolBrowserContext,
    ) -> Result<(labby_codemode::host::ToolsRender, ToolScope), ToolError> {
        let caller = CodeModeCaller::Scoped {
            capabilities: CodeModeCallerCapabilities {
                can_read: true,
                can_execute: true,
                can_use_snippets: false,
                is_admin: true,
            },
            sub: context.subject,
        };
        let scope = ToolScope::default();
        let render =
            CodeModeHost::list_tools(self, &caller, CodeModeSurface::Api, &scope, false, true)
                .await?;
        Ok((render, scope))
    }
}
