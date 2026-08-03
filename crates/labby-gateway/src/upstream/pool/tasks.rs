//! Routing for task handles returned by upstream MCP servers.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use rmcp::RoleServer;
use rmcp::model::{
    CallToolResponse, CancelTaskParams, GetTaskParams, GetTaskResult, UpdateTaskParams,
};
use rmcp::service::Peer;

use super::UpstreamPool;
use super::relay::RelayCachedConnection;

const TASK_ROUTE_IDLE_TTL: Duration = Duration::from_hours(24);
const TASK_ROUTE_MAX_ENTRIES: usize = 4096;
const TASK_NOTIFICATION_DELIVERY_GRACE: Duration = Duration::from_millis(500);
static TASK_HANDLE_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(super) struct TaskRoute {
    upstream_name: String,
    native_task_id: String,
    caller_subject: Option<String>,
    connection: RelayCachedConnection,
    last_used: Instant,
}

fn mint_task_handle() -> String {
    format!(
        "labby-task-{:016x}",
        TASK_HANDLE_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn task_not_found() -> String {
    "task not found".to_string()
}

fn prune_task_routes(routes: &mut std::collections::HashMap<String, TaskRoute>) {
    routes.retain(|_, route| {
        route.last_used.elapsed() < TASK_ROUTE_IDLE_TTL
            && !route.connection.peer.is_transport_closed()
    });
    while routes.len() >= TASK_ROUTE_MAX_ENTRIES {
        let Some(oldest) = routes
            .iter()
            .min_by_key(|(_, route)| route.last_used)
            .map(|(id, _)| id.clone())
        else {
            break;
        };
        routes.remove(&oldest);
    }
}

impl UpstreamPool {
    /// Convert an upstream task handle into a gateway-owned, subject-bound
    /// handle. The relay connection that created the task is moved out of the
    /// general relay cache and retained for the task lifecycle.
    pub async fn register_task_response(
        &self,
        relay_key: &(String, u64, Option<String>),
        caller_subject: Option<&str>,
        response: CallToolResponse,
    ) -> CallToolResponse {
        let CallToolResponse::Task(mut created) = response else {
            return response;
        };
        let native_task_id = created.task.task_id.clone();
        let Some(connection) = self.relay_connections.write().await.remove(relay_key) else {
            tracing::warn!(
                upstream = %relay_key.0,
                native_task_id = %native_task_id,
                "upstream returned a task but its relay connection was unavailable"
            );
            return CallToolResponse::Task(created);
        };

        let gateway_task_id = mint_task_handle();
        created.task.task_id = gateway_task_id.clone();
        let pending = connection
            .routes
            .register_task_id(&native_task_id, &gateway_task_id)
            .await;
        connection.flush_task_status_notifications(pending).await;
        let mut routes = self.task_routes.write().await;
        prune_task_routes(&mut routes);
        routes.insert(
            gateway_task_id,
            TaskRoute {
                upstream_name: relay_key.0.clone(),
                native_task_id,
                caller_subject: caller_subject.map(str::to_owned),
                connection,
                last_used: Instant::now(),
            },
        );
        CallToolResponse::Task(created)
    }

    fn authorize_task_route(route: &TaskRoute, caller_subject: Option<&str>) -> Result<(), String> {
        if route.caller_subject.as_deref() == caller_subject {
            Ok(())
        } else {
            Err(task_not_found())
        }
    }

    pub async fn get_task_routed(
        &self,
        mut params: GetTaskParams,
        caller_subject: Option<&str>,
        downstream: Peer<RoleServer>,
    ) -> Result<GetTaskResult, String> {
        let gateway_task_id = params.task_id.clone();
        let (peer, native_task_id, upstream_name) = {
            let mut routes = self.task_routes.write().await;
            prune_task_routes(&mut routes);
            let route = routes
                .get_mut(&gateway_task_id)
                .ok_or_else(task_not_found)?;
            Self::authorize_task_route(route, caller_subject)?;
            route.connection.rebind_downstream(downstream).await;
            route.last_used = Instant::now();
            (
                route.connection.peer.clone(),
                route.native_task_id.clone(),
                route.upstream_name.clone(),
            )
        };
        params.task_id = native_task_id;
        let mut result = peer
            .get_task(params)
            .await
            .map_err(|error| format!("upstream `{upstream_name}` tasks/get failed: {error}"))?;
        result.task.task.task_id = gateway_task_id;
        Ok(result)
    }

    pub async fn update_task_routed(
        &self,
        mut params: UpdateTaskParams,
        caller_subject: Option<&str>,
        gateway_task_id: &str,
        downstream: Peer<RoleServer>,
    ) -> Result<(), String> {
        let (peer, native_task_id, upstream_name, relay_routes) = {
            let mut routes = self.task_routes.write().await;
            prune_task_routes(&mut routes);
            let route = routes.get_mut(gateway_task_id).ok_or_else(task_not_found)?;
            Self::authorize_task_route(route, caller_subject)?;
            route.connection.rebind_downstream(downstream).await;
            route.last_used = Instant::now();
            (
                route.connection.peer.clone(),
                route.native_task_id.clone(),
                route.upstream_name.clone(),
                Arc::clone(&route.connection.routes),
            )
        };
        let notification_sequence = relay_routes.task_notification_sequence();
        params.task_id = native_task_id;
        peer.update_task(params)
            .await
            .map_err(|error| format!("upstream `{upstream_name}` tasks/update failed: {error}"))?;
        relay_routes
            .wait_for_task_notification_after(
                notification_sequence,
                TASK_NOTIFICATION_DELIVERY_GRACE,
            )
            .await;
        Ok(())
    }

    pub async fn cancel_task_routed(
        &self,
        mut params: CancelTaskParams,
        caller_subject: Option<&str>,
        gateway_task_id: &str,
        downstream: Peer<RoleServer>,
    ) -> Result<(), String> {
        let (peer, native_task_id, upstream_name, relay_routes) = {
            let mut routes = self.task_routes.write().await;
            prune_task_routes(&mut routes);
            let route = routes.get_mut(gateway_task_id).ok_or_else(task_not_found)?;
            Self::authorize_task_route(route, caller_subject)?;
            route.connection.rebind_downstream(downstream).await;
            route.last_used = Instant::now();
            (
                route.connection.peer.clone(),
                route.native_task_id.clone(),
                route.upstream_name.clone(),
                Arc::clone(&route.connection.routes),
            )
        };
        let notification_sequence = relay_routes.task_notification_sequence();
        params.task_id = native_task_id;
        peer.cancel_task(params)
            .await
            .map_err(|error| format!("upstream `{upstream_name}` tasks/cancel failed: {error}"))?;
        relay_routes
            .wait_for_task_notification_after(
                notification_sequence,
                TASK_NOTIFICATION_DELIVERY_GRACE,
            )
            .await;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use rmcp::model::{
        CallToolResponse, CancelTaskParams, ClientCapabilities, ClientInfo, CreateTaskResult,
        DetailedTask, GetTaskParams, GetTaskResult, InputResponses, ProtocolVersion,
        ServerCapabilities, ServerInfo, Task, TaskPayload, TaskStatus, UpdateTaskParams,
    };
    use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RequestContext, RunningService};
    use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
    use tokio::sync::Mutex;

    use super::super::relay::{
        RelayCachedConnection, RelayClientHandler, RelayRouteState, capability_fingerprint,
    };
    use super::super::{UpstreamConnection, UpstreamPool};

    const NATIVE_TASK_ID: &str = "native-task-1";

    #[derive(Clone, Default)]
    struct TaskServer {
        updates: Arc<Mutex<Vec<String>>>,
        cancellations: Arc<Mutex<Vec<String>>>,
    }

    impl ServerHandler for TaskServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_tasks()
                    .build(),
            )
        }

        async fn get_task(
            &self,
            request: GetTaskParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetTaskResult, ErrorData> {
            let task = Task::new(
                request.task_id.clone(),
                TaskStatus::Working,
                "2026-07-31T00:00:00Z",
                "2026-07-31T00:00:01Z",
            )
            .with_status_message(format!("native:{}", request.task_id));
            Ok(GetTaskResult::new(DetailedTask::new(
                task,
                TaskPayload::Working,
            )))
        }

        async fn update_task(
            &self,
            request: UpdateTaskParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<(), ErrorData> {
            self.updates.lock().await.push(request.task_id);
            Ok(())
        }

        async fn cancel_task(
            &self,
            request: CancelTaskParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<(), ErrorData> {
            self.cancellations.lock().await.push(request.task_id);
            Ok(())
        }
    }

    #[derive(Clone)]
    struct DownstreamServer;

    impl ServerHandler for DownstreamServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }
    }

    async fn task_pool() -> (
        UpstreamPool,
        TaskServer,
        RunningService<RoleServer, DownstreamServer>,
        (String, u64, Option<String>),
    ) {
        let capabilities = ClientCapabilities::builder().enable_tasks().build();

        let (downstream_server_transport, downstream_client_transport) =
            tokio::io::duplex(64 * 1024);
        let mut downstream_client_info = ClientInfo::default();
        downstream_client_info.capabilities = capabilities.clone();
        tokio::spawn(async move {
            if let Ok(running) = downstream_client_info
                .serve_with_lifecycle(
                    downstream_client_transport,
                    ClientLifecycleMode::Discover {
                        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                    },
                )
                .await
            {
                running.waiting().await.ok();
            }
        });
        let downstream_server = DownstreamServer
            .serve(downstream_server_transport)
            .await
            .expect("downstream server connects");
        let downstream = downstream_server.peer().clone();

        let server = TaskServer::default();
        let server_clone = server.clone();
        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(async move {
            let running = server_clone
                .serve(server_transport)
                .await
                .expect("task server starts");
            running.waiting().await.expect("task server runs");
        });
        let routes = Arc::new(RelayRouteState::default());
        let (notification_tx, _receiver) = tokio::sync::broadcast::channel(1);
        let handler = RelayClientHandler::new_with_routes(
            downstream,
            Arc::from("task-upstream"),
            capabilities.clone(),
            Arc::clone(&routes),
            notification_tx,
            false,
        );
        let client_service = handler
            .serve_with_lifecycle(
                client_transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .expect("task relay client connects");
        let peer = client_service.peer().clone();
        let connection = RelayCachedConnection {
            _connection: UpstreamConnection::new(
                client_service,
                Some(server_task),
                peer.clone(),
                Default::default(),
            ),
            peer,
            capability_fingerprint: capability_fingerprint(&capabilities),
            routes,
            cancellation_sender: None,
            last_used: Instant::now(),
        };

        let key = ("task-upstream".to_string(), 7, None);
        let pool = UpstreamPool::new();
        pool.relay_connections
            .write()
            .await
            .insert(key.clone(), connection);
        (pool, server, downstream_server, key)
    }

    fn create_task_response() -> CallToolResponse {
        CallToolResponse::Task(CreateTaskResult::new(Task::new(
            NATIVE_TASK_ID,
            TaskStatus::Working,
            "2026-07-31T00:00:00Z",
            "2026-07-31T00:00:00Z",
        )))
    }

    #[tokio::test]
    async fn routed_task_lifecycle_uses_gateway_handle_and_native_upstream_id() {
        let (pool, server, downstream, relay_key) = task_pool().await;

        let registered = pool
            .register_task_response(&relay_key, Some("alice"), create_task_response())
            .await;
        let CallToolResponse::Task(created) = registered else {
            panic!("expected task response");
        };
        let gateway_task_id = created.task.task_id.clone();
        assert_ne!(gateway_task_id, NATIVE_TASK_ID);
        assert!(gateway_task_id.starts_with("labby-task-"));

        let denied = pool
            .get_task_routed(
                GetTaskParams::new(&gateway_task_id),
                Some("bob"),
                downstream.peer().clone(),
            )
            .await;
        assert!(denied.is_err(), "task handles must remain subject-bound");

        let task = pool
            .get_task_routed(
                GetTaskParams::new(&gateway_task_id),
                Some("alice"),
                downstream.peer().clone(),
            )
            .await
            .expect("owner polls task");
        assert_eq!(task.task.task.task_id, gateway_task_id);
        assert_eq!(
            task.task.task.status_message.as_deref(),
            Some("native:native-task-1")
        );

        pool.update_task_routed(
            UpdateTaskParams::new(&gateway_task_id, InputResponses::new()),
            Some("alice"),
            &gateway_task_id,
            downstream.peer().clone(),
        )
        .await
        .expect("owner updates task");
        pool.cancel_task_routed(
            CancelTaskParams::new(&gateway_task_id),
            Some("alice"),
            &gateway_task_id,
            downstream.peer().clone(),
        )
        .await
        .expect("owner cancels task");

        assert_eq!(server.updates.lock().await.as_slice(), [NATIVE_TASK_ID]);
        assert_eq!(
            server.cancellations.lock().await.as_slice(),
            [NATIVE_TASK_ID]
        );
    }
}
