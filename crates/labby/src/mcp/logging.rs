use std::sync::atomic::Ordering;

use rmcp::RoleServer;
use rmcp::service::{Peer, RequestContext};
use serde_json::json;

use super::server::LabMcpServer;

// rmcp 2.1 deprecates the legacy logging capability under SEP-2577, but the
// server still supports it for clients that have not moved to protocol logging.
#[allow(deprecated)]
use rmcp::model::LoggingLevel as RmcpLoggingLevel;
#[allow(deprecated)]
use rmcp::model::LoggingMessageNotificationParam;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoggingLevel {
    Debug,
    Info,
    Notice,
    Warning,
    Error,
    Critical,
    Alert,
    Emergency,
}

impl LoggingLevel {
    #[allow(deprecated)]
    pub(crate) fn from_rmcp(level: RmcpLoggingLevel) -> Self {
        match level {
            RmcpLoggingLevel::Debug => Self::Debug,
            RmcpLoggingLevel::Info => Self::Info,
            RmcpLoggingLevel::Notice => Self::Notice,
            RmcpLoggingLevel::Warning => Self::Warning,
            RmcpLoggingLevel::Error => Self::Error,
            RmcpLoggingLevel::Critical => Self::Critical,
            RmcpLoggingLevel::Alert => Self::Alert,
            RmcpLoggingLevel::Emergency => Self::Emergency,
        }
    }

    #[allow(deprecated)]
    fn to_rmcp(self) -> RmcpLoggingLevel {
        match self {
            Self::Debug => RmcpLoggingLevel::Debug,
            Self::Info => RmcpLoggingLevel::Info,
            Self::Notice => RmcpLoggingLevel::Notice,
            Self::Warning => RmcpLoggingLevel::Warning,
            Self::Error => RmcpLoggingLevel::Error,
            Self::Critical => RmcpLoggingLevel::Critical,
            Self::Alert => RmcpLoggingLevel::Alert,
            Self::Emergency => RmcpLoggingLevel::Emergency,
        }
    }
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
        LoggingLevel::Debug => 0,
        LoggingLevel::Info => 1,
        LoggingLevel::Notice => 2,
        LoggingLevel::Warning => 3,
        LoggingLevel::Error => 4,
        LoggingLevel::Critical => 5,
        LoggingLevel::Alert => 6,
        LoggingLevel::Emergency => 7,
    }
}

pub(crate) fn decode_logging_level(value: u8) -> LoggingLevel {
    match value {
        0 => LoggingLevel::Debug,
        1 => LoggingLevel::Info,
        2 => LoggingLevel::Notice,
        3 => LoggingLevel::Warning,
        4 => LoggingLevel::Error,
        5 => LoggingLevel::Critical,
        6 => LoggingLevel::Alert,
        _ => LoggingLevel::Emergency,
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

#[allow(deprecated)]
pub(crate) fn spawn_dispatch_notification(
    peer: Peer<RoleServer>,
    actor_key: Option<String>,
    service: String,
    action: String,
    elapsed_ms: u128,
    outcome: DispatchLogOutcome,
) {
    let (level, mut payload) =
        notification_payload(&service, &action, elapsed_ms, outcome, actor_key.as_deref());
    tokio::spawn(async move {
        if let Err(error) = peer
            .notify_logging_message(
                LoggingMessageNotificationParam::new(level.to_rmcp(), payload.take())
                    .with_logger("lab.mcp.dispatch"),
            )
            .await
        {
            tracing::debug!(
                surface = "mcp",
                service = %service,
                action = %action,
                level = ?level,
                error = %error,
                "failed to send rmcp logging notification"
            );
        }
    });
}

impl LabMcpServer {
    pub(crate) fn current_logging_level(&self) -> LoggingLevel {
        decode_logging_level(self.logging_level.load(Ordering::Relaxed))
    }

    pub(crate) fn should_emit_logging_notification(&self, level: LoggingLevel) -> bool {
        logging_level_rank(level) >= logging_level_rank(self.current_logging_level())
    }

    #[allow(deprecated)]
    pub(crate) async fn emit_dispatch_notification(
        &self,
        context: &RequestContext<RoleServer>,
        service: &str,
        action: &str,
        elapsed_ms: u128,
        outcome: DispatchLogOutcome,
    ) {
        let actor_key = super::context::actor_key_from_extensions(&context.extensions);
        let (level, payload) =
            notification_payload(service, action, elapsed_ms, outcome, actor_key);

        if !self.should_emit_logging_notification(level) {
            return;
        }

        if let Err(error) = context
            .peer
            .notify_logging_message(
                LoggingMessageNotificationParam::new(level.to_rmcp(), payload)
                    .with_logger("lab.mcp.dispatch"),
            )
            .await
        {
            tracing::debug!(
                surface = "mcp",
                service,
                action,
                level = ?level,
                error = %error,
                "failed to send rmcp logging notification"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_logging_level, logging_level_rank, notification_payload};
    use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel};

    #[test]
    fn logging_level_encoding_round_trips() {
        for level in [
            LoggingLevel::Debug,
            LoggingLevel::Info,
            LoggingLevel::Notice,
            LoggingLevel::Warning,
            LoggingLevel::Error,
            LoggingLevel::Critical,
            LoggingLevel::Alert,
            LoggingLevel::Emergency,
        ] {
            assert_eq!(decode_logging_level(logging_level_rank(level)), level);
        }
    }

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
