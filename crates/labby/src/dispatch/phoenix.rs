//! Container-local Codex App Server adapter for the Phoenix assistant.

use std::{collections::HashMap, process::Stdio, sync::Arc, time::Duration};

use labby_primitives::action::{ActionSpec, ParamSpec};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{ChildStdout, Command},
    sync::Mutex,
};

use crate::{config::PhoenixPreferences, dispatch::error::ToolError};

const TURN_TIMEOUT: Duration = Duration::from_mins(5);
const MAX_INPUT_BYTES: usize = 32 * 1024;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_MESSAGES: usize = 100;

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
    action(
        "phoenix.session.start",
        "Start a caller-scoped Phoenix session",
        &[],
    ),
    action(
        "phoenix.session.read",
        "Read a caller-scoped Phoenix session",
        &[param("session_id", true)],
    ),
    action(
        "phoenix.turn.send",
        "Send a message to a caller-scoped Phoenix session",
        &[param("session_id", true), param("input", true)],
    ),
];

#[derive(Clone, Debug)]
struct Message {
    role: &'static str,
    text: String,
}

#[derive(Clone, Debug)]
struct Session {
    owner: String,
    thread_id: Option<String>,
    messages: Vec<Message>,
}

#[derive(Clone)]
pub(crate) struct PhoenixRuntime {
    config: PhoenixPreferences,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
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
            "phoenix.session.start" => self.start(owner).await,
            "phoenix.session.read" => self.read(owner, &required(&params, "session_id")?).await,
            "phoenix.turn.send" => {
                let session_id = required(&params, "session_id")?;
                let input = required(&params, "input")?;
                self.send(owner, &session_id, &input).await
            }
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
        })
    }

    async fn start(&self, owner: &str) -> Result<Value, ToolError> {
        self.require_available()?;
        let session_id = format!("phoenix-{}", uuid::Uuid::new_v4());
        self.sessions.lock().await.insert(
            session_id.clone(),
            Session {
                owner: owner.to_owned(),
                thread_id: None,
                messages: Vec::new(),
            },
        );
        Ok(json!({"session_id":session_id,"status":"ready","messages":[]}))
    }

    async fn read(&self, owner: &str, session_id: &str) -> Result<Value, ToolError> {
        let sessions = self.sessions.lock().await;
        let session = authorized_session(&sessions, owner, session_id)?;
        Ok(render_session(session_id, session))
    }

    async fn send(&self, owner: &str, session_id: &str, input: &str) -> Result<Value, ToolError> {
        self.require_available()?;
        if input.trim().is_empty() || input.len() > MAX_INPUT_BYTES {
            return Err(invalid("input", "input must contain 1-32768 bytes"));
        }
        let mut sessions = self.sessions.lock().await;
        let session = authorized_session_mut(&mut sessions, owner, session_id)?;
        let result = tokio::time::timeout(
            TURN_TIMEOUT,
            run_codex_turn(&self.config, session.thread_id.as_deref(), input),
        )
        .await
        .map_err(|_| unavailable("Phoenix turn exceeded the five minute runtime limit"))??;
        session.thread_id = Some(result.thread_id);
        session.messages.push(Message {
            role: "user",
            text: input.to_owned(),
        });
        session.messages.push(Message {
            role: "assistant",
            text: result.output,
        });
        if session.messages.len() > MAX_MESSAGES {
            let excess = session.messages.len() - MAX_MESSAGES;
            session.messages.drain(..excess);
        }
        Ok(render_session(session_id, session))
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

fn authorized_session<'a>(
    sessions: &'a HashMap<String, Session>,
    owner: &str,
    session_id: &str,
) -> Result<&'a Session, ToolError> {
    sessions
        .get(session_id)
        .filter(|session| session.owner == owner)
        .ok_or_else(denied)
}

fn authorized_session_mut<'a>(
    sessions: &'a mut HashMap<String, Session>,
    owner: &str,
    session_id: &str,
) -> Result<&'a mut Session, ToolError> {
    sessions
        .get_mut(session_id)
        .filter(|session| session.owner == owner)
        .ok_or_else(denied)
}

fn render_session(session_id: &str, session: &Session) -> Value {
    json!({
        "session_id": session_id,
        "status": "ready",
        "messages": session.messages.iter().map(|message| json!({
            "role": message.role,
            "text": message.text,
        })).collect::<Vec<_>>(),
    })
}

struct TurnResult {
    thread_id: String,
    output: String,
}

async fn run_codex_turn(
    config: &PhoenixPreferences,
    existing_thread_id: Option<&str>,
    input: &str,
) -> Result<TurnResult, ToolError> {
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
    let command_path = command.parent().map_or_else(
        || "/usr/local/bin:/usr/bin:/bin".into(),
        |directory| format!("{}:/usr/local/bin:/usr/bin:/bin", directory.display()),
    );
    let mut child = Command::new(command)
        .args(["app-server", "--stdio"])
        .env_clear()
        .env("HOME", codex_home)
        .env("CODEX_HOME", codex_home)
        .env("PATH", command_path)
        .env("LANG", "C.UTF-8")
        .current_dir(workspace_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| unavailable("Container-local Codex App Server could not be started"))?;
    let mut stdin = child.stdin.take().ok_or_else(protocol_error)?;
    let stdout = child.stdout.take().ok_or_else(protocol_error)?;
    let mut lines = BufReader::new(stdout).lines();

    write_message(&mut stdin, json!({
        "method":"initialize","id":1,
        "params":{"clientInfo":{"name":"labby_phoenix","title":"Labby Phoenix","version":env!("CARGO_PKG_VERSION")}}
    })).await?;
    response(&mut stdin, &mut lines, 1).await?;
    write_message(&mut stdin, json!({"method":"initialized","params":{}})).await?;

    let start_params = if let Some(thread_id) = existing_thread_id {
        json!({
            "threadId": thread_id,
            "cwd": workspace_root,
            "approvalPolicy": "never",
            "sandbox": "read-only",
        })
    } else {
        let mut params = json!({
            "cwd": workspace_root,
            "approvalPolicy": "never",
            "sandbox": "read-only",
            "serviceName": "labby-phoenix",
            "threadSource": "appServer",
            "developerInstructions": "You are Phoenix, Labby's concise operator assistant. You run only inside the Labby container. Treat the workspace as read-only, never request access to another machine or device, and explain any action that requires an operator.",
        });
        if let Some(model) = &config.model {
            params["model"] = Value::String(model.clone());
        }
        params
    };
    let method = if existing_thread_id.is_some() {
        "thread/resume"
    } else {
        "thread/start"
    };
    write_message(
        &mut stdin,
        json!({"method":method,"id":2,"params":start_params}),
    )
    .await?;
    let start = response(&mut stdin, &mut lines, 2).await?;
    let thread_id = start
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .ok_or_else(protocol_error)?
        .to_owned();

    write_message(&mut stdin, json!({
        "method":"turn/start","id":3,
        "params":{"threadId":thread_id,"input":[{"type":"text","text":input,"text_elements":[]}]}
    })).await?;

    let mut turn_id = None;
    let mut deltas = String::new();
    let mut completed_text = String::new();
    let mut completed = None;
    while completed.is_none() || turn_id.is_none() {
        let value = next_message(&mut lines).await?;
        if value.get("id").and_then(Value::as_u64) == Some(3) {
            if let Some(error) = value.get("error") {
                return Err(app_server_error(error));
            }
            turn_id = value
                .pointer("/result/turn/id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            continue;
        }
        if value.get("id").is_some() && value.get("method").is_some() {
            reject_server_request(&mut stdin, &value).await?;
            continue;
        }
        match value.get("method").and_then(Value::as_str) {
            Some("item/agentMessage/delta") => {
                if let Some(delta) = value.pointer("/params/delta").and_then(Value::as_str)
                    && deltas.len().saturating_add(delta.len()) <= MAX_OUTPUT_BYTES
                {
                    deltas.push_str(delta);
                }
            }
            Some("item/completed") => {
                if value.pointer("/params/item/type").and_then(Value::as_str)
                    == Some("agentMessage")
                    && let Some(text) = value.pointer("/params/item/text").and_then(Value::as_str)
                {
                    completed_text = bounded(text);
                }
            }
            Some("turn/completed") => completed = value.get("params").cloned(),
            _ => {}
        }
    }
    let completed = completed.ok_or_else(protocol_error)?;
    let status = completed
        .pointer("/turn/status")
        .and_then(Value::as_str)
        .unwrap_or("failed");
    if status != "completed" {
        let message = completed
            .pointer("/turn/error/message")
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
    drop(stdin);
    drop(child.kill().await);
    Ok(TurnResult { thread_id, output })
}

async fn write_message(
    stdin: &mut tokio::process::ChildStdin,
    value: Value,
) -> Result<(), ToolError> {
    let mut bytes = serde_json::to_vec(&value).map_err(|_| protocol_error())?;
    bytes.push(b'\n');
    stdin.write_all(&bytes).await.map_err(|_| protocol_error())
}

async fn next_message(lines: &mut Lines<BufReader<ChildStdout>>) -> Result<Value, ToolError> {
    let line = lines
        .next_line()
        .await
        .map_err(|_| protocol_error())?
        .ok_or_else(protocol_error)?;
    serde_json::from_str(&line).map_err(|_| protocol_error())
}

async fn response(
    stdin: &mut tokio::process::ChildStdin,
    lines: &mut Lines<BufReader<ChildStdout>>,
    id: u64,
) -> Result<Value, ToolError> {
    loop {
        let value = next_message(lines).await?;
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            if let Some(error) = value.get("error") {
                return Err(app_server_error(error));
            }
            return value.get("result").cloned().ok_or_else(protocol_error);
        }
        if value.get("id").is_some() && value.get("method").is_some() {
            reject_server_request(stdin, &value).await?;
        }
    }
}

async fn reject_server_request(
    stdin: &mut tokio::process::ChildStdin,
    request: &Value,
) -> Result<(), ToolError> {
    write_message(stdin, json!({
        "id": request.get("id"),
        "error": {"code":-32601,"message":"Phoenix does not permit interactive App Server requests"}
    })).await
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

fn app_server_error(error: &Value) -> ToolError {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Codex App Server request failed");
    unavailable(message)
}

#[cfg(test)]
mod tests {
    use super::*;
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
read turn
printf '%s\n' "$turn" >> '{}'
printf '%s\n' '{{"id":3,"result":{{"turn":{{"id":"turn-1"}}}}}}'
printf '%s\n' '{{"method":"item/agentMessage/delta","params":{{"threadId":"thread-container","turnId":"turn-1","itemId":"message-1","delta":"hello from container"}}}}'
printf '%s\n' '{{"method":"turn/completed","params":{{"threadId":"thread-container","turn":{{"id":"turn-1","status":"completed","items":[],"error":null}}}}}}'
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
        assert!(requests.contains("\"method\":\"thread/resume\""));
        assert!(!requests.contains("excludeTurns"));
    }
}
