//! `UpstreamEntry` constructors and exposure-policy resolution.
//!
//! These free functions build the catalog snapshot entries the pool stores for
//! lazy, healthy in-process, and failed upstreams, plus the `health_str`
//! classifier and the `resolve_exposure_policy` fail-closed helper. They are
//! `pub(super)` so the pool module and its descendants can call them.
//!
//! All three operator allowlists — `expose_tools`, `expose_resources`, and
//! `expose_prompts` — compile through the *same* fail-closed resolver
//! ([`resolve_named_exposure_policy`]). There is deliberately no second policy
//! implementation: an unparseable allowlist hides everything for that
//! capability rather than silently degrading to "expose all".

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types;
use super::super::types::{ToolExposurePolicy, UpstreamEntry, UpstreamHealth, UpstreamTool};

pub(super) fn health_str(health: UpstreamHealth) -> &'static str {
    match health {
        UpstreamHealth::Healthy => "healthy",
        UpstreamHealth::Unhealthy {
            consecutive_failures,
        } if consecutive_failures >= types::CIRCUIT_BREAKER_THRESHOLD => "open",
        UpstreamHealth::Unhealthy { .. } => "degraded",
    }
}

pub(super) fn lazy_upstream_entry(config: &UpstreamConfig, name: Arc<str>) -> UpstreamEntry {
    UpstreamEntry {
        name,
        tools: HashMap::new(),
        exposure_policy: resolve_exposure_policy(&config.name, config.expose_tools.clone()),
        resource_exposure_policy: resolve_resource_exposure_policy(
            &config.name,
            config.expose_resources.clone(),
        ),
        prompt_exposure_policy: resolve_prompt_exposure_policy(
            &config.name,
            config.expose_prompts.clone(),
        ),
        proxy_resources: config.proxy_resources,
        prompt_count: 0,
        resource_count: 0,
        prompt_names: Vec::new(),
        resource_uris: Vec::new(),
        tool_health: UpstreamHealth::Healthy,
        prompt_health: UpstreamHealth::Healthy,
        resource_health: UpstreamHealth::Healthy,
        tool_unhealthy_since: None,
        prompt_unhealthy_since: None,
        resource_unhealthy_since: None,
        tool_last_error: None,
        prompt_last_error: None,
        resource_last_error: None,
    }
}

pub(super) fn healthy_in_process_entry(
    name: Arc<str>,
    tools: HashMap<String, UpstreamTool>,
) -> UpstreamEntry {
    UpstreamEntry {
        name,
        tools,
        exposure_policy: ToolExposurePolicy::All,
        resource_exposure_policy: ToolExposurePolicy::All,
        prompt_exposure_policy: ToolExposurePolicy::All,
        proxy_resources: true,
        prompt_count: 0,
        resource_count: 0,
        prompt_names: Vec::new(),
        resource_uris: Vec::new(),
        tool_health: UpstreamHealth::Healthy,
        prompt_health: UpstreamHealth::Healthy,
        resource_health: UpstreamHealth::Healthy,
        tool_unhealthy_since: None,
        prompt_unhealthy_since: None,
        resource_unhealthy_since: None,
        tool_last_error: None,
        prompt_last_error: None,
        resource_last_error: None,
    }
}

pub(super) fn failed_in_process_entry(name: Arc<str>, error_message: String) -> UpstreamEntry {
    UpstreamEntry {
        name,
        tools: HashMap::new(),
        exposure_policy: ToolExposurePolicy::All,
        resource_exposure_policy: ToolExposurePolicy::All,
        prompt_exposure_policy: ToolExposurePolicy::All,
        proxy_resources: true,
        prompt_count: 0,
        resource_count: 0,
        prompt_names: Vec::new(),
        resource_uris: Vec::new(),
        tool_health: UpstreamHealth::Unhealthy {
            consecutive_failures: 1,
        },
        prompt_health: UpstreamHealth::Unhealthy {
            consecutive_failures: 1,
        },
        resource_health: UpstreamHealth::Unhealthy {
            consecutive_failures: 1,
        },
        tool_unhealthy_since: Some(Instant::now()),
        prompt_unhealthy_since: Some(Instant::now()),
        resource_unhealthy_since: Some(Instant::now()),
        tool_last_error: Some(error_message.clone()),
        prompt_last_error: Some(error_message.clone()),
        resource_last_error: Some(error_message),
    }
}

pub(super) fn failed_in_process_entry_from_existing(
    mut existing: UpstreamEntry,
    error_message: String,
) -> UpstreamEntry {
    existing.tool_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    existing.prompt_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    existing.resource_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    existing.tool_unhealthy_since = Some(Instant::now());
    existing.prompt_unhealthy_since = Some(Instant::now());
    existing.resource_unhealthy_since = Some(Instant::now());
    existing.tool_last_error = Some(error_message.clone());
    existing.prompt_last_error = Some(error_message.clone());
    existing.resource_last_error = Some(error_message);
    existing
}

/// Compile an upstream's `expose_tools` allowlist, failing closed.
///
/// A malformed allowlist hides every tool rather than exposing every tool: an
/// operator who mistyped a restriction must not silently get no restriction.
///
/// Use this from **catalog-build** paths (seeding, discovery, reprobe), which run
/// once per config change — the `warn!` is a per-config-defect event there. On
/// request paths use [`resolve_request_exposure_policy`] instead.
pub(super) fn resolve_exposure_policy(
    upstream_name: &str,
    expose_tools: Option<Vec<String>>,
) -> ToolExposurePolicy {
    resolve_named_exposure_policy(upstream_name, "expose_tools", "tools", expose_tools)
}

/// [`resolve_exposure_policy`] for paths that run once per request.
///
/// The OAuth subject-scoped paths have no cached `UpstreamEntry::exposure_policy`
/// to read, so they resolve the allowlist from live config on every `list_tools`,
/// every `tools/list_changed` diff, and every call. Warning there would turn one
/// static config typo into an unbounded WARN stream, and a config defect is not a
/// per-request caller error (see the level conventions in the root `CLAUDE.md`).
/// The fail-closed behavior is identical; only the log level differs.
pub fn resolve_request_exposure_policy(
    upstream_name: &str,
    expose_tools: Option<Vec<String>>,
) -> ToolExposurePolicy {
    resolve_request_named_exposure_policy(upstream_name, "expose_tools", "tools", expose_tools)
}

/// All three compiled allowlists for one upstream.
///
/// Resolved together from a single `UpstreamConfig` so a catalog entry can
/// never be built with one capability's policy applied and another's dropped —
/// the exact drift that left `expose_resources`/`expose_prompts` unenforced.
pub(super) struct UpstreamExposurePolicies {
    pub(super) tools: ToolExposurePolicy,
    pub(super) resources: ToolExposurePolicy,
    pub(super) prompts: ToolExposurePolicy,
}

pub(super) fn resolve_upstream_exposure_policies(
    config: &UpstreamConfig,
) -> UpstreamExposurePolicies {
    UpstreamExposurePolicies {
        tools: resolve_exposure_policy(&config.name, config.expose_tools.clone()),
        resources: resolve_resource_exposure_policy(&config.name, config.expose_resources.clone()),
        prompts: resolve_prompt_exposure_policy(&config.name, config.expose_prompts.clone()),
    }
}

/// Compile `expose_resources` into the shared exposure policy (catalog-build).
pub(super) fn resolve_resource_exposure_policy(
    upstream_name: &str,
    expose_resources: Option<Vec<String>>,
) -> ToolExposurePolicy {
    resolve_named_exposure_policy(
        upstream_name,
        "expose_resources",
        "resources",
        expose_resources,
    )
}

/// [`resolve_resource_exposure_policy`] for paths that run once per request.
///
/// Resource listing, direct reads, MRTR-relayed reads, and completion all
/// resolve from live config per request, so they need the same WARN-suppression
/// the tools request path has.
pub fn resolve_request_resource_exposure_policy(
    upstream_name: &str,
    expose_resources: Option<Vec<String>>,
) -> ToolExposurePolicy {
    resolve_request_named_exposure_policy(
        upstream_name,
        "expose_resources",
        "resources",
        expose_resources,
    )
}

/// Compile `expose_prompts` into the shared exposure policy (catalog-build).
pub(super) fn resolve_prompt_exposure_policy(
    upstream_name: &str,
    expose_prompts: Option<Vec<String>>,
) -> ToolExposurePolicy {
    resolve_named_exposure_policy(upstream_name, "expose_prompts", "prompts", expose_prompts)
}

/// [`resolve_prompt_exposure_policy`] for paths that run once per request.
pub fn resolve_request_prompt_exposure_policy(
    upstream_name: &str,
    expose_prompts: Option<Vec<String>>,
) -> ToolExposurePolicy {
    resolve_request_named_exposure_policy(
        upstream_name,
        "expose_prompts",
        "prompts",
        expose_prompts,
    )
}

/// The one fail-closed allowlist compiler shared by all three capabilities.
///
/// `field` names the operator-facing config key and `capability` names what
/// gets hidden — both are log-only. An invalid allowlist collapses to an empty
/// `AllowList`, i.e. *nothing* is exposed. Falling back to `All` here would
/// turn a typo into a silent, total loss of the restriction the operator asked
/// for, which is exactly the failure mode this helper exists to prevent.
///
/// Use this from **catalog-build** paths (seeding, discovery, reprobe), which run
/// once per config change. On request paths use
/// [`resolve_request_named_exposure_policy`] instead.
pub(super) fn resolve_named_exposure_policy(
    upstream_name: &str,
    field: &str,
    capability: &str,
    patterns: Option<Vec<String>>,
) -> ToolExposurePolicy {
    resolve_named_exposure_policy_inner(upstream_name, field, capability, patterns, true)
}

fn resolve_request_named_exposure_policy(
    upstream_name: &str,
    field: &str,
    capability: &str,
    patterns: Option<Vec<String>>,
) -> ToolExposurePolicy {
    resolve_named_exposure_policy_inner(upstream_name, field, capability, patterns, false)
}

fn resolve_named_exposure_policy_inner(
    upstream_name: &str,
    field: &str,
    capability: &str,
    patterns: Option<Vec<String>>,
    warn: bool,
) -> ToolExposurePolicy {
    match ToolExposurePolicy::from_optional(patterns) {
        Ok(policy) => policy,
        Err(error) => {
            if warn {
                tracing::warn!(
                    upstream = %upstream_name,
                    field,
                    capability,
                    error = %error,
                    "invalid upstream exposure policy; hiding all upstream {capability}"
                );
            } else {
                tracing::debug!(
                    upstream = %upstream_name,
                    field,
                    capability,
                    error = %error,
                    "invalid upstream exposure policy; hiding all upstream {capability}"
                );
            }
            ToolExposurePolicy::AllowList(Vec::new())
        }
    }
}

/// Whether one upstream prompt is exposed by `policy`.
///
/// `prompt_name` may arrive in either spelling: the bare name the upstream
/// itself advertises (`summarize`), or the `{upstream}/{name}` namespaced form
/// the gateway publishes downstream and shows in `gateway.discovered_prompts`
/// (`github/summarize`). Operators copy allowlist entries from either surface,
/// so both are accepted. This is a spelling accommodation, not a widening: the
/// pattern still has to match one of the two names for this one upstream.
pub fn prompt_exposed(policy: &ToolExposurePolicy, upstream_name: &str, prompt_name: &str) -> bool {
    if matches!(policy, ToolExposurePolicy::All) {
        return true;
    }
    let bare = super::helpers::bare_upstream_prompt_name(upstream_name, prompt_name);
    policy.matches(bare)
        || policy.matches(&super::helpers::prefixed_upstream_prompt_name(
            upstream_name,
            bare,
        ))
}

/// Whether one upstream resource is exposed by `policy`.
///
/// `resource_uri` must be the bare, upstream-native URI — the caller strips any
/// `lab://upstream/{name}/` gateway prefix first, so a `ui://` MCP App URI and
/// a plain `file:///…` URI are both matched in the form the operator sees in
/// `gateway.discovered_resources`.
pub fn resource_exposed(policy: &ToolExposurePolicy, resource_uri: &str) -> bool {
    policy.matches(resource_uri)
}

/// Emit the one-line exposure-filter observation shared by every filtered path.
///
/// Kept at `debug` (not `info`) because it fires on ordinary list traffic; the
/// point is that the filter is never silent when an operator is trying to work
/// out why an item disappeared. Mirrors the shape used for `expose_tools`.
pub(super) fn log_exposure_filter(
    upstream_name: &str,
    capability: &str,
    hidden_count: usize,
    exposed_count: usize,
    subject_scoped: bool,
) {
    if hidden_count == 0 {
        return;
    }
    tracing::debug!(
        surface = "dispatch",
        service = "upstream.pool",
        upstream = %upstream_name,
        capability,
        subject_scoped,
        hidden_count,
        exposed_count,
        "upstream {capability} hidden by exposure policy"
    );
}

#[cfg(test)]
mod tests {
    use super::super::testsupport::test_upstream_tools;
    use super::*;

    #[test]
    fn invalid_exposure_policy_fails_closed() {
        let policy = resolve_exposure_policy("github", Some(vec!["   ".to_string()]));
        assert_eq!(policy, ToolExposurePolicy::AllowList(Vec::new()));
        assert!(!policy.matches("search_repos"));
    }

    #[test]
    fn failed_in_process_entry_from_existing_preserves_last_known_good_catalog() {
        let upstream_name: Arc<str> = Arc::from("labby::github-chat");
        let tools = test_upstream_tools(&upstream_name, &["query_repository"]);
        let mut existing = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        existing.exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["query_repository".to_string()])
                .expect("policy");
        existing.prompt_count = 2;
        existing.resource_count = 3;
        existing.prompt_names = vec!["prompt.one".into(), "prompt.two".into()];
        existing.resource_uris = vec!["lab://resource/one".into(), "lab://resource/two".into()];

        let failed = failed_in_process_entry_from_existing(
            existing,
            "in-process peer registration timed out after 5s".to_string(),
        );

        assert_eq!(failed.tools.len(), 1);
        assert!(failed.tools.contains_key("query_repository"));
        assert_eq!(failed.prompt_count, 2);
        assert_eq!(failed.resource_count, 3);
        assert_eq!(failed.prompt_names.len(), 2);
        assert_eq!(failed.resource_uris.len(), 2);
        assert!(matches!(
            failed.exposure_policy,
            ToolExposurePolicy::AllowList(_)
        ));
        assert!(matches!(
            failed.tool_health,
            UpstreamHealth::Unhealthy {
                consecutive_failures: 1
            }
        ));
        assert_eq!(
            failed.tool_last_error.as_deref(),
            Some("in-process peer registration timed out after 5s")
        );
        assert_eq!(
            failed.prompt_last_error.as_deref(),
            Some("in-process peer registration timed out after 5s")
        );
        assert_eq!(
            failed.resource_last_error.as_deref(),
            Some("in-process peer registration timed out after 5s")
        );
    }
}
