use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Header, Validation, decode, decode_header};
use reqwest::Url;
use reqwest::header;
use serde::Deserialize;
use tokio::sync::{Mutex, OnceCell, RwLock, Semaphore};
use tracing::{debug, info, warn};

use crate::error::AuthError;
use crate::oauth_provider::ProviderExchange;
use crate::provider_http::{RequestErrors, RequestTrace, read_json_response};
use crate::util::fingerprint;

/// Which RFC 6749 §2.3.1 client-authentication method to use on the token
/// endpoint (`exchange_code`/`refresh`).
///
/// Google's token endpoint accepts `client_secret` in the POST body
/// regardless of how the client credential was provisioned, so Google keeps
/// using [`Self::ClientSecretPost`] (this crate's original, pre-multi-provider
/// behavior). Authelia's OIDC provider defaults confidential clients to
/// `client_secret_basic` unless the operator explicitly sets
/// `token_endpoint_auth_method: client_secret_post` on the client — sending
/// `client_secret` in the body instead of the `Authorization` header would
/// silently fail authentication against that (documented) default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenAuthMethod {
    /// `client_id`/`client_secret` in the POST body (RFC 6749 §2.3.1,
    /// "NOT RECOMMENDED" but widely supported; Google's default).
    ClientSecretPost,
    /// `client_id`/`client_secret` via HTTP Basic `Authorization` header
    /// (RFC 6749 §2.3.1's default/recommended method; Authelia's default).
    ClientSecretBasic,
}

const DEFAULT_JWKS_TTL: Duration = Duration::from_hours(1);
/// Per-request timeout on the JWKS GET. Bound aggressively (5s) so a slow
/// upstream JWKS endpoint cannot consume admission capacity or hold
/// single-flight waiters indefinitely. Token exchange / refresh keep the
/// provider's own looser timeout because those can legitimately take longer.
const JWKS_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_ADMISSION_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Deserialize)]
pub(crate) struct IdTokenClaims {
    pub iss: String,
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: Option<bool>,
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default)]
    pub azp: Option<String>,
    #[serde(default)]
    pub aud: Audience,
    pub iat: i64,
    #[serde(default)]
    pub nbf: Option<i64>,
    #[serde(default)]
    pub at_hash: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
pub(crate) enum Audience {
    One(String),
    Many(Vec<String>),
    #[default]
    Missing,
}

#[derive(Debug, Deserialize)]
struct OidcTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    id_token: String,
}

#[derive(Clone, Debug, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Clone, Debug, Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    #[serde(default)]
    alg: Option<String>,
    #[serde(rename = "use", default)]
    usage: Option<String>,
    #[serde(default)]
    key_ops: Vec<String>,
    n: String,
    e: String,
}

#[derive(Clone, Debug)]
struct CachedJwks {
    jwks: Jwks,
    expires_at: Instant,
}

type JwksFlight = Arc<OnceCell<Result<Jwks, AuthError>>>;

/// RS256 ID-token verifier used by the Authelia inbound provider. Caches the
/// provider's JWKS document and validates
/// signature, expiry, audience, and issuer on every [`Self::verify`] call.
///
/// The verifier is intentionally kept separate from Google's established
/// verifier contract: Google has provider-specific issuer aliases, hosted
/// domain claims, and token-response behavior. `Clone` is cheap because all
/// mutable state is held behind shared handles.
#[derive(Clone)]
pub(crate) struct OidcVerifier {
    provider_id: &'static str,
    issuer: String,
    jwks_endpoint: Url,
    http: reqwest::Client,
    jwks_cache: Arc<RwLock<Option<CachedJwks>>>,
    refresh_flight: Arc<Mutex<Option<JwksFlight>>>,
    last_forced_refresh: Arc<Mutex<Option<Instant>>>,
    request_permits: Arc<Semaphore>,
    /// Token-endpoint client authentication method. See [`TokenAuthMethod`].
    token_auth_method: TokenAuthMethod,
}

impl OidcVerifier {
    /// Explicitly warm and validate the bounded production JWKS cache.
    pub(crate) async fn probe_jwks(&self) -> Result<(), AuthError> {
        self.fetch_jwks().await.map(|_| ())
    }
    pub(crate) fn new(
        provider_id: &'static str,
        issuer: String,
        jwks_endpoint: Url,
        http: reqwest::Client,
    ) -> Self {
        Self {
            provider_id,
            issuer,
            jwks_endpoint,
            http,
            jwks_cache: Arc::new(RwLock::new(None)),
            refresh_flight: Arc::new(Mutex::new(None)),
            last_forced_refresh: Arc::new(Mutex::new(None)),
            request_permits: Arc::new(Semaphore::new(16)),
            token_auth_method: TokenAuthMethod::ClientSecretPost,
        }
    }

    /// Override the token-endpoint client authentication method. Defaults to
    /// [`TokenAuthMethod::ClientSecretPost`] (Google's behavior); Authelia
    /// opts into [`TokenAuthMethod::ClientSecretBasic`].
    #[must_use]
    pub(crate) fn with_token_auth_method(mut self, method: TokenAuthMethod) -> Self {
        self.token_auth_method = method;
        self
    }

    /// Builds the token-endpoint POST request, applying `client_id`/
    /// `client_secret` per [`Self::token_auth_method`]: appended to the form
    /// body for [`TokenAuthMethod::ClientSecretPost`], or sent via the HTTP
    /// `Authorization: Basic` header (and omitted from the body, per RFC
    /// 6749 §3.2.1 — a client authenticated at the transport level does not
    /// also repeat its credentials in the body) for
    /// [`TokenAuthMethod::ClientSecretBasic`].
    fn token_request<'a>(
        &self,
        token_endpoint: &Url,
        client_id: &'a str,
        client_secret: &'a str,
        mut form: Vec<(&'a str, &'a str)>,
    ) -> reqwest::RequestBuilder {
        let mut request = self.http.post(token_endpoint.clone());
        match self.token_auth_method {
            TokenAuthMethod::ClientSecretPost => {
                form.push(("client_id", client_id));
                form.push(("client_secret", client_secret));
            }
            TokenAuthMethod::ClientSecretBasic => {
                request = request.basic_auth(client_id, Some(client_secret));
            }
        }
        request.form(&form)
    }

    pub(crate) async fn exchange_code(
        &self,
        token_endpoint: &Url,
        client_id: &str,
        client_secret: &str,
        redirect_uri: &Url,
        code: &str,
        code_verifier: &str,
        expected_nonce: Option<&str>,
    ) -> Result<ProviderExchange, AuthError> {
        let trace = RequestTrace::start(self.provider_id, "code_exchange", "POST", token_endpoint);
        info!(
            provider = self.provider_id,
            oauth_code_id = %fingerprint(code),
            redirect_uri_id = %fingerprint(redirect_uri.as_str()),
            "oauth upstream code exchange started"
        );
        let payload: OidcTokenResponse = {
            let _permit = self.acquire_request_permit().await?;
            read_json_response(
                trace,
                self.token_request(
                    token_endpoint,
                    client_id,
                    client_secret,
                    vec![
                        ("grant_type", "authorization_code"),
                        ("code", code),
                        ("redirect_uri", redirect_uri.as_str()),
                        ("code_verifier", code_verifier),
                    ],
                ),
                RequestErrors::new(
                    self.provider_id,
                    format!("exchange {} auth code", self.provider_id),
                    format!("{} token endpoint error", self.provider_id),
                    format!("decode {} token response", self.provider_id),
                ),
            )
            .await?
        };
        self.finish_exchange(payload, client_id, expected_nonce, "code_exchange")
            .await
    }

    async fn finish_exchange(
        &self,
        payload: OidcTokenResponse,
        client_id: &str,
        expected_nonce: Option<&str>,
        operation: &'static str,
    ) -> Result<ProviderExchange, AuthError> {
        let claims = self
            .verify(&payload.id_token, client_id, expected_nonce)
            .await?;
        if let Some(expected) = claims.at_hash.as_deref() {
            use base64::Engine;
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(payload.access_token.as_bytes());
            let actual = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(&digest[..digest.len() / 2]);
            if actual != expected {
                return Err(AuthError::AuthFailed(format!(
                    "invalid {} id_token access-token hash",
                    self.provider_id
                )));
            }
        }
        if claims.email.as_deref().is_none_or(str::is_empty) || claims.email_verified != Some(true)
        {
            return Err(AuthError::AuthFailed(format!(
                "{} did not return a verified email address",
                self.provider_id
            )));
        }
        if operation == "code_exchange" {
            info!(
                provider = self.provider_id,
                subject_id = %fingerprint(&claims.sub),
                has_refresh_token = payload.refresh_token.is_some(),
                expires_in_secs = payload.expires_in,
                "oauth upstream code exchange succeeded"
            );
        } else {
            info!(
                provider = self.provider_id,
                subject_id = %fingerprint(&claims.sub),
                has_refresh_token = payload.refresh_token.is_some(),
                expires_in_secs = payload.expires_in,
                "oauth upstream refresh succeeded"
            );
        }
        Ok(ProviderExchange {
            subject: claims.sub,
            email: claims.email,
            email_verified: claims.email_verified,
            hosted_domain: None,
            access_token: payload.access_token,
            refresh_token: payload.refresh_token,
            expires_in: payload.expires_in,
            granted_scopes: Vec::new(),
            id_token: Some(payload.id_token),
        })
    }

    pub(crate) async fn verify(
        &self,
        id_token: &str,
        audience: &str,
        expected_nonce: Option<&str>,
    ) -> Result<IdTokenClaims, AuthError> {
        if id_token.len() > 64 * 1024 {
            return Err(AuthError::AuthFailed(format!(
                "{} id_token exceeds 64 KiB",
                self.provider_id
            )));
        }
        let header = decode_header(id_token).map_err(|error| {
            AuthError::Storage(format!("verify {} id_token: {error}", self.provider_id))
        })?;
        validate_header_alg(self.provider_id, &header)?;
        let kid = header.kid.ok_or_else(|| {
            AuthError::Storage(format!("{} id_token is missing a key id", self.provider_id))
        })?;
        let key = self.find_jwk_for_kid(&kid).await?;
        if let Some(alg) = key.alg.as_deref()
            && alg != "RS256"
        {
            return Err(AuthError::Storage(format!(
                "{} JWKS key `{}` uses unsupported algorithm `{alg}`",
                self.provider_id, key.kid
            )));
        }

        let decoding_key = DecodingKey::from_rsa_components(&key.n, &key.e).map_err(|error| {
            AuthError::Storage(format!(
                "build {} id_token decoding key: {error}",
                self.provider_id
            ))
        })?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = true;
        validation.leeway = 60;
        validation.validate_nbf = true;
        validation.set_audience(&[audience]);

        let claims = decode::<IdTokenClaims>(id_token, &decoding_key, &validation)
            .map(|data| data.claims)
            .map_err(|error| {
                AuthError::Storage(format!("invalid {} id_token: {error}", self.provider_id))
            })?;

        validate_claims(
            self.provider_id,
            &self.issuer,
            audience,
            expected_nonce,
            &claims,
        )?;

        Ok(claims)
    }

    async fn find_jwk_for_kid(&self, kid: &str) -> Result<Jwk, AuthError> {
        let jwks = self.fetch_jwks().await?;
        if let Some(key) = jwks.keys.into_iter().find(|key| key.kid == kid) {
            return Ok(key);
        }

        debug!(
            provider = self.provider_id,
            "jwks cache miss; attempting bounded refresh"
        );
        {
            let mut last = self.last_forced_refresh.lock().await;
            if last.is_some_and(|instant| instant.elapsed() < Duration::from_secs(5)) {
                return Err(AuthError::AuthFailed(format!(
                    "{} id_token key id was not found in JWKS",
                    self.provider_id
                )));
            }
            *last = Some(Instant::now());
        }
        self.refresh_jwks()
            .await?
            .keys
            .into_iter()
            .find(|key| key.kid == kid)
            .ok_or_else(|| {
                AuthError::Storage(format!(
                    "{} id_token key id was not found in JWKS",
                    self.provider_id
                ))
            })
    }

    async fn fetch_jwks(&self) -> Result<Jwks, AuthError> {
        if let Some(jwks) = self.cached_jwks().await {
            debug!(provider = self.provider_id, "jwks cache hit");
            return Ok(jwks);
        }

        self.run_jwks_flight(false).await
    }

    async fn refresh_jwks(&self) -> Result<Jwks, AuthError> {
        self.run_jwks_flight(true).await
    }

    async fn run_jwks_flight(&self, force: bool) -> Result<Jwks, AuthError> {
        let flight = {
            let mut slot = self.refresh_flight.lock().await;
            slot.get_or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let result = flight
            .get_or_init(|| async {
                if !force && let Some(jwks) = self.cached_jwks().await {
                    return Ok(jwks);
                }
                self.fetch_jwks_network().await
            })
            .await
            .clone();
        let mut slot = self.refresh_flight.lock().await;
        if slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &flight))
        {
            *slot = None;
        }
        result
    }

    async fn fetch_jwks_network(&self) -> Result<Jwks, AuthError> {
        let _permit = self.acquire_request_permit().await?;
        let trace = RequestTrace::start(self.provider_id, "fetch_jwks", "GET", &self.jwks_endpoint);
        let response = self
            .http
            .get(self.jwks_endpoint.clone())
            .timeout(JWKS_FETCH_TIMEOUT)
            .send()
            .await
            .map_err(|error| {
                let error = error.without_url();
                trace.error(None, &error);
                warn!(provider = self.provider_id, error = %error, "jwks request failed");
                AuthError::Storage(format!("fetch {} jwks: {error}", self.provider_id))
            })?;
        let status = response.status();
        let ttl = jwks_ttl(response.headers());
        let response = response.error_for_status().map_err(|error| {
            let error = error.without_url();
            trace.error(Some(status), &error);
            warn!(provider = self.provider_id, error = %error, "jwks request returned error status");
            AuthError::Storage(format!("{} jwks endpoint error: {error}", self.provider_id))
        })?;
        trace.finish(status);
        let jwks =
            crate::provider_http::bounded_json::<Jwks>(response, "OIDC JWKS document").await?;
        validate_jwks(self.provider_id, &jwks)?;

        *self.jwks_cache.write().await = Some(CachedJwks {
            jwks: jwks.clone(),
            expires_at: Instant::now() + ttl,
        });

        Ok(jwks)
    }

    async fn acquire_request_permit(&self) -> Result<tokio::sync::SemaphorePermit<'_>, AuthError> {
        tokio::time::timeout(REQUEST_ADMISSION_TIMEOUT, self.request_permits.acquire())
            .await
            .map_err(|_| AuthError::RateLimited {
                message: "OIDC provider request capacity is saturated".into(),
                retry_after_ms: REQUEST_ADMISSION_TIMEOUT.as_millis() as u64,
            })?
            .map_err(|_| AuthError::Server("OIDC request limiter closed".into()))
    }

    async fn cached_jwks(&self) -> Option<Jwks> {
        let cache = self.jwks_cache.read().await;
        cache
            .as_ref()
            .filter(|cached| cached.expires_at > Instant::now())
            .map(|cached| cached.jwks.clone())
    }
}

fn validate_claims(
    provider_id: &str,
    issuer: &str,
    audience: &str,
    expected_nonce: Option<&str>,
    claims: &IdTokenClaims,
) -> Result<(), AuthError> {
    let invalid = |field| AuthError::AuthFailed(format!("invalid {provider_id} id_token {field}"));
    if claims.iss != issuer {
        return Err(invalid("issuer"));
    }
    if claims.sub.is_empty() || claims.sub.len() > 255 {
        return Err(invalid("subject"));
    }
    if expected_nonce.is_some() && claims.nonce.as_deref() != expected_nonce {
        return Err(invalid("nonce"));
    }
    let audiences = match &claims.aud {
        Audience::One(value) => std::slice::from_ref(value),
        Audience::Many(values) => values.as_slice(),
        Audience::Missing => &[],
    };
    if audiences.is_empty()
        || audiences.len() > 8
        || !audiences.iter().any(|value| value == audience)
        || audiences
            .iter()
            .any(|value| value.is_empty() || value.len() > 1024)
    {
        return Err(invalid("audience"));
    }
    if claims
        .nonce
        .as_deref()
        .is_some_and(|value| value.len() > 255)
        || claims
            .email
            .as_deref()
            .is_some_and(|value| value.len() > 320)
        || claims
            .azp
            .as_deref()
            .is_some_and(|value| value.len() > 1024)
    {
        return Err(invalid("claim size"));
    }
    if claims.azp.as_deref().is_some_and(|value| value != audience)
        || (audiences.len() > 1 && claims.azp.as_deref() != Some(audience))
    {
        return Err(invalid("authorized party"));
    }
    let now = crate::util::now_unix();
    if claims.iat > now.saturating_add(60) {
        return Err(invalid("issued-at time"));
    }
    if claims.nbf.is_some_and(|nbf| nbf > now.saturating_add(60)) {
        return Err(invalid("not-before time"));
    }
    Ok(())
}

fn jwks_ttl(headers: &header::HeaderMap) -> Duration {
    headers
        .get(header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_max_age)
        .map_or(DEFAULT_JWKS_TTL, |seconds| {
            Duration::from_secs(seconds.clamp(60, 3600))
        })
}

fn validate_jwks(provider_id: &str, jwks: &Jwks) -> Result<(), AuthError> {
    use base64::Engine;
    use std::collections::HashSet;
    if jwks.keys.is_empty() || jwks.keys.len() > 32 {
        return Err(AuthError::Validation(format!(
            "{provider_id} JWKS must contain 1 to 32 keys"
        )));
    }
    let mut kids = HashSet::new();
    for key in &jwks.keys {
        if key.kid.is_empty() || key.kid.len() > 255 || !kids.insert(&key.kid) {
            return Err(AuthError::Validation(format!(
                "{provider_id} JWKS contains an invalid or duplicate key id"
            )));
        }
        if key.kty != "RSA"
            || key.usage.as_deref().is_some_and(|value| value != "sig")
            || (!key.key_ops.is_empty() && !key.key_ops.iter().any(|value| value == "verify"))
        {
            return Err(AuthError::Validation(format!(
                "{provider_id} JWKS contains a key not valid for RSA signature verification"
            )));
        }
        let modulus = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&key.n)
            .map_err(|_| {
                AuthError::Validation(format!(
                    "{provider_id} JWKS contains invalid RSA key material"
                ))
            })?;
        if modulus.len() < 256 {
            return Err(AuthError::Validation(format!(
                "{provider_id} JWKS RSA key is weaker than 2048 bits"
            )));
        }
        let exponent = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&key.e)
            .map_err(|_| {
                AuthError::Validation(format!("{provider_id} JWKS contains invalid RSA exponent"))
            })?;
        if exponent.is_empty()
            || exponent.len() > 8
            || exponent.last().is_none_or(|byte| byte & 1 == 0)
        {
            return Err(AuthError::Validation(format!(
                "{provider_id} JWKS contains an invalid RSA exponent"
            )));
        }
    }
    Ok(())
}

fn parse_max_age(cache_control: &str) -> Option<u64> {
    cache_control.split(',').find_map(|directive| {
        let directive = directive.trim();
        let value = directive.strip_prefix("max-age=")?;
        value.parse::<u64>().ok()
    })
}

fn validate_header_alg(provider_id: &str, header: &Header) -> Result<(), AuthError> {
    if header.alg != Algorithm::RS256 {
        return Err(AuthError::Storage(format!(
            "verify {provider_id} id_token: unsupported algorithm `{:?}`",
            header.alg
        )));
    }
    if header.jku.is_some() || header.jwk.is_some() || header.x5u.is_some() {
        return Err(AuthError::AuthFailed(format!(
            "verify {provider_id} id_token: remote or embedded key headers are forbidden"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Audience, IdTokenClaims, validate_claims};

    fn claims() -> IdTokenClaims {
        IdTokenClaims {
            iss: "https://auth.example.test".into(),
            sub: "subject".into(),
            email: Some("user@example.test".into()),
            email_verified: Some(true),
            nonce: Some("nonce".into()),
            azp: Some("client".into()),
            aud: Audience::One("client".into()),
            iat: crate::util::now_unix(),
            nbf: None,
            at_hash: None,
        }
    }

    #[test]
    fn claims_reject_wrong_nonce_audience_azp_and_future_times() {
        let validate = |claims: &IdTokenClaims| {
            validate_claims(
                "authelia",
                "https://auth.example.test",
                "client",
                Some("nonce"),
                claims,
            )
        };
        assert!(validate(&claims()).is_ok());
        let mut invalid = claims();
        invalid.nonce = Some("wrong".into());
        assert!(validate(&invalid).is_err());
        let mut invalid = claims();
        invalid.aud = Audience::One("wrong".into());
        assert!(validate(&invalid).is_err());
        let mut invalid = claims();
        invalid.azp = Some("wrong".into());
        assert!(validate(&invalid).is_err());
        let mut invalid = claims();
        invalid.iat += 120;
        assert!(validate(&invalid).is_err());
        let mut invalid = claims();
        invalid.nbf = Some(crate::util::now_unix() + 120);
        assert!(validate(&invalid).is_err());
    }

    #[test]
    fn claims_bound_identity_and_collection_sizes() {
        let validate = |claims: &IdTokenClaims| {
            validate_claims(
                "authelia",
                "https://auth.example.test",
                "client",
                Some("nonce"),
                claims,
            )
        };
        let mut invalid = claims();
        invalid.sub = "x".repeat(256);
        assert!(validate(&invalid).is_err());
        let mut invalid = claims();
        invalid.email = Some("x".repeat(321));
        assert!(validate(&invalid).is_err());
        let mut invalid = claims();
        invalid.aud = Audience::Many((0..9).map(|_| "client".into()).collect());
        assert!(validate(&invalid).is_err());
    }

    #[test]
    fn parse_max_age_reads_cache_control_max_age() {
        assert_eq!(super::parse_max_age("public, max-age=3600"), Some(3600));
        assert_eq!(super::parse_max_age("no-cache"), None);
    }
}
