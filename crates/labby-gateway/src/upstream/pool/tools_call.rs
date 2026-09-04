//! Tool invocation: `subject_scoped_call_tool` (OAuth-subject-aware) and
//! `call_tool`. Both acquire the upstream peer, invoke the tool with a request
//! timeout, enforce the response-size cap, and emit structured request logs.

use std::time::Instant;

use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult};
use rmcp::service::Peer;
use rmcp::{RoleClient, ServiceError};

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::capability_call::{
    CapabilityCallError, bounded_service_error_text, timed_capability_call,
    timed_capability_call_str, timed_capability_call_with_timeout,
};
use super::catalog_pagination;
use super::entries::resolve_request_exposure_policy;
use super::helpers::{
    DISCOVERY_TIMEOUT, estimate_call_tool_response_size, estimate_response_size, upstream_transport,
};
use super::logging::{UpstreamRequestLog, log_upstream_request_error, log_upstream_request_start};
use super::tool_call_cancel::{call_tool_cancel_aware, call_tool_once_cancel_aware};
use super::tools::MAX_UPSTREAM_TOOLS;

/// Fail-closed `expose_tools` check for the OAuth subject-scoped call paths.
///
/// The catalog-backed (non-OAuth) path gets this for free: a hidden tool is
/// absent from `find_tool*` / `healthy_tools_for_upstream`, so no upstream owner
/// resolves and the call is never routed. The subject-scoped path has no catalog
/// entry, so the equivalent guarantee has to be enforced from the live
/// `UpstreamConfig` — here, at the execution primitive itself rather than only in
/// the caller that resolves the owner. Keeping it here means the pool API is safe
/// for every caller, including the `pre_resolved_oauth_config` branch in
/// `crates/labby/src/mcp/call_tool_upstream.rs` that resolves its owner from the
/// catalog and therefore never passes through `subject_scoped_tools`.
///
/// Uses the same fail-closed policy helper as every other exposure decision, so
/// an unparseable allowlist blocks the call.
pub(super) fn subject_scoped_tool_is_exposed(config: &UpstreamConfig, tool_name: &str) -> bool {
    resolve_request_exposure_policy(&config.name, config.expose_tools.clone()).matches(tool_name)
}

/// Error returned when `expose_tools` hides the requested tool.
///
/// Deliberately identical in shape to a missing tool: an excluded tool must not
/// be distinguishable from one the upstream never advertised.
pub(super) fn hidden_tool_error(config: &UpstreamConfig, tool_name: &str) -> String {
    tracing::warn!(
        upstream = %config.name,
        "refusing subject-scoped call to a tool hidden by the upstream exposure policy"
    );
    format!(
        "upstream `{}` does not expose tool `{tool_name}`",
        config.name
    )
}

/// [`hidden_tool_error`] as a classified failure.
///
/// `Mcp` + `METHOD_NOT_FOUND`, not `Transport`: this is a permanent policy
/// decision, and `mcp_error_data_kind` maps `METHOD_NOT_FOUND` to the
/// `unknown_tool` stable kind (`upstream/tool_error.rs`). A `Transport` class
/// would instead fall through the string classifier to `upstream_error`, whose
/// recovery contract tells the agent to retry — a retry loop against a denial
/// that can never succeed. `unknown_tool` also keeps the response
/// indistinguishable from a tool the upstream never advertised.
pub(super) fn hidden_tool_call_error(
    config: &UpstreamConfig,
    tool_name: &str,
) -> CapabilityCallError {
    let message = hidden_tool_error(config, tool_name);
    CapabilityCallError::Mcp {
        data: rmcp::model::ErrorData::new(
            rmcp::model::ErrorCode::METHOD_NOT_FOUND,
            message.clone(),
            None,
        ),
        message: message.clone(),
    }
}

/// Whether the server rejected a request before dispatch because its SEP-2243
/// routing headers were missing or stale. rmcp models this as the dedicated
/// `HEADER_MISMATCH` code; the human-readable message is intentionally not part
/// of the contract and may contain only the concrete validation reason.
pub(super) fn is_tool_header_mismatch(error: &ServiceError) -> bool {
    matches!(
        error,
        ServiceError::McpError(data) if data.code == rmcp::model::ErrorCode::HEADER_MISMATCH
    )
}

pub(super) fn record_header_mismatch(pool: &UpstreamPool, upstream_name: &str) {
    let mismatch_count = pool.record_header_mismatch_detected(upstream_name);
    tracing::warn!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "tool.header_mismatch",
        event = "detected",
        upstream = upstream_name,
        mismatch_count,
        "upstream rejected tools/call because SEP-2243 routing headers were stale or missing"
    );
}

pub(super) fn record_header_retry<T>(
    pool: &UpstreamPool,
    upstream_name: &str,
    result: &Result<T, ServiceError>,
) {
    match result {
        Ok(_) => {
            let retry_success_count = pool.record_header_schema_retry_success(upstream_name);
            tracing::info!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "tool.header_cache.retry",
                event = "finish",
                upstream = upstream_name,
                retry_success_count,
                "SEP-2243 schema refresh retry completed successfully"
            );
        }
        Err(error) => {
            let retry_failure_count = pool.record_header_schema_retry_failure(upstream_name);
            tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "tool.header_cache.retry",
                event = "error",
                upstream = upstream_name,
                retry_failure_count,
                error = %bounded_service_error_text(error),
                "SEP-2243 schema refresh retry failed"
            );
        }
    }
}

/// Refresh rmcp's per-transport tool-schema cache after HeaderMismatch. A
/// successful tools/list response is consumed by rmcp itself and repopulates
/// the x-mcp-header annotations used to build Mcp-Param-* on the retry.
pub(super) async fn refresh_tool_header_cache(
    pool: &UpstreamPool,
    peer: &Peer<RoleClient>,
    upstream_name: &str,
) -> Result<(), ServiceError> {
    let schema_refresh_count = pool.record_header_schema_refresh(upstream_name);
    tracing::info!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "tool.header_cache.refresh",
        event = "start",
        upstream = upstream_name,
        schema_refresh_count,
        "refreshing upstream tool schemas after SEP-2243 header mismatch"
    );
    match refresh_tool_header_cache_raw(peer, DISCOVERY_TIMEOUT).await {
        Ok(tools) => {
            tracing::info!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "tool.header_cache.refresh",
                event = "finish",
                upstream = upstream_name,
                schema_refresh_count,
                tool_count = tools.len(),
                "refreshed upstream tool schemas after SEP-2243 header mismatch"
            );
            Ok(())
        }
        Err(error) => {
            tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "tool.header_cache.refresh",
                event = "error",
                upstream = upstream_name,
                schema_refresh_count,
                kind = error.kind(),
                error = %error.bounded_text(),
                "failed to refresh upstream tool schemas after SEP-2243 header mismatch"
            );
            Err(error.into_service_error(upstream_name))
        }
    }
}

/// Refresh rmcp's peer-local SEP-2243 schema hint without logs or counters.
/// The cache is transport compatibility state, never routing authority.
pub(super) async fn refresh_tool_header_cache_raw(
    peer: &Peer<RoleClient>,
    timeout: std::time::Duration,
) -> Result<Vec<rmcp::model::Tool>, catalog_pagination::CatalogPaginationError> {
    catalog_pagination::list_tools(peer, timeout, MAX_UPSTREAM_TOOLS).await
}

pub(super) async fn call_tool_once_with_header_recovery(
    pool: &UpstreamPool,
    peer: &Peer<RoleClient>,
    upstream_name: &str,
    params: CallToolRequestParams,
) -> Result<CallToolResponse, ServiceError> {
    match peer.call_tool_once(params.clone()).await {
        Err(error) if is_tool_header_mismatch(&error) => {
            record_header_mismatch(pool, upstream_name);
            refresh_tool_header_cache(pool, peer, upstream_name).await?;
            let result = peer.call_tool_once(params).await;
            record_header_retry(pool, upstream_name, &result);
            result
        }
        result => result,
    }
}

/// [`call_tool_once_with_header_recovery`] whose RPC cancels upstream if the
/// returned future is dropped before the response arrives.
async fn call_tool_once_with_header_recovery_cancel_aware(
    pool: &UpstreamPool,
    peer: &Peer<RoleClient>,
    upstream_name: &str,
    params: CallToolRequestParams,
) -> Result<CallToolResponse, ServiceError> {
    match call_tool_once_cancel_aware(peer, upstream_name, params.clone()).await {
        Err(error) if is_tool_header_mismatch(&error) => {
            record_header_mismatch(pool, upstream_name);
            refresh_tool_header_cache(pool, peer, upstream_name).await?;
            let result = call_tool_once_cancel_aware(peer, upstream_name, params).await;
            record_header_retry(pool, upstream_name, &result);
            result
        }
        result => result,
    }
}

pub(super) async fn call_tool_with_header_recovery(
    pool: &UpstreamPool,
    peer: &Peer<RoleClient>,
    upstream_name: &str,
    params: CallToolRequestParams,
) -> Result<CallToolResult, ServiceError> {
    // Cancel-aware unconditionally: Code Mode reaches the pool through this
    // helper and abandons the call future on cancellation
    // (`labby-codemode/src/execute.rs`), so the guard is what stops the
    // upstream — no token needs threading across the crate boundary.
    match call_tool_cancel_aware(peer, upstream_name, params.clone()).await {
        Err(error) if is_tool_header_mismatch(&error) => {
            record_header_mismatch(pool, upstream_name);
            refresh_tool_header_cache(pool, peer, upstream_name).await?;
            let result = call_tool_cancel_aware(peer, upstream_name, params).await;
            record_header_retry(pool, upstream_name, &result);
            result
        }
        result => result,
    }
}

impl UpstreamPool {
    /// Call an OAuth-subject-scoped tool once, preserving MRTR/task outcomes
    /// and the upstream failure class.
    ///
    /// A connect failure surfaces as [`CapabilityCallError::Transport`] and is
    /// NOT recorded against the circuit breaker here (subject-scoped connects
    /// are per-caller credentials, and the pooled upstream connection may be
    /// perfectly healthy).
    ///
    /// `cancel` is the downstream request's token. When it fires the call is
    /// abandoned and the upstream is sent `notifications/cancelled`, so a tool
    /// with side effects stops instead of running on for a caller that has
    /// gone away. `None` is correct only where no downstream request can be
    /// withdrawn.
    pub async fn subject_scoped_call_tool_once_classified(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        params: CallToolRequestParams,
        cancel: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<CallToolResponse, CapabilityCallError> {
        let start = Instant::now();
        let tool_name = params.name.to_string();
        if !subject_scoped_tool_is_exposed(config, &tool_name) {
            return Err(hidden_tool_call_error(config, &tool_name));
        }
        let event = UpstreamRequestLog::tool(&config.name, &tool_name, true)
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        let (peer, _tools) = self
            .acquire_or_connect_subject(config, subject)
            .await
            .map_err(|error| CapabilityCallError::Transport {
                message: error.to_string(),
            })?;
        let timeout_ms = self.request_timeout.as_millis();
        timed_capability_call_with_timeout(
            self,
            self.request_timeout,
            &config.name,
            UpstreamCapability::Tools,
            event,
            start,
            call_tool_once_with_header_recovery_cancel_aware(self, &peer, &config.name, params),
            estimate_call_tool_response_size,
            Some(subject),
            |error| format!("upstream call failed: {error}"),
            format!("upstream call timed out after {timeout_ms}ms"),
            cancel,
        )
        .await
    }

    /// Call an OAuth-subject-scoped tool for Code Mode while preserving the
    /// same complete-result and classified-error contract as
    /// [`Self::call_tool_classified`].
    pub(crate) async fn subject_scoped_call_tool_classified(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        params: CallToolRequestParams,
    ) -> Result<CallToolResult, CapabilityCallError> {
        let start = Instant::now();
        let tool_name = params.name.to_string();
        if !subject_scoped_tool_is_exposed(config, &tool_name) {
            return Err(hidden_tool_call_error(config, &tool_name));
        }
        let event = UpstreamRequestLog::tool(&config.name, &tool_name, true)
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        let (peer, _tools) = self
            .acquire_or_connect_subject(config, subject)
            .await
            .map_err(|error| CapabilityCallError::Transport {
                message: error.to_string(),
            })?;
        let timeout_ms = self.request_timeout.as_millis();
        timed_capability_call(
            self,
            &config.name,
            UpstreamCapability::Tools,
            event,
            start,
            call_tool_with_header_recovery(self, &peer, &config.name, params),
            estimate_response_size,
            Some(subject),
            |error| format!("upstream call failed: {error}"),
            format!("upstream call timed out after {timeout_ms}ms"),
        )
        .await
    }

    /// Call a tool on an OAuth-subject-scoped upstream.
    ///
    /// P-C1 fix: uses `acquire_or_connect_subject` so the per-(upstream,subject)
    /// connection is reused from cache instead of opening a fresh TLS connection
    /// on every call.
    pub async fn subject_scoped_call_tool(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        params: CallToolRequestParams,
    ) -> Result<CallToolResult, String> {
        let start = Instant::now();
        let tool_name = params.name.to_string();
        if !subject_scoped_tool_is_exposed(config, &tool_name) {
            return Err(hidden_tool_error(config, &tool_name));
        }
        let event = UpstreamRequestLog::tool(&config.name, &tool_name, true)
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        let (peer, _tools) = match self.acquire_or_connect_subject(config, subject).await {
            Ok(pair) => pair,
            Err(error) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Tools,
                    format!("upstream connect failed: {error}"),
                )
                .await;
                let elapsed_ms = start.elapsed().as_millis();
                log_upstream_request_error(
                    event,
                    elapsed_ms,
                    "upstream_connect_error",
                    Some(&error),
                    None,
                    None,
                );
                super::usage_record::record_usage_call(
                    self,
                    event,
                    Some(subject),
                    "upstream_connect_error",
                    elapsed_ms,
                );
                return Err(error.to_string());
            }
        };
        let timeout_ms = self.request_timeout.as_millis();
        timed_capability_call_str(
            self,
            &config.name,
            UpstreamCapability::Tools,
            event,
            start,
            call_tool_with_header_recovery(self, &peer, &config.name, params),
            estimate_response_size,
            Some(subject),
            |e| format!("upstream call failed: {e}"),
            format!("upstream call timed out after {timeout_ms}ms"),
        )
        .await
    }

    /// Call a tool on an upstream server.
    ///
    /// Returns `None` if the upstream is not connected or the tool is not found.
    /// Enforces a response size cap (`LABBY_UPSTREAM_MAX_RESPONSE_BYTES`, default 10 MiB).
    ///
    /// Cap layering by transport:
    /// - **HTTP non-OAuth**: cap is enforced at the rmcp transport layer by
    ///   `BodyCappedHttpClient` (see `dispatch/upstream/http_client.rs`) —
    ///   bytes are checked during streaming, *before* allocation.
    /// - **stdio**: cap is post-hoc here (rmcp's stdio transport buffers the
    ///   full JSON response before we see it). The check at the end of this
    ///   function guards against forwarding oversized payloads but cannot
    ///   prevent the underlying allocation.
    /// - **HTTP OAuth**: also post-hoc for now — threading the cap through
    ///   `OauthClientCache` is tracked as a follow-up.
    ///
    /// The post-hoc check below is therefore defense-in-depth for HTTP
    /// non-OAuth and the primary line of defense for stdio / OAuth.
    pub async fn call_tool(
        &self,
        upstream_name: &str,
        params: CallToolRequestParams,
    ) -> Option<Result<CallToolResult, String>> {
        self.call_tool_classified(upstream_name, params)
            .await
            .map(|result| result.map_err(|error| error.to_string()))
    }

    /// Call a tool while preserving the upstream failure class for Code Mode.
    pub(crate) async fn call_tool_classified(
        &self,
        upstream_name: &str,
        params: CallToolRequestParams,
    ) -> Option<Result<CallToolResult, CapabilityCallError>> {
        let start = Instant::now();
        let tool_name = params.name.to_string();
        let event = UpstreamRequestLog::tool(upstream_name, &tool_name, false);
        let peer = self
            .acquire_peer(upstream_name, UpstreamCapability::Tools, "tool.call")
            .await?;
        log_upstream_request_start(event);
        let timeout_ms = self.request_timeout.as_millis();
        Some(
            timed_capability_call(
                self,
                upstream_name,
                UpstreamCapability::Tools,
                event,
                start,
                call_tool_with_header_recovery(self, &peer, upstream_name, params),
                estimate_response_size,
                None,
                |e| format!("upstream call failed: {e}"),
                format!("upstream call timed out after {timeout_ms}ms"),
            )
            .await,
        )
    }

    /// Call a tool once, preserving MRTR `input_required`/task outcomes and the
    /// upstream failure class for the MCP proxy.
    ///
    /// Health accounting is owned entirely by `timed_capability_call`: a
    /// [`CapabilityCallError::Mcp`] means the pool recorded SUCCESS (a valid
    /// JSON-RPC error proves the connection is alive); transport-class
    /// failures were already recorded as breaker failures. Callers must NOT
    /// call `record_failure`/`record_success` again for these outcomes.
    ///
    /// `cancel` is the downstream request's token — see
    /// [`Self::subject_scoped_call_tool_once_classified`] for the contract.
    pub async fn call_tool_once_classified(
        &self,
        upstream_name: &str,
        params: CallToolRequestParams,
        cancel: Option<&tokio_util::sync::CancellationToken>,
    ) -> Option<Result<CallToolResponse, CapabilityCallError>> {
        let start = Instant::now();
        let tool_name = params.name.to_string();
        let event = UpstreamRequestLog::tool(upstream_name, &tool_name, false);
        let peer = self
            .acquire_peer(upstream_name, UpstreamCapability::Tools, "tool.call")
            .await?;
        log_upstream_request_start(event);
        let timeout_ms = self.request_timeout.as_millis();
        Some(
            timed_capability_call_with_timeout(
                self,
                self.request_timeout,
                upstream_name,
                UpstreamCapability::Tools,
                event,
                start,
                call_tool_once_with_header_recovery_cancel_aware(
                    self,
                    &peer,
                    upstream_name,
                    params,
                ),
                estimate_call_tool_response_size,
                None,
                |error| format!("upstream call failed: {error}"),
                format!("upstream call timed out after {timeout_ms}ms"),
                cancel,
            )
            .await,
        )
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
// test fixtures construct upstream Tool values directly
// `panic!` is how tests assert; `panic = "warn"` targets production paths.
#[allow(clippy::panic)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode,
        ErrorData, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo,
    };
    use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};

    use super::super::super::types::{UpstreamHealth, UpstreamRuntimeMetadata};
    use super::super::SubjectScopedConnection;
    use super::super::entries::healthy_in_process_entry;
    use super::super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
    use super::super::testsupport::*;
    use super::super::{UpstreamConnection, UpstreamPool};
    use super::CapabilityCallError;

    #[tokio::test]
    async fn call_tool_times_out_slow_upstream_response() {
        let pool = slow_response_pool("slow").await;

        let result = pool
            .call_tool("slow", CallToolRequestParams::new("slow.tool"))
            .await
            .expect("upstream is connected")
            .expect_err("slow tool call should time out");

        assert!(result.contains("timed out"));
    }

    /// The classified once-variant (the MCP proxy's pooled path) records a
    /// transport-class failure EXACTLY once. The proxy layer must not record
    /// again on top of this — a double count would halve the effective
    /// `CIRCUIT_BREAKER_THRESHOLD` (bead `lab-ak0mh`).
    #[tokio::test]
    async fn call_tool_once_classified_timeout_records_exactly_one_failure() {
        let pool = slow_response_pool("slow").await;

        let error = pool
            .call_tool_once_classified("slow", CallToolRequestParams::new("slow.tool"), None)
            .await
            .expect("upstream is connected")
            .expect_err("slow tool call should time out");

        assert!(
            matches!(error, CapabilityCallError::Timeout { .. }),
            "expected Timeout class, got {error:?}"
        );
        assert!(
            matches!(
                pool.upstream_tool_health("slow").await,
                Some(UpstreamHealth::Unhealthy {
                    consecutive_failures: 1
                })
            ),
            "one transport failure must be recorded exactly once"
        );
        assert!(
            pool.upstream_tool_last_error("slow")
                .await
                .expect("timeout recorded as last error")
                .contains("timed out")
        );
    }

    /// The classified once-variant preserves the MCP application-error class
    /// and the pool records SUCCESS for it: a valid JSON-RPC error proves the
    /// connection is alive, so health stays `Healthy` and no last-error is set.
    #[tokio::test]
    async fn call_tool_once_classified_mcp_error_keeps_health_and_class() {
        let pool = UpstreamPool::new();
        pool.insert_mcp_error_server_for_tests(
            "rejecting",
            ErrorData::invalid_params("unknown field `since`", None),
        )
        .await;

        let error = pool
            .call_tool_once_classified("rejecting", CallToolRequestParams::new("reject.tool"), None)
            .await
            .expect("upstream is connected")
            .expect_err("application error should reach the caller");

        assert!(
            matches!(error, CapabilityCallError::Mcp { .. }),
            "expected Mcp class, got {error:?}"
        );
        assert_eq!(pool.upstream_tool_last_error("rejecting").await, None);
        assert!(
            matches!(
                pool.upstream_tool_health("rejecting").await,
                Some(UpstreamHealth::Healthy)
            ),
            "valid MCP error response must not poison connection health"
        );
    }

    #[tokio::test]
    async fn header_mismatch_refreshes_tool_schema_and_retries_once() {
        struct HeaderMismatchServer {
            list_calls: Arc<AtomicUsize>,
            tool_calls: Arc<AtomicUsize>,
        }

        impl ServerHandler for HeaderMismatchServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }

            async fn list_tools(
                &self,
                _: Option<PaginatedRequestParams>,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<ListToolsResult, ErrorData> {
                self.list_calls.fetch_add(1, Ordering::SeqCst);
                Ok(ListToolsResult::with_all_items(vec![
                    rmcp::model::Tool::new(
                        "header.tool",
                        "requires refreshed SEP-2243 parameter metadata",
                        Arc::new(serde_json::Map::new()),
                    ),
                ]))
            }

            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                let attempt = self.tool_calls.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    return Err(ErrorData::new(
                        ErrorCode::HEADER_MISMATCH,
                        "header mismatch: missing Mcp-Param-owner header for parameter \"owner\"",
                        None,
                    ));
                }
                Ok(CallToolResult::success(vec![ContentBlock::text("recovered")]).into())
            }
        }

        let list_calls = Arc::new(AtomicUsize::new(0));
        let tool_calls = Arc::new(AtomicUsize::new(0));
        let server = HeaderMismatchServer {
            list_calls: Arc::clone(&list_calls),
            tool_calls: Arc::clone(&tool_calls),
        };
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_task = tokio::spawn(async move {
            let running = server
                .serve(server_transport)
                .await
                .expect("header mismatch server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("header mismatch client starts");
        let peer = client_service.peer().clone();

        let upstream_name = "header-mismatch";
        let pool = Arc::new(UpstreamPool::new());
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        pool.catalog_write().await.insert(
            upstream_name.to_string(),
            healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new()),
        );
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );

        let response = pool
            .call_tool_once_classified(
                upstream_name,
                CallToolRequestParams::new("header.tool"),
                None,
            )
            .await
            .expect("upstream is connected")
            .expect("HeaderMismatch should self-heal after one schema refresh");

        assert!(matches!(response, CallToolResponse::Complete(_)));
        assert_eq!(
            list_calls.load(Ordering::SeqCst),
            1,
            "recovery must perform exactly one tools/list refresh"
        );
        assert_eq!(
            tool_calls.load(Ordering::SeqCst),
            2,
            "HeaderMismatch must be replayed exactly once"
        );
        let metrics = pool.header_recovery_metrics(upstream_name);
        assert_eq!(metrics.mismatch_detected, 1);
        assert_eq!(metrics.schema_refreshes, 1);
        assert_eq!(metrics.retry_successes, 1);
        assert_eq!(metrics.retry_failures, 0);
    }

    #[test]
    fn header_mismatch_detection_uses_the_rmcp_error_code() {
        let ordinary = rmcp::ServiceError::McpError(ErrorData::invalid_params(
            "ordinary validation failure",
            None,
        ));
        assert!(!super::is_tool_header_mismatch(&ordinary));

        let sdk_header_mismatch = rmcp::ServiceError::McpError(ErrorData::header_mismatch(
            "missing Mcp-Param-owner header for `owner`",
            None,
        ));
        assert!(super::is_tool_header_mismatch(&sdk_header_mismatch));

        let same_message_wrong_code = rmcp::ServiceError::McpError(ErrorData::internal_error(
            "header mismatch: application-level message only",
            None,
        ));
        assert!(!super::is_tool_header_mismatch(&same_message_wrong_code));
    }

    #[tokio::test]
    async fn call_tool_mcp_error_keeps_connection_healthy() {
        struct RejectingServer;
        impl ServerHandler for RejectingServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }

            async fn list_tools(
                &self,
                _: Option<PaginatedRequestParams>,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<ListToolsResult, ErrorData> {
                Ok(ListToolsResult::with_all_items(vec![
                    rmcp::model::Tool::new(
                        "reject.tool",
                        "returns an MCP application error",
                        Arc::new(serde_json::Map::new()),
                    ),
                ]))
            }

            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                Err(ErrorData::internal_error(
                    "forbidden: requires scope: example:write".to_string(),
                    None,
                ))
            }
        }

        let upstream_name = "rejecting";
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_task = tokio::spawn(async move {
            let running = RejectingServer
                .serve(server_transport)
                .await
                .expect("rejecting server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("rejecting client starts");
        let peer = client_service.peer().clone();

        let pool = Arc::new(UpstreamPool::new());
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        pool.catalog_write().await.insert(
            upstream_name.to_string(),
            healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new()),
        );
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );

        let result = pool
            .call_tool(upstream_name, CallToolRequestParams::new("reject.tool"))
            .await
            .expect("upstream is connected")
            .expect_err("application error should reach the caller");

        assert!(result.contains("requires scope"));
        assert_eq!(pool.upstream_tool_last_error(upstream_name).await, None);
        assert!(
            pool.upstream_tool_health(upstream_name)
                .await
                .expect("health entry")
                .is_routable(),
            "valid MCP error response must not poison connection health"
        );
    }

    /// A multi-MB JSON-RPC error payload (message + data) is bounded at the
    /// pool boundary: the retained `CapabilityCallError::Mcp` never carries
    /// the unbounded upstream payload, and the stringified error stays small.
    #[tokio::test]
    async fn call_tool_huge_mcp_error_payload_is_bounded() {
        struct HugeErrorServer;
        impl ServerHandler for HugeErrorServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }

            async fn list_tools(
                &self,
                _: Option<PaginatedRequestParams>,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<ListToolsResult, ErrorData> {
                Ok(ListToolsResult::with_all_items(vec![
                    rmcp::model::Tool::new(
                        "huge.error",
                        "returns a multi-MB error payload",
                        Arc::new(serde_json::Map::new()),
                    ),
                ]))
            }

            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                let huge = "e".repeat(3 * 1024 * 1024);
                Err(ErrorData::internal_error(
                    format!("upstream exploded: {huge}"),
                    Some(serde_json::json!({ "detail": huge })),
                ))
            }
        }

        let upstream_name = "huge-error";
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_task = tokio::spawn(async move {
            let running = HugeErrorServer
                .serve(server_transport)
                .await
                .expect("huge error server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("huge error client starts");
        let peer = client_service.peer().clone();

        let pool = Arc::new(UpstreamPool::new());
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        pool.catalog_write().await.insert(
            upstream_name.to_string(),
            healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new()),
        );
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );

        let error = pool
            .call_tool_classified(upstream_name, CallToolRequestParams::new("huge.error"))
            .await
            .expect("upstream is connected")
            .expect_err("huge error payload should surface as an error");

        let CapabilityCallError::Mcp { data, message } = &error else {
            panic!("expected Mcp error variant, got: {error:?}");
        };
        assert!(
            message.len() < 32 * 1024,
            "stringified message must be bounded, got {} bytes",
            message.len()
        );
        assert!(
            message.contains("upstream exploded"),
            "the leading upstream message must survive bounding"
        );
        assert!(
            data.message.len() < 32 * 1024,
            "retained ErrorData message must be bounded, got {} bytes",
            data.message.len()
        );
        let serialized_data = serde_json::to_vec(&data.data).expect("bounded data serializes");
        assert!(
            serialized_data.len() < 8 * 1024,
            "retained ErrorData data payload must be bounded, got {} bytes",
            serialized_data.len()
        );
    }

    /// T9: an upstream that returns an oversized body gets a structured cap error,
    /// not a panic or OOM.
    #[tokio::test]
    async fn call_tool_oversized_response_returns_cap_error() {
        struct OversizedServer;
        impl ServerHandler for OversizedServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }
            async fn list_tools(
                &self,
                _: Option<PaginatedRequestParams>,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<ListToolsResult, ErrorData> {
                Ok(ListToolsResult::with_all_items(vec![
                    rmcp::model::Tool::new(
                        "big.tool",
                        "returns huge payload",
                        Arc::new(serde_json::Map::new()),
                    ),
                ]))
            }
            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                // 12 MiB of 'x' characters — above the default 10 MiB cap.
                let payload = "x".repeat(12 * 1024 * 1024);
                Ok(CallToolResult::success(vec![ContentBlock::text(payload)]).into())
            }
        }

        let upstream_name = "oversized";
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_task = tokio::spawn(async move {
            let running = OversizedServer
                .serve(server_transport)
                .await
                .expect("oversized server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("oversized client starts");
        let peer = client_service.peer().clone();

        let pool = Arc::new(UpstreamPool::new());
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        pool.catalog_write().await.insert(
            upstream_name.to_string(),
            healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new()),
        );
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );

        let result = pool
            .call_tool(upstream_name, CallToolRequestParams::new("big.tool"))
            .await
            .expect("upstream is connected")
            .expect_err("oversized response should be rejected");

        assert!(
            result.contains("too large"),
            "expected 'too large' in error, got: {result}"
        );
        assert!(
            result.contains("bytes"),
            "expected byte count in error, got: {result}"
        );
        assert_eq!(pool.upstream_tool_last_error(upstream_name).await, None);
        assert!(
            pool.upstream_tool_health(upstream_name)
                .await
                .expect("health entry")
                .is_routable(),
            "gateway response cap must not poison connection health"
        );
    }

    /// T6/T8: two sequential Code Mode calls for the same (upstream, subject)
    /// reuse the subject cache instead of falling back to the shared peer.
    #[tokio::test]
    async fn subject_connection_cache_reuse_no_new_discovery() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        #[derive(Clone, Default)]
        struct CountingServer {
            init_count: Arc<AtomicUsize>,
        }

        impl ServerHandler for CountingServer {
            fn get_info(&self) -> ServerInfo {
                self.init_count.fetch_add(1, Ordering::SeqCst);
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }
            async fn list_tools(
                &self,
                _: Option<PaginatedRequestParams>,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<ListToolsResult, ErrorData> {
                Ok(ListToolsResult::with_all_items(vec![
                    rmcp::model::Tool::new("echo", "echo tool", Arc::new(serde_json::Map::new())),
                ]))
            }
            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                Ok(CallToolResult::success(vec![]).into())
            }
        }

        let upstream_name = "counting-upstream";
        let init_count = Arc::new(AtomicUsize::new(0));
        let server = CountingServer {
            init_count: Arc::clone(&init_count),
        };

        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let running = server_clone
                .serve(server_transport)
                .await
                .expect("counting server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("counting client starts");
        let peer = client_service.peer().clone();

        // Build pool with a short timeout; seed normal connection for call_tool.
        let pool = Arc::new(UpstreamPool::new().with_request_timeout(Duration::from_secs(5)));
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        pool.catalog_write().await.insert(
            upstream_name.to_string(),
            healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new()),
        );
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer: peer.clone(),
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );

        // Seed the subject_connections cache — simulates what acquire_or_connect_subject
        // stores on a first OAuth connection for (upstream, subject).
        let subject = "user@example.com";
        {
            // Move the connection into the subject cache so we can test reuse.
            let conn = pool
                .connections
                .write()
                .await
                .remove(upstream_name)
                .expect("connection present");
            pool.subject_connections.write().await.insert(
                (upstream_name.to_string(), subject.to_string()),
                SubjectScopedConnection {
                    _connection: conn,
                    peer: peer.clone(),
                    tools: vec![],
                    last_used: Instant::now(),
                },
            );
        }

        let before = init_count.load(Ordering::SeqCst);
        let mut config = test_upstream_config();
        config.name = upstream_name.to_string();

        let r1 = pool
            .subject_scoped_call_tool_classified(
                &config,
                subject,
                CallToolRequestParams::new("echo"),
            )
            .await;
        let r2 = pool
            .subject_scoped_call_tool_classified(
                &config,
                subject,
                CallToolRequestParams::new("echo"),
            )
            .await;

        assert!(r1.is_ok(), "first call should reach subject peer: {r1:?}");
        assert!(r2.is_ok(), "second call should reach subject peer: {r2:?}");

        let after = init_count.load(Ordering::SeqCst);
        assert_eq!(after, before, "subject calls must reuse the cached peer");
    }

    /// Usage telemetry: a successful `call_tool` through the pool writes one
    /// row to the wired `UsageStore`, with capability/tool/upstream/outcome set.
    #[tokio::test]
    async fn call_tool_records_usage_when_store_is_wired() {
        use crate::usage::UsageStore;

        struct EchoServer;
        impl ServerHandler for EchoServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }
            async fn list_tools(
                &self,
                _: Option<PaginatedRequestParams>,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<ListToolsResult, ErrorData> {
                Ok(ListToolsResult::with_all_items(vec![
                    rmcp::model::Tool::new("echo", "echo tool", Arc::new(serde_json::Map::new())),
                ]))
            }
            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                Ok(CallToolResult::success(vec![]).into())
            }
        }

        let upstream_name = "usage-upstream";
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_task = tokio::spawn(async move {
            let running = EchoServer
                .serve(server_transport)
                .await
                .expect("server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> =
            ().serve(client_transport).await.expect("client starts");
        let peer = client_service.peer().clone();

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(UsageStore::open(dir.path().join("usage.db")).await.unwrap());
        let pool = Arc::new(UpstreamPool::new().with_usage_store(Some(Arc::clone(&store))));
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        pool.catalog_write().await.insert(
            upstream_name.to_string(),
            healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new()),
        );
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );

        pool.call_tool(upstream_name, CallToolRequestParams::new("echo"))
            .await
            .expect("upstream is connected")
            .expect("echo call succeeds");

        // The write is fire-and-forget (`tokio::spawn`); give it a beat to land.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let count: i64 = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM upstream_calls WHERE upstream_name = ?1 AND tool_name = ?2 AND outcome = 'ok'",
                    rusqlite::params!["usage-upstream", "echo"],
                    |row| row.get(0),
                )
                .map_err(crate::usage::store::sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    /// End-to-end proof that the write semaphore actually bounds writes: with
    /// every permit held, `call_tool` still succeeds (telemetry is
    /// best-effort and must never affect the call path) but no row lands in
    /// `upstream_calls`, proving the drop path in
    /// `upstream/pool/usage_record.rs` is reached rather than the write
    /// silently succeeding anyway.
    #[tokio::test]
    async fn call_tool_drops_usage_write_when_semaphore_is_saturated() {
        use crate::usage::UsageStore;

        struct EchoServer;
        impl ServerHandler for EchoServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }
            async fn list_tools(
                &self,
                _: Option<PaginatedRequestParams>,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<ListToolsResult, ErrorData> {
                Ok(ListToolsResult::with_all_items(vec![
                    rmcp::model::Tool::new("echo", "echo tool", Arc::new(serde_json::Map::new())),
                ]))
            }
            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                Ok(CallToolResult::success(vec![]).into())
            }
        }

        let upstream_name = "saturated-usage-upstream";
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_task = tokio::spawn(async move {
            let running = EchoServer
                .serve(server_transport)
                .await
                .expect("server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> =
            ().serve(client_transport).await.expect("client starts");
        let peer = client_service.peer().clone();

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(UsageStore::open(dir.path().join("usage.db")).await.unwrap());
        let pool = Arc::new(UpstreamPool::new().with_usage_store(Some(Arc::clone(&store))));
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        pool.catalog_write().await.insert(
            upstream_name.to_string(),
            healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new()),
        );
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );

        // Exhaust every write-semaphore permit before making the call.
        let semaphore = store.write_semaphore();
        let mut held_permits = Vec::new();
        while let Ok(permit) = semaphore.clone().try_acquire_owned() {
            held_permits.push(permit);
        }

        pool.call_tool(upstream_name, CallToolRequestParams::new("echo"))
            .await
            .expect("upstream is connected")
            .expect("echo call succeeds even when usage-write is dropped");

        // Give any (unexpected) fire-and-forget write a beat to land.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        drop(held_permits);

        let count: i64 = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM upstream_calls WHERE upstream_name = ?1",
                    rusqlite::params!["saturated-usage-upstream"],
                    |row| row.get(0),
                )
                .map_err(crate::usage::store::sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "usage write must be dropped, not queued, when the semaphore is saturated"
        );
    }

    /// Usage telemetry: a non-success outcome (upstream timeout) also lands
    /// in the usage store, with `outcome = 'timeout'` — covers the failure
    /// path, not just the `call_tool_records_usage_when_store_is_wired`
    /// success case above. Builds on the same `SlowResponseServer` fixture
    /// used by `call_tool_times_out_slow_upstream_response`.
    #[tokio::test]
    async fn call_tool_records_timeout_outcome_when_store_is_wired() {
        use super::super::testsupport::SlowResponseServer;
        use crate::usage::UsageStore;

        let upstream_name = "timeout-usage-upstream";
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_task = tokio::spawn(async move {
            let running = SlowResponseServer
                .serve(server_transport)
                .await
                .expect("slow response server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("slow response client starts");
        let peer = client_service.peer().clone();

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(UsageStore::open(dir.path().join("usage.db")).await.unwrap());
        let pool = Arc::new(
            UpstreamPool::new()
                .with_request_timeout(std::time::Duration::from_millis(25))
                .with_usage_store(Some(Arc::clone(&store))),
        );
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        pool.catalog_write().await.insert(
            upstream_name.to_string(),
            healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new()),
        );
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );

        let result = pool
            .call_tool(upstream_name, CallToolRequestParams::new("slow.tool"))
            .await
            .expect("upstream is connected")
            .expect_err("slow tool call should time out");
        assert!(result.contains("timed out"));

        // The write is fire-and-forget (`tokio::spawn`); give it a beat to land.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let count: i64 = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM upstream_calls WHERE upstream_name = ?1 AND tool_name = ?2 AND outcome = 'timeout'",
                    rusqlite::params!["timeout-usage-upstream", "slow.tool"],
                    |row| row.get(0),
                )
                .map_err(crate::usage::store::sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
