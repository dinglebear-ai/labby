//! Commit durable routing and install the live companion before exposing an ID.

use std::collections::hash_map::Entry;
use std::sync::Arc;

use rmcp::model::CallToolResponse;

use super::super::relay_cache::{RelayCacheKey, RelayCachedConnection};
use super::super::task_route_record::timestamp_millis;
use super::{TaskRoute, TaskRouteAuthorization, TaskRouteRecord, UpstreamPool};

impl UpstreamPool {
    /// Store only trusted routing metadata. The expected fingerprint comes from
    /// the configuration used to dispatch the creating request, not a later reload.
    pub async fn register_task_response(
        &self,
        relay_key: &RelayCacheKey,
        expected_fingerprint: &str,
        caller_subject: Option<&str>,
        authorization: TaskRouteAuthorization,
        response: CallToolResponse,
    ) -> Result<CallToolResponse, String> {
        let CallToolResponse::Task(mut created) = response else {
            return Ok(response);
        };
        let store = self
            .task_route_store
            .as_ref()
            .ok_or_else(|| registration_error("route_store_unavailable"))?;
        if !self.task_config_matches(&relay_key.0, expected_fingerprint) {
            return Err(registration_error("stale_configuration"));
        }
        let id = super::mint_task_handle();
        let record = TaskRouteRecord {
            public_task_id: id.clone(),
            native_task_id: created.task.task_id.clone(),
            upstream_name: relay_key.0.clone(),
            caller_subject: caller_subject.map(str::to_owned),
            oauth_subject: relay_key.2.clone(),
            authorization: authorization.clone(),
            config_fingerprint: expected_fingerprint.to_string(),
            created_at_unix_ms: timestamp_millis(&created.task.created_at)
                .map_err(|_| registration_error("invalid_creation_timestamp"))?,
            updated_at_unix_ms: timestamp_millis(&created.task.last_updated_at)
                .map_err(|_| registration_error("invalid_update_timestamp"))?,
            ttl_ms: created.task.ttl_ms,
            poll_interval_ms: created.task.poll_interval_ms,
        };
        record
            .validate()
            .map_err(|_| registration_error("invalid_metadata"))?;
        if !record.authorized(caller_subject, &authorization)
            || record.expired(super::unix_millis_now()?)
        {
            return Err(registration_error("unresolvable_task"));
        }
        let connection = self
            .relay_connections
            .write()
            .await
            .remove(relay_key)
            .ok_or_else(|| registration_error("relay_connection_unavailable"))?;
        if connection.peer.is_transport_closed() {
            self.restore_task_relay(relay_key, connection).await;
            return Err(registration_error("relay_connection_closed"));
        }
        if let Err(error) = store.insert(record.clone()).await {
            tracing::error!(action = "task.registration.persist", error = %error, "task route commit failed");
            self.restore_task_relay(relay_key, connection).await;
            return Err(registration_error("route_persistence_failed"));
        }
        let handler = connection._connection._client_service.service().clone();
        let relay_routes = Arc::clone(&connection.routes);
        {
            let mut routes = self.task_routes.write().await;
            // The SQLite await is a reload/cancellation boundary. Fail closed
            // rather than binding an old task to a newly configured server.
            if !self.task_config_matches(&record.upstream_name, &record.config_fingerprint)
                || record.expired(super::unix_millis_now()?)
                || connection.peer.is_transport_closed()
            {
                drop(routes);
                if let Err(error) = store.remove(&id).await {
                    tracing::error!(action = "task.registration.rollback", error = %error, "unacknowledged route cleanup failed");
                }
                self.restore_task_relay(relay_key, connection).await;
                return Err(registration_error("route_changed_during_commit"));
            }
            super::prune_task_routes(&mut routes);
            routes.insert(
                id.clone(),
                TaskRoute {
                    record: record.clone(),
                    connection,
                },
            );
        }
        // Both durable and live routes are now visible. Publishing a translation
        // before this point would let an early notification expose an unusable ID.
        let pending = relay_routes
            .register_task_id(&record.native_task_id, &id)
            .await;
        let delivery = async {
            for notification in pending {
                handler.forward_task_status(notification).await;
            }
        };
        if tokio::time::timeout(super::TASK_NOTIFICATION_DELIVERY_GRACE, delivery)
            .await
            .is_err()
        {
            tracing::warn!(
                action = "task.registration.notification",
                "initial task notification delivery timed out; polling remains available"
            );
        }
        created.task.task_id = id;
        Ok(CallToolResponse::Task(created))
    }

    async fn restore_task_relay(&self, key: &RelayCacheKey, connection: RelayCachedConnection) {
        let displaced = {
            let mut cache = self.relay_connections.write().await;
            match cache.entry(key.clone()) {
                Entry::Vacant(entry) => {
                    entry.insert(connection);
                    None
                }
                Entry::Occupied(_) => Some(connection),
            }
        };
        if let Some(connection) = displaced {
            connection
                ._connection
                .shutdown(&key.0, "task.registration.rollback")
                .await;
        }
    }
}

fn registration_error(reason: &'static str) -> String {
    tracing::warn!(
        action = "task.registration.reject",
        reason,
        "upstream task was not acknowledged"
    );
    "upstream task registration failed".to_string()
}
