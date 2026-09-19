//! Per-upstream capability probing: count an upstream's resources and prompts
//! (when proxying is enabled), classifying each capability's health.
//!
//! `discover_capability_counts` is `pub(super)` because it is called from the
//! discovery module across the module boundary.

use rmcp::{RoleClient, service::Peer};

use super::super::types::UpstreamHealth;

pub(super) fn peer_declares_resources(peer: &Peer<RoleClient>) -> bool {
    peer.peer_info()
        .is_some_and(|info| info.capabilities.resources.is_some())
}

pub(super) fn peer_declares_prompts(peer: &Peer<RoleClient>) -> bool {
    peer.peer_info()
        .is_some_and(|info| info.capabilities.prompts.is_some())
}
use super::helpers::DISCOVERY_TIMEOUT;
use super::logging::is_capability_unsupported;

pub(super) async fn discover_capability_counts(
    name: &str,
    peer: &Peer<RoleClient>,
    proxy_resources: bool,
    proxy_prompts: bool,
) -> (
    usize,
    Option<String>,
    UpstreamHealth,
    usize,
    Option<String>,
    UpstreamHealth,
) {
    let (resource_count, resource_error, resource_health) = if proxy_resources
        && peer_declares_resources(peer)
    {
        tracing::info!(upstream = %name, capability = "resources", "starting upstream capability discovery");
        match tokio::time::timeout(DISCOVERY_TIMEOUT, peer.list_resources(None)).await {
            Ok(Ok(result)) => (result.resources.len(), None, UpstreamHealth::Healthy),
            Ok(Err(ref error)) if is_capability_unsupported(error) => {
                (0, None, UpstreamHealth::Healthy)
            }
            Ok(Err(error)) => (
                0,
                Some(format!("failed to list resources from upstream: {error}")),
                UpstreamHealth::Unhealthy {
                    consecutive_failures: 1,
                },
            ),
            Err(_) => (
                0,
                Some(format!(
                    "listing resources from upstream timed out after {}s",
                    DISCOVERY_TIMEOUT.as_secs()
                )),
                UpstreamHealth::Unhealthy {
                    consecutive_failures: 1,
                },
            ),
        }
    } else {
        if proxy_resources {
            tracing::debug!(
                upstream = %name,
                capability = "resources",
                "initialize did not advertise resources; skipping resources/list discovery"
            );
        }
        (0, None, UpstreamHealth::Healthy)
    };

    let (prompt_count, prompt_error, prompt_health) = if proxy_prompts
        && peer_declares_prompts(peer)
    {
        tracing::info!(upstream = %name, capability = "prompts", "starting upstream capability discovery");
        match tokio::time::timeout(DISCOVERY_TIMEOUT, peer.list_prompts(None)).await {
            Ok(Ok(result)) => (result.prompts.len(), None, UpstreamHealth::Healthy),
            Ok(Err(ref error)) if is_capability_unsupported(error) => {
                (0, None, UpstreamHealth::Healthy)
            }
            Ok(Err(error)) => (
                0,
                Some(format!("failed to list prompts from upstream: {error}")),
                UpstreamHealth::Unhealthy {
                    consecutive_failures: 1,
                },
            ),
            Err(_) => (
                0,
                Some(format!(
                    "listing prompts from upstream timed out after {}s",
                    DISCOVERY_TIMEOUT.as_secs()
                )),
                UpstreamHealth::Unhealthy {
                    consecutive_failures: 1,
                },
            ),
        }
    } else {
        if proxy_prompts {
            tracing::debug!(
                upstream = %name,
                capability = "prompts",
                "initialize did not advertise prompts; skipping prompts/list discovery"
            );
        }
        (0, None, UpstreamHealth::Healthy)
    };

    if let Some(error) = &resource_error {
        tracing::warn!(upstream = %name, error = %error, "failed to discover upstream resources");
    }
    if let Some(error) = &prompt_error {
        tracing::warn!(upstream = %name, error = %error, "failed to discover upstream prompts");
    }

    (
        resource_count,
        resource_error,
        resource_health,
        prompt_count,
        prompt_error,
        prompt_health,
    )
}
