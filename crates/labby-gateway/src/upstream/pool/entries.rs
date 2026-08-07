//! `UpstreamEntry` constructors and exposure-policy resolution.
//!
//! These free functions build the catalog snapshot entries the pool stores for
//! lazy, healthy in-process, and failed upstreams, plus the `health_str`
//! classifier and the `resolve_exposure_policy` fail-closed helper. They are
//! `pub(super)` so the pool module and its descendants can call them.

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
    resolve_exposure_policy_inner(upstream_name, expose_tools, true)
}

/// [`resolve_exposure_policy`] for paths that run once per request.
///
/// The OAuth subject-scoped paths have no cached `UpstreamEntry::exposure_policy`
/// to read, so they resolve the allowlist from live config on every `list_tools`,
/// every `tools/list_changed` diff, and every call. Warning there would turn one
/// static config typo into an unbounded WARN stream, and a config defect is not a
/// per-request caller error (see the level conventions in the root `CLAUDE.md`).
/// The fail-closed behavior is identical; only the log level differs.
pub(super) fn resolve_request_exposure_policy(
    upstream_name: &str,
    expose_tools: Option<Vec<String>>,
) -> ToolExposurePolicy {
    resolve_exposure_policy_inner(upstream_name, expose_tools, false)
}

fn resolve_exposure_policy_inner(
    upstream_name: &str,
    expose_tools: Option<Vec<String>>,
    warn: bool,
) -> ToolExposurePolicy {
    match ToolExposurePolicy::from_optional(expose_tools) {
        Ok(policy) => policy,
        Err(error) => {
            if warn {
                tracing::warn!(
                    upstream = %upstream_name,
                    error = %error,
                    "invalid upstream exposure policy; hiding all upstream tools"
                );
            } else {
                tracing::debug!(
                    upstream = %upstream_name,
                    error = %error,
                    "invalid upstream exposure policy; hiding all upstream tools"
                );
            }
            ToolExposurePolicy::AllowList(Vec::new())
        }
    }
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
