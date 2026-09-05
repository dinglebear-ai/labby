//! Refresh replay policy and current-authorization revalidation.

use crate::error::AuthError;
use crate::state::AuthState;
use crate::types::TokenResponse;
use std::time::Duration;

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
        Some(token) => state.store.find_bound_refresh_token(token).await?,
        None => None,
    };
    let Some(replacement) = replacement else {
        state
            .store
            .revoke_refresh_token_replay(predecessor, client_id)
            .await?;
        return Err(AuthError::InvalidGrant(
            "refresh token subject is no longer authorized".into(),
        ));
    };
    if replacement.binding != state.inbound_provider_binding() {
        return Err(AuthError::InvalidGrant(
            "refresh token provider binding is stale".into(),
        ));
    }
    let binding = replacement.binding;
    let replacement = replacement.value;
    let authorized = match state.inbound_provider.kind() {
        crate::config::InboundProviderKind::Google => match state
            .store
            .find_google_provider_credential(&replacement.subject)
            .await?
        {
            Some(credential) => match credential.email.as_deref() {
                Some(email) => state.is_email_explicitly_allowed(email).await?,
                None => false,
            },
            None => false,
        },
        crate::config::InboundProviderKind::Authelia => match state
            .store
            .current_verified_inbound_email(&binding.identity_issuer, &replacement.subject)
            .await?
        {
            Some(email) => state.is_email_authorized(&email).await?,
            None => false,
        },
    };
    if !authorized {
        state
            .store
            .revoke_inbound_identity(&binding.identity_issuer, &replacement.subject)
            .await?;
        return Err(AuthError::InvalidGrant(
            "refresh token subject is no longer authorized".to_string(),
        ));
    }
    Ok(Some(response))
}

/// Join an in-flight rotation across the short visibility windows before the
/// durable predecessor-to-successor replay record is published.
pub(super) async fn await_cached_refresh_response(
    state: &AuthState,
    client_id: &str,
    predecessor: &str,
    requested_resource: Option<&str>,
) -> Result<Option<TokenResponse>, AuthError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(response) =
            cached_refresh_response(state, client_id, predecessor, requested_resource).await?
        {
            return Ok(Some(response));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
