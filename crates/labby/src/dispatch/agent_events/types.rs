use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use url::Url;
use zeroize::Zeroizing;

use super::super::{
    agent_messages::MessageEnrollment,
    codemode_notices::{NoticeError, NoticeRecipient},
};

pub(crate) const EVENT_NAME: &str = "labby.notification.created";
pub(super) const MAX_SUBSCRIPTIONS: usize = 2_000;
pub(super) const MAX_PER_INBOX: i64 = 8;
pub(super) const ROTATION_MS: i64 = 300_000;
pub(super) const DEFAULT_TTL_MS: i64 = 3_600_000;
pub(super) const MAX_TTL_MS: i64 = 86_400_000;

#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum EventError {
    #[error(transparent)]
    Store(#[from] NoticeError),
    #[error("invalid event subscription parameters")]
    Invalid,
    #[error("only webhook event delivery is supported")]
    UnsupportedDelivery,
    #[error("callback could not be verified")]
    Callback(&'static str),
}

#[derive(Clone, Debug)]
pub(super) struct EventOwnerBinding {
    pub recipient: NoticeRecipient,
    pub authority_key: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EventFilter {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_source"
    )]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub session_messages: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn present_source<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}

impl EventFilter {
    pub fn validate(&self) -> Result<(), EventError> {
        if self.source.as_ref().is_some_and(|source| {
            source.is_empty() || source.len() > 64 || source.chars().any(char::is_control)
        }) {
            return Err(EventError::Invalid);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct SubscriptionTarget {
    pub url: Url,
    pub filter: EventFilter,
}

impl SubscriptionTarget {
    pub fn new(name: &str, filter: EventFilter, mode: &str, url: &str) -> Result<Self, EventError> {
        if name != EVENT_NAME {
            return Err(EventError::Invalid);
        }
        if mode != "webhook" {
            return Err(EventError::UnsupportedDelivery);
        }
        filter.validate()?;
        let url = parse_callback(url)?;
        Ok(Self { url, filter })
    }

    pub fn id(&self, recipient: &NoticeRecipient) -> Result<String, NoticeError> {
        let identity = serde_json::to_vec(&(
            "labby.event.subscription.v1",
            recipient.key()?,
            EVENT_NAME,
            self.url.as_str(),
            &self.filter,
        ))
        .map_err(|_| NoticeError::Invalid)?;
        Ok(format!("sub_{}", hex::encode(Sha256::digest(identity))))
    }
}

pub(super) fn parse_callback(raw: &str) -> Result<Url, EventError> {
    if raw.len() > 2_048 || raw.trim() != raw || raw.chars().any(char::is_control) {
        return Err(EventError::Callback("invalid_url"));
    }
    let url = Url::parse(raw).map_err(|_| EventError::Callback("invalid_url"))?;
    // Webhook receivers may carry a query token. Apply the canonical SSRF
    // validation to the same URL without the query, and never log either form.
    // Reject empty userinfo too (the URL parser canonicalizes it away).
    if raw.split_once("://").is_none_or(|(_, tail)| {
        tail.split(['/', '?', '#', '\\'])
            .next()
            .is_none_or(|host| host.is_empty() || host.contains('@'))
    }) {
        return Err(EventError::Callback("invalid_url"));
    }
    let mut target = url.clone();
    target.set_query(None);
    match labby_primitives::ssrf::parse_validated_https_url(target.as_str()) {
        Ok(_) => {}
        Err(labby_primitives::ssrf::SsrfError::Blocked(_)) => {
            return Err(EventError::Callback("destination_blocked"));
        }
        Err(_) => return Err(EventError::Callback("invalid_url")),
    }
    if let Some(url::Host::Domain(host)) = url.host() {
        labby_primitives::ssrf::check_host_not_private(host.trim_end_matches('.'))
            .map_err(|_| EventError::Callback("destination_blocked"))?;
    }
    if url.as_str().len() > 2_048 {
        return Err(EventError::Callback("invalid_url"));
    }
    Ok(url)
}

#[derive(Clone)]
pub(super) struct WebhookSecret(Zeroizing<Vec<u8>>);

impl std::fmt::Debug for WebhookSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebhookSecret(<redacted>)")
    }
}

impl WebhookSecret {
    pub fn parse(input: &str) -> Result<Self, EventError> {
        let encoded = input
            .strip_prefix("whsec_")
            .filter(|s| s.len() <= 88)
            .ok_or(EventError::Invalid)?;
        let decoded = STANDARD
            .decode(encoded)
            .or_else(|_| STANDARD_NO_PAD.decode(encoded))
            .map_err(|_| EventError::Invalid)?;
        Self::from_bytes(decoded)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, EventError> {
        let bytes = Zeroizing::new(bytes);
        if !(24..=64).contains(&bytes.len()) {
            return Err(EventError::Invalid);
        }
        Ok(Self(bytes))
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn fingerprint(&self) -> String {
        hex::encode(Sha256::digest(self.bytes()))
    }
}

pub(super) struct ValidatedSubscription {
    pub target: SubscriptionTarget,
    pub enrollment: Option<MessageEnrollment>,
    pub sealed_key: Vec<u8>,
    pub key_fingerprint: String,
    pub expires: i64,
}

#[derive(Debug)]
pub(super) struct EventSubscription {
    pub id: String,
    pub inbox_id: String,
    pub expires: i64,
    pub replaced_authority: Option<String>,
}

pub(super) fn timestamp(ms: i64) -> Result<String, NoticeError> {
    jiff::Timestamp::from_millisecond(ms)
        .map(|instant| instant.to_string())
        .map_err(|_| NoticeError::Invalid)
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventsListParams {
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default, rename = "_meta")]
    _meta: Option<rmcp::model::JsonObject>,
}

impl EventsListParams {
    pub(crate) fn list(self, session_messages: bool) -> Result<serde_json::Value, EventError> {
        if self.cursor.is_some() {
            return Err(EventError::Invalid);
        }
        let mut event = serde_json::json!({
            "name": EVENT_NAME,
            "description": "An advisory notice is published to your Labby inbox.",
            "delivery": ["webhook"],
            "inputSchema": {
                "type": "object", "additionalProperties": false,
                "properties": {"source": {"type": "string", "minLength": 1, "maxLength": 64,
                    "description": "Only deliver notices with this exact source."}}
            },
            "payloadSchema": {
                "type": "object", "additionalProperties": false,
                "required": ["id","source","level","message","expires_at_unix_ms"],
                "properties": {"id":{"type":"string"},"source":{"type":"string"},
                    "level":{"enum":["info","warning"]},"message":{"type":"string"},
                    "expires_at_unix_ms":{"type":"integer"}}
            }
        });
        if session_messages {
            event["inputSchema"]["properties"]["session_messages"] = serde_json::json!({
                "type":"boolean",
                "description":"Deliver only references for this registered message session.",
                "default":false,
            });
            event["payloadSchema"]["properties"]["message_id"] = serde_json::json!({
                "type":"string",
                "description":"Read the full message through the messages tool with this session's proof.",
            });
        }
        Ok(serde_json::json!({"events":[event]}))
    }
}

#[derive(Default)]
enum RequestedTtl {
    #[default]
    Default,
    Millis(u64),
    NoExpiryRequested,
}

impl<'de> Deserialize<'de> for RequestedTtl {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Option::<u64>::deserialize(d).map(|v| v.map_or(Self::NoExpiryRequested, Self::Millis))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubscribeDelivery {
    mode: String,
    url: String,
    #[serde(deserialize_with = "read_secret")]
    secret: Zeroizing<String>,
}

fn read_secret<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Zeroizing<String>, D::Error> {
    String::deserialize(d).map(Zeroizing::new)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventsSubscribeParams {
    name: String,
    #[serde(default)]
    arguments: EventFilter,
    delivery: SubscribeDelivery,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default, rename = "ttlMs")]
    ttl: RequestedTtl,
    #[serde(default, rename = "_meta")]
    _meta: Option<rmcp::model::JsonObject>,
}

impl EventsSubscribeParams {
    pub(crate) fn session_messages(&self) -> bool {
        self.arguments.session_messages
    }

    pub(super) fn validate(self) -> Result<(SubscriptionTarget, WebhookSecret, i64), EventError> {
        if self.cursor.is_some() {
            return Err(EventError::Invalid);
        }
        let ttl = match self.ttl {
            RequestedTtl::Default => DEFAULT_TTL_MS,
            RequestedTtl::Millis(0) => return Err(EventError::Invalid),
            RequestedTtl::Millis(ms) => ms.clamp(60_000, MAX_TTL_MS as u64) as i64,
            RequestedTtl::NoExpiryRequested => MAX_TTL_MS,
        };
        let target = SubscriptionTarget::new(
            &self.name,
            self.arguments,
            &self.delivery.mode,
            &self.delivery.url,
        )?;
        Ok((target, WebhookSecret::parse(&self.delivery.secret)?, ttl))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnsubscribeDelivery {
    mode: String,
    url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventsUnsubscribeParams {
    name: String,
    #[serde(default)]
    arguments: EventFilter,
    delivery: UnsubscribeDelivery,
    #[serde(default, rename = "_meta")]
    _meta: Option<rmcp::model::JsonObject>,
}

impl EventsUnsubscribeParams {
    pub(crate) fn session_messages(&self) -> bool {
        self.arguments.session_messages
    }

    pub(super) fn validate(self) -> Result<SubscriptionTarget, EventError> {
        SubscriptionTarget::new(
            &self.name,
            self.arguments,
            &self.delivery.mode,
            &self.delivery.url,
        )
    }
}
