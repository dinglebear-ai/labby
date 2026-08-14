//! Relay connection identity, cached state, and bounded LRU lifecycle helpers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use rmcp::model::{ClientCapabilities, TaskStatusNotificationParams};
use rmcp::service::Peer;
use rmcp::{RoleClient, RoleServer};

use super::UpstreamConnection;
use super::http_cancellation::HttpCancellationSender;
use super::relay::{RelayClientHandler, RelayRouteState};

pub(super) fn capability_fingerprint(capabilities: &ClientCapabilities) -> String {
    serde_json::to_string(capabilities)
        .expect("MCP client capabilities must serialize to a JSON object")
}

pub(super) type RelayCacheKey = (String, u64, Option<String>, String);

pub(super) struct RelayCachedConnection {
    pub(super) _connection: UpstreamConnection<RelayClientHandler>,
    pub(super) peer: Peer<RoleClient>,
    pub(super) capability_fingerprint: String,
    pub(super) routes: Arc<RelayRouteState>,
    pub(super) cancellation_sender: Option<HttpCancellationSender>,
    pub(super) last_used: Instant,
}

impl RelayCachedConnection {
    pub(super) async fn rebind_downstream(&self, downstream: Peer<RoleServer>) {
        self._connection
            ._client_service
            .service()
            .rebind_downstream(downstream)
            .await;
    }

    pub(super) async fn flush_task_status_notifications(
        &self,
        notifications: Vec<TaskStatusNotificationParams>,
    ) {
        let handler = self._connection._client_service.service();
        for params in notifications {
            handler.forward_task_status(params).await;
        }
    }
}

pub(super) fn evict_relay_lru_over_cap(
    cache: &mut HashMap<RelayCacheKey, RelayCachedConnection>,
    max_entries: usize,
    protect: &RelayCacheKey,
) -> Vec<(String, UpstreamConnection<RelayClientHandler>)> {
    let mut evicted = Vec::new();
    while cache.len() > max_entries {
        let lru_key = cache
            .iter()
            .filter(|(key, _)| *key != protect)
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone());
        match lru_key {
            Some(key) => {
                if let Some(entry) = cache.remove(&key) {
                    evicted.push((key.0, entry._connection));
                }
            }
            None => break,
        }
    }
    evicted
}
