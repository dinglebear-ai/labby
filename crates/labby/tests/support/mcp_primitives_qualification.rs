use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use axum::Router;
use labby_gateway::upstream::http_client::BodyCappedHttpClient;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams, ClientCapabilities,
    ClientInfo, CompleteRequestParams, CompleteResult, CompletionInfo, ContentBlock,
    CreateTaskResult, DetailedTask, ErrorCode, ErrorData, GetPromptRequestParams,
    GetPromptResponse, GetPromptResult, GetTaskParams, GetTaskResult, Implementation,
    ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, ProgressNotificationParam, Prompt, PromptMessage, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ResourceTemplate, Role, ServerCapabilities, ServerInfo, Task, TaskPayload,
    TaskStatus, Tool, UpdateTaskParams,
};
use rmcp::service::{
    ClientLifecycleMode, ClientServiceExt, NotificationContext, RequestContext, RunningService,
};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use rmcp::{ClientHandler, RoleClient, RoleServer, ServerHandler};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::live_labby::{CleanupResult, LiveLabbyBuilder, LiveLabbyGuard};

pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CLIENT_RESPONSE_CAP: usize = 12 * 1024 * 1024;
const BYTE_BOMB_BYTES: usize = 10 * 1024 * 1024 + 16 * 1024;
const TOKEN: &str = "q1-primitives-qualification-token";

#[derive(Clone, Debug, Default)]
pub(crate) struct PrimitiveClient {
    progress: Arc<tokio::sync::Mutex<Vec<ProgressNotificationParam>>>,
}

impl ClientHandler for PrimitiveClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::builder().enable_tasks().build(),
            Implementation::new("labby-q1-primitives", "1"),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
    }

    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.progress.lock().await.push(params);
    }
}

impl PrimitiveClient {
    pub(crate) async fn progress(&self) -> Vec<ProgressNotificationParam> {
        self.progress.lock().await.clone()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FixtureMode {
    Normal,
    CursorBomb,
    PageBomb,
    ItemBomb,
    ByteBomb,
}

#[derive(Default)]
struct FixtureCounters {
    tools: AtomicUsize,
    tool_calls: AtomicUsize,
    resources: AtomicUsize,
    templates: AtomicUsize,
    prompts: AtomicUsize,
    prompt_gets: AtomicUsize,
    completions: AtomicUsize,
    task_updates: AtomicUsize,
    task_cancellations: AtomicUsize,
    progress_cancellations: AtomicUsize,
    progress_tokens: tokio::sync::Mutex<Vec<rmcp::model::ProgressToken>>,
}

#[derive(Clone)]
struct FixtureServer {
    owner: Arc<str>,
    mode: FixtureMode,
    generation: Arc<AtomicUsize>,
    counters: Arc<FixtureCounters>,
}

impl FixtureServer {
    fn unsupported(method: &'static str) -> ErrorData {
        ErrorData::new(ErrorCode::METHOD_NOT_FOUND, method, None)
    }

    fn normal_resources(&self) -> Vec<Resource> {
        let generation = self.generation.load(Ordering::SeqCst);
        vec![
            Resource::new("fixture://text", "text"),
            Resource::new("fixture://blob", "blob"),
            Resource::new("fixture://shared", "shared"),
            {
                let mut meta = rmcp::model::MetaObject::new();
                meta.0.insert(
                    "qualificationOwner".to_string(),
                    serde_json::json!(self.owner.as_ref()),
                );
                Resource::new("ui://fixture/app.html", "fixture-app")
                    .with_mime_type("text/html;profile=mcp-app")
                    .with_meta(meta)
            },
            Resource::new(
                format!("fixture://dynamic-v{generation}"),
                format!("dynamic-v{generation}"),
            ),
        ]
    }

    fn normal_prompts(&self) -> Vec<Prompt> {
        let generation = self.generation.load(Ordering::SeqCst);
        vec![
            Prompt::new("shared", Some("shared fixture prompt"), None),
            Prompt::new("oversized", Some("oversized fixture prompt"), None),
            Prompt::new(
                format!("dynamic-v{generation}"),
                Some("generation fixture prompt"),
                None,
            ),
        ]
    }

    fn normal_templates(&self) -> Vec<ResourceTemplate> {
        let generation = self.generation.load(Ordering::SeqCst);
        vec![
            ResourceTemplate::new("fixture://template/{value}", "template"),
            ResourceTemplate::new(
                format!("fixture://dynamic-v{generation}/{{value}}"),
                format!("dynamic-v{generation}"),
            ),
        ]
    }
}

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerInfo {
        let capabilities = match self.mode {
            FixtureMode::Normal => ServerCapabilities::builder()
                .enable_tools()
                .enable_tasks()
                .enable_resources()
                .enable_resources_list_changed()
                .enable_prompts()
                .enable_prompts_list_changed()
                .enable_completions()
                .build(),
            FixtureMode::PageBomb => ServerCapabilities::builder().enable_prompts().build(),
            FixtureMode::CursorBomb | FixtureMode::ItemBomb | FixtureMode::ByteBomb => {
                ServerCapabilities::builder().enable_resources().build()
            }
        };
        ServerInfo::new(capabilities)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.counters.tools.fetch_add(1, Ordering::SeqCst);
        if self.mode != FixtureMode::Normal {
            return Err(Self::unsupported("tools/list"));
        }
        let generation = self.generation.load(Ordering::SeqCst);
        let mut tool: Tool = serde_json::from_value(serde_json::json!({
            "name": format!("fixture.echo_v{generation}"),
            "description": "Echo an exact proxy qualification payload",
            "inputSchema": {
                "type": "object",
                "properties": {"subject": {"type": "string"}, "correlation": {"type": "string"}},
                "required": ["subject", "correlation"]
            }
        }))
        .expect("independent upstream fixture descriptor");
        tool.annotations = Some(
            rmcp::model::ToolAnnotations::new()
                .read_only(true)
                .destructive(false)
                .idempotent(true),
        );
        let mut meta = rmcp::model::MetaObject::new();
        meta.0.insert(
            "ui".to_string(),
            serde_json::json!({"resourceUri":"ui://fixture/app.html"}),
        );
        tool.meta = Some(meta);
        let mut task: Tool = serde_json::from_value(serde_json::json!({
            "name": "fixture.task",
            "description": "Return a deterministic native MCP task",
            "inputSchema": {"type": "object"}
        }))
        .expect("independent upstream task descriptor");
        task.annotations = Some(
            rmcp::model::ToolAnnotations::new()
                .read_only(true)
                .destructive(false),
        );
        let mut progress: Tool = serde_json::from_value(serde_json::json!({
            "name": "fixture.progress",
            "description": "Emit progress until the downstream request is cancelled",
            "inputSchema": {"type": "object"}
        }))
        .expect("independent upstream progress descriptor");
        progress.annotations = Some(
            rmcp::model::ToolAnnotations::new()
                .read_only(true)
                .destructive(false),
        );
        Ok(ListToolsResult::with_all_items(vec![tool, task, progress]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if self.mode != FixtureMode::Normal {
            return Err(ErrorData::invalid_params("fixture tool not found", None));
        }
        self.counters.tool_calls.fetch_add(1, Ordering::SeqCst);
        if request.name.as_ref() == "fixture.task" {
            return Ok(CreateTaskResult::new(Task::new(
                "native-q1-task",
                TaskStatus::Working,
                "2026-09-13T00:00:00Z",
                "2026-09-13T00:00:01Z",
            ))
            .into());
        }
        if request.name.as_ref() == "fixture.progress" {
            let progress_token = context
                .meta
                .get_progress_token()
                .ok_or_else(|| ErrorData::invalid_params("progress token required", None))?;
            self.counters
                .progress_tokens
                .lock()
                .await
                .push(progress_token.clone());
            context
                .peer
                .notify_progress(
                    ProgressNotificationParam::new(progress_token.clone(), 0.25)
                        .with_message("fixture-quarter"),
                )
                .await
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
            context.ct.cancelled().await;
            self.counters
                .progress_cancellations
                .fetch_add(1, Ordering::SeqCst);
            drop(
                context
                    .peer
                    .notify_progress(
                        ProgressNotificationParam::new(progress_token, 1.0)
                            .with_message("forbidden-late-progress"),
                    )
                    .await,
            );
            return Err(ErrorData::internal_error("cancelled by downstream", None));
        }
        if !request.name.starts_with("fixture.echo_v") {
            return Err(ErrorData::invalid_params("fixture tool not found", None));
        }
        if request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("emit_list_changed"))
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            self.generation.fetch_add(1, Ordering::SeqCst);
            context
                .peer
                .notify_tool_list_changed()
                .await
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
            context
                .peer
                .notify_resource_list_changed()
                .await
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
            context
                .peer
                .notify_prompt_list_changed()
                .await
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::json!({
                "owner": self.owner.as_ref(),
                "arguments": request.arguments,
            })
            .to_string(),
        )])
        .into())
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        if request.task_id != "native-q1-task" {
            return Err(ErrorData::invalid_params("task not found", None));
        }
        let cancelled = self.counters.task_cancellations.load(Ordering::SeqCst) > 0;
        let status = if cancelled {
            TaskStatus::Cancelled
        } else {
            TaskStatus::Working
        };
        let payload = if cancelled {
            TaskPayload::Cancelled
        } else {
            TaskPayload::Working
        };
        Ok(GetTaskResult::new(DetailedTask::new(
            Task::new(
                request.task_id,
                status,
                "2026-09-13T00:00:00Z",
                "2026-09-13T00:00:02Z",
            )
            .with_status_message(if cancelled {
                "native-q1-cancelled"
            } else {
                "native-q1-working"
            }),
            payload,
        )))
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        if request.task_id != "native-q1-task" {
            return Err(ErrorData::invalid_params("task not found", None));
        }
        self.counters.task_updates.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        if request.task_id != "native-q1-task" {
            return Err(ErrorData::invalid_params("task not found", None));
        }
        self.counters
            .task_cancellations
            .fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.counters.resources.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            FixtureMode::Normal => Ok(ListResourcesResult::with_all_items(self.normal_resources())),
            FixtureMode::CursorBomb => {
                let mut result = ListResourcesResult::with_all_items(Vec::new());
                result.next_cursor = Some("c".repeat(8 * 1024 + 1));
                Ok(result)
            }
            FixtureMode::ItemBomb => Ok(ListResourcesResult::with_all_items(Vec::new())),
            FixtureMode::ByteBomb => Ok(ListResourcesResult::with_all_items(vec![
                Resource::new("fixture://byte-bomb", "byte-bomb")
                    .with_description("x".repeat(BYTE_BOMB_BYTES)),
            ])),
            FixtureMode::PageBomb => Err(Self::unsupported("resources/list")),
        }
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        self.counters.templates.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            FixtureMode::Normal => Ok(ListResourceTemplatesResult::with_all_items(
                self.normal_templates(),
            )),
            FixtureMode::ItemBomb => Ok(ListResourceTemplatesResult::with_all_items(
                (0..=1_000)
                    .map(|index| {
                        ResourceTemplate::new(
                            format!("fixture://item-bomb/{index}/{{value}}"),
                            format!("item-bomb-{index}"),
                        )
                    })
                    .collect(),
            )),
            FixtureMode::CursorBomb | FixtureMode::ByteBomb | FixtureMode::PageBomb => {
                Err(Self::unsupported("resources/templates/list"))
            }
        }
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if self.mode != FixtureMode::Normal {
            return Err(Self::unsupported("resources/read"));
        }
        let contents = match request.uri.as_str() {
            "fixture://text" => {
                ResourceContents::text(format!("{}:exact-text", self.owner), request.uri)
                    .with_mime_type("text/plain")
            }
            "fixture://blob" => ResourceContents::blob("AAEC/w==", request.uri)
                .with_mime_type("application/octet-stream"),
            "fixture://shared" => {
                ResourceContents::text(format!("{}:shared-owner", self.owner), request.uri)
            }
            "ui://fixture/app.html" => ResourceContents::text(
                format!("<!doctype html><title>{} fixture app</title>", self.owner),
                request.uri,
            )
            .with_mime_type("text/html;profile=mcp-app"),
            uri if uri.starts_with("fixture://dynamic-v")
                || uri.starts_with("fixture://template/") =>
            {
                ResourceContents::text(format!("{}:{uri}", self.owner), request.uri)
            }
            _ => {
                return Err(ErrorData::resource_not_found(
                    "fixture resource not found",
                    None,
                ));
            }
        };
        Ok(ReadResourceResult::new(vec![contents]).into())
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let call = self.counters.prompts.fetch_add(1, Ordering::SeqCst) + 1;
        match self.mode {
            FixtureMode::Normal => Ok(ListPromptsResult::with_all_items(self.normal_prompts())),
            FixtureMode::PageBomb => {
                if call > 1 {
                    let expected = format!("page-{}", call - 1);
                    if request.as_ref().and_then(|params| params.cursor.as_deref())
                        != Some(&expected)
                    {
                        return Err(ErrorData::invalid_params("page-bomb cursor mismatch", None));
                    }
                }
                let mut result = ListPromptsResult::with_all_items(Vec::new());
                result.next_cursor = Some(format!("page-{call}"));
                Ok(result)
            }
            FixtureMode::CursorBomb | FixtureMode::ItemBomb | FixtureMode::ByteBomb => {
                Err(Self::unsupported("prompts/list"))
            }
        }
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        self.counters.prompt_gets.fetch_add(1, Ordering::SeqCst);
        if self.mode != FixtureMode::Normal {
            return Err(Self::unsupported("prompts/get"));
        }
        let arguments = request.arguments.as_ref().ok_or_else(|| {
            ErrorData::invalid_params("fixture prompt requires exact arguments", None)
        })?;
        if arguments.len() != 1 || !arguments.contains_key("subject") {
            return Err(ErrorData::invalid_params(
                "fixture prompt requires exact arguments",
                None,
            ));
        }
        if request.name != "shared"
            && request.name != "oversized"
            && !request.name.starts_with("dynamic-v")
        {
            return Err(ErrorData::invalid_params("fixture prompt not found", None));
        }
        let subject = arguments
            .get("subject")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ErrorData::invalid_params("subject must be a string", None))?;
        if request.name == "oversized" {
            return Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                Role::User,
                "x".repeat(BYTE_BOMB_BYTES),
            )])
            .into());
        }
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            format!("{}:{}:{subject}", self.owner, request.name),
        )])
        .into())
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        self.counters.completions.fetch_add(1, Ordering::SeqCst);
        if self.mode != FixtureMode::Normal {
            return Err(Self::unsupported("completion/complete"));
        }
        let reference = request
            .r#ref
            .as_prompt_name()
            .or_else(|| request.r#ref.as_resource_uri())
            .ok_or_else(|| ErrorData::invalid_params("unsupported reference", None))?;
        let values = vec![format!(
            "{}:{reference}:{}:{}",
            self.owner, request.argument.name, request.argument.value
        )];
        Ok(CompleteResult::new(
            CompletionInfo::with_pagination(values, Some(1), false)
                .expect("one completion is valid"),
        ))
    }
}

pub(crate) struct PrimitiveFixture {
    owner: Arc<str>,
    generation: Arc<AtomicUsize>,
    counters: Arc<FixtureCounters>,
    address: std::net::SocketAddr,
    shutdown: CancellationToken,
    task: JoinHandle<()>,
}

impl PrimitiveFixture {
    pub(crate) async fn start(owner: &str, mode: FixtureMode) -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| error.to_string())?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        let owner: Arc<str> = Arc::from(owner);
        let generation = Arc::new(AtomicUsize::new(0));
        let counters = Arc::new(FixtureCounters::default());
        let server = FixtureServer {
            owner: Arc::clone(&owner),
            mode,
            generation: Arc::clone(&generation),
            counters: Arc::clone(&counters),
        };
        let service = StreamableHttpService::new(
            move || Ok(server.clone()),
            Arc::new(NeverSessionManager::default()),
            StreamableHttpServerConfig::default()
                .with_allowed_hosts(vec![address.to_string()])
                .with_json_response(true),
        );
        let shutdown = CancellationToken::new();
        let cancelled = shutdown.clone().cancelled_owned();
        let task = tokio::spawn(async move {
            drop(
                axum::serve(listener, Router::new().nest_service("/mcp", service))
                    .with_graceful_shutdown(cancelled)
                    .await,
            );
        });
        Ok(Self {
            owner,
            generation,
            counters,
            address,
            shutdown,
            task,
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.owner
    }

    pub(crate) fn url(&self) -> String {
        format!("http://{}/mcp", self.address)
    }

    pub(crate) fn set_generation(&self, generation: usize) {
        self.generation.store(generation, Ordering::SeqCst);
    }

    pub(crate) fn resource_lists(&self) -> usize {
        self.counters.resources.load(Ordering::SeqCst)
    }

    pub(crate) fn template_lists(&self) -> usize {
        self.counters.templates.load(Ordering::SeqCst)
    }

    pub(crate) fn prompt_lists(&self) -> usize {
        self.counters.prompts.load(Ordering::SeqCst)
    }

    pub(crate) fn prompt_gets(&self) -> usize {
        self.counters.prompt_gets.load(Ordering::SeqCst)
    }

    pub(crate) fn completions(&self) -> usize {
        self.counters.completions.load(Ordering::SeqCst)
    }

    pub(crate) fn tool_calls(&self) -> usize {
        self.counters.tool_calls.load(Ordering::SeqCst)
    }

    pub(crate) fn task_updates(&self) -> usize {
        self.counters.task_updates.load(Ordering::SeqCst)
    }

    pub(crate) fn task_cancellations(&self) -> usize {
        self.counters.task_cancellations.load(Ordering::SeqCst)
    }

    pub(crate) fn progress_cancellations(&self) -> usize {
        self.counters.progress_cancellations.load(Ordering::SeqCst)
    }

    pub(crate) async fn progress_tokens(&self) -> Vec<rmcp::model::ProgressToken> {
        self.counters.progress_tokens.lock().await.clone()
    }

    pub(crate) async fn finish(mut self) -> Result<(), String> {
        self.shutdown.cancel();
        if tokio::time::timeout(Duration::from_secs(2), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
            tokio::time::timeout(Duration::from_secs(2), &mut self.task)
                .await
                .map_err(|_| "fixture abort did not settle".to_string())?
                .ok();
        }
        Ok(())
    }
}

pub(crate) struct PrimitiveQualification {
    guard: Option<LiveLabbyGuard>,
    service: Option<RunningService<RoleClient, PrimitiveClient>>,
    client: PrimitiveClient,
}

impl PrimitiveQualification {
    pub(crate) async fn start(fixtures: &[&PrimitiveFixture]) -> Result<Self, String> {
        drop(rustls::crypto::ring::default_provider().install_default());
        let mut config = String::from(
            "upstream_request_timeout_ms = 15000\nupstream_relay_timeout_ms = 15000\n\n",
        );
        for fixture in fixtures {
            config.push_str(&format!(
                "[[upstream]]\nname = {}\nenabled = true\nurl = {}\nproxy_resources = true\nproxy_prompts = true\n\n",
                serde_json::to_string(fixture.name()).map_err(|error| error.to_string())?,
                serde_json::to_string(&fixture.url()).map_err(|error| error.to_string())?,
            ));
        }
        let guard = LiveLabbyBuilder::new()
            .env("LABBY_MCP_HTTP_TOKEN", TOKEN)
            .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
            .config(config)
            .start()
            .await?;
        let config_path = guard.root().join("labby-home/config.toml");
        let persisted = std::fs::read_to_string(&config_path).map_err(|error| error.to_string())?;
        let changed = persisted.replacen(
            "upstream_request_timeout_ms = 15000",
            "upstream_request_timeout_ms = 15001",
            1,
        );
        if changed == persisted {
            return Err("Q1 fixture could not force the first full gateway reload".to_string());
        }
        std::fs::write(config_path, changed).map_err(|error| error.to_string())?;
        let endpoint = format!("{}/mcp", guard.connection().base_url);
        let mut transport = StreamableHttpClientTransportConfig::with_uri(endpoint);
        transport.auth_header = Some(TOKEN.to_string());
        let worker = StreamableHttpClientWorker::new(
            BodyCappedHttpClient::new(reqwest::Client::new(), CLIENT_RESPONSE_CAP),
            transport,
        );
        let client = PrimitiveClient::default();
        let service = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client.clone().serve_with_lifecycle(
                worker,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            ),
        )
        .await
        .map_err(|_| "Q1 MCP discovery timed out".to_string())?
        .map_err(|error| error.to_string())?;
        let reload = CallToolRequestParams::new("gateway").with_arguments(
            serde_json::json!({"action":"gateway.reload","params":{}})
                .as_object()
                .expect("reload arguments")
                .clone(),
        );
        let result = tokio::time::timeout(REQUEST_TIMEOUT, service.call_tool(reload))
            .await
            .map_err(|_| "Q1 gateway reload timed out".to_string())?
            .map_err(|error| error.to_string())?;
        if result.is_error == Some(true) {
            return Err(format!("Q1 gateway reload failed: {:?}", result.content));
        }
        Ok(Self {
            guard: Some(guard),
            service: Some(service),
            client,
        })
    }

    pub(crate) fn service(&self) -> &RunningService<RoleClient, PrimitiveClient> {
        self.service.as_ref().expect("active Q1 client")
    }

    pub(crate) fn client(&self) -> &PrimitiveClient {
        &self.client
    }

    pub(crate) fn diagnostic_log_tail(&self) -> String {
        let root = self.guard.as_ref().expect("active Q1 guard").root();
        [root.join("stderr.log"), root.join("stdout.log")]
            .into_iter()
            .filter_map(|path| std::fs::read_to_string(path).ok())
            .map(|text| {
                text.chars()
                    .rev()
                    .take(16_384)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect()
            })
            .collect::<Vec<String>>()
            .join("\n")
    }

    pub(crate) async fn finish(mut self) -> CleanupResult {
        let mut cleanup = CleanupResult::default();
        if let Some(service) = self.service.take() {
            match tokio::time::timeout(REQUEST_TIMEOUT, service.cancel()).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => cleanup
                    .failures
                    .push(format!("Q1 client cancellation failed: {error}")),
                Err(_) => cleanup
                    .failures
                    .push("Q1 client cancellation timed out".to_string()),
            }
        }
        if let Some(guard) = self.guard.take() {
            let owned = guard.finish().await;
            cleanup.failures.extend(owned.failures);
        }
        cleanup
    }
}
