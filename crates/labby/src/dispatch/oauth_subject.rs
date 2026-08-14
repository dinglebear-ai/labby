//! Surface-neutral upstream OAuth credential-subject policy.

use std::borrow::Cow;

use labby_auth::auth_context::AuthContext;

/// Resolve the credential-cache subject for an upstream OAuth request.
///
/// A missing authentication context represents a trusted local/stdio caller.
/// Network adapters must establish a verified [`AuthContext`] before calling
/// this helper so unauthenticated traffic cannot inherit shared credentials.
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

#[cfg(test)]
mod tests {
    use super::oauth_upstream_subject_for_request;
    use labby_auth::auth_context::AuthContext;

    fn auth(scopes: &[&str]) -> AuthContext {
        AuthContext {
            sub: "verified-user".to_string(),
            actor_key: None,
            scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: None,
        }
    }

    #[test]
    fn trusted_local_and_admin_callers_share_gateway_credentials() {
        let shared = crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
        assert_eq!(
            oauth_upstream_subject_for_request(None, Some("ignored-local-subject")).as_deref(),
            Some(shared)
        );
        assert_eq!(
            oauth_upstream_subject_for_request(Some(&auth(&["lab:admin"])), Some("admin"))
                .as_deref(),
            Some(shared)
        );
    }

    #[test]
    fn authenticated_non_admin_callers_are_subject_scoped_and_fail_closed() {
        let reader = auth(&["lab:read"]);
        assert_eq!(
            oauth_upstream_subject_for_request(Some(&reader), Some("reader")).as_deref(),
            Some("reader")
        );
        assert!(oauth_upstream_subject_for_request(Some(&reader), None).is_none());
    }
}
