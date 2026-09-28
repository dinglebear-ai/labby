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
use crate::util::{fingerprint, now_unix, oauth_state_diagnostic_id, random_token};

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
    let oauth_state_id = oauth_state_diagnostic_id(&request_state);
    state
        .store
        .insert_bound_browser_login_state(
            BrowserLoginStateRow {
                state: request_state.clone(),
                return_to: return_to.clone(),
                provider_code_verifier,
                created_at: now_unix(),
                expires_at: now_unix() + AUTH_REQUEST_TTL_SECS,
            },
            state.inbound_provider_binding(),
        )
        .await?;
    let location = state.inbound_provider.authorize_url(&AuthorizeUrlRequest {
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

async fn register_client_inner(
    state: AuthState,
    addr: SocketAddr,
    request: Result<Json<ClientRegistrationRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<ClientRegistrationResponse, RegistrationError> {
    state.check_register_rate_limit(remote_ip(addr)).await?;
    // Adapt extraction failures at the OAuth boundary without reflecting request data.
    let Json(request) = request.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            RegistrationError::PayloadTooLarge
        } else {
            RegistrationError::InvalidMetadata(
                "registration requires a JSON object with an array of redirect URI strings"
                    .to_string(),
            )
        }
    })?;
    if request.redirect_uris.is_empty() {
        warn!("oauth register rejected: no redirect URIs provided");
        return Err(RegistrationError::InvalidMetadata(
            "at least one redirect URI is required".to_string(),
        ));
    }
    let native_callback_endpoint = crate::metadata::native_callback_endpoint(&state);
    let mut rejected = false;
    let mut reported = 0usize;
    for redirect_uri in &request.redirect_uris {
        if redirect_uri != &native_callback_endpoint
            && !is_allowed_redirect_uri(redirect_uri, &state.config.allowed_client_redirect_uris)
        {
            rejected = true;
            // Show the complete callback-host mismatch without logging paths,
            // query values, or credentials, and bound diagnostics for large requests.
            if reported < 16 {
                let redirect_origin = reqwest::Url::parse(redirect_uri)
                    .ok()
                    .map(|url| url.origin().ascii_serialization());
                warn!(
                    redirect_uri_id = %fingerprint(redirect_uri),
                    redirect_origin = ?redirect_origin,
                    redirect_uri_count = request.redirect_uris.len(),
                    "oauth register rejected: redirect URI is not in the allowlist, native callback, or loopback set"
                );
                reported += 1;
            }
        }
    }
    if rejected {
        return Err(RegistrationError::InvalidRedirectUri(
            "redirect URI must target a loopback host, match the native callback endpoint, or match an allowed redirect pattern".to_string(),
        ));
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
        client_id = %fingerprint(&client.client_id),
        redirect_uri_count = client.redirect_uris.len(),
        "oauth client registration accepted"
    );
    Ok(ClientRegistrationResponse {
        client_id: client.client_id,
        redirect_uris: client.redirect_uris,
        token_endpoint_auth_method: "none".to_string(),
    })
}

/// RFC 7591 response adaptation belongs to this endpoint, not generic auth errors.
pub async fn register_client(
    State(state): State<AuthState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Result<Json<ClientRegistrationRequest>, axum::extract::rejection::JsonRejection>,
) -> axum::response::Response {
    match register_client_inner(state, addr, request).await {
        Ok(client) => {
            registration_cache_headers((StatusCode::CREATED, Json(client)).into_response())
        }
        Err(error) => error.into_response(),
    }
}

enum RegistrationError {
    Auth(AuthError),
    InvalidRedirectUri(String),
    InvalidMetadata(String),
    PayloadTooLarge,
}

impl From<AuthError> for RegistrationError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl IntoResponse for RegistrationError {
    fn into_response(self) -> axum::response::Response {
        let (status, error, description) = match self {
            Self::InvalidRedirectUri(description) => {
                (StatusCode::BAD_REQUEST, "invalid_redirect_uri", description)
            }
            Self::InvalidMetadata(description) => (
                StatusCode::BAD_REQUEST,
                "invalid_client_metadata",
                description,
            ),
            Self::PayloadTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "invalid_client_metadata",
                "registration request exceeds the body size limit".to_string(),
            ),
            Self::Auth(error) => {
                let code = if matches!(&error, AuthError::RateLimited { .. }) {
                    "temporarily_unavailable"
                } else {
                    "server_error"
                };
                // Retain status, retry headers and diagnostic kind, but do not
                // expose storage paths or other internal failures to clients.
                let (mut parts, _) = error.into_response().into_parts();
                parts.headers.remove(header::CONTENT_LENGTH);
                let body = axum::body::Body::from(
                    serde_json::json!({
                        "error": code,
                        "error_description": "client registration is temporarily unavailable",
                    })
                    .to_string(),
                );
                return axum::response::Response::from_parts(parts, body);
            }
        };
        let mut response = (
            status,
            Json(serde_json::json!({
                "error": error,
                "error_description": description,
            })),
        )
            .into_response();
        response
            .extensions_mut()
            .insert(crate::error::AuthErrorKind(error));
        registration_cache_headers(response)
    }
}

fn registration_cache_headers(mut response: axum::response::Response) -> axum::response::Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        header::PRAGMA,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    response
}

#[cfg(test)]
mod registration_response_tests {
    use super::*;

    #[tokio::test]
    async fn registration_storage_errors_keep_diagnostics_but_hide_internal_details() {
        let secret = "private-storage-path-must-not-be-reflected";
        let error = AuthError::Storage(secret.to_string());
        let kind = error.kind();
        let response = RegistrationError::Auth(error).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::PRAGMA], "no-cache");
        assert_eq!(
            response
                .extensions()
                .get::<crate::error::AuthErrorKind>()
                .unwrap()
                .0,
            kind
        );
        let bytes = axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "server_error");
        assert!(!String::from_utf8_lossy(&bytes).contains(secret));
    }

    #[tokio::test]
    async fn non_registration_validation_errors_keep_the_product_contract() {
        let response = AuthError::Validation("ordinary validation".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["kind"], "validation_failed");
        assert_eq!(body["message"], "ordinary validation");
        assert!(body.get("error").is_none());
    }
}
