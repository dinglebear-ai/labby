//! Minimal stdio MCP fixture binary used by integration and conformance tests.
//!
//! It exposes a deterministic tool/resource surface so transport, lifecycle, and
//! relay behavior can be tested without depending on an external MCP server.

#![allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

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
    forge_ledger: Option<PathBuf>,
}

impl FixtureServer {
    fn begin_forge_invocation(&self) -> Result<ForgeEffectGuard, ErrorData> {
        match self.forge_ledger.as_deref() {
            Some(path) => {
                update_ledger(path, 1, 1)?;
                Ok(ForgeEffectGuard(Some(path.to_owned())))
            }
            None => {
                self.invocation_count.fetch_add(1, Ordering::SeqCst);
                Ok(ForgeEffectGuard(None))
            }
        }
    }

    fn observed_effects(&self) -> Result<(u64, u64), ErrorData> {
        match self.forge_ledger.as_deref() {
            Some(path) => update_ledger(path, 0, 0),
            None => Ok((self.invocation_count.load(Ordering::SeqCst), 0)),
        }
    }
}

struct ForgeEffectGuard(Option<PathBuf>);

impl Drop for ForgeEffectGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.as_deref() {
            drop(update_ledger(path, 0, -1));
        }
    }
}

fn update_ledger(
    path: &Path,
    invocation_delta: u64,
    active_delta: i64,
) -> Result<(u64, u64), ErrorData> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| ErrorData::internal_error("forge ledger unavailable", None))?;
    file.lock()
        .map_err(|_| ErrorData::internal_error("forge ledger unavailable", None))?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(33)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorData::internal_error("forge ledger unavailable", None))?;
    if bytes.len() > 32 {
        return Err(ErrorData::internal_error("forge ledger is invalid", None));
    }
    let (invocations, active) = if bytes.is_empty() {
        (0, 0)
    } else {
        let value = std::str::from_utf8(&bytes)
            .map_err(|_| ErrorData::internal_error("forge ledger is invalid", None))?;
        let mut fields = value.split_ascii_whitespace();
        let invocations = fields
            .next()
            .and_then(|field| field.parse::<u64>().ok())
            .ok_or_else(|| ErrorData::internal_error("forge ledger is invalid", None))?;
        let active = fields
            .next()
            .and_then(|field| field.parse::<u64>().ok())
            .ok_or_else(|| ErrorData::internal_error("forge ledger is invalid", None))?;
        if fields.next().is_some() {
            return Err(ErrorData::internal_error("forge ledger is invalid", None));
        }
        (invocations, active)
    };
    let invocations = invocations
        .checked_add(invocation_delta)
        .ok_or_else(|| ErrorData::internal_error("forge ledger exhausted", None))?;
    let active = active
        .checked_add_signed(active_delta)
        .ok_or_else(|| ErrorData::internal_error("forge ledger active count is invalid", None))?;
    if invocation_delta != 0 || active_delta != 0 {
        file.set_len(0)
            .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .and_then(|()| write!(file, "{invocations} {active}").map(|_| ()))
            .and_then(|()| file.sync_data())
            .map_err(|_| ErrorData::internal_error("forge ledger unavailable", None))?;
    }
    Ok((invocations, active))
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
            let mut pending = Tool::new(
                "forge.pending",
                "Read-only pending operation for cancellation qualification",
                object(serde_json::Map::new()),
            );
            pending.annotations = safe.annotations.clone();
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
                pending,
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
        context: RequestContext<RoleServer>,
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
        let _effect = self.begin_forge_invocation()?;
        if request.name.as_ref() == "forge.pending" {
            // rmcp signals request cancellation through the context token;
            // handlers must cooperate. An unconditional sleep would measure
            // fixture behavior rather than gateway cancellation delivery.
            tokio::select! {
                () = context.ct.cancelled() => {
                    return Err(ErrorData::internal_error("fixture request cancelled", None));
                }
                () = tokio::time::sleep(std::time::Duration::from_secs(10)) => {}
            }
        }
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
            let (invocation_count, active_count) = self.observed_effects()?;
            let mut status = serde_json::json!({
                "invocation_count": invocation_count,
                "schema_revision": self.schema_revision,
            });
            if self.forge_ledger.is_some() {
                status["active_count"] = serde_json::json!(active_count);
            }
            return Ok(ReadResourceResult::new(vec![ResourceContents::text(
                status.to_string(),
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
    let mut forge_ledger = None;
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
        } else if arg == "--forge-ledger" {
            forge_ledger = Some(
                args.next()
                    .ok_or_else(|| anyhow::anyhow!("missing forge ledger path"))?
                    .into(),
            );
        }
    }

    let running = FixtureServer {
        saw_non_utf8_argument,
        forge,
        schema_revision,
        invocation_count: AtomicU64::new(0),
        forge_ledger,
    }
    .serve((tokio::io::stdin(), tokio::io::stdout()))
    .await?;
    running.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod ledger_tests {
    use super::*;

    #[test]
    fn durable_ledger_serializes_concurrent_effects() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("effects.count");
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..20 {
                        let guard = update_ledger(path, 1, 1).unwrap();
                        assert!(guard.1 > 0);
                        update_ledger(path, 0, -1).unwrap();
                    }
                });
            }
        });
        assert_eq!(update_ledger(&path, 0, 0).unwrap(), (160, 0));
    }

    #[test]
    fn durable_ledger_rejects_malformed_state() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("effects.count");
        std::fs::write(&path, "not-a-ledger").unwrap();
        let error = update_ledger(&path, 0, 0).unwrap_err();
        assert_eq!(error.message, "forge ledger is invalid");
    }

    #[test]
    fn durable_ledger_rejects_oversized_state_instead_of_accepting_a_prefix() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("effects.count");
        std::fs::write(&path, format!("1 0{}", " ".repeat(30))).unwrap();
        let error = update_ledger(&path, 0, 0).unwrap_err();
        assert_eq!(error.message, "forge ledger is invalid");
    }
}
