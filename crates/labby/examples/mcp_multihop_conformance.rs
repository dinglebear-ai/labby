//! Labby-native MCP multi-hop conformance driver.
//!
//! The driver launches this process as a synthetic leaf behind two real Labby
//! stdio gateways:
//!
//! client -> root Labby -> middle Labby -> synthetic leaf

// The conformance client drives labby itself, not an untrusted upstream,
// and deliberately exercises the raw rmcp helpers end to end.
#![allow(clippy::disallowed_methods)]
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, ensure};
use labby::config::{GatewayPreferences, LabConfig, UpstreamConfig};
use rmcp::model::{
    ArgumentInfo, CallToolRequest, CallToolRequestParams, CallToolResponse, CallToolResult,
    CancelTaskParams, ClientCapabilities, ClientInfo, ClientRequest, CompleteRequestParams,
    CompleteResult, CompletionInfo, ContentBlock, CreateTaskResult, DetailedTask, ElicitRequest,
    ElicitRequestParams, ElicitationSchema, ErrorData, GetPromptRequestParams, GetPromptResponse,
    GetPromptResult, GetTaskParams, GetTaskResult, Implementation, InputRequest, InputRequests,
    InputRequiredResult, InputResponses, JsonRpcMessage, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, MetaObject,
    PaginatedRequestParams, PrimitiveSchemaDefinition, ProgressNotificationParam, Prompt,
    PromptMessage, ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, Reference, Resource, ResourceContents, ResourceTemplate, Role,
    ServerCapabilities, ServerInfo, ServerNotification, ServerResult, SubscriptionFilter, Task,
    TaskPayload, TaskStatus, TaskStatusNotification, TaskStatusNotificationParams, Tool,
    UpdateTaskParams,
};
use rmcp::service::{
    ClientLifecycleMode, ClientServiceExt, NotificationContext, Peer, PeerRequestOptions,
    RequestContext, RxJsonRpcMessage, SubscriptionContext, TxJsonRpcMessage,
};
use rmcp::transport::{
    ConfigureCommandExt, TokioChildProcess, Transport, TransportAdapterIdentity,
};
use rmcp::{ClientHandler, RoleClient, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};
use tempfile::TempDir;
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock};

const TOOL_COUNT: usize = 75;
const PROMPT_COUNT: usize = 70;
const RESOURCE_COUNT: usize = 70;
const TEMPLATE_COUNT: usize = 70;
const SERVER_INFO_KEY: &str = "io.modelcontextprotocol/serverInfo";
const UPSTREAM_SERVER_INFO_KEY: &str = "ai.dinglebear.labby/upstreamServerInfo";
const NATIVE_RESOURCE_URI: &str = "fixture://resource/069";
const TASK_CREATED_AT: &str = "2026-08-01T00:00:00Z";

#[derive(Clone, Default)]
struct LeafServer {
    tasks: Arc<RwLock<BTreeMap<String, DetailedTask>>>,
    next_task_id: Arc<AtomicUsize>,
}

impl LeafServer {
    fn marker_path(name: &str) -> Option<PathBuf> {
        std::env::var_os("MULTIHOP_MARKER_DIR")
            .map(PathBuf::from)
            .map(|dir| dir.join(name))
    }

    fn subscription_catalog_changed() -> bool {
        Self::marker_path("emit-subscriptions").is_some_and(|path| path.exists())
    }

    fn task(task_id: impl Into<String>, payload: TaskPayload, updated_at: &str) -> DetailedTask {
        DetailedTask::new(
            Task::new(task_id, payload.status(), TASK_CREATED_AT, updated_at)
                .with_poll_interval_ms(10),
            payload,
        )
    }

    async fn publish_task(
        peer: Peer<RoleServer>,
        task: DetailedTask,
    ) -> Result<(), rmcp::service::ServiceError> {
        peer.send_notification(ServerNotification::TaskStatusNotification(
            TaskStatusNotification::new(TaskStatusNotificationParams::new(task)),
        ))
        .await
    }
}

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
                .enable_tool_list_changed()
                .enable_resources()
                .enable_resources_list_changed()
                .enable_resources_subscribe()
                .enable_prompts()
                .enable_prompts_list_changed()
                .enable_completions()
                .enable_tasks()
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
        tools.extend([
            Tool::new(
                "needs_input",
                "Return a first-class MRTR input_required result",
                Arc::new(Map::new()),
            ),
            Tool::new(
                "task_lifecycle",
                "Create a task that can be polled, updated, cancelled, and observed",
                Arc::new(Map::new()),
            ),
            Tool::new(
                "progress",
                "Emit request-scoped progress notifications",
                Arc::new(Map::new()),
            ),
            Tool::new(
                "cancellable",
                "Wait until downstream cancellation reaches the leaf",
                Arc::new(Map::new()),
            ),
        ]);
        if Self::subscription_catalog_changed() {
            tools.push(Tool::new(
                "subscription_added",
                "Tool added when the subscription conformance signal fires",
                Arc::new(Map::new()),
            ));
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        match request.name.as_ref() {
            "needs_input" => Ok(input_required().into()),
            "task_lifecycle" => {
                let sequence = self.next_task_id.fetch_add(1, Ordering::SeqCst) + 1;
                let task_id = format!("leaf-task-{sequence}");
                let task = Self::task(&task_id, TaskPayload::Working, TASK_CREATED_AT);
                self.tasks
                    .write()
                    .await
                    .insert(task_id.clone(), task.clone());

                Self::publish_task(context.peer.clone(), task)
                    .await
                    .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;

                Ok(CreateTaskResult::new(
                    Task::new(
                        &task_id,
                        TaskStatus::Working,
                        TASK_CREATED_AT,
                        TASK_CREATED_AT,
                    )
                    .with_poll_interval_ms(10),
                )
                .with_meta(leaf_meta())
                .into())
            }
            "progress" => {
                let token = context.meta.get_progress_token().ok_or_else(|| {
                    ErrorData::invalid_params("progress tool requires a progress token", None)
                })?;
                context
                    .peer
                    .notify_progress(
                        ProgressNotificationParam::new(token.clone(), 0.25)
                            .with_total(1.0)
                            .with_message("quarter"),
                    )
                    .await
                    .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                context
                    .peer
                    .notify_progress(
                        ProgressNotificationParam::new(token, 0.75)
                            .with_total(1.0)
                            .with_message("three-quarters"),
                    )
                    .await
                    .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                if let Some(path) = Self::marker_path("progress-emitted") {
                    std::fs::write(path, b"emitted")
                        .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                }
                Ok(CallToolResult::success(vec![ContentBlock::text("progress-complete")]).into())
            }
            "cancellable" => {
                if let Some(path) = Self::marker_path("cancellation-started") {
                    std::fs::write(path, b"started")
                        .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                }
                context.ct.cancelled().await;
                if let Some(path) = Self::marker_path("cancellation-observed") {
                    std::fs::write(path, b"cancelled")
                        .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                }
                Err(ErrorData::internal_error("cancelled by downstream", None))
            }
            _ => {
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
        }
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        let task = self
            .tasks
            .read()
            .await
            .get(&request.task_id)
            .cloned()
            .ok_or_else(|| ErrorData::invalid_params("task not found", None))?;
        Ok(GetTaskResult::new(task))
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let completed = Self::task(
            &request.task_id,
            TaskPayload::Completed {
                result: Map::from_iter([("updated".to_string(), json!(true))]),
            },
            "2026-08-01T00:00:02Z",
        );
        let mut tasks = self.tasks.write().await;
        if !tasks.contains_key(&request.task_id) {
            return Err(ErrorData::invalid_params("task not found", None));
        }
        tasks.insert(request.task_id.clone(), completed.clone());
        drop(tasks);
        Self::publish_task(context.peer, completed)
            .await
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
        if let Some(path) = Self::marker_path("task-update-notification-emitted") {
            std::fs::write(path, b"emitted")
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
        }
        Ok(())
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let cancelled = Self::task(
            &request.task_id,
            TaskPayload::Cancelled,
            "2026-08-01T00:00:03Z",
        );
        let mut tasks = self.tasks.write().await;
        if !tasks.contains_key(&request.task_id) {
            return Err(ErrorData::invalid_params("task not found", None));
        }
        tasks.insert(request.task_id.clone(), cancelled.clone());
        drop(tasks);
        Self::publish_task(context.peer, cancelled)
            .await
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
        Ok(())
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(requested.clone())
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        let sink = context.sink().clone();
        let trigger = Self::marker_path("emit-subscriptions");
        let mut emitted = false;
        loop {
            tokio::select! {
                _ = context.cancelled() => break,
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                    if !emitted && trigger.as_ref().is_some_and(|path| path.exists()) {
                        sink.notify_tool_list_changed().await
                            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                        sink.notify_prompt_list_changed().await
                            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                        sink.notify_resource_list_changed().await
                            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                        sink.notify_resource_updated(NATIVE_RESOURCE_URI).await
                            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                        emitted = true;
                    }
                }
            }
        }
        Ok(())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let mut prompts = (0..PROMPT_COUNT)
            .map(|index| Prompt::new(format!("prompt_{index:03}"), Some("Multi-hop prompt"), None))
            .collect::<Vec<_>>();
        if Self::subscription_catalog_changed() {
            prompts.push(Prompt::new(
                "subscription_prompt",
                Some("Appears after the subscription trigger"),
                None,
            ));
        }
        Ok(ListPromptsResult::with_all_items(prompts))
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
        let mut resources = (0..RESOURCE_COUNT)
            .map(|index| {
                Resource::new(
                    format!("fixture://resource/{index:03}"),
                    format!("resource_{index:03}"),
                )
            })
            .collect::<Vec<_>>();
        if Self::subscription_catalog_changed() {
            resources.push(Resource::new(
                "fixture://resource/subscription",
                "subscription_resource",
            ));
        }
        Ok(ListResourcesResult::with_all_items(resources))
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

struct DriverProgressObserver<T> {
    inner: T,
    progress: Arc<Mutex<Vec<ProgressNotificationParam>>>,
}

impl<T> DriverProgressObserver<T> {
    fn new(inner: T, progress: Arc<Mutex<Vec<ProgressNotificationParam>>>) -> Self {
        Self { inner, progress }
    }
}

impl<T> Transport<RoleClient> for DriverProgressObserver<T>
where
    T: Transport<RoleClient>,
{
    type Error = T::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        let message = self.inner.receive().await?;
        if let JsonRpcMessage::Notification(notification) = &message
            && let ServerNotification::ProgressNotification(progress) = &notification.notification
        {
            self.progress.lock().await.push(progress.params.clone());
        }
        Some(message)
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

#[derive(Default)]
struct DriverEvents {
    progress: Mutex<Vec<ProgressNotificationParam>>,
    tasks: Mutex<Vec<TaskStatusNotificationParams>>,
}

#[derive(Clone, Default)]
struct DriverClient {
    events: Arc<DriverEvents>,
}

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

    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.events.progress.lock().await.push(params);
    }

    async fn on_task_status(
        &self,
        params: TaskStatusNotificationParams,
        _context: NotificationContext<RoleClient>,
    ) {
        self.events.tasks.lock().await.push(params);
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
        proxy_skills: false,
        expose_skills: None,
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
        proxy_skills: false,
        expose_skills: None,
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
    let config = LabConfig {
        gateway: GatewayPreferences {
            disable_spawn_guard: true,
            upstream_stderr_level: Some("warn".to_string()),
            ..GatewayPreferences::default()
        },
        upstream: vec![upstream],
        ..LabConfig::default()
    };
    std::fs::write(path, toml::to_string(&config)?)?;
    Ok(())
}

fn force_full_reload_on_next_request(home: &Path, timeout_ms: u64) -> Result<()> {
    let path = config_path(home);
    let raw = std::fs::read_to_string(&path)?;
    let mut config: LabConfig = toml::from_str(&raw)?;
    config.upstream_request_timeout_ms = Some(timeout_ms);
    config.upstream_relay_timeout_ms = Some(timeout_ms);
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
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            if client
                .get(format!("{base_url}/ready"))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .with_context(|| format!("middle Labby HTTP daemon did not become ready at {base_url}"))?;
    Ok(())
}

async fn wait_for_nested_tool_catalog(peer: &Peer<RoleClient>) -> Result<Vec<Tool>> {
    let mut last_leaf_tools = 0;
    let mut last_observed = Vec::<String>::new();
    let ready = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let tools = peer.list_all_tools().await?;
            last_leaf_tools = tools
                .iter()
                .filter(|tool| tool.name.ends_with("needs_input") || tool.name.contains("echo_"))
                .count();
            last_observed = tools
                .iter()
                .map(|tool| tool.name.to_string())
                .take(30)
                .collect();
            if last_leaf_tools > TOOL_COUNT {
                return Ok::<_, rmcp::service::ServiceError>(tools);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await;

    match ready {
        Ok(Ok(tools)) => Ok(tools),
        Ok(Err(error)) => Err(error.into()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "timed out waiting for nested leaf tool catalog; got {last_leaf_tools} leaf tools; observed {last_observed:?}"
            )
        }),
    }
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

async fn wait_for_marker(path: &Path) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !path.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .with_context(|| format!("timed out waiting for marker {}", path.display()))?;
    Ok(())
}

async fn verify_nested_timeout(
    peer: &Peer<RoleClient>,
    root_home: &Path,
    cancellable_name: &str,
    cancellation_started: &Path,
    cancellation_observed: &Path,
) -> Result<()> {
    std::fs::remove_file(cancellation_started).ok();
    std::fs::remove_file(cancellation_observed).ok();
    force_full_reload_on_next_request(root_home, 500)?;
    call_service_action(peer, "gateway", "gateway.reload").await?;
    let timeout_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        peer.call_tool_once(CallToolRequestParams::new(cancellable_name.to_owned())),
    )
    .await
    .context("nested timeout request exceeded terminal bound")??;
    let CallToolResponse::Complete(timeout_result) = timeout_result else {
        anyhow::bail!("nested timeout did not return a complete error");
    };
    ensure!(timeout_result.is_error == Some(true));
    let timeout_text = timeout_result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|text| text.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    ensure!(timeout_text.contains("\"kind\":\"timeout\""));
    wait_for_marker(cancellation_started).await?;
    wait_for_marker(cancellation_observed).await?;
    force_full_reload_on_next_request(root_home, 30_003)?;
    call_service_action(peer, "gateway", "gateway.reload").await?;
    Ok(())
}

async fn wait_for_progress(
    events: &DriverEvents,
    count: usize,
) -> Result<Vec<ProgressNotificationParam>> {
    match tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let values = events.progress.lock().await.clone();
            if values.len() >= count {
                break values;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    {
        Ok(values) => Ok(values),
        Err(_) => {
            let values = events.progress.lock().await.clone();
            let received = values
                .iter()
                .map(|value| {
                    format!(
                        "{:?}:{}",
                        value.progress_token,
                        value.message.as_deref().unwrap_or("<no-message>")
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "timed out waiting for {count} progress notifications; received {} [{}]",
                values.len(),
                received
            )
        }
    }
}

async fn wait_for_task_status(
    events: &DriverEvents,
    task_id: &str,
    status: TaskStatus,
) -> Result<TaskStatusNotificationParams> {
    match tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(value) = events
                .tasks
                .lock()
                .await
                .iter()
                .find(|value| value.task.task.task_id == task_id && value.task.status() == status)
                .cloned()
            {
                break value;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    {
        Ok(value) => Ok(value),
        Err(error) => {
            let observed = events
                .tasks
                .lock()
                .await
                .iter()
                .map(|value| (value.task.task.task_id.clone(), value.task.status()))
                .collect::<Vec<_>>();
            Err(error).with_context(|| {
                format!(
                    "timed out waiting for {status:?} task notification for {task_id}; observed {observed:?}"
                )
            })
        }
    }
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
    let marker_dir = temp.path().join("markers");
    let child_cwd = temp.path().join("cwd");
    std::fs::create_dir_all(&root_home)?;
    std::fs::create_dir_all(&middle_home)?;
    std::fs::create_dir_all(&marker_dir)?;
    std::fs::create_dir_all(&child_cwd)?;

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
            BTreeMap::from([(
                "MULTIHOP_MARKER_DIR".to_string(),
                marker_dir.display().to_string(),
            )]),
        ),
    )?;

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let middle_port = listener.local_addr()?.port();
    drop(listener);
    let middle_base_url = format!("http://127.0.0.1:{middle_port}");
    let middle_token = "8c1f97449584ebcc6025655d738a8b40a3a488dd407ac89a1c42146864bd0179";
    let mut middle_child = Command::new(&labby_bin)
        // Labby resolves ./config.toml before HOME-scoped config. Pin the
        // fixture cwd so an unrelated caller cwd cannot shadow this test config.
        .current_dir(&middle_home)
        .arg("serve")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(middle_port.to_string())
        .env("HOME", &middle_home)
        .env("LABBY_HOME", middle_home.join(".config/labby"))
        .env("LABBY_AUTH_MODE", "bearer")
        .env("LABBY_MCP_HTTP_TOKEN", middle_token)
        .env("LABBY_CODE_MODE_JOURNAL_DISABLED", "1")
        .env("LABBY_GATEWAY_USAGE_DISABLED", "1")
        .env("LABBY_LOG", "labby=debug,labby_gateway=debug")
        // Labby intentionally resolves ./config.toml before HOME-scoped config.
        // Use an empty controlled cwd so an ambient developer/CI config cannot
        // shadow the fixture configs written under middle_home.
        .current_dir(&child_cwd)
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
    force_full_reload_on_next_request(&root_home, 500)?;

    let transport = TokioChildProcess::new(Command::new(&labby_bin).configure(|command| {
        command
            // Keep the root fixture hermetic for the same reason as middle.
            .current_dir(&root_home)
            .arg("serve")
            .arg("mcp")
            .arg("--stdio")
            .env("HOME", &root_home)
            .env("LABBY_HOME", root_home.join(".config/labby"))
            .env("LABBY_CODE_MODE_JOURNAL_DISABLED", "1")
            .env("LABBY_GATEWAY_USAGE_DISABLED", "1")
            .env("MULTIHOP_MIDDLE_TOKEN", middle_token)
            .env("LABBY_LOG", "labby=debug,labby_gateway=debug")
            // Same isolation as the middle daemon: never let a caller's cwd
            // config.toml override the generated root fixture config.
            .current_dir(&child_cwd);
    }))?;
    let wire_progress = Arc::new(Mutex::new(Vec::new()));
    let transport = DriverProgressObserver::new(transport, Arc::clone(&wire_progress));
    let driver = DriverClient::default();
    let events = Arc::clone(&driver.events);
    let service = driver
        .serve_with_lifecycle::<_, _, TransportAdapterIdentity>(
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
    ensure!(
        peer_info.capabilities.supports_tasks(),
        "root discovery must advertise tasks"
    );
    ensure!(
        peer_info
            .capabilities
            .resources
            .as_ref()
            .is_some_and(|resources| resources.subscribe == Some(true)),
        "root discovery must advertise resource subscriptions"
    );

    // Raw catalog listing intentionally never cold-connects upstreams. Middle
    // was warmed through its authenticated operator API above; now force a full
    // root reconcile so its serving pool discovers middle's complete catalog.
    force_full_reload_on_next_request(&root_home, 30_003)?;
    call_service_action(&peer, "gateway", "gateway.reload").await?;

    let tools = wait_for_nested_tool_catalog(&peer).await?;
    let echo_name = nested_name(&tools, |tool| tool.name.as_ref(), "echo_074")?.to_string();
    let needs_input_name =
        nested_name(&tools, |tool| tool.name.as_ref(), "needs_input")?.to_string();
    let task_name = nested_name(&tools, |tool| tool.name.as_ref(), "task_lifecycle")?.to_string();
    let progress_name = nested_name(&tools, |tool| tool.name.as_ref(), "progress")?.to_string();
    let cancellable_name =
        nested_name(&tools, |tool| tool.name.as_ref(), "cancellable")?.to_string();

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

    let progress_request = ClientRequest::CallToolRequest(CallToolRequest::new(
        CallToolRequestParams::new(progress_name),
    ));
    let progress_handle = peer
        .send_cancellable_request(progress_request, PeerRequestOptions::no_options())
        .await?;
    let progress_token = progress_handle.progress_token.clone();
    let ServerResult::CallToolResult(progress_result) = progress_handle.await_response().await?
    else {
        anyhow::bail!("nested progress tool did not complete");
    };
    ensure!(progress_result.is_error != Some(true));
    wait_for_marker(&marker_dir.join("progress-emitted")).await?;
    let progress = wait_for_progress(events.as_ref(), 2).await?;
    ensure!(
        progress
            .iter()
            .all(|value| value.progress_token == progress_token)
    );
    ensure!(progress.len() == 2);
    ensure!(
        progress
            .iter()
            .any(|value| value.message.as_deref() == Some("quarter"))
    );
    ensure!(
        progress
            .iter()
            .any(|value| value.message.as_deref() == Some("three-quarters"))
    );
    let wire_progress = wire_progress
        .lock()
        .await
        .iter()
        .filter(|value| value.progress_token == progress_token)
        .cloned()
        .collect::<Vec<_>>();
    ensure!(wire_progress.len() == 2);
    ensure!(wire_progress[0].message.as_deref() == Some("quarter"));
    ensure!(wire_progress[1].message.as_deref() == Some("three-quarters"));

    let CallToolResponse::Task(created) = peer
        .call_tool_once(CallToolRequestParams::new(task_name.clone()))
        .await?
    else {
        anyhow::bail!("nested task tool did not return a task");
    };
    let gateway_task_id = created.task.task_id.clone();
    ensure!(!gateway_task_id.starts_with("leaf-task-"));
    wait_for_task_status(events.as_ref(), &gateway_task_id, TaskStatus::Working).await?;
    let working = peer.get_task(GetTaskParams::new(&gateway_task_id)).await?;
    ensure!(working.task.task.task_id == gateway_task_id);
    ensure!(working.task.status() == TaskStatus::Working);

    peer.update_task(UpdateTaskParams::new(
        &gateway_task_id,
        InputResponses::new(),
    ))
    .await?;
    wait_for_marker(&marker_dir.join("task-update-notification-emitted")).await?;
    wait_for_task_status(events.as_ref(), &gateway_task_id, TaskStatus::Completed).await?;
    let completed = peer.get_task(GetTaskParams::new(&gateway_task_id)).await?;
    ensure!(completed.task.task.task_id == gateway_task_id);
    ensure!(completed.task.status() == TaskStatus::Completed);

    let CallToolResponse::Task(cancel_created) = peer
        .call_tool_once(CallToolRequestParams::new(task_name))
        .await?
    else {
        anyhow::bail!("second nested task tool did not return a task");
    };
    let cancelled_task_id = cancel_created.task.task_id.clone();
    peer.cancel_task(CancelTaskParams::new(&cancelled_task_id))
        .await?;
    wait_for_task_status(events.as_ref(), &cancelled_task_id, TaskStatus::Cancelled).await?;
    let cancelled = peer
        .get_task(GetTaskParams::new(&cancelled_task_id))
        .await?;
    ensure!(cancelled.task.task.task_id == cancelled_task_id);
    ensure!(cancelled.task.status() == TaskStatus::Cancelled);

    let cancellation_started = marker_dir.join("cancellation-started");
    let cancellation_observed = marker_dir.join("cancellation-observed");
    let cancellation_request = ClientRequest::CallToolRequest(CallToolRequest::new(
        CallToolRequestParams::new(cancellable_name.clone()),
    ));
    let cancellation_handle = peer
        .send_cancellable_request(cancellation_request, PeerRequestOptions::no_options())
        .await?;
    wait_for_marker(&cancellation_started).await?;
    cancellation_handle
        .cancel(Some("multi-hop cancellation check".to_string()))
        .await?;
    wait_for_marker(&cancellation_observed).await?;

    verify_nested_timeout(
        &peer,
        &root_home,
        &cancellable_name,
        &cancellation_started,
        &cancellation_observed,
    )
    .await?;

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
        .read_resource_once(ReadResourceRequestParams::new(resource_uri.clone()))
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

    let requested_notifications = SubscriptionFilter::builder()
        .tools_list_changed()
        .prompts_list_changed()
        .resources_list_changed()
        .resource_subscription(resource_uri.clone())
        .build();
    let mut subscription = peer.listen(requested_notifications).await?;
    ensure!(subscription.acknowledged().tools_list_changed == Some(true));
    ensure!(subscription.acknowledged().prompts_list_changed == Some(true));
    ensure!(subscription.acknowledged().resources_list_changed == Some(true));
    ensure!(
        subscription
            .acknowledged()
            .resource_subscriptions
            .as_ref()
            .is_some_and(|uris| uris == &[resource_uri.clone()])
    );

    std::fs::write(marker_dir.join("emit-subscriptions"), b"emit")?;
    let mut tool_changed = false;
    let mut prompt_changed = false;
    let mut resource_list_changed = false;
    let mut resource_updated = false;
    let mut observed = BTreeSet::new();
    let mut observed_resource_uris = BTreeSet::new();
    let wait_result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !(tool_changed && prompt_changed && resource_list_changed && resource_updated) {
            let Some(notification) = subscription.next().await? else {
                anyhow::bail!("subscription ended before all notifications arrived");
            };
            match notification {
                ServerNotification::ToolListChangedNotification(_) => {
                    observed.insert("tools/list_changed");
                    tool_changed = true;
                }
                ServerNotification::PromptListChangedNotification(_) => {
                    observed.insert("prompts/list_changed");
                    prompt_changed = true;
                }
                ServerNotification::ResourceListChangedNotification(_) => {
                    observed.insert("resources/list_changed");
                    resource_list_changed = true;
                }
                ServerNotification::ResourceUpdatedNotification(notification) => {
                    observed.insert("resources/updated");
                    observed_resource_uris.insert(notification.params.uri.clone());
                    if notification.params.uri == resource_uri {
                        resource_updated = true;
                    }
                }
                _ => {}
            }
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;
    let expected = BTreeSet::from([
        "tools/list_changed",
        "prompts/list_changed",
        "resources/list_changed",
        "resources/updated",
    ]);
    let missing = expected.difference(&observed).copied().collect::<Vec<_>>();
    let diagnostics = format!(
        "observed={observed:?}; missing={missing:?}; expected_resource_uri={resource_uri:?}; observed_resource_uris={observed_resource_uris:?}"
    );
    match wait_result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(error).context(diagnostics),
        Err(error) => {
            return Err(anyhow::Error::new(error)).context(format!(
                "timed out waiting for multi-hop subscription notifications; {diagnostics}"
            ));
        }
    }

    let subscription_tools = peer.list_all_tools().await?;
    nested_name(
        &subscription_tools,
        |tool| tool.name.as_ref(),
        "subscription_added",
    )?;
    let subscription_prompts = peer.list_all_prompts().await?;
    nested_name(
        &subscription_prompts,
        |prompt| prompt.name.as_str(),
        "subscription_prompt",
    )?;
    let subscription_resources = peer.list_all_resources().await?;
    nested_name(
        &subscription_resources,
        |resource| resource.name.as_str(),
        "subscription_resource",
    )?;
    subscription.cancel().await?;

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
            LeafServer::default()
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
