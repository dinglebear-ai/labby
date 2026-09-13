//! Persistent JSONL transport for a container-local Codex App Server.
//!
//! This module owns one child process per Phoenix session. Requests are
//! multiplexed by JSON-RPC id while notifications are broadcast to turn
//! consumers, allowing a turn to be observed and interrupted while it runs.

use std::{
    collections::HashMap,
    ffi::OsString,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, broadcast, oneshot},
    task::JoinHandle,
};

use crate::dispatch::error::ToolError;

const EVENT_CAPACITY: usize = 256;
const MAX_PROTOCOL_LINE_BYTES: usize = 1024 * 1024;

/// Operator-owned process launch configuration. Secret values are passed only
/// through the child environment and never serialized into protocol events.
pub(crate) struct LaunchSpec {
    pub(crate) command: PathBuf,
    pub(crate) args: Vec<OsString>,
    pub(crate) env: Vec<(OsString, OsString)>,
    pub(crate) cwd: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct AppServerEvent(pub(crate) Value);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, ToolError>>>>>;

/// A persistent App Server connection. Clones share the same process and can
/// issue requests concurrently.
#[derive(Clone)]
pub(crate) struct AppServerRuntime {
    stdin: Arc<Mutex<ChildStdin>>,
    _child: Arc<Mutex<Child>>,
    pending: Pending,
    events: broadcast::Sender<AppServerEvent>,
    next_id: Arc<AtomicU64>,
    _reader: Arc<JoinHandle<()>>,
}

impl AppServerRuntime {
    pub(crate) async fn launch(spec: LaunchSpec) -> Result<Self, ToolError> {
        let mut command = Command::new(&spec.command);
        command
            .args(spec.args)
            .env_clear()
            .envs(spec.env)
            .current_dir(spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|_| unavailable("Container-local Codex App Server could not be started"))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().ok_or_else(protocol_error)?));
        let stdout = child.stdout.take().ok_or_else(protocol_error)?;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let reader_pending = pending.clone();
        let reader_events = events.clone();
        let reader_stdin = stdin.clone();
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                let line = match lines.next_line().await {
                    Ok(Some(line)) if line.len() <= MAX_PROTOCOL_LINE_BYTES => line,
                    _ => break,
                };
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    break;
                };
                if let Some(id) = value.get("id").and_then(Value::as_u64)
                    && value.get("method").is_none()
                {
                    if let Some(sender) = reader_pending.lock().await.remove(&id) {
                        let result = if let Some(error) = value.get("error") {
                            Err(app_server_error(error))
                        } else {
                            value.get("result").cloned().ok_or_else(protocol_error)
                        };
                        drop(sender.send(result));
                    }
                } else if value.get("method").is_some() && value.get("id").is_none() {
                    drop(reader_events.send(AppServerEvent(value)));
                } else if value.get("method").is_some()
                    && let Some(id) = value.get("id").cloned()
                {
                    let rejection = json!({
                        "id": id,
                        "error": {
                            "code": -32601,
                            "message": "Phoenix does not permit interactive App Server requests"
                        }
                    });
                    if let Ok(mut bytes) = serde_json::to_vec(&rejection) {
                        bytes.push(b'\n');
                        drop(reader_stdin.lock().await.write_all(&bytes).await);
                    }
                }
            }
            let waiters = std::mem::take(&mut *reader_pending.lock().await);
            for (_, sender) in waiters {
                drop(sender.send(Err(unavailable("Container-local Codex App Server stopped"))));
            }
        });
        Ok(Self {
            stdin,
            _child: Arc::new(Mutex::new(child)),
            pending,
            events,
            next_id: Arc::new(AtomicU64::new(1)),
            _reader: Arc::new(reader),
        })
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<AppServerEvent> {
        self.events.subscribe()
    }

    pub(crate) async fn notify(&self, method: &str, params: Value) -> Result<(), ToolError> {
        self.write(&json!({"method":method,"params":params})).await
    }

    pub(crate) async fn request(&self, method: &str, params: Value) -> Result<Value, ToolError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        if let Err(error) = self
            .write(&json!({"id":id,"method":method,"params":params}))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        receiver
            .await
            .map_err(|_| unavailable("Container-local Codex App Server stopped"))?
    }

    pub(crate) async fn interrupt(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<Value, ToolError> {
        self.request(
            "turn/interrupt",
            json!({"threadId":thread_id,"turnId":turn_id}),
        )
        .await
    }

    async fn write(&self, value: &Value) -> Result<(), ToolError> {
        let mut bytes = serde_json::to_vec(value).map_err(|_| protocol_error())?;
        bytes.push(b'\n');
        if bytes.len() > MAX_PROTOCOL_LINE_BYTES {
            return Err(protocol_error());
        }
        self.stdin
            .lock()
            .await
            .write_all(&bytes)
            .await
            .map_err(|_| protocol_error())
    }
}

fn protocol_error() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "decode_error".into(),
        message: "Container-local Codex App Server returned an invalid protocol response".into(),
    }
}

fn unavailable(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "executor_unavailable".into(),
        message: message.into(),
    }
}

fn app_server_error(error: &Value) -> ToolError {
    unavailable(
        error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Codex App Server request failed"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    #[tokio::test]
    async fn multiplexes_requests_broadcasts_events_and_interrupts_without_restarting() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let command = root.path().join("fixture");
        let capture = root.path().join("requests.jsonl");
        fs::write(
            &command,
            format!(
                r#"#!/bin/sh
while read request; do
  printf '%s\n' "$request" >> '{}'
  id=$(printf '%s' "$request" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$request" in
    *turn/start*)
      printf '%s\n' '{{"method":"item/started","params":{{"turnId":"turn-1"}}}}'
      printf '{{"id":%s,"result":{{"turn":{{"id":"turn-1"}}}}}}\n' "$id" ;;
    *turn/interrupt*) printf '{{"id":%s,"result":{{}}}}\n' "$id" ;;
    *) printf '{{"id":%s,"result":{{"ok":true}}}}\n' "$id" ;;
  esac
done
"#,
                capture.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&command, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = AppServerRuntime::launch(LaunchSpec {
            command,
            args: vec![],
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            cwd: root.path().to_path_buf(),
        })
        .await
        .unwrap();
        let mut events = runtime.subscribe();
        let (first, second) = tokio::join!(
            runtime.request("first", json!({})),
            runtime.request("turn/start", json!({}))
        );
        assert_eq!(first.unwrap()["ok"], true);
        assert_eq!(second.unwrap()["turn"]["id"], "turn-1");
        assert_eq!(events.recv().await.unwrap().0["method"], "item/started");
        runtime.interrupt("thread-1", "turn-1").await.unwrap();
        drop(runtime);
        let requests = fs::read_to_string(capture).unwrap();
        assert_eq!(requests.matches("turn/start").count(), 1);
        assert_eq!(requests.matches("turn/interrupt").count(), 1);
    }
}
