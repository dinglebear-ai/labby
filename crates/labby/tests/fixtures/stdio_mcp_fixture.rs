//! Minimal stdio MCP fixture binary used by integration and conformance tests.
//!
//! It exposes a deterministic tool/resource surface so transport, lifecycle, and
//! relay behavior can be tested without depending on an external MCP server.

#![allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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
    forge: bool,
    schema_revision: u64,
    invocation_count: AtomicU64,
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
        let mut tools = vec![Tool::new(
            "fixture.echo",
            "Echo fixture input",
            Arc::new(serde_json::Map::new()),
        )];
        if self.forge {
            let object = |properties: serde_json::Map<String, serde_json::Value>| {
                Arc::new(serde_json::Map::from_iter([
                    ("type".to_string(), serde_json::json!("object")),
                    (
                        "properties".to_string(),
                        serde_json::Value::Object(properties),
                    ),
                ]))
            };
            let mut safe = Tool::new(
                "forge.safe",
                "Safe primitive Forge tool",
                object(serde_json::Map::from_iter([
                    ("query".to_string(), serde_json::json!({"type":"string"})),
                    (
                        "limit".to_string(),
                        serde_json::json!({"type":"integer","minimum":1,"maximum":1000}),
                    ),
                    ("enabled".to_string(), serde_json::json!({"type":"boolean"})),
                ])),
            );
            safe.annotations = Some(
                rmcp::model::ToolAnnotations::new()
                    .read_only(true)
                    .destructive(false)
                    .idempotent(true),
            );
            let mut destructive = Tool::new(
                "forge.destructive",
                "Destructive Forge fixture",
                object(serde_json::Map::new()),
            );
            destructive.annotations = Some(
                rmcp::model::ToolAnnotations::new()
                    .read_only(false)
                    .destructive(true),
            );
            let mutable_property = format!("optional_v{}", self.schema_revision);
            tools.extend([
                safe,
                Tool::new(
                    "forge.unsupported",
                    "Unsupported nested Forge schema",
                    object(serde_json::Map::from_iter([(
                        "nested".to_string(),
                        serde_json::json!({"type":"array","items":{"type":"object"}}),
                    )])),
                ),
                destructive,
                Tool::new(
                    "forge.delay",
                    "Delayed Forge result",
                    object(serde_json::Map::new()),
                ),
                Tool::new(
                    "forge.error",
                    "Structured Forge error",
                    object(serde_json::Map::new()),
                ),
                Tool::new(
                    "forge.large",
                    "Bounded large Forge result",
                    object(serde_json::Map::new()),
                ),
                Tool::new(
                    "forge.subject",
                    "Subject-specific Forge result",
                    object(serde_json::Map::from_iter([(
                        "subject".to_string(),
                        serde_json::json!({"type":"string"}),
                    )])),
                ),
                Tool::new(
                    "forge.mutable",
                    "Mutable Forge schema",
                    object(serde_json::Map::from_iter([(
                        mutable_property,
                        serde_json::json!({"type":"string"}),
                    )])),
                ),
            ]);
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name.as_ref() == "fixture.echo" {
            let payload = serde_json::json!({
                "cwd": std::env::current_dir().ok(),
                "explicit_env": std::env::var("PROXY_EXPLICIT").ok(),
                "inherited_path": std::env::var("PATH").ok(),
                "scrub_canary": std::env::var("PROXY_SCRUB_CANARY").ok(),
                "arguments": request.arguments,
                "saw_non_utf8_argument": self.saw_non_utf8_argument,
            });
            return Ok(
                CallToolResult::success(vec![ContentBlock::text(payload.to_string())]).into(),
            );
        }
        if !self.forge || !request.name.as_ref().starts_with("forge.") {
            return Err(ErrorData::invalid_params("unknown fixture tool", None));
        }
        self.invocation_count.fetch_add(1, Ordering::SeqCst);
        if request.name.as_ref() == "forge.delay" {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        if request.name.as_ref() == "forge.error" {
            return Err(ErrorData::invalid_params("forge_fixture_error", None));
        }
        let payload = match request.name.as_ref() {
            "forge.large" => {
                serde_json::json!({"rows": (0..1000).map(|index| serde_json::json!({"index": index, "value": "x".repeat(1024)})).collect::<Vec<_>>() })
            }
            "forge.subject" => {
                serde_json::json!({"subject": request.arguments.as_ref().and_then(|arguments| arguments.get("subject")).cloned()})
            }
            _ => {
                serde_json::json!({"tool": request.name, "arguments": request.arguments, "schema_revision": self.schema_revision})
            }
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(payload.to_string())]).into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let mut resources = vec![Resource::new("fixture://status", "fixture.status")];
        if self.forge {
            resources.push(Resource::new(
                "fixture://forge-status",
                "fixture.forge-status",
            ));
        }
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if request.uri == "fixture://forge-status" && self.forge {
            return Ok(ReadResourceResult::new(vec![ResourceContents::text(
                serde_json::json!({
                    "invocation_count": self.invocation_count.load(Ordering::SeqCst),
                    "schema_revision": self.schema_revision,
                })
                .to_string(),
                request.uri,
            )])
            .into());
        }
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
    let mut forge = false;
    let mut schema_revision = 1;
    while let Some(arg) = args.next() {
        if arg == "--pid-file" {
            let path = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing pid path"))?;
            std::fs::write(path, std::process::id().to_string())?;
        } else if is_non_utf8_marker(&arg) {
            saw_non_utf8_argument = true;
        } else if arg == "--forge" {
            forge = true;
        } else if arg == "--schema-revision" {
            schema_revision = args
                .next()
                .and_then(|value| value.to_string_lossy().parse().ok())
                .unwrap_or(1);
        }
    }

    let running = FixtureServer {
        saw_non_utf8_argument,
        forge,
        schema_revision,
        invocation_count: AtomicU64::new(0),
    }
    .serve((tokio::io::stdin(), tokio::io::stdout()))
    .await?;
    running.waiting().await?;
    Ok(())
}
