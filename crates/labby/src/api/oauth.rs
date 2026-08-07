//! Compatibility re-export shim — `AuthContext` now lives in the shared
//! `labby_auth` crate. Existing `use crate::api::oauth::AuthContext;` import
//! sites continue to compile via this re-export.
//!
//! `www_authenticate_value` likewise re-exported for the (rare) lab callers
//! that build their own `WWW-Authenticate` header outside of the auth layer.

#[cfg(feature = "gateway")]
use std::borrow::Cow;

pub use labby_auth::auth_context::AuthContext;
// Re-exported for lab callers that build WWW-Authenticate headers directly;
// not used within this crate itself.
#[allow(unused_imports)]
pub use labby_auth::www_authenticate_value;

/// Resolve the credential-cache subject for an upstream OAuth request.
///
/// `None` authentication is reserved for trusted stdio MCP callers. HTTP
/// surfaces must call this only after establishing a verified `AuthContext`;
/// otherwise an unauthenticated request could inherit the shared admin cache.
#[cfg(feature = "gateway")]
pub(crate) fn oauth_upstream_subject_for_request<'a>(
    auth: Option<&AuthContext>,
    request_subject: Option<&'a str>,
) -> Option<Cow<'a, str>> {
    match auth {
        None => Some(Cow::Borrowed(
            crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT,
        )),
        Some(ctx) if ctx.scopes.iter().any(|scope| scope == "lab:admin") => Some(Cow::Borrowed(
            crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT,
        )),
        Some(_) => request_subject.map(Cow::Borrowed),
    }
}
