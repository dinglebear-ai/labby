//! Load-time SSRF validation of the mandatory operator-configured base URL.
//!
//! The converter ignores the spec's `servers[]`. This module validates the
//! operator's `base_url` and remote spec URL with the canonical static guard.
//! Request-time DNS validation, address pinning, and peer-IP checks live in
//! `http.rs`.

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
    validate_https_url(&cfg.label, &cfg.base_url)?;
    Ok(cfg.base_url.clone())
}

/// Shared guard: run a URL through the canonical `parse_validated_https_url` and
/// wrap any rejection in a labeled [`OpenApiError::SsrfRejected`].
fn validate_https_url(label: &str, url: &url::Url) -> Result<(), OpenApiError> {
    labby_primitives::ssrf::parse_validated_https_url(url.as_str())
        .map(|_| ())
        .map_err(|e| OpenApiError::SsrfRejected {
            label: label.to_string(),
            reason: e.kind().to_string(),
        })
}

/// Validate a remote spec-document URL (`SpecSource::Url`) with the SAME canonical
/// guard as [`validate_base_url`], BEFORE the boot-time fetch. Without this the
/// spec fetch — which happens during `labby serve` startup — could target an
/// arbitrary endpoint; the request-time peer-IP re-check in `http.rs` still
/// applies, but this rejects userinfo / non-https / private-TLD / private-IP-literal
/// spec URLs up front with a clear error rather than mid-fetch.
///
/// # Errors
/// Returns [`OpenApiError::SsrfRejected`] when the guard rejects the URL.
pub fn validate_spec_url(label: &str, url: &url::Url) -> Result<(), OpenApiError> {
    validate_https_url(label, url)
}
