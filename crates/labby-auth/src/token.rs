use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Extension, Form, State};
use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use jsonwebtoken::jwk::{Jwk, JwkSet, KeyAlgorithm};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tracing::{debug, info, warn};

use crate::error::AuthError;
use crate::google::{GoogleExchange, merge_google_scopes};
use crate::jwt::AccessClaims;
use crate::state::AuthState;
use crate::types::{RefreshTokenRow, RevocationRequest, TokenRequest, TokenResponse};
use crate::util::{
    duration_secs_usize, expires_at, fingerprint, now_unix, random_token, timestamp_usize,
};

mod refresh;
mod refresh_replay;
mod response;
use refresh::refresh_token_grant;
#[cfg(test)]
use refresh::{
    RefreshClaimLease, RefreshClaimLeaseObserver, claim_refresh_after_subject_lock,
    refresh_lock_waiter_counter,
};
use refresh_replay::{await_cached_refresh_response, cached_refresh_response};
use response::{TokenEndpointError, TokenResponseWithCache, apply_token_cache_headers};

pub async fn token(
    State(state): State<AuthState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: axum::http::HeaderMap,
    Form(mut request): Form<TokenRequest>,
) -> Response {
    let remote_ip = connect_info
        .map(|Extension(ConnectInfo(address))| address.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    if let Err(error) = state.check_token_rate_limit(remote_ip).await {
        return TokenEndpointError::Auth(error).into_response();
    }
    match basic_client_credentials(&headers) {
        Ok(Some((client_id, client_secret))) => {
            if request.client_id.is_some()
                || request.client_secret.is_some()
                || request.client_assertion.is_some()
            {
                return TokenEndpointError::Auth(AuthError::AuthFailed(
                    "invalid client credentials".to_string(),
                ))
                .into_response();
            }
            request.client_id = Some(client_id);
            request.client_secret = Some(client_secret);
        }
        Ok(None) => {}
        Err(error) => return TokenEndpointError::Auth(error).into_response(),
    }
    if request.client_id.is_none()
        && let Some(assertion) = request.client_assertion.as_deref()
        && let Ok(data) =
            jsonwebtoken::dangerous::insecure_decode::<ClientAssertionClaims>(assertion)
    {
        request.client_id = Some(data.claims.sub);
    }
    debug!(
        grant_type_id = %fingerprint(&request.grant_type),
        client_id = %request.client_id.as_deref().map(fingerprint).unwrap_or_else(|| "<missing>".to_string()),
        requested_resource_id = %request.resource.as_deref().map(fingerprint).unwrap_or_else(|| "<default>".to_string()),
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
            warn!(grant_type_id = %fingerprint(other), "oauth token rejected: unsupported grant type");
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
    let header = decode_header(&assertion)
        .map_err(|_| AuthError::InvalidGrant("invalid ID-JAG header".to_string()))?;
    ensure_allowed_algorithm(header.alg)
        .map_err(|_| AuthError::InvalidGrant("unsupported ID-JAG algorithm".to_string()))?;
    if header.typ.as_deref() != Some("oauth-id-jag+jwt") {
        return Err(AuthError::InvalidGrant(
            "ID-JAG typ must be oauth-id-jag+jwt".to_string(),
        ));
    }
    let kid = header
        .kid
        .as_deref()
        .ok_or_else(|| AuthError::InvalidGrant("ID-JAG is missing kid".to_string()))?;
    let unverified = jsonwebtoken::dangerous::insecure_decode::<IdJagClaims>(&assertion)
        .map_err(|_| AuthError::InvalidGrant("invalid ID-JAG".to_string()))?
        .claims;
    let issuer = state
        .config
        .enterprise_issuers
        .iter()
        .find(|issuer| issuer.issuer.trim_end_matches('/') == unverified.iss.trim_end_matches('/'))
        .ok_or_else(|| AuthError::InvalidGrant("untrusted ID-JAG issuer".to_string()))?;
    let jwks = load_enterprise_jwks(&state, issuer, kid).await?;
    let jwk = jwks
        .find(kid)
        .ok_or_else(|| AuthError::InvalidGrant("unknown ID-JAG signing key".to_string()))?;
    ensure_jwk_algorithm(jwk, header.alg)
        .map_err(|_| AuthError::InvalidGrant("ID-JAG key algorithm mismatch".to_string()))?;
    let key = DecodingKey::from_jwk(jwk)
        .map_err(|_| AuthError::InvalidGrant("invalid ID-JAG signing key".to_string()))?;
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
    {
        return Err(AuthError::InvalidGrant(
            "expired, replayed, or misdirected ID-JAG".to_string(),
        ));
    }
    let client_id = require_field(request.client_id.clone(), "client_id")?;
    if claims.client_id != client_id
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
    authenticate_oauth_client(
        &state,
        &client_id,
        request.client_secret.as_deref(),
        request.client_assertion_type.as_deref(),
        request.client_assertion.as_deref(),
    )
    .await?;
    if matches!(
        state.inbound_provider.kind(),
        crate::config::InboundProviderKind::Google
    ) && !state
        .consume_assertion_jti(&claims.iss, &claims.jti, claims.iat, claims.exp)
        .await?
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
    let scope = validate_token_scope(&state, &resource, &claims.scope)?;
    if let Some(requested) = request.scope.as_deref() {
        let requested = validate_token_scope(&state, &resource, requested)?;
        if !requested
            .split_whitespace()
            .all(|value| scope.split_whitespace().any(|granted| granted == value))
        {
            return Err(AuthError::InvalidScope(
                "requested scope exceeds ID-JAG grant".to_string(),
            ));
        }
        return build_token_response(
            &state,
            client_id,
            claims.sub,
            resource,
            requested,
            None,
            TokenIdentity::ExternalIssuer(claims.iss),
        );
    }
    build_token_response(
        &state,
        client_id,
        claims.sub,
        resource,
        scope,
        None,
        TokenIdentity::ExternalIssuer(claims.iss),
    )
}

fn ensure_allowed_algorithm(algorithm: Algorithm) -> Result<(), AuthError> {
    if matches!(
        algorithm,
        Algorithm::EdDSA | Algorithm::RS256 | Algorithm::ES256
    ) {
        Ok(())
    } else {
        Err(invalid_client())
    }
}

fn ensure_jwk_algorithm(jwk: &Jwk, algorithm: Algorithm) -> Result<(), AuthError> {
    let matches = match jwk.common.key_algorithm {
        None => true,
        Some(KeyAlgorithm::EdDSA) => algorithm == Algorithm::EdDSA,
        Some(KeyAlgorithm::RS256) => algorithm == Algorithm::RS256,
        Some(KeyAlgorithm::ES256) => algorithm == Algorithm::ES256,
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(invalid_client())
    }
}

async fn load_enterprise_jwks(
    state: &AuthState,
    issuer: &crate::config::EnterpriseIssuerConfig,
    required_kid: &str,
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
    load_remote_jwks(state, uri, required_kid, "enterprise JWKS").await
}

const MAX_JWKS_CACHE_ENTRIES: usize = 256;
const JWKS_NEGATIVE_CACHE_SECS: i64 = 30;

fn jwks_negative_cache_hit(state: &AuthState, cache_key: &str) -> bool {
    let hit = state
        .jwks_negative_cache
        .get(cache_key)
        .is_some_and(|entry| *entry.value() > now_unix());
    if hit {
        tracing::info!(
            event = "oauth.jwks.cache",
            document = "remote_jwks",
            outcome = "negative_hit",
            "remote JWKS refresh suppressed"
        );
    }
    hit
}

fn record_jwks_negative_cache(state: &AuthState, cache_key: &str) {
    let Ok(_maintenance) = state.jwks_cache_maintenance.lock() else {
        tracing::warn!(
            kind = "internal_error",
            "JWKS negative cache maintenance lock poisoned"
        );
        return;
    };
    state
        .jwks_negative_cache
        .retain(|_, expires_at| *expires_at > now_unix());
    while state.jwks_negative_cache.len() >= MAX_JWKS_CACHE_ENTRIES {
        let oldest = state
            .jwks_negative_cache
            .iter()
            .min_by_key(|entry| *entry.value())
            .map(|entry| entry.key().clone());
        let Some(oldest) = oldest else { break };
        state.jwks_negative_cache.remove(&oldest);
    }
    state.jwks_negative_cache.insert(
        cache_key.to_string(),
        now_unix().saturating_add(JWKS_NEGATIVE_CACHE_SECS),
    );
    tracing::info!(
        event = "oauth.jwks.cache",
        document = "remote_jwks",
        outcome = "negative_recorded",
        ttl_secs = JWKS_NEGATIVE_CACHE_SECS,
        "remote JWKS negative result cached"
    );
}

/// Fetch (or reuse a cached) remote JWK set, keyed by URL.
///
/// `required_kid` participates in the cache-hit test so a key rotation the
/// client already signs with forces a refetch instead of failing for the
/// remainder of the cached document's lifetime.
async fn load_remote_jwks(
    state: &AuthState,
    uri: &url::Url,
    required_kid: &str,
    document_name: &str,
) -> Result<JwkSet, AuthError> {
    tokio::time::timeout(
        crate::remote::REMOTE_FETCH_DEADLINE,
        load_remote_jwks_within_deadline(state, uri, required_kid, document_name),
    )
    .await
    .map_err(|_| AuthError::Network(format!("{document_name} resolution timed out")))?
}

async fn load_remote_jwks_within_deadline(
    state: &AuthState,
    uri: &url::Url,
    required_kid: &str,
    document_name: &str,
) -> Result<JwkSet, AuthError> {
    let cache_key = uri.as_str();
    let now = now_unix();
    if let Some(entry) = state.jwks_cache.get(cache_key)
        && entry.value().1 > now
    {
        if entry.value().0.find(required_kid).is_some() || jwks_negative_cache_hit(state, cache_key)
        {
            return Ok(entry.value().0.clone());
        }
    }
    if jwks_negative_cache_hit(state, cache_key) {
        return Err(AuthError::Validation(format!(
            "{document_name} is temporarily unavailable"
        )));
    }
    let lock_key = format!("jwks:{cache_key}");
    let fetch_lock = crate::cimd::acquire_remote_fetch_lock(state, &lock_key)?;
    let _guard = fetch_lock.lock().await;
    if let Some(entry) = state.jwks_cache.get(cache_key)
        && entry.value().1 > now_unix()
        && (entry.value().0.find(required_kid).is_some()
            || jwks_negative_cache_hit(state, cache_key))
    {
        let jwks = entry.value().0.clone();
        return Ok(jwks);
    }
    if jwks_negative_cache_hit(state, cache_key) {
        return Err(AuthError::Validation(format!(
            "{document_name} is temporarily unavailable"
        )));
    }
    let _permit = tokio::time::timeout(
        crate::remote::REMOTE_FETCH_DEADLINE,
        state.remote_fetch_permits.acquire(),
    )
    .await
    .map_err(|_| AuthError::Network("remote JWKS permit timed out".to_string()))?
    .map_err(|_| AuthError::Server("remote metadata fetch limiter closed".to_string()))?;
    let (jwks, cache_policy) = match crate::remote::fetch_json::<JwkSet>(uri, document_name).await {
        Ok(value) => value,
        Err(error) => {
            record_jwks_negative_cache(state, cache_key);
            return Err(error);
        }
    };
    if jwks.find(required_kid).is_some() {
        state.jwks_negative_cache.remove(cache_key);
        tracing::info!(
            event = "oauth.jwks.refresh",
            document = document_name,
            outcome = "required_key_found",
            "remote JWKS refresh completed"
        );
    } else {
        record_jwks_negative_cache(state, cache_key);
        tracing::warn!(
            event = "oauth.jwks.refresh",
            document = document_name,
            outcome = "required_key_absent",
            "remote JWKS refresh completed without the required key"
        );
    }
    if cache_policy.cacheable {
        let _maintenance = state
            .jwks_cache_maintenance
            .lock()
            .map_err(|_| AuthError::Server("JWKS cache maintenance lock poisoned".to_string()))?;
        state
            .jwks_cache
            .retain(|_, (_, expires_at)| *expires_at > now_unix());
        if state.jwks_cache.len() >= MAX_JWKS_CACHE_ENTRIES
            && let Some(oldest) = state
                .jwks_cache
                .iter()
                .min_by_key(|entry| entry.value().1)
                .map(|entry| entry.key().clone())
        {
            state.jwks_cache.remove(&oldest);
        }
        state.jwks_cache.insert(
            cache_key.to_string(),
            (
                jwks.clone(),
                now_unix().saturating_add(cache_policy.max_age_secs),
            ),
        );
    }
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
        .ok_or_else(invalid_client)?;
    authenticate_machine_client(
        &state,
        client,
        request.client_secret.as_deref(),
        request.client_assertion_type.as_deref(),
        request.client_assertion.as_deref(),
    )
    .await?;
    let resource = crate::authorize::validate_resource(&state, request.resource.as_deref())
        .map_err(|error| match error {
            AuthError::Validation(message) => AuthError::InvalidScope(message),
            other => other,
        })?;
    if !client
        .resources
        .iter()
        .any(|allowed| allowed.trim_end_matches('/') == resource)
    {
        return Err(AuthError::InvalidScope(
            "requested resource exceeds machine client grant".to_string(),
        ));
    }
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
    let scope = validate_token_scope(&state, &resource, &requested_scope)?;
    if !client.scopes.is_empty()
        && !scope
            .split_whitespace()
            .all(|requested| client.scopes.iter().any(|allowed| allowed == requested))
    {
        return Err(AuthError::InvalidScope(
            "requested scope exceeds machine client grant".to_string(),
        ));
    }
    build_token_response(
        &state,
        client_id.clone(),
        client_id.clone(),
        resource,
        scope,
        None,
        TokenIdentity::LocalCredential(format!("machine-client:{client_id}")),
    )
}

async fn authenticate_machine_client(
    state: &AuthState,
    client: &crate::config::MachineClientConfig,
    client_secret: Option<&str>,
    client_assertion_type: Option<&str>,
    client_assertion: Option<&str>,
) -> Result<(), AuthError> {
    match (client_secret, client_assertion) {
        (Some(supplied_secret), None) => {
            let expected_secret = client.client_secret.as_deref().ok_or_else(invalid_client)?;
            if !bool::from(supplied_secret.as_bytes().ct_eq(expected_secret.as_bytes())) {
                return Err(invalid_client());
            }
        }
        (None, Some(assertion)) => {
            if client_assertion_type
                != Some("urn:ietf:params:oauth:client-assertion-type:jwt-bearer")
            {
                return Err(invalid_client());
            }
            let jwks: JwkSet = serde_json::from_value(
                client.jwks.clone().ok_or_else(invalid_client)?,
            )
            .map_err(|error| AuthError::Config(format!("invalid machine client JWKS: {error}")))?;
            validate_client_assertion(
                state,
                assertion,
                &client.client_id,
                ClientKeySource::Inline(&jwks),
            )
            .await?;
        }
        _ => return Err(invalid_client()),
    }
    Ok(())
}

async fn authenticate_oauth_client(
    state: &AuthState,
    client_id: &str,
    client_secret: Option<&str>,
    client_assertion_type: Option<&str>,
    client_assertion: Option<&str>,
) -> Result<(), AuthError> {
    if let Some(client) = state
        .config
        .machine_clients
        .iter()
        .find(|client| client.client_id == client_id)
    {
        return authenticate_machine_client(
            state,
            client,
            client_secret,
            client_assertion_type,
            client_assertion,
        )
        .await;
    }
    let client = crate::cimd::resolve_client(state, client_id)
        .await?
        .ok_or_else(invalid_client)?;
    // Match on the method the client actually presents, then check that the
    // client published it. A client that declares `private_key_jwt` as its
    // preference but also lists `none` in
    // `token_endpoint_auth_methods_supported` may legitimately use either;
    // keying off the singular preference alone rejects it for doing exactly
    // what its own metadata document advertises.
    let published = |method: &str| {
        if client.token_endpoint_auth_methods.is_empty() {
            client.token_endpoint_auth_method == method
        } else {
            client
                .token_endpoint_auth_methods
                .iter()
                .any(|m| m == method)
        }
    };
    // Treat only *non-empty* fields as presented credentials. `axum::Form`
    // deserializes a present-but-empty field (`client_assertion_type=`) to
    // `Some("")`, and SDKs that serialize every optional field emit exactly
    // that. Counting it as a credential classifies an ordinary public client
    // as `private_key_jwt` and rejects it — a client that authenticates fine
    // today would start failing for sending an empty string.
    let client_secret = client_secret.filter(|value| !value.is_empty());
    let client_assertion = client_assertion.filter(|value| !value.is_empty());
    let client_assertion_type = client_assertion_type.filter(|value| !value.is_empty());
    // Proof of possession is the assertion itself; a bare type is not a
    // credential. Requiring the assertion here keeps a stray
    // `client_assertion_type=` from diverting a public client.
    let presented = if client_assertion.is_some() {
        "private_key_jwt"
    } else if client_secret.is_some() {
        "client_secret"
    } else {
        "none"
    };
    if !published(presented) {
        warn!(
            client_id = %fingerprint(client_id),
            presented_auth_method = presented,
            declared_auth_method = %client.token_endpoint_auth_method,
            published_auth_methods = ?client.token_endpoint_auth_methods,
            "oauth token rejected: client authenticated with a method it did not publish"
        );
        return Err(invalid_client());
    }
    match presented {
        "none" => Ok(()),
        "private_key_jwt"
            if client_secret.is_none()
                && client_assertion_type
                    == Some("urn:ietf:params:oauth:client-assertion-type:jwt-bearer") =>
        {
            let inline = client
                .jwks
                .map(|value| serde_json::from_value::<JwkSet>(value).map_err(|_| invalid_client()))
                .transpose()?;
            let remote = client
                .jwks_uri
                .as_deref()
                .map(|uri| url::Url::parse(uri).map_err(|_| invalid_client()))
                .transpose()?;
            let source = match (&inline, &remote) {
                (Some(jwks), _) => ClientKeySource::Inline(jwks),
                (None, Some(uri)) => ClientKeySource::Remote(uri),
                (None, None) => return Err(invalid_client()),
            };
            validate_client_assertion(
                state,
                client_assertion.ok_or_else(invalid_client)?,
                client_id,
                source,
            )
            .await
        }
        _ => {
            // Reachable when the presented method is published but its
            // preconditions fail — e.g. `private_key_jwt` accompanied by a
            // client_secret, or an assertion carrying the wrong
            // `client_assertion_type`. This is the arm the silent-401 bug
            // came through; it must never be quiet again.
            warn!(
                kind = "auth_failed",
                client_id = %fingerprint(client_id),
                presented_auth_method = presented,
                has_client_secret = client_secret.is_some(),
                has_client_assertion = client_assertion.is_some(),
                assertion_type_matches = client_assertion_type
                    == Some("urn:ietf:params:oauth:client-assertion-type:jwt-bearer"),
                "oauth token rejected: client authentication preconditions not met"
            );
            Err(invalid_client())
        }
    }
}

/// Where a client's public keys come from when verifying its assertion.
///
/// Resolution is deferred until the assertion's `kid` is known so a remote
/// key set can be cache-checked (and refetched on rotation) against that
/// exact key id.
enum ClientKeySource<'a> {
    Inline(&'a JwkSet),
    Remote(&'a url::Url),
}

fn invalid_client() -> AuthError {
    AuthError::AuthFailed("invalid client credentials".to_string())
}

fn validate_token_scope(
    state: &AuthState,
    resource: &str,
    scope: &str,
) -> Result<String, AuthError> {
    crate::authorize::validate_scope(state, resource, scope).map_err(|error| match error {
        AuthError::Validation(message) => AuthError::InvalidScope(message),
        other => other,
    })
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

async fn validate_client_assertion(
    state: &AuthState,
    assertion: &str,
    client_id: &str,
    keys: ClientKeySource<'_>,
) -> Result<(), AuthError> {
    let header = decode_header(assertion).map_err(|_| {
        warn!(
            kind = "auth_failed",
            client_id = %fingerprint(client_id),
            reason = "undecodable_header",
            "oauth token rejected: client assertion header could not be decoded"
        );
        AuthError::AuthFailed("invalid client assertion".to_string())
    })?;
    ensure_allowed_algorithm(header.alg).inspect_err(|_| {
        warn!(
            kind = "auth_failed",
            client_id = %fingerprint(client_id),
            alg = ?header.alg,
            reason = "disallowed_algorithm",
            "oauth token rejected: client assertion uses an unsupported signing algorithm"
        );
    })?;
    let kid = header.kid.as_deref().ok_or_else(|| {
        warn!(
            kind = "auth_failed",
            client_id = %fingerprint(client_id),
            reason = "missing_kid",
            "oauth token rejected: client assertion header has no kid"
        );
        AuthError::AuthFailed("client assertion is missing kid".to_string())
    })?;
    let fetched;
    let jwks = match keys {
        ClientKeySource::Inline(jwks) => jwks,
        ClientKeySource::Remote(uri) => {
            fetched = load_remote_jwks(state, uri, kid, "client JWKS").await?;
            &fetched
        }
    };
    let jwk = jwks.find(kid).ok_or_else(|| {
        warn!(
            kind = "auth_failed",
            client_id = %fingerprint(client_id),
            kid_id = %fingerprint(kid),
            key_count = jwks.keys.len(),
            reason = "unknown_kid",
            "oauth token rejected: client assertion kid is absent from the client key set"
        );
        AuthError::AuthFailed("unknown client assertion key".to_string())
    })?;
    ensure_jwk_algorithm(jwk, header.alg).inspect_err(|_| {
        warn!(
            kind = "auth_failed",
            client_id = %fingerprint(client_id),
            kid_id = %fingerprint(kid),
            reason = "key_algorithm_mismatch",
            "oauth token rejected: client assertion key algorithm does not match its header"
        );
    })?;
    let key = DecodingKey::from_jwk(jwk).map_err(|_| {
        warn!(
            kind = "auth_failed",
            client_id = %fingerprint(client_id),
            kid_id = %fingerprint(kid),
            reason = "unusable_key",
            "oauth token rejected: client assertion key could not be parsed"
        );
        AuthError::AuthFailed("invalid client assertion key".to_string())
    })?;
    let audience = format!("{}/token", crate::metadata::public_base_url(state));
    let mut validation = Validation::new(header.alg);
    validation.set_audience(&[audience.as_str()]);
    validation.set_issuer(&[client_id]);
    validation.set_required_spec_claims(&["exp", "iat", "iss", "sub", "aud", "jti"]);
    let claims = decode::<ClientAssertionClaims>(assertion, &key, &validation)
        .map_err(|_| {
            warn!(
                kind = "auth_failed",
                client_id = %fingerprint(client_id),
                kid_id = %fingerprint(kid),
                reason = "signature_or_claims_rejected",
                "oauth token rejected: client assertion failed signature or claim validation"
            );
            AuthError::AuthFailed("invalid client assertion".to_string())
        })?
        .claims;
    let now = now_unix();
    if claims.iss != client_id
        || claims.sub != client_id
        || claims.aud != audience
        || claims.iat > now + 60
        || claims.exp <= now
    {
        warn!(
            kind = "auth_failed",
            client_id = %fingerprint(client_id),
            kid_id = %fingerprint(kid),
            reason = "claim_mismatch_or_expired",
            "oauth token rejected: client assertion issuer, subject, audience, or lifetime is invalid"
        );
        return Err(AuthError::AuthFailed(
            "invalid or replayed client assertion".to_string(),
        ));
    }
    if !state
        .consume_assertion_jti(&claims.iss, &claims.jti, claims.iat, claims.exp)
        .await?
    {
        return Err(AuthError::AuthFailed(
            "invalid or replayed client assertion".to_string(),
        ));
    }
    Ok(())
}

fn basic_client_credentials(
    headers: &axum::http::HeaderMap,
) -> Result<Option<(String, String)>, AuthError> {
    let Some(raw) = headers.get(header::AUTHORIZATION) else {
        return Ok(None);
    };
    let raw = raw
        .to_str()
        .map_err(|_| AuthError::AuthFailed("invalid client credentials".to_string()))?;
    let Some((scheme, encoded)) = raw.split_once(' ') else {
        return Err(AuthError::AuthFailed(
            "invalid client credentials".to_string(),
        ));
    };
    if !scheme.eq_ignore_ascii_case("basic") {
        return Err(AuthError::AuthFailed(
            "invalid client credentials".to_string(),
        ));
    }
    let decoded = STANDARD
        .decode(encoded.trim())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| AuthError::AuthFailed("invalid client credentials".to_string()))?;
    let (client_id, client_secret) = decoded
        .split_once(':')
        .ok_or_else(|| AuthError::AuthFailed("invalid client credentials".to_string()))?;
    let decode_component = |value: &str| {
        url::form_urlencoded::parse(format!("value={value}").as_bytes())
            .next()
            .map(|(_, value)| value.into_owned())
            .ok_or_else(|| AuthError::AuthFailed("invalid client credentials".to_string()))
    };
    Ok(Some((
        decode_component(client_id)?,
        decode_component(client_secret)?,
    )))
}

pub async fn revoke(
    State(state): State<AuthState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: axum::http::HeaderMap,
    Form(mut request): Form<RevocationRequest>,
) -> Response {
    let remote_ip = connect_info
        .map(|Extension(ConnectInfo(address))| address.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    if let Err(error) = state.check_token_rate_limit(remote_ip).await {
        return TokenEndpointError::Auth(error).into_response();
    }
    match basic_client_credentials(&headers) {
        Ok(Some((client_id, client_secret))) => {
            if request.client_id.is_some()
                || request.client_secret.is_some()
                || request.client_assertion.is_some()
            {
                return TokenEndpointError::Auth(invalid_client()).into_response();
            }
            request.client_id = Some(client_id);
            request.client_secret = Some(client_secret);
        }
        Ok(None) => {}
        Err(error) => return TokenEndpointError::Auth(error).into_response(),
    }
    let token_id = fingerprint(&request.token);
    match state.store.find_refresh_token(&request.token).await {
        Ok(Some(row)) => {
            let Some(client_id) = request.client_id.as_deref() else {
                return TokenEndpointError::Auth(invalid_client()).into_response();
            };
            if client_id != row.client_id {
                return TokenEndpointError::Auth(invalid_client()).into_response();
            }
            if let Err(error) = authenticate_oauth_client(
                &state,
                client_id,
                request.client_secret.as_deref(),
                request.client_assertion_type.as_deref(),
                request.client_assertion.as_deref(),
            )
            .await
            {
                return TokenEndpointError::Auth(error).into_response();
            }
            if let Err(error) = state.store.revoke_refresh_token(&request.token).await {
                return TokenEndpointError::Auth(error).into_response();
            }
            info!(token_id = %token_id, client_id = %fingerprint(&row.client_id), "oauth refresh token revoked");
            apply_token_cache_headers(StatusCode::OK.into_response())
        }
        Ok(None) => {
            let replay_client = match state
                .store
                .find_refresh_token_replay_client(&request.token)
                .await
            {
                Ok(client) => client,
                Err(error) => return TokenEndpointError::Auth(error).into_response(),
            };
            let Some(replay_client) = replay_client else {
                return apply_token_cache_headers(StatusCode::OK.into_response());
            };
            let Some(client_id) = request.client_id.as_deref() else {
                return TokenEndpointError::Auth(invalid_client()).into_response();
            };
            if client_id != replay_client {
                return TokenEndpointError::Auth(invalid_client()).into_response();
            }
            if let Err(error) = authenticate_oauth_client(
                &state,
                client_id,
                request.client_secret.as_deref(),
                request.client_assertion_type.as_deref(),
                request.client_assertion.as_deref(),
            )
            .await
            {
                return TokenEndpointError::Auth(error).into_response();
            }
            if let Err(error) = state
                .store
                .revoke_refresh_token_replay(&request.token, client_id)
                .await
            {
                return TokenEndpointError::Auth(error).into_response();
            }
            info!(token_id = %token_id, client_id, "oauth refresh replay successor revoked");
            apply_token_cache_headers(StatusCode::OK.into_response())
        }
        Err(error) => TokenEndpointError::Auth(error).into_response(),
    }
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
    authenticate_oauth_client(
        &state,
        &client_id,
        request.client_secret.as_deref(),
        request.client_assertion_type.as_deref(),
        request.client_assertion.as_deref(),
    )
    .await?;
    let auth_code_id = fingerprint(&code);
    info!(
        grant_type = "authorization_code",
        client_id = %fingerprint(&client_id),
        auth_code_id = %auth_code_id,
        redirect_uri_id = %fingerprint(&redirect_uri),
        requested_resource_id = %requested_resource.as_deref().map(fingerprint).unwrap_or_else(|| "<authorization-code-resource>".to_string()),
        "oauth authorization_code grant redeeming local code"
    );

    let expected_challenge = pkce_challenge(&code_verifier);
    let bound_row = state
        .store
        .redeem_bound_verified_auth_code(
            &code,
            &client_id,
            &redirect_uri,
            requested_resource.as_deref(),
            &expected_challenge,
            "S256",
        )
        .await
        .map_err(|error| {
            warn!(
                auth_code_id = %auth_code_id,
                client_id = %fingerprint(&client_id),
                error = %error,
                "oauth token rejected: authorization code is invalid, expired, already redeemed, or does not match the grant"
            );
            error
        })?;
    let binding = bound_row.binding;
    let row = bound_row.value;
    if matches!(
        state.inbound_provider.kind(),
        crate::config::InboundProviderKind::Google
    ) && !state
        .store
        .has_google_provider_credential_for_subject(&row.subject)
        .await?
    {
        warn!(
            grant_type = "authorization_code",
            client_id = %fingerprint(&row.client_id),
            auth_code_id = %auth_code_id,
            subject_id = %fingerprint(&row.subject),
            kind = "oauth_needs_reauth",
            "oauth token rejected: google provider credential disappeared before code redemption"
        );
        return Err(AuthError::OauthNeedsReauth(
            "google provider credential is unavailable; reauthorization required".to_string(),
        ));
    }

    let refresh_token = random_token(24)?;
    let created_at = now_unix();
    state
        .store
        .upsert_bound_refresh_token(
            RefreshTokenRow {
                refresh_token: refresh_token.clone(),
                client_id: row.client_id.clone(),
                subject: row.subject.clone(),
                resource: row.resource.clone(),
                scope: row.scope.clone(),
                provider_refresh_token: None,
                created_at,
                expires_at: expires_at(
                    created_at,
                    state.config.refresh_token_ttl,
                    &format!("{}_AUTH_REFRESH_TOKEN_TTL_SECS", state.config.env_prefix),
                )?,
            },
            binding.clone(),
        )
        .await?;
    info!(
        grant_type = "authorization_code",
        client_id = %fingerprint(&row.client_id),
        auth_code_id = %auth_code_id,
        subject_id = %fingerprint(&row.subject),
        resource_id = %fingerprint(&row.resource),
        scope_id = %fingerprint(&row.scope),
        "oauth authorization_code grant issued lab access token and refresh token"
    );
    let refresh_token = Some(refresh_token);

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
        TokenIdentity::ExternalIssuer(binding.identity_issuer),
    )
}

enum TokenIdentity {
    ExternalIssuer(String),
    LocalCredential(String),
}

fn build_token_response(
    state: &AuthState,
    client_id: String,
    subject: String,
    resource: String,
    scope: String,
    refresh_token: Option<String>,
    identity: TokenIdentity,
) -> Result<TokenResponse, AuthError> {
    let issuer = crate::metadata::public_base_url(state);
    let now = timestamp_usize(now_unix(), "current unix timestamp")?;
    let access_token_ttl = duration_secs_usize(
        state.config.access_token_ttl,
        &format!("{}_AUTH_ACCESS_TOKEN_TTL_SECS", state.config.env_prefix),
    )?;
    let subject_id = fingerprint(&subject);
    let (identity_issuer, identity_credential_id) = match identity {
        TokenIdentity::ExternalIssuer(issuer) => (Some(issuer), None),
        TokenIdentity::LocalCredential(credential_id) => (None, Some(credential_id)),
    };
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
        nbf: None,
        iat: now,
        jti: random_token(18)?,
        scope: scope.clone(),
        azp: client_id.clone(),
        identity_issuer,
        identity_credential_id,
    })?;
    info!(
        client_id = %fingerprint(&client_id),
        subject_id = %subject_id,
        resource_id = %fingerprint(&resource),
        scope_id = %fingerprint(&scope),
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

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
    use crate::types::TokenResponse;

    use super::super::authorize::tests::{
        test_auth_state_with_mock_google, test_auth_state_with_registered_client,
    };

    #[tokio::test(flavor = "current_thread")]
    async fn rejected_token_logs_redact_attacker_controlled_values() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK.lock().await;
        let buf = crate::test_support::global_tracing_buffer();
        let app = router(test_auth_state_with_registered_client().await);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("grant_type=sentinel-grant-secret&client_id=sentinel-client-secret&resource=https%3A%2F%2Fsentinel-resource.example%2Fmcp"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let logs = crate::test_support::captured_logs(buf);
        for sentinel in [
            "sentinel-grant-secret",
            "sentinel-client-secret",
            "sentinel-resource.example",
        ] {
            assert!(
                !logs.contains(sentinel),
                "OAuth token value leaked into logs: {sentinel}\n{logs}"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rejected_client_assertion_logs_redact_attacker_controlled_kid() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK.lock().await;
        let buf = crate::test_support::global_tracing_buffer();
        let state = test_auth_state_with_registered_client().await;
        let sentinel = "sentinel-jwt-kid-secret";
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(sentinel.to_string());
        let encoded_header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap());
        let assertion = format!("{encoded_header}.e30.invalid");
        let empty_keys = jsonwebtoken::jwk::JwkSet { keys: Vec::new() };

        let result = super::validate_client_assertion(
            &state,
            &assertion,
            "client",
            super::ClientKeySource::Inline(&empty_keys),
        )
        .await;
        assert!(result.is_err());
        let logs = crate::test_support::captured_logs(buf);
        assert!(!logs.contains(sentinel), "JWT kid leaked into logs: {logs}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn malformed_client_assertion_claims_are_not_logged() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK.lock().await;
        let buf = crate::test_support::global_tracing_buffer();
        let state = test_auth_state_with_registered_client().await;
        let (key, keys) = assertion_key();
        let keys: jsonwebtoken::jwk::JwkSet = serde_json::from_value(keys).unwrap();
        let sentinel = "sentinel-invalid-exp-secret";
        let assertion = sign_assertion(
            &key,
            &serde_json::json!({
                "iss": "client", "sub": "client", "aud": "https://lab.example.com/token",
                "exp": sentinel, "iat": crate::util::now_unix(), "jti": "test-assertion"
            }),
            None,
        );
        assert!(
            super::validate_client_assertion(
                &state,
                &assertion,
                "client",
                super::ClientKeySource::Inline(&keys)
            )
            .await
            .is_err()
        );
        let logs = crate::test_support::captured_logs(buf);
        assert!(
            !logs.contains(sentinel),
            "malformed assertion claims leaked: {logs}"
        );
    }

    #[tokio::test]
    async fn unknown_jwks_kid_is_negatively_cached_per_document() {
        let state = test_auth_state_with_registered_client().await;
        let uri = Url::parse("https://keys.example.com/jwks.json").unwrap();
        let jwks: jsonwebtoken::jwk::JwkSet = serde_json::from_value(serde_json::json!({
            "keys": []
        }))
        .unwrap();
        state.jwks_cache.insert(
            uri.to_string(),
            (jwks.clone(), crate::util::now_unix() + 60),
        );
        super::record_jwks_negative_cache(&state, uri.as_str());

        // A cache miss for an attacker-selected kid must not turn into another
        // remote fetch while this document's short negative entry is live.
        let resolved = super::load_remote_jwks(&state, &uri, "attacker-kid", "client JWKS")
            .await
            .unwrap();
        assert_eq!(resolved.keys.len(), 0);
    }

    #[tokio::test]
    async fn jwks_negative_cache_cardinality_is_bounded_by_document_count() {
        let state = test_auth_state_with_registered_client().await;
        for index in 0..=super::MAX_JWKS_CACHE_ENTRIES {
            super::record_jwks_negative_cache(
                &state,
                &format!("https://keys-{index}.example/jwks.json"),
            );
        }
        assert_eq!(
            state.jwks_negative_cache.len(),
            super::MAX_JWKS_CACHE_ENTRIES
        );
        assert!(super::jwks_negative_cache_hit(
            &state,
            "https://keys-256.example/jwks.json"
        ));
        state.jwks_negative_cache.insert(
            "https://expired.example/jwks.json".to_string(),
            crate::util::now_unix() - 1,
        );
        assert!(!super::jwks_negative_cache_hit(
            &state,
            "https://expired.example/jwks.json"
        ));
    }

    #[tokio::test]
    async fn negative_jwks_result_without_a_cached_document_skips_remote_io() {
        let state = test_auth_state_with_registered_client().await;
        let uri = Url::parse("https://does-not-resolve.invalid/jwks.json").unwrap();
        super::record_jwks_negative_cache(&state, uri.as_str());

        let error = super::load_remote_jwks(&state, &uri, "attacker-kid", "client JWKS")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("temporarily unavailable"));
    }

    #[tokio::test]
    async fn previously_unseen_kid_still_attempts_one_rotation_refresh() {
        let state = test_auth_state_with_registered_client().await;
        let uri = Url::parse("https://does-not-resolve.invalid/jwks.json").unwrap();
        let jwks: jsonwebtoken::jwk::JwkSet = serde_json::from_value(serde_json::json!({
            "keys": []
        }))
        .unwrap();
        state
            .jwks_cache
            .insert(uri.to_string(), (jwks, crate::util::now_unix() + 60));

        let error = super::load_remote_jwks(&state, &uri, "rotated-key", "client JWKS")
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("temporarily unavailable"));
        assert!(super::jwks_negative_cache_hit(&state, uri.as_str()));
    }

    async fn install_google_provider_credential(state: &AuthState) {
        state
            .store
            .upsert_google_provider_credential(
                "google-subject-123",
                Some("user@example.com"),
                "provider-refresh",
            )
            .await
            .unwrap();
    }

    async fn test_auth_state_with_refreshable_google() -> AuthState {
        let state = test_auth_state_with_mock_google().await;
        install_google_provider_credential(&state).await;
        state
    }

    async fn test_auth_state_with_failing_google_refresh() -> AuthState {
        test_auth_state_with_failing_google_refresh_and_server()
            .await
            .0
    }

    async fn test_auth_state_with_failing_google_refresh_and_server()
    -> (AuthState, &'static MockServer) {
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
        let state = AuthState::for_tests(
            (*state.config).clone(),
            state.store.clone(),
            (*state.signing_keys).clone(),
            google,
        );
        install_google_provider_credential(&state).await;
        (state, server)
    }

    async fn test_auth_state_with_invalid_google_refresh() -> AuthState {
        let state = test_auth_state_with_registered_client().await;
        let server = Box::leak(Box::new(MockServer::start().await));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "Token has been expired or revoked."
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
        let state = AuthState::for_tests(
            (*state.config).clone(),
            state.store.clone(),
            (*state.signing_keys).clone(),
            google,
        );
        install_google_provider_credential(&state).await;
        state
    }

    #[tokio::test]
    async fn token_endpoint_mints_lab_jwt_and_refresh_token() {
        let state = test_auth_state_with_registered_client().await;
        seed_authorization_code(&state).await;
        let app = router(state.clone());
        let response = app.oneshot(
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
        assert_eq!(
            claims.exp - claims.iat,
            json["expires_in"].as_u64().unwrap() as usize
        );
        assert!(
            claims.exp - claims.iat <= 3_600,
            "authorization server access tokens must remain short-lived"
        );
    }

    #[tokio::test]
    async fn black_box_machine_auth_discovery_and_token_flow() {
        let base = test_auth_state_with_registered_client().await;
        let mut config = (*base.config).clone();
        config.machine_clients = vec![crate::config::MachineClientConfig {
            client_id: "ci-agent".to_string(),
            client_secret: Some("correct-horse".to_string()),
            jwks: None,
            scopes: vec!["lab".to_string()],
            resources: vec!["https://lab.example.com/mcp".to_string()],
        }];
        let state = AuthState::for_tests(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            base.google().clone(),
        );
        let app = router(state.clone());
        let metadata = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-authorization-server")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(metadata.status(), StatusCode::OK);
        let metadata: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(metadata.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(
            metadata["grant_types_supported"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("client_credentials"))
        );
        assert!(
            metadata["token_endpoint_auth_methods_supported"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("client_secret_basic"))
        );
        let response = app.oneshot(
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
        let access_token = json["access_token"].as_str().unwrap();
        let claims = insecure_decode::<crate::jwt::AccessClaims>(access_token)
            .unwrap()
            .claims;
        assert_eq!(claims.sub, "ci-agent");
        assert_eq!(claims.aud, "https://lab.example.com/mcp");
        assert_eq!(claims.scope, "lab");
        assert_eq!(claims.identity_issuer, None);
        assert_eq!(
            claims.identity_credential_id.as_deref(),
            Some("machine-client:ci-agent")
        );

        let identity_app = axum::Router::new()
            .route(
                "/probe",
                axum::routing::get(
                    |axum::Extension(identity): axum::Extension<crate::VerifiedIdentity>| async move {
                        let link = match identity.principal_link() {
                            crate::PrincipalLink::LocalCredential { credential_id } => {
                                format!("local|{credential_id}")
                            }
                            crate::PrincipalLink::External { issuer, subject } => {
                                format!("external|{issuer}|{subject}")
                            }
                        };
                        format!("{link}|{}", identity.transport_credential_issuer())
                    },
                ),
            )
            .route_layer(crate::AuthLayer::from_state(std::sync::Arc::new(state)));
        let middleware_response = identity_app
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(middleware_response.status(), StatusCode::OK);
        let middleware_body = axum::body::to_bytes(middleware_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            &middleware_body[..],
            b"local|machine-client:ci-agent|https://lab.example.com"
        );
        assert!(json.get("refresh_token").is_none());
    }

    #[test]
    fn basic_client_credentials_decode_form_components_and_reject_malformed_values() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!(
                "bAsIc {}",
                base64::engine::general_purpose::STANDARD.encode("client%2Bid:p%40ss%3Aword")
            )
            .parse()
            .unwrap(),
        );
        assert_eq!(
            super::basic_client_credentials(&headers).unwrap(),
            Some(("client+id".to_string(), "p@ss:word".to_string()))
        );

        for value in ["Basic not-base64", "Bearer token", "Basic bm9jb2xvbg=="] {
            headers.insert(header::AUTHORIZATION, value.parse().unwrap());
            assert!(super::basic_client_credentials(&headers).is_err());
        }
    }

    #[tokio::test]
    async fn client_credentials_rejects_ungranted_resource_and_scope() {
        let base = test_auth_state_with_registered_client().await;
        let mut config = (*base.config).clone();
        config.machine_clients = vec![crate::config::MachineClientConfig {
            client_id: "bounded-agent".to_string(),
            client_secret: Some("secret".to_string()),
            jwks: None,
            scopes: vec!["lab".to_string()],
            resources: vec!["https://lab.example.com/mcp".to_string()],
        }];
        let state = AuthState::for_tests(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            base.google().clone(),
        );
        let app = router(state);
        for (resource, scope, expected_error) in [
            ("https://other.example/mcp", "lab", "invalid_scope"),
            ("https://lab.example.com/mcp", "lab:admin", "invalid_scope"),
        ] {
            let response = app
                .clone()
                .oneshot(form_request(
                    "/token",
                    form(&[
                        ("grant_type", "client_credentials"),
                        ("client_id", "bounded-agent"),
                        ("client_secret", "secret"),
                        ("resource", resource),
                        ("scope", scope),
                    ]),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["error"], expected_error);
        }
    }

    #[tokio::test]
    async fn token_rate_limit_applies_before_client_authentication() {
        let base = test_auth_state_with_registered_client().await;
        let mut config = (*base.config).clone();
        config.token_requests_per_minute = 1;
        let state = AuthState::for_tests(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            base.google().clone(),
        );
        let app = router(state);
        let request_body = form(&[
            ("grant_type", "client_credentials"),
            ("client_id", "unknown"),
            ("client_secret", "wrong"),
        ]);
        let first = app
            .clone()
            .oneshot(form_request("/token", request_body.clone()))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::UNAUTHORIZED);
        let second = app
            .oneshot(form_request("/token", request_body))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(second.headers().contains_key(header::RETRY_AFTER));
    }

    #[tokio::test]
    async fn invalid_client_responses_are_uniform_and_auth_methods_cannot_be_combined() {
        let base = test_auth_state_with_registered_client().await;
        let mut config = (*base.config).clone();
        config.machine_clients = vec![crate::config::MachineClientConfig {
            client_id: "known".to_string(),
            client_secret: Some("correct".to_string()),
            jwks: None,
            scopes: vec!["lab".to_string()],
            resources: vec!["https://lab.example.com/mcp".to_string()],
        }];
        let state = AuthState::for_tests(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            base.google().clone(),
        );
        let app = router(state);
        let mut bodies = Vec::new();
        for (client_id, secret) in [("unknown", "wrong"), ("known", "wrong")] {
            let response = app
                .clone()
                .oneshot(form_request(
                    "/token",
                    form(&[
                        ("grant_type", "client_credentials"),
                        ("client_id", client_id),
                        ("client_secret", secret),
                    ]),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            bodies.push(
                axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(bodies[0], bodies[1]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(
                        header::AUTHORIZATION,
                        format!(
                            "Basic {}",
                            base64::engine::general_purpose::STANDARD.encode("known:correct")
                        ),
                    )
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(form(&[
                        ("grant_type", "client_credentials"),
                        ("client_id", "known"),
                        ("client_secret", "correct"),
                    ])))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
            resources: vec!["https://lab.example.com/mcp".to_string()],
        }];
        let state = AuthState::for_tests(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            base.google().clone(),
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
    async fn private_key_jwt_authenticates_authorization_code_and_refresh_grants_before_redemption()
    {
        let base = test_auth_state_with_registered_client().await;
        let (encoding_key, jwks) = assertion_key();
        let mut config = (*base.config).clone();
        config.machine_clients = vec![crate::config::MachineClientConfig {
            client_id: "jwt-agent".to_string(),
            client_secret: None,
            jwks: Some(jwks),
            scopes: vec!["lab".to_string()],
            resources: vec!["https://lab.example.com/mcp".to_string()],
        }];
        let state = AuthState::for_tests(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            base.google().clone(),
        );
        state
            .store
            .register_client(crate::types::RegisteredClient {
                client_id: "jwt-agent".to_string(),
                redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
                created_at: crate::util::now_unix(),
                token_endpoint_auth_method: "private_key_jwt".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        state
            .store
            .insert_auth_code(crate::types::AuthorizationCodeRow {
                code: "jwt-code".to_string(),
                client_id: "jwt-agent".to_string(),
                subject: "subject".to_string(),
                redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                code_challenge: super::pkce_challenge("verifier"),
                code_challenge_method: "S256".to_string(),
                provider_refresh_token: None,
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 3_600,
            })
            .await
            .unwrap();
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "jwt-refresh".to_string(),
                client_id: "jwt-agent".to_string(),
                subject: "subject".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_refresh_token: None,
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 3_600,
            })
            .await
            .unwrap();
        state
            .store
            .upsert_google_provider_credential(
                "subject",
                Some("user@example.com"),
                "provider-refresh",
            )
            .await
            .unwrap();

        let app = router(state.clone());
        let unauthorized_code = app
            .clone()
            .oneshot(form_request(
                "/token",
                form(&[
                    ("grant_type", "authorization_code"),
                    ("code", "jwt-code"),
                    ("client_id", "jwt-agent"),
                    ("redirect_uri", "http://127.0.0.1:7777/callback"),
                    ("code_verifier", "verifier"),
                ]),
            ))
            .await
            .unwrap();
        assert_eq!(unauthorized_code.status(), StatusCode::UNAUTHORIZED);

        let unauthorized_refresh = app
            .clone()
            .oneshot(form_request(
                "/token",
                form(&[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", "jwt-refresh"),
                    ("client_id", "jwt-agent"),
                ]),
            ))
            .await
            .unwrap();
        assert_eq!(unauthorized_refresh.status(), StatusCode::UNAUTHORIZED);

        let now = crate::util::now_unix();
        let assertion = sign_assertion(
            &encoding_key,
            &super::ClientAssertionClaims {
                iss: "jwt-agent".to_string(),
                sub: "jwt-agent".to_string(),
                aud: "https://lab.example.com/token".to_string(),
                exp: now + 300,
                iat: now,
                jti: "authorization-code-redemption".to_string(),
            },
            None,
        );
        let authorized_code = app
            .oneshot(form_request(
                "/token",
                form(&[
                    ("grant_type", "authorization_code"),
                    ("code", "jwt-code"),
                    ("client_id", "jwt-agent"),
                    ("redirect_uri", "http://127.0.0.1:7777/callback"),
                    ("code_verifier", "verifier"),
                    (
                        "client_assertion_type",
                        "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
                    ),
                    ("client_assertion", &assertion),
                ]),
            ))
            .await
            .unwrap();
        assert_eq!(authorized_code.status(), StatusCode::OK);
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
        config.machine_clients = vec![crate::config::MachineClientConfig {
            client_id: "client".to_string(),
            client_secret: Some("enterprise-client-secret".to_string()),
            jwks: None,
            scopes: vec!["lab".to_string()],
            resources: vec!["https://lab.example.com/mcp".to_string()],
        }];
        let state = AuthState::for_tests(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            base.google().clone(),
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
        let app = router(state);
        let missing_auth = app
            .clone()
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
        assert_eq!(missing_auth.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .oneshot(form_request(
                "/token",
                form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                    ("assertion", &assertion),
                    ("client_id", "client"),
                    ("client_secret", "enterprise-client-secret"),
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
        assert_eq!(
            claims.identity_issuer.as_deref(),
            Some("https://idp.example.com")
        );
        assert_eq!(claims.identity_credential_id, None);
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
    async fn token_endpoint_rejects_authorization_code_without_provider_credential() {
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
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
        assert_eq!(json["error"], "oauth_needs_reauth");
        assert!(json.get("access_token").is_none());
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
    async fn authelia_authorization_code_redeems_without_google_credential_and_keeps_issuer() {
        let base = test_auth_state_with_registered_client().await;
        let issuer = "https://auth.example.test/application/o/labby";
        base.store
            .activate_inbound_provider("authelia", issuer, "authelia-test", crate::util::now_unix())
            .await
            .unwrap();
        let provider = crate::authelia::AutheliaProvider::new(
            Url::parse(issuer).unwrap(),
            "authelia-client".into(),
            "authelia-secret".into(),
            Url::parse("https://lab.example.com/auth/oidc/callback").unwrap(),
        )
        .unwrap();
        let mut config = (*base.config).clone();
        config.admin_email = "admin@example.com".into();
        let state = AuthState::for_tests_with_provider(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            crate::oauth_provider::InboundProviderRuntime::Authelia(Box::new(provider)),
            base.store
                .inbound_provider_state()
                .await
                .unwrap()
                .generation,
        );
        state
            .store
            .insert_auth_code(crate::types::AuthorizationCodeRow {
                code: "authelia-code".into(),
                client_id: "client".into(),
                subject: "authelia-subject".into(),
                redirect_uri: "http://127.0.0.1:7777/callback".into(),
                resource: "https://lab.example.com/mcp".into(),
                scope: "lab".into(),
                code_challenge: super::pkce_challenge("verifier"),
                code_challenge_method: "S256".into(),
                provider_refresh_token: None,
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let response = router(state).oneshot(Request::builder().method("POST").uri("/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("grant_type=authorization_code&code=authelia-code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&code_verifier=verifier"))
            .unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let token: TokenResponse = serde_json::from_slice(&body).unwrap();
        let claims = insecure_decode::<crate::jwt::AccessClaims>(&token.access_token)
            .unwrap()
            .claims;
        assert_eq!(claims.identity_issuer.as_deref(), Some(issuer));
    }

    #[tokio::test]
    async fn authelia_refresh_is_local_rotating_and_current_policy_revokes_access() {
        let base = test_auth_state_with_registered_client().await;
        let issuer = "https://auth.example.test/application/o/labby";
        base.store
            .activate_inbound_provider(
                "authelia",
                issuer,
                "authelia-refresh-test",
                crate::util::now_unix(),
            )
            .await
            .unwrap();
        let provider = crate::authelia::AutheliaProvider::new(
            Url::parse(issuer).unwrap(),
            "authelia-client".into(),
            "authelia-secret".into(),
            Url::parse("https://lab.example.com/auth/oidc/callback").unwrap(),
        )
        .unwrap();
        let mut config = (*base.config).clone();
        config.admin_email = "admin@example.com".into();
        let state = AuthState::for_tests_with_provider(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            crate::oauth_provider::InboundProviderRuntime::Authelia(Box::new(provider)),
            base.store
                .inbound_provider_state()
                .await
                .unwrap()
                .generation,
        );
        state
            .store
            .add_allowed_user("user@example.com", "admin", crate::util::now_unix())
            .await
            .unwrap();
        let provider_verified_at = crate::util::now_unix()
            - i64::try_from(state.config.refresh_token_ttl.as_secs()).unwrap()
            + 30;
        state
            .store
            .upsert_verified_inbound_identity(
                issuer,
                "authelia-subject",
                "user@example.com",
                provider_verified_at,
            )
            .await
            .unwrap();
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "authelia-refresh".into(),
                client_id: "client".into(),
                subject: "authelia-subject".into(),
                resource: "https://lab.example.com/mcp".into(),
                scope: "lab".into(),
                provider_refresh_token: None,
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let app = router(state.clone());
        let refresh = app
            .clone()
            .oneshot(form_request(
                "/token",
                form(&[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", "authelia-refresh"),
                    ("client_id", "client"),
                ]),
            ))
            .await
            .unwrap();
        assert_eq!(refresh.status(), StatusCode::OK);
        let body = axum::body::to_bytes(refresh.into_body(), usize::MAX)
            .await
            .unwrap();
        let token: TokenResponse = serde_json::from_slice(&body).unwrap();
        let replacement = token.refresh_token.unwrap();
        let replacement_row = state
            .store
            .find_refresh_token(&replacement)
            .await
            .unwrap()
            .unwrap();
        assert!(
            replacement_row.expires_at
                <= provider_verified_at
                    + i64::try_from(state.config.refresh_token_ttl.as_secs()).unwrap()
        );

        state
            .store
            .add_allowed_user("expired@example.com", "admin", crate::util::now_unix())
            .await
            .unwrap();
        state
            .store
            .upsert_verified_inbound_identity(
                issuer,
                "expired-subject",
                "expired@example.com",
                crate::util::now_unix()
                    - i64::try_from(state.config.refresh_token_ttl.as_secs()).unwrap()
                    - 1,
            )
            .await
            .unwrap();
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "expired-provider-auth-refresh".into(),
                client_id: "client".into(),
                subject: "expired-subject".into(),
                resource: "https://lab.example.com/mcp".into(),
                scope: "lab".into(),
                provider_refresh_token: None,
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let expired_auth = app
            .clone()
            .oneshot(form_request(
                "/token",
                form(&[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", "expired-provider-auth-refresh"),
                    ("client_id", "client"),
                ]),
            ))
            .await
            .unwrap();
        assert_eq!(expired_auth.status(), StatusCode::UNAUTHORIZED);
        assert!(
            state
                .store
                .find_refresh_token("expired-provider-auth-refresh")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            state
                .store
                .current_verified_inbound_email(issuer, "expired-subject")
                .await
                .unwrap()
                .is_none()
        );
        let replay_predecessor = replacement.clone();
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..10 {
            let app = app.clone();
            let replacement = replacement.clone();
            tasks.spawn(async move {
                app.oneshot(form_request(
                    "/token",
                    form(&[
                        ("grant_type", "refresh_token"),
                        ("refresh_token", &replacement),
                        ("client_id", "client"),
                    ]),
                ))
                .await
                .unwrap()
            });
        }
        let mut concurrent_replacement = None;
        while let Some(result) = tasks.join_next().await {
            let response = result.unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
            let response: TokenResponse = serde_json::from_slice(&body).unwrap();
            if let Some(expected) = concurrent_replacement.as_ref() {
                assert_eq!(response.refresh_token.as_ref(), Some(expected));
            } else {
                concurrent_replacement = response.refresh_token;
            }
        }
        let replacement = concurrent_replacement.unwrap();
        state
            .store
            .remove_allowed_user("user@example.com")
            .await
            .unwrap();
        let denied = app
            .oneshot(form_request(
                "/token",
                form(&[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", &replay_predecessor),
                    ("client_id", "client"),
                ]),
            ))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::BAD_REQUEST);
        assert!(
            state
                .store
                .find_refresh_token(&replacement)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            state
                .store
                .current_verified_inbound_email(issuer, "authelia-subject")
                .await
                .unwrap()
                .is_none()
        );
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
        let state = test_auth_state_with_refreshable_google().await;
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
    async fn refresh_grant_replay_returns_the_rotated_response() {
        let state = test_auth_state_with_refreshable_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "replayed-refresh-token".to_string(),
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
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    "grant_type=refresh_token&refresh_token=replayed-refresh-token&client_id=client",
                ))
                .unwrap()
        };

        let first = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();

        let replay = app.oneshot(request()).await.unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let replay_body = axum::body::to_bytes(replay.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(replay_body, first_body);
    }

    #[tokio::test]
    async fn cached_refresh_replay_rechecks_allowlist_and_revokes_replacement() {
        let state = test_auth_state_with_refreshable_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "offboarded-replay-predecessor".to_string(),
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
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    "grant_type=refresh_token&refresh_token=offboarded-replay-predecessor&client_id=client",
                ))
                .unwrap()
        };
        let first = router(state.clone()).oneshot(request()).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();
        let first_json: TokenResponse = serde_json::from_slice(&first_body).unwrap();
        let replacement = first_json.refresh_token.expect("replacement refresh token");

        let mut config = (*state.config).clone();
        config.admin_email = "replacement-admin@example.com".to_string();
        let offboarded = AuthState::for_tests(
            config,
            state.store.clone(),
            (*state.signing_keys).clone(),
            state.google().clone(),
        );
        let replay = router(offboarded).oneshot(request()).await.unwrap();
        assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
        assert!(
            state
                .store
                .find_refresh_token(&replacement)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn concurrent_refresh_grants_share_the_rotated_response() {
        let state = test_auth_state_with_refreshable_google().await;
        let subject = "google-subject-123";
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "concurrent-refresh-token".to_string(),
                client_id: "client".to_string(),
                subject: subject.to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_refresh_token: None,
                created_at: crate::util::now_unix() - 60,
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let subject_lock = crate::google_refresh::lock(subject);
        let subject_guard = subject_lock.clone().lock_owned().await;
        let waiters = super::refresh_lock_waiter_counter("concurrent-refresh-token");
        let app = router(state);
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    "grant_type=refresh_token&refresh_token=concurrent-refresh-token&client_id=client",
                ))
                .unwrap()
        };
        let first = tokio::spawn(app.clone().oneshot(request()));
        let second = tokio::spawn(app.oneshot(request()));
        tokio::time::timeout(Duration::from_secs(2), async {
            while waiters.load(std::sync::atomic::Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both refresh requests reached the held subject lock");
        drop(subject_guard);

        let first = first.await.unwrap().unwrap();
        let second = second.await.unwrap().unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);
        let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();
        let second_body = axum::body::to_bytes(second.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(second_body, first_body);
    }

    #[tokio::test]
    async fn refresh_grant_claim_does_not_wait_on_provider_io_coordination() {
        let state = test_auth_state_with_refreshable_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "contended-refresh-token".to_string(),
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

        let subject_guard = crate::google_refresh::lock("google-subject-123")
            .lock_owned()
            .await;
        let claim = super::claim_refresh_after_subject_lock(
            &state.store,
            "contended-refresh-token",
            "request-claim",
            crate::util::now_unix() + 90,
        );
        let (claimed, _) = tokio::time::timeout(Duration::from_secs(1), claim)
            .await
            .expect("local claim must not convoy behind provider I/O")
            .unwrap();
        drop(subject_guard);
        assert!(claimed.is_some());
        state
            .store
            .release_refresh_claim("contended-refresh-token", "request-claim")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn refresh_claim_renewal_requires_current_live_owner() {
        let state = test_auth_state_with_refreshable_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "renewable-refresh-token".to_string(),
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
        state
            .store
            .claim_refresh_token(
                "renewable-refresh-token",
                "owner",
                crate::util::now_unix() + 30,
            )
            .await
            .unwrap()
            .expect("initial owner claims token");

        assert!(
            !state
                .store
                .renew_refresh_claim(
                    "renewable-refresh-token",
                    "intruder",
                    crate::util::now_unix() + 90,
                )
                .await
                .unwrap()
        );
        assert!(
            state
                .store
                .renew_refresh_claim(
                    "renewable-refresh-token",
                    "owner",
                    crate::util::now_unix() + 90,
                )
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn cancelling_refresh_releases_local_claim_without_waiting_for_lease_expiry() {
        let state = test_auth_state_with_refreshable_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "cancelled-refresh-token".to_string(),
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

        state
            .store
            .claim_refresh_token(
                "cancelled-refresh-token",
                "cancelled-owner",
                crate::util::now_unix() + 90,
            )
            .await
            .unwrap()
            .expect("claim acquired");
        let observer = std::sync::Arc::new(super::RefreshClaimLeaseObserver::default());
        let (lease, _lost) = super::RefreshClaimLease::start_with_timing_observed(
            state.store.clone(),
            "cancelled-refresh-token".to_string(),
            "cancelled-owner".to_string(),
            crate::util::fingerprint("cancelled-refresh-token"),
            90,
            Duration::from_mins(1),
            observer.clone(),
        );

        drop(lease);
        tokio::time::timeout(
            Duration::from_secs(2),
            observer.cancellation_released.notified(),
        )
        .await
        .expect("detached cancellation cleanup completed");
        let recovered = state
            .store
            .claim_refresh_token(
                "cancelled-refresh-token",
                "post-cancel",
                crate::util::now_unix() + 30,
            )
            .await
            .unwrap();
        assert!(recovered.is_some());
    }

    #[tokio::test]
    async fn refresh_claim_heartbeat_renews_and_reports_ownership_loss() {
        let state = test_auth_state_with_refreshable_google().await;
        let now = crate::util::now_unix();
        let original = crate::types::RefreshTokenRow {
            refresh_token: "heartbeat-refresh-token".to_string(),
            client_id: "client".to_string(),
            subject: "google-subject-123".to_string(),
            resource: "https://lab.example.com/mcp".to_string(),
            scope: "lab".to_string(),
            provider_refresh_token: None,
            created_at: now - 60,
            expires_at: now + 3600,
        };
        state
            .store
            .upsert_refresh_token(original.clone())
            .await
            .unwrap();
        state
            .store
            .claim_refresh_token("heartbeat-refresh-token", "heartbeat-owner", now + 10)
            .await
            .unwrap()
            .expect("claim acquired");
        let original_expiry = state
            .store
            .refresh_claim_state("heartbeat-refresh-token")
            .await
            .unwrap()
            .unwrap()
            .1;
        let observer = std::sync::Arc::new(super::RefreshClaimLeaseObserver::default());
        let (lease, mut lost) = super::RefreshClaimLease::start_with_timing_observed(
            state.store.clone(),
            "heartbeat-refresh-token".to_string(),
            "heartbeat-owner".to_string(),
            crate::util::fingerprint("heartbeat-refresh-token"),
            30,
            Duration::from_millis(10),
            observer.clone(),
        );
        tokio::time::timeout(Duration::from_secs(2), observer.renewal_finished.notified())
            .await
            .expect("heartbeat extended the original claim");
        let renewed_expiry = state
            .store
            .refresh_claim_state("heartbeat-refresh-token")
            .await
            .unwrap()
            .unwrap()
            .1;
        assert!(renewed_expiry > original_expiry);
        state
            .store
            .release_refresh_claim("heartbeat-refresh-token", "heartbeat-owner")
            .await
            .unwrap();
        let lost_error = tokio::time::timeout(Duration::from_secs(2), &mut lost)
            .await
            .expect("heartbeat detected ownership loss")
            .expect("heartbeat reports an error");
        assert_eq!(lost_error.kind(), "invalid_grant");
        drop(lease);
    }

    #[tokio::test]
    async fn successful_refresh_claim_disarm_does_not_run_cancellation_cleanup() {
        let state = test_auth_state_with_refreshable_google().await;
        let now = crate::util::now_unix();
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "completed-refresh-token".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_refresh_token: None,
                created_at: now - 60,
                expires_at: now + 3600,
            })
            .await
            .unwrap();
        state
            .store
            .claim_refresh_token("completed-refresh-token", "completed-owner", now + 90)
            .await
            .unwrap()
            .expect("claim acquired");
        let observer = std::sync::Arc::new(super::RefreshClaimLeaseObserver::default());
        let (mut lease, _lost) = super::RefreshClaimLease::start_with_timing_observed(
            state.store.clone(),
            "completed-refresh-token".to_string(),
            "completed-owner".to_string(),
            crate::util::fingerprint("completed-refresh-token"),
            90,
            Duration::from_mins(1),
            observer,
        );

        lease.disarm();
        drop(lease);

        assert_eq!(
            state
                .store
                .refresh_claim_state("completed-refresh-token")
                .await
                .unwrap()
                .unwrap()
                .0,
            "completed-owner"
        );
    }

    #[tokio::test]
    async fn cancelling_explicit_release_still_releases_durable_claim() {
        let state = test_auth_state_with_refreshable_google().await;
        let now = crate::util::now_unix();
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "release-cancel-refresh-token".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_refresh_token: None,
                created_at: now - 60,
                expires_at: now + 3600,
            })
            .await
            .unwrap();
        state
            .store
            .claim_refresh_token("release-cancel-refresh-token", "release-owner", now + 90)
            .await
            .unwrap()
            .expect("claim acquired");
        let observer = std::sync::Arc::new(super::RefreshClaimLeaseObserver::default());
        let (lease, _lost) = super::RefreshClaimLease::start_with_timing_observed(
            state.store.clone(),
            "release-cancel-refresh-token".to_string(),
            "release-owner".to_string(),
            crate::util::fingerprint("release-cancel-refresh-token"),
            90,
            Duration::from_mins(1),
            observer.clone(),
        );

        let release = tokio::spawn(lease.release());
        tokio::time::timeout(
            Duration::from_secs(2),
            observer.explicit_release_started.notified(),
        )
        .await
        .expect("explicit release reached durable cleanup");
        release.abort();
        release.await.expect_err("release request was cancelled");
        observer.explicit_release_continue.notify_one();

        let recovered = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(claim) = state
                    .store
                    .claim_refresh_token(
                        "release-cancel-refresh-token",
                        "post-release-cancel",
                        crate::util::now_unix() + 30,
                    )
                    .await
                    .unwrap()
                {
                    break claim;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached release freed the claim");
        assert_eq!(recovered.refresh_token, "release-cancel-refresh-token");
    }

    #[tokio::test]
    async fn token_endpoint_refresh_grant_preserves_stored_resource_when_omitted() {
        let state = test_auth_state_with_refreshable_google().await;
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
        assert_eq!(claims.scope, "mcp:read mcp:write");
    }

    #[tokio::test]
    async fn token_endpoint_rejects_mismatched_resource_parameter() {
        let state = test_auth_state_with_registered_client().await;
        seed_authorization_code(&state).await;
        let app = router(state);
        let response = app
            .clone()
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

        let legitimate = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(
                        header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(Body::from("grant_type=authorization_code&code=lab-code&client_id=client&resource=https%3A%2F%2Flab.example.com%2Fmcp&redirect_uri=http://127.0.0.1:7777/callback&code_verifier=verifier"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legitimate.status(), StatusCode::OK);
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

    #[tokio::test(flavor = "current_thread")]
    async fn token_endpoint_rejects_refresh_token_client_mismatch() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK.lock().await;
        let buf = crate::test_support::global_tracing_buffer();
        let state = test_auth_state_with_registered_client().await;
        let mismatched_client = "sentinel-refresh-client-secret";
        // Authenticate the second client successfully so this test reaches the
        // refresh-token binding check instead of testing unknown-client auth.
        state
            .store
            .register_client(crate::types::RegisteredClient {
                client_id: mismatched_client.to_string(),
                redirect_uris: vec!["http://127.0.0.1:8888/callback".to_string()],
                created_at: crate::util::now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        state
            .store
            .register_client(crate::types::RegisteredClient {
                client_id: mismatched_client.to_string(),
                redirect_uris: vec!["http://127.0.0.1:8888/callback".to_string()],
                created_at: crate::util::now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
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
                    .body(Body::from(format!(
                        "grant_type=refresh_token&refresh_token=refresh-token&client_id={mismatched_client}",
                    )))
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
        assert_eq!(
            json["error_description"],
            "client_id does not match the refresh token"
        );
        let logs = crate::test_support::captured_logs(buf);
        assert!(
            !logs.contains(mismatched_client),
            "refresh client ID leaked into logs: {logs}"
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
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "oauth_needs_reauth");
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
        install_google_provider_credential(state).await;
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
                expires_at,
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn refresh_grant_rotates_local_token_on_success() {
        let state = test_auth_state_with_refreshable_google().await;
        let now = crate::util::now_unix();
        state
            .store
            .upsert_google_provider_token_bundle(crate::types::GoogleProviderCredentialUpdate {
                subject: "google-subject-123".to_string(),
                email: Some("user@example.com".to_string()),
                client_id: "client-id".to_string(),
                granted_scopes: vec![
                    "openid".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                    "https://www.googleapis.com/auth/drive.readonly".to_string(),
                ],
                access_token: "pre-refresh-access".to_string(),
                refresh_token: "provider-refresh".to_string(),
                token_received_at: now,
                access_token_expires_at: now + 3600,
                issuer: Some("https://accounts.google.com".to_string()),
                refreshed: false,
                scope_upgraded: true,
            })
            .await
            .unwrap();
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
        let credential = state
            .store
            .find_google_provider_credential("google-subject-123")
            .await
            .unwrap()
            .unwrap();
        assert!(
            credential
                .granted_scopes
                .contains(&"https://www.googleapis.com/auth/drive.readonly".to_string()),
            "refresh must preserve Workspace scopes granted by earlier incremental authorization"
        );
    }

    #[tokio::test]
    async fn concurrent_refresh_requests_observe_one_rotated_response() {
        let state = test_auth_state_with_refreshable_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "concurrent-refresh".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_refresh_token: Some("provider-refresh".to_string()),
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let app = router(state);
        let request = || {
            form_request(
                "/token",
                form(&[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", "concurrent-refresh"),
                    ("client_id", "client"),
                ]),
            )
        };
        let (first, second) = tokio::join!(
            app.clone().oneshot(request()),
            app.clone().oneshot(request())
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);
        let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();
        let second_body = axum::body::to_bytes(second.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(second_body, first_body);
    }

    #[tokio::test]
    async fn refresh_generation_loss_revokes_stale_local_grant_without_issuing_replacement() {
        let base = test_auth_state_with_registered_client().await;
        let server = Box::leak(Box::new(MockServer::start().await));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(500))
                    .set_body_json(serde_json::json!({
                        "access_token": "stale-provider-access",
                        "refresh_token": "stale-provider-refresh",
                        "expires_in": 3600,
                        "id_token": super::super::authorize::tests::signed_test_id_token(),
                    })),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(super::super::authorize::tests::test_jwks()),
            )
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
        )
        .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());
        let state = AuthState::for_tests(
            (*base.config).clone(),
            base.store.clone(),
            (*base.signing_keys).clone(),
            google,
        );
        install_google_provider_credential(&state).await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "race-refresh".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_refresh_token: None,
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let peer_store = crate::sqlite::SqliteStore::open_with_key(
            state.config.sqlite_path.clone(),
            state.config.token_encryption_key.clone(),
        )
        .await
        .unwrap();
        let app = router(state.clone());
        let request = tokio::spawn(async move {
            app.oneshot(form_request(
                "/token",
                form(&[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", "race-refresh"),
                    ("client_id", "client"),
                ]),
            ))
            .await
            .unwrap()
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if server.received_requests().await.is_some_and(|requests| {
                    requests
                        .iter()
                        .any(|request| request.url.path() == "/token")
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("provider refresh request observed");
        let generation = peer_store
            .find_google_provider_credential("google-subject-123")
            .await
            .unwrap()
            .unwrap()
            .generation;
        let now = crate::util::now_unix();
        assert!(
            peer_store
                .replace_google_provider_token_bundle_if_generation(
                    crate::types::GoogleProviderCredentialUpdate {
                        subject: "google-subject-123".to_string(),
                        email: Some("user@example.com".to_string()),
                        client_id: "client-id".to_string(),
                        granted_scopes: vec!["openid".to_string()],
                        access_token: "fresh-provider-access".to_string(),
                        refresh_token: "fresh-provider-refresh".to_string(),
                        token_received_at: now,
                        access_token_expires_at: now + 3600,
                        issuer: Some("https://accounts.google.com".to_string()),
                        refreshed: true,
                        scope_upgraded: false,
                    },
                    generation,
                )
                .await
                .unwrap()
        );

        assert_eq!(request.await.unwrap().status(), StatusCode::UNAUTHORIZED);
        assert!(
            state
                .store
                .find_refresh_token("race-refresh")
                .await
                .unwrap()
                .is_none(),
            "the stale local grant must not survive provider lifecycle loss"
        );
        assert_eq!(
            state
                .store
                .find_google_provider_credential("google-subject-123")
                .await
                .unwrap()
                .unwrap()
                .refresh_token,
            "fresh-provider-refresh"
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
    async fn concurrent_transient_refresh_failure_is_shared_with_one_provider_call() {
        let (state, server) = test_auth_state_with_failing_google_refresh_and_server().await;
        let subject = "google-transient-shared-subject";
        state
            .store
            .upsert_google_provider_credential(
                subject,
                Some("user@example.com"),
                "provider-refresh",
            )
            .await
            .unwrap();
        for token in ["transient-one", "transient-two"] {
            state
                .store
                .upsert_refresh_token(crate::types::RefreshTokenRow {
                    refresh_token: token.to_string(),
                    client_id: "client".to_string(),
                    subject: subject.to_string(),
                    resource: String::new(),
                    scope: "lab".to_string(),
                    provider_refresh_token: None,
                    created_at: crate::util::now_unix() - 60,
                    expires_at: crate::util::now_unix() + 3600,
                })
                .await
                .unwrap();
        }
        let request = |token: &'static str| {
            let app = router(state.clone());
            tokio::spawn(async move {
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/token")
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(Body::from(format!(
                            "grant_type=refresh_token&refresh_token={token}&client_id=client"
                        )))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
            })
        };
        let (first, second) = tokio::join!(request("transient-one"), request("transient-two"));
        assert_ne!(first.unwrap(), StatusCode::OK);
        assert_ne!(second.unwrap(), StatusCode::OK);
        let provider_calls = server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|request| request.url.path() == "/token")
            .count();
        assert_eq!(provider_calls, 1);
    }

    #[tokio::test]
    async fn refresh_grant_surfaces_google_invalid_grant_as_oauth_needs_reauth() {
        let state = test_auth_state_with_invalid_google_refresh().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "expired-provider-token".to_string(),
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
                        "grant_type=refresh_token&refresh_token=expired-provider-token&client_id=client",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "oauth_needs_reauth");
        assert!(
            state
                .store
                .find_google_provider_credential("google-subject-123")
                .await
                .unwrap()
                .is_none(),
            "the rejected Google credential must be permanently invalidated"
        );
        assert!(
            state
                .store
                .find_refresh_token("expired-provider-token")
                .await
                .unwrap()
                .is_none(),
            "dependent local refresh tokens must be revoked so they cannot loop"
        );
    }

    #[tokio::test]
    async fn refresh_grant_does_not_elevate_legacy_scope() {
        let state = test_auth_state_with_refreshable_google().await;
        let now = crate::util::now_unix();
        state
            .store
            .upsert_google_provider_token_bundle(crate::types::GoogleProviderCredentialUpdate {
                subject: "google-subject-123".to_string(),
                email: Some("user@example.com".to_string()),
                client_id: "client-id".to_string(),
                granted_scopes: vec!["openid".to_string()],
                access_token: "cached-access".to_string(),
                refresh_token: "provider-refresh".to_string(),
                token_received_at: now,
                access_token_expires_at: now + 3600,
                issuer: Some(crate::google::GOOGLE_ISSUER.to_string()),
                refreshed: false,
                scope_upgraded: false,
            })
            .await
            .unwrap();
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
        // Decode the access token and verify refresh preserved its old scope.
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
            !scopes.contains(&"lab:admin"),
            "refresh must not elevate scope"
        );
        assert!(scopes.contains(&"lab"));
    }

    #[tokio::test]
    async fn refresh_grant_rejects_subject_removed_from_current_allowlist() {
        let authorized = test_auth_state_with_refreshable_google().await;
        authorized
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "offboarded-token".to_string(),
                client_id: "client".to_string(),
                subject: "google-subject-123".to_string(),
                resource: String::new(),
                scope: "lab:admin".to_string(),
                provider_refresh_token: None,
                created_at: crate::util::now_unix() - 60,
                expires_at: crate::util::now_unix() + 3600,
            })
            .await
            .unwrap();
        let mut config = (*authorized.config).clone();
        config.admin_email = "replacement-admin@example.com".to_string();
        let state = AuthState::for_tests(
            config,
            authorized.store.clone(),
            (*authorized.signing_keys).clone(),
            authorized.google().clone(),
        );

        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/token")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "grant_type=refresh_token&refresh_token=offboarded-token&client_id=client",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            state
                .store
                .find_refresh_token("offboarded-token")
                .await
                .unwrap()
                .is_none(),
            "offboarding must revoke the renewable local grant"
        );
    }

    #[tokio::test]
    async fn refresh_grant_rejects_reuse_after_replay_grace_expires() {
        let state = test_auth_state_with_refreshable_google().await;
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
        let app = router(state.clone());
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
        state
            .store
            .expire_refresh_token_replay("once-only-token")
            .await
            .unwrap();
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
            "a rotated refresh token must not be reusable after the retry window"
        );
    }

    #[tokio::test]
    async fn refresh_replay_window_is_capped_by_access_token_ttl() {
        let base = test_auth_state_with_refreshable_google().await;
        let mut config = (*base.config).clone();
        config.access_token_ttl = Duration::from_secs(30);
        let state = AuthState::for_tests(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            base.google().clone(),
        );
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "short-access-token-replay".to_string(),
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
        let before = crate::util::now_unix();
        let response = router(state.clone())
            .oneshot(form_request(
                "/token",
                "grant_type=refresh_token&refresh_token=short-access-token-replay&client_id=client"
                    .to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let after = crate::util::now_unix();
        let expires_at = state
            .store
            .refresh_token_replay_expires_at("short-access-token-replay")
            .await
            .unwrap()
            .unwrap();
        assert!(expires_at > before);
        assert!(expires_at <= after + 30);
    }

    #[tokio::test]
    async fn refresh_grant_replay_rejects_a_revoked_replacement_token() {
        let state = test_auth_state_with_refreshable_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "pre-revocation-token".to_string(),
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
        let app = router(state.clone());
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    "grant_type=refresh_token&refresh_token=pre-revocation-token&client_id=client",
                ))
                .unwrap()
        };

        let first = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();
        let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
        let replacement = first_json["refresh_token"]
            .as_str()
            .expect("rotated refresh token");
        state.store.revoke_refresh_token(replacement).await.unwrap();

        let replay = app.oneshot(request()).await.unwrap();
        assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn revoking_a_rotated_predecessor_revokes_its_replay_successor() {
        let state = test_auth_state_with_refreshable_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "revoked-predecessor-token".to_string(),
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
        let refresh = || {
            form_request(
                "/token",
                "grant_type=refresh_token&refresh_token=revoked-predecessor-token&client_id=client"
                    .to_string(),
            )
        };
        let rotated = app.clone().oneshot(refresh()).await.unwrap();
        assert_eq!(rotated.status(), StatusCode::OK);
        let rotated_body = axum::body::to_bytes(rotated.into_body(), usize::MAX)
            .await
            .unwrap();
        let rotated_json: serde_json::Value = serde_json::from_slice(&rotated_body).unwrap();
        let successor = rotated_json["refresh_token"]
            .as_str()
            .expect("rotated refresh token")
            .to_string();
        let revoke = form_request(
            "/revoke",
            "token=revoked-predecessor-token&client_id=client".to_string(),
        );
        assert_eq!(
            app.clone().oneshot(revoke).await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(
            app.clone().oneshot(refresh()).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            app.oneshot(form_request(
                "/token",
                form(&[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", successor.as_str()),
                    ("client_id", "client"),
                ]),
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn refresh_grant_replay_rejects_a_consumed_replacement_token() {
        let state = test_auth_state_with_refreshable_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "first-generation-token".to_string(),
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
        let first_request = || {
            form_request(
                "/token",
                "grant_type=refresh_token&refresh_token=first-generation-token&client_id=client"
                    .to_string(),
            )
        };
        let first = app.clone().oneshot(first_request()).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();
        let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
        let second_generation = first_json["refresh_token"]
            .as_str()
            .expect("second-generation refresh token");

        let second = app
            .clone()
            .oneshot(form_request(
                "/token",
                form(&[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", second_generation),
                    ("client_id", "client"),
                ]),
            ))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);

        let stale_replay = app.oneshot(first_request()).await.unwrap();
        assert_eq!(stale_replay.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn refresh_grant_replay_survives_auth_state_restart() {
        let state = test_auth_state_with_refreshable_google().await;
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "restart-replay-token".to_string(),
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
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    "grant_type=refresh_token&refresh_token=restart-replay-token&client_id=client",
                ))
                .unwrap()
        };
        let first = router(state.clone()).oneshot(request()).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();

        let reopened_store = state.store.reopen_for_test().await.unwrap();
        let restarted = AuthState::for_tests(
            (*state.config).clone(),
            reopened_store,
            (*state.signing_keys).clone(),
            state.google().clone(),
        );
        let replay = router(restarted).oneshot(request()).await.unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let replay_body = axum::body::to_bytes(replay.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(replay_body, first_body);
    }

    #[tokio::test]
    async fn refresh_grant_replay_rejects_a_different_resource() {
        let state = test_auth_state_with_refreshable_google().await;
        state.set_allowed_resource_urls(["https://other.example.com/mcp".to_string()]);
        state
            .store
            .upsert_refresh_token(crate::types::RefreshTokenRow {
                refresh_token: "resource-bound-replay-token".to_string(),
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
        let first = app
            .clone()
            .oneshot(form_request(
                "/token",
                "grant_type=refresh_token&refresh_token=resource-bound-replay-token&client_id=client"
                    .to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let replay = app
            .oneshot(form_request(
                "/token",
                "grant_type=refresh_token&refresh_token=resource-bound-replay-token&client_id=client&resource=https%3A%2F%2Fother.example.com%2Fmcp"
                    .to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
    }
}
