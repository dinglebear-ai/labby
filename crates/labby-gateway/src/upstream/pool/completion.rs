//! Upstream completion proxying for prompt and resource-template references.

use std::time::Instant;

use rmcp::model::{CompleteRequestParams, CompleteResult, Reference};

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::capability_call::timed_capability_call_str;
use super::entries::{prompt_exposed, resolve_request_prompt_exposure_policy};
use super::helpers::{bare_upstream_prompt_name, upstream_transport};
use super::logging::{UpstreamRequestLog, log_upstream_request_error, log_upstream_request_start};

fn rewrite_completion_reference(
    params: &mut CompleteRequestParams,
    upstream_name: &str,
) -> Option<(UpstreamCapability, String)> {
    match &mut params.r#ref {
        Reference::Prompt(prompt) => {
            prompt.name = bare_upstream_prompt_name(upstream_name, &prompt.name).to_string();
            Some((UpstreamCapability::Prompts, prompt.name.clone()))
        }
        Reference::Resource(resource) => {
            let prefix = format!("lab://upstream/{upstream_name}/");
            resource.uri = resource.uri.strip_prefix(&prefix)?.to_string();
            Some((UpstreamCapability::Resources, resource.uri.clone()))
        }
        _ => None,
    }
}

fn estimate_complete_result_size(result: &CompleteResult) -> usize {
    serde_json::to_vec(result).map_or(0, |bytes| bytes.len())
}

impl UpstreamPool {
    /// Whether a rewritten completion reference is exposed by the cached policy.
    ///
    /// Only prompt references are gated. A `Reference::Resource` completion
    /// carries a resource *template* URI (`file:///{path}`), and
    /// `expose_resources` lists concrete URIs — there is nothing well-defined to
    /// match, and gating on it would break template completion for every
    /// upstream that sets an allowlist. Templates are unfiltered for the same
    /// reason (see `docs/services/UPSTREAM.md`); reads of the concrete URIs a
    /// template expands to are still gated by `expose_resources`.
    async fn completion_reference_is_exposed(
        &self,
        upstream_name: &str,
        capability: UpstreamCapability,
        reference: &str,
    ) -> bool {
        match capability {
            UpstreamCapability::Prompts => self.prompt_is_exposed(upstream_name, reference).await,
            UpstreamCapability::Resources | UpstreamCapability::Tools => true,
        }
    }

    /// Proxy a completion request to a connected upstream, rewriting only the
    /// gateway-owned prompt/resource namespace while preserving request metadata
    /// and completion context.
    pub async fn complete_reference(
        &self,
        upstream_name: &str,
        mut params: CompleteRequestParams,
    ) -> Option<Result<CompleteResult, String>> {
        let start = Instant::now();
        let (capability, reference) = match rewrite_completion_reference(&mut params, upstream_name)
        {
            Some(rewritten) => rewritten,
            None => {
                return Some(Err(format!(
                    "completion reference does not belong to upstream `{upstream_name}`"
                )));
            }
        };
        // Completions are argument autocomplete *for* a prompt or resource
        // template. Answering for a reference the operator excluded would leak
        // the hidden item's shape, so gate on the same cached policy the list
        // and fetch paths use.
        if !self
            .completion_reference_is_exposed(upstream_name, capability, &reference)
            .await
        {
            return Some(Err(format!(
                "completion reference is not exposed by upstream `{upstream_name}`"
            )));
        }
        let peer = self
            .acquire_peer(upstream_name, capability, "completion.complete")
            .await?;
        let event = UpstreamRequestLog::completion(upstream_name, &reference, false);
        log_upstream_request_start(event);
        let timeout_ms = self.request_timeout.as_millis();

        Some(
            timed_capability_call_str(
                self,
                upstream_name,
                capability,
                event,
                start,
                peer.complete(params),
                estimate_complete_result_size,
                None,
                |error| format!("upstream completion failed: {error}"),
                format!("upstream completion timed out after {timeout_ms}ms"),
            )
            .await,
        )
    }

    /// Proxy completion over the cached OAuth subject connection.
    pub async fn subject_scoped_complete_reference(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        mut params: CompleteRequestParams,
    ) -> Result<CompleteResult, String> {
        let start = Instant::now();
        let (capability, reference) = rewrite_completion_reference(&mut params, &config.name)
            .ok_or_else(|| {
                format!(
                    "completion reference does not belong to upstream `{}`",
                    config.name
                )
            })?;
        // Prompt references only — see `completion_reference_is_exposed`.
        let exposed = match capability {
            UpstreamCapability::Prompts => prompt_exposed(
                &resolve_request_prompt_exposure_policy(
                    &config.name,
                    config.expose_prompts.clone(),
                ),
                &config.name,
                &reference,
            ),
            UpstreamCapability::Resources | UpstreamCapability::Tools => true,
        };
        if !exposed {
            return Err(format!(
                "completion reference is not exposed by upstream `{}`",
                config.name
            ));
        }
        let event = UpstreamRequestLog::completion(&config.name, &reference, true)
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        let (peer, _tools) = match self.acquire_or_connect_subject(config, subject).await {
            Ok(pair) => pair,
            Err(error) => {
                self.record_failure_for(
                    &config.name,
                    capability,
                    format!("upstream completion connect failed: {error}"),
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
            capability,
            event,
            start,
            peer.complete(params),
            estimate_complete_result_size,
            Some(subject),
            |error| format!("upstream completion failed: {error}"),
            format!("upstream completion timed out after {timeout_ms}ms"),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rmcp::model::{
        ArgumentInfo, ClientCapabilities, CompleteRequestParams, CompletionContext, Implementation,
        ProtocolVersion, Reference, RequestMetaObject,
    };

    use super::rewrite_completion_reference;
    use crate::upstream::types::UpstreamCapability;

    fn completion_request(reference: Reference) -> CompleteRequestParams {
        let mut request = CompleteRequestParams::new(reference, ArgumentInfo::new("value", "par"))
            .with_context(CompletionContext::with_arguments(HashMap::from([(
                "scope".to_string(),
                "system".to_string(),
            )])));
        let mut meta = RequestMetaObject::with_client_context(
            ProtocolVersion::V_2026_07_28,
            Implementation::new("completion-client", "1.0.0"),
            ClientCapabilities::default(),
        );
        meta.set_traceparent("00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01");
        request.meta = Some(meta);
        request
    }

    #[test]
    fn prompt_completion_rewrites_only_gateway_namespace() {
        let mut request = completion_request(Reference::for_prompt("alpha/command"));
        let original_meta = request.meta.clone();
        let original_context = request.context.clone();

        let (capability, reference) =
            rewrite_completion_reference(&mut request, "alpha").expect("prompt owner");

        assert_eq!(capability, UpstreamCapability::Prompts);
        assert_eq!(reference, "command");
        assert_eq!(request.r#ref.as_prompt_name(), Some("command"));
        assert_eq!(request.meta, original_meta);
        assert_eq!(request.context, original_context);
        assert_eq!(request.argument.value, "par");
    }

    #[test]
    fn resource_completion_preserves_metadata_and_context() {
        let mut request = completion_request(Reference::for_resource(
            "lab://upstream/alpha/file:///{path}",
        ));
        let original_meta = request.meta.clone();
        let original_context = request.context.clone();

        let (capability, reference) =
            rewrite_completion_reference(&mut request, "alpha").expect("resource owner");

        assert_eq!(capability, UpstreamCapability::Resources);
        assert_eq!(reference, "file:///{path}");
        assert_eq!(request.r#ref.as_resource_uri(), Some("file:///{path}"));
        assert_eq!(request.meta, original_meta);
        assert_eq!(request.context, original_context);
    }
}
