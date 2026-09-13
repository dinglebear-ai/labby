//! Model-actionable errors for MCP resource reads.

use labby_runtime::agent_error::{AgentErrorContext, AgentErrorOrigin, AgentSideEffectRisk};
use rmcp::ErrorData;
use serde_json::json;

use crate::mcp::agent_error::{
    internal as internal_agent_error, invalid_params as invalid_params_agent_error,
    resource_not_found as resource_not_found_agent_error,
};

fn context(uri: &str) -> AgentErrorContext {
    let mut context = AgentErrorContext::for_service_action("labby", "read_resource");
    context.resource = Some(uri.to_string());
    context
}

#[must_use]
pub(crate) fn unknown(uri: &str, ui: bool) -> ErrorData {
    let context = context(uri);
    let label = if ui {
        "unknown UI resource"
    } else {
        "unknown resource"
    };
    resource_not_found_agent_error(
        format!("{label}: {uri}. Call resources/list and retry with an advertised URI."),
        None,
        &context,
    )
}

#[must_use]
pub(crate) fn forbidden(uri: &str, message: &str, required_scopes: &[&str]) -> ErrorData {
    let mut context = context(uri);
    context.origin = Some(AgentErrorOrigin::Policy);
    context.side_effects = Some(AgentSideEffectRisk::NoneExpected);
    let extra = json!({ "required_scopes": required_scopes });
    invalid_params_agent_error("forbidden", message, Some(&extra), &context)
}

#[must_use]
pub(crate) fn route_scope(uri: &str, service: &str, message: &str) -> ErrorData {
    let mut context = context(uri);
    context.origin = Some(AgentErrorOrigin::Policy);
    context.side_effects = Some(AgentSideEffectRisk::NoneExpected);
    // `denied_service`, not `service`: the context's `service` field is
    // "labby" (the surface that denied the read); this key names the service
    // the caller asked for.
    let extra = json!({ "denied_service": service });
    invalid_params_agent_error("route_scope_denied", message, Some(&extra), &context)
}

#[must_use]
pub(crate) fn render(uri: &str, message: impl Into<String>) -> ErrorData {
    let message = message.into();
    let mut context = context(uri);
    context.cause = Some(labby_runtime::agent_error::sanitize_error_text(
        &message, 4096,
    ));
    internal_agent_error(
        "internal_error",
        format!("Failed to render resource `{uri}`."),
        None,
        &context,
    )
}

/// A read failure reported under its own stable kind.
///
/// Gateway cancellation, timeout, queue saturation and response-size failures
/// carry distinct recovery advice (docs/dev/ERRORS.md). The typed classifier
/// selects the kind; this renderer never includes upstream-authored detail.
#[must_use]
#[cfg(feature = "gateway")]
pub(crate) fn fetch_classified(uri: &str, kind: &'static str, summary: &str) -> ErrorData {
    let context = context(uri);
    internal_agent_error(kind, format!("Resource `{uri}` {summary}."), None, &context)
}

/// Map the gateway-owned failure variant without inspecting upstream-authored text.
#[must_use]
#[cfg(feature = "gateway")]
pub(crate) fn classify_fetch_failure(
    error: &crate::dispatch::upstream::pool::CapabilityCallError,
) -> (&'static str, &'static str) {
    use crate::dispatch::upstream::pool::CapabilityCallError;
    match error {
        CapabilityCallError::ResponseTooLarge { .. } => {
            ("response_too_large", "response exceeded the gateway cap")
        }
        CapabilityCallError::QueueSaturated { .. } => (
            "queue_saturated",
            "could not be admitted to the gateway queue",
        ),
        CapabilityCallError::Timeout { .. } => ("timeout", "read timed out"),
        CapabilityCallError::Cancelled { .. } => ("cancelled", "read was cancelled"),
        _ => ("upstream_error", "could not be fetched"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_resource_points_to_discovery() {
        let error = unknown("lab://missing", false);
        let data = error.data.expect("agent error data");
        assert_eq!(data["resource"], "lab://missing");
        assert_eq!(data["recovery"]["action"], "rediscover");
        assert!(error.message.contains("resources/list"));
    }

    #[test]
    fn route_scope_denial_names_denied_service_without_colliding() {
        let error = route_scope("lab://gateway/actions", "gateway", "not exposed");
        let data = error.data.expect("agent error data");
        // The context service (the denying surface) stays "labby"; the
        // requested service rides in `denied_service`.
        assert_eq!(data["service"], "labby");
        assert_eq!(data["denied_service"], "gateway");
        assert_eq!(data["origin"], "policy");
        assert_eq!(data["side_effects"], "none_expected");
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn fetch_failure_classification_uses_variants_and_redacts_upstream_detail() {
        use crate::dispatch::upstream::pool::CapabilityCallError;
        for message in ["cancelled", "timed out", "response too large"] {
            let error = CapabilityCallError::Mcp {
                data: ErrorData::invalid_params(message, None),
                message: format!("upstream resource read failed: {message}"),
            };
            let (kind, summary) = classify_fetch_failure(&error);
            assert_eq!(kind, "upstream_error");
            let rendered = fetch_classified("lab://upstream/alpha/item", kind, summary);
            assert_eq!(rendered.data.as_ref().unwrap()["kind"], "upstream_error");
            assert!(!rendered.message.contains(message));
        }
        for (error, expected_kind) in [
            (
                CapabilityCallError::ResponseTooLarge {
                    message: "opaque".into(),
                },
                "response_too_large",
            ),
            (
                CapabilityCallError::Timeout {
                    message: "opaque".into(),
                },
                "timeout",
            ),
            (
                CapabilityCallError::Cancelled {
                    message: "opaque".into(),
                },
                "cancelled",
            ),
            (
                CapabilityCallError::QueueSaturated {
                    message: "opaque".into(),
                },
                "queue_saturated",
            ),
            (
                CapabilityCallError::Other {
                    message: "cancelled timed out response too large".into(),
                },
                "upstream_error",
            ),
        ] {
            assert_eq!(classify_fetch_failure(&error).0, expected_kind);
        }
    }
}
