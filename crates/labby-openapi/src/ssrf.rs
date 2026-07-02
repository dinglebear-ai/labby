//! Load-time SSRF validation of the mandatory operator-configured base URL.
//!
//! `rmcp-openapi` never reads the spec's `servers[]`, so this reduces to running
//! the mandatory `base_url` through the workspace-canonical guard. Request-time
//! peer-IP re-validation (the DNS-rebinding defense) lives in `http.rs`, not here.

use crate::config::OpenApiSpecConfig;
use crate::error::OpenApiError;

/// Validate a spec's mandatory `base_url`: https-only, no userinfo/query/fragment,
/// and reject loopback / link-local / RFC1918 / CGNAT / private-TLD hosts via the
/// canonical `labby_primitives::ssrf` guard. Performs NO DNS — an IP literal is
/// checked directly; a hostname is re-resolved and re-checked at request time.
///
/// # Errors
/// Returns [`OpenApiError::SsrfRejected`] when the guard rejects the URL.
pub fn validate_base_url(cfg: &OpenApiSpecConfig) -> Result<url::Url, OpenApiError> {
    labby_primitives::ssrf::parse_validated_https_url(cfg.base_url.as_str()).map_err(|e| {
        OpenApiError::SsrfRejected {
            label: cfg.label.clone(),
            reason: e.kind().to_string(),
        }
    })
}
