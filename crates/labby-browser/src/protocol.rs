//! Versioned extension protocol vocabulary.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Current browser protocol version.
pub(crate) const PROTOCOL_VERSION: u32 = 1;

/// JSON envelope exchanged with the MV3 extension.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BrowserEnvelope {
    /// Protocol version. Unknown versions fail closed.
    pub version: u32,
    /// Optional request identity for request/reply correlation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Typed message payload.
    #[serde(flatten)]
    pub message: BrowserMessage,
}

impl BrowserEnvelope {
    /// Build an envelope using the current protocol version.
    #[must_use]
    pub fn new(request_id: Option<String>, message: BrowserMessage) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            message,
        }
    }

    /// Reject unknown protocol versions before interpreting their messages.
    pub fn validate_version(&self) -> crate::Result<()> {
        if self.version == PROTOCOL_VERSION {
            Ok(())
        } else {
            Err(crate::BrowserError::InvalidRequest(format!(
                "unsupported browser protocol version {}",
                self.version
            )))
        }
    }
}

/// Extension and runtime messages.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserMessage {
    /// Begin operator-approved pairing.
    PairingRequest {
        /// Human-facing browser/profile name.
        display_name: String,
        /// Chrome extension identity.
        extension_id: String,
        /// Base64-encoded Ed25519 public key.
        public_key: String,
    },
    /// Pairing request accepted for later polling.
    PairingPending {
        /// Durable pairing identity.
        pairing_id: String,
        /// Unix expiry timestamp.
        expires_at: i64,
    },
    /// Poll pairing state.
    PairingStatus { pairing_id: String },
    /// Approved pairing result.
    PairingApproved { browser_id: String },
    /// One-time authentication challenge.
    AuthChallenge { browser_id: String },
    /// Nonce to sign, encoded as base64url without padding.
    AuthNonce {
        challenge_id: String,
        nonce: String,
        expires_at: i64,
    },
    /// Signed response to a challenge.
    AuthResponse {
        challenge_id: String,
        signature: String,
    },
    /// Authentication completed.
    Authenticated { browser_id: String },
    /// Browser catalog observation.
    Observe(CatalogObservation),
    /// Close one exact browser document.
    DocumentClosed { tab_id: i64, document_id: String },
    /// Invoke one tool on one immutable document/catalog tuple.
    ToolCall {
        call_id: String,
        tab_id: i64,
        document_id: String,
        catalog_revision: i64,
        tool_name: String,
        arguments: Value,
    },
    /// Cancel one pending call.
    ToolCancel { call_id: String },
    /// Successful page tool result.
    ToolResult { call_id: String, result: Value },
    /// Failed page tool result.
    ToolError {
        call_id: String,
        kind: String,
        message: String,
    },
    /// Protocol-level acknowledgement.
    Acknowledged { received: String },
    /// Stable protocol error.
    Error { kind: String, message: String },
}

/// Sanitized catalog observation for a concrete browser document.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CatalogObservation {
    pub tab_id: i64,
    pub document_id: String,
    pub origin: String,
    pub sanitized_path: String,
    pub page_title: String,
    pub catalog_revision: i64,
    pub catalog_fingerprint: String,
    pub tools: Vec<ToolDescriptor>,
}

/// Serializable WebMCP tool metadata. Executable callbacks never cross the wire.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ToolDescriptor {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_object_schema")]
    pub input_schema: Value,
    #[serde(default)]
    pub annotations: Value,
}

fn default_object_schema() -> Value {
    serde_json::json!({"type": "object"})
}
