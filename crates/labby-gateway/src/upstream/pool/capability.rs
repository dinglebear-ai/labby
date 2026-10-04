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
use super::catalog_pagination;
use super::helpers::DISCOVERY_TIMEOUT;
use super::logging::{
    UpstreamRequestLog, is_capability_unsupported, log_upstream_capability_skipped,
};
use super::tools::{MAX_UPSTREAM_PROMPTS, MAX_UPSTREAM_RESOURCES};

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
        match catalog_pagination::list_resources(peer, DISCOVERY_TIMEOUT, MAX_UPSTREAM_RESOURCES)
            .await
        {
            Ok(result) => (result.len(), None, UpstreamHealth::Healthy),
            Err(catalog_pagination::CatalogPaginationError::Service(ref error))
                if is_capability_unsupported(error) =>
            {
                (0, None, UpstreamHealth::Healthy)
            }
            Err(error) => (
                0,
                Some(format!(
                    "{} {}",
                    super::helpers::UPSTREAM_RESOURCE_LISTING_ERROR_PREFIX,
                    error.bounded_text()
                )),
                UpstreamHealth::Unhealthy {
                    consecutive_failures: 1,
                },
            ),
        }
    } else {
        if proxy_resources {
            log_upstream_capability_skipped(UpstreamRequestLog::resources_list(name, false));
        }
        (0, None, UpstreamHealth::Healthy)
    };

    let (prompt_count, prompt_error, prompt_health) = if proxy_prompts
        && peer_declares_prompts(peer)
    {
        tracing::info!(upstream = %name, capability = "prompts", "starting upstream capability discovery");
        match catalog_pagination::list_prompts(peer, DISCOVERY_TIMEOUT, MAX_UPSTREAM_PROMPTS).await
        {
            Ok(result) => (result.len(), None, UpstreamHealth::Healthy),
            Err(catalog_pagination::CatalogPaginationError::Service(ref error))
                if is_capability_unsupported(error) =>
            {
                (0, None, UpstreamHealth::Healthy)
            }
            Err(error) => (
                0,
                Some(format!(
                    "{} {}",
                    super::helpers::UPSTREAM_PROMPT_LISTING_ERROR_PREFIX,
                    error.bounded_text()
                )),
                UpstreamHealth::Unhealthy {
                    consecutive_failures: 1,
                },
            ),
        }
    } else {
        if proxy_prompts {
            log_upstream_capability_skipped(UpstreamRequestLog::prompts_list(name, false));
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

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{
        ErrorData, ListPromptsResult, ListResourcesResult, PaginatedRequestParams, Prompt,
        Resource, ServerCapabilities, ServerInfo,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler, ServiceExt};

    #[derive(Clone)]
    struct PaginatedCapabilities {
        repeat_cursor: bool,
    }

    impl PaginatedCapabilities {
        fn cursor(&self, request: &Option<PaginatedRequestParams>) -> Option<String> {
            (self.repeat_cursor || request.as_ref().and_then(|p| p.cursor.as_ref()).is_none())
                .then(|| "page-two".to_owned())
        }
    }

    impl ServerHandler for PaginatedCapabilities {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(
                ServerCapabilities::builder()
                    .enable_resources()
                    .enable_prompts()
                    .build(),
            )
        }

        async fn list_resources(
            &self,
            request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            let mut result = ListResourcesResult::with_all_items(vec![Resource::new(
                "test://resource",
                "resource",
            )]);
            result.next_cursor = self.cursor(&request);
            Ok(result)
        }

        async fn list_prompts(
            &self,
            request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListPromptsResult, ErrorData> {
            let mut result = ListPromptsResult::with_all_items(vec![Prompt::new(
                "prompt",
                None::<String>,
                None,
            )]);
            result.next_cursor = self.cursor(&request);
            Ok(result)
        }
    }

    async fn counts(
        repeat_cursor: bool,
    ) -> (
        usize,
        Option<String>,
        UpstreamHealth,
        usize,
        Option<String>,
        UpstreamHealth,
    ) {
        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let task = tokio::spawn(async move {
            let service = PaginatedCapabilities { repeat_cursor }
                .serve(server_transport)
                .await
                .unwrap();
            service.waiting().await.unwrap();
        });
        let client = ().serve(client_transport).await.unwrap();
        let result = discover_capability_counts("fixture", client.peer(), true, true).await;
        client.cancel().await.unwrap();
        task.await.unwrap();
        result
    }

    #[tokio::test]
    async fn capability_discovery_counts_every_page() {
        let (resources, resource_error, resource_health, prompts, prompt_error, prompt_health) =
            counts(false).await;
        assert_eq!((resources, prompts), (2, 2));
        assert!(resource_error.is_none() && prompt_error.is_none());
        assert!(matches!(resource_health, UpstreamHealth::Healthy));
        assert!(matches!(prompt_health, UpstreamHealth::Healthy));
    }

    #[tokio::test]
    async fn capability_discovery_rejects_cursor_cycles() {
        let (resources, resource_error, resource_health, prompts, prompt_error, prompt_health) =
            counts(true).await;
        assert_eq!((resources, prompts), (0, 0));
        assert!(resource_error.unwrap().contains("repeated"));
        assert!(prompt_error.unwrap().contains("repeated"));
        assert!(matches!(resource_health, UpstreamHealth::Unhealthy { .. }));
        assert!(matches!(prompt_health, UpstreamHealth::Unhealthy { .. }));
    }
}
