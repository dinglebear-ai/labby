//! Provider adapter for the Phoenix assistant.

use std::{collections::HashMap, ffi::OsString, sync::Arc, time::Duration};

use base64::Engine as _;
use labby_primitives::action::{ActionSpec, ParamSpec};
use serde_json::{Value, json};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::{
    config::{PhoenixPreferences, PhoenixProvider},
    dispatch::{
        error::ToolError,
        phoenix_openai::{ChatMessage, OpenAiBackend},
        phoenix_runtime::{AppServerRuntime, LaunchSpec},
    },
};

const TURN_TIMEOUT: Duration = Duration::from_mins(5);
const MAX_INPUT_BYTES: usize = 32 * 1024;
const MAX_ATTACHMENTS: usize = 4;
const MAX_ATTACHMENT_BYTES: usize = 5 * 1024 * 1024;
const MAX_TEXT_ATTACHMENT_BYTES: usize = 512 * 1024;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_MESSAGES: usize = 100;
const MAX_EVENTS: usize = 500;
const MAX_SESSIONS: usize = 32;
const MAX_SESSION_TITLE_CHARS: usize = 120;
const MAX_SESSION_PREVIEW_CHARS: usize = 240;
const APP_SERVER_PROTOCOL_SCHEMA: &str = "v2";
const LOCAL_MCP_URL: &str = "http://127.0.0.1:8765/mcp";
const LOCAL_MCP_TOKEN_ENV: &str = "LABBY_MCP_HTTP_TOKEN";
const PHOENIX_DEVELOPER_INSTRUCTIONS: &str = "You are Phoenix, Labby's concise operator assistant. You run only inside the Labby container. Treat the workspace as read-only, never request access to another machine or device, and explain any action that requires an operator.";

#[cfg(feature = "skills")]
async fn phoenix_developer_instructions() -> Result<String, ToolError> {
    const BOOTSTRAP_URI: &str = "skill://labby/using-labby/SKILL.md";
    let context = crate::skills::facade::SkillRegistryContext::first_party_only();
    let file = crate::skills::facade::read_visible_skill_file(&context, BOOTSTRAP_URI).await?;
    let body = file
        .content
        .text()
        .ok_or_else(|| unavailable("Phoenix bootstrap skill is not text"))?;
    Ok(format!(
        "{PHOENIX_DEVELOPER_INSTRUCTIONS}\n\nThe bundled Agent Skill at {BOOTSTRAP_URI} is loaded for this session. Follow its instructions when operating Labby:\n\n{body}"
    ))
}

#[cfg(not(feature = "skills"))]
async fn phoenix_developer_instructions() -> Result<String, ToolError> {
    Ok(PHOENIX_DEVELOPER_INSTRUCTIONS.to_owned())
}

/// Resolve the local MCP destination from the HTTP listener's address.
pub(crate) fn listener_mcp_url(mut address: std::net::SocketAddr) -> String {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => address.set_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        IpAddr::V6(ip) if ip.is_unspecified() => address.set_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)),
        _ => {}
    }
    format!("http://{address}/mcp")
}

#[cfg(test)]
mod listener_url_tests {
    use super::listener_mcp_url;

    #[test]
    fn preserves_specific_addresses_and_normalizes_wildcards() {
        for (host, expected) in [
            ("127.0.0.1", "127.0.0.1"),
            ("192.0.2.42", "192.0.2.42"),
            ("0.0.0.0", "127.0.0.1"),
            ("[::1]", "[::1]"),
            ("[2001:db8::42]", "[2001:db8::42]"),
            ("[::]", "[::1]"),
        ] {
            assert_eq!(
                listener_mcp_url(format!("{host}:9123").parse().unwrap()),
                format!("http://{expected}:9123/mcp")
            );
        }
    }

    #[test]
    fn ephemeral_destination_connects_to_the_actual_listener() {
        use std::net::{TcpListener, TcpStream};
        use std::time::Duration;

        for host in ["127.0.0.1", "0.0.0.0", "::1", "::"] {
            let listener = TcpListener::bind((host, 0)).unwrap();
            let address = listener.local_addr().unwrap();
            let port = address.port();
            assert_ne!(port, 0, "the OS must assign the requested ephemeral port");
            let url = listener_mcp_url(address);
            let destination = url
                .strip_prefix("http://")
                .unwrap()
                .strip_suffix("/mcp")
                .unwrap()
                .parse()
                .unwrap();
            let stream = TcpStream::connect_timeout(&destination, Duration::from_secs(1))
                .unwrap_or_else(|error| panic!("{url} cannot reach listener {host}: {error}"));
            assert_eq!(stream.peer_addr().unwrap().port(), port);
        }
    }
}

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
        output_schema: None,
    }
}

pub(crate) const ACTIONS: &[ActionSpec] = &[
    action("phoenix.status", "Read Phoenix availability", &[]),
    action("phoenix.models.list", "List selectable Codex models", &[]),
    action(
        "phoenix.session.list",
        "List caller-scoped Phoenix sessions",
        &[],
    ),
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
        "phoenix.session.rename",
        "Rename a caller-scoped Phoenix session",
        &[param("session_id", true), param("title", true)],
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
    created_at_ms: u64,
}

#[derive(Clone)]
enum SessionBackend {
    Codex {
        thread_id: String,
        runtime: AppServerRuntime,
    },
    OpenAi {
        session_id: String,
        backend: OpenAiBackend,
    },
}

struct Session {
    owner: String,
    backend: SessionBackend,
    turn_in_progress: bool,
    closing: bool,
    active_turn_id: Option<String>,
    messages: Vec<Message>,
    events: Vec<Value>,
    next_event_sequence: u64,
    model: Option<String>,
    effort: Option<String>,
    title: Option<String>,
    _capacity: OwnedSemaphorePermit,
}

#[derive(Clone)]
pub(crate) struct PhoenixRuntime {
    config: PhoenixPreferences,
    local_mcp_url: Arc<str>,
    openai: Option<OpenAiBackend>,
    sessions: Arc<Mutex<HashMap<String, Arc<Mutex<Session>>>>>,
    capacity: Arc<Semaphore>,
}

impl PhoenixRuntime {
    #[must_use]
    pub(crate) fn new(config: PhoenixPreferences) -> Self {
        Self::new_with_mcp_url(config, LOCAL_MCP_URL)
    }

    #[must_use]
    pub(crate) fn new_with_mcp_url(
        config: PhoenixPreferences,
        local_mcp_url: impl Into<Arc<str>>,
    ) -> Self {
        Self::new_with_backends(config, local_mcp_url, OpenAiBackend::from_env())
    }

    fn new_with_backends(
        config: PhoenixPreferences,
        local_mcp_url: impl Into<Arc<str>>,
        openai: Option<OpenAiBackend>,
    ) -> Self {
        Self {
            config,
            local_mcp_url: local_mcp_url.into(),
            openai,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            capacity: Arc::new(Semaphore::new(MAX_SESSIONS)),
        }
    }

    /// Host of the server-owned endpoint used by the local client.
    pub(crate) fn local_mcp_host(&self) -> Option<String> {
        let url = url::Url::parse(&self.local_mcp_url).ok()?;
        Some(url.host_str()?.to_owned())
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
            "phoenix.session.list" => self.list(owner).await,
            "phoenix.session.start" => {
                self.start(
                    owner,
                    optional(&params, "model"),
                    optional(&params, "effort"),
                )
                .await
            }
            "phoenix.session.read" => self.read(owner, &required(&params, "session_id")?).await,
            "phoenix.session.rename" => {
                self.rename(
                    owner,
                    &required(&params, "session_id")?,
                    &required(&params, "title")?,
                )
                .await
            }
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
        if !self.config.enabled {
            return false;
        }
        match self.config.provider {
            PhoenixProvider::CodexAppServer => {
                self.config
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
            PhoenixProvider::OpenAiCompatible => self.openai.is_some(),
        }
    }

    fn status(&self) -> Value {
        match self.config.provider {
            PhoenixProvider::CodexAppServer => json!({
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
                    "session_lifecycle": ["list", "start", "read", "close"],
                    "turn_lifecycle": ["start", "steer", "interrupt", "completed"],
                    "operations": ["review"],
                    "review": ["uncommitted_changes", "base_branch", "commit", "custom"],
                    "diagnostics": ["account", "rate_limits", "usage", "config", "mcp_server_status"],
                    "server_requests": ["deterministic_decline", "audit_event"],
                    "preserved_events": [
                        "items", "agent_message", "reasoning", "plan", "diff",
                        "tool_output", "hooks", "subagents", "model_events",
                        "token_usage", "mcp_status", "mcp_tool_progress", "warnings", "errors"
                    ],
                    "inputs": ["text", "image_data_url", "text_file_data_url", "audio_data_url"],
                    "unsupported": [
                        "approvals", "elicitation_response",
                        "realtime", "local_path_attachments", "account_mutation", "filesystem_mutation",
                        "remote_control"
                    ],
                },
            }),
            PhoenixProvider::OpenAiCompatible => json!({
                "enabled": self.config.enabled,
                "available": self.available(),
                "runtime": "remote_http",
                "service": "openai-compatible",
                "sandbox": "remote",
                "protocol": {
                    "schema": "openai-v1",
                    "runtime_version": Value::Null,
                    "adapter": 1,
                    "experimental_api": false,
                },
                "capabilities": {
                    "session_lifecycle": ["list", "start", "read", "close"],
                    "turn_lifecycle": ["start", "interrupt", "completed"],
                    "operations": [],
                    "review": [],
                    "diagnostics": [],
                    "server_requests": [],
                    "preserved_events": [],
                    "inputs": ["text"],
                    "unsupported": [
                        "steer", "review", "diagnostics", "image_data_url", "audio_data_url",
                        "approvals", "elicitation_response", "realtime", "local_path_attachments",
                        "account_mutation", "filesystem_mutation", "remote_control"
                    ],
                },
            }),
        }
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
        match self.config.provider {
            PhoenixProvider::CodexAppServer => {
                let runtime = initialized_app_server(&self.config, &self.local_mcp_url).await?;
                let response = runtime
                    .request("model/list", json!({"limit":100,"includeHidden":false}))
                    .await?;
                let models = response
                    .get("data")
                    .and_then(Value::as_array)
                    .ok_or_else(protocol_error)?;
                Ok(json!({"models": models}))
            }
            PhoenixProvider::OpenAiCompatible => {
                let backend = self.openai_backend()?;
                let raw = backend.models().await?;
                let configured = self.config.model.as_deref();
                let models = raw
                    .iter()
                    .filter_map(|model| {
                        let id = model.get("id")?.as_str()?;
                        let effort = model
                            .pointer("/gateway/thinking_effort")
                            .and_then(Value::as_str)
                            .unwrap_or("default");
                        Some(json!({
                            "id": id,
                            "model": id,
                            "displayName": id,
                            "description": "OpenAI-compatible provider model",
                            "isDefault": configured.map_or(id == "chatgpt-browser", |value| value == id),
                            "inputModalities": ["text"],
                            "defaultReasoningEffort": effort,
                            "supportedReasoningEfforts": [{
                                "reasoningEffort": effort,
                                "description": "Provider-defined reasoning profile"
                            }]
                        }))
                    })
                    .collect::<Vec<_>>();
                Ok(json!({"models": models}))
            }
        }
    }

    async fn start(
        &self,
        owner: &str,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<Value, ToolError> {
        self.require_available()?;
        let capacity = Arc::clone(&self.capacity).try_acquire_owned().map_err(|_| {
            unavailable(
                "Phoenix has reached its 32-session limit; close a session before starting another",
            )
        })?;
        validate_selection("model", model.as_deref())?;
        validate_selection("effort", effort.as_deref())?;
        match self.config.provider {
            PhoenixProvider::CodexAppServer => {
                self.start_codex(owner, model, effort, capacity).await
            }
            PhoenixProvider::OpenAiCompatible => {
                self.start_openai(owner, model, effort, capacity).await
            }
        }
    }

    async fn start_codex(
        &self,
        owner: &str,
        model: Option<String>,
        effort: Option<String>,
        capacity: OwnedSemaphorePermit,
    ) -> Result<Value, ToolError> {
        let runtime = initialized_app_server(&self.config, &self.local_mcp_url).await?;
        let workspace_root = self
            .config
            .workspace_root
            .as_ref()
            .ok_or_else(|| unavailable("Phoenix workspace is not configured"))?;
        let selected_model = model.or_else(|| self.config.model.clone());
        let developer_instructions = phoenix_developer_instructions().await?;
        let mut params = json!({
            "cwd": workspace_root,
            "approvalPolicy": "never",
            "sandbox": "read-only",
            "serviceName": "labby-phoenix",
            "threadSource": "appServer",
            "developerInstructions": developer_instructions,
        });
        if let Some(model) = selected_model.as_ref() {
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
                backend: SessionBackend::Codex { thread_id, runtime },
                turn_in_progress: false,
                closing: false,
                active_turn_id: None,
                messages: Vec::new(),
                events: Vec::new(),
                next_event_sequence: 0,
                model: selected_model,
                effort,
                title: None,
                _capacity: capacity,
            })),
        );
        Ok(json!({"session_id":session_id,"status":"ready","messages":[]}))
    }

    async fn start_openai(
        &self,
        owner: &str,
        requested_model: Option<String>,
        effort: Option<String>,
        capacity: OwnedSemaphorePermit,
    ) -> Result<Value, ToolError> {
        let backend = self.openai_backend()?;
        let models = backend.models().await?;
        let selected_model = select_openai_model(
            &models,
            requested_model.as_deref(),
            self.config.model.as_deref(),
            effort.as_deref(),
        )?;
        let session_id = format!("phoenix-{}", uuid::Uuid::new_v4());
        backend.create_session(&session_id).await?;
        self.sessions.lock().await.insert(
            session_id.clone(),
            Arc::new(Mutex::new(Session {
                owner: owner.to_owned(),
                backend: SessionBackend::OpenAi {
                    session_id: session_id.clone(),
                    backend,
                },
                turn_in_progress: false,
                closing: false,
                active_turn_id: None,
                messages: Vec::new(),
                events: Vec::new(),
                next_event_sequence: 0,
                model: Some(selected_model),
                effort,
                title: None,
                _capacity: capacity,
            })),
        );
        Ok(json!({"session_id":session_id,"status":"ready","messages":[]}))
    }

    async fn list(&self, owner: &str) -> Result<Value, ToolError> {
        let sessions = self
            .sessions
            .lock()
            .await
            .iter()
            .map(|(session_id, session)| (session_id.clone(), Arc::clone(session)))
            .collect::<Vec<_>>();
        let mut summaries = Vec::with_capacity(sessions.len().min(MAX_SESSIONS));
        for (session_id, session) in sessions {
            let state = session.lock().await;
            if state.owner == owner {
                summaries.push(render_session_summary(&session_id, &state));
            }
        }
        summaries.sort_by(|left, right| {
            right["session_id"]
                .as_str()
                .cmp(&left["session_id"].as_str())
        });
        summaries.truncate(MAX_SESSIONS);
        Ok(json!({"sessions": summaries}))
    }

    async fn read(&self, owner: &str, session_id: &str) -> Result<Value, ToolError> {
        let session = self.session(owner, session_id).await?;
        let state = session.lock().await;
        Ok(render_session(session_id, &state))
    }

    async fn rename(&self, owner: &str, session_id: &str, title: &str) -> Result<Value, ToolError> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > MAX_SESSION_TITLE_CHARS {
            return Err(invalid("title", "title must contain 1-120 characters"));
        }
        let session = self.session(owner, session_id).await?;
        let backend = {
            let state = session.lock().await;
            if state.closing {
                return Err(unavailable("Phoenix session is closing"));
            }
            state.backend.clone()
        };
        if let SessionBackend::OpenAi {
            session_id,
            backend,
        } = backend
        {
            backend.rename_session(&session_id, title).await?;
        }
        let mut state = session.lock().await;
        state.title = Some(title.to_owned());
        Ok(render_session(session_id, &state))
    }

    async fn close(&self, owner: &str, session_id: &str) -> Result<Value, ToolError> {
        let session = self.session(owner, session_id).await?;
        let sessions = Arc::clone(&self.sessions);
        let session_id = session_id.to_owned();
        tokio::spawn(async move { Self::run_close(sessions, session, session_id).await })
            .await
            .map_err(|_| unavailable("Phoenix close task stopped"))?
    }

    async fn run_close(
        sessions: Arc<Mutex<HashMap<String, Arc<Mutex<Session>>>>>,
        session: Arc<Mutex<Session>>,
        session_id: String,
    ) -> Result<Value, ToolError> {
        let backend = {
            let mut state = session.lock().await;
            if state.turn_in_progress {
                return Err(unavailable(
                    "Phoenix cannot close a session with an active turn",
                ));
            }
            if state.closing {
                return Err(unavailable("Phoenix session is already closing"));
            }
            state.closing = true;
            state.backend.clone()
        };
        let close_result = match backend {
            SessionBackend::Codex { thread_id, runtime } => runtime
                .request("thread/close", json!({"threadId":thread_id}))
                .await
                .map(|_| ()),
            SessionBackend::OpenAi {
                session_id,
                backend,
            } => backend.close_session(&session_id).await,
        };
        if let Err(error) = close_result {
            session.lock().await.closing = false;
            return Err(error);
        }
        let mut sessions = sessions.lock().await;
        if sessions
            .get(&session_id)
            .is_some_and(|current| Arc::ptr_eq(current, &session))
        {
            sessions.remove(&session_id);
        }
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
        if input.len() > MAX_INPUT_BYTES {
            return Err(invalid("input", "input cannot exceed 32768 bytes"));
        }
        let session = self.session(owner, session_id).await?;
        let openai = matches!(session.lock().await.backend, SessionBackend::OpenAi { .. });
        let protocol_inputs = if openai {
            if attachments_present(attachments) {
                return Err(invalid(
                    "attachments",
                    "OpenAI-compatible Phoenix providers currently accept text-only Assistant messages",
                ));
            }
            Vec::new()
        } else {
            turn_inputs(input, attachments)?
        };
        let session_id = session_id.to_owned();
        let input = message_display_text(input, attachments);
        tokio::spawn(
            async move { Self::run_turn(session, session_id, input, protocol_inputs).await },
        )
        .await
        .map_err(|_| unavailable("Phoenix turn task stopped"))?
    }

    async fn run_turn(
        session: Arc<Mutex<Session>>,
        session_id: String,
        input: String,
        protocol_inputs: Vec<Value>,
    ) -> Result<Value, ToolError> {
        let (backend, model, effort) = {
            let mut state = session.lock().await;
            if state.closing {
                return Err(unavailable("Phoenix session is closing"));
            }
            if state.turn_in_progress {
                return Err(unavailable("Phoenix session already has an active turn"));
            }
            state.turn_in_progress = true;
            (
                state.backend.clone(),
                state.model.clone(),
                state.effort.clone(),
            )
        };
        match backend {
            SessionBackend::Codex { thread_id, runtime } => {
                Self::run_codex_turn(
                    session,
                    session_id,
                    input,
                    protocol_inputs,
                    runtime,
                    thread_id,
                    model,
                    effort,
                )
                .await
            }
            SessionBackend::OpenAi {
                session_id: upstream_session_id,
                backend,
            } => {
                Self::run_openai_turn(
                    session,
                    session_id,
                    upstream_session_id,
                    input,
                    backend,
                    model,
                )
                .await
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_codex_turn(
        session: Arc<Mutex<Session>>,
        session_id: String,
        input: String,
        protocol_inputs: Vec<Value>,
        runtime: AppServerRuntime,
        thread_id: String,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<Value, ToolError> {
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
        {
            let mut state = session.lock().await;
            state.active_turn_id = Some(turn_id.clone());
            push_message(&mut state, "user", input);
        }
        let result = tokio::time::timeout(
            TURN_TIMEOUT,
            collect_turn(&mut events, &thread_id, &turn_id, &session),
        )
        .await;
        let result = match result {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                drop(runtime.interrupt(&thread_id, &turn_id).await);
                clear_active_turn(&session).await;
                return Err(error);
            }
            Err(_) => {
                drop(runtime.interrupt(&thread_id, &turn_id).await);
                clear_active_turn(&session).await;
                return Err(unavailable(
                    "Phoenix turn exceeded the five minute runtime limit",
                ));
            }
        };
        let mut state = session.lock().await;
        state.turn_in_progress = false;
        state.active_turn_id = None;
        push_message(&mut state, "assistant", result.output);
        Ok(render_session(&session_id, &state))
    }

    async fn run_openai_turn(
        session: Arc<Mutex<Session>>,
        session_id: String,
        upstream_session_id: String,
        input: String,
        backend: OpenAiBackend,
        model: Option<String>,
    ) -> Result<Value, ToolError> {
        let model =
            model.ok_or_else(|| unavailable("Phoenix OpenAI provider has no selected model"))?;
        let turn_id = format!("openai-{}", uuid::Uuid::new_v4());
        {
            let mut state = session.lock().await;
            state.active_turn_id = Some(turn_id);
            push_message(&mut state, "user", input.clone());
        }
        let result = tokio::time::timeout(
            TURN_TIMEOUT,
            backend.chat(&upstream_session_id, &model, &[ChatMessage::User(&input)]),
        )
        .await;
        let output = match result {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                clear_active_turn(&session).await;
                return Err(error);
            }
            Err(_) => {
                drop(backend.cancel_session(&upstream_session_id).await);
                clear_active_turn(&session).await;
                return Err(unavailable(
                    "Phoenix turn exceeded the five minute runtime limit",
                ));
            }
        };
        let mut state = session.lock().await;
        state.turn_in_progress = false;
        state.active_turn_id = None;
        push_message(&mut state, "assistant", output);
        Ok(render_session(&session_id, &state))
    }

    async fn interrupt(&self, owner: &str, session_id: &str) -> Result<Value, ToolError> {
        let session = self.session(owner, session_id).await?;
        let (backend, turn_id) = {
            let state = session.lock().await;
            let turn_id = state
                .active_turn_id
                .clone()
                .ok_or_else(|| invalid("session_id", "Phoenix session has no active turn"))?;
            (state.backend.clone(), turn_id)
        };
        match backend {
            SessionBackend::Codex { thread_id, runtime } => {
                runtime.interrupt(&thread_id, &turn_id).await?;
            }
            SessionBackend::OpenAi {
                session_id,
                backend,
            } => {
                backend.cancel_session(&session_id).await?;
            }
        }
        Ok(json!({"session_id":session_id,"status":"interrupting","turn_id":turn_id}))
    }

    async fn steer(
        &self,
        owner: &str,
        session_id: &str,
        input: &str,
        attachments: Option<&Value>,
    ) -> Result<Value, ToolError> {
        if input.len() > MAX_INPUT_BYTES {
            return Err(invalid("input", "input cannot exceed 32768 bytes"));
        }
        let display_input = message_display_text(input, attachments);
        let submitted_at_ms = now_millis();
        let session = self.session(owner, session_id).await?;
        let (runtime, thread_id, turn_id) = {
            let state = session.lock().await;
            let turn_id = state
                .active_turn_id
                .clone()
                .ok_or_else(|| invalid("session_id", "Phoenix session has no active turn"))?;
            match &state.backend {
                SessionBackend::Codex { thread_id, runtime } => {
                    (runtime.clone(), thread_id.clone(), turn_id)
                }
                SessionBackend::OpenAi { .. } => {
                    return Err(unavailable(
                        "OpenAI-compatible Phoenix providers do not support steering active turns",
                    ));
                }
            }
        };
        let protocol_inputs = turn_inputs(input, attachments)?;
        let response = runtime
            .request(
                "turn/steer",
                json!({"threadId":thread_id,"expectedTurnId":turn_id,"input":protocol_inputs}),
            )
            .await?;
        {
            let mut state = session.lock().await;
            state.messages.push(Message {
                role: "user",
                text: display_input,
                created_at_ms: submitted_at_ms,
            });
            if state.messages.len() > MAX_MESSAGES {
                let excess = state.messages.len() - MAX_MESSAGES;
                state.messages.drain(..excess);
            }
        }
        Ok(json!({"session_id":session_id,"status":"steered","turn":safe_value(&response)}))
    }

    async fn review(
        &self,
        owner: &str,
        session_id: &str,
        target_type: &str,
        target: Option<String>,
    ) -> Result<Value, ToolError> {
        let review_target = match target_type {
            "uncommitted_changes" | "uncommittedChanges" => json!({"type":"uncommittedChanges"}),
            "base_branch" => json!({"type":"baseBranch","branch":required_target(target)?}),
            "commit" => json!({"type":"commit","sha":required_target(target)?}),
            "custom" => json!({"type":"custom","instructions":required_target(target)?}),
            _ => return Err(invalid("target_type", "unsupported review target")),
        };
        let session = self.session(owner, session_id).await?;
        let session_id = session_id.to_owned();
        tokio::spawn(async move { Self::run_review(session, session_id, review_target).await })
            .await
            .map_err(|_| unavailable("Phoenix review task stopped"))?
    }

    async fn run_review(
        session: Arc<Mutex<Session>>,
        session_id: String,
        review_target: Value,
    ) -> Result<Value, ToolError> {
        let (runtime, thread_id) = {
            let mut state = session.lock().await;
            if state.closing {
                return Err(unavailable("Phoenix session is closing"));
            }
            if state.turn_in_progress {
                return Err(unavailable("Phoenix session already has an active turn"));
            }
            let (runtime, thread_id) = match &state.backend {
                SessionBackend::Codex { thread_id, runtime } => {
                    (runtime.clone(), thread_id.clone())
                }
                SessionBackend::OpenAi { .. } => {
                    return Err(unavailable(
                        "OpenAI-compatible Phoenix providers do not support Codex reviews",
                    ));
                }
            };
            state.turn_in_progress = true;
            (runtime, thread_id)
        };
        let mut events = runtime.subscribe();
        let response = runtime
            .request(
                "review/start",
                json!({"threadId":thread_id,"target":review_target,"delivery":"inline"}),
            )
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                session.lock().await.turn_in_progress = false;
                return Err(error);
            }
        };
        let Some(turn_id) = response
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
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
                drop(runtime.interrupt(&thread_id, &turn_id).await);
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
                    "Phoenix review exceeded the five minute runtime limit",
                ));
            }
        };
        let mut state = session.lock().await;
        state.turn_in_progress = false;
        state.active_turn_id = None;
        if !result.output.is_empty() {
            state.messages.push(Message {
                role: "assistant",
                text: result.output,
                created_at_ms: now_millis(),
            });
        }
        if state.messages.len() > MAX_MESSAGES {
            let excess = state.messages.len() - MAX_MESSAGES;
            state.messages.drain(..excess);
        }
        let mut rendered = render_session(&session_id, &state);
        rendered["review"] = safe_value(&response);
        Ok(rendered)
    }

    async fn diagnostics(&self) -> Result<Value, ToolError> {
        self.require_available()?;
        if self.config.provider == PhoenixProvider::OpenAiCompatible {
            return Err(unavailable(
                "OpenAI-compatible Phoenix providers do not expose Codex diagnostics",
            ));
        }
        let runtime = initialized_app_server(&self.config, &self.local_mcp_url).await?;
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

    fn openai_backend(&self) -> Result<OpenAiBackend, ToolError> {
        self.openai.clone().ok_or_else(|| {
            unavailable(format!(
                "Phoenix OpenAI-compatible provider requires {}",
                crate::dispatch::phoenix_openai::BASE_URL_ENV
            ))
        })
    }

    fn require_available(&self) -> Result<(), ToolError> {
        if self.available() {
            return Ok(());
        }
        let requirement = match self.config.provider {
            PhoenixProvider::CodexAppServer => {
                "an enabled container-local Codex App Server runtime".to_owned()
            }
            PhoenixProvider::OpenAiCompatible => format!(
                "an enabled OpenAI-compatible provider configured with {}",
                crate::dispatch::phoenix_openai::BASE_URL_ENV
            ),
        };
        Err(unavailable(format!("Phoenix requires {requirement}")))
    }
}

impl Default for PhoenixRuntime {
    fn default() -> Self {
        Self::new(PhoenixPreferences::default())
    }
}

fn openai_model_id(model: &Value) -> Option<&str> {
    model.get("id").and_then(Value::as_str)
}

fn select_openai_model(
    models: &[Value],
    requested: Option<&str>,
    configured: Option<&str>,
    effort: Option<&str>,
) -> Result<String, ToolError> {
    let find_id = |candidate: &str| {
        models
            .iter()
            .filter_map(openai_model_id)
            .find(|id| *id == candidate)
            .map(str::to_owned)
    };
    if let Some(requested) = requested {
        return find_id(requested).ok_or_else(|| {
            invalid(
                "model",
                "selected model is not advertised by the OpenAI-compatible provider",
            )
        });
    }
    if let Some(configured) = configured {
        return find_id(configured).ok_or_else(|| {
            invalid(
                "model",
                "configured Phoenix model is not advertised by the OpenAI-compatible provider",
            )
        });
    }
    if let Some(effort) = effort
        && let Some(id) = models.iter().find_map(|model| {
            (model
                .pointer("/gateway/thinking_effort")
                .and_then(Value::as_str)
                == Some(effort))
            .then(|| openai_model_id(model))
            .flatten()
        })
    {
        return Ok(id.to_owned());
    }
    models
        .iter()
        .find_map(openai_model_id)
        .map(str::to_owned)
        .ok_or_else(|| unavailable("OpenAI-compatible Phoenix provider advertised no models"))
}

fn attachments_present(attachments: Option<&Value>) -> bool {
    match attachments {
        None | Some(Value::Null) => false,
        Some(Value::Array(values)) => !values.is_empty(),
        Some(_) => true,
    }
}

fn push_message(session: &mut Session, role: &'static str, text: String) {
    session.messages.push(Message {
        role,
        text,
        created_at_ms: now_millis(),
    });
    if session.messages.len() > MAX_MESSAGES {
        let excess = session.messages.len() - MAX_MESSAGES;
        session.messages.drain(..excess);
    }
}

async fn clear_active_turn(session: &Arc<Mutex<Session>>) {
    let mut state = session.lock().await;
    state.turn_in_progress = false;
    state.active_turn_id = None;
}

fn render_session(session_id: &str, session: &Session) -> Value {
    json!({
        "session_id": session_id,
        "status": "ready",
        "messages": session.messages.iter().map(|message| json!({
            "role": message.role,
            "text": message.text,
            "created_at_ms": message.created_at_ms,
        })).collect::<Vec<_>>(),
        "events": session.events,
        "model": session.model,
        "effort": session.effort,
        "title": session_title(session),
        "turn_status": if session.turn_in_progress { "in_progress" } else if session.closing { "closing" } else { "ready" },
    })
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn render_session_summary(session_id: &str, session: &Session) -> Value {
    let title = session_title(session);
    let preview = session.messages.last().map_or_else(
        || "No messages yet".to_owned(),
        |message| display_text(&message.text, MAX_SESSION_PREVIEW_CHARS),
    );
    json!({
        "session_id": session_id,
        "title": title,
        "preview": preview,
        "model": session.model,
        "effort": session.effort,
        "message_count": session.messages.len(),
        "turn_status": if session.turn_in_progress { "in_progress" } else if session.closing { "closing" } else { "ready" },
    })
}

fn session_title(session: &Session) -> String {
    session.title.clone().unwrap_or_else(|| {
        session
            .messages
            .iter()
            .find(|message| message.role == "user")
            .map_or_else(
                || "New Phoenix session".to_owned(),
                |message| display_text(&message.text, MAX_SESSION_TITLE_CHARS),
            )
    })
}

fn display_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let text = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{text}…")
    } else {
        text
    }
}

struct TurnResult {
    output: String,
}

async fn launch_app_server(
    config: &PhoenixPreferences,
    local_mcp_url: &str,
) -> Result<AppServerRuntime, ToolError> {
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
        args: app_server_args(local_mcp_url),
        env,
        cwd: workspace_root.clone(),
    })
    .await
}

fn app_server_args(local_mcp_url: &str) -> Vec<OsString> {
    vec![
        "app-server".into(),
        "--stdio".into(),
        "-c".into(),
        format!("mcp_servers.labby.url=\"{local_mcp_url}\"").into(),
        "-c".into(),
        format!("mcp_servers.labby.bearer_token_env_var=\"{LOCAL_MCP_TOKEN_ENV}\"").into(),
    ]
}

async fn initialized_app_server(
    config: &PhoenixPreferences,
    local_mcp_url: &str,
) -> Result<AppServerRuntime, ToolError> {
    let runtime = launch_app_server(config, local_mcp_url).await?;
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

fn attachment_name(attachment: &Value) -> Option<String> {
    attachment
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty() && name.len() <= 160 && !name.chars().any(char::is_control))
        .map(str::to_owned)
}

fn message_display_text(input: &str, attachments: Option<&Value>) -> String {
    if !input.trim().is_empty() {
        return input.to_owned();
    }
    let names = attachments
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(attachment_name)
        .take(MAX_ATTACHMENTS)
        .collect::<Vec<_>>();
    if names.is_empty() {
        "Attachment".to_owned()
    } else {
        format!("Attached: {}", names.join(", "))
    }
}

fn turn_inputs(input: &str, attachments: Option<&Value>) -> Result<Vec<Value>, ToolError> {
    let mut values = Vec::new();
    if !input.trim().is_empty() {
        values.push(json!({"type":"text","text":input,"text_elements":[]}));
    }
    let Some(attachments) = attachments else {
        if values.is_empty() {
            return Err(invalid("input", "provide text or at least one attachment"));
        }
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
        let payload = url.split_once(',').map(|(_, payload)| payload);
        let decoded = payload.and_then(|payload| {
            base64::engine::general_purpose::STANDARD
                .decode(payload)
                .ok()
        });
        match kind {
            "image" => {
                let allowed = [
                    "data:image/png;base64,",
                    "data:image/jpeg;base64,",
                    "data:image/webp;base64,",
                ]
                .iter()
                .any(|prefix| url.starts_with(prefix));
                if !allowed
                    || decoded
                        .as_ref()
                        .is_none_or(|bytes| bytes.is_empty() || bytes.len() > MAX_ATTACHMENT_BYTES)
                {
                    return Err(invalid(
                        "attachments",
                        "images must be bounded PNG, JPEG, or WebP data URLs",
                    ));
                }
                values.push(json!({"type":"image","url":url}));
            }
            "audio" => {
                let allowed = [
                    "data:audio/mpeg;base64,",
                    "data:audio/wav;base64,",
                    "data:audio/mp4;base64,",
                    "data:audio/webm;base64,",
                ]
                .iter()
                .any(|prefix| url.starts_with(prefix));
                if !allowed
                    || decoded
                        .as_ref()
                        .is_none_or(|bytes| bytes.is_empty() || bytes.len() > MAX_ATTACHMENT_BYTES)
                {
                    return Err(invalid(
                        "attachments",
                        "audio must be a bounded MP3, WAV, M4A, or WebM data URL",
                    ));
                }
                // Kept for compatibility with deployed App Server builds that advertise audio input.
                values.push(json!({"type":"audio","url":url}));
            }
            "text" => {
                let allowed = url.starts_with("data:text/")
                    || url.starts_with("data:application/json")
                    || url.starts_with("data:application/javascript")
                    || url.starts_with("data:application/xml")
                    || url.starts_with("data:application/yaml")
                    || url.starts_with("data:application/x-yaml");
                let Some(bytes) = decoded
                    .filter(|bytes| !bytes.is_empty() && bytes.len() <= MAX_TEXT_ATTACHMENT_BYTES)
                else {
                    return Err(invalid(
                        "attachments",
                        "text attachments must be valid UTF-8 data URLs no larger than 512 KiB",
                    ));
                };
                if !allowed {
                    return Err(invalid(
                        "attachments",
                        "unsupported text attachment media type",
                    ));
                }
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| invalid("attachments", "text attachments must be UTF-8"))?;
                let name =
                    attachment_name(attachment).unwrap_or_else(|| "attachment.txt".to_owned());
                values.push(json!({
                    "type":"text",
                    "text":format!("Attached file {name}:\n\n{text}"),
                    "text_elements":[]
                }));
            }
            _ => return Err(invalid("attachments", "unsupported attachment type")),
        }
    }
    if values.is_empty() {
        return Err(invalid("input", "provide text or at least one attachment"));
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
    let mut completed_segments: Vec<String> = Vec::new();
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
        if let Some(mut candidate) = sanitized_event(&event) {
            let mut state = session.lock().await;
            state.next_event_sequence = state.next_event_sequence.saturating_add(1);
            candidate["sequence"] = json!(state.next_event_sequence);
            candidate["received_at_ms"] = json!(now_millis());
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
                    let segment = bounded(text);
                    if !segment.is_empty() && completed_segments.last() != Some(&segment) {
                        completed_segments.push(segment);
                    }
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
                let output = if !deltas.is_empty() {
                    // Deltas preserve the exact chronological text stream across multiple
                    // agentMessage items. A single completed item is not necessarily the
                    // whole response and previously caused the beginning of replies to vanish.
                    bounded(&deltas)
                } else {
                    bounded(&completed_segments.concat())
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
            | "item/plan/delta"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/summaryPartAdded"
            | "item/reasoning/textDelta"
            | "item/commandExecution/outputDelta"
            | "item/mcpToolCall/progress"
            | "hook/started"
            | "hook/completed"
            | "model/rerouted"
            | "model/safety/updated"
            | "model/verification/updated"
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
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    #[cfg(feature = "skills")]
    #[tokio::test]
    async fn codex_bootstrap_loads_the_bundled_skill_from_the_registry() {
        let instructions = phoenix_developer_instructions().await.unwrap();
        assert!(instructions.starts_with(PHOENIX_DEVELOPER_INSTRUCTIONS));
        assert!(instructions.contains("skill://labby/using-labby/SKILL.md"));
        assert!(instructions.contains("codemode.listSkills()"));
        assert!(instructions.contains("codemode.getSkill(uri)"));
        assert!(instructions.contains("codemode.readSkill(uri)"));
    }

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
            provider: PhoenixProvider::CodexAppServer,
            command: Some("/missing/codex".into()),
            codex_home: Some("/missing/codex-home".into()),
            workspace_root: Some("/missing/workspace".into()),
            model: None,
        });
        assert!(!runtime.available());
    }

    #[tokio::test]
    async fn openai_compatible_provider_drives_assistant_session_lifecycle() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "object": "list",
                "data": [{
                    "id": "chatgpt-browser-medium",
                    "object": "model",
                    "gateway": {"thinking_effort": "medium"}
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(|request: &wiremock::Request| match request.url.path() {
                "/v1/sessions" => {
                    ResponseTemplate::new(200).set_body_json(json!({"status":"ready"}))
                }
                "/v1/chat/completions" => ResponseTemplate::new(200).set_body_json(json!({
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "EXGPT_ADAPTER_OK"},
                        "finish_reason": "stop"
                    }]
                })),
                path if path.starts_with("/v1/sessions/") && path.ends_with("/close") => {
                    ResponseTemplate::new(200).set_body_json(json!({"status":"closed"}))
                }
                _ => ResponseTemplate::new(404),
            })
            .mount(&server)
            .await;

        let backend = OpenAiBackend::from_url(&format!("{}/v1", server.uri()), None).unwrap();
        let runtime = PhoenixRuntime::new_with_backends(
            PhoenixPreferences {
                enabled: true,
                provider: PhoenixProvider::OpenAiCompatible,
                model: Some("chatgpt-browser-medium".into()),
                ..PhoenixPreferences::default()
            },
            LOCAL_MCP_URL,
            Some(backend),
        );

        let status = runtime
            .dispatch("principal-a", "phoenix.status", json!({}))
            .await
            .unwrap();
        assert_eq!(status["available"], true);
        assert_eq!(status["runtime"], "remote_http");
        assert_eq!(status["service"], "openai-compatible");

        let models = runtime
            .dispatch("principal-a", "phoenix.models.list", json!({}))
            .await
            .unwrap();
        assert_eq!(models["models"][0]["id"], "chatgpt-browser-medium");
        assert_eq!(models["models"][0]["defaultReasoningEffort"], "medium");

        let started = runtime
            .dispatch("principal-a", "phoenix.session.start", json!({}))
            .await
            .unwrap();
        let session_id = started["session_id"].as_str().unwrap();
        let completed = runtime
            .dispatch(
                "principal-a",
                "phoenix.turn.send",
                json!({"session_id": session_id, "input": "ping exgpt"}),
            )
            .await
            .unwrap();
        assert_eq!(completed["messages"][0]["role"], "user");
        assert_eq!(completed["messages"][1]["text"], "EXGPT_ADAPTER_OK");

        let closed = runtime
            .dispatch(
                "principal-a",
                "phoenix.session.close",
                json!({"session_id": session_id}),
            )
            .await
            .unwrap();
        assert_eq!(closed["status"], "closed");
    }

    #[tokio::test]
    #[ignore = "requires a live OpenAI-compatible Phoenix provider"]
    async fn openai_compatible_provider_live_round_trip() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let backend = OpenAiBackend::from_env()
            .expect("set LABBY_PHOENIX_OPENAI_BASE_URL for the live Phoenix provider test");
        let runtime = PhoenixRuntime::new_with_backends(
            PhoenixPreferences {
                enabled: true,
                provider: PhoenixProvider::OpenAiCompatible,
                model: Some("chatgpt-browser-medium".into()),
                ..PhoenixPreferences::default()
            },
            LOCAL_MCP_URL,
            Some(backend),
        );

        let started = runtime
            .dispatch("principal-live", "phoenix.session.start", json!({}))
            .await
            .unwrap();
        let session_id = started["session_id"].as_str().unwrap().to_owned();
        let completed = runtime
            .dispatch(
                "principal-live",
                "phoenix.turn.send",
                json!({
                    "session_id": session_id,
                    "input": "Reply exactly LABBY_PHOENIX_EXGPT_OK"
                }),
            )
            .await
            .unwrap();
        assert_eq!(
            completed["messages"][1]["text"].as_str().map(str::trim),
            Some("LABBY_PHOENIX_EXGPT_OK")
        );

        let closed = runtime
            .dispatch(
                "principal-live",
                "phoenix.session.close",
                json!({"session_id": session_id}),
            )
            .await
            .unwrap();
        assert_eq!(closed["status"], "closed");
    }

    #[test]
    fn app_server_uses_the_effective_nondefault_listener_port() {
        let args = app_server_args("http://127.0.0.1:9876/mcp");
        assert!(
            args.iter()
                .any(|arg| arg == "mcp_servers.labby.url=\"http://127.0.0.1:9876/mcp\"")
        );
        assert!(
            !args
                .iter()
                .any(|arg| arg.to_string_lossy().contains(":8765/mcp"))
        );
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
IFS= read -r thread
printf '%s\n' "$thread" >> '{}'
printf '%s\n' '{{"id":2,"result":{{"thread":{{"id":"thread-container"}}}}}}'
while read turn; do
  printf '%s\n' "$turn" >> '{}'
  id=$(printf '%s' "$turn" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  printf '{{"id":%s,"result":{{"turn":{{"id":"turn-1"}}}}}}\n' "$id"
  printf '%s\n' '{{"method":"turn/plan/updated","params":{{"threadId":"thread-container","turnId":"turn-1","plan":[{{"step":"Inspect health","status":"completed"}}]}}}}'
  printf '%s\n' '{{"method":"thread/tokenUsage/updated","params":{{"threadId":"thread-container","tokenUsage":{{"total":{{"totalTokens":42}}}}}}}}'
  printf '%s\n' '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-container","turnId":"turn-1","itemId":"message-1","delta":"hello from container"}}}}'
  printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"thread-container","turnId":"turn-1","item":{{"id":"message-1","type":"agentMessage","text":"hello from container"}}}}}}'
  printf '%s\n' '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-container","turnId":"turn-1","itemId":"message-2","delta":" and still here"}}}}'
  printf '%s\n' '{{"method":"item/completed","params":{{"threadId":"thread-container","turnId":"turn-1","item":{{"id":"message-2","type":"agentMessage","text":" and still here"}}}}}}'
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
            provider: PhoenixProvider::CodexAppServer,
            command: Some(command),
            codex_home: Some(root.path().to_path_buf()),
            workspace_root: Some(root.path().to_path_buf()),
            model: Some("fixture-model".into()),
        });
        let started = runtime
            .dispatch("principal-a", "phoenix.session.start", json!({}))
            .await
            .unwrap();
        #[cfg(feature = "skills")]
        {
            let requests = fs::read_to_string(&capture).unwrap();
            let thread_start: Value =
                serde_json::from_str(requests.lines().nth(2).unwrap()).unwrap();
            let instructions = thread_start["params"]["developerInstructions"]
                .as_str()
                .unwrap();
            assert!(instructions.contains("name: using-labby"));
            assert!(instructions.contains("codemode.listSkills()"));
        }
        let session_id = started["session_id"].as_str().unwrap();
        let initial_list = runtime
            .dispatch("principal-a", "phoenix.session.list", json!({}))
            .await
            .unwrap();
        assert_eq!(initial_list["sessions"][0]["session_id"], session_id);
        assert_eq!(initial_list["sessions"][0]["title"], "New Phoenix session");
        assert_eq!(initial_list["sessions"][0]["preview"], "No messages yet");
        assert_eq!(initial_list["sessions"][0]["message_count"], 0);
        assert_eq!(initial_list["sessions"][0]["turn_status"], "ready");
        assert!(!initial_list.to_string().contains("thread-container"));
        assert_eq!(
            runtime
                .dispatch("principal-b", "phoenix.session.list", json!({}))
                .await
                .unwrap()["sessions"],
            json!([])
        );
        let completed = runtime
            .dispatch(
                "principal-a",
                "phoenix.turn.send",
                json!({"session_id":session_id,"input":"hello"}),
            )
            .await
            .unwrap();
        assert_eq!(
            completed["messages"][1]["text"],
            "hello from container and still here"
        );
        assert_eq!(completed["events"][0]["method"], "turn/plan/updated");
        assert_eq!(
            completed["events"][1]["method"],
            "thread/tokenUsage/updated"
        );
        assert_eq!(completed["events"][2]["method"], "item/agentMessage/delta");
        let completed_list = runtime
            .dispatch("principal-a", "phoenix.session.list", json!({}))
            .await
            .unwrap();
        assert_eq!(completed_list["sessions"][0]["title"], "hello");
        assert_eq!(
            completed_list["sessions"][0]["preview"],
            "hello from container and still here"
        );
        assert_eq!(completed_list["sessions"][0]["message_count"], 2);
        let resumed = runtime
            .dispatch(
                "principal-a",
                "phoenix.turn.send",
                json!({"session_id":session_id,"input":"again"}),
            )
            .await
            .unwrap();
        assert_eq!(
            resumed["messages"][3]["text"],
            "hello from container and still here"
        );
        let renamed = runtime
            .dispatch(
                "principal-a",
                "phoenix.session.rename",
                json!({"session_id":session_id,"title":"Gateway investigation"}),
            )
            .await
            .unwrap();
        assert_eq!(renamed["title"], "Gateway investigation");
        assert_eq!(
            runtime
                .dispatch("principal-a", "phoenix.session.list", json!({}))
                .await
                .unwrap()["sessions"][0]["title"],
            "Gateway investigation"
        );
        let reviewed = runtime
            .dispatch(
                "principal-a",
                "phoenix.review.start",
                json!({"session_id":session_id,"target_type":"uncommitted_changes"}),
            )
            .await
            .unwrap();
        assert_eq!(reviewed["turn_status"], "ready");
        assert_eq!(reviewed["messages"][4]["role"], "assistant");
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
        assert!(requests.contains("\"method\":\"review/start\""));
        assert!(!requests.contains("excludeTurns"));
        assert!(requests.contains("\"experimentalApi\":true"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_turn_admission_does_not_create_a_phantom_message() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let command = root.path().join("codex-fixture");
        fs::write(
            &command,
            r#"#!/bin/sh
read initialize
printf '%s\n' '{"id":1,"result":{"userAgent":"fixture"}}'
read initialized
read thread
printf '%s\n' '{"id":2,"result":{"thread":{"id":"thread-container"}}}'
read turn
id=$(printf '%s' "$turn" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
printf '{"id":%s,"error":{"code":-32000,"message":"rejected"}}\n' "$id"
"#,
        )
        .unwrap();
        fs::set_permissions(&command, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = PhoenixRuntime::new(PhoenixPreferences {
            enabled: true,
            provider: PhoenixProvider::CodexAppServer,
            command: Some(command),
            codex_home: Some(root.path().to_path_buf()),
            workspace_root: Some(root.path().to_path_buf()),
            model: None,
        });
        let started = runtime
            .dispatch("principal-a", "phoenix.session.start", json!({}))
            .await
            .unwrap();
        let session_id = started["session_id"].as_str().unwrap();

        assert!(
            runtime
                .dispatch(
                    "principal-a",
                    "phoenix.turn.send",
                    json!({"session_id":session_id,"input":"not admitted"}),
                )
                .await
                .is_err()
        );
        let state = runtime
            .dispatch(
                "principal-a",
                "phoenix.session.read",
                json!({"session_id":session_id}),
            )
            .await
            .unwrap();
        assert_eq!(state["messages"], json!([]));
        assert_eq!(state["turn_status"], "ready");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn abandoned_send_finishes_cleanup_and_session_can_be_closed() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let command = root.path().join("codex-fixture");
        fs::write(
            &command,
            r#"#!/bin/sh
read initialize
printf '%s\n' '{"id":1,"result":{"userAgent":"fixture"}}'
read initialized
read thread
printf '%s\n' '{"id":2,"result":{"thread":{"id":"thread-container"}}}'
while read request; do
  id=$(printf '%s' "$request" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$request" in
    *turn/start*)
      printf '{"id":%s,"result":{"turn":{"id":"turn-abandoned"}}}\n' "$id"
      sleep 0.05
      printf '%s\n' '{"method":"item/agentMessage/delta","params":{"threadId":"thread-container","turnId":"turn-abandoned","delta":"completed after caller left"}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"thread-container","turn":{"id":"turn-abandoned","status":"completed"}}}' ;;
    *) printf '{"id":%s,"result":{}}\n' "$id" ;;
  esac
done
"#,
        )
        .unwrap();
        fs::set_permissions(&command, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = PhoenixRuntime::new(PhoenixPreferences {
            enabled: true,
            provider: PhoenixProvider::CodexAppServer,
            command: Some(command),
            codex_home: Some(root.path().to_path_buf()),
            workspace_root: Some(root.path().to_path_buf()),
            model: None,
        });
        let started = runtime
            .dispatch("principal-a", "phoenix.session.start", json!({}))
            .await
            .unwrap();
        let session_id = started["session_id"].as_str().unwrap().to_owned();
        let abandoned = tokio::spawn({
            let runtime = runtime.clone();
            let session_id = session_id.clone();
            async move {
                runtime
                    .dispatch(
                        "principal-a",
                        "phoenix.turn.send",
                        json!({"session_id":session_id,"input":"keep cleaning up"}),
                    )
                    .await
            }
        });
        loop {
            let state = runtime
                .dispatch(
                    "principal-a",
                    "phoenix.session.read",
                    json!({"session_id":session_id}),
                )
                .await
                .unwrap();
            if state["turn_status"] == "in_progress" {
                break;
            }
            tokio::task::yield_now().await;
        }
        abandoned.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let recovered = runtime
            .dispatch(
                "principal-a",
                "phoenix.session.read",
                json!({"session_id":session_id}),
            )
            .await
            .unwrap();
        assert_eq!(recovered["turn_status"], "ready");
        assert_eq!(
            recovered["messages"][1]["text"],
            "completed after caller left"
        );
        assert_eq!(
            runtime
                .dispatch(
                    "principal-a",
                    "phoenix.session.close",
                    json!({"session_id":session_id}),
                )
                .await
                .unwrap()["status"],
            "closed"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn session_capacity_is_reserved_before_process_launch() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let command = root.path().join("codex-fixture");
        fs::write(&command, "#!/bin/sh\nexit 99\n").unwrap();
        fs::set_permissions(&command, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = PhoenixRuntime::new(PhoenixPreferences {
            enabled: true,
            provider: PhoenixProvider::CodexAppServer,
            command: Some(command),
            codex_home: Some(root.path().to_path_buf()),
            workspace_root: Some(root.path().to_path_buf()),
            model: None,
        });
        let mut reservations = Vec::new();
        for _ in 0..MAX_SESSIONS {
            reservations.push(Arc::clone(&runtime.capacity).try_acquire_owned().unwrap());
        }

        let error = runtime
            .dispatch("principal-a", "phoenix.session.start", json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "executor_unavailable");
        drop(reservations);
        assert_eq!(runtime.capacity.available_permits(), MAX_SESSIONS);
    }

    #[test]
    fn turn_inputs_accept_bounded_media_text_files_and_attachment_only_turns() {
        let inputs = turn_inputs(
            "inspect",
            Some(&json!([{
                "type":"image", "name":"screen.png", "url":"data:image/png;base64,aGVsbG8="
            }])),
        )
        .unwrap();
        assert_eq!(inputs[1]["type"], "image");
        let text_only = turn_inputs(
            "",
            Some(&json!([{
                "type":"text", "name":"notes.md", "url":"data:text/plain;base64,IyBOb3Rlcw=="
            }])),
        )
        .unwrap();
        assert_eq!(text_only.len(), 1);
        assert_eq!(text_only[0]["type"], "text");
        assert!(text_only[0]["text"].as_str().unwrap().contains("notes.md"));
        assert_eq!(
            message_display_text("", Some(&json!([{"name":"notes.md"}]))),
            "Attached: notes.md"
        );
        assert!(turn_inputs("", None).is_err());
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
            json!([
                "text",
                "image_data_url",
                "text_file_data_url",
                "audio_data_url"
            ])
        );
        assert_eq!(
            status["capabilities"]["turn_lifecycle"],
            json!(["start", "steer", "interrupt", "completed"])
        );
        assert_eq!(status["capabilities"]["operations"], json!(["review"]));
        assert_eq!(
            status["capabilities"]["session_lifecycle"],
            json!(["list", "start", "read", "close"])
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
        assert!(
            sanitized_event(&json!({"method":"hook/started","params":{"turnId":"turn-1"}}))
                .is_some()
        );
        assert!(
            sanitized_event(
                &json!({"method":"item/reasoning/textDelta","params":{"delta":"thinking"}})
            )
            .is_some()
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

    #[test]
    fn session_summary_text_is_single_line_unicode_safe_and_bounded() {
        assert_eq!(display_text("  one\n two\tthree ", 20), "one two three");
        assert_eq!(display_text("🦀🦀🦀", 2), "🦀🦀…");
    }
}
