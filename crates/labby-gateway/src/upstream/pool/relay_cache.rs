//! Relay connection identity, cached state, and bounded LRU lifecycle helpers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use rmcp::model::ClientCapabilities;
use rmcp::service::Peer;
use rmcp::{RoleClient, RoleServer};

use super::UpstreamConnection;
use super::http_cancellation::HttpCancellationSender;
use super::relay::{RelayClientHandler, RelayRouteState};

/// Project request capabilities to the protocol fields supported by the relay.
pub fn relay_client_capabilities(mut capabilities: ClientCapabilities) -> ClientCapabilities {
    capabilities.experimental = None;
    if let Some(sampling) = &mut capabilities.sampling {
        sampling.tools.iter_mut().for_each(serde_json::Map::clear);
        sampling.context.iter_mut().for_each(serde_json::Map::clear);
    }
    if let Some(extensions) = &mut capabilities.extensions {
        extensions.retain(|name, settings| {
            match name.as_str() {
                rmcp::model::TASKS_EXTENSION_ID => settings.clear(),
                "io.modelcontextprotocol/ui" => {
                    const MIME: &str = "text/html;profile=mcp-app";
                    let supports_app = settings
                        .get("mimeTypes")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|types| types.iter().any(|item| item.as_str() == Some(MIME)));
                    if settings.contains_key("mimeTypes") && !supports_app {
                        return false;
                    }
                    settings.clear();
                    if supports_app {
                        settings.insert("mimeTypes".into(), serde_json::json!([MIME]));
                    }
                }
                "io.modelcontextprotocol/skills" => {
                    let directory = settings
                        .get("directoryRead")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true);
                    settings.clear();
                    if directory {
                        settings.insert("directoryRead".into(), true.into());
                    }
                }
                _ => return false,
            }
            true
        });
        if extensions.is_empty() {
            capabilities.extensions = None;
        }
    }
    capabilities
}

pub(super) fn capability_fingerprint(capabilities: &ClientCapabilities) -> String {
    serde_json::to_string(capabilities)
        .expect("MCP client capabilities must serialize to a JSON object")
}

pub(super) type RelayCacheKey = (String, u64, Option<String>, String);

pub(super) struct RelayCachedConnection {
    pub(super) _connection: UpstreamConnection<RelayClientHandler>,
    pub(super) peer: Peer<RoleClient>,
    pub(super) capability_fingerprint: String,
    // Each acquired call keeps a clone until its terminal outcome.
    pub(super) active_calls: Arc<()>,
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
            .filter(|(key, entry)| *key != protect && Arc::strong_count(&entry.active_calls) == 1)
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
