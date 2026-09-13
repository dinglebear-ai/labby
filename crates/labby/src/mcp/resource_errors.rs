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
/// `fetch` flattens every cause to `upstream_error`. `cancelled`, `timeout`,
/// and `response_too_large` are distinct documented kinds with different
/// recovery advice (docs/dev/ERRORS.md), so a caller that can act on the
/// difference must be able to see it.
#[must_use]
#[cfg(feature = "gateway")]
pub(crate) fn fetch_classified(uri: &str, kind: &'static str, summary: &str) -> ErrorData {
    let context = context(uri);
    internal_agent_error(kind, format!("Resource `{uri}` {summary}."), None, &context)
}

/// Classify the bounded string form returned by the legacy upstream resource
/// pool without copying upstream-authored detail into the model-facing error.
///
/// The pool has typed errors internally, but its established resource proxy
/// API predates them and returns `String`. Keep this compatibility classifier
/// deliberately narrow: only Labby's fixed timeout/cancellation/size phrases
/// refine the kind; every upstream-authored application or transport failure
/// remains the conservative `upstream_error`.
#[must_use]
#[cfg(feature = "gateway")]
pub(crate) fn classify_fetch_failure(message: &str) -> (&'static str, &'static str) {
    const CLASSIFY_PREFIX_BYTES: usize = 1024;
    let mut end = message.len().min(CLASSIFY_PREFIX_BYTES);
    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = message[..end].to_ascii_lowercase();
    if prefix.contains("response too large") {
        ("response_too_large", "response exceeded the gateway cap")
    } else if prefix.contains("timed out") {
        ("timeout", "read timed out")
    } else if prefix.contains("cancelled") {
        ("cancelled", "read was cancelled")
    } else {
        ("upstream_error", "could not be fetched")
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
    fn fetch_failure_classification_is_bounded_and_does_not_expose_detail() {
        for (message, expected_kind) in [
            (
                "upstream response too large (11 bytes, max 10)",
                "response_too_large",
            ),
            ("upstream resource read timed out after 25ms", "timeout"),
            ("downstream request cancelled while queued", "cancelled"),
            (
                "Mcp error: -32602: private fixture detail",
                "upstream_error",
            ),
        ] {
            let (kind, summary) = classify_fetch_failure(message);
            assert_eq!(kind, expected_kind);
            let error = fetch_classified("lab://upstream/alpha/item", kind, summary);
            assert_eq!(error.data.as_ref().unwrap()["kind"], expected_kind);
            assert!(!error.to_string().contains("private fixture detail"));
        }

        let hostile = format!("opaque failure {}timed out", "x".repeat(2048));
        assert_eq!(
            classify_fetch_failure(&hostile).0,
            "upstream_error",
            "classification must inspect only the bounded prefix"
        );
    }
}
