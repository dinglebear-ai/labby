//! Container-local Codex App Server adapter for the Phoenix assistant.

use std::{collections::HashMap, ffi::OsString, sync::Arc, time::Duration};

use base64::Engine as _;
use labby_primitives::action::{ActionSpec, ParamSpec};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{
    config::PhoenixPreferences,
    dispatch::{
        error::ToolError,
        phoenix_runtime::{AppServerRuntime, LaunchSpec},
    },
};

const TURN_TIMEOUT: Duration = Duration::from_mins(5);
const MAX_INPUT_BYTES: usize = 32 * 1024;
const MAX_ATTACHMENTS: usize = 4;
const MAX_ATTACHMENT_BYTES: usize = 5 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_MESSAGES: usize = 100;
const MAX_EVENTS: usize = 500;
const MAX_SESSIONS: usize = 32;
const APP_SERVER_PROTOCOL_SCHEMA: &str = "v2";
const LOCAL_MCP_URL: &str = "http://127.0.0.1:8765/mcp";
const LOCAL_MCP_TOKEN_ENV: &str = "LABBY_MCP_HTTP_TOKEN";

const fn param(name: &'static str, required: bool) -> ParamSpec {
    ParamSpec {
        name,
        ty: "string",
        required,
        description: "",
    }
}

const fn action(
    name: &'static str,
    description: &'static str,
    params: &'static [ParamSpec],
) -> ActionSpec {
    ActionSpec {
        name,
        description,
        destructive: false,
        requires_admin: false,
        params,
        returns: "object",
    }
}

pub(crate) const ACTIONS: &[ActionSpec] = &[
    action("phoenix.status", "Read Phoenix availability", &[]),
    action("phoenix.models.list", "List selectable Codex models", &[]),
    action(
        "phoenix.session.start",
        "Start a caller-scoped Phoenix session",
        &[param("model", false), param("effort", false)],
    ),
    action(
        "phoenix.session.read",
        "Read a caller-scoped Phoenix session",
        &[param("session_id", true)],
    ),
    action(
        "phoenix.session.close",
        "Close a caller-scoped Phoenix session",
        &[param("session_id", true)],
    ),
    action(
        "phoenix.turn.send",
        "Send a message to a caller-scoped Phoenix session",
        &[
            param("session_id", true),
            param("input", true),
            param("attachments", false),
        ],
    ),
    action(
        "phoenix.turn.interrupt",
        "Interrupt the active turn in a caller-scoped Phoenix session",
        &[param("session_id", true)],
    ),
    action(
        "phoenix.turn.steer",
        "Add caller input to the active Phoenix turn",
        &[
            param("session_id", true),
            param("input", true),
            param("attachments", false),
        ],
    ),
    action(
        "phoenix.review.start",
        "Start a read-only review in a caller-scoped Phoenix session",
        &[
            param("session_id", true),
            param("target_type", true),
            param("target", false),
        ],
    ),
    action(
        "phoenix.diagnostics.read",
        "Read safe container-local Codex diagnostics",
        &[],
    ),
];

#[derive(Clone, Debug)]
struct Message {
    role: &'static str,
    text: String,
}

#[derive(Clone)]
struct Session {
    owner: String,
    thread_id: String,
    turn_in_progress: bool,
    active_turn_id: Option<String>,
    runtime: AppServerRuntime,
    messages: Vec<Message>,
    events: Vec<Value>,
    model: Option<String>,
    effort: Option<String>,
}

#[derive(Clone)]
pub(crate) struct PhoenixRuntime {
    config: PhoenixPreferences,
    sessions: Arc<Mutex<HashMap<String, Arc<Mutex<Session>>>>>,
}

impl PhoenixRuntime {
    #[must_use]
    pub(crate) fn new(config: PhoenixPreferences) -> Self {
        Self {
            config,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) async fn dispatch(
        &self,
        owner: &str,
        name: &str,
        params: Value,
    ) -> Result<Value, ToolError> {
        if name == "help" {
            return Ok(crate::dispatch::helpers::help_payload("phoenix", ACTIONS));
        }
        if name == "schema" {
            let action = required(&params, "action")?;
            return crate::dispatch::helpers::action_schema(ACTIONS, &action);
        }
        match name {
            "phoenix.status" => Ok(self.status()),
            "phoenix.models.list" => self.models().await,
            "phoenix.session.start" => {
                self.start(
                    owner,
                    optional(&params, "model"),
                    optional(&params, "effort"),
                )
                .await
            }
            "phoenix.session.read" => self.read(owner, &required(&params, "session_id")?).await,
            "phoenix.session.close" => self.close(owner, &required(&params, "session_id")?).await,
            "phoenix.turn.send" => {
                let session_id = required(&params, "session_id")?;
                let input = required(&params, "input")?;
                self.send(owner, &session_id, &input, params.get("attachments"))
                    .await
            }
            "phoenix.turn.interrupt" => {
                self.interrupt(owner, &required(&params, "session_id")?)
                    .await
            }
            "phoenix.turn.steer" => {
                let session_id = required(&params, "session_id")?;
                let input = required(&params, "input")?;
                self.steer(owner, &session_id, &input, params.get("attachments"))
                    .await
            }
            "phoenix.review.start" => {
                self.review(
                    owner,
                    &required(&params, "session_id")?,
                    &required(&params, "target_type")?,
                    optional(&params, "target"),
                )
                .await
            }
            "phoenix.diagnostics.read" => self.diagnostics().await,
            _ => Err(ToolError::UnknownAction {
                message: format!("unknown action: `{name}`"),
                valid: ACTIONS
                    .iter()
                    .map(|action| action.name.to_owned())
                    .collect(),
                hint: None,
            }),
        }
    }

    fn available(&self) -> bool {
        self.config.enabled
            && self
                .config
                .command
                .as_ref()
                .is_some_and(|path| executable(path))
            && self
                .config
                .codex_home
                .as_ref()
                .is_some_and(|path| path.is_dir())
            && self
                .config
                .workspace_root
                .as_ref()
                .is_some_and(|path| path.is_dir())
    }

    fn status(&self) -> Value {
        json!({
            "enabled": self.config.enabled,
            "available": self.available(),
            "runtime": "container_local",
            "service": "codex-app-server",
            "sandbox": "read-only",
            "protocol": {
                "schema": APP_SERVER_PROTOCOL_SCHEMA,
                "runtime_version": self.detected_version(),
                "adapter": 2,
                "experimental_api": true,
            },
            "mcp": {
                "name": "labby",
                "transport": "streamable_http",
                "scope": "container_loopback",
                "authentication": "bearer_token_env",
                "configured": std::env::var_os(LOCAL_MCP_TOKEN_ENV).is_some(),
            },
            "capabilities": {
                "session_lifecycle": ["start", "read", "close"],
                "turn_lifecycle": ["start", "steer", "interrupt", "completed"],
                "operations": ["review"],
                "review": ["uncommitted_changes", "base_branch", "commit", "custom"],
                "diagnostics": ["account", "rate_limits", "usage", "config", "mcp_server_status"],
                "server_requests": ["deterministic_decline", "audit_event"],
                "preserved_events": [
                    "items", "agent_message", "reasoning", "plan", "diff",
                    "token_usage", "mcp_status", "mcp_tool_progress", "warnings", "errors"
                ],
                "inputs": ["text", "image_data_url", "audio_data_url"],
                "unsupported": [
                    "approvals", "elicitation_response",
                    "realtime", "local_path_attachments", "account_mutation", "filesystem_mutation",
                    "remote_control"
                ],
            },
        })
    }

    fn detected_version(&self) -> Option<String> {
        let command = self.config.command.as_ref()?;
        let output = std::process::Command::new(command)
            .arg("--version")
            .env_clear()
            .env("PATH", command_path(command))
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let version = String::from_utf8(output.stdout).ok()?;
        let version = version.trim();
        (!version.is_empty() && version.len() <= 128).then(|| version.to_owned())
    }

    async fn models(&self) -> Result<Value, ToolError> {
        self.require_available()?;
        let runtime = initialized_app_server(&self.config).await?;
        let response = runtime
            .request("model/list", json!({"limit":100,"includeHidden":false}))
            .await?;
        let models = response
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(protocol_error)?;
        Ok(json!({"models": models}))
    }

    async fn start(
        &self,
        owner: &str,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<Value, ToolError> {
        self.require_available()?;
        if self.sessions.lock().await.len() >= MAX_SESSIONS {
            return Err(unavailable(
                "Phoenix has reached its 32-session limit; close a session before starting another",
            ));
        }
        validate_selection("model", model.as_deref())?;
        validate_selection("effort", effort.as_deref())?;
        let runtime = initialized_app_server(&self.config).await?;
        let workspace_root = self
            .config
            .workspace_root
            .as_ref()
            .ok_or_else(|| unavailable("Phoenix workspace is not configured"))?;
        let mut params = json!({
            "cwd": workspace_root,
            "approvalPolicy": "never",
            "sandbox": "read-only",
            "serviceName": "labby-phoenix",
            "threadSource": "appServer",
            "developerInstructions": "You are Phoenix, Labby's concise operator assistant. You run only inside the Labby container. Treat the workspace as read-only, never request access to another machine or device, and explain any action that requires an operator.",
        });
        if let Some(model) = model.as_ref().or(self.config.model.as_ref()) {
            params["model"] = Value::String(model.clone());
        }
        let started = runtime.request("thread/start", params).await?;
        let thread_id = started
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(protocol_error)?
            .to_owned();
        let session_id = format!("phoenix-{}", uuid::Uuid::new_v4());
        self.sessions.lock().await.insert(
            session_id.clone(),
            Arc::new(Mutex::new(Session {
                owner: owner.to_owned(),
                thread_id,
                turn_in_progress: false,
                active_turn_id: None,
                runtime,
                messages: Vec::new(),
                events: Vec::new(),
                model,
                effort,
            })),
        );
        Ok(json!({"session_id":session_id,"status":"ready","messages":[]}))
    }

    async fn read(&self, owner: &str, session_id: &str) -> Result<Value, ToolError> {
        let session = self.session(owner, session_id).await?;
        let state = session.lock().await;
        Ok(render_session(session_id, &state))
    }

    async fn close(&self, owner: &str, session_id: &str) -> Result<Value, ToolError> {
        let session = self.session(owner, session_id).await?;
        let (runtime, thread_id) = {
            let state = session.lock().await;
            if state.turn_in_progress {
                return Err(unavailable(
                    "Phoenix cannot close a session with an active turn",
                ));
            }
            (state.runtime.clone(), state.thread_id.clone())
        };
        runtime
            .request("thread/close", json!({"threadId":thread_id}))
            .await?;
        self.sessions.lock().await.remove(session_id);
        Ok(json!({"session_id":session_id,"status":"closed"}))
    }

    async fn send(
        &self,
        owner: &str,
        session_id: &str,
        input: &str,
        attachments: Option<&Value>,
    ) -> Result<Value, ToolError> {
        self.require_available()?;
        if input.trim().is_empty() || input.len() > MAX_INPUT_BYTES {
            return Err(invalid("input", "input must contain 1-32768 bytes"));
        }
        let session = self.session(owner, session_id).await?;
        let protocol_inputs = turn_inputs(input, attachments)?;
        let (runtime, thread_id, model, effort) = {
            let mut state = session.lock().await;
            if state.turn_in_progress {
                return Err(unavailable("Phoenix session already has an active turn"));
            }
            state.turn_in_progress = true;
            state.messages.push(Message {
                role: "user",
                text: input.to_owned(),
            });
            (
                state.runtime.clone(),
                state.thread_id.clone(),
                state.model.clone(),
                state.effort.clone(),
            )
        };
        let mut events = runtime.subscribe();
        let started = runtime
            .request("turn/start", {
                let mut params = json!({
                  "threadId":thread_id,
                  "input":protocol_inputs
                });
                if let Some(model) = model {
                    params["model"] = Value::String(model);
                }
                if let Some(effort) = effort {
                    params["effort"] = Value::String(effort);
                }
                params
            })
            .await;
        let started = match started {
            Ok(started) => started,
            Err(error) => {
                session.lock().await.turn_in_progress = false;
                return Err(error);
            }
        };
        let turn_id = started
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let Some(turn_id) = turn_id else {
            session.lock().await.turn_in_progress = false;
            return Err(protocol_error());
        };
        session.lock().await.active_turn_id = Some(turn_id.clone());
        let result = tokio::time::timeout(
            TURN_TIMEOUT,
            collect_turn(&mut events, &thread_id, &turn_id, &session),
        )
        .await;
        let result = match result {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                let mut state = session.lock().await;
                state.turn_in_progress = false;
                state.active_turn_id = None;
                return Err(error);
            }
            Err(_) => {
                drop(runtime.interrupt(&thread_id, &turn_id).await);
                let mut state = session.lock().await;
                state.turn_in_progress = false;
                state.active_turn_id = None;
                return Err(unavailable(
                    "Phoenix turn exceeded the five minute runtime limit",
                ));
            }
        };
        let mut state = session.lock().await;
        state.turn_in_progress = false;
        state.active_turn_id = None;
        state.messages.push(Message {
            role: "assistant",
            text: result.output,
        });
        if state.messages.len() > MAX_MESSAGES {
            let excess = state.messages.len() - MAX_MESSAGES;
            state.messages.drain(..excess);
        }
        Ok(render_session(session_id, &state))
    }

    async fn interrupt(&self, owner: &str, session_id: &str) -> Result<Value, ToolError> {
        let session = self.session(owner, session_id).await?;
        let (runtime, thread_id, turn_id) = {
            let state = session.lock().await;
            let turn_id = state
                .active_turn_id
                .clone()
                .ok_or_else(|| invalid("session_id", "Phoenix session has no active turn"))?;
            (state.runtime.clone(), state.thread_id.clone(), turn_id)
        };
        runtime.interrupt(&thread_id, &turn_id).await?;
        Ok(json!({"session_id":session_id,"status":"interrupting","turn_id":turn_id}))
    }

    async fn steer(
        &self,
        owner: &str,
        session_id: &str,
        input: &str,
        attachments: Option<&Value>,
    ) -> Result<Value, ToolError> {
        if input.trim().is_empty() || input.len() > MAX_INPUT_BYTES {
            return Err(invalid("input", "input must contain 1-32768 bytes"));
        }
        let protocol_inputs = turn_inputs(input, attachments)?;
        let session = self.session(owner, session_id).await?;
        let (runtime, thread_id, turn_id) = {
            let state = session.lock().await;
            let turn_id = state
                .active_turn_id
                .clone()
                .ok_or_else(|| invalid("session_id", "Phoenix session has no active turn"))?;
            (state.runtime.clone(), state.thread_id.clone(), turn_id)
        };
        let response = runtime
            .request(
                "turn/steer",
                json!({"threadId":thread_id,"expectedTurnId":turn_id,"input":protocol_inputs}),
            )
            .await?;
        Ok(json!({"session_id":session_id,"status":"steered","turn":safe_value(&response)}))
    }

    async fn review(
        &self,
        owner: &str,
        session_id: &str,
        target_type: &str,
        target: Option<String>,
    ) -> Result<Value, ToolError> {
        let session = self.session(owner, session_id).await?;
        let (runtime, thread_id) = {
            let state = session.lock().await;
            if state.turn_in_progress {
                return Err(unavailable("Phoenix session already has an active turn"));
            }
            (state.runtime.clone(), state.thread_id.clone())
        };
        let review_target = match target_type {
            "uncommitted_changes" | "uncommittedChanges" => json!({"type":"uncommittedChanges"}),
            "base_branch" => json!({"type":"baseBranch","branch":required_target(target)?}),
            "commit" => json!({"type":"commit","sha":required_target(target)?}),
            "custom" => json!({"type":"custom","instructions":required_target(target)?}),
            _ => return Err(invalid("target_type", "unsupported review target")),
        };
        let response = runtime
            .request(
                "review/start",
                json!({"threadId":thread_id,"target":review_target,"delivery":"inline"}),
            )
            .await?;
        Ok(json!({"session_id":session_id,"status":"reviewing","review":safe_value(&response)}))
    }

    async fn diagnostics(&self) -> Result<Value, ToolError> {
        self.require_available()?;
        let runtime = initialized_app_server(&self.config).await?;
        let workspace_root = self
            .config
            .workspace_root
            .as_ref()
            .ok_or_else(|| unavailable("Phoenix workspace is not configured"))?;
        let account = runtime
            .request("account/read", json!({"refreshToken":false}))
            .await?;
        let rate_limits = runtime
            .request("account/rateLimits/read", json!({}))
            .await?;
        let usage = runtime.request("account/usage/read", json!({})).await?;
        let config = runtime
            .request(
                "config/read",
                json!({"cwd":workspace_root,"includeLayers":false}),
            )
            .await?;
        let mcp_servers = runtime
            .request(
                "mcpServerStatus/list",
                json!({"limit":100,"detail":"toolsAndAuthOnly"}),
            )
            .await?;
        Ok(json!({
            "account": safe_account(&account),
            "rate_limits": safe_value(&rate_limits),
            "usage": safe_value(&usage),
            "config": safe_config(&config),
            "mcp_servers": safe_value(&mcp_servers),
        }))
    }

    async fn session(
        &self,
        owner: &str,
        session_id: &str,
    ) -> Result<Arc<Mutex<Session>>, ToolError> {
        let session = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(denied)?;
        if session.lock().await.owner != owner {
            return Err(denied());
        }
        Ok(session)
    }

    fn require_available(&self) -> Result<(), ToolError> {
        if self.available() {
            Ok(())
        } else {
            Err(unavailable(
                "Phoenix requires an enabled container-local Codex App Server runtime",
            ))
        }
    }
}

impl Default for PhoenixRuntime {
    fn default() -> Self {
        Self::new(PhoenixPreferences::default())
    }
}

fn render_session(session_id: &str, session: &Session) -> Value {
    json!({
        "session_id": session_id,
        "status": "ready",
        "messages": session.messages.iter().map(|message| json!({
            "role": message.role,
            "text": message.text,
        })).collect::<Vec<_>>(),
        "events": session.events,
        "model": session.model,
        "effort": session.effort,
    })
}

struct TurnResult {
    output: String,
}

async fn launch_app_server(config: &PhoenixPreferences) -> Result<AppServerRuntime, ToolError> {
    let command = config
        .command
        .as_ref()
        .ok_or_else(|| unavailable("Codex command is not configured"))?;
    let codex_home = config
        .codex_home
        .as_ref()
        .ok_or_else(|| unavailable("Codex home is not configured"))?;
    let workspace_root = config
        .workspace_root
        .as_ref()
        .ok_or_else(|| unavailable("Phoenix workspace is not configured"))?;
    let mut env = vec![
        (OsString::from("HOME"), codex_home.as_os_str().to_owned()),
        (
            OsString::from("CODEX_HOME"),
            codex_home.as_os_str().to_owned(),
        ),
        (
            OsString::from("PATH"),
            OsString::from(command_path(command)),
        ),
        (OsString::from("LANG"), OsString::from("C.UTF-8")),
    ];
    if let Some(token) = std::env::var_os(LOCAL_MCP_TOKEN_ENV) {
        env.push((OsString::from(LOCAL_MCP_TOKEN_ENV), token));
    }
    AppServerRuntime::launch(LaunchSpec {
        command: command.clone(),
        args: vec![
            "app-server".into(),
            "--stdio".into(),
            "-c".into(),
            format!("mcp_servers.labby.url=\\\"{LOCAL_MCP_URL}\\\"").into(),
            "-c".into(),
            format!("mcp_servers.labby.bearer_token_env_var=\\\"{LOCAL_MCP_TOKEN_ENV}\\\"").into(),
        ],
        env,
        cwd: workspace_root.clone(),
    })
    .await
}

async fn initialized_app_server(
    config: &PhoenixPreferences,
) -> Result<AppServerRuntime, ToolError> {
    let runtime = launch_app_server(config).await?;
    runtime.request("initialize", json!({
        "clientInfo":{"name":"labby_phoenix","title":"Labby Phoenix","version":env!("CARGO_PKG_VERSION")},
        "capabilities":{"experimentalApi":true}
    })).await?;
    runtime.notify("initialized", json!({})).await?;
    Ok(runtime)
}

fn optional(params: &Value, name: &str) -> Option<String> {
    params.get(name).and_then(Value::as_str).map(str::to_owned)
}

fn validate_selection(name: &str, value: Option<&str>) -> Result<(), ToolError> {
    if value.is_some_and(|value| {
        value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    }) {
        return Err(invalid(
            name,
            &format!("{name} must be a valid advertised value"),
        ));
    }
    Ok(())
}

fn turn_inputs(input: &str, attachments: Option<&Value>) -> Result<Vec<Value>, ToolError> {
    let mut values = vec![json!({"type":"text","text":input,"text_elements":[]})];
    let Some(attachments) = attachments else {
        return Ok(values);
    };
    let attachments = attachments
        .as_array()
        .ok_or_else(|| invalid("attachments", "attachments must be an array"))?;
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(invalid("attachments", "at most 4 attachments are allowed"));
    }
    for attachment in attachments {
        let kind = attachment
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("attachments", "attachment type is required"))?;
        let url = attachment
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("attachments", "attachment data URL is required"))?;
        let allowed = match kind {
            "image" => [
                "data:image/png;base64,",
                "data:image/jpeg;base64,",
                "data:image/webp;base64,",
            ]
            .iter()
            .any(|prefix| url.starts_with(prefix)),
            "audio" => [
                "data:audio/mpeg;base64,",
                "data:audio/wav;base64,",
                "data:audio/mp4;base64,",
                "data:audio/webm;base64,",
            ]
            .iter()
            .any(|prefix| url.starts_with(prefix)),
            _ => false,
        };
        let payload = url.split_once(',').map(|(_, payload)| payload);
        let decoded = payload.and_then(|payload| {
            base64::engine::general_purpose::STANDARD
                .decode(payload)
                .ok()
        });
        if !allowed
            || decoded
                .as_ref()
                .is_none_or(|bytes| bytes.is_empty() || bytes.len() > MAX_ATTACHMENT_BYTES)
        {
            return Err(invalid(
                "attachments",
                "attachments must be bounded PNG, JPEG, WebP, MP3, WAV, M4A, or WebM data URLs",
            ));
        }
        values.push(json!({"type":kind,"url":url}));
    }
    Ok(values)
}

async fn collect_turn(
    receiver: &mut tokio::sync::broadcast::Receiver<
        crate::dispatch::phoenix_runtime::AppServerEvent,
    >,
    thread_id: &str,
    turn_id: &str,
    session: &Arc<Mutex<Session>>,
) -> Result<TurnResult, ToolError> {
    let mut deltas = String::new();
    let mut completed_text = String::new();
    loop {
        let event = receiver
            .recv()
            .await
            .map_err(|_| unavailable("Phoenix missed App Server turn events"))?
            .0;
        if event.pointer("/params/threadId").and_then(Value::as_str) != Some(thread_id) {
            continue;
        }
        let event_turn_id = event
            .pointer("/params/turnId")
            .or_else(|| event.pointer("/params/turn/id"))
            .and_then(Value::as_str);
        if event_turn_id.is_some_and(|value| value != turn_id) {
            continue;
        }
        if let Some(candidate) = sanitized_event(&event) {
            let mut state = session.lock().await;
            state.events.push(candidate);
            if state.events.len() > MAX_EVENTS {
                let excess = state.events.len() - MAX_EVENTS;
                state.events.drain(..excess);
            }
        }
        match event.get("method").and_then(Value::as_str) {
            Some("item/agentMessage/delta") => {
                if let Some(delta) = event.pointer("/params/delta").and_then(Value::as_str)
                    && deltas.len().saturating_add(delta.len()) <= MAX_OUTPUT_BYTES
                {
                    deltas.push_str(delta);
                }
            }
            Some("item/completed") => {
                if event.pointer("/params/item/type").and_then(Value::as_str)
                    == Some("agentMessage")
                    && let Some(text) = event.pointer("/params/item/text").and_then(Value::as_str)
                {
                    completed_text = bounded(text);
                }
            }
            Some("turn/completed") => {
                let status = event
                    .pointer("/params/turn/status")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                if status != "completed" {
                    let message = event
                        .pointer("/params/turn/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("Codex turn did not complete");
                    return Err(unavailable(message));
                }
                let output = if completed_text.is_empty() {
                    bounded(&deltas)
                } else {
                    completed_text
                };
                if output.is_empty() {
                    return Err(protocol_error());
                }
                return Ok(TurnResult { output });
            }
            _ => {}
        }
    }
}

fn sanitized_event(event: &Value) -> Option<Value> {
    let Some(method) = event.get("method").and_then(Value::as_str) else {
        return None;
    };
    if !matches!(
        method,
        "thread/started"
            | "thread/status/changed"
            | "thread/tokenUsage/updated"
            | "turn/started"
            | "turn/completed"
            | "turn/plan/updated"
            | "turn/diff/updated"
            | "item/started"
            | "item/completed"
            | "item/agentMessage/delta"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/summaryPartAdded"
            | "item/mcpToolCall/progress"
            | "mcpServer/status/updated"
            | "account/rateLimits/updated"
            | "error"
            | "warning"
            | "phoenix/serverRequestDeclined"
    ) {
        return None;
    }
    let candidate = json!({
        "method": method,
        "params": safe_value(event.get("params").unwrap_or(&Value::Null)),
    });
    serde_json::to_vec(&candidate)
        .is_ok_and(|bytes| bytes.len() <= MAX_OUTPUT_BYTES)
        .then_some(candidate)
}

fn required_target(target: Option<String>) -> Result<String, ToolError> {
    let target = target.filter(|value| !value.trim().is_empty() && value.len() <= MAX_INPUT_BYTES);
    target.ok_or_else(|| invalid("target", "this review target requires a bounded value"))
}

fn safe_account(value: &Value) -> Value {
    json!({
        "requiresOpenaiAuth": value.get("requiresOpenaiAuth").cloned().unwrap_or(Value::Null),
        "account": value.get("account").map(|account| json!({
            "type": account.get("type").cloned().unwrap_or(Value::Null),
            "planType": account.get("planType").cloned().unwrap_or(Value::Null),
        })).unwrap_or(Value::Null),
    })
}

fn safe_config(value: &Value) -> Value {
    let config = value.get("config").unwrap_or(&Value::Null);
    json!({
        "model": config.get("model").cloned().unwrap_or(Value::Null),
        "model_provider": config.get("model_provider").cloned().unwrap_or(Value::Null),
        "model_reasoning_effort": config.get("model_reasoning_effort").cloned().unwrap_or(Value::Null),
        "sandbox_mode": config.get("sandbox_mode").cloned().unwrap_or(Value::Null),
        "service_tier": config.get("service_tier").cloned().unwrap_or(Value::Null),
        "web_search": config.get("web_search").cloned().unwrap_or(Value::Null),
    })
}

fn safe_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| !sensitive_key(key))
                .map(|(key, value)| (key.clone(), safe_value(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(safe_value).collect()),
        Value::String(value) => Value::String(bounded(value)),
        other => other.clone(),
    }
}

fn sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
    normalized == "token"
        || normalized.ends_with("accesstoken")
        || normalized.ends_with("refreshtoken")
        || normalized.contains("secret")
        || normalized.contains("password")
        || normalized.contains("authorization")
        || normalized.contains("apikey")
        || normalized.contains("bearer")
        || normalized == "email"
}

fn required(params: &Value, name: &str) -> Result<String, ToolError> {
    params
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ToolError::MissingParam {
            message: format!("missing required parameter: `{name}`"),
            param: name.to_owned(),
        })
}

fn bounded(value: &str) -> String {
    if value.len() <= MAX_OUTPUT_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_OUTPUT_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn executable(path: &std::path::Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn command_path(command: &std::path::Path) -> String {
    let inherited =
        std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".to_owned());
    command.parent().map_or(inherited.clone(), |directory| {
        format!("{}:{inherited}", directory.display())
    })
}

fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "Phoenix session is not visible to this identity".into(),
        required_scopes: vec![],
    }
}

fn invalid(param: &str, message: &str) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: param.into(),
    }
}

fn unavailable(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "executor_unavailable".into(),
        message: message.into(),
    }
}

fn protocol_error() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "decode_error".into(),
        message: "Container-local Codex App Server returned an invalid protocol response".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::fs;

    #[tokio::test]
    async fn unavailable_by_default_and_sessions_are_owner_scoped() {
        let runtime = PhoenixRuntime::default();
        let status = runtime
            .dispatch("principal-a", "phoenix.status", json!({}))
            .await
            .unwrap();
        assert_eq!(status["enabled"], false);
        assert_eq!(status["available"], false);
        assert_eq!(status["runtime"], "container_local");
        assert_eq!(status["sandbox"], "read-only");
        assert_eq!(
            runtime
                .dispatch("principal-a", "phoenix.session.start", json!({}))
                .await
                .unwrap_err()
                .kind(),
            "executor_unavailable"
        );
    }

    #[test]
    fn config_boundary_requires_executable_and_container_directories() {
        let runtime = PhoenixRuntime::new(PhoenixPreferences {
            enabled: true,
            command: Some("/missing/codex".into()),
            codex_home: Some("/missing/codex-home".into()),
            workspace_root: Some("/missing/workspace".into()),
            model: None,
        });
        assert!(!runtime.available());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn app_server_protocol_runs_in_operator_owned_paths_and_scopes_the_session() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let command = root.path().join("codex-fixture");
        let capture = root.path().join("requests.jsonl");
        let script = format!(
            r#"#!/bin/sh
read initialize
printf '%s\n' "$initialize" >> '{}'
printf '%s\n' '{{"id":1,"result":{{"userAgent":"fixture"}}}}'
read initialized
printf '%s\n' "$initialized" >> '{}'
read thread
printf '%s\n' "$thread" >> '{}'
printf '%s\n' '{{"id":2,"result":{{"thread":{{"id":"thread-container"}}}}}}'
while read turn; do
  printf '%s\n' "$turn" >> '{}'
  id=$(printf '%s' "$turn" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  printf '{{"id":%s,"result":{{"turn":{{"id":"turn-1"}}}}}}\n' "$id"
  printf '%s\n' '{{"method":"turn/plan/updated","params":{{"threadId":"thread-container","turnId":"turn-1","plan":[{{"step":"Inspect health","status":"completed"}}]}}}}'
  printf '%s\n' '{{"method":"thread/tokenUsage/updated","params":{{"threadId":"thread-container","tokenUsage":{{"total":{{"totalTokens":42}}}}}}}}'
  printf '%s\n' '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-container","turnId":"turn-1","itemId":"message-1","delta":"hello from container"}}}}'
  printf '%s\n' '{{"method":"turn/completed","params":{{"threadId":"thread-container","turn":{{"id":"turn-1","status":"completed","items":[],"error":null}}}}}}'
done
"#,
            capture.display(),
            capture.display(),
            capture.display(),
            capture.display()
        );
        fs::write(&command, script).unwrap();
        fs::set_permissions(&command, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = PhoenixRuntime::new(PhoenixPreferences {
            enabled: true,
            command: Some(command),
            codex_home: Some(root.path().to_path_buf()),
            workspace_root: Some(root.path().to_path_buf()),
            model: Some("fixture-model".into()),
        });
        let started = runtime
            .dispatch("principal-a", "phoenix.session.start", json!({}))
            .await
            .unwrap();
        let session_id = started["session_id"].as_str().unwrap();
        let completed = runtime
            .dispatch(
                "principal-a",
                "phoenix.turn.send",
                json!({"session_id":session_id,"input":"hello"}),
            )
            .await
            .unwrap();
        assert_eq!(completed["messages"][1]["text"], "hello from container");
        assert_eq!(completed["events"][0]["method"], "turn/plan/updated");
        assert_eq!(
            completed["events"][1]["method"],
            "thread/tokenUsage/updated"
        );
        assert_eq!(completed["events"][2]["method"], "item/agentMessage/delta");
        let resumed = runtime
            .dispatch(
                "principal-a",
                "phoenix.turn.send",
                json!({"session_id":session_id,"input":"again"}),
            )
            .await
            .unwrap();
        assert_eq!(resumed["messages"][3]["text"], "hello from container");
        assert_eq!(
            runtime
                .dispatch(
                    "principal-b",
                    "phoenix.session.read",
                    json!({"session_id":session_id}),
                )
                .await
                .unwrap_err()
                .kind(),
            "forbidden"
        );
        let requests = fs::read_to_string(capture).unwrap();
        assert!(requests.contains("\"method\":\"initialize\""));
        assert!(requests.contains("\"method\":\"initialized\""));
        assert!(requests.contains("\"sandbox\":\"read-only\""));
        assert!(requests.contains(&format!("\"cwd\":\"{}\"", root.path().display())));
        assert!(requests.contains("\"method\":\"turn/start\""));
        assert!(!requests.contains("excludeTurns"));
        assert!(requests.contains("\"experimentalApi\":true"));
    }

    #[test]
    fn turn_inputs_accept_only_bounded_inline_media() {
        let inputs = turn_inputs(
            "inspect",
            Some(&json!([{
                "type":"image", "url":"data:image/png;base64,aGVsbG8="
            }])),
        )
        .unwrap();
        assert_eq!(inputs[1]["type"], "image");
        assert!(
            turn_inputs(
                "inspect",
                Some(&json!([{
                    "type":"image", "url":"file:///etc/passwd"
                }]))
            )
            .is_err()
        );
        assert!(
            turn_inputs(
                "inspect",
                Some(&json!([{
                    "type":"audio", "url":"https://example.test/audio.mp3"
                }]))
            )
            .is_err()
        );
        assert!(turn_inputs("inspect", Some(&json!([{}, {}, {}, {}, {}]))).is_err());
    }

    #[test]
    fn status_reports_truthful_protocol_boundary_and_local_mcp_transport() {
        let runtime = PhoenixRuntime::default();
        let status = runtime.status();
        assert_eq!(status["protocol"]["schema"], "v2");
        assert_eq!(status["protocol"]["adapter"], 2);
        assert_eq!(status["mcp"]["transport"], "streamable_http");
        assert_eq!(status["mcp"]["scope"], "container_loopback");
        assert_eq!(
            status["capabilities"]["inputs"],
            json!(["text", "image_data_url", "audio_data_url"])
        );
        assert_eq!(
            status["capabilities"]["turn_lifecycle"],
            json!(["start", "steer", "interrupt", "completed"])
        );
        assert_eq!(status["capabilities"]["operations"], json!(["review"]));
        assert_eq!(
            status["capabilities"]["session_lifecycle"],
            json!(["start", "read", "close"])
        );
    }

    #[test]
    fn retained_events_are_whitelisted_bounded_and_redacted() {
        assert!(
            sanitized_event(&json!({
                "method":"unknown/private",
                "params":{"authorization":"secret"}
            }))
            .is_none()
        );
        let event = sanitized_event(&json!({
            "method":"item/completed",
            "params": {
                "threadId":"thread-1",
                "item":{"type":"mcpToolCall","access_token":"secret","tokenUsage":12},
                "email":"private@example.test"
            }
        }))
        .unwrap();
        assert!(event.pointer("/params/item/access_token").is_none());
        assert!(event.pointer("/params/email").is_none());
        assert_eq!(event.pointer("/params/item/tokenUsage"), Some(&json!(12)));
    }

    #[test]
    fn review_targets_are_explicit_and_bounded() {
        assert_eq!(required_target(Some("main".into())).unwrap(), "main");
        assert!(required_target(None).is_err());
        assert!(required_target(Some("".into())).is_err());
    }
}
