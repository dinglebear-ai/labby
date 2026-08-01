use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, ServiceExt};

struct FixtureServer {
    saw_non_utf8_argument: bool,
}

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
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
            "arguments": request.arguments,
            "saw_non_utf8_argument": self.saw_non_utf8_argument,
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(payload.to_string())]).into())
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
