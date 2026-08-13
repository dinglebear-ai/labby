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
use super::UpstreamPool;
use super::capability_call::timed_capability_call_str;
use super::entries::{log_exposure_filter, prompt_exposed, resolve_request_prompt_exposure_policy};
use super::helpers::{
    bare_upstream_prompt_name, merge_upstream_prompts, prefixed_upstream_prompt_name,
    upstream_transport,
};
use super::logging::{UpstreamRequestLog, log_upstream_request_error, log_upstream_request_start};
use super::paginate::list_prompts_bounded;

impl UpstreamPool {
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
                let result = pool
                    .acquire_or_connect_subject(&config, &subject)
                    .await
                    .map(|(peer, _tools)| peer);
                (config.name.clone(), policy, result)
            });
        }

        let mut upstream_prompts = Vec::new();
        while let Some((name, policy, result)) = futures.next().await {
            let peer = match result {
                Ok(peer) => peer,
                Err(error) => {
                    tracing::debug!(
                        upstream = %name,
                        error = %error,
                        "subject-scoped prompt discovery skipped upstream — connect failed"
                    );
                    continue;
                }
            };
            // Subject-scoped listings never land in `self.catalog`, so there
            // is no entry to record a truncation note on — the WARN inside
            // the bounded helper is the visibility here.
            let (prompts, _truncation) = match list_prompts_bounded(&peer, &name).await {
                Ok(listing) => listing,
                Err(error) => {
                    tracing::warn!(
                        upstream = %name,
                        error = %error,
                        "subject-scoped upstream prompt discovery failed"
                    );
                    continue;
                }
            };
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
                let result = pool
                    .acquire_or_connect_subject(&config, &subject)
                    .await
                    .map(|(peer, _tools)| peer);
                (config.name.clone(), policy, target_prompt, result)
            });
        }

        while let Some((name, policy, target_prompt, result)) = futures.next().await {
            let peer = match result {
                Ok(peer) => peer,
                Err(error) => {
                    tracing::debug!(
                        upstream = %name,
                        error = %error,
                        "subject-scoped prompt owner lookup skipped upstream — connect failed"
                    );
                    continue;
                }
            };
            // A listing error must not silently read as "prompt not found" —
            // that hides an erroring upstream behind a lookup miss with no
            // diagnostic trail.
            let (prompts, _truncation) = match list_prompts_bounded(&peer, &name).await {
                Ok(listing) => listing,
                Err(error) => {
                    tracing::warn!(
                        upstream = %name,
                        error = %error,
                        "subject-scoped prompt owner lookup failed to list prompts"
                    );
                    continue;
                }
            };
            if prompts.iter().any(|prompt| {
                // The requested name is namespaced as `{upstream}/{name}`;
                // the upstream advertises the bare name, so compare against
                // the prefixed form. A prompt `expose_prompts` hides must not
                // resolve an owner either, or the caller would route a fetch
                // at a prompt it is not allowed to see.
                prefixed_upstream_prompt_name(&name, &prompt.name) == target_prompt
                    && prompt_exposed(&policy, &name, &prompt.name)
            }) {
                return Some(name);
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
    use rmcp::model::GetPromptRequestParams;

    use super::super::testsupport::*;

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
}
