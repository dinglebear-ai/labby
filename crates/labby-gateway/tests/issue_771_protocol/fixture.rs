//! In-memory upstream fixture. No claimed persistence or gateway ownership proof.

use rmcp::{
    RoleClient, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CancelTaskParams, ClientCapabilities, ClientInfo,
        ClientJsonRpcMessage, CreateTaskResult, ErrorData, GetTaskParams, GetTaskResult,
        Implementation, ServerCapabilities, ServerInfo, SubscriptionFilter, Task, UpdateTaskParams,
    },
    service::RequestContext,
    task_manager::{TaskExit, TaskManager, TaskOptions},
    transport::{IntoTransport, Transport},
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{task::JoinHandle, time::timeout};
use tokio_util::sync::CancellationToken;

pub(super) const DEADLINE: Duration = Duration::from_secs(3);
pub(super) const TASKS: &str = "io.modelcontextprotocol/tasks";

pub(super) fn client_info() -> ClientInfo {
    ClientInfo::new(
        ClientCapabilities::builder().enable_tasks().build(),
        Implementation::new("lane-a-fixture", "1"),
    )
}

pub(super) fn metadata(tasks: bool) -> Value {
    json!({"io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": if tasks {
            serde_json::to_value(ClientCapabilities::builder().enable_tasks().build()).unwrap()
        } else { json!({}) }})
}

pub(super) fn detailed(status: &str) -> Value {
    let mut task = json!({"taskId": "fixture-native-opaque-id", "status": status,
        "createdAt": "2026-09-29T00:00:00Z", "lastUpdatedAt": "2026-09-29T00:00:00Z",
        "ttlMs": 60000, "pollIntervalMs": 25});
    match status {
        "completed" => task["result"] = json!({"resultType": "complete", "content": []}),
        "failed" => task["error"] = json!({"code": -32603, "message": "fixture failure"}),
        "input_required" => {
            task["inputRequests"] = json!({"roots-1": {"method": "roots/list", "params": {}}})
        }
        _ => {}
    }
    task
}

pub(super) async fn exchange(
    wire: &mut impl Transport<RoleClient>,
    id: i64,
    method: &str,
    mut params: Value,
    tasks: bool,
) -> Value {
    params["_meta"] = metadata(tasks);
    let request: ClientJsonRpcMessage = serde_json::from_value(
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
    )
    .unwrap();
    timeout(DEADLINE, wire.send(request))
        .await
        .unwrap()
        .unwrap();
    let response = timeout(DEADLINE, wire.receive())
        .await
        .unwrap()
        .expect("response");
    let response = serde_json::to_value(response).unwrap();
    assert_eq!(response["id"], id);
    response
}

#[derive(Clone)]
struct TaskFixture {
    tasks: TaskManager,
    seed: Task,
    advertise: bool,
    fail_create: bool,
}

impl ServerHandler for TaskFixture {
    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(requested.clone())
    }

    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder().enable_tools();
        ServerInfo::new(if self.advertise {
            capabilities.enable_tasks().build()
        } else {
            capabilities.build()
        })
    }

    async fn call_tool(
        &self,
        _: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if self.fail_create {
            return Err(ErrorData::internal_error("fixture creation rejected", None));
        }
        // Deliberately omit capability preflight to test RMCP's result guard.
        // The seed already exists; no execution side effect happens here.
        Ok(CallToolResponse::Task(CreateTaskResult::new(
            self.seed.clone(),
        )))
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        _: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        Ok(GetTaskResult::new(self.tasks.get_task(&request.task_id)?))
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.tasks.get_task(&request.task_id)?;
        // Accepted no-op, not Lane D input processing.
        Ok(())
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.tasks.get_task(&request.task_id)?;
        // A cooperative acknowledgement need not change the task state.
        Ok(())
    }
}

pub(super) fn task_wire(
    advertise: bool,
    fail_create: bool,
) -> (impl Transport<RoleClient>, FixtureServer) {
    let (stream, server) = task_stream(advertise, fail_create);
    (
        IntoTransport::<RoleClient, _, _>::into_transport(stream),
        server,
    )
}

pub(super) fn task_stream(
    advertise: bool,
    fail_create: bool,
) -> (tokio::io::DuplexStream, FixtureServer) {
    let tasks = TaskManager::new();
    let seed = tasks.spawn(TaskOptions::new().with_poll_interval_ms(25), |ctx| {
        Box::pin(async move {
            ctx.cancelled().await;
            Err::<rmcp::model::CallToolResult, _>(TaskExit::Cancelled)
        })
    });
    let fixture = TaskFixture {
        tasks: tasks.clone(),
        seed: seed.clone(),
        advertise,
        fail_create,
    };
    let (server_transport, client_transport) = tokio::io::duplex(16384);
    let stop = CancellationToken::new();
    let server_stop = stop.clone();
    let handle = tokio::spawn(async move {
        let service = fixture
            .serve_with_ct(server_transport, server_stop)
            .await
            .expect("serve fixture");
        service.waiting().await.expect("fixture stopped");
    });
    let server = FixtureServer {
        handle,
        stop,
        tasks,
        task_id: seed.task_id,
    };
    (client_transport, server)
}

pub(super) struct FixtureServer {
    handle: JoinHandle<()>,
    stop: CancellationToken,
    tasks: TaskManager,
    task_id: String,
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        // Also clean up on a test assertion failure; these resources are fixture-owned.
        drop(self.tasks.cancel_task(&self.task_id));
        self.stop.cancel();
        self.handle.abort();
    }
}

pub(super) async fn close(wire: &mut impl Transport<RoleClient>, server: FixtureServer) {
    server.stop.cancel();
    timeout(DEADLINE, wire.close()).await.unwrap().unwrap();
    finish(server).await;
}

pub(super) async fn finish(mut server: FixtureServer) {
    server.stop.cancel();
    server
        .tasks
        .cancel_task(&server.task_id)
        .expect("stop fixture task");
    timeout(DEADLINE, &mut server.handle)
        .await
        .unwrap()
        .unwrap();
}
