//! Atomic contract-checked tool invocation on one exact upstream peer.

use std::sync::Arc;
use std::time::Instant;

use labby_codemode::CodeModeCallError;
use labby_runtime::gateway_config::UpstreamConfig;
use rmcp::model::{CallToolRequestParams, CallToolResult};

use super::super::types::{UpstreamCapability, UpstreamTool};
use super::UpstreamPool;
use super::capability_call::{CapabilityCallError, timed_capability_call};
use super::catalog_pagination;
use super::entries::resolve_request_exposure_policy;
use super::helpers::{DISCOVERY_TIMEOUT, cached_upstream_tool, estimate_response_size};
use super::logging::{UpstreamRequestLog, log_upstream_request_start};
use super::tools::MAX_UPSTREAM_TOOLS;
use super::tools_call::call_tool_with_header_recovery;

pub(crate) struct CheckedToolCall<T> {
    pub(crate) result: CallToolResult,
    pub(crate) checked: T,
    pub(crate) catalog_revision: String,
}

#[derive(Debug)]
pub(crate) enum CheckedToolCallError {
    Check(Box<CodeModeCallError>),
    Capability(CapabilityCallError),
    SubjectRequired,
    Connect(anyhow::Error),
    AuthorityChangedAfterDispatch,
    Catalog { kind: &'static str, message: String },
    MissingTool,
    Unavailable,
}

impl UpstreamPool {
    /// Resolve, check, and dispatch against one exact peer while pool drain and
    /// OAuth invalidation wait. A published descriptor is used only with its exact connection incarnation, so
    /// validation and authoritative safety classification cannot be paired with
    /// a later connection generation.
    pub(crate) async fn checked_call_tool<T>(
        &self,
        config: &UpstreamConfig,
        oauth_subject: Option<&str>,
        params: CallToolRequestParams,
        check: impl FnOnce(&UpstreamTool) -> Result<T, Box<CodeModeCallError>>,
    ) -> Result<CheckedToolCall<T>, CheckedToolCallError> {
        let _pool_generation = self.invocation_barrier.read().await;
        let tool_name = params.name.to_string();
        if !resolve_request_exposure_policy(&config.name, config.expose_tools.clone())
            .matches(&tool_name)
        {
            return Err(CheckedToolCallError::MissingTool);
        }

        self.drain_oauth_client_capacity_evictions().await;
        let oauth_epoch = if config.oauth.is_some() {
            oauth_subject.and_then(|subject| self.oauth_lifecycle_epoch(&config.name, subject))
        } else {
            None
        };
        let subject = oauth_subject.filter(|_| config.oauth.is_some());
        let (peer, exact_tool) = if config.oauth.is_some() {
            let subject = subject.ok_or(CheckedToolCallError::SubjectRequired)?;
            let (peer, tools) = self
                .acquire_or_connect_subject_guarded(config, subject)
                .await
                .map_err(CheckedToolCallError::Connect)?;
            let cache = self.subject_connections.read().await;
            let entry = cache
                .get(&(config.name.clone(), subject.to_owned()))
                .filter(|entry| Arc::ptr_eq(&entry.tools, &tools))
                .ok_or(CheckedToolCallError::Unavailable)?;
            let tool = entry
                .routed_tool(&config.name, &tool_name, None)
                .ok_or(CheckedToolCallError::MissingTool)?;
            (peer, tool.clone())
        } else {
            let (peer, cached) = {
                let _binding = self.connection_catalog_binding.read().await;
                let connections = self.connections.read().await;
                let connection = connections
                    .get(&config.name)
                    .ok_or(CheckedToolCallError::Unavailable)?;
                let catalog = self.catalog.read().await;
                let cached = if connection.incarnation.is_some()
                    && connection.incarnation == catalog.incarnation(&config.name)
                {
                    let entry = catalog
                        .get(&config.name)
                        .filter(|entry| entry.tool_health.is_routable())
                        .ok_or(CheckedToolCallError::Unavailable)?;
                    if !entry.exposure_policy.matches(&tool_name) {
                        return Err(CheckedToolCallError::MissingTool);
                    }
                    Some(
                        entry
                            .tools
                            .get(&tool_name)
                            .ok_or(CheckedToolCallError::MissingTool)?
                            .clone(),
                    )
                } else {
                    None
                };
                (connection.peer.clone(), cached)
            };
            let tool = if let Some(tool) = cached {
                tool
            } else {
                let budget = self.request_timeout.min(DISCOVERY_TIMEOUT);
                let _permit =
                    tokio::time::timeout(budget, self.acquire_upstream_call_permit(&config.name))
                        .await
                        .map_err(|_| CheckedToolCallError::Catalog {
                            kind: "timeout",
                            message: "upstream catalog admission timed out".into(),
                        })?
                        .map_err(|message| CheckedToolCallError::Catalog {
                            kind: "upstream_error",
                            message,
                        })?;
                let tools = catalog_pagination::list_tools(&peer, budget, MAX_UPSTREAM_TOOLS)
                    .await
                    .map_err(|error| CheckedToolCallError::Catalog {
                        kind: error.kind(),
                        message: error.bounded_text(),
                    })?;
                let tool = tools
                    .into_iter()
                    .find(|candidate| candidate.name.as_ref() == tool_name)
                    .ok_or(CheckedToolCallError::MissingTool)?;
                cached_upstream_tool(tool, &Arc::from(config.name.as_str())).1
            };
            (peer, tool)
        };
        let checked = check(&exact_tool).map_err(CheckedToolCallError::Check)?;

        let event = UpstreamRequestLog::tool(&config.name, &tool_name, config.oauth.is_some());
        log_upstream_request_start(event);
        let timeout_ms = self.request_timeout.as_millis();
        let result = timed_capability_call(
            self,
            &config.name,
            UpstreamCapability::Tools,
            event,
            Instant::now(),
            call_tool_with_header_recovery(self, &peer, &config.name, params),
            estimate_response_size,
            subject,
            |error| format!("upstream call failed: {error}"),
            format!("upstream call timed out after {timeout_ms}ms"),
        )
        .await
        .map_err(CheckedToolCallError::Capability)?;
        self.oauth_publication_guard(oauth_epoch.as_ref())
            .await
            .map_err(|_| CheckedToolCallError::AuthorityChangedAfterDispatch)?;

        Ok(CheckedToolCall {
            result,
            checked,
            catalog_revision: self.revision_label(),
        })
    }
}
