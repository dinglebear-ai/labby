use std::net::SocketAddr;

use axum::Json;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use super::{AUTH_REQUEST_TTL_SECS, is_allowed_redirect_uri, remote_ip, sanitize_return_to};
use crate::error::AuthError;
use crate::google::AuthorizeUrlRequest;
use crate::state::AuthState;
use crate::types::{
    BrowserLoginQuery, BrowserLoginStateRow, ClientRegistrationRequest, ClientRegistrationResponse,
    RegisteredClient,
};
use crate::util::{fingerprint, now_unix, random_token};

pub async fn browser_login(
    State(state): State<AuthState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<BrowserLoginQuery>,
) -> Result<axum::response::Response, AuthError> {
    state.check_authorize_rate_limit(remote_ip(addr)).await?;
    state.ensure_pending_oauth_state_capacity().await?;
    let return_to = sanitize_return_to(&state, query.return_to.as_deref());
    let provider_code_verifier = random_token(32)?;
    let provider_code_challenge =
        URL_SAFE_NO_PAD.encode(Sha256::digest(provider_code_verifier.as_bytes()));
    let request_state = random_token(24)?;
    let oauth_state_id = fingerprint(&request_state);
    state
        .store
        .insert_browser_login_state(BrowserLoginStateRow {
            state: request_state.clone(),
            return_to: return_to.clone(),
            provider_code_verifier,
            created_at: now_unix(),
            expires_at: now_unix() + AUTH_REQUEST_TTL_SECS,
        })
        .await?;
    let location = state.google.authorize_url(&AuthorizeUrlRequest {
        state: request_state,
        scope: state.config.default_scope.clone(),
        code_challenge: provider_code_challenge,
        code_challenge_method: "S256".to_string(),
        offline_access: false,
        force_consent: false,
    })?;
    info!(oauth_state_id = %oauth_state_id, "browser login redirected to upstream provider");
    Ok((
        StatusCode::FOUND,
        [(header::LOCATION, location.to_string())],
    )
        .into_response())
}

pub async fn register_client(
    State(state): State<AuthState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(request): Json<ClientRegistrationRequest>,
) -> Result<Json<ClientRegistrationResponse>, AuthError> {
    state.check_register_rate_limit(remote_ip(addr)).await?;
    if request.redirect_uris.is_empty() {
        warn!("oauth register rejected: no redirect URIs provided");
        return Err(AuthError::Validation(
            "at least one redirect URI is required".to_string(),
        ));
    }
    let native_callback_endpoint = crate::metadata::native_callback_endpoint(&state);
    for redirect_uri in &request.redirect_uris {
        if redirect_uri != &native_callback_endpoint
            && !is_allowed_redirect_uri(redirect_uri, &state.config.allowed_client_redirect_uris)
        {
            warn!(
                redirect_uri_id = %fingerprint(redirect_uri),
                "oauth register rejected: redirect URI is not in the allowlist, native callback, or loopback set"
            );
            return Err(AuthError::Validation(format!(
                "redirect URI `{redirect_uri}` must target a loopback host, match the native callback endpoint, or match an allowed redirect pattern"
            )));
        }
    }
    let client = RegisteredClient {
        client_id: format!("dcr_{}", random_token(18)?),
        redirect_uris: request.redirect_uris,
        created_at: now_unix(),
        token_endpoint_auth_method: "none".to_string(),
        token_endpoint_auth_methods: Vec::new(),
        jwks: None,
        jwks_uri: None,
    };
    state.store.register_client(client.clone()).await?;
    info!(
        client_id = %client.client_id,
        redirect_uri_count = client.redirect_uris.len(),
        "oauth client registration accepted"
    );
    Ok(Json(ClientRegistrationResponse {
        client_id: client.client_id,
        redirect_uris: client.redirect_uris,
        token_endpoint_auth_method: "none".to_string(),
    }))
}
