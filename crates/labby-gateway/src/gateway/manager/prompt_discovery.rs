//! Bounded cold-start discovery for regular upstream prompts.

use std::collections::BTreeSet;
use std::sync::Arc;

use futures::StreamExt as _;

use super::GatewayManager;
use crate::upstream::pool::{UpstreamPool, upstream_discovery_concurrency};

impl GatewayManager {
    /// Connect prompt providers before a first prompt lookup or catalog read.
    ///
    /// Only enabled, route-visible, non-OAuth providers with prompt proxying
    /// participate. In particular, a prompt-only provider need not expose
    /// tools or resources to be discovered. The pool owns single-flight,
    /// cooldown, transport policy and connection publication.
    ///
    /// The caller deadline is shared with the subsequent catalog pass and is
    /// further capped by the configured MCP warm-up budget. OAuth discovery
    /// remains on the caller-subject path, never the global pool.
    pub async fn ensure_prompt_upstreams_ready_until(
        &self,
        pool: &Arc<UpstreamPool>,
        allowed: Option<&BTreeSet<String>>,
        caller_deadline: tokio::time::Instant,
    ) {
        let config = self.current_config().await;
        let budget = super::super::runtime::mcp_runtime_warm_timeout(&config);
        let deadline = caller_deadline.min(tokio::time::Instant::now() + budget);
        let concurrency =
            upstream_discovery_concurrency(config.gateway.upstream_discovery_concurrency);
        let upstreams = config.upstream.into_iter().filter(|upstream| {
            upstream.enabled
                && upstream.proxy_prompts
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
                        tracing::warn!(
                            surface = "dispatch", service = "gateway",
                            action = "prompts.discovery", upstream = %upstream.name,
                            error = %error, "prompt upstream discovery failed"
                        );
                        false
                    }
                    // A shared budget expiring is not a second upstream failure.
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
                action = "prompts.discovery",
                timeout_ms = budget.as_millis(),
                unfinished,
                "prompt discovery deadline reached; returning current catalog"
            );
        }
    }
}
