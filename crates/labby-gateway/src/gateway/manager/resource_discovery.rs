//! Bounded discovery of regular resource and template upstream connections.

use std::collections::BTreeSet;
use std::sync::Arc;

use futures::StreamExt as _;

use super::GatewayManager;
use crate::upstream::pool::{UpstreamPool, upstream_discovery_concurrency};

impl GatewayManager {
    /// Warm eligible regular peers within one shared catalog warm-up deadline.
    /// OAuth and relay peers remain isolated from this global connection pool.
    pub async fn ensure_resource_upstreams_ready(
        &self,
        pool: &Arc<UpstreamPool>,
        allowed: Option<&BTreeSet<String>>,
    ) {
        let config = self.current_config().await;
        let budget = super::super::runtime::mcp_runtime_warm_timeout(&config);
        let deadline = tokio::time::Instant::now() + budget;
        let concurrency =
            upstream_discovery_concurrency(config.gateway.upstream_discovery_concurrency);
        let upstreams = config.upstream.into_iter().filter(|upstream| {
            upstream.enabled
                && upstream.proxy_resources
                && upstream.oauth.is_none()
                && allowed.is_none_or(|names| names.contains(&upstream.name))
        });
        let mut discoveries = futures::stream::iter(upstreams)
            .map(|upstream| async move {
                if tokio::time::Instant::now() >= deadline {
                    return true;
                }
                match tokio::time::timeout_at(
                    deadline,
                    pool.ensure_connection_for_upstream(&upstream, None, None),
                )
                .await
                {
                    Ok(Ok(_)) => false,
                    Ok(Err(error)) => {
                        tracing::warn!(surface = "dispatch", service = "gateway",
                            action = "resources.discovery", upstream = %upstream.name,
                            error = %error, "resource upstream discovery failed");
                        false
                    }
                    // This deadline also covers lock contention and cache refresh.
                    // Only the connection owner can account actual upstream failures.
                    Err(_) => true,
                }
            })
            .buffer_unordered(concurrency);
        let mut unfinished = 0;
        while let Some(timed_out) = discoveries.next().await {
            unfinished += usize::from(timed_out);
        }
        if unfinished > 0 {
            tracing::warn!(
                surface = "dispatch",
                service = "gateway",
                action = "resources.discovery",
                timeout_ms = budget.as_millis(),
                unfinished,
                "resource discovery deadline reached; returning current snapshot"
            );
        }
    }
}
