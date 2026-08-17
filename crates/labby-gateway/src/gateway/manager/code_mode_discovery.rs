//! API-private orchestration for the authenticated root-admin tool browser.

use labby_codemode::host::CodeModeHost;
use labby_codemode::{
    CodeModeCaller, CodeModeCallerCapabilities, CodeModeDescribeResponse, CodeModeSearchResponse,
    CodeModeSurface, ToolScope, describe_visible_tool, search_visible_tools,
};
use labby_runtime::error::ToolError;

use super::GatewayManager;

impl GatewayManager {
    /// Search the root catalog for the API browser. The product API owns the
    /// `lab:admin` authorization gate; this method only projects catalog data.
    pub async fn search_admin_tools(
        &self,
        subject: Option<String>,
        query: &str,
        limit: usize,
    ) -> Result<CodeModeSearchResponse, ToolError> {
        let (render, scope) = self.admin_tool_browser_render(subject).await?;
        search_visible_tools(&render.entries, &scope, query, limit)
    }

    pub async fn describe_admin_tool(
        &self,
        subject: Option<String>,
        target: &str,
    ) -> Result<CodeModeDescribeResponse, ToolError> {
        let (render, scope) = self.admin_tool_browser_render(subject).await?;
        describe_visible_tool(&render.entries, &scope, target)
    }

    async fn admin_tool_browser_render(
        &self,
        subject: Option<String>,
    ) -> Result<(labby_codemode::host::ToolsRender, ToolScope), ToolError> {
        let caller = CodeModeCaller::Scoped {
            capabilities: CodeModeCallerCapabilities {
                can_read: true,
                can_execute: true,
                can_use_snippets: false,
                is_admin: true,
            },
            sub: subject,
        };
        let scope = ToolScope::default();
        let render =
            CodeModeHost::list_tools(self, &caller, CodeModeSurface::Api, &scope, false, true)
                .await?;
        Ok((render, scope))
    }
}
