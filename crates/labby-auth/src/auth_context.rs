//! Auth context injected into request extensions by [`crate::middleware::AuthLayer`].
//!
//! Downstream handlers can read this when they need caller identity or scope
//! checks, but not every route consumes it yet.

use axum::http::request::Parts;
use std::sync::Arc;

/// Stored in request extensions by the HTTP auth middleware (see
/// [`crate::middleware::AuthLayer`]).
#[derive(Debug, Clone)]
pub struct AuthContext {
    /// JWT `sub` claim (or `"static-bearer"` for static-token requests).
    pub sub: String,
    /// Optional opaque actor key (lab-specific observability hook); produced
    /// by the [`crate::middleware::ActorKeyDeriver`] closure when one is
    /// installed on the layer. Consumers without an actor-key concept
    /// (syslog-mcp etc.) leave this `None`.
    pub actor_key: Option<Arc<str>>,
    /// Effective scopes for this request.
    pub scopes: Vec<String>,
    /// JWT `iss` claim (or `"local"` / `"browser-session"` sentinel).
    pub issuer: String,
    /// `true` when the request was authenticated via the browser session
    /// cookie rather than a bearer token.
    pub via_session: bool,
    /// Browser-session CSRF token, when the request was authenticated via
    /// session cookie. Echoed back to handlers that need to mint a fresh
    /// `x-csrf-token` for follow-up state-changing requests.
    pub csrf_token: Option<String>,
    /// Verified Google email tied to the browser session, when known.
    pub email: Option<String>,
}

/// Build the value of an `WWW-Authenticate: Bearer ...` response header
/// pointing browsers/agents at the protected-resource metadata document.
#[must_use]
pub fn www_authenticate_value(resource_url: &str) -> String {
    format!(
        "Bearer resource_metadata=\"{}\"",
        protected_resource_metadata_url(resource_url)
    )
}

/// Build the RFC 9728 metadata endpoint from the resource's authorization
/// origin. Resource paths, queries, and fragments are deliberately excluded:
/// Labby's metadata route is mounted at the origin root.
#[must_use]
pub fn protected_resource_metadata_url(resource_url: &str) -> String {
    let Ok(mut url) = url::Url::parse(resource_url) else {
        return format!(
            "{}/.well-known/oauth-protected-resource",
            resource_url.trim_end_matches('/')
        );
    };
    url.set_path("/.well-known/oauth-protected-resource");
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

/// Convenience accessor for handlers that have already split a request into
/// [`Parts`].
#[must_use]
pub fn auth_context(parts: &Parts) -> Option<&AuthContext> {
    parts.extensions.get::<AuthContext>()
}

#[cfg(test)]
mod tests {
    use super::{protected_resource_metadata_url, www_authenticate_value};

    #[test]
    fn www_authenticate_value_appends_metadata_path_and_strips_trailing_slash() {
        assert_eq!(
            www_authenticate_value("https://lab.example.com/"),
            "Bearer resource_metadata=\"https://lab.example.com/.well-known/oauth-protected-resource\""
        );
        assert_eq!(
            www_authenticate_value("https://lab.example.com"),
            "Bearer resource_metadata=\"https://lab.example.com/.well-known/oauth-protected-resource\""
        );
    }

    #[test]
    fn metadata_url_uses_origin_and_preserves_scheme_host_and_port() {
        assert_eq!(
            protected_resource_metadata_url(
                "https://proxy.example:53147/mcp/nested?code=secret#fragment"
            ),
            "https://proxy.example:53147/.well-known/oauth-protected-resource"
        );
        assert_eq!(
            www_authenticate_value("http://127.0.0.1:8765/prefix/mcp"),
            "Bearer resource_metadata=\"http://127.0.0.1:8765/.well-known/oauth-protected-resource\""
        );
    }
}
