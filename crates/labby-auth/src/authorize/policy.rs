use tracing::{debug, warn};

use crate::error::AuthError;
use crate::state::AuthState;
use crate::util::fingerprint;

/// Enforces the configured email allowlist using Google's verified identity claims.
pub(crate) fn check_email_allowlist(
    email: Option<&str>,
    email_verified: Option<bool>,
    hosted_domain: Option<&str>,
    allowed_emails: &[String],
    allowed_domains: &[String],
) -> Result<(), AuthError> {
    if allowed_emails.is_empty() && allowed_domains.is_empty() {
        return Ok(());
    }
    if email_verified != Some(true) {
        warn!("oauth callback rejected: google did not return a verified email address");
        return Err(AuthError::AuthFailed(
            "google did not return a verified email address".to_string(),
        ));
    }
    let Some(email) = email else {
        warn!("oauth callback rejected: google did not return an email address");
        return Err(AuthError::AuthFailed(
            "google did not return an email address".to_string(),
        ));
    };
    let email = email.trim();
    if allowed_emails
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(email))
    {
        return Ok(());
    }
    if let Some(domain) = hosted_domain
        .map(str::trim)
        .filter(|domain| !domain.is_empty())
        && allowed_domains
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(domain))
    {
        return Ok(());
    }
    warn!(
        email_id = %fingerprint(email),
        "oauth callback rejected: email not in allowed list"
    );
    Err(AuthError::AuthFailed(
        "google account is not permitted to access this gateway".to_string(),
    ))
}

pub(crate) fn validate_response_type(response_type: &str) -> Result<(), AuthError> {
    if response_type == "code" {
        return Ok(());
    }
    warn!(
        response_type = %response_type,
        "oauth authorize rejected: unsupported response_type"
    );
    Err(AuthError::Validation(
        "response_type must be `code`".to_string(),
    ))
}

/// Elevates an allowlisted user to the admin scope associated with the configured base scope.
pub(crate) fn elevate_scope_for_allowed_user(scope: &str, default_scope: &str) -> String {
    let base = default_scope.split(':').next().unwrap_or(default_scope);
    let admin_scope = format!("{base}:admin");
    let mut scopes: Vec<&str> = scope
        .split_whitespace()
        .filter(|scope| !scope.is_empty())
        .collect();
    if !scopes.iter().any(|scope| *scope == admin_scope.as_str()) {
        scopes.push(admin_scope.as_str());
    }
    scopes.join(" ")
}

pub(crate) fn validate_scope(
    state: &AuthState,
    resource: &str,
    scope: &str,
) -> Result<String, AuthError> {
    let canonical = crate::metadata::canonical_resource_url(state);
    let supported = if resource.trim_end_matches('/') == canonical {
        state.config.scopes_supported.clone()
    } else {
        state
            .allowed_resource_scopes(resource)
            .filter(|scopes| !scopes.is_empty())
            .ok_or_else(|| {
                AuthError::Validation(format!(
                    "resource must be `{canonical}` or a configured protected MCP route"
                ))
            })?
    };
    let normalized = scope.trim();
    if normalized.is_empty() {
        let scope = if resource.trim_end_matches('/') == canonical {
            state.config.default_scope.clone()
        } else {
            supported.join(" ")
        };
        debug!(resource = %resource, scope = %scope, "oauth authorize defaulted scope");
        return Ok(scope);
    }
    let requested = normalized.split_whitespace().collect::<Vec<_>>();
    if requested
        .iter()
        .all(|scope| supported.iter().any(|allowed| allowed == scope))
    {
        let scope = requested.join(" ");
        debug!(resource = %resource, requested_scope = %normalized, normalized_scope = %scope, "oauth authorize scope accepted");
        return Ok(scope);
    }
    warn!(scope = %normalized, resource = %resource, supported_scopes = ?supported, "oauth authorize rejected: unsupported scope");
    Err(AuthError::Validation(format!(
        "scope must be one of: {}",
        supported.join(", ")
    )))
}

pub(crate) fn validate_resource(
    state: &AuthState,
    requested: Option<&str>,
) -> Result<String, AuthError> {
    let canonical = crate::metadata::canonical_resource_url(state);
    let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(canonical);
    };
    let requested = requested.trim_end_matches('/');
    if requested == canonical || state.is_allowed_resource_url(requested) {
        debug!(requested_resource = %requested, canonical_resource = %canonical, protected_resource = requested != canonical, "oauth resource accepted");
        return Ok(requested.to_string());
    }
    warn!(requested_resource = %requested, expected_resource = %canonical, "oauth request rejected: resource does not match an allowed MCP endpoint");
    Err(AuthError::Validation(format!(
        "resource must be `{canonical}` or a configured protected MCP route"
    )))
}
