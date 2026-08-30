//! Refresh replay policy and current-authorization revalidation.

use crate::error::AuthError;
use crate::state::AuthState;
use crate::types::TokenResponse;

pub(super) async fn cached_refresh_response(
    state: &AuthState,
    client_id: &str,
    predecessor: &str,
    requested_resource: Option<&str>,
) -> Result<Option<TokenResponse>, AuthError> {
    let response = state
        .store
        .find_refresh_token_replay(predecessor, client_id, requested_resource)
        .await?;
    let Some(response) = response else {
        return Ok(None);
    };
    let replacement = match response.refresh_token.as_deref() {
        Some(token) => state.store.find_refresh_token(token).await?,
        None => None,
    };
    let authorized = if let Some(replacement) = replacement {
        if let Some(credential) = state
            .store
            .find_google_provider_credential(&replacement.subject)
            .await?
        {
            let allowed = state.resolve_allowed_emails().await?;
            crate::authorize::check_email_allowlist(
                credential.email.as_deref(),
                credential.email.as_ref().map(|_| true),
                None,
                &allowed,
                &state.config.allowed_email_domains,
            )
            .is_ok()
        } else {
            false
        }
    } else {
        false
    };
    if !authorized {
        state
            .store
            .revoke_refresh_token_replay(predecessor, client_id)
            .await?;
        return Err(AuthError::InvalidGrant(
            "refresh token subject is no longer authorized".to_string(),
        ));
    }
    Ok(Some(response))
}
