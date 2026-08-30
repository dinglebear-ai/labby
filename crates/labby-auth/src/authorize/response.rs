//! Authorization response construction and RFC 9207 issuer binding.

use axum::response::{IntoResponse, Redirect, Response};
use tracing::warn;

use super::AuthorizeQuery;
use crate::error::AuthError;
use crate::state::AuthState;

pub(super) fn authorization_error_redirect(
    state: &AuthState,
    query: &AuthorizeQuery,
    error_code: &str,
    error: AuthError,
) -> Result<Response, AuthError> {
    let mut redirect = url::Url::parse(&query.redirect_uri).map_err(|parse_error| {
        AuthError::Config(format!(
            "validated redirect_uri could not be parsed: {parse_error}"
        ))
    })?;
    redirect
        .query_pairs_mut()
        .append_pair("error", error_code)
        .append_pair("error_description", public_error_description(error_code))
        .append_pair("state", &query.state);
    warn!(
        kind = error.kind(),
        oauth_error_code = error_code,
        "authorization request rejected"
    );
    append_authorization_response_issuer(state, &mut redirect);
    Ok(Redirect::to(redirect.as_str()).into_response())
}

pub(super) fn append_authorization_response_issuer(state: &AuthState, redirect: &mut url::Url) {
    if !state.config.codex_issuer_compatibility {
        redirect
            .query_pairs_mut()
            .append_pair("iss", &crate::metadata::public_base_url(state));
    }
}

pub(super) fn authorization_callback_error_redirect(
    state: &AuthState,
    redirect_uri: &str,
    client_state: &str,
    error_code: &str,
    error: &AuthError,
) -> Result<Response, AuthError> {
    let mut redirect = url::Url::parse(redirect_uri).map_err(|parse_error| {
        AuthError::Config(format!(
            "validated redirect_uri could not be parsed: {parse_error}"
        ))
    })?;
    redirect
        .query_pairs_mut()
        .append_pair("error", error_code)
        .append_pair("error_description", public_error_description(error_code))
        .append_pair("state", client_state);
    warn!(
        kind = error.kind(),
        oauth_error_code = error_code,
        "authorization callback rejected"
    );
    append_authorization_response_issuer(state, &mut redirect);
    Ok(Redirect::to(redirect.as_str()).into_response())
}

const fn public_error_description(error_code: &str) -> &'static str {
    match error_code.as_bytes() {
        b"access_denied" => "The authorization request was denied",
        b"invalid_scope" => "The requested scope is invalid",
        b"invalid_target" => "The requested resource is invalid",
        b"invalid_request" => "The authorization request is invalid",
        _ => "The authorization request could not be completed",
    }
}

pub(super) fn authorization_response_query_presence(redirect: &url::Url) -> (bool, bool, bool) {
    let mut has_code = false;
    let mut has_state = false;
    let mut has_issuer = false;
    for (name, _) in redirect.query_pairs() {
        match name.as_ref() {
            "code" => has_code = true,
            "state" => has_state = true,
            "iss" => has_issuer = true,
            _ => {}
        }
    }
    (has_code, has_state, has_issuer)
}

#[cfg(test)]
mod tests {
    use super::{append_authorization_response_issuer, authorization_callback_error_redirect};

    #[tokio::test]
    async fn successful_authorization_response_uses_exact_metadata_issuer() {
        let state = crate::authorize::tests::test_auth_state().await;
        let mut redirect = url::Url::parse("https://client.example/callback?code=abc").unwrap();
        append_authorization_response_issuer(&state, &mut redirect);
        assert_eq!(
            redirect
                .query_pairs()
                .find(|(name, _)| name == "iss")
                .map(|(_, value)| value.into_owned()),
            Some("https://lab.example.com".to_string())
        );
    }

    #[tokio::test]
    async fn error_authorization_response_uses_exact_metadata_issuer() {
        let state = crate::authorize::tests::test_auth_state().await;
        let response = authorization_callback_error_redirect(
            &state,
            "https://client.example/callback",
            "state-1",
            "access_denied",
            &crate::error::AuthError::AuthFailed("access denied".to_string()),
        )
        .unwrap();
        let location = response
            .headers()
            .get(axum::http::header::LOCATION)
            .unwrap();
        let redirect = url::Url::parse(location.to_str().unwrap()).unwrap();
        assert_eq!(
            redirect
                .query_pairs()
                .find(|(name, _)| name == "iss")
                .map(|(_, value)| value.into_owned()),
            Some("https://lab.example.com".to_string())
        );
        assert_eq!(
            redirect
                .query_pairs()
                .find(|(name, _)| name == "error_description")
                .map(|(_, value)| value.into_owned()),
            Some("The authorization request was denied".to_string())
        );
        assert!(!redirect.as_str().contains("access%20denied"));
    }

    #[tokio::test]
    async fn callback_error_redirect_does_not_disclose_internal_error_detail() {
        let state = crate::authorize::tests::test_auth_state().await;
        let response = authorization_callback_error_redirect(
            &state,
            "https://client.example/callback",
            "state-1",
            "server_error",
            &crate::error::AuthError::Storage(
                "/private/auth.db: signed-query=super-secret".to_string(),
            ),
        )
        .unwrap();
        let location = response
            .headers()
            .get(axum::http::header::LOCATION)
            .unwrap();
        let location = location.to_str().unwrap();
        assert!(!location.contains("auth.db"));
        assert!(!location.contains("super-secret"));
        let redirect = url::Url::parse(location).unwrap();
        assert_eq!(
            redirect
                .query_pairs()
                .find(|(name, _)| name == "error_description")
                .map(|(_, value)| value.into_owned()),
            Some("The authorization request could not be completed".to_string())
        );
    }
}
