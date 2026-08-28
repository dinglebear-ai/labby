//! Subject-scoped prompt discovery and prompt fetching.
//!
//! `subject_scoped_prompts`/`subject_scoped_prompt_owner` discover prompts for
//! OAuth upstreams under a subject; `get_prompt`/`subject_scoped_get_prompt`
//! fetch a single prompt with a request timeout and structured logging.
//!
//! Every one of them enforces `expose_prompts`. Filtering only `prompts/list`
//! would leave an excluded prompt directly fetchable by name — the prompt-side
//! twin of the `resources/read` bypass. The subject-scoped variants resolve the
//! policy from the live `UpstreamConfig` (their prompt lists never reach
//! `self.catalog`); `get_prompt` reads the cached
//! `UpstreamEntry::prompt_exposure_policy`.

use std::time::Instant;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use rmcp::model::{GetPromptRequestParams, GetPromptResult, Prompt};

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::PromptCatalogGeneration;
use super::UpstreamPool;
use super::capability_call::timed_capability_call_str;
use super::capability_call::{
    RawCallOutcome, classify_timeout_result, service_error_affects_connection_health,
};
use super::catalog_pagination;
use super::entries::{log_exposure_filter, prompt_exposed, resolve_request_prompt_exposure_policy};
use super::helpers::{
    bare_upstream_prompt_name, merge_upstream_prompts, prefixed_upstream_prompt_name,
    upstream_transport,
};
use super::logging::{UpstreamRequestLog, log_upstream_request_error, log_upstream_request_start};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum ExactPromptCallError {
    #[error("published prompt target is unavailable")]
    Unavailable,
    #[error("published prompt call queue is unavailable")]
    QueueUnavailable,
    #[error("upstream prompt call failed")]
    Upstream,
    #[error("upstream prompt call timed out")]
    Timeout,
    #[error("upstream prompt call was cancelled")]
    Cancelled,
}

pub(crate) struct PreparedExactPromptCall {
    observed: super::incarnation::ObservedConnectionCatalogEntry,
    generation: PromptCatalogGeneration,
    native_name: String,
    outcome: RawCallOutcome<GetPromptResult>,
}

impl UpstreamPool {
    /// Execute one regular Prompt call only when its exact current publication
    /// route and connection incarnation still agree. This kernel is unmounted.
    pub(crate) async fn get_published_prompt_exact(
        &self,
        upstream_name: &str,
        native_name: &str,
        generation: PromptCatalogGeneration,
        params: GetPromptRequestParams,
    ) -> Result<GetPromptResult, ExactPromptCallError> {
        let prepared = self
            .prepare_published_prompt_exact(upstream_name, native_name, generation, params)
            .await?;
        self.apply_prepared_prompt_exact(prepared).await
    }

    pub(crate) async fn prepare_published_prompt_exact(
        &self,
        upstream_name: &str,
        native_name: &str,
        generation: PromptCatalogGeneration,
        params: GetPromptRequestParams,
    ) -> Result<PreparedExactPromptCall, ExactPromptCallError> {
        if params.name != native_name {
            return Err(ExactPromptCallError::Unavailable);
        }
        let start = tokio::time::Instant::now();
        let permit = tokio::time::timeout(
            self.request_timeout,
            self.acquire_upstream_call_permit(upstream_name),
        )
        .await;
        let _permit = match permit {
            Ok(Ok(permit)) => permit,
            _ => return Err(ExactPromptCallError::QueueUnavailable),
        };
        let Some(observed) = self
            .observe_prompt_call(upstream_name, native_name, generation)
            .await
        else {
            return Err(ExactPromptCallError::Unavailable);
        };
        let remaining = self.request_timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Err(ExactPromptCallError::QueueUnavailable);
        }
        let outcome = classify_timeout_result(
            tokio::time::timeout(remaining, observed.peer.get_prompt(params)).await,
        );
        Ok(PreparedExactPromptCall {
            observed,
            generation,
            native_name: native_name.to_string(),
            outcome,
        })
    }

    pub(crate) async fn apply_prepared_prompt_exact(
        &self,
        prepared: PreparedExactPromptCall,
    ) -> Result<GetPromptResult, ExactPromptCallError> {
        let upstream_name = prepared.observed.upstream().to_string();
        match prepared.outcome {
            RawCallOutcome::Ok(result) => {
                let applied = self
                    .apply_to_observed_prompt_call(
                        &prepared.observed,
                        prepared.generation,
                        &prepared.native_name,
                        |entry| {
                            super::health::record_success_on_entry(
                                &upstream_name,
                                entry,
                                UpstreamCapability::Prompts,
                            );
                        },
                    )
                    .await;
                applied
                    .map(|()| result)
                    .ok_or(ExactPromptCallError::Unavailable)
            }
            RawCallOutcome::UpstreamError(error) => {
                let affects_health = service_error_affects_connection_health(&error);
                let message = super::capability_call::bounded_service_error_text(&error);
                let applied = self
                    .apply_to_observed_prompt_call(
                        &prepared.observed,
                        prepared.generation,
                        &prepared.native_name,
                        |entry| {
                            if affects_health {
                                super::health::record_failure_on_entry(
                                    &upstream_name,
                                    entry,
                                    UpstreamCapability::Prompts,
                                    format!("upstream prompt get failed: {message}"),
                                );
                            } else {
                                super::health::record_success_on_entry(
                                    &upstream_name,
                                    entry,
                                    UpstreamCapability::Prompts,
                                );
                            }
                        },
                    )
                    .await;
                if applied.is_none() {
                    Err(ExactPromptCallError::Unavailable)
                } else {
                    Err(ExactPromptCallError::Upstream)
                }
            }
            RawCallOutcome::Timeout => {
                let message = format!(
                    "upstream prompt get timed out after {}ms",
                    self.request_timeout.as_millis()
                );
                let applied = self
                    .apply_to_observed_prompt_call(
                        &prepared.observed,
                        prepared.generation,
                        &prepared.native_name,
                        |entry| {
                            super::health::record_failure_on_entry(
                                &upstream_name,
                                entry,
                                UpstreamCapability::Prompts,
                                message.clone(),
                            );
                        },
                    )
                    .await;
                if applied.is_none() {
                    Err(ExactPromptCallError::Unavailable)
                } else {
                    Err(ExactPromptCallError::Timeout)
                }
            }
            RawCallOutcome::Cancelled => Err(ExactPromptCallError::Cancelled),
        }
    }
    /// Discover prompts from all OAuth upstreams visible to `subject`.
    ///
    /// P-C1 fix: uses `acquire_or_connect_subject` so connections are cached;
    /// the tools list from connect is not needed here but the cached peer is
    /// used directly for `list_prompts`.
    pub async fn subject_scoped_prompts(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        builtin_names: &[&str],
    ) -> Vec<Prompt> {
        let deadline_at = tokio::time::Instant::now() + self.request_timeout;
        self.subject_scoped_prompts_until(configs, subject, builtin_names, deadline_at)
            .await
    }

    /// Discover OAuth prompts within a caller-owned absolute deadline.
    pub async fn subject_scoped_prompts_until(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        builtin_names: &[&str],
        deadline_at: tokio::time::Instant,
    ) -> Vec<Prompt> {
        let mut futures = FuturesUnordered::new();
        for config in configs.iter().filter(|config| config.oauth.is_some()) {
            let config = config.clone();
            let subject = subject.to_string();
            let pool = self.clone();
            futures.push(async move {
                // No catalog entry backs a subject-scoped prompt list, so the
                // policy comes from the live config — the same fail-closed
                // resolver the catalog path compiles into the entry.
                let policy = resolve_request_prompt_exposure_policy(
                    &config.name,
                    config.expose_prompts.clone(),
                );
                let _fanout_permit = match tokio::time::timeout_at(
                    deadline_at,
                    pool.acquire_catalog_fanout_permit(),
                )
                .await
                {
                    Ok(Ok(permit)) => permit,
                    Ok(Err(error)) => {
                        tracing::warn!(
                            upstream = %config.name,
                            kind = "queue_closed",
                            error = %error,
                            "subject-scoped upstream prompt discovery failed"
                        );
                        return (config.name.clone(), policy, None);
                    }
                    Err(_) => {
                        tracing::warn!(
                            upstream = %config.name,
                            kind = "timeout",
                            phase = "oauth_fanout_gate",
                            partial_result = true,
                            "subject-scoped prompt fanout permit wait exceeded request deadline"
                        );
                        return (config.name.clone(), policy, None);
                    }
                };
                let result = tokio::time::timeout_at(
                    deadline_at,
                    pool.acquire_or_connect_subject(&config, &subject),
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!("subject-scoped prompt acquisition exceeded request deadline")
                })
                .and_then(|result| result.map(|(peer, _tools)| peer));
                (config.name.clone(), policy, Some(result))
            });
        }

        let mut upstream_prompts = Vec::new();
        while let Some((name, policy, result)) = futures.next().await {
            let Some(result) = result else {
                continue;
            };
            let peer = match result {
                Ok(peer) => peer,
                Err(error) => {
                    let error_text = error.to_string();
                    tracing::warn!(
                        upstream = %name,
                        kind = super::helpers::classify_upstream_error(&error_text),
                        phase = "oauth_acquisition",
                        partial_result = true,
                        error = %error_text,
                        "subject-scoped prompt acquisition failed within shared request deadline"
                    );
                    continue;
                }
            };
            let remaining = deadline_at.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                tracing::warn!(
                    upstream = %name,
                    kind = "timeout",
                    phase = "oauth_pagination",
                    partial_result = true,
                    "subject-scoped prompt pagination skipped after request deadline"
                );
                continue;
            }
            match catalog_pagination::list_prompts(
                &peer,
                remaining,
                super::tools::MAX_UPSTREAM_PROMPTS,
            )
            .await
            {
                Ok(prompts) => {
                    let discovered_count = prompts.len();
                    let exposed: Vec<Prompt> = prompts
                        .into_iter()
                        .filter(|prompt| prompt_exposed(&policy, &name, &prompt.name))
                        .collect();
                    log_exposure_filter(
                        &name,
                        "prompts",
                        discovered_count - exposed.len(),
                        exposed.len(),
                        true,
                    );
                    upstream_prompts.push((name, exposed));
                }
                Err(error) => {
                    tracing::warn!(
                        upstream = %name,
                        kind = error.kind(),
                        phase = "oauth_pagination",
                        partial_result = true,
                        error = %error.bounded_text(),
                        "subject-scoped upstream prompt discovery failed"
                    );
                }
            }
        }

        let (prompts, _) = merge_upstream_prompts(builtin_names, upstream_prompts);
        prompts
    }

    /// Find which upstream owns `prompt_name` for `subject`.
    ///
    /// P-C1 fix: uses `acquire_or_connect_subject` so connections are cached.
    pub async fn subject_scoped_prompt_owner(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        prompt_name: &str,
    ) -> Option<String> {
        let mut futures = FuturesUnordered::new();
        for config in configs.iter().filter(|config| config.oauth.is_some()) {
            let config = config.clone();
            let subject = subject.to_string();
            let pool = self.clone();
            let target_prompt = prompt_name.to_string();
            futures.push(async move {
                let policy = resolve_request_prompt_exposure_policy(
                    &config.name,
                    config.expose_prompts.clone(),
                );
                let _fanout_permit = match pool.acquire_catalog_fanout_permit().await {
                    Ok(permit) => permit,
                    Err(_) => return (config.name.clone(), policy, target_prompt, None),
                };
                let result = pool
                    .acquire_or_connect_subject(&config, &subject)
                    .await
                    .map(|(peer, _tools)| peer);
                (config.name.clone(), policy, target_prompt, Some(result))
            });
        }

        while let Some((name, policy, target_prompt, result)) = futures.next().await {
            let Some(result) = result else {
                continue;
            };
            let Ok(peer) = result else {
                continue;
            };
            match catalog_pagination::list_prompts(
                &peer,
                self.request_timeout,
                super::tools::MAX_UPSTREAM_PROMPTS,
            )
            .await
            {
                Ok(prompts)
                    if prompts.iter().any(|prompt| {
                        // The requested name is namespaced as `{upstream}/{name}`;
                        // the upstream advertises the bare name, so compare against
                        // the prefixed form. A prompt `expose_prompts` hides must not
                        // resolve an owner either, or the caller would route a fetch
                        // at a prompt it is not allowed to see.
                        prefixed_upstream_prompt_name(&name, &prompt.name) == target_prompt
                            && prompt_exposed(&policy, &name, &prompt.name)
                    }) =>
                {
                    return Some(name);
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    upstream = %name,
                    kind = error.kind(),
                    error = %error.bounded_text(),
                    "subject-scoped upstream prompt ownership discovery failed"
                ),
            }
        }
        None
    }

    /// Whether `prompt_name` is exposed by the cached `expose_prompts` policy
    /// for `upstream_name`. Fails closed when the upstream has no catalog entry.
    pub(super) async fn prompt_is_exposed(&self, upstream_name: &str, prompt_name: &str) -> bool {
        let catalog = self.catalog.read().await;
        catalog.get(upstream_name).is_some_and(|entry| {
            prompt_exposed(&entry.prompt_exposure_policy, upstream_name, prompt_name)
        })
    }

    /// Proxy a get-prompt request to a specific upstream.
    pub async fn get_prompt(
        &self,
        upstream_name: &str,
        mut params: GetPromptRequestParams,
    ) -> Option<Result<GetPromptResult, String>> {
        let start = Instant::now();
        // The gateway namespaces upstream prompt names as `{upstream}/{name}`,
        // but the upstream only knows the bare name — strip the prefix before
        // forwarding (mirrors `read_upstream_resource` stripping the URI prefix).
        params.name = bare_upstream_prompt_name(upstream_name, &params.name).to_string();
        let prompt_name = params.name.to_string();
        // Enforce `expose_prompts` before forwarding. The cached ownership
        // snapshot is deliberately unfiltered (it backs the admin exposure
        // editor), so this is the gate that stops a hidden prompt from being
        // fetched by name.
        if !self.prompt_is_exposed(upstream_name, &prompt_name).await {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "prompt.get",
                capability = "prompts",
                upstream = %upstream_name,
                prompt = %prompt_name,
                kind = "prompt_not_exposed",
                "upstream prompt get blocked by exposure policy"
            );
            return Some(Err(format!(
                "prompt `{prompt_name}` is not exposed by upstream `{upstream_name}`"
            )));
        }
        let event = UpstreamRequestLog::prompt(upstream_name, &prompt_name, false);
        let peer = self
            .acquire_peer(upstream_name, UpstreamCapability::Prompts, "prompt.get")
            .await?;

        log_upstream_request_start(event);

        let timeout_ms = self.request_timeout.as_millis();
        Some(
            timed_capability_call_str(
                self,
                upstream_name,
                UpstreamCapability::Prompts,
                event,
                start,
                peer.get_prompt(params),
                |_result: &GetPromptResult| 0, // prompts have no size cap
                None,
                |e| format!("upstream prompt get failed: {e}"),
                format!("upstream prompt get timed out after {timeout_ms}ms"),
            )
            .await,
        )
    }

    /// Get a prompt from an OAuth-subject-scoped upstream.
    ///
    /// P-C1 fix: uses `acquire_or_connect_subject` so the per-(upstream,subject)
    /// connection is reused from cache rather than opened fresh each call.
    pub async fn subject_scoped_get_prompt(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        mut params: GetPromptRequestParams,
    ) -> Result<GetPromptResult, String> {
        let start = Instant::now();
        // Strip the `{upstream}/` namespace before forwarding the bare name.
        params.name = bare_upstream_prompt_name(&config.name, &params.name).to_string();
        let prompt_name = params.name.to_string();
        if !prompt_exposed(
            &resolve_request_prompt_exposure_policy(&config.name, config.expose_prompts.clone()),
            &config.name,
            &prompt_name,
        ) {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "prompt.get",
                capability = "prompts",
                upstream = %config.name,
                subject_scoped = true,
                prompt = %prompt_name,
                kind = "prompt_not_exposed",
                "upstream prompt get blocked by exposure policy"
            );
            return Err(format!(
                "prompt `{prompt_name}` is not exposed by upstream `{}`",
                config.name
            ));
        }
        let event = UpstreamRequestLog::prompt(&config.name, &prompt_name, true)
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        // P-C1: reuse cached per-(upstream,subject) connection.
        let (peer, _tools) = match self.acquire_or_connect_subject(config, subject).await {
            Ok(pair) => pair,
            Err(error) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Prompts,
                    format!("upstream prompt connect failed: {error}"),
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
            UpstreamCapability::Prompts,
            event,
            start,
            peer.get_prompt(params),
            |_result: &GetPromptResult| 0, // prompts have no size cap
            Some(subject),
            |e| format!("upstream prompt get failed: {e}"),
            format!("upstream prompt get timed out after {timeout_ms}ms"),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use rmcp::model::{
        ErrorData, GetPromptRequestParams, GetPromptResponse, GetPromptResult, Prompt,
        PromptMessage, Role,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler};
    use tokio::sync::{Mutex, Notify};

    use super::super::testsupport::*;
    use super::ExactPromptCallError;
    use crate::upstream::types::{CIRCUIT_BREAKER_THRESHOLD, ToolExposurePolicy, UpstreamHealth};

    #[derive(Clone)]
    struct DelayedGetPromptServer {
        started: Arc<Notify>,
        release: Arc<Notify>,
        fail: bool,
    }

    impl ServerHandler for DelayedGetPromptServer {
        async fn get_prompt(
            &self,
            request: GetPromptRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetPromptResponse, ErrorData> {
            self.started.notify_one();
            self.release.notified().await;
            if self.fail {
                return Err(ErrorData::internal_error("delayed private failure", None));
            }
            Ok(
                GetPromptResult::new(vec![PromptMessage::new_text(Role::User, request.name)])
                    .into(),
            )
        }
    }

    #[derive(Clone)]
    struct InspectingGetPromptServer {
        received: Arc<Mutex<Vec<GetPromptRequestParams>>>,
        calls: Arc<AtomicUsize>,
        fail: bool,
    }

    impl ServerHandler for InspectingGetPromptServer {
        async fn get_prompt(
            &self,
            request: GetPromptRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetPromptResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.received.lock().await.push(request.clone());
            if self.fail {
                return Err(ErrorData::invalid_params(
                    "private application detail",
                    None,
                ));
            }
            let argument = request
                .arguments
                .as_ref()
                .and_then(|arguments| arguments.get("target"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("missing");
            Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                Role::User,
                format!("{}:{argument}", request.name),
            )])
            .into())
        }
    }

    #[derive(Clone)]
    struct SlowCountingPromptServer {
        calls: Arc<AtomicUsize>,
        delay: Duration,
        started: Option<Arc<Notify>>,
    }

    impl ServerHandler for SlowCountingPromptServer {
        async fn get_prompt(
            &self,
            request: GetPromptRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetPromptResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(started) = &self.started {
                started.notify_one();
            }
            tokio::time::sleep(self.delay).await;
            Ok(
                GetPromptResult::new(vec![PromptMessage::new_text(Role::User, request.name)])
                    .into(),
            )
        }
    }

    #[tokio::test]
    async fn get_prompt_times_out_slow_upstream_response() {
        let pool = slow_response_pool("slow").await;

        let result = pool
            .get_prompt("slow", GetPromptRequestParams::new("slow.prompt"))
            .await
            .expect("upstream is connected")
            .expect_err("slow prompt get should time out");

        assert!(result.contains("timed out"));
    }

    #[tokio::test]
    async fn exact_prompt_kernel_requires_current_publication_and_native_name() {
        let pool = slow_response_pool("slow").await;
        pool.insert_prompt_routes_for_tests(
            "slow",
            vec![Prompt::new("nested/name", None::<String>, None)],
        )
        .await;
        let generation = pool
            .published_prompt_catalog()
            .await
            .expect("prompt publication")
            .generation();

        let wrong_name = pool
            .get_published_prompt_exact(
                "slow",
                "nested/name",
                generation,
                GetPromptRequestParams::new("other"),
            )
            .await
            .expect_err("request envelope must match exact native name");
        assert_eq!(wrong_name, ExactPromptCallError::Unavailable);

        pool.insert_prompt_routes_for_tests(
            "slow",
            vec![Prompt::new("replacement", None::<String>, None)],
        )
        .await;
        let stale = pool
            .get_published_prompt_exact(
                "slow",
                "nested/name",
                generation,
                GetPromptRequestParams::new("nested/name"),
            )
            .await
            .expect_err("stale publication must fail closed before RPC");
        assert_eq!(stale, ExactPromptCallError::Unavailable);
    }

    #[tokio::test]
    async fn exact_prompt_kernel_discards_prompt_generation_aba_outcomes() {
        for fail in [false, true] {
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let pool = catalog_pool_with_server(
                "alpha",
                DelayedGetPromptServer {
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                    fail,
                },
            )
            .await;
            pool.insert_prompt_routes_for_tests(
                "alpha",
                vec![Prompt::new("nested/name", None::<String>, None)],
            )
            .await;
            {
                let mut catalog = pool.catalog_write().await;
                catalog.get_mut("alpha").unwrap().prompt_last_error = Some("sentinel".into());
            }
            let generation = pool.published_prompt_catalog().await.unwrap().generation();
            let calling = Arc::clone(&pool);
            let task = tokio::spawn(async move {
                calling
                    .get_published_prompt_exact(
                        "alpha",
                        "nested/name",
                        generation,
                        GetPromptRequestParams::new("nested/name"),
                    )
                    .await
            });
            started.notified().await;
            pool.insert_prompt_routes_for_tests(
                "alpha",
                vec![Prompt::new("other", None::<String>, None)],
            )
            .await;
            pool.insert_prompt_routes_for_tests(
                "alpha",
                vec![Prompt::new("nested/name", None::<String>, None)],
            )
            .await;
            release.notify_one();
            assert_eq!(task.await.unwrap(), Err(ExactPromptCallError::Unavailable));
            assert_eq!(
                pool.catalog
                    .read()
                    .await
                    .get("alpha")
                    .unwrap()
                    .prompt_last_error
                    .as_deref(),
                Some("sentinel")
            );
        }
    }

    #[tokio::test]
    async fn exact_prompt_kernel_returns_current_success_and_attributes_timeout() {
        let pool = static_catalog_pool("alpha").await;
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("upstream.prompt.one", None::<String>, None)],
        )
        .await;
        let generation = pool.published_prompt_catalog().await.unwrap().generation();
        let result = pool
            .get_published_prompt_exact(
                "alpha",
                "upstream.prompt.one",
                generation,
                GetPromptRequestParams::new("upstream.prompt.one"),
            )
            .await
            .expect("current exact call");
        assert_eq!(result.messages.len(), 1);
        assert!(
            pool.catalog
                .read()
                .await
                .get("alpha")
                .unwrap()
                .prompt_last_error
                .is_none()
        );

        let mut slow = catalog_pool_with_server(
            "slow",
            SlowCountingPromptServer {
                calls: Arc::new(AtomicUsize::new(0)),
                delay: Duration::from_millis(200),
                started: None,
            },
        )
        .await;
        Arc::get_mut(&mut slow)
            .expect("test owns pool")
            .request_timeout = Duration::from_millis(25);
        slow.insert_prompt_routes_for_tests(
            "slow",
            vec![Prompt::new("nested/name", None::<String>, None)],
        )
        .await;
        let generation = slow.published_prompt_catalog().await.unwrap().generation();
        let timeout = slow
            .get_published_prompt_exact(
                "slow",
                "nested/name",
                generation,
                GetPromptRequestParams::new("nested/name"),
            )
            .await;
        assert_eq!(timeout, Err(ExactPromptCallError::Timeout));
    }

    #[tokio::test]
    async fn exact_prompt_kernel_cancellation_does_not_apply_outcome() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let pool = catalog_pool_with_server(
            "alpha",
            DelayedGetPromptServer {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                fail: false,
            },
        )
        .await;
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("nested/name", None::<String>, None)],
        )
        .await;
        let generation = pool.published_prompt_catalog().await.unwrap().generation();
        {
            let mut catalog = pool.catalog_write().await;
            catalog.get_mut("alpha").unwrap().prompt_last_error = Some("sentinel".into());
        }
        let calling = Arc::clone(&pool);
        let task = tokio::spawn(async move {
            calling
                .get_published_prompt_exact(
                    "alpha",
                    "nested/name",
                    generation,
                    GetPromptRequestParams::new("nested/name"),
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
                .prompt_last_error
                .as_deref(),
            Some("sentinel")
        );
    }

    #[tokio::test]
    async fn exact_prompt_kernel_discards_connection_aba_success_and_failure() {
        for fail in [false, true] {
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let pool = catalog_pool_with_server(
                "alpha",
                DelayedGetPromptServer {
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                    fail,
                },
            )
            .await;
            pool.insert_prompt_routes_for_tests(
                "alpha",
                vec![Prompt::new("nested/name", None::<String>, None)],
            )
            .await;
            let generation = pool.published_prompt_catalog().await.unwrap().generation();
            let calling = Arc::clone(&pool);
            let task = tokio::spawn(async move {
                calling
                    .get_published_prompt_exact(
                        "alpha",
                        "nested/name",
                        generation,
                        GetPromptRequestParams::new("nested/name"),
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
            entry_a.prompt_last_error = Some("replacement sentinel".into());
            pool.install_connection_catalog_entry("alpha".into(), previous_a, entry_a)
                .await
                .unwrap();
            pool.insert_prompt_routes_for_tests(
                "alpha",
                vec![Prompt::new("nested/name", None::<String>, None)],
            )
            .await;
            release.notify_one();
            assert_eq!(task.await.unwrap(), Err(ExactPromptCallError::Unavailable));
            assert_eq!(
                pool.catalog
                    .read()
                    .await
                    .get("alpha")
                    .unwrap()
                    .prompt_last_error
                    .as_deref(),
                Some("replacement sentinel")
            );
            if let Some(connection) = removed_b {
                connection.shutdown("alpha", "test.prompt-get.aba").await;
            }
        }
    }

    #[tokio::test]
    async fn exact_prompt_kernel_forwards_nested_name_and_arguments() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let pool = catalog_pool_with_server(
            "alpha",
            InspectingGetPromptServer {
                received: Arc::clone(&received),
                calls: Arc::clone(&calls),
                fail: false,
            },
        )
        .await;
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("owner/nested/name", None::<String>, None)],
        )
        .await;
        let generation = pool.published_prompt_catalog().await.unwrap().generation();
        let arguments = serde_json::Map::from_iter([(
            "target".to_string(),
            serde_json::Value::String("exact-value".to_string()),
        )]);
        let result = pool
            .get_published_prompt_exact(
                "alpha",
                "owner/nested/name",
                generation,
                GetPromptRequestParams::new("owner/nested/name").with_arguments(arguments.clone()),
            )
            .await
            .expect("current exact nested call");

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let received = received.lock().await;
        assert_eq!(received[0].name, "owner/nested/name");
        assert_eq!(received[0].arguments.as_ref(), Some(&arguments));
        assert_eq!(result.messages.len(), 1);
    }

    #[tokio::test]
    async fn exact_prompt_kernel_treats_current_mcp_error_as_healthy() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let pool = catalog_pool_with_server(
            "alpha",
            InspectingGetPromptServer {
                received,
                calls: Arc::new(AtomicUsize::new(0)),
                fail: true,
            },
        )
        .await;
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("nested/name", None::<String>, None)],
        )
        .await;
        {
            let mut catalog = pool.catalog_write().await;
            catalog.get_mut("alpha").unwrap().prompt_last_error = Some("sentinel".into());
        }
        let generation = pool.published_prompt_catalog().await.unwrap().generation();
        let error = pool
            .get_published_prompt_exact(
                "alpha",
                "nested/name",
                generation,
                GetPromptRequestParams::new("nested/name"),
            )
            .await
            .expect_err("MCP application error remains an upstream error");

        assert_eq!(error, ExactPromptCallError::Upstream);
        assert!(!error.to_string().contains("private application detail"));
        let catalog = pool.catalog.read().await;
        let entry = catalog.get("alpha").unwrap();
        assert_eq!(entry.prompt_health, UpstreamHealth::Healthy);
        assert!(entry.prompt_last_error.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn exact_prompt_kernel_queue_and_rpc_share_one_deadline() {
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let mut pool = catalog_pool_with_server(
            "alpha",
            SlowCountingPromptServer {
                calls: Arc::clone(&calls),
                delay: Duration::from_millis(80),
                started: Some(Arc::clone(&started)),
            },
        )
        .await;
        let pool_mut = Arc::get_mut(&mut pool).expect("test owns the sole pool Arc");
        pool_mut.request_timeout = Duration::from_millis(100);
        pool_mut.call_concurrency = 1;
        pool_mut.call_semaphores = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("nested/name", None::<String>, None)],
        )
        .await;
        {
            let mut catalog = pool.catalog_write().await;
            catalog.get_mut("alpha").unwrap().prompt_last_error = Some("sentinel".into());
        }
        let generation = pool.published_prompt_catalog().await.unwrap().generation();
        let held = pool.acquire_upstream_call_permit("alpha").await.unwrap();
        let calling = Arc::clone(&pool);
        let task = tokio::spawn(async move {
            calling
                .get_published_prompt_exact(
                    "alpha",
                    "nested/name",
                    generation,
                    GetPromptRequestParams::new("nested/name"),
                )
                .await
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(70)).await;
        drop(held);
        started.notified().await;
        tokio::time::advance(Duration::from_millis(30)).await;
        let result = task.await.unwrap();

        assert_eq!(result, Err(ExactPromptCallError::Timeout));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(
            pool.catalog
                .read()
                .await
                .get("alpha")
                .unwrap()
                .prompt_last_error
                .as_deref()
                .is_some_and(|error| error.contains("timed out"))
        );
    }

    #[tokio::test]
    async fn exact_prompt_kernel_queue_saturation_does_not_call_or_mutate_health() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut pool = catalog_pool_with_server(
            "alpha",
            SlowCountingPromptServer {
                calls: Arc::clone(&calls),
                delay: Duration::from_millis(1),
                started: None,
            },
        )
        .await;
        let pool_mut = Arc::get_mut(&mut pool).expect("test owns pool");
        pool_mut.request_timeout = Duration::from_millis(25);
        pool_mut.call_concurrency = 1;
        pool_mut.call_semaphores = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("nested/name", None::<String>, None)],
        )
        .await;
        {
            let mut catalog = pool.catalog_write().await;
            catalog.get_mut("alpha").unwrap().prompt_last_error = Some("sentinel".into());
        }
        let generation = pool.published_prompt_catalog().await.unwrap().generation();
        let _held = pool.acquire_upstream_call_permit("alpha").await.unwrap();
        assert_eq!(
            pool.get_published_prompt_exact(
                "alpha",
                "nested/name",
                generation,
                GetPromptRequestParams::new("nested/name"),
            )
            .await,
            Err(ExactPromptCallError::QueueUnavailable)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            pool.catalog
                .read()
                .await
                .get("alpha")
                .unwrap()
                .prompt_last_error
                .as_deref(),
            Some("sentinel")
        );
    }

    #[tokio::test]
    async fn exact_prompt_kernel_observes_target_after_queue_wait() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut pool = catalog_pool_with_server(
            "alpha",
            InspectingGetPromptServer {
                received,
                calls: Arc::clone(&calls),
                fail: false,
            },
        )
        .await;
        let pool_mut = Arc::get_mut(&mut pool).expect("test owns pool");
        pool_mut.request_timeout = Duration::from_millis(100);
        pool_mut.call_concurrency = 1;
        pool_mut.call_semaphores = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("nested/name", None::<String>, None)],
        )
        .await;
        let generation = pool.published_prompt_catalog().await.unwrap().generation();
        let held = pool.acquire_upstream_call_permit("alpha").await.unwrap();
        let calling = Arc::clone(&pool);
        let task = tokio::spawn(async move {
            calling
                .get_published_prompt_exact(
                    "alpha",
                    "nested/name",
                    generation,
                    GetPromptRequestParams::new("nested/name"),
                )
                .await
        });
        tokio::task::yield_now().await;
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("replacement", None::<String>, None)],
        )
        .await;
        drop(held);

        assert_eq!(task.await.unwrap(), Err(ExactPromptCallError::Unavailable));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn exact_prompt_kernel_rejects_hidden_unhealthy_and_missing_without_rpc() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let pool = catalog_pool_with_server(
            "alpha",
            InspectingGetPromptServer {
                received,
                calls: Arc::clone(&calls),
                fail: false,
            },
        )
        .await;
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("nested/name", None::<String>, None)],
        )
        .await;

        {
            let mut catalog = pool.catalog_write().await;
            catalog.get_mut("alpha").unwrap().prompt_exposure_policy =
                ToolExposurePolicy::from_patterns(vec!["other".to_string()]).unwrap();
        }
        let hidden_generation = pool.published_prompt_catalog().await.unwrap().generation();
        assert_eq!(
            pool.get_published_prompt_exact(
                "alpha",
                "nested/name",
                hidden_generation,
                GetPromptRequestParams::new("nested/name"),
            )
            .await,
            Err(ExactPromptCallError::Unavailable)
        );

        {
            let mut catalog = pool.catalog_write().await;
            let entry = catalog.get_mut("alpha").unwrap();
            entry.prompt_exposure_policy = ToolExposurePolicy::All;
            entry.prompt_health = UpstreamHealth::Unhealthy {
                consecutive_failures: CIRCUIT_BREAKER_THRESHOLD,
            };
        }
        let unhealthy_generation = pool.published_prompt_catalog().await.unwrap().generation();
        assert_eq!(
            pool.get_published_prompt_exact(
                "alpha",
                "nested/name",
                unhealthy_generation,
                GetPromptRequestParams::new("nested/name"),
            )
            .await,
            Err(ExactPromptCallError::Unavailable)
        );
        assert_eq!(
            pool.get_published_prompt_exact(
                "missing",
                "nested/name",
                unhealthy_generation,
                GetPromptRequestParams::new("nested/name"),
            )
            .await,
            Err(ExactPromptCallError::Unavailable)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
