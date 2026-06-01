//! `UpstreamEntry` constructors and exposure-policy resolution.
//!
//! These free functions build the catalog snapshot entries the pool stores for
//! lazy, healthy in-process, and failed upstreams, plus the `health_str`
//! classifier and the `resolve_exposure_policy` fail-closed helper. They are
//! `pub(super)` so the pool module and its descendants can call them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::config::UpstreamConfig;

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

pub(super) fn resolve_exposure_policy(
    upstream_name: &str,
    expose_tools: Option<Vec<String>>,
) -> ToolExposurePolicy {
    match ToolExposurePolicy::from_optional(expose_tools) {
        Ok(policy) => policy,
        Err(error) => {
            tracing::warn!(
                upstream = %upstream_name,
                error = %error,
                "invalid upstream exposure policy; hiding all upstream tools"
            );
            ToolExposurePolicy::AllowList(Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_exposure_policy_fails_closed() {
        let policy = resolve_exposure_policy("github", Some(vec!["   ".to_string()]));
        assert_eq!(policy, ToolExposurePolicy::AllowList(Vec::new()));
        assert!(!policy.matches("search_repos"));
    }

    // `failed_in_process_entry_from_existing_preserves_last_known_good_catalog`
    // is relocated here in the testsupport step (lab-kvji.12.5 / step 8): it
    // depends on the `test_upstream_tools` fixture which does not exist as a
    // shared module until then. Kept in the pool.rs test mod until that step to
    // keep every intermediate build green without duplicating the fixture.
}
