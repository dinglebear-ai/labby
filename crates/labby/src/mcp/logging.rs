use rmcp::RoleServer;
use rmcp::service::RequestContext;
use serde_json::json;

use super::server::LabMcpServer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoggingLevel {
    Info,
    Warning,
    Error,
}

pub(crate) enum DispatchLogOutcome {
    Success,
    Failure {
        level: LoggingLevel,
        kind: std::borrow::Cow<'static, str>,
    },
}

fn notification_payload(
    service: &str,
    action: &str,
    elapsed_ms: u128,
    outcome: DispatchLogOutcome,
    actor_key: Option<&str>,
) -> (LoggingLevel, serde_json::Value) {
    let (level, kind) = match outcome {
        DispatchLogOutcome::Success => (LoggingLevel::Info, None),
        DispatchLogOutcome::Failure { level, kind } => (level, Some(kind)),
    };

    let mut payload = json!({
        "surface": "mcp",
        "service": service,
        "action": action,
        "elapsed_ms": elapsed_ms,
    });
    if let Some(kind) = kind {
        payload["kind"] = json!(kind);
    }
    if let Some(actor_key) = actor_key {
        payload["actor_key"] = json!(actor_key);
    }

    (level, payload)
}

impl LabMcpServer {
    pub(crate) fn trace_dispatch_outcome(
        &self,
        context: &RequestContext<RoleServer>,
        service: &str,
        action: &str,
        elapsed_ms: u128,
        outcome: DispatchLogOutcome,
    ) {
        if tracing::enabled!(tracing::Level::TRACE) {
            let (level, fields) = notification_payload(
                service,
                action,
                elapsed_ms,
                outcome,
                super::context::actor_key_from_extensions(&context.extensions),
            );
            tracing::trace!(?level, %fields, "MCP dispatch outcome");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::notification_payload;
    use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel};

    #[test]
    fn notification_payload_omits_kind_for_success() {
        let (level, payload) = notification_payload(
            "lab",
            "list_resources",
            12,
            DispatchLogOutcome::Success,
            None,
        );
        assert_eq!(level, LoggingLevel::Info);
        assert_eq!(payload["surface"], "mcp");
        assert_eq!(payload["service"], "lab");
        assert_eq!(payload["action"], "list_resources");
        assert_eq!(payload["elapsed_ms"], 12);
        assert!(payload.get("kind").is_none());
    }

    #[test]
    fn notification_payload_preserves_failure_level_and_kind() {
        let (level, payload) = notification_payload(
            "lab",
            "call_tool",
            44,
            DispatchLogOutcome::Failure {
                level: LoggingLevel::Error,
                kind: "upstream_error".into(),
            },
            Some("actor-fixture"),
        );
        assert_eq!(level, LoggingLevel::Error);
        assert_eq!(payload["kind"], "upstream_error");
        assert_eq!(payload["actor_key"], "actor-fixture");
    }

    #[test]
    fn notification_payload_does_not_include_raw_error_message() {
        let (_level, payload) = notification_payload(
            "lab",
            "call_tool",
            44,
            DispatchLogOutcome::Failure {
                level: LoggingLevel::Error,
                kind: "internal_error".into(),
            },
            None,
        );
        assert!(payload.get("error").is_none());
        assert!(payload.get("message").is_none());
    }
}
