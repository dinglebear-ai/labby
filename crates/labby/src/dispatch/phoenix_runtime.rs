//! Persistent JSONL transport for a container-local Codex App Server.
//!
//! This module owns a persistent Codex App Server child shared by Phoenix sessions. Requests
//! are multiplexed by JSON-RPC id while notifications are broadcast to turn consumers,
//! allowing independent threads to be observed and interrupted without process-per-session churn.

use std::{
    collections::HashMap,
    ffi::OsString,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, broadcast, mpsc, oneshot},
    task::JoinHandle,
};

use crate::dispatch::error::ToolError;

const EVENT_CAPACITY: usize = 256;
const THREAD_EVENT_CAPACITY: usize = 128;
const MAX_PROTOCOL_LINE_BYTES: usize = 1024 * 1024;
#[cfg(not(test))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(test)]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

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
type ThreadEventRoutes = Arc<Mutex<HashMap<String, mpsc::Sender<AppServerEvent>>>>;

fn event_thread_id(event: &Value) -> Option<&str> {
    event
        .pointer("/params/threadId")
        .or_else(|| event.pointer("/params/thread/id"))
        .and_then(Value::as_str)
}

async fn publish_event(
    global: &broadcast::Sender<AppServerEvent>,
    routes: &ThreadEventRoutes,
    event: AppServerEvent,
) {
    if let Some(thread_id) = event_thread_id(&event.0) {
        let sender = routes.lock().await.get(thread_id).cloned();
        if let Some(sender) = sender {
            drop(sender.send(event.clone()).await);
        }
    }
    drop(global.send(event));
}

/// A persistent App Server connection. Clones share the same process and can
/// issue requests concurrently.
#[derive(Clone)]
pub(crate) struct AppServerRuntime {
    stdin: Arc<Mutex<ChildStdin>>,
    _child: Arc<Mutex<Child>>,
    pending: Pending,
    events: broadcast::Sender<AppServerEvent>,
    thread_events: ThreadEventRoutes,
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
        let child = Arc::new(Mutex::new(child));
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let thread_events: ThreadEventRoutes = Arc::new(Mutex::new(HashMap::new()));
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let reader_pending = pending.clone();
        let reader_events = events.clone();
        let reader_thread_events = thread_events.clone();
        let reader_stdin = stdin.clone();
        let reader_child = child.clone();
        let reader = tokio::spawn(async move {
            let mut output = BufReader::new(stdout);
            loop {
                let mut frame = Vec::new();
                let read = match (&mut output)
                    .take((MAX_PROTOCOL_LINE_BYTES + 2) as u64)
                    .read_until(b'\n', &mut frame)
                    .await
                {
                    Ok(read) => read,
                    Err(_) => break,
                };
                if read == 0 {
                    break;
                }
                if frame.last() != Some(&b'\n') || frame.len() > MAX_PROTOCOL_LINE_BYTES + 1 {
                    break;
                }
                frame.pop();
                if frame.last() == Some(&b'\r') {
                    frame.pop();
                }
                let Ok(value) = serde_json::from_slice::<Value>(&frame) else {
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
                    publish_event(&reader_events, &reader_thread_events, AppServerEvent(value))
                        .await;
                } else if value.get("method").is_some()
                    && let Some(id) = value.get("id").cloned()
                {
                    let method = value
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let category = request_category(method);
                    let mut decline_params = json!({
                        "requestMethod":method,
                        "category":category,
                        "decision":"declined"
                    });
                    if let Some(thread_id) = event_thread_id(&value) {
                        decline_params["threadId"] = Value::String(thread_id.to_owned());
                    }
                    publish_event(
                        &reader_events,
                        &reader_thread_events,
                        AppServerEvent(json!({
                            "method":"phoenix/serverRequestDeclined",
                            "params":decline_params
                        })),
                    )
                    .await;
                    let rejection = json!({
                        "id": id,
                        "error": {
                            "code": -32001,
                            "message": "Phoenix declined this App Server request under its read-only, never-approval policy",
                            "data":{"category":category,"decision":"declined"}
                        }
                    });
                    if let Ok(mut bytes) = serde_json::to_vec(&rejection) {
                        bytes.push(b'\n');
                        drop(reader_stdin.lock().await.write_all(&bytes).await);
                    }
                }
            }
            drop(reader_child.lock().await.kill().await);
            let waiters = std::mem::take(&mut *reader_pending.lock().await);
            for (_, sender) in waiters {
                drop(sender.send(Err(unavailable("Container-local Codex App Server stopped"))));
            }
        });
        Ok(Self {
            stdin,
            _child: child,
            pending,
            events,
            thread_events,
            next_id: Arc::new(AtomicU64::new(1)),
            _reader: Arc::new(reader),
        })
    }

    #[cfg(test)]
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<AppServerEvent> {
        self.events.subscribe()
    }

    pub(crate) async fn subscribe_thread(&self, thread_id: &str) -> mpsc::Receiver<AppServerEvent> {
        let (sender, receiver) = mpsc::channel(THREAD_EVENT_CAPACITY);
        self.thread_events
            .lock()
            .await
            .insert(thread_id.to_owned(), sender);
        receiver
    }

    pub(crate) async fn forget_thread(&self, thread_id: &str) {
        self.thread_events.lock().await.remove(thread_id);
    }

    pub(crate) async fn is_running(&self) -> bool {
        !self._reader.is_finished()
            && self
                ._child
                .lock()
                .await
                .try_wait()
                .is_ok_and(|status| status.is_none())
    }

    #[cfg(test)]
    pub(crate) fn shares_process(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self._child, &other._child)
    }

    pub(crate) async fn notify(&self, method: &str, params: Value) -> Result<(), ToolError> {
        self.write(&json!({"method":method,"params":params})).await
    }

    pub(crate) async fn request(&self, method: &str, params: Value) -> Result<Value, ToolError> {
        let runtime = self.clone();
        let method = method.to_owned();
        tokio::spawn(async move { runtime.request_owned(&method, params).await })
            .await
            .map_err(|_| unavailable("Container-local Codex App Server request task stopped"))?
    }

    async fn request_owned(&self, method: &str, params: Value) -> Result<Value, ToolError> {
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
        match tokio::time::timeout(REQUEST_TIMEOUT, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(unavailable("Container-local Codex App Server stopped")),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(unavailable(
                    "Container-local Codex App Server request exceeded its 30 second limit",
                ))
            }
        }
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

fn request_category(method: &str) -> &'static str {
    let lower = method.to_ascii_lowercase();
    if lower.contains("approval") || lower.contains("permission") {
        "approval"
    } else if lower.contains("elicitation") || lower.contains("userinput") {
        "user_input"
    } else if lower.contains("tool") {
        "tool"
    } else {
        "interactive"
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;

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
      printf '%s\n' '{{"method":"item/started","params":{{"threadId":"thread-1","turnId":"turn-1"}}}}'
      printf '{{"id":%s,"result":{{"turn":{{"id":"turn-1"}}}}}}\n' "$id" ;;
    *turn/interrupt*) printf '{{"id":%s,"result":{{}}}}\n' "$id" ;;
    *trigger/request*)
      printf '%s\n' '{{"id":"server-1","method":"item/commandExecution/requestApproval","params":{{"command":"rm -rf /"}}}}'
      printf '{{"id":%s,"result":{{"ok":true}}}}\n' "$id" ;;
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
        let mut thread_events = runtime.subscribe_thread("thread-1").await;
        let mut other_thread_events = runtime.subscribe_thread("thread-2").await;
        let (first, second) = tokio::join!(
            runtime.request("first", json!({})),
            runtime.request("turn/start", json!({"threadId":"thread-1"}))
        );
        assert_eq!(first.unwrap()["ok"], true);
        assert_eq!(second.unwrap()["turn"]["id"], "turn-1");
        assert_eq!(events.recv().await.unwrap().0["method"], "item/started");
        assert_eq!(
            thread_events.recv().await.unwrap().0["method"],
            "item/started"
        );
        assert!(other_thread_events.try_recv().is_err());
        runtime.interrupt("thread-1", "turn-1").await.unwrap();
        runtime.request("trigger/request", json!({})).await.unwrap();
        let declined = events.recv().await.unwrap().0;
        assert_eq!(declined["method"], "phoenix/serverRequestDeclined");
        assert_eq!(declined["params"]["category"], "approval");
        drop(runtime);
        let requests = fs::read_to_string(capture).unwrap();
        assert_eq!(requests.matches("turn/start").count(), 1);
        assert_eq!(requests.matches("turn/interrupt").count(), 1);
    }

    #[tokio::test]
    async fn abandoned_requests_time_out_and_release_pending_capacity() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let command = root.path().join("fixture");
        fs::write(&command, "#!/bin/sh\nwhile read request; do :; done\n").unwrap();
        fs::set_permissions(&command, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = AppServerRuntime::launch(LaunchSpec {
            command,
            args: vec![],
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            cwd: root.path().to_path_buf(),
        })
        .await
        .unwrap();

        let request = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.request("never/responds", json!({})).await }
        });
        while runtime.pending.lock().await.is_empty() {
            tokio::task::yield_now().await;
        }
        request.abort();
        tokio::time::sleep(REQUEST_TIMEOUT + Duration::from_millis(50)).await;

        assert!(runtime.pending.lock().await.is_empty());
    }

    #[tokio::test]
    async fn oversized_protocol_output_without_a_newline_is_rejected_at_the_frame_limit() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let command = root.path().join("fixture");
        fs::write(
            &command,
            format!(
                "#!/bin/sh\nhead -c {} /dev/zero | tr '\\0' x\nsleep 5\n",
                MAX_PROTOCOL_LINE_BYTES + 2
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

        let error = runtime.request("oversized", json!({})).await.unwrap_err();
        assert!(
            matches!(error, ToolError::Sdk { ref sdk_kind, .. } if sdk_kind == "executor_unavailable")
        );
        assert!(runtime.pending.lock().await.is_empty());
        assert!(!runtime.is_running().await);
    }
}
