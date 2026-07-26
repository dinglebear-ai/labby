use axum::extract::{Form, State};
use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tracing::{info, warn};

use crate::error::{AuthError, AuthErrorKind};
use crate::jwt::AccessClaims;
use crate::state::AuthState;
use crate::types::AuthorizationCodeRow;
use crate::types::{RefreshTokenRow, RevocationRequest, TokenRequest, TokenResponse};
use crate::util::{
    duration_secs_usize, expires_at, fingerprint, now_unix, random_token, timestamp_usize,
};

pub async fn token(
    State(state): State<AuthState>,
    headers: axum::http::HeaderMap,
    Form(mut request): Form<TokenRequest>,
) -> Response {
    if let Some((client_id, client_secret)) = basic_client_credentials(&headers) {
        request.client_id.get_or_insert(client_id);
        request.client_secret.get_or_insert(client_secret);
    }
    if request.client_id.is_none()
        && let Some(assertion) = request.client_assertion.as_deref()
        && let Ok(data) =
            jsonwebtoken::dangerous::insecure_decode::<ClientAssertionClaims>(assertion)
    {
        request.client_id = Some(data.claims.sub);
    }
    info!(
        grant_type = %request.grant_type,
        client_id = request.client_id.as_deref().unwrap_or("<missing>"),
        requested_resource = request.resource.as_deref().unwrap_or("<default>"),
        "oauth token request received"
    );
    let response: Result<TokenResponseWithCache, TokenEndpointError> = match request
        .grant_type
        .as_str()
    {
        "authorization_code" => authorization_code_grant(state, request)
            .await
            .map(|response| TokenResponseWithCache(Json(response)))
            .map_err(TokenEndpointError::Auth),
        "refresh_token" => refresh_token_grant(state, request)
            .await
            .map(|response| TokenResponseWithCache(Json(response)))
            .map_err(TokenEndpointError::Auth),
        "client_credentials" => client_credentials_grant(state, request)
            .await
            .map(|response| TokenResponseWithCache(Json(response)))
            .map_err(TokenEndpointError::Auth),
        "urn:ietf:params:oauth:grant-type:jwt-bearer" => enterprise_managed_grant(state, request)
            .await
            .map(|response| TokenResponseWithCache(Json(response)))
            .map_err(TokenEndpointError::Auth),
        other => {
            warn!(grant_type = %other, "oauth token rejected: unsupported grant type");
            Err(TokenEndpointError::UnsupportedGrantType(other.to_string()))
        }
    };

    match response {
        Ok(response) => response.into_response(),
        Err(error) => error.into_response(),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct IdJagClaims {
    iss: String,
    sub: String,
    aud: String,
    exp: i64,
    iat: i64,
    jti: String,
    client_id: String,
    resource: String,
    scope: String,
}

async fn enterprise_managed_grant(
    state: AuthState,
    request: TokenRequest,
) -> Result<TokenResponse, AuthError> {
    let assertion = require_field(request.assertion, "assertion")?;
    let unverified = jsonwebtoken::dangerous::insecure_decode::<IdJagClaims>(&assertion)
        .map_err(|_| AuthError::InvalidGrant("invalid ID-JAG".to_string()))?
        .claims;
    let issuer = state
        .config
        .enterprise_issuers
        .iter()
        .find(|issuer| issuer.issuer.trim_end_matches('/') == unverified.iss.trim_end_matches('/'))
        .ok_or_else(|| AuthError::InvalidGrant("untrusted ID-JAG issuer".to_string()))?;
    let client_id = require_field(request.client_id, "client_id")?;
    if unverified.client_id != client_id
        || (!issuer.allowed_client_ids.is_empty()
            && !issuer
                .allowed_client_ids
                .iter()
                .any(|allowed| allowed == &client_id))
    {
        return Err(AuthError::InvalidGrant(
            "ID-JAG client_id is not authorized".to_string(),
        ));
    }
    let known_client = state.store.find_client(&client_id).await?.is_some()
        || state
            .config
            .machine_clients
            .iter()
            .any(|client| client.client_id == client_id)
        || crate::cimd::resolve_client(&state, &client_id)
            .await?
            .is_some();
    if !known_client {
        return Err(AuthError::AuthFailed(
            "enterprise client is not registered".to_string(),
        ));
    }
    let jwks = load_enterprise_jwks(&state, issuer).await?;
    let header = decode_header(&assertion)
        .map_err(|_| AuthError::InvalidGrant("invalid ID-JAG header".to_string()))?;
    if header.typ.as_deref() != Some("oauth-id-jag+jwt") {
        return Err(AuthError::InvalidGrant(
            "ID-JAG typ must be oauth-id-jag+jwt".to_string(),
        ));
    }
    let kid = header
        .kid
        .as_deref()
        .ok_or_else(|| AuthError::InvalidGrant("ID-JAG is missing kid".to_string()))?;
    let key = jwks
        .find(kid)
        .ok_or_else(|| AuthError::InvalidGrant("unknown ID-JAG signing key".to_string()))
        .and_then(|jwk| {
            DecodingKey::from_jwk(jwk)
                .map_err(|_| AuthError::InvalidGrant("invalid ID-JAG signing key".to_string()))
        })?;
    let audience = crate::metadata::public_base_url(&state);
    let mut validation = Validation::new(header.alg);
    validation.set_audience(&[audience.as_str()]);
    validation.set_issuer(&[issuer.issuer.as_str()]);
    validation.set_required_spec_claims(&[
        "exp",
        "iat",
        "iss",
        "sub",
        "aud",
        "jti",
        "client_id",
        "resource",
        "scope",
    ]);
    let claims = decode::<IdJagClaims>(&assertion, &key, &validation)
        .map_err(|_| AuthError::InvalidGrant("invalid ID-JAG".to_string()))?
        .claims;
    let now = now_unix();
    if claims.aud.trim_end_matches('/') != audience.trim_end_matches('/')
        || claims.iat > now + 60
        || claims.exp <= now
        || !state.consume_assertion_jti(&claims.iss, &claims.jti, claims.exp)
    {
        return Err(AuthError::InvalidGrant(
            "expired, replayed, or misdirected ID-JAG".to_string(),
        ));
    }
    let resource = crate::authorize::validate_resource(&state, Some(&claims.resource))?;
    if request
        .resource
        .as_deref()
        .is_some_and(|requested| requested.trim_end_matches('/') != resource)
    {
        return Err(AuthError::InvalidGrant(
            "requested resource does not match ID-JAG".to_string(),
        ));
    }
    let scope = crate::authorize::validate_scope(&state, &resource, &claims.scope)?;
    if let Some(requested) = request.scope.as_deref() {
        let requested = crate::authorize::validate_scope(&state, &resource, requested)?;
        if !requested
            .split_whitespace()
            .all(|value| scope.split_whitespace().any(|granted| granted == value))
        {
            return Err(AuthError::InvalidGrant(
                "requested scope exceeds ID-JAG grant".to_string(),
            ));
        }
        return build_token_response(&state, client_id, claims.sub, resource, requested, None);
    }
    build_token_response(&state, client_id, claims.sub, resource, scope, None)
}

async fn load_enterprise_jwks(
    state: &AuthState,
    issuer: &crate::config::EnterpriseIssuerConfig,
) -> Result<JwkSet, AuthError> {
    if let Some(value) = &issuer.jwks {
        return serde_json::from_value(value.clone())
            .map_err(|error| AuthError::Config(format!("invalid enterprise JWKS: {error}")));
    }
    let uri = issuer
        .jwks_uri
        .as_ref()
        .ok_or_else(|| AuthError::Config("enterprise issuer has no JWKS".to_string()))?;
    labby_primitives::ssrf::parse_validated_https_url(uri.as_str())
        .map_err(|error| AuthError::Config(error.to_string()))?;
    let cache_key = uri.as_str();
    let now = now_unix();
    if let Some(entry) = state.jwks_cache.get(cache_key)
        && entry.value().1 > now
    {
        return Ok(entry.value().0.clone());
    }
    let response = crate::remote::secure_get(uri)
        .await?
        .error_for_status()
        .map_err(|error| AuthError::Network(format!("fetch enterprise JWKS: {error}")))?;
    let max_age = response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.split(',').find_map(|directive| {
                directive
                    .trim()
                    .strip_prefix("max-age=")?
                    .parse::<i64>()
                    .ok()
            })
        })
        .unwrap_or(300);
    let jwks = response
        .json::<JwkSet>()
        .await
        .map_err(|error| AuthError::Decode(format!("decode enterprise JWKS: {error}")))?;
    state
        .jwks_cache
        .insert(cache_key.to_string(), (jwks.clone(), now + max_age.max(0)));
    Ok(jwks)
}

async fn client_credentials_grant(
    state: AuthState,
    request: TokenRequest,
) -> Result<TokenResponse, AuthError> {
    let client_id = require_field(request.client_id, "client_id")?;
    let client = state
        .config
        .machine_clients
        .iter()
        .find(|client| client.client_id == client_id)
        .ok_or_else(|| AuthError::AuthFailed("unknown machine client".to_string()))?;
    match (
        request.client_secret.as_deref(),
        request.client_assertion.as_deref(),
    ) {
        (Some(supplied_secret), None) => {
            let expected_secret = client.client_secret.as_deref().ok_or_else(|| {
                AuthError::AuthFailed("client secret authentication is disabled".into())
            })?;
            if !bool::from(supplied_secret.as_bytes().ct_eq(expected_secret.as_bytes())) {
                return Err(AuthError::AuthFailed(
                    "invalid machine client credentials".to_string(),
                ));
            }
        }
        (None, Some(assertion)) => {
            if request.client_assertion_type.as_deref()
                != Some("urn:ietf:params:oauth:client-assertion-type:jwt-bearer")
            {
                return Err(AuthError::Validation(
                    "unsupported client_assertion_type".to_string(),
                ));
            }
            let jwks: JwkSet = serde_json::from_value(client.jwks.clone().ok_or_else(|| {
                AuthError::AuthFailed("private_key_jwt authentication is disabled".into())
            })?)
            .map_err(|error| AuthError::Config(format!("invalid machine client JWKS: {error}")))?;
            validate_client_assertion(&state, assertion, &client_id, &jwks)?;
        }
        _ => {
            return Err(AuthError::AuthFailed(
                "exactly one machine client authentication method is required".to_string(),
            ));
        }
    }
    let resource = crate::authorize::validate_resource(&state, request.resource.as_deref())?;
    let requested_scope = request.scope.as_deref().unwrap_or_else(|| {
        if client.scopes.is_empty() {
            state.config.default_scope.as_str()
        } else {
            ""
        }
    });
    let requested_scope = if requested_scope.is_empty() {
        client.scopes.join(" ")
    } else {
        requested_scope.to_string()
    };
    let scope = crate::authorize::validate_scope(&state, &resource, &requested_scope)?;
    if !client.scopes.is_empty()
        && !scope
            .split_whitespace()
            .all(|requested| client.scopes.iter().any(|allowed| allowed == requested))
    {
        return Err(AuthError::AuthFailed(
            "requested scope exceeds machine client grant".to_string(),
        ));
    }
    build_token_response(&state, client_id.clone(), client_id, resource, scope, None)
}

#[derive(Debug, Serialize, Deserialize)]
struct ClientAssertionClaims {
    iss: String,
    sub: String,
    aud: String,
    exp: i64,
    iat: i64,
    jti: String,
}

fn validate_client_assertion(
    state: &AuthState,
    assertion: &str,
    client_id: &str,
    jwks: &JwkSet,
) -> Result<(), AuthError> {
    let header = decode_header(assertion)
        .map_err(|_| AuthError::AuthFailed("invalid client assertion".to_string()))?;
    let kid = header
        .kid
        .as_deref()
        .ok_or_else(|| AuthError::AuthFailed("client assertion is missing kid".to_string()))?;
    let jwk = jwks
        .find(kid)
        .ok_or_else(|| AuthError::AuthFailed("unknown client assertion key".to_string()))?;
    let key = DecodingKey::from_jwk(jwk)
        .map_err(|_| AuthError::AuthFailed("invalid client assertion key".to_string()))?;
    let audience = format!("{}/token", crate::metadata::public_base_url(state));
    let mut validation = Validation::new(header.alg);
    validation.set_audience(&[audience.as_str()]);
    validation.set_issuer(&[client_id]);
    validation.set_required_spec_claims(&["exp", "iat", "iss", "sub", "aud", "jti"]);
    let claims = decode::<ClientAssertionClaims>(assertion, &key, &validation)
        .map_err(|_| AuthError::AuthFailed("invalid client assertion".to_string()))?
        .claims;
    let now = now_unix();
    if claims.iss != client_id
        || claims.sub != client_id
        || claims.aud != audience
        || claims.iat > now + 60
        || claims.exp <= now
        || !state.consume_assertion_jti(&claims.iss, &claims.jti, claims.exp)
    {
        return Err(AuthError::AuthFailed(
            "invalid or replayed client assertion".to_string(),
        ));
    }
    Ok(())
}

fn basic_client_credentials(headers: &axum::http::HeaderMap) -> Option<(String, String)> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = raw.strip_prefix("Basic ")?;
    let decoded = STANDARD.decode(encoded).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (client_id, client_secret) = decoded.split_once(':')?;
    Some((client_id.to_string(), client_secret.to_string()))
}

pub async fn revoke(
    State(state): State<AuthState>,
    Form(request): Form<RevocationRequest>,
) -> Response {
    let token_id = fingerprint(&request.token);
    match state.store.find_refresh_token(&request.token).await {
        Ok(Some(row))
            if request
                .client_id
                .as_deref()
                .is_none_or(|client_id| client_id == row.client_id) =>
        {
            if let Err(error) = state.store.revoke_refresh_token(&request.token).await {
                return TokenEndpointError::Auth(error).into_response();
            }
            info!(token_id = %token_id, client_id = %row.client_id, "oauth refresh token revoked");
            apply_token_cache_headers(StatusCode::OK.into_response())
        }
        Ok(_) => apply_token_cache_headers(StatusCode::OK.into_response()),
        Err(error) => TokenEndpointError::Auth(error).into_response(),
    }
}

enum TokenEndpointError {
    Auth(AuthError),
    UnsupportedGrantType(String),
}

impl TokenEndpointError {
    fn oauth_error(&self) -> &'static str {
        match self {
            Self::Auth(AuthError::InvalidGrant(_)) => "invalid_grant",
            Self::UnsupportedGrantType(_) => "unsupported_grant_type",
            Self::Auth(AuthError::AuthFailed(_) | AuthError::InvalidAccessToken) => {
                "invalid_client"
            }
            Self::Auth(AuthError::RateLimited { .. }) => "temporarily_unavailable",
            Self::Auth(AuthError::Validation(_)) => "invalid_request",
            Self::Auth(
                AuthError::Config(_)
                | AuthError::Storage(_)
                | AuthError::Network(_)
                | AuthError::Server(_)
                | AuthError::Decode(_)
                | AuthError::InsecurePermissions { .. },
            ) => "server_error",
        }
    }

    fn log_kind(&self) -> &'static str {
        match self {
            Self::Auth(error) => error.kind(),
            Self::UnsupportedGrantType(_) => "unsupported_grant_type",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::Auth(AuthError::InvalidGrant(_) | AuthError::Validation(_))
            | Self::UnsupportedGrantType(_) => StatusCode::BAD_REQUEST,
            Self::Auth(AuthError::AuthFailed(_) | AuthError::InvalidAccessToken) => {
                StatusCode::UNAUTHORIZED
            }
            Self::Auth(AuthError::RateLimited { .. }) => StatusCode::TOO_MANY_REQUESTS,
            Self::Auth(
                AuthError::Config(_)
                | AuthError::Storage(_)
                | AuthError::Network(_)
                | AuthError::Server(_)
                | AuthError::Decode(_)
                | AuthError::InsecurePermissions { .. },
            ) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn description(&self) -> String {
        match self {
            Self::Auth(error) => error.to_string(),
            Self::UnsupportedGrantType(grant_type) => {
                format!("unsupported grant_type `{grant_type}`")
            }
        }
    }

    fn retry_after_ms(&self) -> Option<u64> {
        match self {
            Self::Auth(AuthError::RateLimited { retry_after_ms, .. }) => Some(*retry_after_ms),
            _ => None,
        }
    }
}

impl IntoResponse for TokenEndpointError {
    fn into_response(self) -> Response {
        let status = self.status();
        let log_kind = self.log_kind();
        let retry_after_ms = self.retry_after_ms();
        let body = Json(serde_json::json!({
            "error": self.oauth_error(),
            "error_description": self.description(),
        }));
        let mut response = (status, body).into_response();
        response.extensions_mut().insert(AuthErrorKind(log_kind));
        if let Some(retry_after_ms) = retry_after_ms
            && let Ok(value) = HeaderValue::from_str(&(retry_after_ms / 1_000).max(1).to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        apply_token_cache_headers(response)
    }
}

struct TokenResponseWithCache(Json<TokenResponse>);

impl IntoResponse for TokenResponseWithCache {
    fn into_response(self) -> Response {
        apply_token_cache_headers(self.0.into_response())
    }
}

fn apply_token_cache_headers(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

async fn authorization_code_grant(
    state: AuthState,
    request: TokenRequest,
) -> Result<TokenResponse, AuthError> {
    let requested_resource = request
        .resource
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string());
    crate::authorize::validate_resource(&state, request.resource.as_deref())?;
    let code = require_field(request.code, "code")?;
    let client_id = require_field(request.client_id, "client_id")?;
    let redirect_uri = require_field(request.redirect_uri, "redirect_uri")?;
    let code_verifier = require_field(request.code_verifier, "code_verifier")?;
    let auth_code_id = fingerprint(&code);
    info!(
        grant_type = "authorization_code",
        client_id = %client_id,
        auth_code_id = %auth_code_id,
        redirect_uri = %redirect_uri,
        requested_resource = requested_resource.as_deref().unwrap_or("<authorization-code-resource>"),
        "oauth authorization_code grant redeeming local code"
    );

    let row = state.store.redeem_auth_code(&code).await.map_err(|error| {
        warn!(
            auth_code_id = %auth_code_id,
            client_id = %client_id,
            error = %error,
            "oauth token rejected: authorization code is invalid, expired, or already redeemed"
        );
        error
    })?;
    validate_authorization_code_row(
        &row,
        &client_id,
        &redirect_uri,
        &code_verifier,
        &auth_code_id,
    )?;
    if let Some(requested_resource) = requested_resource
        && requested_resource != row.resource
    {
        warn!(
            auth_code_id = %auth_code_id,
            requested_resource = %requested_resource,
            stored_resource = %row.resource,
            "oauth token rejected: resource does not match authorization code"
        );
        return Err(AuthError::InvalidGrant(
            "resource does not match the authorization code".to_string(),
        ));
    }

    let refresh_token = if let Some(provider_refresh_token) = row.provider_refresh_token {
        let refresh_token = random_token(24)?;
        let created_at = now_unix();
        state
            .store
            .upsert_refresh_token(RefreshTokenRow {
                refresh_token: refresh_token.clone(),
                client_id: row.client_id.clone(),
                subject: row.subject.clone(),
                resource: row.resource.clone(),
                scope: row.scope.clone(),
                provider_refresh_token: Some(provider_refresh_token),
                created_at,
                expires_at: expires_at(
                    created_at,
                    state.config.refresh_token_ttl,
                    &format!("{}_AUTH_REFRESH_TOKEN_TTL_SECS", state.config.env_prefix),
                )?,
            })
            .await?;
        info!(
            grant_type = "authorization_code",
            client_id = %row.client_id,
            auth_code_id = %auth_code_id,
            subject_id = %fingerprint(&row.subject),
            resource = %row.resource,
            scope = %row.scope,
            "oauth authorization_code grant issued lab access token and refresh token"
        );
        Some(refresh_token)
    } else {
        info!(
            grant_type = "authorization_code",
            client_id = %row.client_id,
            auth_code_id = %auth_code_id,
            subject_id = %fingerprint(&row.subject),
            resource = %row.resource,
            scope = %row.scope,
            "oauth authorization_code grant issued lab access token without refresh token"
        );
        None
    };

    let resource = if row.resource.trim().is_empty() {
        crate::metadata::canonical_resource_url(&state)
    } else {
        row.resource
    };
    build_token_response(
        &state,
        row.client_id,
        row.subject,
        resource,
        row.scope,
        refresh_token,
    )
}

async fn refresh_token_grant(
    state: AuthState,
    request: TokenRequest,
) -> Result<TokenResponse, AuthError> {
    let requested_resource = request
        .resource
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| crate::authorize::validate_resource(&state, Some(value)))
        .transpose()?;
    let client_id = require_field(request.client_id, "client_id")?;
    let refresh_token = require_field(request.refresh_token, "refresh_token")?;
    let refresh_token_id = fingerprint(&refresh_token);
    info!(
        grant_type = "refresh_token",
        client_id = %client_id,
        refresh_token_id = %refresh_token_id,
        requested_resource = requested_resource.as_deref().unwrap_or("<refresh-token-resource>"),
        "oauth refresh_token grant received"
    );
    let stored = state
        .store
        .find_refresh_token(&refresh_token)
        .await?
        .ok_or_else(|| {
            warn!(
                refresh_token_id = %refresh_token_id,
                client_id = %client_id,
                "oauth token rejected: unknown or expired refresh token"
            );
            AuthError::InvalidGrant("unknown refresh_token".to_string())
        })?;
    if stored.client_id != client_id {
        warn!(
            refresh_token_id = %refresh_token_id,
            requested_client_id = %client_id,
            stored_client_id = %stored.client_id,
            "oauth token rejected: client_id does not match refresh token"
        );
        return Err(AuthError::InvalidGrant(
            "client_id does not match the refresh token".to_string(),
        ));
    }
    let stored_resource = if stored.resource.trim().is_empty() {
        crate::metadata::canonical_resource_url(&state)
    } else {
        stored.resource.clone()
    };
    if let Some(requested_resource) = requested_resource
        && requested_resource != stored_resource
    {
        warn!(
            refresh_token_id = %refresh_token_id,
            requested_resource = %requested_resource,
            stored_resource = %stored_resource,
            "oauth token rejected: resource does not match refresh token"
        );
        return Err(AuthError::InvalidGrant(
            "resource does not match the refresh token".to_string(),
        ));
    }

    let Some(provider_refresh_token) = stored.provider_refresh_token.clone() else {
        warn!(
            refresh_token_id = %refresh_token_id,
            client_id = %stored.client_id,
            "oauth token rejected: refresh token is not backed by an upstream refresh token"
        );
        return Err(AuthError::InvalidGrant(
            "refresh token is not backed by an upstream refresh token".to_string(),
        ));
    };

    // Refresh upstream before consuming the local token. If Google or id-token
    // verification fails, the client can retry the same local refresh token
    // instead of being stranded with an unreturned replacement.
    let google = state.google.refresh(&provider_refresh_token).await?;

    let now = now_unix();
    let refreshed_expires_at = expires_at(
        now,
        state.config.refresh_token_ttl,
        &format!("{}_AUTH_REFRESH_TOKEN_TTL_SECS", state.config.env_prefix),
    )?;
    let next_provider_refresh_token = google
        .refresh_token
        .clone()
        .unwrap_or_else(|| provider_refresh_token.clone());
    // Re-apply admin elevation in case this refresh token was originally
    // issued before elevation was wired in, or before the user's email was
    // on the allowlist.  elevate_scope_for_allowed_user is idempotent — if
    // the scope already contains the admin token it is left unchanged.
    let elevated_scope = crate::authorize::elevate_scope_for_allowed_user(
        &stored.scope,
        &state.config.default_scope,
    );

    let replacement_refresh_token = random_token(24)?;
    let replacement = RefreshTokenRow {
        refresh_token: replacement_refresh_token.clone(),
        client_id: stored.client_id.clone(),
        subject: google.subject.clone(),
        resource: stored_resource.clone(),
        scope: elevated_scope.clone(),
        provider_refresh_token: Some(next_provider_refresh_token),
        created_at: now,
        expires_at: refreshed_expires_at,
    };
    state
        .store
        .rotate_refresh_token(&refresh_token, replacement)
        .await?
        .ok_or_else(|| AuthError::InvalidGrant("refresh token was already used".to_string()))?;

    info!(
        grant_type = "refresh_token",
        client_id = %stored.client_id,
        refresh_token_id = %refresh_token_id,
        subject_id = %fingerprint(&google.subject),
        resource = %stored_resource,
        scope = %elevated_scope,
        "oauth refresh_token grant rotated local token and issued new access token"
    );

    build_token_response(
        &state,
        stored.client_id,
        google.subject,
        stored_resource,
        elevated_scope,
        Some(replacement_refresh_token),
    )
}

fn build_token_response(
    state: &AuthState,
    client_id: String,
    subject: String,
    resource: String,
    scope: String,
    refresh_token: Option<String>,
) -> Result<TokenResponse, AuthError> {
    let issuer = crate::metadata::public_base_url(state);
    let now = timestamp_usize(now_unix(), "current unix timestamp")?;
    let access_token_ttl = duration_secs_usize(
        state.config.access_token_ttl,
        &format!("{}_AUTH_ACCESS_TOKEN_TTL_SECS", state.config.env_prefix),
    )?;
    let subject_id = fingerprint(&subject);
    let access_token = state.signing_keys.issue_access_token(&AccessClaims {
        iss: issuer,
        sub: subject.clone(),
        aud: resource.clone(),
        exp: now.checked_add(access_token_ttl).ok_or_else(|| {
            AuthError::Config(format!(
                "{}_AUTH_ACCESS_TOKEN_TTL_SECS exceeds supported range",
                state.config.env_prefix
            ))
        })?,
        iat: now,
        jti: random_token(18)?,
        scope: scope.clone(),
        azp: client_id.clone(),
    })?;
    info!(
        client_id = %client_id,
        subject_id = %subject_id,
        resource = %resource,
        scope = %scope,
        expires_in_secs = state.config.access_token_ttl.as_secs(),
        refresh_token_issued = refresh_token.is_some(),
        "oauth token response minted access token"
    );
    Ok(TokenResponse {
        access_token,
        token_type: "Bearer".to_string(),
        expires_in: state.config.access_token_ttl.as_secs(),
        refresh_token,
        scope,
    })
}

fn require_field(value: Option<String>, field: &str) -> Result<String, AuthError> {
    value.ok_or_else(|| AuthError::Validation(format!("missing `{field}` parameter")))
}

fn pkce_challenge(code_verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()))
}

fn validate_authorization_code_row(
    row: &AuthorizationCodeRow,
    client_id: &str,
    redirect_uri: &str,
    code_verifier: &str,
    auth_code_id: &str,
) -> Result<(), AuthError> {
    if row.client_id != client_id {
        warn!(
            auth_code_id = %auth_code_id,
            requested_client_id = %client_id,
            stored_client_id = %row.client_id,
            "oauth token rejected: client_id does not match authorization code"
        );
        return Err(AuthError::InvalidGrant(
            "client_id does not match the authorization code".to_string(),
        ));
    }
    if row.redirect_uri != redirect_uri {
        warn!(
            auth_code_id = %auth_code_id,
            requested_redirect_uri = %redirect_uri,
            stored_redirect_uri = %row.redirect_uri,
            "oauth token rejected: redirect_uri does not match authorization code"
        );
        return Err(AuthError::InvalidGrant(
            "redirect_uri does not match the authorization code".to_string(),
        ));
    }
    if row.code_challenge_method != "S256" {
        warn!(
            auth_code_id = %auth_code_id,
            code_challenge_method = %row.code_challenge_method,
            "oauth token rejected: unsupported PKCE method on authorization code"
        );
        return Err(AuthError::InvalidGrant(
            "authorization code uses an unsupported PKCE method".to_string(),
        ));
    }
    if !bool::from(
        pkce_challenge(code_verifier)
            .as_bytes()
            .ct_eq(row.code_challenge.as_bytes()),
    ) {
        warn!(
            auth_code_id = %auth_code_id,
            client_id = %row.client_id,
            "oauth token rejected: code_verifier did not match authorization code"
        );
        return Err(AuthError::InvalidGrant(
            "code_verifier does not match the authorization code".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    use jsonwebtoken::dangerous::insecure_decode;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use tower::util::ServiceExt;
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::google::GoogleProvider;
    use crate::routes::router;
    use crate::state::AuthState;

    use super::super::authorize::tests::{
        test_auth_state_with_mock_google, test_auth_state_with_registered_client,
    };

    async fn test_auth_state_with_failing_google_refresh() -> AuthState {
        let state = test_auth_state_with_registered_client().await;
        let server = Box::leak(Box::new(MockServer::start().await));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
                "error": "temporarily_unavailable"
            })))
            .mount(server)
            .await;
        let google = GoogleProvider::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
        )
        .unwrap()
        .with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        );
        AuthState::for_tests(
            (*state.config).clone(),
            state.store.clone(),
            (*state.signing_keys).clone(),
            google,
        )
    }

    #[tokio::test]
    async fn token_endpoint_mints_lab_jwt_and_refresh_token() {
        let state = test_auth_state_with_registered_client().await;
        seed_authorization_code(&state).await;
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(
                        header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(Body::from("grant_type=authorization_code&code=lab-code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&code_verifier=verifier"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            response
                .headers()
                .get(header::PRAGMA)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache")
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["access_token"].is_string());
        assert!(json["refresh_token"].is_string());
        let access_token = json["access_token"].as_str().expect("access token string");
        let claims = insecure_decode::<crate::jwt::AccessClaims>(access_token)
            .expect("decode access token")
            .claims;
        assert_eq!(claims.aud, "https://lab.example.com/mcp");
    }

    #[tokio::test]
    async fn client_credentials_secret_mints_audience_bound_machine_token() {
        let base = test_auth_state_with_registered_client().await;
        let mut config = (*base.config).clone();
        config.machine_clients = vec![crate::config::MachineClientConfig {
            client_id: "ci-agent".to_string(),
            client_secret: Some("correct-horse".to_string()),
            jwks: None,
            scopes: vec!["lab".to_string()],
        }];
        let state = AuthState::for_tests(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            (*base.google).clone(),
        );
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(
                        header::AUTHORIZATION,
                        format!(
                            "Basic {}",
                            base64::engine::general_purpose::STANDARD
                                .encode("ci-agent:correct-horse")
                        ),
                    )
                    .header(
                        header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(Body::from(
                        "grant_type=client_credentials&resource=https%3A%2F%2Flab.example.com%2Fmcp&scope=lab",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let claims =
            insecure_decode::<crate::jwt::AccessClaims>(json["access_token"].as_str().unwrap())
                .unwrap()
                .claims;
        assert_eq!(claims.sub, "ci-agent");
        assert_eq!(claims.aud, "https://lab.example.com/mcp");
        assert_eq!(claims.scope, "lab");
        assert!(json.get("refresh_token").is_none());
    }

    #[tokio::test]
    async fn revocation_endpoint_invalidates_refresh_token_and_is_idempotent() {
        let state = test_auth_state_with_registered_client().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "revoke-me".to_string(),
                client_id: "client".to_string(),
                subject: "subject".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_refresh_token: Some("provider-refresh".to_string()),
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let app = router(state.clone());
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(form_request(
                    "/revoke",
                    form(&[("token", "revoke-me"), ("client_id", "client")]),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        assert!(
            state
                .store
                .find_refresh_token("revoke-me")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn private_key_jwt_client_credentials_rejects_assertion_replay() {
        let base = test_auth_state_with_registered_client().await;
        let (encoding_key, jwks) = assertion_key();
        let mut config = (*base.config).clone();
        config.machine_clients = vec![crate::config::MachineClientConfig {
            client_id: "jwt-agent".to_string(),
            client_secret: None,
            jwks: Some(jwks),
            scopes: vec!["lab".to_string()],
        }];
        let state = AuthState::for_tests(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            (*base.google).clone(),
        );
        let now = crate::util::now_unix();
        let assertion = sign_assertion(
            &encoding_key,
            &super::ClientAssertionClaims {
                iss: "jwt-agent".to_string(),
                sub: "jwt-agent".to_string(),
                aud: "https://lab.example.com/token".to_string(),
                exp: now + 300,
                iat: now,
                jti: "one-shot".to_string(),
            },
            None,
        );
        let body = form(&[
            ("grant_type", "client_credentials"),
            (
                "client_assertion_type",
                "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
            ),
            ("client_assertion", &assertion),
            ("resource", "https://lab.example.com/mcp"),
            ("scope", "lab"),
        ]);
        let app = router(state);
        let first = app
            .clone()
            .oneshot(form_request("/token", body.clone()))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let replay = app.oneshot(form_request("/token", body)).await.unwrap();
        assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn enterprise_id_jag_mints_user_token_bound_to_claimed_resource() {
        let base = test_auth_state_with_registered_client().await;
        let (encoding_key, jwks) = assertion_key();
        let mut config = (*base.config).clone();
        config.enterprise_issuers = vec![crate::config::EnterpriseIssuerConfig {
            issuer: "https://idp.example.com".to_string(),
            jwks_uri: None,
            jwks: Some(jwks),
            allowed_client_ids: vec!["client".to_string()],
        }];
        let state = AuthState::for_tests(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            (*base.google).clone(),
        );
        let now = crate::util::now_unix();
        let assertion = sign_assertion(
            &encoding_key,
            &super::IdJagClaims {
                iss: "https://idp.example.com".to_string(),
                sub: "employee-42".to_string(),
                aud: "https://lab.example.com".to_string(),
                exp: now + 300,
                iat: now,
                jti: "id-jag-1".to_string(),
                client_id: "client".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
            },
            Some("oauth-id-jag+jwt"),
        );
        let response = router(state)
            .oneshot(form_request(
                "/token",
                form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                    ("assertion", &assertion),
                    ("client_id", "client"),
                ]),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let claims =
            insecure_decode::<crate::jwt::AccessClaims>(json["access_token"].as_str().unwrap())
                .unwrap()
                .claims;
        assert_eq!(claims.sub, "employee-42");
        assert_eq!(claims.aud, "https://lab.example.com/mcp");
    }

    fn assertion_key() -> (EncodingKey, serde_json::Value) {
        let key = SigningKey::from_bytes(&[7_u8; 32]);
        let der = key.to_pkcs8_der().unwrap();
        let encoding = EncodingKey::from_ed_der(der.as_bytes());
        let jwks = serde_json::json!({
            "keys": [{
                "kty": "OKP",
                "use": "sig",
                "alg": "EdDSA",
                "kid": "assertion-key",
                "crv": "Ed25519",
                "x": URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes())
            }]
        });
        (encoding, jwks)
    }

    fn sign_assertion<T: serde::Serialize>(
        key: &EncodingKey,
        claims: &T,
        typ: Option<&str>,
    ) -> String {
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some("assertion-key".to_string());
        header.typ = typ.map(ToOwned::to_owned);
        encode(&header, claims, key).unwrap()
    }

    fn form(fields: &[(&str, &str)]) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.extend_pairs(fields.iter().copied());
        serializer.finish()
    }

    fn form_request(uri: &str, body: String) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn token_endpoint_omits_refresh_token_without_upstream_refresh_capability() {
        let state = test_auth_state_with_registered_client().await;
        seed_authorization_code_without_provider_refresh(&state).await;
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(
                        header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(Body::from("grant_type=authorization_code&code=lab-code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&code_verifier=verifier"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            response
                .headers()
                .get(header::PRAGMA)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache")
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["access_token"].is_string());
        assert!(json.get("refresh_token").is_none());
    }

    #[tokio::test]
    async fn token_endpoint_redeems_authorization_code_once() {
        let state = test_auth_state_with_registered_client().await;
        seed_authorization_code(&state).await;
        let app = router(state);
        let (a, b) = tokio::join!(
            app.clone().oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(
                        header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(Body::from("grant_type=authorization_code&code=lab-code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&code_verifier=verifier"))
                    .unwrap()
            ),
            app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(
                        header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(Body::from("grant_type=authorization_code&code=lab-code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&code_verifier=verifier"))
                    .unwrap()
            )
        );
        let a = a.unwrap();
        let b = b.unwrap();
        assert!(a.status() == StatusCode::OK || b.status() == StatusCode::OK);
        assert!(a.status() == StatusCode::BAD_REQUEST || b.status() == StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn token_endpoint_rejects_expired_authorization_code() {
        let state = test_auth_state_with_registered_client().await;
        seed_authorization_code_with_expiry(&state, crate::util::now_unix() - 1).await;
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(
                        header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(Body::from("grant_type=authorization_code&code=lab-code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&code_verifier=verifier"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            response
                .headers()
                .get(header::PRAGMA)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache")
        );
    }

    #[tokio::test]
    async fn token_endpoint_errors_use_oauth_error_shape() {
        let state = test_auth_state_with_registered_client().await;
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "grant_type=refresh_token&refresh_token=missing&client_id=client",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            response
                .headers()
                .get(header::PRAGMA)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache")
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "invalid_grant");
        assert_eq!(json["error_description"], "unknown refresh_token");
        assert!(json.get("kind").is_none());
        assert!(json.get("message").is_none());
    }

    #[tokio::test]
    async fn token_endpoint_unsupported_grant_type_uses_oauth_error_shape() {
        let state = test_auth_state_with_registered_client().await;
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("grant_type=password&client_id=client"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "unsupported_grant_type");
        assert_eq!(
            json["error_description"],
            "unsupported grant_type `password`"
        );
    }

    #[tokio::test]
    async fn token_endpoint_refresh_grant_sets_cache_headers() {
        let state = test_auth_state_with_mock_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "refresh-token".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_refresh_token: Some("provider-refresh".to_string()),
                created_at: crate::util::now_unix() - 60,
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "grant_type=refresh_token&refresh_token=refresh-token&client_id=client",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            response
                .headers()
                .get(header::PRAGMA)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache")
        );
    }

    #[tokio::test]
    async fn token_endpoint_refresh_grant_preserves_stored_resource_when_omitted() {
        let state = test_auth_state_with_mock_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "refresh-token".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: "https://mcp.example.com/syslog".to_string(),
                scope: "mcp:read mcp:write".to_string(),
                provider_refresh_token: Some("provider-refresh".to_string()),
                created_at: crate::util::now_unix() - 60,
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "grant_type=refresh_token&refresh_token=refresh-token&client_id=client",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let access_token = json["access_token"].as_str().expect("access token string");
        let claims = insecure_decode::<crate::jwt::AccessClaims>(access_token)
            .expect("decode access token")
            .claims;
        assert_eq!(claims.aud, "https://mcp.example.com/syslog");
        assert_eq!(claims.scope, "mcp:read mcp:write lab:admin");
    }

    #[tokio::test]
    async fn token_endpoint_rejects_mismatched_resource_parameter() {
        let state = test_auth_state_with_registered_client().await;
        seed_authorization_code(&state).await;
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(
                        header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(Body::from("grant_type=authorization_code&code=lab-code&client_id=client&resource=https%3A%2F%2Fother.example.com%2Fmcp&redirect_uri=http://127.0.0.1:7777/callback&code_verifier=verifier"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn token_endpoint_rejects_expired_refresh_token() {
        let state = test_auth_state_with_registered_client().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "refresh-token".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_refresh_token: Some("provider-refresh".to_string()),
                created_at: crate::util::now_unix() - 3600,
                expires_at: crate::util::now_unix() - 1,
            })
            .await
            .unwrap();
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "grant_type=refresh_token&refresh_token=refresh-token&client_id=client",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            response
                .headers()
                .get(header::PRAGMA)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache")
        );
    }

    #[tokio::test]
    async fn token_endpoint_rejects_refresh_token_client_mismatch() {
        let state = test_auth_state_with_registered_client().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "refresh-token".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_refresh_token: Some("provider-refresh".to_string()),
                created_at: crate::util::now_unix() - 60,
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(
                        header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(Body::from(
                        "grant_type=refresh_token&refresh_token=refresh-token&client_id=other-client",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            response
                .headers()
                .get(header::PRAGMA)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache")
        );
    }

    #[tokio::test]
    async fn token_endpoint_rejects_refresh_token_without_upstream_refresh_capability() {
        let state = test_auth_state_with_registered_client().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "refresh-token".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_refresh_token: None,
                created_at: crate::util::now_unix() - 60,
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "grant_type=refresh_token&refresh_token=refresh-token&client_id=client",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    async fn seed_authorization_code(state: &AuthState) {
        seed_authorization_code_with_expiry(state, 4_102_444_800).await;
    }

    async fn seed_authorization_code_without_provider_refresh(state: &AuthState) {
        state
            .store
            .insert_auth_code(crate::types::AuthorizationCodeRow {
                code: "lab-code".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                code_challenge: super::pkce_challenge("verifier"),
                code_challenge_method: "S256".to_string(),
                provider_refresh_token: None,
                created_at: 1_700_000_000,
                expires_at: 4_102_444_800,
            })
            .await
            .unwrap();
    }

    async fn seed_authorization_code_with_expiry(state: &AuthState, expires_at: i64) {
        state
            .store
            .insert_auth_code(crate::types::AuthorizationCodeRow {
                code: "lab-code".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                code_challenge: super::pkce_challenge("verifier"),
                code_challenge_method: "S256".to_string(),
                provider_refresh_token: Some("provider-refresh".to_string()),
                created_at: 1_700_000_000,
                expires_at,
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn refresh_grant_rotates_local_token_on_success() {
        let state = test_auth_state_with_mock_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "original-token".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: String::new(),
                scope: "lab".to_string(),
                provider_refresh_token: Some("provider-refresh".to_string()),
                created_at: crate::util::now_unix() - 60,
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let app = router(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "grant_type=refresh_token&refresh_token=original-token&client_id=client",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let new_token = json["refresh_token"].as_str().expect("refresh_token");
        assert_ne!(new_token, "original-token", "refresh token must rotate");
        assert!(
            state
                .store
                .find_refresh_token("original-token")
                .await
                .unwrap()
                .is_none(),
            "rotated refresh token must be revoked"
        );
        assert!(
            state
                .store
                .find_refresh_token(new_token)
                .await
                .unwrap()
                .is_some(),
            "replacement refresh token must be usable"
        );
    }

    #[tokio::test]
    async fn refresh_grant_preserves_original_token_when_upstream_refresh_fails() {
        let state = test_auth_state_with_failing_google_refresh().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "recoverable-token".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: String::new(),
                scope: "lab".to_string(),
                provider_refresh_token: Some("provider-refresh".to_string()),
                created_at: crate::util::now_unix() - 60,
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let app = router(state.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "grant_type=refresh_token&refresh_token=recoverable-token&client_id=client",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(response.status(), StatusCode::OK);
        assert!(
            state
                .store
                .find_refresh_token("recoverable-token")
                .await
                .unwrap()
                .is_some(),
            "local refresh token must remain usable after upstream refresh failure"
        );
    }

    #[tokio::test]
    async fn refresh_grant_elevates_stale_scope_to_admin() {
        // Simulate a refresh token that was issued before elevation was wired in,
        // storing only the base scope ("lab") without "lab:admin".  The refresh
        // grant must re-apply elevate_scope_for_allowed_user so the new access
        // token carries "lab:admin".
        let state = test_auth_state_with_mock_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "stale-token".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: String::new(),
                scope: "lab".to_string(), // stale — no lab:admin
                provider_refresh_token: Some("provider-refresh".to_string()),
                created_at: crate::util::now_unix() - 60,
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let app = router(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "grant_type=refresh_token&refresh_token=stale-token&client_id=client",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Decode the access token and verify the scope was elevated.
        let access_token = json["access_token"].as_str().expect("access_token");
        let claims = state
            .signing_keys
            .validate_access_token_with_issuer(
                access_token,
                "https://lab.example.com/mcp",
                "https://lab.example.com",
            )
            .expect("access token must be valid");
        let scopes: Vec<&str> = claims.scope.split_whitespace().collect();
        assert!(
            scopes.contains(&"lab:admin"),
            "elevated access token must contain lab:admin, got: {:?}",
            scopes
        );
    }

    #[tokio::test]
    async fn refresh_grant_rejects_reuse_of_rotated_local_token() {
        let state = test_auth_state_with_mock_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "once-only-token".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: String::new(),
                scope: "lab".to_string(),
                provider_refresh_token: Some("provider-refresh".to_string()),
                created_at: crate::util::now_unix() - 60,
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let app = router(state);
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "grant_type=refresh_token&refresh_token=once-only-token&client_id=client",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let replay = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "grant_type=refresh_token&refresh_token=once-only-token&client_id=client",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            replay.status(),
            StatusCode::BAD_REQUEST,
            "a rotated refresh token must not be reusable"
        );
    }
}
