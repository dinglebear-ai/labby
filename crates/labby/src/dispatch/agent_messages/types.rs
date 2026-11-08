use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::dispatch::codemode_notices::{NoticeConsumer, NoticeError, NoticeRecipient};

pub(crate) const MAX_MESSAGE_TOOL_BYTES: usize = 128 * 1024;
pub(super) const MAX_MESSAGE_BODY_BYTES: usize = 16_384;
pub(super) const RETENTION_MS: i64 = 86_400_000;
pub(super) const MAX_SESSIONS_PER_OWNER: i64 = 32;
pub(super) const MAX_SESSIONS: i64 = 512;
pub(super) const MAX_MESSAGES_PER_OWNER: i64 = 64;
pub(super) const MAX_MESSAGES: i64 = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionKind {
    Http,
    Stdio,
}

impl SessionKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Stdio => "stdio",
        }
    }

    pub(super) fn lease_ms(self) -> i64 {
        match self {
            Self::Http => 300_000,
            Self::Stdio => 120_000,
        }
    }
}

/// Constructed only from verified MCP transport authority, never request JSON.
#[derive(Clone)]
pub(crate) struct SessionBinding {
    pub(crate) owner_key: String,
    pub(crate) binding_key: String,
    pub(crate) recipient: NoticeRecipient,
    pub(crate) kind: SessionKind,
    pub(crate) stdio_closed: Option<Arc<AtomicBool>>,
}

impl SessionBinding {
    pub(super) fn validate(&self) -> Result<(), NoticeError> {
        let exact_recipient = match (&self.recipient.consumer, self.kind) {
            (NoticeConsumer::MessageSession { binding }, SessionKind::Http) => {
                binding == &self.binding_key
                    && self.recipient.actor.as_deref() == Some(self.owner_key.as_str())
                    && self.stdio_closed.is_none()
            }
            (NoticeConsumer::Stdio { .. }, SessionKind::Stdio) => {
                self.recipient.actor.is_none() && self.stdio_closed.is_some()
            }
            _ => false,
        };
        if !exact_recipient
            || self.recipient.route != "root"
            || !digest_key(&self.owner_key)
            || !digest_key(&self.binding_key)
        {
            return Err(NoticeError::Identity);
        }
        self.recipient.key()?;
        Ok(())
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.stdio_closed
            .as_ref()
            .is_some_and(|closed| closed.load(Ordering::Acquire))
    }

    pub(crate) fn close(&self) {
        if let Some(closed) = &self.stdio_closed {
            closed.store(true, Ordering::Release);
        }
    }
}

fn digest_key(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Identity of an already registered session, carried across callback verification.
#[derive(Clone)]
pub(crate) struct MessageEnrollment {
    pub(crate) session_id: String,
    pub(crate) inbox_id: String,
    pub(crate) binding: SessionBinding,
}

#[derive(Deserialize)]
#[serde(
    tag = "action",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum MessageRequest {
    Register(RegisterParams),
    Sessions(EmptyParams),
    Send(SendParams),
    Unread(UnreadParams),
    Read(ReadParams),
    Ack(AckParams),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisterParams {
    pub(super) label: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EmptyParams {}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum MessageTarget {
    Session { session_id: String },
    AllActive {},
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SendParams {
    pub(super) to: MessageTarget,
    pub(super) body: String,
    pub(super) summary: Option<String>,
    pub(super) idempotency_key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UnreadParams {
    #[serde(default = "default_unread_limit")]
    pub(super) limit: u32,
}

fn default_unread_limit() -> u32 {
    10
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadParams {
    pub(super) message_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AckParams {
    pub(super) message_ids: Vec<String>,
}

fn short_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn typed_id(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|id| id.len() == 26 && ulid::Ulid::from_string(id).is_ok())
}

impl MessageRequest {
    pub(super) fn validate(&self) -> Result<(), NoticeError> {
        let valid = match self {
            Self::Register(params) => short_text(&params.label, 64),
            Self::Sessions(_) => true,
            Self::Send(params) => {
                !params.body.trim().is_empty()
                    && params.body.len() <= MAX_MESSAGE_BODY_BYTES
                    && params
                        .body
                        .chars()
                        .all(|c| !c.is_control() || matches!(c, '\r' | '\n' | '\t'))
                    && params
                        .summary
                        .as_ref()
                        .is_none_or(|value| short_text(value, 160))
                    && short_text(&params.idempotency_key, 128)
                    && match &params.to {
                        MessageTarget::Session { session_id } => typed_id(session_id, "session_"),
                        MessageTarget::AllActive {} => true,
                    }
            }
            Self::Unread(params) => (1..=32).contains(&params.limit),
            Self::Read(params) => typed_id(&params.message_id, "message_"),
            Self::Ack(params) => {
                params.message_ids.len() <= 32
                    && params.message_ids.iter().all(|id| typed_id(id, "message_"))
            }
        };
        if valid {
            Ok(())
        } else {
            Err(NoticeError::Invalid)
        }
    }
}
