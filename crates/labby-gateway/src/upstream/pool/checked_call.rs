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
    Connect(String),
    Catalog { kind: &'static str, message: String },
    MissingTool,
    Unavailable,
}

impl UpstreamPool {
    /// Resolve, check, and dispatch against one exact peer while pool drain and
    /// OAuth invalidation wait. The descriptor is re-listed from that peer, so
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
        let started = Instant::now();
        let tool_name = params.name.to_string();
        if !resolve_request_exposure_policy(&config.name, config.expose_tools.clone())
            .matches(&tool_name)
        {
            return Err(CheckedToolCallError::MissingTool);
        }

        if config.oauth.is_some() {
            self.drain_oauth_client_capacity_evictions().await;
        }
        let oauth_epoch = if config.oauth.is_some() {
            self.oauth_lifecycle_epoch()
        } else {
            None
        };
        let subject;
        let subject_tools;
        let peer = if config.oauth.is_some() {
            subject = oauth_subject.ok_or_else(|| {
                CheckedToolCallError::Connect(format!(
                    "upstream `{}` requires an authenticated subject",
                    config.name
                ))
            })?;
            let (peer, tools) = self
                .acquire_or_connect_subject_guarded(config, subject)
                .await
                .map_err(|error| CheckedToolCallError::Connect(error.to_string()))?;
            subject_tools = Some(tools);
            peer
        } else {
            subject = "";
            subject_tools = None;
            self.acquire_peer(&config.name, UpstreamCapability::Tools, "tool.checked_call")
                .await
                .ok_or(CheckedToolCallError::Unavailable)?
        };

        let tools = if let Some(tools) = subject_tools {
            tools
        } else {
            let listing_deadline = self.request_timeout.min(DISCOVERY_TIMEOUT);
            catalog_pagination::list_tools(&peer, listing_deadline, MAX_UPSTREAM_TOOLS)
                .await
                .map_err(|error| CheckedToolCallError::Catalog {
                    kind: error.kind(),
                    message: error.bounded_text(),
                })?
        };
        let tool = tools
            .into_iter()
            .find(|candidate| candidate.name.as_ref() == tool_name)
            .ok_or(CheckedToolCallError::MissingTool)?;
        let (_, exact_tool) = cached_upstream_tool(tool, &Arc::from(config.name.as_str()));
        let checked = check(&exact_tool).map_err(CheckedToolCallError::Check)?;

        let event = UpstreamRequestLog::tool(&config.name, &tool_name, config.oauth.is_some());
        log_upstream_request_start(event);
        let timeout_ms = self.request_timeout.as_millis();
        let result = timed_capability_call(
            self,
            &config.name,
            UpstreamCapability::Tools,
            event,
            started,
            call_tool_with_header_recovery(self, &peer, &config.name, params),
            estimate_response_size,
            (!subject.is_empty()).then_some(subject),
            |error| format!("upstream call failed: {error}"),
            format!("upstream call timed out after {timeout_ms}ms"),
        )
        .await
        .map_err(CheckedToolCallError::Capability)?;
        self.oauth_publication_guard(oauth_epoch)
            .await
            .map_err(|error| CheckedToolCallError::Connect(error.to_string()))?;

        Ok(CheckedToolCall {
            result,
            checked,
            catalog_revision: self.revision_label(),
        })
    }
}
