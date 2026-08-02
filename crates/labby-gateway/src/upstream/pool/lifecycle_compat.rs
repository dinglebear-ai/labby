//! Compatibility negotiation for gateway-to-upstream MCP connections.
//!
//! Labby's downstream server remains on the current stateless lifecycle. This
//! module only handles independently versioned upstream servers.

use rmcp::model::{ProtocolVersion, ServerResult};
use rmcp::service::{ClientInitializeError, ClientLifecycleMode};

const DISCOVERY_SERVER_INFO_META_KEY: &str = "io.modelcontextprotocol/serverInfo";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LifecycleAttempt {
    Modern,
    LegacyInitialize,
}

impl LifecycleAttempt {
    pub(super) fn mode(self) -> ClientLifecycleMode {
        match self {
            // Modern first. Callers retry on a newly-created transport when a
            // peer rejects discovery or returns a discovery-shaped result that
            // the active SDK cannot decode; never reuse a partially-negotiated
            // stream for the initialize fallback.
            Self::Modern => ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
            Self::LegacyInitialize => ClientLifecycleMode::Initialize,
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Modern => "discover-2026",
            Self::LegacyInitialize => "initialize",
        }
    }
}

fn result_carries_discovery_server_info(result: &ServerResult) -> bool {
    let Ok(value) = serde_json::to_value(result) else {
        return false;
    };

    value
        .get("_meta")
        .and_then(|meta| meta.as_object())
        .is_some_and(|meta| meta.contains_key(DISCOVERY_SERVER_INFO_META_KEY))
}

fn discovery_response_was_misclassified(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let Some(ClientInitializeError::ExpectedInitResult(Some(result))) =
            cause.downcast_ref::<ClientInitializeError>()
        else {
            return false;
        };

        result_carries_discovery_server_info(result)
    })
}

/// Select a retry only when an error proves lifecycle incompatibility.
pub(super) fn compatibility_retry(error: &anyhow::Error) -> Option<LifecycleAttempt> {
    if discovery_response_was_misclassified(error) {
        return Some(LifecycleAttempt::LegacyInitialize);
    }

    let message = format!("{error:#}").to_ascii_lowercase();

    if message.contains("unsupported mcp-protocol-version")
        || message.contains("unsupported protocol version")
        || message.contains("method not found")
        || message.contains("unknown method")
        || (message.contains("-32601") && message.contains("server/discover"))
    {
        return Some(LifecycleAttempt::LegacyInitialize);
    }

    if message.contains("missing session id")
        || message.contains("no valid session id")
        || message.contains("expect initialize request")
        || message.contains("expected initialize request")
        || message.contains("connection closed: discover response")
        || (message.contains("server/discover")
            && (message.contains("invalid params")
                || message.contains("invalid request parameters")))
    {
        return Some(LifecycleAttempt::LegacyInitialize);
    }

    None
}

pub(super) fn log_fallback(
    upstream: &str,
    transport: &str,
    attempt: LifecycleAttempt,
    error: &anyhow::Error,
) {
    tracing::warn!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.lifecycle.fallback",
        kind = "upstream_lifecycle_incompatible",
        upstream,
        transport,
        from = LifecycleAttempt::Modern.label(),
        to = attempt.label(),
        reason = %error,
        "upstream is incompatible with the modern MCP lifecycle; retrying with compatibility negotiation"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_when_an_unexpected_result_carries_discovery_server_info() {
        let result = serde_json::from_value::<ServerResult>(serde_json::json!({
            "resultType": "complete",
            "supportedVersions": ["2026-07-28", "2025-11-25"],
            "capabilities": {"tools": {}},
            "ttlMs": 0,
            "cacheScope": "private",
            "_meta": {
                "io.modelcontextprotocol/serverInfo": {
                    "name": "modern-server",
                    "version": "1.0.0"
                }
            }
        }))
        .expect("unexpected result should deserialize through the SDK union");
        let error = anyhow::Error::new(ClientInitializeError::ExpectedInitResult(Some(result)));

        assert_eq!(
            compatibility_retry(&error),
            Some(LifecycleAttempt::LegacyInitialize)
        );
    }

    #[test]
    fn does_not_retry_an_unexpected_result_without_discovery_server_info() {
        let result = serde_json::from_value::<ServerResult>(serde_json::json!({
            "resultType": "complete",
            "_meta": {"traceId": "not-discovery"}
        }))
        .expect("tool-shaped result should deserialize");
        let error = anyhow::Error::new(ClientInitializeError::ExpectedInitResult(Some(result)));

        assert_eq!(compatibility_retry(&error), None);
    }

    #[test]
    fn retries_only_for_explicit_lifecycle_incompatibility() {
        for message in [
            "HTTP 400: Unsupported MCP-Protocol-Version: 2026-07-28",
            "JSON-RPC error: -32601: server/discover",
            "server/discover failed: No valid session ID provided",
            "server/discover: Invalid request parameters",
            "HTTP 422 Unprocessable Entity: Unexpected message, expect initialize request",
            "connection closed: discover response",
        ] {
            assert_eq!(
                compatibility_retry(&anyhow::anyhow!(message)),
                Some(LifecycleAttempt::LegacyInitialize)
            );
        }
    }

    #[test]
    fn does_not_downgrade_operational_or_authentication_failures() {
        for message in [
            "HTTP 401 Unauthorized",
            "HTTP 500 Internal Server Error",
            "connection timed out",
            "certificate verify failed",
        ] {
            assert_eq!(compatibility_retry(&anyhow::anyhow!(message)), None);
        }
    }
}
