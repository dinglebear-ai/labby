//! Routing for task handles returned by upstream MCP servers.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rmcp::model::{CancelTaskParams, GetTaskParams, GetTaskResult, UpdateTaskParams};
use rmcp::service::Peer;
use rmcp::{RoleClient, RoleServer};

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::capability_call::timed_capability_call_str;
use super::helpers::estimate_task_response_size;
use super::logging::{UpstreamRequestLog, log_upstream_request_start};
use super::relay_cache::RelayCachedConnection;
use super::task_route::TaskRouteAuthorization;
use super::task_route_store::TaskRouteRecord;

#[cfg(test)]
#[path = "task_registration_tests.rs"]
mod registration_tests;
#[cfg(test)]
#[path = "tasks_review_tests.rs"]
mod review_tests;

#[path = "task_registration.rs"]
mod registration;
const TASK_NOTIFICATION_DELIVERY_GRACE: Duration = Duration::from_millis(500);

pub(super) struct TaskRoute {
    record: TaskRouteRecord,
    connection: RelayCachedConnection,
}

fn mint_task_handle() -> String {
    format!("labby-task-{}", uuid::Uuid::new_v4().simple())
}

fn unix_millis_now() -> Result<i64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock precedes Unix epoch: {error}"))?
        .as_millis();
    i64::try_from(millis).map_err(|_| "current timestamp exceeds SQLite range".to_string())
}

fn task_not_found() -> String {
    "task not found".to_string()
}

fn prune_task_routes(routes: &mut std::collections::HashMap<String, TaskRoute>) {
    let now = jiff::Timestamp::now().as_millisecond();
    routes.retain(|_, route| {
        !route.record.expired(now) && !route.connection.peer.is_transport_closed()
    });
}

impl UpstreamPool {
    pub(super) async fn invalidate_task_routes_for_oauth_subject(
        &self,
        upstream: &str,
        subject: &str,
        reason: &'static str,
    ) -> usize {
        self.invalidate_oauth_task_routes(reason, |route| {
            route.record.upstream_name == upstream
                && route.record.oauth_subject.as_deref() == Some(subject)
        })
        .await
    }

    pub(super) async fn invalidate_all_oauth_task_routes(&self, reason: &'static str) -> usize {
        self.invalidate_oauth_task_routes(reason, |route| route.record.oauth_subject.is_some())
            .await
    }

    pub(super) async fn invalidate_oauth_task_routes_for_upstreams(
        &self,
        upstreams: &HashSet<&str>,
        reason: &'static str,
    ) -> usize {
        self.invalidate_oauth_task_routes(reason, |route| {
            route.record.oauth_subject.is_some()
                && upstreams.contains(route.record.upstream_name.as_str())
        })
        .await
    }

    async fn invalidate_oauth_task_routes(
        &self,
        reason: &'static str,
        should_remove: impl Fn(&TaskRoute) -> bool,
    ) -> usize {
        let removed = {
            let mut routes = self.task_routes.write().await;
            let ids = routes
                .iter()
                .filter(|(_, route)| should_remove(route))
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| routes.remove(&id))
                .collect::<Vec<_>>()
        };
        let count = removed.len();
        futures::future::join_all(removed.into_iter().map(|route| async move {
            let upstream_name = route.record.upstream_name;
            route
                .connection
                ._connection
                .shutdown(&upstream_name, reason)
                .await
        }))
        .await;
        tracing::debug!(
            action = "task.routes.invalidate",
            reason,
            invalidated_count = count,
            "OAuth task routes invalidated"
        );
        count
    }

    fn task_config_matches(&self, upstream: &str, fingerprint: &str) -> bool {
        self.upstream_config_fingerprints
            .get(upstream)
            .is_some_and(|current| current.as_str() == fingerprint)
    }

    async fn resolve_task_route(
        &self,
        id: &str,
        caller: Option<&str>,
        authorization: &TaskRouteAuthorization,
        downstream: Peer<RoleServer>,
    ) -> Result<
        (
            Peer<RoleClient>,
            TaskRouteRecord,
            Arc<super::relay::RelayRouteState>,
        ),
        String,
    > {
        let store = self.task_route_store.as_ref().ok_or_else(task_not_found)?;
        let record = store.get_for_caller(id, caller, authorization).await.map_err(|error| {
            tracing::error!(action = "task.route.resolve", error = %error, "durable task route lookup failed");
            "task routing unavailable".to_string()
        })?.ok_or_else(task_not_found)?;
        if !record.authorized(caller, authorization)
            || record.expired(unix_millis_now()?)
            || !self.task_config_matches(&record.upstream_name, &record.config_fingerprint)
        {
            return Err(task_not_found());
        }
        let (peer, relay_routes, handler) = {
            let routes = self.task_routes.read().await;
            let live = routes
                .get(id)
                .ok_or_else(|| "upstream task connection unavailable".to_string())?;
            if !live.record.same_binding(&record) {
                return Err(task_not_found());
            }
            if live.connection.peer.is_transport_closed() {
                return Err("upstream task connection unavailable".to_string());
            }
            (
                live.connection.peer.clone(),
                Arc::clone(&live.connection.routes),
                live.connection
                    ._connection
                    ._client_service
                    .service()
                    .clone(),
            )
        };
        // Downstream rebinding does not hold a fleet-wide task map write lock.
        handler.rebind_downstream(downstream).await;
        if !self.task_config_matches(&record.upstream_name, &record.config_fingerprint) {
            return Err(task_not_found());
        }
        Ok((peer, record, relay_routes))
    }

    pub async fn get_task_routed(
        &self,
        mut params: GetTaskParams,
        caller_subject: Option<&str>,
        authorization: &TaskRouteAuthorization,
        downstream: Peer<RoleServer>,
    ) -> Result<GetTaskResult, String> {
        let start = Instant::now();
        let gateway_task_id = params.task_id.clone();
        let (peer, mut durable_route, _) = self
            .resolve_task_route(&gateway_task_id, caller_subject, authorization, downstream)
            .await?;
        let upstream_name = durable_route.upstream_name.clone();
        params.task_id = durable_route.native_task_id.clone();
        // Task RPCs ride the retained relay connection captured at task
        // creation, but they share the pooled path's per-upstream bulkhead,
        // timeout, telemetry, and circuit-breaker contract: the concurrency
        // permit is keyed by upstream name, not by connection, so a wedged
        // upstream cannot absorb unbounded task polls either. `subject: None`
        // because caller authorization is enforced above against the
        // subject-bound `task_routes` entry — there is no `subject_connections`
        // entry to evict for this dedicated connection.
        let event = UpstreamRequestLog::task(&upstream_name, &gateway_task_id, "task.get");
        log_upstream_request_start(event);
        let timeout_ms = self.request_timeout.as_millis();
        let mut result = timed_capability_call_str(
            self,
            &upstream_name,
            UpstreamCapability::Tools,
            event,
            start,
            peer.get_task(params),
            estimate_task_response_size,
            None,
            |error| format!("upstream `{upstream_name}` tasks/get failed: {error}"),
            format!("upstream `{upstream_name}` tasks/get timed out after {timeout_ms}ms"),
        )
        .await?;
        let previous = durable_route.clone();
        durable_route.observe(&result.task.task)?;
        // Stable polls need only a durable read, not a redundant FULL-sync write.
        if durable_route != previous {
            self.task_route_store.as_ref().ok_or_else(task_not_found)?
                .update_hints(durable_route.clone()).await.map_err(|error| {
                    tracing::error!(action = "task.route.update", error = %error, "task retention update was not durable");
                    "task routing unavailable".to_string()
                })?;
        }
        if let Some(live) = self.task_routes.write().await.get_mut(&gateway_task_id)
            && live.record.updated_at_unix_ms <= durable_route.updated_at_unix_ms
        {
            live.record = durable_route;
        }
        result.task.task.task_id = gateway_task_id;
        Ok(result)
    }

    pub async fn update_task_routed(
        &self,
        mut params: UpdateTaskParams,
        caller_subject: Option<&str>,
        authorization: &TaskRouteAuthorization,
        gateway_task_id: &str,
        downstream: Peer<RoleServer>,
    ) -> Result<(), String> {
        let start = Instant::now();
        if params.task_id != gateway_task_id {
            return Err(task_not_found());
        }
        let (peer, durable_route, relay_routes) = self
            .resolve_task_route(gateway_task_id, caller_subject, authorization, downstream)
            .await?;
        let native_task_id = durable_route.native_task_id;
        let upstream_name = durable_route.upstream_name;
        let notification_sequence = relay_routes.task_notification_sequence();
        params.task_id = native_task_id;
        // Bulkhead + telemetry parity with `get_task_routed` — see the comment
        // there for why `subject` is `None` on this retained relay connection.
        // The notification delivery barrier below runs after the permit is
        // released, so waiting on it never holds a bulkhead slot.
        let event = UpstreamRequestLog::task(&upstream_name, gateway_task_id, "task.update");
        log_upstream_request_start(event);
        let timeout_ms = self.request_timeout.as_millis();
        timed_capability_call_str(
            self,
            &upstream_name,
            UpstreamCapability::Tools,
            event,
            start,
            peer.update_task(params),
            |_: &()| 0,
            None,
            |error| format!("upstream `{upstream_name}` tasks/update failed: {error}"),
            format!("upstream `{upstream_name}` tasks/update timed out after {timeout_ms}ms"),
        )
        .await?;
        let delivered = relay_routes
            .wait_for_task_notification_after(
                notification_sequence,
                TASK_NOTIFICATION_DELIVERY_GRACE,
            )
            .await;
        tracing::debug!(
            upstream = %upstream_name,
            gateway_task_id,
            delivered,
            "task update notification delivery barrier finished"
        );
        Ok(())
    }

    pub async fn cancel_task_routed(
        &self,
        mut params: CancelTaskParams,
        caller_subject: Option<&str>,
        authorization: &TaskRouteAuthorization,
        gateway_task_id: &str,
        downstream: Peer<RoleServer>,
    ) -> Result<(), String> {
        let start = Instant::now();
        if params.task_id != gateway_task_id {
            return Err(task_not_found());
        }
        let (peer, durable_route, relay_routes) = self
            .resolve_task_route(gateway_task_id, caller_subject, authorization, downstream)
            .await?;
        let native_task_id = durable_route.native_task_id;
        let upstream_name = durable_route.upstream_name;
        let notification_sequence = relay_routes.task_notification_sequence();
        params.task_id = native_task_id;
        // Bulkhead + telemetry parity with `get_task_routed` — see the comment
        // there for why `subject` is `None` on this retained relay connection.
        let event = UpstreamRequestLog::task(&upstream_name, gateway_task_id, "task.cancel");
        log_upstream_request_start(event);
        let timeout_ms = self.request_timeout.as_millis();
        timed_capability_call_str(
            self,
            &upstream_name,
            UpstreamCapability::Tools,
            event,
            start,
            peer.cancel_task(params),
            |_: &()| 0,
            None,
            |error| format!("upstream `{upstream_name}` tasks/cancel failed: {error}"),
            format!("upstream `{upstream_name}` tasks/cancel timed out after {timeout_ms}ms"),
        )
        .await?;
        let delivered = relay_routes
            .wait_for_task_notification_after(
                notification_sequence,
                TASK_NOTIFICATION_DELIVERY_GRACE,
            )
            .await;
        tracing::debug!(
            upstream = %upstream_name,
            gateway_task_id,
            delivered,
            "task cancel notification delivery barrier finished"
        );
        Ok(())
    }
}

#[cfg(test)]
// `panic!` is how tests assert; `panic = "warn"` targets production paths.
#[allow(clippy::panic)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use rmcp::model::{
        CallToolResponse, CancelTaskParams, ClientCapabilities, ClientInfo, CreateTaskResult,
        DetailedTask, GetTaskParams, GetTaskResult, InputResponses, ProtocolVersion,
        ServerCapabilities, ServerInfo, Task, TaskPayload, TaskStatus, UpdateTaskParams,
    };
    use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RequestContext, RunningService};
    use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
    use tokio::sync::Mutex;

    use super::super::relay::{RelayClientHandler, RelayRouteState};
    use super::super::relay_cache::{RelayCachedConnection, capability_fingerprint};
    use super::super::{UpstreamConnection, UpstreamPool};

    const NATIVE_TASK_ID: &str = "native-task-1";

    #[derive(Clone, Default)]
    pub(super) struct TaskServer {
        pub(super) updates: Arc<Mutex<Vec<String>>>,
        pub(super) cancellations: Arc<Mutex<Vec<String>>>,
        fail_get_task: Arc<std::sync::atomic::AtomicBool>,
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
            if self.fail_get_task.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(ErrorData::internal_error("task backend unavailable", None));
            }
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
    pub(super) struct DownstreamServer;

    impl ServerHandler for DownstreamServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }
    }

    pub(super) async fn task_pool() -> (
        UpstreamPool,
        TaskServer,
        RunningService<RoleServer, DownstreamServer>,
        super::super::relay_cache::RelayCacheKey,
    ) {
        task_pool_with_store(None).await
    }

    async fn task_pool_with_store(
        usage_store: Option<Arc<crate::usage::UsageStore>>,
    ) -> (
        UpstreamPool,
        TaskServer,
        RunningService<RoleServer, DownstreamServer>,
        super::super::relay_cache::RelayCacheKey,
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

        let key = (
            "task-upstream".to_string(),
            7,
            None,
            capability_fingerprint(&capabilities),
        );
        let route_store = Arc::new(
            super::super::TaskRouteStore::open_in_memory()
                .await
                .expect("task route store opens"),
        );
        let pool = UpstreamPool::new()
            .with_usage_store(usage_store)
            .with_task_route_store(route_store);
        pool.upstream_config_fingerprints.insert(
            "task-upstream".to_string(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        );
        pool.relay_connections
            .write()
            .await
            .insert(key.clone(), connection);
        (pool, server, downstream_server, key)
    }

    pub(super) fn create_task_response() -> CallToolResponse {
        CallToolResponse::Task(CreateTaskResult::new(Task::new(
            NATIVE_TASK_ID,
            TaskStatus::Working,
            "2026-07-31T00:00:00Z",
            "2026-07-31T00:00:00Z",
        )))
    }

    #[test]
    fn task_handles_are_opaque_and_unique() {
        let first = super::mint_task_handle();
        let second = super::mint_task_handle();

        assert_ne!(first, second);
        assert!(first.starts_with("labby-task-"));
        assert_eq!(first.len(), "labby-task-".len() + 32);
        assert!(
            first["labby-task-".len()..]
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        );
        assert!(!first.ends_with("0000000000000001"));
    }

    #[tokio::test]
    async fn missing_relay_connection_rejects_orphan_task_response() {
        let (pool, _server, _downstream, relay_key) = task_pool().await;
        pool.relay_connections.write().await.remove(&relay_key);

        let response = pool
            .register_task_response(
                &relay_key,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                Some("alice"),
                super::TaskRouteAuthorization::root(),
                create_task_response(),
            )
            .await;

        assert_eq!(
            response.expect_err(
                "a native task handle must not escape when the gateway cannot register its route",
            ),
            "upstream task registration failed"
        );
    }

    #[tokio::test]
    async fn persistence_failure_prevents_task_acknowledgement() {
        let (pool, _server, _downstream, relay_key) = task_pool().await;
        let store = pool
            .task_route_store
            .as_ref()
            .expect("task route store configured");
        store.set_fail_writes_for_tests(true);

        let result = pool
            .register_task_response(
                &relay_key,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                Some("alice"),
                super::TaskRouteAuthorization::root(),
                create_task_response(),
            )
            .await;

        assert_eq!(
            result.expect_err("failed persistence must prevent acknowledgement"),
            "upstream task registration failed"
        );
        assert!(
            pool.relay_connections.read().await.contains_key(&relay_key),
            "relay connection is restored when durable commit fails"
        );
        assert!(pool.task_routes.read().await.is_empty());
    }

    #[tokio::test]
    async fn routed_task_lifecycle_uses_gateway_handle_and_native_upstream_id() {
        let (pool, server, downstream, relay_key) = task_pool().await;

        let registered = pool
            .register_task_response(
                &relay_key,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                Some("alice"),
                super::TaskRouteAuthorization::root(),
                create_task_response(),
            )
            .await
            .expect("task route registers");
        assert!(
            matches!(&registered, CallToolResponse::Task(_)),
            "expected task response"
        );
        let CallToolResponse::Task(created) = registered else {
            return;
        };
        let gateway_task_id = created.task.task_id.clone();
        assert_ne!(gateway_task_id, NATIVE_TASK_ID);
        assert!(gateway_task_id.starts_with("labby-task-"));

        let denied = pool
            .get_task_routed(
                GetTaskParams::new(&gateway_task_id),
                Some("bob"),
                &super::TaskRouteAuthorization::root(),
                downstream.peer().clone(),
            )
            .await;
        assert!(denied.is_err(), "task handles must remain subject-bound");

        let task = pool
            .get_task_routed(
                GetTaskParams::new(&gateway_task_id),
                Some("alice"),
                &super::TaskRouteAuthorization::root(),
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
            &super::TaskRouteAuthorization::root(),
            &gateway_task_id,
            downstream.peer().clone(),
        )
        .await
        .expect("owner updates task");
        pool.cancel_task_routed(
            CancelTaskParams::new(&gateway_task_id),
            Some("alice"),
            &super::TaskRouteAuthorization::root(),
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

    #[tokio::test]
    async fn protected_task_route_rejects_disjoint_route_lifecycle_operations() {
        let (pool, server, downstream, relay_key) = task_pool().await;
        let route_a = super::TaskRouteAuthorization::new(
            "protected:a",
            Some(std::iter::once("task-upstream".to_string()).collect()),
        );
        let route_b = super::TaskRouteAuthorization::new(
            "protected:b",
            Some(std::iter::once("other-upstream".to_string()).collect()),
        );
        let registered = pool
            .register_task_response(
                &relay_key,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                Some("alice"),
                route_a,
                create_task_response(),
            )
            .await
            .expect("task route registers");
        let CallToolResponse::Task(created) = registered else {
            panic!("expected task response");
        };
        let gateway_task_id = created.task.task_id;

        let get = pool
            .get_task_routed(
                GetTaskParams::new(&gateway_task_id),
                Some("alice"),
                &route_b,
                downstream.peer().clone(),
            )
            .await;
        assert_eq!(
            get.expect_err("disjoint route must reject get"),
            super::task_not_found()
        );
        let update = pool
            .update_task_routed(
                UpdateTaskParams::new(&gateway_task_id, InputResponses::new()),
                Some("alice"),
                &route_b,
                &gateway_task_id,
                downstream.peer().clone(),
            )
            .await;
        assert_eq!(
            update.expect_err("disjoint route must reject update"),
            super::task_not_found()
        );
        let cancel = pool
            .cancel_task_routed(
                CancelTaskParams::new(&gateway_task_id),
                Some("alice"),
                &route_b,
                &gateway_task_id,
                downstream.peer().clone(),
            )
            .await;
        assert_eq!(
            cancel.expect_err("disjoint route must reject cancel"),
            super::task_not_found()
        );
        assert!(server.updates.lock().await.is_empty());
        assert!(server.cancellations.lock().await.is_empty());
    }

    async fn rekey_relay_for_oauth_subject(
        pool: &UpstreamPool,
        relay_key: &super::super::relay_cache::RelayCacheKey,
        oauth_subject: &str,
    ) -> super::super::relay_cache::RelayCacheKey {
        let connection = pool
            .relay_connections
            .write()
            .await
            .remove(relay_key)
            .expect("relay connection exists");
        let oauth_key = (
            relay_key.0.clone(),
            relay_key.1,
            Some(oauth_subject.to_string()),
            relay_key.3.clone(),
        );
        pool.relay_connections
            .write()
            .await
            .insert(oauth_key.clone(), connection);
        oauth_key
    }

    #[tokio::test]
    async fn shared_admin_oauth_task_invalidation_uses_credential_subject() {
        let (pool, _server, _downstream, relay_key) = task_pool().await;
        let oauth_key = rekey_relay_for_oauth_subject(&pool, &relay_key, "shared-admin").await;
        pool.register_task_response(
            &oauth_key,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some("alice"),
            super::TaskRouteAuthorization::root(),
            create_task_response(),
        )
        .await
        .expect("shared-admin OAuth task registers");

        assert_eq!(
            pool.invalidate_task_routes_for_oauth_subject("task-upstream", "shared-admin", "test")
                .await,
            1
        );
    }

    #[tokio::test]
    async fn oauth_revocation_is_not_blocked_by_inflight_network_work() {
        let cache =
            labby_auth::upstream::cache::OauthClientCache::new(Arc::new(dashmap::DashMap::new()));
        let epoch = cache.lifecycle_epoch();

        // An in-flight request holds no lifecycle reader. Revocation can take
        // the writer immediately and makes the request's snapshot stale.
        let barrier = cache.invalidation_barrier();
        let writer = tokio::time::timeout(Duration::from_millis(100), barrier.write_owned())
            .await
            .expect("revocation writer must not wait on network work");
        cache.advance_lifecycle_epoch();
        drop(writer);

        assert_ne!(cache.lifecycle_epoch(), epoch);
    }

    #[tokio::test]
    async fn trusted_stdio_oauth_task_without_caller_subject_is_invalidated() {
        let (pool, _server, _downstream, relay_key) = task_pool().await;
        let oauth_key = rekey_relay_for_oauth_subject(&pool, &relay_key, "stdio-oauth").await;
        pool.register_task_response(
            &oauth_key,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            None,
            super::TaskRouteAuthorization::root(),
            create_task_response(),
        )
        .await
        .expect("trusted stdio OAuth task registers");

        assert_eq!(pool.invalidate_all_oauth_task_routes("test").await, 1);
    }

    #[tokio::test]
    async fn global_oauth_invalidation_preserves_raw_authenticated_task() {
        let (pool, _server, _downstream, relay_key) = task_pool().await;
        pool.register_task_response(
            &relay_key,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some("alice"),
            super::TaskRouteAuthorization::root(),
            create_task_response(),
        )
        .await
        .expect("raw authenticated task registers");

        assert_eq!(pool.invalidate_all_oauth_task_routes("test").await, 0);
        assert_eq!(pool.task_routes.read().await.len(), 1);
    }

    /// The bulkhead-routed task path preserves the historical error-string
    /// format on upstream failure and records usage telemetry for both the
    /// failing and succeeding RPCs (`timed_capability_call` contract).
    #[tokio::test]
    async fn routed_task_get_failure_preserves_error_format_and_records_usage() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            crate::usage::UsageStore::open(dir.path().join("usage.db"))
                .await
                .unwrap(),
        );
        let (pool, server, downstream, relay_key) =
            task_pool_with_store(Some(Arc::clone(&store))).await;

        let registered = pool
            .register_task_response(
                &relay_key,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                Some("alice"),
                super::TaskRouteAuthorization::root(),
                create_task_response(),
            )
            .await
            .expect("task route registers");
        let CallToolResponse::Task(created) = registered else {
            panic!("expected task response");
        };
        let gateway_task_id = created.task.task_id.clone();

        server
            .fail_get_task
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let error = pool
            .get_task_routed(
                GetTaskParams::new(&gateway_task_id),
                Some("alice"),
                &super::TaskRouteAuthorization::root(),
                downstream.peer().clone(),
            )
            .await
            .expect_err("failing upstream get_task surfaces an error");
        assert!(
            error.starts_with("upstream `task-upstream` tasks/get failed:"),
            "error string format must be preserved, got: {error}"
        );
        assert!(
            error.contains("task backend unavailable"),
            "upstream message must survive, got: {error}"
        );

        server
            .fail_get_task
            .store(false, std::sync::atomic::Ordering::SeqCst);
        pool.get_task_routed(
            GetTaskParams::new(&gateway_task_id),
            Some("alice"),
            &super::TaskRouteAuthorization::root(),
            downstream.peer().clone(),
        )
        .await
        .expect("recovered upstream get_task succeeds");

        // Usage writes are fire-and-forget (`tokio::spawn`), so poll to a
        // deadline rather than betting on a fixed sleep under CI load.
        let read_rows = async || -> Vec<(String, String)> {
            store
                .with_conn(|conn| {
                    let mut statement = conn
                        .prepare(
                            "SELECT tool_name, outcome FROM upstream_calls \
                             WHERE upstream_name = 'task-upstream' ORDER BY outcome",
                        )
                        .map_err(crate::usage::store::sqlite_error)?;
                    let rows = statement
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                        .map_err(crate::usage::store::sqlite_error)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(crate::usage::store::sqlite_error)?;
                    Ok(rows)
                })
                .await
                .unwrap()
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut rows = read_rows().await;
        while rows.len() < 2 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
            rows = read_rows().await;
        }
        assert_eq!(
            rows,
            vec![
                (gateway_task_id.clone(), "ok".to_string()),
                (gateway_task_id.clone(), "upstream_error".to_string()),
            ],
            "task RPCs must record usage telemetry through the bulkhead path"
        );
    }
}
