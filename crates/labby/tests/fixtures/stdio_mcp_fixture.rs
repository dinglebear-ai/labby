//! Minimal stdio MCP fixture binary used by integration and conformance tests.
//!
//! It exposes a deterministic tool/resource surface so transport, lifecycle, and
//! relay behavior can be tested without depending on an external MCP server.

#![allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
    GetPromptRequestParams, GetPromptResponse, GetPromptResult, ListPromptsResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, Prompt, PromptMessage,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, Role, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, ServiceExt};

struct FixtureServer {
    saw_non_utf8_argument: bool,
}

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![Tool::new(
            "fixture.echo",
            "Echo fixture input",
            Arc::new(serde_json::Map::new()),
        )]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name.as_ref() != "fixture.echo" {
            return Err(ErrorData::invalid_params("unknown fixture tool", None));
        }
        let payload = serde_json::json!({
            "cwd": std::env::current_dir().ok(),
            "explicit_env": std::env::var("PROXY_EXPLICIT").ok(),
            "inherited_path": std::env::var("PATH").ok(),
            "scrub_canary": std::env::var("PROXY_SCRUB_CANARY").ok(),
            "arguments": request.arguments,
            "saw_non_utf8_argument": self.saw_non_utf8_argument,
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(payload.to_string())]).into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![Resource::new(
            "fixture://status",
            "fixture.status",
        )]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if request.uri != "fixture://status" {
            return Err(ErrorData::invalid_params("unknown fixture resource", None));
        }
        Ok(
            ReadResourceResult::new(vec![ResourceContents::text("fixture-ready", request.uri)])
                .into(),
        )
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult::with_all_items(vec![Prompt::new(
            "fixture.prompt",
            Some("Fixture prompt"),
            None,
        )]))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        if request.name != "fixture.prompt" {
            return Err(ErrorData::invalid_params("unknown fixture prompt", None));
        }
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            "fixture prompt result",
        )])
        .into())
    }
}

#[cfg(unix)]
fn is_non_utf8_marker(argument: &std::ffi::OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt as _;
    argument.as_bytes() == [b'x', 0xff]
}

#[cfg(not(unix))]
fn is_non_utf8_marker(_argument: &std::ffi::OsStr) -> bool {
    false
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let mut saw_non_utf8_argument = false;
    while let Some(arg) = args.next() {
        if arg == "--pid-file" {
            let path = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing pid path"))?;
            std::fs::write(path, std::process::id().to_string())?;
        } else if is_non_utf8_marker(&arg) {
            saw_non_utf8_argument = true;
        }
    }

    let running = FixtureServer {
        saw_non_utf8_argument,
    }
    .serve((tokio::io::stdin(), tokio::io::stdout()))
    .await?;
    running.waiting().await?;
    Ok(())
}
