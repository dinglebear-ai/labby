//! Scrubbed error type for the `openapi` provider. NEVER carries a raw
//! `rmcp_openapi::*` or `reqwest` error string, an upstream response body, or a
//! credential — every variant's `Display` is a fixed, operator-safe template.
//!
//! The `From<OpenApiError> for ToolError` impl targets the REAL `ToolError`
//! variants (`crates/labby-runtime/src/error.rs`): there is NO `Timeout` or
//! `Internal` variant — timeout/internal go through `Sdk { sdk_kind }`.

use labby_runtime::error::ToolError;

/// Errors from spec loading and outbound dispatch. All messages are scrubbed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum OpenApiError {
    /// The operator-configured base URL failed the SSRF preflight guard.
    #[error("spec `{label}` base URL rejected by SSRF guard: {reason}")]
    SsrfRejected {
        /// Spec label.
        label: String,
        /// SSRF error kind (already scrubbed — no caller secrets).
        reason: String,
    },
    /// The spec document could not be parsed by the parse-only surface.
    #[error("failed to parse OpenAPI spec `{label}`")]
    SpecParse {
        /// Spec label.
        label: String,
    },
    /// The spec document exceeded the pre-parse size cap.
    #[error("spec document `{label}` exceeds the size cap")]
    SpecTooLarge {
        /// Spec label.
        label: String,
    },
    /// No spec is registered under the requested label.
    #[error("unknown spec label `{label}`")]
    UnknownInstance {
        /// Requested label.
        label: String,
        /// Known labels.
        valid: Vec<String>,
    },
    /// The operation is not in the spec's deny-by-default allowlist (or does not exist).
    #[error("unknown operation `{operation_id}` in spec `{label}`")]
    UnknownOperation {
        /// Spec label.
        label: String,
        /// Requested operationId.
        operation_id: String,
    },
    /// The request resolved (or redirected) to a private/loopback address.
    #[error("request for spec `{label}` blocked: resolved to a private address")]
    RequestBlockedPrivateAddr {
        /// Spec label.
        label: String,
    },
    /// The outbound request failed. NO body/url/auth is ever included.
    #[error("upstream request for spec `{label}` failed")]
    UpstreamRequest {
        /// Spec label.
        label: String,
    },
    /// The outbound request timed out.
    #[error("upstream request for spec `{label}` timed out")]
    UpstreamTimeout {
        /// Spec label.
        label: String,
    },
}

impl OpenApiError {
    /// Stable kind tag mirroring the dispatcher error vocabulary.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SsrfRejected { .. } | Self::SpecParse { .. } | Self::SpecTooLarge { .. } => {
                "config_error"
            }
            Self::RequestBlockedPrivateAddr { .. } => "forbidden",
            Self::UnknownInstance { .. } => "unknown_instance",
            Self::UnknownOperation { .. } => "unknown_action",
            Self::UpstreamRequest { .. } => "internal_error",
            Self::UpstreamTimeout { .. } => "timeout",
        }
    }
}

impl From<OpenApiError> for ToolError {
    fn from(e: OpenApiError) -> Self {
        // Message is our OWN scrubbed Display — never a raw upstream error string.
        let msg = e.to_string();
        match &e {
            OpenApiError::UnknownInstance { valid, .. } => ToolError::UnknownInstance {
                message: msg,
                valid: valid.clone(),
            },
            OpenApiError::UnknownOperation { .. } => ToolError::UnknownAction {
                message: msg,
                valid: vec![],
                hint: None,
            },
            OpenApiError::RequestBlockedPrivateAddr { .. } => ToolError::Forbidden {
                message: msg,
                required_scopes: vec![],
            },
            OpenApiError::SsrfRejected { .. }
            | OpenApiError::SpecParse { .. }
            | OpenApiError::SpecTooLarge { .. } => ToolError::InvalidParam {
                message: msg,
                param: "spec".into(),
            },
            OpenApiError::UpstreamTimeout { .. } => ToolError::Sdk {
                sdk_kind: "timeout".into(),
                message: msg,
            },
            OpenApiError::UpstreamRequest { .. } => ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: msg,
            },
        }
    }
}
