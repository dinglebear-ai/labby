//! Resource reads: `read_upstream_resource` and `subject_scoped_read_resource`.
//!
//! Both acquire/connect the upstream peer, read the resource with a request
//! timeout, normalize the returned URIs to the gateway form, enforce the
//! response-size cap, and emit structured request logs.
//!
//! Both also enforce `expose_resources`. Filtering only `resources/list` would
//! be a bypass, not a restriction: an excluded resource would stay directly
//! readable by anyone who knows (or guesses) its URI. The read is the gate that
//! actually matters, so it is applied at the single choke point every read
//! funnels through (`read_resource_request_from_peer`) and, for the OAuth path,
//! on the live config in `subject_scoped_read_resource_request`.

use std::time::Instant;

use rmcp::model::{ReadResourceRequestParams, ReadResourceResult};

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::ResourceCatalogGeneration;
use super::UpstreamPool;
use super::capability_call::timed_capability_call_str;
use super::capability_call::{
    RawCallOutcome, classify_timeout_result, service_error_affects_connection_health,
};
use super::entries::{resolve_request_resource_exposure_policy, resource_exposed};
use super::helpers::{
    estimate_resource_response_size, max_response_bytes, normalize_resource_result_uri,
    redact_resource_uri_for_logging, upstream_transport,
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum ExactResourceReadError {
    #[error("published resource target is unavailable")]
    Unavailable,
    #[error("published resource read queue is unavailable")]
    QueueUnavailable,
    #[error("upstream resource read failed")]
    Upstream,
    #[error("upstream resource read timed out")]
    Timeout,
    #[error("upstream resource read was cancelled")]
    Cancelled,
    #[error("upstream resource response is too large")]
    TooLarge,
}

pub(crate) struct PreparedExactResourceRead {
    observed: super::incarnation::ObservedConnectionCatalogEntry,
    generation: ResourceCatalogGeneration,
    native_uri: String,
    gateway_uri: String,
    outcome: RawCallOutcome<ReadResourceResult>,
}
use super::logging::{UpstreamRequestLog, log_upstream_request_error, log_upstream_request_start};
use super::tools::mcp_tool_owns_mcp_app_resource;

impl UpstreamPool {
    /// Read one regular non-OAuth Resource only while its exact publication
    /// (which embodies routability and exposure) and connection incarnation
    /// agree. The legacy `resource_upstreams` list is deliberately not an
    /// authority here. This kernel is deliberately unmounted from handlers.
    pub(crate) async fn read_published_resource_exact(
        &self,
        upstream_name: &str,
        native_uri: &str,
        generation: ResourceCatalogGeneration,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, ExactResourceReadError> {
        let prepared = self
            .prepare_published_resource_exact(upstream_name, native_uri, generation, params)
            .await?;
        self.apply_prepared_resource_exact(prepared).await
    }

    pub(crate) async fn prepare_published_resource_exact(
        &self,
        upstream_name: &str,
        native_uri: &str,
        generation: ResourceCatalogGeneration,
        params: ReadResourceRequestParams,
    ) -> Result<PreparedExactResourceRead, ExactResourceReadError> {
        if params.uri != native_uri {
            return Err(ExactResourceReadError::Unavailable);
        }
        let start = tokio::time::Instant::now();
        let permit = tokio::time::timeout(
            self.request_timeout,
            self.acquire_upstream_call_permit(upstream_name),
        )
        .await;
        let _permit = match permit {
            Ok(Ok(permit)) => permit,
            _ => return Err(ExactResourceReadError::QueueUnavailable),
        };
        let Some(observed) = self
            .observe_resource_call(upstream_name, native_uri, generation)
            .await
        else {
            return Err(ExactResourceReadError::Unavailable);
        };
        let remaining = self.request_timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Err(ExactResourceReadError::QueueUnavailable);
        }
        let outcome = classify_timeout_result(
            tokio::time::timeout(remaining, observed.peer.read_resource(params)).await,
        );
        Ok(PreparedExactResourceRead {
            observed,
            generation,
            native_uri: native_uri.to_string(),
            gateway_uri: format!("lab://upstream/{upstream_name}/{native_uri}"),
            outcome,
        })
    }

    pub(crate) async fn apply_prepared_resource_exact(
        &self,
        prepared: PreparedExactResourceRead,
    ) -> Result<ReadResourceResult, ExactResourceReadError> {
        let upstream_name = prepared.observed.upstream().to_string();
        match prepared.outcome {
            RawCallOutcome::Ok(result) => {
                let result = normalize_resource_result_uri(result, &prepared.gateway_uri);
                let too_large = estimate_resource_response_size(&result) > max_response_bytes();
                let applied = self
                    .apply_to_observed_resource_call(
                        &prepared.observed,
                        prepared.generation,
                        &prepared.native_uri,
                        |entry| {
                            super::health::record_success_on_entry(
                                &upstream_name,
                                entry,
                                UpstreamCapability::Resources,
                            );
                        },
                    )
                    .await;
                if applied.is_none() {
                    return Err(ExactResourceReadError::Unavailable);
                }
                if too_large {
                    Err(ExactResourceReadError::TooLarge)
                } else {
                    Ok(result)
                }
            }
            RawCallOutcome::UpstreamError(error) => {
                let affects_health = service_error_affects_connection_health(&error);
                let message = super::capability_call::bounded_service_error_text(&error);
                let applied = self
                    .apply_to_observed_resource_call(
                        &prepared.observed,
                        prepared.generation,
                        &prepared.native_uri,
                        |entry| {
                            if affects_health {
                                super::health::record_failure_on_entry(
                                    &upstream_name,
                                    entry,
                                    UpstreamCapability::Resources,
                                    format!("upstream resource read failed: {message}"),
                                );
                            } else {
                                super::health::record_success_on_entry(
                                    &upstream_name,
                                    entry,
                                    UpstreamCapability::Resources,
                                );
                            }
                        },
                    )
                    .await;
                if applied.is_some() {
                    Err(ExactResourceReadError::Upstream)
                } else {
                    Err(ExactResourceReadError::Unavailable)
                }
            }
            RawCallOutcome::Timeout => {
                let applied = self
                    .apply_to_observed_resource_call(
                        &prepared.observed,
                        prepared.generation,
                        &prepared.native_uri,
                        |entry| {
                            super::health::record_failure_on_entry(
                                &upstream_name,
                                entry,
                                UpstreamCapability::Resources,
                                "upstream resource read timed out".to_string(),
                            );
                        },
                    )
                    .await;
                if applied.is_some() {
                    Err(ExactResourceReadError::Timeout)
                } else {
                    Err(ExactResourceReadError::Unavailable)
                }
            }
            RawCallOutcome::Cancelled => Err(ExactResourceReadError::Cancelled),
        }
    }

    /// Read a resource from an upstream, given a prefixed URI.
    ///
    /// Expects URIs in the form `lab://upstream/{name}/{original_uri}`.
    /// Returns `None` if the upstream name is not found or not resource-enabled.
    pub async fn read_upstream_resource(
        &self,
        uri: &str,
    ) -> Option<Result<ReadResourceResult, String>> {
        self.read_upstream_resource_request(ReadResourceRequestParams::new(uri))
            .await
    }

    /// Read a resource while preserving the complete 2026 request envelope.
    pub async fn read_upstream_resource_request(
        &self,
        mut params: ReadResourceRequestParams,
    ) -> Option<Result<ReadResourceResult, String>> {
        let start = Instant::now();
        let gateway_uri = params.uri.clone();
        let prefix = "lab://upstream/";
        let rest = gateway_uri.strip_prefix(prefix)?;

        // Extract upstream name and original URI.
        let slash_pos = rest.find('/')?;
        let upstream_name = &rest[..slash_pos];
        params.uri = rest[slash_pos + 1..].to_string();

        // Check if this upstream has resource proxying enabled.
        // Clone the vec and drop the lock before any async work.
        let is_configured_for_resources = {
            let resource_names = self.resource_upstreams.read().await;
            resource_names.iter().any(|name| name == upstream_name)
        };
        let is_resource_enabled = is_configured_for_resources
            && self
                .catalog
                .read()
                .await
                .get(upstream_name)
                .is_some_and(|entry| entry.resource_health.is_routable());
        if !is_resource_enabled {
            return None;
        }

        // Send the original URI upstream; normalize content URIs back to the
        // gateway-prefixed form the caller passed in.
        self.read_resource_request_from_peer(upstream_name, params, &gateway_uri, start)
            .await
    }

    /// Read a native upstream `ui://…` resource by reverse-looking-up the
    /// owning upstream from each entry's cached `resource_uris`.
    ///
    /// Unlike [`read_upstream_resource`], the URI is **not** gateway-prefixed:
    /// MCP Apps (mcp-ui) widget resources are referenced by their native
    /// `ui://<upstream>/…` URI — carried in a tool result's
    /// `_meta.ui.resourceUri` — so they must be routed by reverse-lookup and
    /// returned without any URI rewriting. ContentBlock URIs are normalized back to
    /// the same native URI so the host sees a self-consistent read.
    ///
    /// Returns `None` if no routable upstream lists the URI (caller falls
    /// through to a resource-not-found).
    ///
    /// [`read_upstream_resource`]: Self::read_upstream_resource
    pub async fn read_upstream_ui_resource(
        &self,
        uri: &str,
    ) -> Option<Result<ReadResourceResult, String>> {
        self.read_upstream_ui_resource_allowed(uri, None).await
    }

    pub async fn read_upstream_ui_resource_allowed(
        &self,
        uri: &str,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Option<Result<ReadResourceResult, String>> {
        let start = Instant::now();
        let redacted_uri = redact_resource_uri_for_logging(uri);

        // Reverse-lookup the owning upstream by scanning cached resource URIs
        // and tool metadata. Some MCP App servers advertise `ui://` widgets only
        // from a tool's `_meta.ui.resourceUri` and do not list the widget from
        // `resources/list`; the host still reads that URI verbatim after the
        // tool call, so tool metadata is ownership evidence too.
        let cached_owner = {
            let catalog = self.catalog.read().await;
            catalog
                .iter()
                .find(|(name, entry)| {
                    allowed.is_none_or(|allowed| allowed.contains(name.as_str()))
                        && entry.resource_health.is_routable()
                        && (entry.resource_uris.iter().any(|cached| cached == uri)
                            || entry.tools.values().any(|tool| {
                                entry.exposure_policy.matches(tool.tool.name.as_ref())
                                    && mcp_tool_owns_mcp_app_resource(&tool.tool, uri)
                            }))
                })
                .map(|(name, _)| name.clone())
        };

        let (upstream_name, resolution) = if let Some(name) = cached_owner {
            (name, "cached_resource_uri")
        } else if let Some(name) = ui_uri_authority(uri)
            && allowed.is_none_or(|allowed| allowed.contains(name))
            && self.catalog.read().await.contains_key(name)
        {
            (name.to_string(), "uri_authority")
        } else {
            tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "resource.read",
                event = "owner_lookup.empty",
                operation = "resource.read",
                capability = "resources",
                resource_uri = %redacted_uri,
                kind = "ui_resource_owner_not_found",
                "no upstream owns native ui resource"
            );
            return None;
        };

        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "resource.read",
            event = "owner_lookup.finish",
            operation = "resource.read",
            capability = "resources",
            upstream = %upstream_name,
            resource_uri = %redacted_uri,
            resolution,
            "resolved native ui resource owner"
        );

        // Native `ui://` URI is both the request and the normalization target.
        self.read_resource_from_peer(&upstream_name, uri, uri, start)
            .await
    }

    /// Acquire the upstream peer and forward `read_resource(request_uri)` with
    /// the shared timeout / size-cap / structured-log skeleton, normalizing
    /// returned content URIs to `normalize_uri`. Returns `None` when the peer
    /// cannot be acquired. `start` is threaded in so the caller's lookup time is
    /// included in the measured elapsed.
    async fn read_resource_from_peer(
        &self,
        upstream_name: &str,
        request_uri: &str,
        normalize_uri: &str,
        start: Instant,
    ) -> Option<Result<ReadResourceResult, String>> {
        self.read_resource_request_from_peer(
            upstream_name,
            ReadResourceRequestParams::new(request_uri),
            normalize_uri,
            start,
        )
        .await
    }

    async fn read_resource_request_from_peer(
        &self,
        upstream_name: &str,
        params: ReadResourceRequestParams,
        normalize_uri: &str,
        start: Instant,
    ) -> Option<Result<ReadResourceResult, String>> {
        // Single choke point for `expose_resources` on the catalog-backed read
        // path: both callers have already reduced `params.uri` to the bare,
        // upstream-native URI (the gateway prefix stripped, or a native `ui://`
        // URI passed through), which is the form the allowlist is written in.
        // Fail closed when the upstream has no catalog entry — an unknown
        // policy is not permission to read.
        if !self
            .resource_uri_is_exposed(upstream_name, &params.uri)
            .await
        {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "resource.read",
                capability = "resources",
                upstream = %upstream_name,
                resource_uri = %redact_resource_uri_for_logging(&params.uri),
                kind = "resource_not_exposed",
                "upstream resource read blocked by exposure policy"
            );
            return None;
        }

        let peer = self
            .acquire_peer(
                upstream_name,
                UpstreamCapability::Resources,
                "resource.read",
            )
            .await?;

        let redacted_uri = redact_resource_uri_for_logging(normalize_uri);
        let event = UpstreamRequestLog::resource(upstream_name, redacted_uri, false);
        log_upstream_request_start(event);
        let timeout_ms = self.request_timeout.as_millis();

        Some(
            timed_capability_call_str(
                self,
                upstream_name,
                UpstreamCapability::Resources,
                event,
                start,
                peer.read_resource(params),
                estimate_resource_response_size,
                None,
                |e| format!("upstream resource read failed: {e}"),
                format!("upstream resource read timed out after {timeout_ms}ms"),
            )
            .await
            .map(|result| normalize_resource_result_uri(result, normalize_uri)),
        )
    }

    pub async fn read_upstream_resource_allowed(
        &self,
        uri: &str,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Option<Result<ReadResourceResult, String>> {
        self.read_upstream_resource_request_allowed(ReadResourceRequestParams::new(uri), allowed)
            .await
    }

    pub async fn read_upstream_resource_request_allowed(
        &self,
        params: ReadResourceRequestParams,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Option<Result<ReadResourceResult, String>> {
        if let Some(allowed) = allowed {
            let upstream = params
                .uri
                .strip_prefix("lab://upstream/")
                .and_then(|rest| rest.split('/').next())?;
            if !allowed.contains(upstream) {
                return None;
            }
        }
        self.read_upstream_resource_request(params).await
    }

    pub async fn subject_scoped_read_resource(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        uri: &str,
    ) -> Result<ReadResourceResult, String> {
        self.subject_scoped_read_resource_request(
            config,
            subject,
            ReadResourceRequestParams::new(uri),
        )
        .await
    }

    /// Whether `resource_uri` (bare, upstream-native) is exposed by the cached
    /// `expose_resources` policy for `upstream_name`.
    ///
    /// Returns `false` when the upstream has no catalog entry: without a known
    /// policy there is nothing authorizing the read.
    async fn resource_uri_is_exposed(&self, upstream_name: &str, resource_uri: &str) -> bool {
        let catalog = self.catalog.read().await;
        catalog
            .get(upstream_name)
            .is_some_and(|entry| resource_exposed(&entry.resource_exposure_policy, resource_uri))
    }

    pub async fn subject_scoped_read_resource_request(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        mut params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, String> {
        let start = Instant::now();
        let gateway_uri = params.uri.clone();
        let prefix = format!("lab://upstream/{}/", config.name);
        params.uri = if let Some(uri) = gateway_uri.strip_prefix(&prefix) {
            uri.to_string()
        } else if gateway_uri.starts_with("ui://") {
            gateway_uri.clone()
        } else {
            return Err("resource uri does not match upstream".to_string());
        };
        // OAuth reads never touch the catalog, so resolve the same fail-closed
        // policy from the live config. Without this the list filter above would
        // be cosmetic: the URI stays readable by anyone who knows it.
        if !resource_exposed(
            &resolve_request_resource_exposure_policy(
                &config.name,
                config.expose_resources.clone(),
            ),
            &params.uri,
        ) {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "resource.read",
                capability = "resources",
                upstream = %config.name,
                subject_scoped = true,
                resource_uri = %redact_resource_uri_for_logging(&gateway_uri),
                kind = "resource_not_exposed",
                "upstream resource read blocked by exposure policy"
            );
            return Err(format!(
                "resource is not exposed by upstream `{}`",
                config.name
            ));
        }
        let redacted_uri = redact_resource_uri_for_logging(&gateway_uri);
        let event = UpstreamRequestLog::resource(&config.name, redacted_uri, true)
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        // P-C1: reuse cached per-(upstream,subject) connection instead of opening fresh.
        let (peer, _tools) = match self.acquire_or_connect_subject(config, subject).await {
            Ok(pair) => pair,
            Err(error) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Resources,
                    format!("upstream resource connect failed: {error}"),
                )
                .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "upstream_connect_error",
                    Some(&error),
                    None,
                    None,
                );
                return Err(error.to_string());
            }
        };
        let timeout_ms = self.request_timeout.as_millis();

        timed_capability_call_str(
            self,
            &config.name,
            UpstreamCapability::Resources,
            event,
            start,
            peer.read_resource(params),
            estimate_resource_response_size,
            Some(subject),
            |e| format!("upstream resource read failed: {e}"),
            format!("upstream resource read timed out after {timeout_ms}ms"),
        )
        .await
        .map(|result| normalize_resource_result_uri(result, &gateway_uri))
    }
}

fn ui_uri_authority(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix("ui://")?;
    let authority = rest.split('/').next()?;
    (!authority.is_empty()).then_some(authority)
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rmcp::model::{
        ErrorData, ReadResourceRequestParams as ExactReadParams,
        ReadResourceResponse as ExactReadResponse, ReadResourceResult, Resource, ResourceContents,
    };
    use rmcp::service::RequestContext as ExactRequestContext;
    use rmcp::{RoleServer, ServerHandler};
    use tokio::sync::{Mutex, Notify};

    use super::super::testsupport::*;
    use super::ExactResourceReadError;
    use crate::upstream::types::{CIRCUIT_BREAKER_THRESHOLD, ToolExposurePolicy, UpstreamHealth};

    #[derive(Clone)]
    struct InspectingResourceServer {
        calls: Arc<AtomicUsize>,
        received: Arc<Mutex<Vec<(ExactReadParams, rmcp::model::RequestMetaObject)>>>,
        fail: bool,
    }

    impl ServerHandler for InspectingResourceServer {
        async fn read_resource(
            &self,
            request: ExactReadParams,
            context: ExactRequestContext<RoleServer>,
        ) -> Result<ExactReadResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.received
                .lock()
                .await
                .push((request.clone(), context.meta.clone()));
            if self.fail {
                return Err(ErrorData::invalid_params("private resource detail", None));
            }
            Ok(ReadResourceResult::new(vec![
                ResourceContents::text(format!("body:{}", request.uri), request.uri.clone()),
                ResourceContents::blob("YWJj", "https://malicious.invalid/leak"),
            ])
            .into())
        }
    }

    #[derive(Clone)]
    struct DelayedResourceReadServer {
        started: Arc<Notify>,
        release: Arc<Notify>,
        fail: bool,
    }

    #[derive(Clone)]
    struct SlowResourceReadServer {
        calls: Arc<AtomicUsize>,
        delay: std::time::Duration,
        started: Option<Arc<Notify>>,
    }

    #[derive(Clone)]
    struct SizedResourceReadServer {
        payload_bytes: usize,
    }

    impl ServerHandler for SizedResourceReadServer {
        async fn read_resource(
            &self,
            request: ExactReadParams,
            _: ExactRequestContext<RoleServer>,
        ) -> Result<ExactReadResponse, ErrorData> {
            Ok(ReadResourceResult::new(vec![
                ResourceContents::text("x".repeat(self.payload_bytes), request.uri)
                    .with_mime_type("text/plain"),
            ])
            .into())
        }
    }

    impl ServerHandler for SlowResourceReadServer {
        async fn read_resource(
            &self,
            request: ExactReadParams,
            _: ExactRequestContext<RoleServer>,
        ) -> Result<ExactReadResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(started) = &self.started {
                started.notify_one();
            }
            tokio::time::sleep(self.delay).await;
            Ok(ReadResourceResult::new(vec![ResourceContents::text("slow", request.uri)]).into())
        }
    }

    impl ServerHandler for DelayedResourceReadServer {
        async fn read_resource(
            &self,
            request: ExactReadParams,
            _: ExactRequestContext<RoleServer>,
        ) -> Result<ExactReadResponse, ErrorData> {
            self.started.notify_one();
            self.release.notified().await;
            if self.fail {
                return Err(ErrorData::internal_error("private delayed failure", None));
            }
            Ok(
                ReadResourceResult::new(vec![ResourceContents::text("delayed", request.uri)])
                    .into(),
            )
        }
    }

    #[tokio::test]
    async fn exact_resource_kernel_forwards_native_uri_and_normalizes_result() {
        let calls = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Mutex::new(Vec::new()));
        let pool = catalog_pool_with_server(
            "alpha",
            InspectingResourceServer {
                calls: Arc::clone(&calls),
                received: Arc::clone(&received),
                fail: false,
            },
        )
        .await;
        let native_uri = "lab://upstream/inner/file:///nested/path/value";
        pool.insert_resource_routes_for_tests(
            "alpha",
            vec![Resource::new(native_uri, "nested resource")],
        )
        .await;
        let generation = pool
            .published_resource_catalog()
            .await
            .unwrap()
            .generation();

        let mut meta = rmcp::model::RequestMetaObject::new();
        meta.insert("trace".to_string(), serde_json::json!("opaque"));
        let result = pool
            .read_published_resource_exact(
                "alpha",
                native_uri,
                generation,
                ExactReadParams::new(native_uri).with_meta(meta.clone()),
            )
            .await
            .expect("exact published resource is readable");

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let received = received.lock().await;
        assert_eq!(received[0].0.uri, native_uri);
        assert_eq!(received[0].1.get("trace"), meta.get("trace"));
        let encoded = serde_json::to_value(result).unwrap();
        assert_eq!(
            encoded["contents"][0]["uri"],
            "lab://upstream/alpha/lab://upstream/inner/file:///nested/path/value"
        );
        assert_eq!(
            encoded["contents"][1]["uri"],
            "lab://upstream/alpha/lab://upstream/inner/file:///nested/path/value"
        );
        assert_eq!(
            encoded["contents"][0]["text"],
            "body:lab://upstream/inner/file:///nested/path/value"
        );
    }

    #[tokio::test]
    async fn exact_resource_kernel_rejects_wrong_envelope_and_stale_generation_without_rpc() {
        let calls = Arc::new(AtomicUsize::new(0));
        let pool = catalog_pool_with_server(
            "alpha",
            InspectingResourceServer {
                calls: Arc::clone(&calls),
                received: Arc::new(Mutex::new(Vec::new())),
                fail: false,
            },
        )
        .await;
        pool.insert_resource_routes_for_tests("alpha", vec![Resource::new("file:///one", "one")])
            .await;
        let generation = pool
            .published_resource_catalog()
            .await
            .unwrap()
            .generation();

        assert_eq!(
            pool.read_published_resource_exact(
                "alpha",
                "file:///one",
                generation,
                ExactReadParams::new("file:///other"),
            )
            .await,
            Err(ExactResourceReadError::Unavailable)
        );
        pool.insert_resource_routes_for_tests("alpha", vec![Resource::new("file:///two", "two")])
            .await;
        assert_eq!(
            pool.read_published_resource_exact(
                "alpha",
                "file:///one",
                generation,
                ExactReadParams::new("file:///one"),
            )
            .await,
            Err(ExactResourceReadError::Unavailable)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn exact_resource_kernel_rejects_ui_hidden_and_unhealthy_routes_without_rpc() {
        let calls = Arc::new(AtomicUsize::new(0));
        let pool = catalog_pool_with_server(
            "alpha",
            InspectingResourceServer {
                calls: Arc::clone(&calls),
                received: Arc::new(Mutex::new(Vec::new())),
                fail: false,
            },
        )
        .await;
        pool.insert_resource_routes_for_tests("alpha", vec![Resource::new("UI://widget", "ui")])
            .await;
        let ui_generation = pool
            .published_resource_catalog()
            .await
            .unwrap()
            .generation();
        assert_eq!(
            pool.read_published_resource_exact(
                "alpha",
                "UI://widget",
                ui_generation,
                ExactReadParams::new("UI://widget"),
            )
            .await,
            Err(ExactResourceReadError::Unavailable)
        );

        pool.insert_resource_routes_for_tests("alpha", vec![Resource::new("file:///one", "one")])
            .await;
        {
            let mut catalog = pool.catalog_write().await;
            catalog.get_mut("alpha").unwrap().resource_exposure_policy =
                ToolExposurePolicy::AllowList(Vec::new());
        }
        let hidden_generation = pool
            .published_resource_catalog()
            .await
            .unwrap()
            .generation();
        assert_eq!(
            pool.read_published_resource_exact(
                "alpha",
                "file:///one",
                hidden_generation,
                ExactReadParams::new("file:///one"),
            )
            .await,
            Err(ExactResourceReadError::Unavailable)
        );
        {
            let mut catalog = pool.catalog_write().await;
            let entry = catalog.get_mut("alpha").unwrap();
            entry.resource_exposure_policy = ToolExposurePolicy::All;
            entry.resource_health = UpstreamHealth::Unhealthy {
                consecutive_failures: CIRCUIT_BREAKER_THRESHOLD,
            };
        }
        let unhealthy_generation = pool
            .published_resource_catalog()
            .await
            .unwrap()
            .generation();
        assert_eq!(
            pool.read_published_resource_exact(
                "alpha",
                "file:///one",
                unhealthy_generation,
                ExactReadParams::new("file:///one"),
            )
            .await,
            Err(ExactResourceReadError::Unavailable)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn exact_resource_kernel_redacts_application_error_and_preserves_routability() {
        let calls = Arc::new(AtomicUsize::new(0));
        let pool = catalog_pool_with_server(
            "alpha",
            InspectingResourceServer {
                calls: Arc::clone(&calls),
                received: Arc::new(Mutex::new(Vec::new())),
                fail: true,
            },
        )
        .await;
        pool.insert_resource_routes_for_tests("alpha", vec![Resource::new("file:///one", "one")])
            .await;
        let generation = pool
            .published_resource_catalog()
            .await
            .unwrap()
            .generation();

        let error = pool
            .read_published_resource_exact(
                "alpha",
                "file:///one",
                generation,
                ExactReadParams::new("file:///one"),
            )
            .await
            .expect_err("application error is returned");
        assert_eq!(error, ExactResourceReadError::Upstream);
        assert!(!error.to_string().contains("private resource detail"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(pool.published_resource_catalog().await.is_ok());
    }

    #[tokio::test]
    async fn exact_resource_kernel_uses_publication_not_legacy_resource_upstream_list() {
        let calls = Arc::new(AtomicUsize::new(0));
        let pool = catalog_pool_with_server(
            "alpha",
            InspectingResourceServer {
                calls: Arc::clone(&calls),
                received: Arc::new(Mutex::new(Vec::new())),
                fail: false,
            },
        )
        .await;
        pool.insert_resource_routes_for_tests("alpha", vec![Resource::new("file:///one", "one")])
            .await;
        let generation = pool
            .published_resource_catalog()
            .await
            .unwrap()
            .generation();
        pool.resource_upstreams.write().await.clear();

        pool.read_published_resource_exact(
            "alpha",
            "file:///one",
            generation,
            ExactReadParams::new("file:///one"),
        )
        .await
        .expect("exact publication remains the authoritative route");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn exact_resource_kernel_discards_resource_generation_aba_outcomes() {
        for fail in [false, true] {
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let pool = catalog_pool_with_server(
                "alpha",
                DelayedResourceReadServer {
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                    fail,
                },
            )
            .await;
            pool.insert_resource_routes_for_tests(
                "alpha",
                vec![Resource::new("file:///one", "one")],
            )
            .await;
            {
                let mut catalog = pool.catalog_write().await;
                catalog.get_mut("alpha").unwrap().resource_last_error = Some("sentinel".into());
            }
            let generation = pool
                .published_resource_catalog()
                .await
                .unwrap()
                .generation();
            let calling = Arc::clone(&pool);
            let task = tokio::spawn(async move {
                calling
                    .read_published_resource_exact(
                        "alpha",
                        "file:///one",
                        generation,
                        ExactReadParams::new("file:///one"),
                    )
                    .await
            });
            started.notified().await;
            pool.insert_resource_routes_for_tests(
                "alpha",
                vec![Resource::new("file:///two", "two")],
            )
            .await;
            pool.insert_resource_routes_for_tests(
                "alpha",
                vec![Resource::new("file:///one", "one")],
            )
            .await;
            release.notify_one();
            assert_eq!(
                task.await.unwrap(),
                Err(ExactResourceReadError::Unavailable)
            );
            assert_eq!(
                pool.catalog
                    .read()
                    .await
                    .get("alpha")
                    .unwrap()
                    .resource_last_error
                    .as_deref(),
                Some("sentinel")
            );
        }
    }

    #[tokio::test]
    async fn exact_resource_kernel_discards_post_rpc_policy_health_and_removal_changes() {
        for change in ["policy", "health", "removal"] {
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let pool = catalog_pool_with_server(
                "alpha",
                DelayedResourceReadServer {
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                    fail: false,
                },
            )
            .await;
            pool.insert_resource_routes_for_tests(
                "alpha",
                vec![Resource::new("file:///one", "one")],
            )
            .await;
            let generation = pool
                .published_resource_catalog()
                .await
                .unwrap()
                .generation();
            let calling = Arc::clone(&pool);
            let task = tokio::spawn(async move {
                calling
                    .read_published_resource_exact(
                        "alpha",
                        "file:///one",
                        generation,
                        ExactReadParams::new("file:///one"),
                    )
                    .await
            });
            started.notified().await;
            {
                let mut catalog = pool.catalog_write().await;
                match change {
                    "policy" => {
                        catalog.get_mut("alpha").unwrap().resource_exposure_policy =
                            ToolExposurePolicy::AllowList(Vec::new());
                    }
                    "health" => {
                        catalog.get_mut("alpha").unwrap().resource_health =
                            UpstreamHealth::Unhealthy {
                                consecutive_failures: CIRCUIT_BREAKER_THRESHOLD,
                            };
                    }
                    "removal" => {
                        catalog.remove("alpha");
                    }
                    _ => unreachable!(),
                }
            }
            release.notify_one();
            assert_eq!(
                task.await.unwrap(),
                Err(ExactResourceReadError::Unavailable)
            );
        }
    }

    #[tokio::test]
    async fn exact_resource_kernel_cancellation_does_not_apply_outcome() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let pool = catalog_pool_with_server(
            "alpha",
            DelayedResourceReadServer {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                fail: false,
            },
        )
        .await;
        pool.insert_resource_routes_for_tests("alpha", vec![Resource::new("file:///one", "one")])
            .await;
        let generation = pool
            .published_resource_catalog()
            .await
            .unwrap()
            .generation();
        {
            let mut catalog = pool.catalog_write().await;
            catalog.get_mut("alpha").unwrap().resource_last_error = Some("sentinel".into());
        }
        let calling = Arc::clone(&pool);
        let task = tokio::spawn(async move {
            calling
                .read_published_resource_exact(
                    "alpha",
                    "file:///one",
                    generation,
                    ExactReadParams::new("file:///one"),
                )
                .await
        });
        started.notified().await;
        task.abort();
        release.notify_one();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            pool.catalog
                .read()
                .await
                .get("alpha")
                .unwrap()
                .resource_last_error
                .as_deref(),
            Some("sentinel")
        );
        let permit = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            pool.acquire_upstream_call_permit("alpha"),
        )
        .await
        .expect("aborting the prior read releases its permit")
        .expect("permit remains available");
        drop(permit);
    }

    #[tokio::test]
    async fn exact_resource_kernel_discards_connection_aba_success_and_failure() {
        for fail in [false, true] {
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let pool = catalog_pool_with_server(
                "alpha",
                DelayedResourceReadServer {
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                    fail,
                },
            )
            .await;
            pool.insert_resource_routes_for_tests(
                "alpha",
                vec![Resource::new("file:///one", "one")],
            )
            .await;
            let generation = pool
                .published_resource_catalog()
                .await
                .unwrap()
                .generation();
            let calling = Arc::clone(&pool);
            let task = tokio::spawn(async move {
                calling
                    .read_published_resource_exact(
                        "alpha",
                        "file:///one",
                        generation,
                        ExactReadParams::new("file:///one"),
                    )
                    .await
            });
            started.notified().await;
            let replacement =
                catalog_pool_with_server("alpha", StaticCatalogServer::default()).await;
            let (connection_b, entry_b) =
                replacement.remove_connection_catalog_entry("alpha").await;
            let previous_a = pool
                .install_connection_catalog_entry(
                    "alpha".into(),
                    connection_b.unwrap(),
                    entry_b.unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
            let (removed_b, _) = pool.remove_connection_catalog_entry("alpha").await;
            let mut entry_a =
                super::super::entries::healthy_in_process_entry(Arc::from("alpha"), HashMap::new());
            entry_a.resource_last_error = Some("replacement sentinel".into());
            pool.install_connection_catalog_entry("alpha".into(), previous_a, entry_a)
                .await
                .unwrap();
            pool.insert_resource_routes_for_tests(
                "alpha",
                vec![Resource::new("file:///one", "one")],
            )
            .await;
            release.notify_one();
            assert_eq!(
                task.await.unwrap(),
                Err(ExactResourceReadError::Unavailable)
            );
            assert_eq!(
                pool.catalog
                    .read()
                    .await
                    .get("alpha")
                    .unwrap()
                    .resource_last_error
                    .as_deref(),
                Some("replacement sentinel")
            );
            if let Some(connection) = removed_b {
                connection.shutdown("alpha", "test.resource-read.aba").await;
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn exact_resource_kernel_uses_one_queue_and_rpc_deadline() {
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let mut pool = catalog_pool_with_server(
            "alpha",
            SlowResourceReadServer {
                calls: Arc::clone(&calls),
                delay: std::time::Duration::from_millis(80),
                started: Some(Arc::clone(&started)),
            },
        )
        .await;
        let pool_mut = Arc::get_mut(&mut pool).expect("test owns pool");
        pool_mut.request_timeout = std::time::Duration::from_millis(100);
        pool_mut.call_concurrency = 1;
        pool_mut.call_semaphores = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        pool.insert_resource_routes_for_tests("alpha", vec![Resource::new("file:///one", "one")])
            .await;
        let generation = pool
            .published_resource_catalog()
            .await
            .unwrap()
            .generation();
        let held = pool.acquire_upstream_call_permit("alpha").await.unwrap();
        let calling = Arc::clone(&pool);
        let task = tokio::spawn(async move {
            calling
                .read_published_resource_exact(
                    "alpha",
                    "file:///one",
                    generation,
                    ExactReadParams::new("file:///one"),
                )
                .await
        });
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_millis(70)).await;
        drop(held);
        started.notified().await;
        tokio::time::advance(std::time::Duration::from_millis(30)).await;
        assert_eq!(task.await.unwrap(), Err(ExactResourceReadError::Timeout));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn exact_resource_kernel_queue_saturation_does_not_call_or_mutate_health() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut pool = catalog_pool_with_server(
            "alpha",
            SlowResourceReadServer {
                calls: Arc::clone(&calls),
                delay: std::time::Duration::from_millis(1),
                started: None,
            },
        )
        .await;
        let pool_mut = Arc::get_mut(&mut pool).unwrap();
        pool_mut.request_timeout = std::time::Duration::from_millis(25);
        pool_mut.call_concurrency = 1;
        pool_mut.call_semaphores = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        pool.insert_resource_routes_for_tests("alpha", vec![Resource::new("file:///one", "one")])
            .await;
        {
            let mut catalog = pool.catalog_write().await;
            catalog.get_mut("alpha").unwrap().resource_last_error = Some("sentinel".into());
        }
        let generation = pool
            .published_resource_catalog()
            .await
            .unwrap()
            .generation();
        let _held = pool.acquire_upstream_call_permit("alpha").await.unwrap();
        assert_eq!(
            pool.read_published_resource_exact(
                "alpha",
                "file:///one",
                generation,
                ExactReadParams::new("file:///one"),
            )
            .await,
            Err(ExactResourceReadError::QueueUnavailable)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            pool.catalog
                .read()
                .await
                .get("alpha")
                .unwrap()
                .resource_last_error
                .as_deref(),
            Some("sentinel")
        );
    }

    #[tokio::test]
    async fn exact_resource_kernel_observes_publication_after_queue_wait() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut pool = catalog_pool_with_server(
            "alpha",
            InspectingResourceServer {
                calls: Arc::clone(&calls),
                received: Arc::new(Mutex::new(Vec::new())),
                fail: false,
            },
        )
        .await;
        let pool_mut = Arc::get_mut(&mut pool).unwrap();
        pool_mut.request_timeout = std::time::Duration::from_millis(100);
        pool_mut.call_concurrency = 1;
        pool_mut.call_semaphores = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        pool.insert_resource_routes_for_tests("alpha", vec![Resource::new("file:///one", "one")])
            .await;
        let generation = pool
            .published_resource_catalog()
            .await
            .unwrap()
            .generation();
        let held = pool.acquire_upstream_call_permit("alpha").await.unwrap();
        let calling = Arc::clone(&pool);
        let task = tokio::spawn(async move {
            calling
                .read_published_resource_exact(
                    "alpha",
                    "file:///one",
                    generation,
                    ExactReadParams::new("file:///one"),
                )
                .await
        });
        tokio::task::yield_now().await;
        pool.insert_resource_routes_for_tests(
            "alpha",
            vec![Resource::new("file:///replacement", "replacement")],
        )
        .await;
        drop(held);
        assert_eq!(
            task.await.unwrap(),
            Err(ExactResourceReadError::Unavailable)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn exact_resource_kernel_enforces_normalized_response_byte_boundary() {
        let native_uri = "file:///one";
        let calibration =
            catalog_pool_with_server("alpha", SizedResourceReadServer { payload_bytes: 0 }).await;
        calibration
            .insert_resource_routes_for_tests("alpha", vec![Resource::new(native_uri, "one")])
            .await;
        let calibration_generation = calibration
            .published_resource_catalog()
            .await
            .unwrap()
            .generation();
        let empty = calibration
            .read_published_resource_exact(
                "alpha",
                native_uri,
                calibration_generation,
                ExactReadParams::new(native_uri),
            )
            .await
            .unwrap();
        let overhead = super::estimate_resource_response_size(&empty);
        let exact_payload = super::max_response_bytes()
            .checked_sub(overhead)
            .expect("configured response cap exceeds an empty normalized Resource response");

        for (extra, expected) in [(0, None), (1, Some(ExactResourceReadError::TooLarge))] {
            let pool = catalog_pool_with_server(
                "alpha",
                SizedResourceReadServer {
                    payload_bytes: exact_payload + extra,
                },
            )
            .await;
            pool.insert_resource_routes_for_tests("alpha", vec![Resource::new(native_uri, "one")])
                .await;
            {
                let mut catalog = pool.catalog_write().await;
                catalog.get_mut("alpha").unwrap().resource_last_error = Some("sentinel".into());
            }
            let generation = pool
                .published_resource_catalog()
                .await
                .unwrap()
                .generation();
            let result = pool
                .read_published_resource_exact(
                    "alpha",
                    native_uri,
                    generation,
                    ExactReadParams::new(native_uri),
                )
                .await;
            match expected {
                Some(error) => assert!(
                    matches!(&result, Err(actual) if *actual == error),
                    "expected {error:?}, got serialized size {:?}",
                    result
                        .as_ref()
                        .ok()
                        .map(super::estimate_resource_response_size)
                ),
                None => assert!(result.is_ok()),
            }
            assert!(
                pool.catalog
                    .read()
                    .await
                    .get("alpha")
                    .unwrap()
                    .resource_last_error
                    .is_none(),
                "a valid oversized upstream result records Resources success"
            );
        }
    }

    // The `LabMcpServer::snapshot_catalog` projection of these cached resource
    // URIs is asserted by the `lab` crate's `gateway_schema_resources` integration
    // test; here we cover only the pool-level listing + cache, which is all the
    // upstream pool owns.
    #[tokio::test]
    async fn successful_resource_listing_populates_snapshot_cache() {
        let pool = static_catalog_pool("static").await;

        let resources = pool.list_upstream_resources().await;
        let listed_uris: Vec<_> = resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect();
        assert_eq!(
            listed_uris,
            vec![
                "lab://upstream/static/file:///tmp/upstream-one",
                "lab://upstream/static/lab://upstream/old-name/file:///tmp/upstream-two",
            ]
        );

        let cached = pool.cached_upstream_resource_uris().await;
        assert_eq!(
            cached,
            vec![(
                "static".to_string(),
                vec![
                    "file:///tmp/upstream-one".to_string(),
                    "lab://upstream/old-name/file:///tmp/upstream-two".to_string(),
                ],
            )]
        );
    }

    #[tokio::test]
    async fn read_resource_times_out_slow_upstream_response() {
        let pool = slow_response_pool("slow").await;

        let result = pool
            .read_upstream_resource("lab://upstream/slow/file:///tmp/slow")
            .await
            .expect("resource upstream is enabled")
            .expect_err("slow resource read should time out");

        assert!(result.contains("timed out"));
    }

    /// T9: an upstream that returns an oversized resource body gets a structured
    /// cap error — not a panic or OOM.
    #[tokio::test]
    async fn read_resource_oversized_response_returns_cap_error() {
        use std::collections::HashMap;

        use rmcp::model::{
            ErrorData, ListResourcesResult, PaginatedRequestParams, ReadResourceResult, Resource,
            ResourceContents, ServerCapabilities, ServerInfo,
        };
        use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};

        use super::super::super::types::UpstreamRuntimeMetadata;
        use super::super::entries::healthy_in_process_entry;
        use super::super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
        use super::super::{UpstreamConnection, UpstreamPool};

        struct OversizedResourceServer;
        impl ServerHandler for OversizedResourceServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
            }
            async fn list_resources(
                &self,
                _: Option<PaginatedRequestParams>,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<ListResourcesResult, ErrorData> {
                Ok(ListResourcesResult::with_all_items(vec![Resource::new(
                    "file:///tmp/big",
                    "big-resource",
                )]))
            }
            async fn read_resource(
                &self,
                _: rmcp::model::ReadResourceRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
                // 12 MiB of text — above the default 10 MiB cap.
                let payload = "x".repeat(12 * 1024 * 1024);
                Ok(ReadResourceResult::new(vec![ResourceContents::text(
                    "file:///tmp/big",
                    payload,
                )])
                .into())
            }
        }

        let upstream_name = "oversized-resource";
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _server_task = tokio::spawn(async move {
            let running = OversizedResourceServer
                .serve(server_transport)
                .await
                .expect("oversized resource server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("oversized resource client starts");
        let peer = client_service.peer().clone();

        let pool = Arc::new(UpstreamPool::new());
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new());
        entry.resource_count = 1;
        entry.resource_uris = vec!["file:///tmp/big".to_string()];
        pool.catalog
            .write()
            .await
            .insert(upstream_name.to_string(), entry);
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(_server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );
        pool.resource_upstreams
            .write()
            .await
            .push(upstream_name.to_string());

        let uri = format!("lab://upstream/{upstream_name}/file:///tmp/big");
        let result = pool
            .read_upstream_resource(&uri)
            .await
            .expect("resource upstream is enabled")
            .expect_err("oversized resource should be rejected");

        assert!(
            result.contains("too large"),
            "expected 'too large' in error, got: {result}"
        );
        assert!(
            result.contains("bytes"),
            "expected byte count in error, got: {result}"
        );
    }

    /// `read_upstream_ui_resource` reverse-looks-up the owning upstream by its
    /// cached native `ui://` URI, forwards the read, and preserves the native
    /// URI (no `lab://upstream/` rewrite). An unknown `ui://` returns `None`.
    #[tokio::test]
    async fn read_upstream_ui_resource_routes_native_uri_to_owner() {
        use std::collections::HashMap;

        use rmcp::model::{
            ErrorData, ReadResourceResult, ResourceContents, ServerCapabilities, ServerInfo,
        };
        use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};

        use super::super::super::types::UpstreamRuntimeMetadata;
        use super::super::entries::healthy_in_process_entry;
        use super::super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
        use super::super::{UpstreamConnection, UpstreamPool};

        const WIDGET_URI: &str = "ui://mock/widget";
        const WIDGET_HTML: &str = "<html><body>dashboard</body></html>";

        struct UiResourceServer;
        impl ServerHandler for UiResourceServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
            }
            async fn read_resource(
                &self,
                params: rmcp::model::ReadResourceRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
                // Echo back the requested (native ui://) URI with mcp-app HTML.
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(WIDGET_HTML, params.uri)
                        .with_mime_type("text/html;profile=mcp-app"),
                ])
                .into())
            }
        }

        let upstream_name = "mock";
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _server_task = tokio::spawn(async move {
            let running = UiResourceServer
                .serve(server_transport)
                .await
                .expect("ui resource server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("ui resource client starts");
        let peer = client_service.peer().clone();

        let pool = Arc::new(UpstreamPool::new());
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new());
        entry.resource_count = 1;
        entry.resource_uris = vec![WIDGET_URI.to_string()];
        pool.catalog
            .write()
            .await
            .insert(upstream_name.to_string(), entry);
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(_server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );

        // Owned native ui:// URI routes to the owner and returns the HTML, with
        // the content URI left as the native ui:// (no gateway rewrite).
        let result = pool
            .read_upstream_ui_resource(WIDGET_URI)
            .await
            .expect("an upstream owns the ui:// resource")
            .expect("ui resource read succeeds");
        let contents = result.contents.first().expect("one content block");
        match contents {
            ResourceContents::TextResourceContents { text, uri, .. } => {
                assert_eq!(text, WIDGET_HTML);
                assert_eq!(uri, WIDGET_URI, "native ui:// URI must be preserved");
            }
            other => panic!("expected text contents, got {other:?}"),
        }

        // An unknown ui:// URI is owned by no upstream → None (caller 404s).
        assert!(
            pool.read_upstream_ui_resource("ui://missing/widget")
                .await
                .is_none(),
            "unknown ui:// must reverse-lookup to no owner"
        );
    }

    #[tokio::test]
    async fn read_upstream_ui_resource_falls_back_to_uri_authority_owner() {
        use std::collections::HashMap;

        use rmcp::model::{
            ErrorData, ReadResourceResult, ResourceContents, ServerCapabilities, ServerInfo,
        };
        use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};

        use super::super::super::types::UpstreamRuntimeMetadata;
        use super::super::entries::healthy_in_process_entry;
        use super::super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
        use super::super::{UpstreamConnection, UpstreamPool};

        const WIDGET_URI: &str = "ui://mock/widget";
        const WIDGET_HTML: &str = "<html><body>dashboard</body></html>";

        struct UiResourceServer;
        impl ServerHandler for UiResourceServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
            }
            async fn read_resource(
                &self,
                params: rmcp::model::ReadResourceRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(WIDGET_HTML, params.uri)
                        .with_mime_type("text/html;profile=mcp-app"),
                ])
                .into())
            }
        }

        let upstream_name = "mock";
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _server_task = tokio::spawn(async move {
            let running = UiResourceServer
                .serve(server_transport)
                .await
                .expect("ui resource server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("ui resource client starts");
        let peer = client_service.peer().clone();

        let pool = Arc::new(UpstreamPool::new());
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new());
        entry.resource_count = 0;
        entry.resource_uris.clear();
        pool.catalog
            .write()
            .await
            .insert(upstream_name.to_string(), entry);
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(_server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );

        let result = pool
            .read_upstream_ui_resource(WIDGET_URI)
            .await
            .expect("uri authority should resolve to upstream owner")
            .expect("ui resource read succeeds");
        let contents = result.contents.first().expect("one content block");
        match contents {
            ResourceContents::TextResourceContents { text, uri, .. } => {
                assert_eq!(text, WIDGET_HTML);
                assert_eq!(uri, WIDGET_URI, "native ui:// URI must be preserved");
            }
            other => panic!("expected text contents, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_upstream_ui_resource_routes_tool_metadata_uri_to_owner() {
        use std::collections::HashMap;

        use rmcp::model::{
            ErrorData, MetaObject, ReadResourceResult, ResourceContents, ServerCapabilities,
            ServerInfo,
        };
        use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};

        use super::super::super::types::UpstreamRuntimeMetadata;
        use super::super::entries::healthy_in_process_entry;
        use super::super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
        use super::super::{UpstreamConnection, UpstreamPool};

        const UPSTREAM_NAME: &str = "ytdl-rmcp";
        const WIDGET_URI: &str = "ui://ytdl/search.html";
        const WIDGET_HTML: &str = "<html><body>youtube search</body></html>";

        struct UiResourceServer;
        impl ServerHandler for UiResourceServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
            }
            async fn read_resource(
                &self,
                params: rmcp::model::ReadResourceRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
                assert_eq!(params.uri, WIDGET_URI);
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(WIDGET_HTML, params.uri)
                        .with_mime_type("text/html;profile=mcp-app"),
                ])
                .into())
            }
        }

        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _server_task = tokio::spawn(async move {
            let running = UiResourceServer
                .serve(server_transport)
                .await
                .expect("ui resource server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("ui resource client starts");
        let peer = client_service.peer().clone();

        let pool = Arc::new(UpstreamPool::new());
        let upstream_name_arc: Arc<str> = Arc::from(UPSTREAM_NAME);
        let mut tool = test_upstream_tool(&upstream_name_arc, "youtube_search");
        tool.tool.meta = Some(MetaObject(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": WIDGET_URI }),
        )])));
        let mut entry = healthy_in_process_entry(
            Arc::clone(&upstream_name_arc),
            HashMap::from([("youtube_search".to_string(), tool)]),
        );
        entry.resource_count = 0;
        entry.resource_uris.clear();
        pool.catalog
            .write()
            .await
            .insert(UPSTREAM_NAME.to_string(), entry);
        pool.connections.write().await.insert(
            UPSTREAM_NAME.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(_server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );

        let result = pool
            .read_upstream_ui_resource(WIDGET_URI)
            .await
            .expect("tool metadata should resolve the ui resource owner")
            .expect("ui resource read succeeds");
        let contents = result.contents.first().expect("one content block");
        match contents {
            ResourceContents::TextResourceContents { text, uri, .. } => {
                assert_eq!(text, WIDGET_HTML);
                assert_eq!(uri, WIDGET_URI, "native ui:// URI must be preserved");
            }
            other => panic!("expected text contents, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_upstream_ui_resource_allowed_denies_hidden_owner_before_forwarding() {
        use std::collections::{BTreeSet, HashMap};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use rmcp::model::{
            ErrorData, ReadResourceResult, ResourceContents, ServerCapabilities, ServerInfo,
        };
        use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};

        use super::super::super::types::UpstreamRuntimeMetadata;
        use super::super::entries::healthy_in_process_entry;
        use super::super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
        use super::super::{UpstreamConnection, UpstreamPool};

        const HIDDEN_URI: &str = "ui://hidden-upstream/app.html";
        const ALLOWED_URI: &str = "ui://allowed-upstream/app.html";
        const WIDGET_HTML: &str = "<html><body>dashboard</body></html>";

        #[derive(Clone)]
        struct CountingUiResourceServer {
            reads: Arc<AtomicUsize>,
        }

        impl ServerHandler for CountingUiResourceServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
            }

            async fn read_resource(
                &self,
                params: rmcp::model::ReadResourceRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
                self.reads.fetch_add(1, Ordering::SeqCst);
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(WIDGET_HTML, params.uri)
                        .with_mime_type("text/html;profile=mcp-app"),
                ])
                .into())
            }
        }

        async fn install_ui_upstream(
            pool: &Arc<UpstreamPool>,
            upstream_name: &str,
            widget_uri: &str,
            reads: Arc<AtomicUsize>,
        ) {
            let (server_transport, client_transport) =
                tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
            let server = CountingUiResourceServer { reads };
            let server_task = tokio::spawn(async move {
                let running = server
                    .serve(server_transport)
                    .await
                    .expect("ui resource server starts");
                running.waiting().await.ok();
            });
            let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
                .serve(client_transport)
                .await
                .expect("ui resource client starts");
            let peer = client_service.peer().clone();
            let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
            let mut entry =
                healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new());
            entry.resource_count = 1;
            entry.resource_uris = vec![widget_uri.to_string()];
            pool.catalog
                .write()
                .await
                .insert(upstream_name.to_string(), entry);
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
        }

        let pool = Arc::new(UpstreamPool::new());
        let hidden_reads = Arc::new(AtomicUsize::new(0));
        let allowed_reads = Arc::new(AtomicUsize::new(0));
        install_ui_upstream(
            &pool,
            "hidden-upstream",
            HIDDEN_URI,
            Arc::clone(&hidden_reads),
        )
        .await;
        install_ui_upstream(
            &pool,
            "allowed-upstream",
            ALLOWED_URI,
            Arc::clone(&allowed_reads),
        )
        .await;
        pool.catalog
            .write()
            .await
            .get_mut("hidden-upstream")
            .expect("hidden upstream")
            .resource_uris
            .push(ALLOWED_URI.to_string());

        let allowed = BTreeSet::from(["allowed-upstream".to_string()]);

        assert!(
            pool.read_upstream_ui_resource_allowed(HIDDEN_URI, Some(&allowed))
                .await
                .is_none(),
            "hidden upstream UI resource should be denied"
        );
        assert_eq!(
            hidden_reads.load(Ordering::SeqCst),
            0,
            "hidden upstream must be denied before forwarding"
        );

        let result = pool
            .read_upstream_ui_resource_allowed(ALLOWED_URI, Some(&allowed))
            .await
            .expect("allowed upstream owns the ui:// resource")
            .expect("allowed upstream read succeeds");
        let contents = result.contents.first().expect("one content block");
        match contents {
            ResourceContents::TextResourceContents { text, uri, .. } => {
                assert_eq!(text, WIDGET_HTML);
                assert_eq!(uri, ALLOWED_URI);
            }
            other => panic!("expected text contents, got {other:?}"),
        }
        assert_eq!(
            allowed_reads.load(Ordering::SeqCst),
            1,
            "allowed upstream should still be forwarded"
        );
        assert_eq!(
            hidden_reads.load(Ordering::SeqCst),
            0,
            "hidden upstream must not win a cached-resource URI collision"
        );
    }
}
