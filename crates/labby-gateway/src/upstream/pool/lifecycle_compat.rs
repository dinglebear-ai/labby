//! Compatibility negotiation for gateway-to-upstream MCP connections.
//!
//! Labby's downstream server remains on the current stateless lifecycle. This
//! module only handles independently versioned upstream servers.

use rmcp::model::{ErrorCode, ProtocolVersion, ServerResult};
use rmcp::service::{ClientInitializeError, ClientLifecycleMode};

const DISCOVERY_SERVER_INFO_META_KEY: &str = "io.modelcontextprotocol/serverInfo";

pub(super) fn legacy_protocol_version() -> ProtocolVersion {
    ProtocolVersion::V_2025_11_25
}

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

fn no_compatible_protocol_version(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<ClientInitializeError>(),
            Some(ClientInitializeError::NoCompatibleProtocolVersion { .. })
        )
    })
}

fn modern_discovery_error_must_not_downgrade(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let Some(ClientInitializeError::JsonRpcError(error)) =
            cause.downcast_ref::<ClientInitializeError>()
        else {
            return false;
        };
        error.code == ErrorCode::HEADER_MISMATCH
            || error.code == ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY
            || error.code == ErrorCode::UNSUPPORTED_PROTOCOL_VERSION
    })
}

/// Which kind of transport produced a connect error.
///
/// The distinction matters for one signal only: a discovery stream that closed
/// before any response arrived. rmcp's streamable HTTP worker does not surface
/// a JSON-RPC *error* frame received over SSE during the initialize phase; it
/// keeps draining for a success response and then quits with an empty stream.
/// A legacy HTTP server's `-32601` for `server/discover` is therefore only ever
/// observable as `connection closed: discover response`, so on network
/// transports that message is treated as legacy evidence. On stdio the same
/// message means the child closed stdout, which proves nothing about the
/// lifecycle it speaks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LifecycleTransport {
    /// Streamable HTTP, Unix-socket HTTP, or WebSocket.
    Network,
    /// A child process speaking MCP over its stdio pipes.
    Stdio,
}

/// Select a retry only when an error proves lifecycle incompatibility.
pub(super) fn compatibility_retry(
    error: &anyhow::Error,
    transport: LifecycleTransport,
) -> Option<LifecycleAttempt> {
    if modern_discovery_error_must_not_downgrade(error) {
        return None;
    }
    if discovery_response_was_misclassified(error) || no_compatible_protocol_version(error) {
        return Some(LifecycleAttempt::LegacyInitialize);
    }

    let message = format!("{error:#}").to_ascii_lowercase();

    if message.contains("unsupported mcp-protocol-version")
        || message.contains("unsupported protocol version")
        || message.contains("method not found")
        || message.contains("method not supported")
        || message.contains("unknown method")
        || (message.contains("-32601") && message.contains("server/discover"))
    {
        return Some(LifecycleAttempt::LegacyInitialize);
    }

    if message.contains("missing session id")
        || message.contains("no valid session id")
        || message.contains("expect initialize request")
        || message.contains("expected initialize request")
        || message.contains("invalid params")
        || message.contains("invalid request parameters")
    {
        return Some(LifecycleAttempt::LegacyInitialize);
    }

    // See `LifecycleTransport`: only a network peer's closed discovery stream
    // carries a swallowed legacy rejection.
    if transport == LifecycleTransport::Network
        && message.contains("connection closed: discover response")
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

        for transport in [LifecycleTransport::Network, LifecycleTransport::Stdio] {
            assert_eq!(
                compatibility_retry(&error, transport),
                Some(LifecycleAttempt::LegacyInitialize)
            );
        }
    }

    #[test]
    fn does_not_retry_an_unexpected_result_without_discovery_server_info() {
        let result = serde_json::from_value::<ServerResult>(serde_json::json!({
            "resultType": "complete",
            "_meta": {"traceId": "not-discovery"}
        }))
        .expect("tool-shaped result should deserialize");
        let error = anyhow::Error::new(ClientInitializeError::ExpectedInitResult(Some(result)));

        for transport in [LifecycleTransport::Network, LifecycleTransport::Stdio] {
            assert_eq!(compatibility_retry(&error, transport), None);
        }
    }

    #[test]
    fn retries_only_for_explicit_lifecycle_incompatibility() {
        for message in [
            "HTTP 400: Unsupported MCP-Protocol-Version: 2026-07-28",
            "JSON-RPC error: -32601: server/discover",
            "server/discover failed: No valid session ID provided",
            "JSON-RPC error: -32601: server/discover",
            "server/discover: Invalid request parameters",
            "JSON-RPC error: -32602: Invalid request parameters(\"\")",
            "JSON-RPC error: -32601: Method not supported",
            "HTTP 422 Unprocessable Entity: Unexpected message, expect initialize request",
        ] {
            for transport in [LifecycleTransport::Network, LifecycleTransport::Stdio] {
                assert_eq!(
                    compatibility_retry(&anyhow::anyhow!(message), transport),
                    Some(LifecycleAttempt::LegacyInitialize),
                    "{message} over {transport:?}"
                );
            }
        }
    }

    #[test]
    fn closed_discovery_stream_is_legacy_evidence_only_on_network_transports() {
        let error = anyhow::anyhow!("connection closed: discover response");
        assert_eq!(
            compatibility_retry(&error, LifecycleTransport::Network),
            Some(LifecycleAttempt::LegacyInitialize),
            "rmcp swallows a legacy server's SSE error frame during initialize"
        );
        assert_eq!(
            compatibility_retry(&error, LifecycleTransport::Stdio),
            None,
            "a stdio child that closed stdout proved nothing about its lifecycle"
        );
    }

    #[test]
    fn retries_when_discovery_has_no_mutually_supported_version() {
        let error = anyhow::Error::new(ClientInitializeError::NoCompatibleProtocolVersion {
            client_supported: vec![ProtocolVersion::V_2026_07_28],
            server_supported: vec![ProtocolVersion::V_2025_11_25],
        });
        for transport in [LifecycleTransport::Network, LifecycleTransport::Stdio] {
            assert_eq!(
                compatibility_retry(&error, transport),
                Some(LifecycleAttempt::LegacyInitialize)
            );
        }
    }

    #[test]
    fn does_not_downgrade_modern_protocol_contract_errors() {
        for code in [
            ErrorCode::HEADER_MISMATCH,
            ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY,
            ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
        ] {
            let error = anyhow::Error::new(ClientInitializeError::JsonRpcError(
                rmcp::model::ErrorData::new(code, "modern protocol contract error", None),
            ));
            for transport in [LifecycleTransport::Network, LifecycleTransport::Stdio] {
                assert_eq!(compatibility_retry(&error, transport), None);
            }
        }
    }

    #[test]
    fn stdio_error_wrapper_preserves_typed_modern_protocol_rejection() {
        let wrapped = super::super::stdio_stderr::StdioConnectError::without_diagnostics(
            ClientInitializeError::JsonRpcError(rmcp::model::ErrorData::new(
                ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
                "unsupported protocol version",
                None,
            )),
        );

        assert_eq!(
            compatibility_retry(wrapped.protocol_error(), LifecycleTransport::Stdio),
            None,
            "stdio diagnostics must not erase typed fail-closed protocol errors"
        );
    }

    #[test]
    fn does_not_downgrade_operational_or_authentication_failures() {
        for message in [
            "HTTP 401 Unauthorized",
            "HTTP 500 Internal Server Error",
            "connection timed out",
            "certificate verify failed",
            "connection closed: initialize response",
        ] {
            for transport in [LifecycleTransport::Network, LifecycleTransport::Stdio] {
                assert_eq!(
                    compatibility_retry(&anyhow::anyhow!(message), transport),
                    None,
                    "{message} over {transport:?}"
                );
            }
        }
    }
}