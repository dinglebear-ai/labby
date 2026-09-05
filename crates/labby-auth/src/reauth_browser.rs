use crate::browser_authority::BrowserAuthority;
use crate::error::AuthError;
use crate::google::GoogleReauthRequest;
use crate::reauth::{ProofError, Proofs, Purpose, TrustedAuthEvent};
use crate::state::AuthState;
use crate::types::{BrowserReauthChallengeRow, BrowserReauthResult};
use crate::util::{now_unix, random_token};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

const CHALLENGE_TTL_SECS: i64 = 300;
pub const RETURN_PATH: &str = "/auth/reauth/return";

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PurposeInput {
    pub action: String,
    pub resource: String,
    pub version: String,
    pub operation: String,
    pub scope: String,
    pub payload: Value,
}

impl std::fmt::Debug for PurposeInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PurposeInput")
            .field("action", &self.action)
            .field("resource", &self.resource)
            .field("version", &self.version)
            .field("operation", &"<redacted>")
            .field("scope", &self.scope)
            .field("payload", &"<redacted>")
            .finish()
    }
}

#[derive(Deserialize, Serialize)]
struct StoredPurpose {
    digest: [u8; 32],
    operation: String,
    scope: String,
}

impl PurposeInput {
    fn purpose(&self) -> Result<Purpose, ProofError> {
        Purpose::new(
            &self.action,
            &self.resource,
            &self.version,
            &self.operation,
            &self.scope,
            &self.payload,
        )
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    pub authorization_url: String,
    pub interaction: String,
    pub expires_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum PollResult {
    Pending,
    Completed { proof: String },
    Expired,
}

pub async fn start(
    state: &AuthState,
    authority: &BrowserAuthority,
    input: &PurposeInput,
) -> Result<Started, ProofError> {
    if authority.identity_provider() != Some("google") {
        return Err(ProofError::Unsupported);
    }
    authority.revalidate().await?;
    let purpose = input.purpose()?;
    let (digest, operation, scope) = purpose.stored_parts();
    let state_token = random_token(32).map_err(|_| ProofError::Unavailable)?;
    let interaction = random_token(32).map_err(|_| ProofError::Unavailable)?;
    let nonce = random_token(32).map_err(|_| ProofError::Unavailable)?;
    let verifier = random_token(32).map_err(|_| ProofError::Unavailable)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let now = now_unix();
    let expires_at = now + CHALLENGE_TTL_SECS;
    let snapshot = authority.session_snapshot();
    state
        .store
        .insert_browser_reauth_challenge(BrowserReauthChallengeRow {
            state: state_token.clone(),
            interaction_hash: Sha256::digest(interaction.as_bytes()).into(),
            session_id: snapshot.session_id,
            subject: snapshot.subject,
            provider_code_verifier: verifier,
            nonce: nonce.clone(),
            purpose_json: serde_json::to_string(&StoredPurpose {
                digest,
                operation: operation.to_owned(),
                scope: scope.to_owned(),
            })
            .map_err(|_| ProofError::InvalidPurpose)?,
            created_at: now,
            expires_at,
        })
        .await
        .map_err(map_store)?;
    let authorization_url = state
        .google
        .reauth_url(&GoogleReauthRequest {
            state: state_token,
            nonce,
            code_challenge: challenge,
        })
        .map_err(|_| ProofError::Unavailable)?
        .to_string();
    Ok(Started {
        authorization_url,
        interaction,
        expires_at,
    })
}

pub async fn callback(
    state: &AuthState,
    callback_state: &str,
    code: &str,
    browser_session_id: Option<&str>,
) -> Result<bool, AuthError> {
    let Some(challenge) = state
        .store
        .take_browser_reauth_challenge(callback_state)
        .await?
    else {
        return Ok(false);
    };
    let result =
        complete_callback(state, callback_state, code, browser_session_id, &challenge).await;
    if result.is_err() {
        state
            .store
            .retry_browser_reauth_challenge(callback_state)
            .await?;
    }
    result.map(|()| true)
}

async fn complete_callback(
    state: &AuthState,
    callback_state: &str,
    code: &str,
    browser_session_id: Option<&str>,
    challenge: &BrowserReauthChallengeRow,
) -> Result<(), AuthError> {
    if browser_session_id != Some(challenge.session_id.as_str()) {
        return Err(AuthError::AuthFailed(
            "reauthentication browser session changed".into(),
        ));
    }
    let session = state
        .store
        .find_browser_session(&challenge.session_id)
        .await?
        .ok_or_else(|| AuthError::AuthFailed("browser session expired".into()))?;
    if session.subject != challenge.subject {
        return Err(AuthError::AuthFailed(
            "reauthentication account changed".into(),
        ));
    }
    let evidence = state
        .google
        .exchange_reauth_code(
            code,
            &challenge.provider_code_verifier,
            &challenge.nonce,
            &challenge.subject,
            challenge.created_at,
        )
        .await?;
    let authority = BrowserAuthority::from_google(
        std::sync::Arc::new(state.clone()),
        session,
        state.config.static_token_scopes.clone(),
    )
    .await
    .map_err(|_| AuthError::AuthFailed("browser authority changed".into()))?;
    let stored: StoredPurpose = serde_json::from_str(&challenge.purpose_json)
        .map_err(|_| AuthError::Storage("stored reauthentication purpose is invalid".into()))?;
    let purpose = Purpose::from_stored(stored.digest, stored.operation, stored.scope)
        .map_err(|_| AuthError::Storage("stored reauthentication purpose is invalid".into()))?;
    let event = TrustedAuthEvent::from_google(&authority, &evidence)
        .map_err(|_| AuthError::AuthFailed("fresh authentication is invalid".into()))?;
    let issued = Proofs::new(state.store.clone())
        .issue(&authority, &event, &purpose)
        .await
        .map_err(map_proof)?;
    state
        .store
        .complete_browser_reauth(callback_state, issued.proof.as_str())
        .await?;
    Ok(())
}

pub async fn poll(
    state: &AuthState,
    authority: &BrowserAuthority,
    interaction: &str,
) -> Result<PollResult, ProofError> {
    authority.revalidate().await?;
    let interaction_hash: [u8; 32] = Sha256::digest(interaction.as_bytes()).into();
    Ok(
        match state
            .store
            .poll_browser_reauth(&interaction_hash, &authority.session_snapshot().session_id)
            .await
            .map_err(map_store)?
        {
            Some(BrowserReauthResult::Pending) => PollResult::Pending,
            Some(BrowserReauthResult::Completed(proof)) => PollResult::Completed { proof },
            None => PollResult::Expired,
        },
    )
}

pub async fn cancel(
    state: &AuthState,
    authority: &BrowserAuthority,
    interaction: &str,
) -> Result<bool, ProofError> {
    authority.revalidate().await?;
    let interaction_hash: [u8; 32] = Sha256::digest(interaction.as_bytes()).into();
    state
        .store
        .cancel_browser_reauth(&interaction_hash, &authority.session_snapshot().session_id)
        .await
        .map_err(map_store)
}

fn map_store(error: AuthError) -> ProofError {
    match error {
        AuthError::RateLimited { .. } => ProofError::RateLimited,
        AuthError::Validation(_) => ProofError::InvalidPurpose,
        _ => ProofError::Unavailable,
    }
}

fn map_proof(error: ProofError) -> AuthError {
    match error {
        ProofError::Denied | ProofError::Required => AuthError::AuthFailed(error.to_string()),
        ProofError::RateLimited => AuthError::RateLimited {
            message: error.to_string(),
            retry_after_ms: 1_000,
        },
        _ => AuthError::InvalidGrant(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stored_purpose_contains_no_raw_payload() {
        let input = PurposeInput {
            action: "provider.save".into(),
            resource: "team".into(),
            version: "7".into(),
            operation: "operation-secret".into(),
            scope: "lab:admin".into(),
            payload: json!({"bearerToken": "credential-secret"}),
        };
        let purpose = input.purpose().unwrap();
        let (digest, operation, scope) = purpose.stored_parts();
        let encoded = serde_json::to_string(&StoredPurpose {
            digest,
            operation: operation.to_owned(),
            scope: scope.to_owned(),
        })
        .unwrap();
        assert!(!encoded.contains("credential-secret"));
        assert!(encoded.contains("operation-secret"));
        let debug = format!("{input:?}");
        assert!(!debug.contains("credential-secret"));
        assert!(!debug.contains("operation-secret"));
    }
}
