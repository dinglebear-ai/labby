//! Scrubbed error type for the `openapi` provider. NEVER carries a raw
//! `rmcp_openapi::*` or `reqwest` error string, an upstream response body, or a
//! credential — every variant's `Display` is a fixed, operator-safe template.
//!
//! The full `ToolError` mapping is finalized in Task 5; this file currently
//! carries the load-time SSRF variant.

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
}
