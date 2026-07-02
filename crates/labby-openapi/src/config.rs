//! Pure config data types for the Code Mode `openapi` provider.
//!
//! These types carry NO env/file-reading logic — the only reads live in
//! `crates/labby/src/config.rs`. Secret-bearing types hand-write `Debug` so a
//! credential value never reaches a log line, an error message, or a panic
//! payload.

use std::path::PathBuf;

/// Provider namespaces that an operator may NOT reuse as an `openapi` spec
/// label, because they already name a Code Mode local provider (or `openapi`
/// itself).
pub const RESERVED_NAMESPACES: [&str; 3] = ["state", "git", "openapi"];

/// Where a spec document is fetched from.
pub enum SpecSource {
    /// Remote HTTPS URL (SSRF-validated at fetch time).
    Url(url::Url),
    /// Local filesystem path.
    Path(PathBuf),
}

impl std::fmt::Debug for SpecSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // A spec URL can carry a token in its query/userinfo — redact it.
            Self::Url(u) => f
                .debug_tuple("Url")
                .field(&labby_primitives::ssrf::redact_url(u.as_str()))
                .finish(),
            Self::Path(p) => f.debug_tuple("Path").field(p).finish(),
        }
    }
}

impl Clone for SpecSource {
    fn clone(&self) -> Self {
        match self {
            Self::Url(u) => Self::Url(u.clone()),
            Self::Path(p) => Self::Path(p.clone()),
        }
    }
}

/// Server-side credential injected into every outbound request for a spec.
///
/// The JS snippet never sees these values — they are attached after the sandbox
/// boundary in `http::execute_operation`.
pub enum OpenApiCredential {
    /// `<header>: <value>` (spec-declared or defaulted apiKey header).
    ApiKey {
        /// Header name (e.g. `X-API-Key`).
        header: String,
        /// Secret key value.
        value: String,
    },
    /// `Authorization: Bearer <token>`.
    BearerToken(String),
}

impl std::fmt::Debug for OpenApiCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey { header, .. } => f
                .debug_struct("ApiKey")
                .field("header", header)
                .field("value", &"<redacted>")
                .finish(),
            Self::BearerToken(_) => f.debug_tuple("BearerToken").field(&"<redacted>").finish(),
        }
    }
}

impl Clone for OpenApiCredential {
    fn clone(&self) -> Self {
        match self {
            Self::ApiKey { header, value } => Self::ApiKey {
                header: header.clone(),
                value: value.clone(),
            },
            Self::BearerToken(t) => Self::BearerToken(t.clone()),
        }
    }
}

/// One configured OpenAPI spec, resolved into runtime-ready form.
///
/// `base_url` is MANDATORY: `rmcp-openapi` never consults the spec's `servers[]`,
/// so the operator must always supply (and we always SSRF-validate) the base URL.
#[derive(Debug, Clone)]
pub struct OpenApiSpecConfig {
    /// Provider label — the `<label>` in `openapi::<label>.<operationId>`.
    pub label: String,
    /// Spec document location.
    pub spec_source: SpecSource,
    /// Mandatory, SSRF-validated base URL for outbound requests.
    pub base_url: url::Url,
    /// Deny-by-default allowlist of raw operationIds that may be dispatched.
    pub allowed_operations: Vec<String>,
    /// Optional server-side credential.
    pub credential: Option<OpenApiCredential>,
}

/// All configured specs for the `openapi` provider.
#[derive(Debug, Clone, Default)]
pub struct OpenApiProviderConfig {
    /// Configured specs, keyed by unique `label`.
    pub specs: Vec<OpenApiSpecConfig>,
}
