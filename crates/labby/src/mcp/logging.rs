use rmcp::RoleServer;
use rmcp::service::RequestContext;
use serde_json::json;

use super::server::LabMcpServer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoggingLevel {
    Info,
    Warning,
    Error,
    Emergency,
}

pub(crate) enum DispatchLogOutcome {
    Success,
    Failure {
        level: LoggingLevel,
        kind: &'static str,
    },
}

pub(crate) fn logging_level_rank(level: LoggingLevel) -> u8 {
    match level {
        LoggingLevel::Info => 0,
        LoggingLevel::Warning => 1,
        LoggingLevel::Error => 2,
        LoggingLevel::Emergency => 3,
    }
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

pub(crate) fn spawn_dispatch_notification(
    _peer: rmcp::service::Peer<RoleServer>,
    actor_key: Option<String>,
    service: String,
    action: String,
    elapsed_ms: u128,
    outcome: DispatchLogOutcome,
) {
    drop(notification_payload(
        &service,
        &action,
        elapsed_ms,
        outcome,
        actor_key.as_deref(),
    ));
    // The 2026-07-28 protocol removed logging/setLevel and only permits
    // notifications/message for requests carrying the per-request log-level
    // metadata. Labby does not emit protocol logging notifications.
}

impl LabMcpServer {
    pub(crate) fn should_emit_logging_notification(&self, level: LoggingLevel) -> bool {
        let _ = level;
        false
    }

    pub(crate) async fn emit_dispatch_notification(
        &self,
        context: &RequestContext<RoleServer>,
        service: &str,
        action: &str,
        elapsed_ms: u128,
        outcome: DispatchLogOutcome,
    ) {
        drop(notification_payload(
            service,
            action,
            elapsed_ms,
            outcome,
            super::context::actor_key_from_extensions(&context.extensions),
        ));
        // Structured observability remains on tracing. Protocol logging is
        // intentionally silent unless a future implementation honors the RC
        // request-scoped log-level metadata.
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
                kind: "upstream_error",
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
                kind: "internal_error",
            },
            None,
        );
        assert!(payload.get("error").is_none());
        assert!(payload.get("message").is_none());
    }
}
