//! Compatibility negotiation for gateway-to-upstream MCP connections.
//!
//! Labby's downstream server remains on the current stateless lifecycle. This
//! module only handles independently versioned upstream servers.

use rmcp::service::ClientLifecycleMode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LifecycleAttempt {
    Modern,
    LegacyInitialize,
}

impl LifecycleAttempt {
    pub(super) fn mode(self) -> ClientLifecycleMode {
        match self {
            // Labby's configured upstream fleet is legacy-only. Keep the
            // compatibility boundary deterministic: probing server/discover
            // first leaves legacy streamable transports in inconsistent state
            // and makes a later initialize retry unreliable.
            Self::Modern => ClientLifecycleMode::Initialize,
            Self::LegacyInitialize => ClientLifecycleMode::Initialize,
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Modern => "initialize-legacy",
            Self::LegacyInitialize => "initialize",
        }
    }
}

/// Select a retry only when an error proves lifecycle incompatibility.
pub(super) fn compatibility_retry(error: &anyhow::Error) -> Option<LifecycleAttempt> {
    let message = format!("{error:#}").to_ascii_lowercase();

    if message.contains("unsupported mcp-protocol-version")
        || message.contains("unsupported protocol version")
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
        "upstream rejected the modern MCP lifecycle; retrying with compatibility negotiation"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_only_for_explicit_lifecycle_incompatibility() {
        for message in [
            "HTTP 400: Unsupported MCP-Protocol-Version: 2026-07-28",
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
