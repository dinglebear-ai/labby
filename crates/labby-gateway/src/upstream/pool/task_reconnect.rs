//! Reacquire an ephemeral task companion from authorized durable metadata.

use std::sync::Arc;
use std::time::Instant;

use rmcp::model::{ClientRequest, ErrorData, ServerResult};
use rmcp::service::{Peer, PeerRequestOptions};
use rmcp::{RoleClient, RoleServer, ServiceError};

use super::super::relay::{RelayClientHandler, RelayRouteState};
use super::super::relay_cache::capability_fingerprint;
use super::super::tool_call_cancel::CancelUpstreamOnDrop;
use super::{TaskCallContext, TaskRoute, TaskRouteRecord, UpstreamPool, task_not_found};

impl UpstreamPool {
    pub(super) async fn check_task_binding(
        &self,
        expected: &TaskRouteRecord,
    ) -> Result<(), String> {
        let store = self.task_route_store.as_ref().ok_or_else(task_not_found)?;
        let current = store.get_for_caller(&expected.public_task_id,
            expected.caller_subject.as_deref(), &expected.authorization).await
            .map_err(|error| {
                tracing::error!(action = "task.route.revalidate", error = %error, "task route lookup failed");
                "task routing unavailable".to_string()
            })?.ok_or_else(task_not_found)?;
        if !current.same_binding(expected)
            || current.expired(super::unix_millis_now()?)
            || !self.task_config_matches(&current.upstream_name, &current.config_fingerprint)
        {
            return Err(task_not_found());
        }
        Ok(())
    }

    /// No task creation is replayed. A failed connect leaves the durable row intact.
    pub(super) async fn acquire_task_companion(
        &self,
        record: &TaskRouteRecord,
        downstream: Peer<RoleServer>,
        context: &TaskCallContext,
        start: Instant,
    ) -> Result<(Peer<RoleClient>, Arc<RelayRouteState>, RelayClientHandler), String> {
        let acquire = async {
            let fingerprint = capability_fingerprint(&context.capabilities);
            if let Some(companion) = self.live_task_companion(record, &fingerprint).await? {
                return Ok(companion);
            }
            // Reuse the upstream's existing acquisition/reconcile gate. No second
            // runtime owner, config inventory, or task scheduler is introduced.
            let gate = self.lazy_connect_lock(&record.upstream_name).await;
            let _connect = gate.lock().await;
            self.check_task_binding(record).await?;
            if let Some(companion) = self.live_task_companion(record, &fingerprint).await? {
                return Ok(companion);
            }
            let config = self
                .configured_upstreams
                .get(&record.upstream_name)
                .map(|entry| entry.clone())
                .ok_or_else(task_not_found)?;
            if !self.upstream_config_matches(&config)
                || crate::gateway::code_mode::catalog_cache::fingerprint(&config)
                    != record.config_fingerprint
                || (config.oauth.is_some() && record.oauth_subject.is_none())
            {
                return Err(task_not_found());
            }
            let epoch = record
                .oauth_subject
                .as_deref()
                .and_then(|subject| self.oauth_lifecycle_epoch(&record.upstream_name, subject));
            // Namespace task peers away from downstream relay keys. Full public
            // IDs, rather than a truncated hash/session counter, preserve isolation.
            let key = (
                record.upstream_name.clone(),
                0,
                record.oauth_subject.clone(),
                format!("task:{}:{fingerprint}", record.public_task_id),
            );
            self.acquire_relay_for_key(
                &config,
                record.oauth_subject.as_deref(),
                downstream,
                context.capabilities.clone(),
                key.clone(),
            )
            .await
            .ok_or_else(|| "upstream task connection unavailable".to_string())?;
            let connection = self
                .relay_connections
                .write()
                .await
                .remove(&key)
                .ok_or_else(|| "upstream task connection unavailable".to_string())?;
            // Credential changes may have completed during connection I/O. The
            // durable row is checked again under their existing publication barrier.
            let _publication = self.oauth_invalidation_barrier.read().await;
            if epoch.as_ref().is_some_and(|epoch| !epoch.is_current()) {
                drop(_publication);
                connection
                    ._connection
                    .shutdown(&record.upstream_name, "task.reconnect.epoch")
                    .await;
                return Err(task_not_found());
            }
            if let Err(error) = self.check_task_binding(record).await {
                drop(_publication);
                connection
                    ._connection
                    .shutdown(&record.upstream_name, "task.reconnect.stale")
                    .await;
                return Err(error);
            }
            let peer = connection.peer.clone();
            let routes = Arc::clone(&connection.routes);
            let handler = connection._connection._client_service.service().clone();
            let pending_notifications = routes
                .register_task_id(&record.native_task_id, &record.public_task_id)
                .await;
            let previous = self.task_routes.write().await.insert(
                record.public_task_id.clone(),
                TaskRoute {
                    record: record.clone(),
                    connection,
                },
            );
            for notification in pending_notifications {
                handler.forward_task_status(notification).await;
            }
            drop(_publication);
            if let Some(previous) = previous {
                previous
                    .connection
                    ._connection
                    .shutdown(&record.upstream_name, "task.reconnect.replace")
                    .await;
            }
            tracing::debug!(action = "task.route.reconnect", upstream = %record.upstream_name,
                "authorized task peer reacquired");
            Ok((peer, routes, handler))
        };
        tokio::select! {
            biased;
            () = context.cancellation.cancelled() => Err("task request cancelled".to_string()),
            result = tokio::time::timeout(self.request_timeout.saturating_sub(start.elapsed()), acquire) =>
                result.map_err(|_| "upstream task connection timed out".to_string())?,
        }
    }

    async fn live_task_companion(
        &self,
        record: &TaskRouteRecord,
        fingerprint: &str,
    ) -> Result<Option<(Peer<RoleClient>, Arc<RelayRouteState>, RelayClientHandler)>, String> {
        let routes = self.task_routes.read().await;
        let Some(live) = routes.get(&record.public_task_id) else {
            return Ok(None);
        };
        if !live.record.same_binding(record) {
            return Err(task_not_found());
        }
        if live.connection.peer.is_transport_closed()
            || live.connection.capability_fingerprint != fingerprint
        {
            return Ok(None);
        }
        Ok(Some((
            live.connection.peer.clone(),
            Arc::clone(&live.connection.routes),
            live.connection
                ._connection
                ._client_service
                .service()
                .clone(),
        )))
    }

    pub(super) async fn check_task_companion(
        &self,
        record: &TaskRouteRecord,
        routes: &Arc<RelayRouteState>,
    ) -> Result<(), String> {
        self.check_task_binding(record).await?;
        if self
            .task_routes
            .read()
            .await
            .get(&record.public_task_id)
            .is_none_or(|live| !Arc::ptr_eq(&live.connection.routes, routes))
        {
            return Err(task_not_found());
        }
        Ok(())
    }

    /// Linearize authorization with the actual RMCP enqueue, then release the
    /// credential reader before waiting for remote I/O. A mutation is never replayed.
    pub(super) async fn send_task_request(
        &self,
        peer: &Peer<RoleClient>,
        record: &TaskRouteRecord,
        routes: &Arc<RelayRouteState>,
        request: ClientRequest,
    ) -> Result<ServerResult, ServiceError> {
        let _publication = self
            .task_publication_guard(record)
            .await
            .map_err(|message| ServiceError::McpError(ErrorData::invalid_params(message, None)))?;
        let companions = self.task_routes.read().await;
        if companions
            .get(&record.public_task_id)
            .is_none_or(|live| !Arc::ptr_eq(&live.connection.routes, routes))
        {
            return Err(ServiceError::McpError(ErrorData::invalid_params(
                task_not_found(),
                None,
            )));
        }
        self.check_task_binding(record)
            .await
            .map_err(|message| ServiceError::McpError(ErrorData::invalid_params(message, None)))?;
        let handle = peer
            .send_request_with_option(request, PeerRequestOptions::no_options())
            .await?;
        let mut cancellation = CancelUpstreamOnDrop::armed(
            handle.peer.clone(),
            &record.upstream_name,
            handle.id.clone(),
        );
        drop(companions);
        drop(_publication);
        let result = handle.await_response().await;
        cancellation.disarm();
        result
    }

    pub(super) async fn task_publication_guard(
        &self,
        record: &TaskRouteRecord,
    ) -> Result<
        (
            tokio::sync::OwnedMutexGuard<()>,
            tokio::sync::OwnedRwLockReadGuard<()>,
        ),
        String,
    > {
        let gate = self.lazy_connect_lock(&record.upstream_name).await;
        let definition = gate.lock_owned().await;
        let credentials = self.oauth_invalidation_barrier.clone().read_owned().await;
        self.check_task_binding(record).await?;
        Ok((definition, credentials))
    }

    pub(in crate::upstream::pool) async fn close_task_companions(&self, reason: &'static str) {
        let removed = self
            .task_routes
            .write()
            .await
            .drain()
            .map(|(_, route)| route)
            .collect::<Vec<_>>();
        futures::future::join_all(removed.into_iter().map(|route| async move {
            route
                .connection
                ._connection
                .shutdown(&route.record.upstream_name, reason)
                .await;
        }))
        .await;
    }
}

pub(super) fn task_error(error: &ServiceError, upstream: &str, method: &str) -> String {
    if let ServiceError::McpError(data) = error
        && data.message == "task not found"
    {
        return task_not_found();
    }
    format!("upstream `{upstream}` tasks/{method} failed: {error}")
}

/// These are lifecycle changes without an ownership change. Everything else
/// fails conservative: clearing/replacing credentials cannot resurrect routes.
pub(super) fn preserves_task_ownership(reason: &str) -> bool {
    matches!(
        reason,
        "oauth.credentials.refresh" | "oauth.client_cache.capacity" | "upstream.restart"
    )
}

/// Final authorization, durable publication and delivery use the same caller
/// budget as connection acquisition and the RPC. Dropping this future never
/// retries an already acknowledged upstream mutation.
pub(super) async fn finish_task_request<T>(
    timeout: std::time::Duration,
    start: Instant,
    context: &TaskCallContext,
    finish: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    if start.elapsed() >= timeout {
        return Err("task request completion timed out".to_string());
    }
    tokio::select! {
        biased;
        () = context.cancellation.cancelled() => Err("task request cancelled".to_string()),
        result = tokio::time::timeout(timeout.saturating_sub(start.elapsed()), finish) =>
            result.map_err(|_| "task request completion timed out".to_string())?,
    }
}

#[cfg(test)]
mod completion_budget_tests {
    use super::*;

    #[tokio::test]
    async fn post_response_gate_wait_obeys_caller_cancellation() {
        let gate = tokio::sync::Mutex::new(());
        let _holder = gate.lock().await;
        let context = TaskCallContext::default();
        let wait = finish_task_request(
            std::time::Duration::from_secs(30),
            Instant::now(),
            &context,
            async {
                let _guard = gate.lock().await;
                Ok(())
            },
        );
        let cancel = async {
            tokio::task::yield_now().await;
            context.cancellation.cancel();
        };
        let (result, ()) = tokio::join!(wait, cancel);
        assert_eq!(result.unwrap_err(), "task request cancelled");
    }

    #[tokio::test]
    async fn post_response_gate_wait_obeys_original_remaining_budget() {
        let gate = tokio::sync::Mutex::new(());
        let _holder = gate.lock().await;
        let context = TaskCallContext::default();
        let result = finish_task_request(
            std::time::Duration::from_millis(20),
            Instant::now()
                .checked_sub(std::time::Duration::from_millis(15))
                .unwrap(),
            &context,
            async {
                let _guard = gate.lock().await;
                Ok(())
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), "task request completion timed out");
    }
    #[tokio::test]
    async fn expired_completion_budget_does_not_poll_a_ready_publication() {
        let published = std::sync::atomic::AtomicBool::new(false);
        let context = TaskCallContext::default();
        let result =
            finish_task_request(std::time::Duration::ZERO, Instant::now(), &context, async {
                published.store(true, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            })
            .await;
        assert_eq!(result.unwrap_err(), "task request completion timed out");
        assert!(!published.load(std::sync::atomic::Ordering::Relaxed));
    }
}
