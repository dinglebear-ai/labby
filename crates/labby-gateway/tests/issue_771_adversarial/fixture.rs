//! Independent HTTP upstream and downstream fixtures; not a production task engine.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use labby_gateway::upstream::pool::{TaskRouteAuthorization, TaskRouteStore, UpstreamPool};
use labby_runtime::gateway_config::UpstreamConfig;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ClientCapabilities, ClientInfo, CreateTaskResult,
    DetailedTask, DiscoverResult, GetTaskParams, GetTaskResult, ProtocolVersion, RequestId,
    ServerCapabilities, Task, TaskPayload, TaskStatus, TaskStatusNotificationParams,
};
use rmcp::service::{
    ClientLifecycleMode, ClientServiceExt, NotificationContext, Peer, RunningService,
};
use rmcp::{ClientHandler, RoleClient, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

pub(crate) const OWNER: &str = "lane-f-owner";
pub(crate) const OAUTH_OWNER: &str = "lane-f-credential-owner";
pub(crate) const UPSTREAM: &str = "lane-f-http";
pub(crate) const BUDGET: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct BackendState {
    pub(crate) creates: usize,
    pub(crate) task_calls: Vec<(String, String)>,
    pub(crate) notifications: usize,
    pub(crate) unavailable: bool,
    pub(crate) changed_hint: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Backend(pub(crate) Arc<Mutex<BackendState>>);

impl Backend {
    pub(crate) fn task_call_count(&self) -> usize {
        self.0.lock().unwrap().task_calls.len()
    }

    fn task(id: &str, changed_hint: bool) -> Task {
        Task::new(
            id,
            TaskStatus::Working,
            "2026-07-31T00:00:00Z",
            if changed_hint {
                "2026-07-31T00:00:01Z"
            } else {
                "2026-07-31T00:00:00Z"
            },
        )
    }
}

impl Respond for Backend {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).expect("JSON-RPC request");
        let method = body["method"].as_str().expect("method");
        let mut state = self.0.lock().unwrap();
        if state.unavailable {
            return ResponseTemplate::new(503);
        }
        let mut task_for_notification = None;
        let result = match method {
            "server/discover" => serde_json::to_value(DiscoverResult::new(
                vec![ProtocolVersion::V_2026_07_28],
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_tasks()
                    .build(),
            ))
            .unwrap(),
            "tools/list" => json!({"resultType":"complete", "tools":[
                {"name":"start", "inputSchema":{"type":"object"}},
                {"name":"sync", "inputSchema":{"type":"object"}}
            ]}),
            "tools/call" if body["params"]["name"] == "sync" => {
                json!({"resultType":"complete", "content":[]})
            }
            "tools/call" => {
                state.creates += 1;
                let task = Self::task(&format!("native-task-{}", state.creates), false);
                task_for_notification = Some(task.clone());
                serde_json::to_value(CreateTaskResult::new(task)).unwrap()
            }
            "tasks/get" | "tasks/update" | "tasks/cancel" => {
                let id = body["params"]["taskId"].as_str().expect("native task ID");
                state.task_calls.push((method.to_owned(), id.to_owned()));
                let exists = id
                    .strip_prefix("native-task-")
                    .and_then(|n| n.parse::<usize>().ok())
                    .is_some_and(|n| n > 0 && n <= state.creates);
                if !exists {
                    return ResponseTemplate::new(200).set_body_json(json!({
                        "jsonrpc":"2.0", "id":body["id"],
                        "error":{"code":-32602,"message":"task not found"}
                    }));
                }
                if method == "tasks/get" {
                    let task = Self::task(id, state.changed_hint);
                    task_for_notification = Some(task.clone());
                    serde_json::to_value(GetTaskResult::new(DetailedTask::new(
                        task,
                        TaskPayload::Working,
                    )))
                    .unwrap()
                } else {
                    // Cooperative acknowledgements do not invent a terminal state.
                    json!({"resultType":"complete"})
                }
            }
            "ping" => json!({"resultType":"complete"}),
            other => panic!("unexpected fixture request: {other}"),
        };
        let response = json!({"jsonrpc":"2.0", "id":body["id"], "result":result});
        if let Some(task) = task_for_notification.filter(|_| state.notifications > 0) {
            let notification = json!({"jsonrpc":"2.0", "method":"notifications/tasks",
                "params":TaskStatusNotificationParams::new(DetailedTask::new(task, TaskPayload::Working))});
            let mut events = String::new();
            for _ in 0..state.notifications {
                events.push_str(&format!("event: message\ndata: {notification}\n\n"));
            }
            events.push_str(&format!("event: message\ndata: {response}\n\n"));
            ResponseTemplate::new(200).set_body_raw(events, "text/event-stream")
        } else {
            ResponseTemplate::new(200).set_body_json(response)
        }
    }
}

#[derive(Clone)]
pub(crate) struct Inbox(mpsc::Sender<TaskStatusNotificationParams>);
impl ClientHandler for Inbox {
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        info.capabilities = capabilities();
        info
    }
    async fn on_task_status(
        &self,
        params: TaskStatusNotificationParams,
        _: NotificationContext<RoleClient>,
    ) {
        self.0
            .send(params)
            .await
            .expect("bounded test notification inbox remains open");
    }
}
#[derive(Clone)]
pub(crate) struct DownstreamServer;
impl ServerHandler for DownstreamServer {}

pub(crate) struct Downstream {
    pub(crate) peer: Peer<RoleServer>,
    pub(crate) inbox: mpsc::Receiver<TaskStatusNotificationParams>,
    client: Option<RunningService<RoleClient, Inbox>>,
    server: Option<RunningService<RoleServer, DownstreamServer>>,
}
impl Downstream {
    pub(crate) async fn new() -> Self {
        let (tx, inbox) = mpsc::channel(32);
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let (server, client) = tokio::join!(
            DownstreamServer.serve(server_io),
            Inbox(tx).serve_with_lifecycle(
                client_io,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                }
            )
        );
        let server = server.expect("downstream server");
        Self {
            peer: server.peer().clone(),
            inbox,
            server: Some(server),
            client: Some(client.expect("downstream client")),
        }
    }
    pub(crate) async fn disconnect(&mut self) {
        if let Some(client) = self.client.take() {
            client.cancel().await.ok();
        }
        if let Some(server) = self.server.take() {
            server.cancel().await.ok();
        }
        assert!(self.peer.is_transport_closed());
    }
    pub(crate) async fn notification(&mut self) -> TaskStatusNotificationParams {
        tokio::time::timeout(BUDGET, self.inbox.recv())
            .await
            .expect("notification deadline")
            .expect("notification")
    }
}

pub(crate) fn capabilities() -> ClientCapabilities {
    ClientCapabilities::builder().enable_tasks().build()
}

pub(crate) struct Harness {
    pub(crate) backend: Backend,
    pub(crate) pool: Arc<UpstreamPool>,
    pub(crate) config: UpstreamConfig,
    pub(crate) directory: tempfile::TempDir,
    pub(crate) server: MockServer,
    sequence: AtomicU64,
}
impl Harness {
    pub(crate) async fn new() -> Self {
        let server = MockServer::start().await;
        let backend = Backend::default();
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(backend.clone())
            .mount(&server)
            .await;
        let config: UpstreamConfig = serde_json::from_value(json!({
            "name":UPSTREAM, "url":format!("{}/mcp", server.uri()),
            "proxy_resources":false, "proxy_prompts":false
        }))
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store = TaskRouteStore::open(directory.path().join("routes.db"))
            .await
            .unwrap();
        let pool = Arc::new(
            UpstreamPool::new()
                .with_task_route_store(Arc::new(store))
                .with_request_timeout(Duration::from_secs(2))
                .with_relay_timeout(BUDGET),
        );
        pool.seed_lazy_upstreams(std::slice::from_ref(&config))
            .await;
        Self {
            backend,
            pool,
            config,
            directory,
            server,
            sequence: AtomicU64::new(1),
        }
    }
    pub(crate) fn database(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.directory.path().join("routes.db")).unwrap()
    }
    pub(crate) fn row_count(&self) -> i64 {
        self.database()
            .query_row("SELECT COUNT(*) FROM task_routes", [], |row| row.get(0))
            .unwrap()
    }
    pub(crate) async fn call(
        &self,
        downstream: &Downstream,
        tool: &'static str,
        caller: Option<&str>,
        oauth: Option<&str>,
        authorization: TaskRouteAuthorization,
    ) -> Result<CallToolResponse, String> {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst);
        tokio::time::timeout(
            BUDGET,
            self.pool.call_tool_relayed(
                &self.config,
                oauth,
                CallToolRequestParams::new(tool),
                downstream.peer.clone(),
                RequestId::String(format!("lane-f-{sequence}").into()),
                CancellationToken::new(),
                sequence,
                capabilities(),
                caller,
                authorization,
            ),
        )
        .await
        .expect("bounded tool relay")
        .expect("upstream connects")
        .map_err(|error| format!("{error:?}"))
    }
    pub(crate) async fn create(&self, downstream: &Downstream, oauth: Option<&str>) -> String {
        let result = self
            .call(
                downstream,
                "start",
                Some(OWNER),
                oauth,
                TaskRouteAuthorization::root(),
            )
            .await
            .unwrap();
        let CallToolResponse::Task(created) = result else {
            panic!("task acknowledgement expected")
        };
        assert_eq!(
            self.row_count(),
            self.backend.0.lock().unwrap().creates as i64
        );
        created.task.task_id
    }
    pub(crate) async fn get(
        &self,
        downstream: &Downstream,
        id: &str,
    ) -> Result<GetTaskResult, String> {
        tokio::time::timeout(
            BUDGET,
            self.pool.get_task_routed(
                GetTaskParams::new(id),
                Some(OWNER),
                &TaskRouteAuthorization::root(),
                downstream.peer.clone(),
            ),
        )
        .await
        .expect("bounded task poll")
    }
    pub(crate) async fn replace_pool(&mut self) {
        self.pool.drain_for_swap("lane-f.reload").await;
        let store = TaskRouteStore::open(self.directory.path().join("routes.db"))
            .await
            .unwrap();
        self.pool = Arc::new(
            UpstreamPool::new()
                .with_task_route_store(Arc::new(store))
                .with_request_timeout(Duration::from_secs(2))
                .with_relay_timeout(BUDGET),
        );
        self.pool
            .seed_lazy_upstreams(std::slice::from_ref(&self.config))
            .await;
    }
}
