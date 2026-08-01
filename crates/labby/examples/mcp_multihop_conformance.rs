//! Labby-native MCP multi-hop conformance driver.
//!
//! The driver launches this process as a synthetic leaf behind two real Labby
//! stdio gateways:
//!
//! client -> root Labby -> middle Labby -> synthetic leaf

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use labby::config::{GatewayPreferences, LabConfig, UpstreamConfig};
use rmcp::model::{
    ArgumentInfo, CallToolRequestParams, CallToolResponse, CallToolResult, ClientCapabilities,
    ClientInfo, CompleteRequestParams, CompleteResult, CompletionInfo, ContentBlock, ElicitRequest,
    ElicitRequestParams, ElicitationSchema, ErrorData, GetPromptRequestParams, GetPromptResponse,
    GetPromptResult, Implementation, InputRequest, InputRequests, InputRequiredResult,
    ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult,
    MetaObject, PaginatedRequestParams, PrimitiveSchemaDefinition, Prompt, PromptMessage,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    Reference, Resource, ResourceContents, ResourceTemplate, Role, ServerCapabilities, ServerInfo,
    Tool,
};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, Peer, RequestContext};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{ClientHandler, RoleClient, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};
use tempfile::TempDir;
use tokio::process::Command;

const TOOL_COUNT: usize = 75;
const PROMPT_COUNT: usize = 70;
const RESOURCE_COUNT: usize = 70;
const TEMPLATE_COUNT: usize = 70;
const SERVER_INFO_KEY: &str = "io.modelcontextprotocol/serverInfo";
const UPSTREAM_SERVER_INFO_KEY: &str = "ai.dinglebear.labby/upstreamServerInfo";

#[derive(Clone, Default)]
struct LeafServer;

fn leaf_meta() -> MetaObject {
    let mut meta = MetaObject::default();
    meta.0.insert(
        SERVER_INFO_KEY.to_string(),
        serde_json::to_value(Implementation::new("labby-conformance-leaf", "1.0.0"))
            .expect("leaf implementation serializes"),
    );
    meta.0.insert("leaf.trace".to_string(), json!("multi-hop"));
    meta
}

fn leaf_tool(index: usize) -> Tool {
    Tool::new(
        format!("echo_{index:03}"),
        "Echo a value through two Labby gateways",
        Arc::new(Map::from_iter([(
            "value".to_string(),
            json!({"type": "string"}),
        )])),
    )
}

fn input_required() -> InputRequiredResult {
    let schema = ElicitationSchema::builder()
        .required_property(
            "confirm",
            PrimitiveSchemaDefinition::Boolean(rmcp::model::BooleanSchema::default()),
        )
        .build()
        .expect("valid elicitation schema");
    let request = ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: "confirm the multi-hop request".to_string(),
        requested_schema: schema,
    });
    InputRequiredResult::from_input_requests(InputRequests::from([(
        "confirmation".to_string(),
        InputRequest::Elicitation(request),
    )]))
}

impl ServerHandler for LeafServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .enable_completions()
                .build(),
        );
        info.server_info = Implementation::new("labby-conformance-leaf", "1.0.0");
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut tools = (0..TOOL_COUNT).map(leaf_tool).collect::<Vec<_>>();
        tools.push(Tool::new(
            "needs_input",
            "Return a first-class MRTR input_required result",
            Arc::new(Map::new()),
        ));
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name == "needs_input" {
            return Ok(input_required().into());
        }
        let value = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("value"))
            .and_then(Value::as_str)
            .unwrap_or("missing");
        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "leaf:{}:{value}",
            request.name
        ))])
        .with_meta(Some(leaf_meta()))
        .into())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult::with_all_items(
            (0..PROMPT_COUNT)
                .map(|index| {
                    Prompt::new(format!("prompt_{index:03}"), Some("Multi-hop prompt"), None)
                })
                .collect(),
        ))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        let mut result = GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            format!("leaf-prompt:{}", request.name),
        )]);
        result.meta = Some(leaf_meta());
        Ok(result.into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(
            (0..RESOURCE_COUNT)
                .map(|index| {
                    Resource::new(
                        format!("fixture://resource/{index:03}"),
                        format!("resource_{index:03}"),
                    )
                })
                .collect(),
        ))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::with_all_items(
            (0..TEMPLATE_COUNT)
                .map(|index| {
                    ResourceTemplate::new(
                        format!("fixture://template/{index:03}/{{value}}"),
                        format!("template_{index:03}"),
                    )
                })
                .collect(),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let mut result = ReadResourceResult::new(vec![ResourceContents::text(
            format!("leaf-resource:{}", request.uri),
            request.uri,
        )]);
        result.meta = Some(leaf_meta());
        Ok(result.into())
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        let value = format!("{}-leaf-completion", request.argument.value);
        let mut result = CompleteResult::new(
            CompletionInfo::with_pagination(vec![value], Some(1), false)
                .expect("valid completion fixture"),
        );
        result.meta = Some(leaf_meta());
        Ok(result)
    }
}

#[derive(Clone, Default)]
struct DriverClient;

impl ClientHandler for DriverClient {
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        info.client_info = Implementation::new("labby-multihop-conformance", "1.0.0");
        info.capabilities = ClientCapabilities::builder()
            .enable_elicitation()
            .enable_tasks()
            .build();
        info
    }
}

fn stdio_upstream(
    name: &str,
    command: PathBuf,
    args: Vec<String>,
    env: BTreeMap<String, String>,
) -> UpstreamConfig {
    UpstreamConfig {
        name: name.to_string(),
        enabled: true,
        priority: 1.0,
        url: None,
        transport: None,
        socket_path: None,
        headers: BTreeMap::new(),
        bearer_token_env: None,
        command: Some(command.display().to_string()),
        args,
        env,
        proxy_resources: true,
        proxy_prompts: true,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
    }
}

fn http_upstream(name: &str, url: String, bearer_token_env: &str) -> UpstreamConfig {
    UpstreamConfig {
        name: name.to_string(),
        enabled: true,
        priority: 1.0,
        url: Some(url),
        transport: None,
        socket_path: None,
        headers: BTreeMap::new(),
        bearer_token_env: Some(bearer_token_env.to_string()),
        command: None,
        args: Vec::new(),
        env: BTreeMap::new(),
        proxy_resources: true,
        proxy_prompts: true,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
    }
}

fn config_path(home: &Path) -> PathBuf {
    home.join(".config/labby/config.toml")
}

fn write_config(home: &Path, upstream: UpstreamConfig) -> Result<()> {
    let path = config_path(home);
    std::fs::create_dir_all(path.parent().context("config parent")?)?;
    let mut config = LabConfig::default();
    config.gateway = GatewayPreferences {
        disable_spawn_guard: true,
        upstream_stderr_level: Some("warn".to_string()),
        ..GatewayPreferences::default()
    };
    config.upstream = vec![upstream];
    std::fs::write(path, toml::to_string(&config)?)?;
    Ok(())
}

fn force_full_reload_on_next_request(home: &Path, timeout_ms: u64) -> Result<()> {
    let path = config_path(home);
    let raw = std::fs::read_to_string(&path)?;
    let mut config: LabConfig = toml::from_str(&raw)?;
    config.upstream_request_timeout_ms = Some(timeout_ms);
    std::fs::write(path, toml::to_string(&config)?)?;
    Ok(())
}

async fn call_service_action(
    peer: &Peer<RoleClient>,
    tool_name: impl Into<String>,
    action: &str,
) -> Result<CallToolResult> {
    let tool_name = tool_name.into();
    let mut request = CallToolRequestParams::new(tool_name.clone());
    request.arguments = Some(Map::from_iter([
        ("action".to_string(), json!(action)),
        ("params".to_string(), json!({})),
    ]));
    match peer.call_tool_once(request).await? {
        CallToolResponse::Complete(result) if result.is_error != Some(true) => Ok(result),
        CallToolResponse::Complete(result) => {
            let detail = result
                .content
                .first()
                .and_then(ContentBlock::as_text)
                .map(|content| content.text.clone())
                .unwrap_or_else(|| "tool returned an error without text".to_string());
            anyhow::bail!("{action} failed through {tool_name}: {detail}")
        }
        CallToolResponse::InputRequired(_) => {
            anyhow::bail!("{action} unexpectedly required interactive input")
        }
        CallToolResponse::Task(_) => anyhow::bail!("{action} unexpectedly returned a task"),
        _ => anyhow::bail!("{action} returned an unsupported result variant"),
    }
}

async fn wait_for_http_ready(base_url: &str) -> Result<()> {
    let client = reqwest::Client::new();
    for _ in 0..80 {
        if client
            .get(format!("{base_url}/ready"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    anyhow::bail!("middle Labby HTTP daemon did not become ready at {base_url}")
}

async fn reload_http_gateway(base_url: &str, token: &str) -> Result<()> {
    let response = reqwest::Client::new()
        .post(format!("{base_url}/v1/gateway"))
        .bearer_auth(token)
        .json(&json!({"action": "gateway.reload", "params": {}}))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    ensure!(
        status.is_success(),
        "middle gateway.reload returned {status}: {body}"
    );
    Ok(())
}

fn nested_name<'a, T>(
    items: &'a [T],
    name: impl Fn(&'a T) -> &'a str,
    suffix: &str,
) -> Result<&'a str> {
    items
        .iter()
        .map(name)
        .find(|candidate| candidate.ends_with(suffix))
        .with_context(|| format!("missing nested catalog item ending in {suffix}"))
}

async fn run_driver() -> Result<()> {
    drop(rustls::crypto::ring::default_provider().install_default());
    let temp = TempDir::new()?;
    let root_home = temp.path().join("root-home");
    let middle_home = temp.path().join("middle-home");
    std::fs::create_dir_all(&root_home)?;
    std::fs::create_dir_all(&middle_home)?;

    let example = std::env::current_exe()?;
    let debug_dir = example
        .parent()
        .and_then(Path::parent)
        .context("example must run from target/<profile>/examples")?;
    let labby_bin = debug_dir.join(if cfg!(windows) { "labby.exe" } else { "labby" });
    ensure!(
        labby_bin.exists(),
        "Labby binary not found at {}",
        labby_bin.display()
    );

    write_config(
        &middle_home,
        stdio_upstream(
            "leaf",
            example.clone(),
            vec!["fixture".to_string()],
            BTreeMap::new(),
        ),
    )?;

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let middle_port = listener.local_addr()?.port();
    drop(listener);
    let middle_base_url = format!("http://127.0.0.1:{middle_port}");
    let middle_token = "8c1f97449584ebcc6025655d738a8b40a3a488dd407ac89a1c42146864bd0179";
    let mut middle_child = Command::new(&labby_bin)
        .arg("serve")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(middle_port.to_string())
        .env("HOME", &middle_home)
        .env("LABBY_AUTH_MODE", "bearer")
        .env("LABBY_MCP_HTTP_TOKEN", middle_token)
        .env("LABBY_CODE_MODE_JOURNAL_DISABLED", "1")
        .env("LABBY_GATEWAY_USAGE_DISABLED", "1")
        .env("LABBY_LOG", "labby=warn,labby_gateway=warn")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    wait_for_http_ready(&middle_base_url).await?;
    force_full_reload_on_next_request(&middle_home, 30_002)?;
    reload_http_gateway(&middle_base_url, middle_token).await?;

    write_config(
        &root_home,
        http_upstream(
            "middle",
            format!("{middle_base_url}/mcp"),
            "MULTIHOP_MIDDLE_TOKEN",
        ),
    )?;

    let transport = TokioChildProcess::new(Command::new(&labby_bin).configure(|command| {
        command
            .arg("serve")
            .arg("mcp")
            .arg("--stdio")
            .env("HOME", &root_home)
            .env("LABBY_CODE_MODE_JOURNAL_DISABLED", "1")
            .env("LABBY_GATEWAY_USAGE_DISABLED", "1")
            .env("MULTIHOP_MIDDLE_TOKEN", middle_token)
            .env("LABBY_LOG", "labby=warn,labby_gateway=warn");
    }))?;
    let service = DriverClient
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await?;
    let peer = service.peer().clone();
    let peer_info = peer.peer_info().context("root discovery result")?;
    ensure!(peer_info.protocol_version == ProtocolVersion::V_2026_07_28);
    ensure!(
        peer_info
            .server_info
            .as_ref()
            .is_some_and(|info| info.name == "labby"),
        "root discovery must identify Labby"
    );

    // Raw catalog listing intentionally never cold-connects upstreams. Middle
    // was warmed through its authenticated operator API above; now force a full
    // root reconcile so its serving pool discovers middle's complete catalog.
    force_full_reload_on_next_request(&root_home, 30_003)?;
    call_service_action(&peer, "gateway", "gateway.reload").await?;

    let mut tools = Vec::new();
    let mut leaf_tools = 0;
    for _ in 0..30 {
        tools = peer.list_all_tools().await?;
        leaf_tools = tools
            .iter()
            .filter(|tool| tool.name.ends_with("needs_input") || tool.name.contains("echo_"))
            .count();
        if leaf_tools >= TOOL_COUNT + 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let observed = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .take(30)
        .collect::<Vec<_>>();
    ensure!(
        leaf_tools >= TOOL_COUNT + 1,
        "expected all nested leaf tools, got {leaf_tools}; observed {observed:?}"
    );
    let echo_name = nested_name(&tools, |tool| tool.name.as_ref(), "echo_074")?.to_string();
    let needs_input_name =
        nested_name(&tools, |tool| tool.name.as_ref(), "needs_input")?.to_string();

    let mut echo = CallToolRequestParams::new(echo_name);
    echo.arguments = Some(Map::from_iter([(
        "value".to_string(),
        json!("through-two-hops"),
    )]));
    let CallToolResponse::Complete(echo_result) = peer.call_tool_once(echo).await? else {
        anyhow::bail!("nested echo did not complete");
    };
    let text = echo_result
        .content
        .first()
        .and_then(ContentBlock::as_text)
        .map(|content| content.text.as_str())
        .context("nested echo text result")?;
    ensure!(text.contains("echo_074:through-two-hops"));
    let meta = echo_result.meta.context("nested echo provenance")?;
    ensure!(meta.0[SERVER_INFO_KEY]["name"] == json!("labby"));
    ensure!(meta.0[UPSTREAM_SERVER_INFO_KEY]["name"] == json!("labby-conformance-leaf"));
    ensure!(meta.0["leaf.trace"] == json!("multi-hop"));

    let input_required = peer
        .call_tool_once(CallToolRequestParams::new(needs_input_name))
        .await?;
    ensure!(matches!(input_required, CallToolResponse::InputRequired(_)));

    let prompts = peer.list_all_prompts().await?;
    ensure!(
        prompts
            .iter()
            .filter(|prompt| prompt.name.contains("middle") && prompt.name.contains("leaf"))
            .count()
            >= PROMPT_COUNT
    );
    let prompt_name =
        nested_name(&prompts, |prompt| prompt.name.as_str(), "prompt_069")?.to_string();
    let GetPromptResponse::Complete(prompt) = peer
        .get_prompt_once(GetPromptRequestParams::new(prompt_name.clone()))
        .await?
    else {
        anyhow::bail!("nested prompt did not complete");
    };
    ensure!(prompt.messages.len() == 1);
    ensure!(
        prompt
            .meta
            .as_ref()
            .is_some_and(|meta| meta.0[SERVER_INFO_KEY]["name"] == json!("labby"))
    );

    let resources = peer.list_all_resources().await?;
    ensure!(
        resources
            .iter()
            .filter(|resource| resource.name.contains("resource_"))
            .count()
            >= RESOURCE_COUNT
    );
    let resource_uri = resources
        .iter()
        .find(|resource| resource.name.ends_with("resource_069"))
        .map(|resource| resource.uri.clone())
        .context("nested resource_069")?;
    let ReadResourceResponse::Complete(resource) = peer
        .read_resource_once(ReadResourceRequestParams::new(resource_uri))
        .await?
    else {
        anyhow::bail!("nested resource did not complete");
    };
    ensure!(resource.contents.len() == 1);
    ensure!(
        resource
            .meta
            .as_ref()
            .is_some_and(|meta| meta.0[SERVER_INFO_KEY]["name"] == json!("labby"))
    );

    let templates = peer.list_all_resource_templates().await?;
    ensure!(
        templates
            .iter()
            .filter(|template| template.name.contains("template_"))
            .count()
            >= TEMPLATE_COUNT
    );
    let template_uri = templates
        .iter()
        .find(|template| template.name.ends_with("template_069"))
        .map(|template| template.uri_template.clone())
        .context("nested template_069")?;
    let completion = peer
        .complete(CompleteRequestParams::new(
            Reference::for_resource(template_uri),
            ArgumentInfo::new("value", "labby"),
        ))
        .await?;
    ensure!(completion.completion.values == ["labby-leaf-completion"]);
    ensure!(
        completion
            .meta
            .as_ref()
            .is_some_and(|meta| meta.0[SERVER_INFO_KEY]["name"] == json!("labby"))
    );

    service.cancel().await?;
    middle_child.kill().await.ok();
    println!("Labby multi-hop conformance passed");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("fixture") => {
            LeafServer
                .serve(rmcp::transport::stdio())
                .await?
                .waiting()
                .await?;
            Ok(())
        }
        Some("driver") | None => run_driver().await,
        Some(other) => anyhow::bail!("unknown mode: {other}"),
    }
}
