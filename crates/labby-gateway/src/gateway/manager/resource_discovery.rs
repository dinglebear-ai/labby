//! Bounded discovery of regular resource and template upstream connections.

use std::collections::BTreeSet;
use std::sync::Arc;

use futures::StreamExt as _;
use labby_runtime::error::ToolError;
use rmcp::model::ReadResourceResult;

use super::GatewayManager;
use crate::upstream::pool::{UpstreamPool, upstream_discovery_concurrency};

impl GatewayManager {
    /// Read one native MCP App resource from the currently published pool.
    ///
    /// This is an exact read against already-published runtime state. It never
    /// creates a pool, reconnects an upstream, or derives routing from a named
    /// provider; native `ui://` ownership remains the pool's responsibility.
    pub async fn read_mcp_app_resource(
        &self,
        uri: &str,
        oauth_subject: Option<&str>,
    ) -> Result<ReadResourceResult, ToolError> {
        validate_mcp_app_uri(uri)?;
        let pool = self.current_pool_sync().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "executor_unavailable".to_string(),
            message: "gateway upstream pool is unavailable".to_string(),
        })?;
        if let Some(subject) = oauth_subject {
            let config = self.current_config().await;
            let owner = pool
                .cached_subject_scoped_ui_resource_owner(&config.upstream, subject, uri, None)
                .await
                .map_err(|message| ToolError::Sdk {
                    sdk_kind: "resource_owner_conflict".to_string(),
                    message,
                })?;
            if let Some(owner) = owner {
                return pool
                    .subject_scoped_read_resource(&owner, subject, uri)
                    .await
                    .map_err(|message| ToolError::Sdk {
                        sdk_kind: "resource_read_failed".to_string(),
                        message,
                    });
            }
        }
        pool.read_upstream_ui_resource(uri)
            .await
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: "MCP App resource is not available from a published upstream".to_string(),
            })?
            .map_err(|message| ToolError::Sdk {
                sdk_kind: "resource_read_failed".to_string(),
                message,
            })
    }

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

fn validate_mcp_app_uri(uri: &str) -> Result<(), ToolError> {
    let parsed = url::Url::parse(uri).map_err(|_| ToolError::InvalidParam {
        message: "MCP App resource URI must be a valid ui:// URI".to_string(),
        param: "uri".to_string(),
    })?;
    if parsed.scheme() != "ui"
        || parsed.host_str().is_none_or(str::is_empty)
        || matches!(parsed.path(), "" | "/")
    {
        return Err(ToolError::InvalidParam {
            message: "MCP App resource URI must include a ui:// authority and path".to_string(),
            param: "uri".to_string(),
        });
    }
    Ok(())
}
