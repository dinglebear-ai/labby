use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Header, Validation, decode, decode_header};
use reqwest::Url;
use reqwest::header;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::error::AuthError;
use crate::util::fingerprint;

const GOOGLE_AUTHORIZE_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_JWKS_ENDPOINT: &str = "https://www.googleapis.com/oauth2/v3/certs";
pub(crate) const GOOGLE_ISSUER: &str = "https://accounts.google.com";
const GOOGLE_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// Per-request timeout on the JWKS GET. Bound aggressively (5s) so a slow
/// google.com call cannot starve a tokio worker holding the JWKS write
/// lock. Token exchange / refresh keep the looser 30s bound because they
/// can legitimately take longer.
const GOOGLE_JWKS_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const GOOGLE_DEFAULT_JWKS_TTL: Duration = Duration::from_hours(1);
const GOOGLE_MAX_JSON_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct AuthorizeUrlRequest {
    pub state: String,
    pub scope: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    /// Request a reusable offline credential. Browser-session login sets this
    /// to false because it needs only verified identity and must not mint an
    /// unused refresh token. MCP authorization and scope upgrades set it true.
    pub offline_access: bool,
    /// Force Google's full "wants access" consent screen even if the user
    /// already granted these scopes. Needed the first time (to guarantee a
    /// refresh token comes back), but forcing it on every retry adds a slow,
    /// interactive round trip that impatient MCP clients can time out on
    /// before the human finishes clicking through it.
    pub force_consent: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct GoogleReauthRequest {
    pub state: String,
    pub nonce: String,
    pub code_challenge: String,
}

impl std::fmt::Debug for GoogleReauthRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleReauthRequest")
            .field("state", &"<redacted>")
            .field("nonce", &"<redacted>")
            .field("code_challenge", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct GoogleFreshAuth {
    subject: String,
    authenticated_at: i64,
}

impl GoogleFreshAuth {
    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn authenticated_at(&self) -> i64 {
        self.authenticated_at
    }
}

impl std::fmt::Debug for GoogleFreshAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleFreshAuth")
            .field("subject", &"<redacted>")
            .field("authenticated_at", &self.authenticated_at)
            .finish()
    }
}

impl std::fmt::Debug for AuthorizeUrlRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizeUrlRequest")
            .field("state", &"<redacted>")
            .field("scope", &self.scope)
            .field("code_challenge", &"<redacted>")
            .field("code_challenge_method", &self.code_challenge_method)
            .field("offline_access", &self.offline_access)
            .field("force_consent", &self.force_consent)
            .finish()
    }
}

#[derive(Clone)]
pub struct GoogleProvider {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: Url,
    pub scopes: Vec<String>,
    pub http: reqwest::Client,
    authorize_endpoint: Url,
    token_endpoint: Url,
    jwks_endpoint: Url,
    jwks_cache: Arc<RwLock<Option<CachedGoogleJwks>>>,
}

impl std::fmt::Debug for GoogleProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleProvider")
            .field("client_id", &self.client_id)
            .field("redirect_uri", &self.redirect_uri)
            .field("scopes", &self.scopes)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct GoogleExchange {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    /// Google Workspace hosted domain (`hd` claim), when the account has one.
    pub hosted_domain: Option<String>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub granted_scopes: Vec<String>,
    pub id_token: Option<String>,
}

impl std::fmt::Debug for GoogleExchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleExchange")
            .field("subject", &"<redacted>")
            .field("email", &self.email.as_ref().map(|_| "<redacted>"))
            .field("email_verified", &self.email_verified)
            .field(
                "hosted_domain",
                &self.hosted_domain.as_ref().map(|_| "<redacted>"),
            )
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_in", &self.expires_in)
            .field("granted_scope_count", &self.granted_scopes.len())
            .field("id_token", &self.id_token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct GoogleIdentity {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    /// Google Workspace hosted domain (`hd` claim), when the account has one.
    pub hosted_domain: Option<String>,
}

impl std::fmt::Debug for GoogleIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleIdentity")
            .field("subject", &"<redacted>")
            .field("email", &self.email.as_ref().map(|_| "<redacted>"))
            .field("email_verified", &self.email_verified)
            .field(
                "hosted_domain",
                &self.hosted_domain.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleTokenErrorResponse {
    error: String,
}

#[derive(Deserialize)]
struct GoogleIdTokenClaims {
    iss: String,
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<bool>,
    /// Google Workspace hosted domain. Present only for accounts hosted in a
    /// Workspace domain; consumer accounts omit it.
    #[serde(default)]
    hd: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    auth_time: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
struct GoogleJwks {
    keys: Vec<GoogleJwk>,
}

#[derive(Clone, Debug, Deserialize)]
struct GoogleJwk {
    kid: String,
    #[serde(default)]
    alg: Option<String>,
    n: String,
    e: String,
}

#[derive(Clone, Debug)]
struct CachedGoogleJwks {
    jwks: GoogleJwks,
    expires_at: Instant,
}

fn parse_google_scopes(raw: &str) -> Vec<String> {
    merge_google_scopes(
        &[],
        &raw.split_whitespace()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>(),
    )
}

fn canonical_google_scope(scope: &str) -> &str {
    match scope {
        "https://www.googleapis.com/auth/userinfo.email" => "email",
        "https://www.googleapis.com/auth/userinfo.profile" => "profile",
        other => other,
    }
}

/// Normalize and union Google scopes across incremental authorization responses.
///
/// Google combines grants when `include_granted_scopes=true`, but client libraries
/// may resolve an omitted `scope` field to only the scopes requested by the latest
/// authorization. Persisting the union prevents one Workspace MCP connection from
/// erasing scopes already granted for another connection.
pub(crate) fn merge_google_scopes(existing: &[String], returned: &[String]) -> Vec<String> {
    let mut scopes: Vec<String> = existing
        .iter()
        .chain(returned.iter())
        .map(|scope| canonical_google_scope(scope.trim()))
        .filter(|scope| !scope.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    scopes.sort();
    scopes.dedup();
    scopes
}

struct GoogleRequestTrace<'a> {
    operation: &'static str,
    method: &'static str,
    endpoint: &'a Url,
    started: Instant,
}

impl<'a> GoogleRequestTrace<'a> {
    fn start(operation: &'static str, method: &'static str, endpoint: &'a Url) -> Self {
        info!(
            provider = "google",
            operation,
            method,
            host = endpoint.host_str().unwrap_or_default(),
            path = endpoint.path(),
            "request.start"
        );
        Self {
            operation,
            method,
            endpoint,
            started: Instant::now(),
        }
    }

    fn finish(&self, status: reqwest::StatusCode) {
        info!(
            provider = "google",
            operation = self.operation,
            method = self.method,
            host = self.endpoint.host_str().unwrap_or_default(),
            path = self.endpoint.path(),
            status = status.as_u16(),
            elapsed_ms = self.started.elapsed().as_millis(),
            "request.finish"
        );
    }

    fn error(&self, status: Option<reqwest::StatusCode>, error: &reqwest::Error) {
        if let Some(status) = status {
            warn!(
                provider = "google",
                operation = self.operation,
                method = self.method,
                host = self.endpoint.host_str().unwrap_or_default(),
                path = self.endpoint.path(),
                status = status.as_u16(),
                elapsed_ms = self.started.elapsed().as_millis(),
                error = %error,
                "request.error"
            );
        } else {
            warn!(
                provider = "google",
                operation = self.operation,
                method = self.method,
                host = self.endpoint.host_str().unwrap_or_default(),
                path = self.endpoint.path(),
                elapsed_ms = self.started.elapsed().as_millis(),
                error = %error,
                "request.error"
            );
        }
    }
}

struct GoogleRequestErrors {
    transport_context: &'static str,
    transport_log: &'static str,
    status_context: &'static str,
    status_log: &'static str,
    decode_context: &'static str,
    decode_log: &'static str,
    invalid_grant_requires_reauth: bool,
}

async fn decode_capped_json<T: DeserializeOwned>(
    mut response: reqwest::Response,
    decode_context: &'static str,
) -> Result<T, AuthError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        AuthError::Network(format!("{decode_context}: read response body: {error}"))
    })? {
        let next_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or(AuthError::ResponseTooLarge {
                limit: GOOGLE_MAX_JSON_RESPONSE_BYTES,
            })?;
        if next_len > GOOGLE_MAX_JSON_RESPONSE_BYTES {
            return Err(AuthError::ResponseTooLarge {
                limit: GOOGLE_MAX_JSON_RESPONSE_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|error| AuthError::Decode(format!("{decode_context}: {error}")))
}

async fn read_json_response<T: DeserializeOwned>(
    trace: GoogleRequestTrace<'_>,
    request: reqwest::RequestBuilder,
    errors: GoogleRequestErrors,
) -> Result<T, AuthError> {
    let response = request.send().await.map_err(|error| {
        let auth_error = AuthError::Network(format!("{}: {error}", errors.transport_context));
        trace.error(None, &error);
        warn!(
            provider = "google",
            error = %error,
            kind = auth_error.kind(),
            "{}",
            errors.transport_log
        );
        auth_error
    })?;
    let status = response.status();
    let retry_after_ms = response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1_000));
    if let Err(error) = response.error_for_status_ref() {
        let auth_error = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            AuthError::RateLimited {
                message: format!("{}: {}", errors.status_context, status),
                retry_after_ms: retry_after_ms.unwrap_or(1_000),
            }
        } else if status.is_server_error() {
            AuthError::Server(format!("{}: {error}", errors.status_context))
        } else if errors.invalid_grant_requires_reauth {
            match decode_capped_json::<GoogleTokenErrorResponse>(response, errors.decode_context)
                .await
            {
                Ok(payload) if payload.error == "invalid_grant" => AuthError::OauthNeedsReauth(
                    "google refresh token is expired or revoked; reauthorization required"
                        .to_string(),
                ),
                Err(error @ AuthError::ResponseTooLarge { .. }) => error,
                _ => AuthError::AuthFailed(format!("{}: {error}", errors.status_context)),
            }
        } else {
            AuthError::AuthFailed(format!("{}: {error}", errors.status_context))
        };
        trace.error(Some(status), &error);
        warn!(
            provider = "google",
            error = %error,
            kind = auth_error.kind(),
            "{}",
            errors.status_log
        );
        return Err(auth_error);
    }
    trace.finish(status);
    decode_capped_json(response, errors.decode_context)
        .await
        .map_err(|auth_error| {
            warn!(
                provider = "google",
                error = %auth_error,
                kind = auth_error.kind(),
                "{}",
                errors.decode_log
            );
            auth_error
        })
}

impl GoogleProvider {
    pub fn new(
        client_id: String,
        client_secret: String,
        redirect_uri: Url,
    ) -> Result<Self, AuthError> {
        // reqwest is built workspace-wide with "rustls-no-provider" (root
        // Cargo.toml) so it never silently pulls in aws-lc-rs; a rustls
        // crypto provider must be installed before the first TLS-capable
        // client is built. `crates/labby/src/entrypoint.rs::run()` does this
        // for the real binary; test binaries never go through it, so this
        // call is also needed here. Idempotent -- Err just means a provider
        // (this one) is already installed, safe to ignore.
        drop(rustls::crypto::ring::default_provider().install_default());
        let http = reqwest::Client::builder()
            .timeout(GOOGLE_HTTP_TIMEOUT)
            .build()
            .map_err(|error| {
                AuthError::Storage(format!("build google oauth http client: {error}"))
            })?;
        let authorize_endpoint = Url::parse(GOOGLE_AUTHORIZE_ENDPOINT).map_err(|error| {
            AuthError::Config(format!("parse google authorize endpoint: {error}"))
        })?;
        let token_endpoint = Url::parse(GOOGLE_TOKEN_ENDPOINT)
            .map_err(|error| AuthError::Config(format!("parse google token endpoint: {error}")))?;
        let jwks_endpoint = Url::parse(GOOGLE_JWKS_ENDPOINT)
            .map_err(|error| AuthError::Config(format!("parse google jwks endpoint: {error}")))?;

        Ok(Self {
            client_id,
            client_secret,
            redirect_uri,
            scopes: vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
            ],
            http,
            authorize_endpoint,
            token_endpoint,
            jwks_endpoint,
            jwks_cache: Arc::new(RwLock::new(None)),
        })
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_endpoints(mut self, authorize_endpoint: Url, token_endpoint: Url) -> Self {
        self.authorize_endpoint = authorize_endpoint;
        self.token_endpoint = token_endpoint;
        self
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_jwks_endpoint(mut self, jwks_endpoint: Url) -> Self {
        self.jwks_endpoint = jwks_endpoint;
        self
    }

    pub fn authorize_url(&self, request: &AuthorizeUrlRequest) -> Result<Url, AuthError> {
        let mut url = self.authorize_endpoint.clone();
        let scope = self.scopes.join(" ");
        url.query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", self.redirect_uri.as_str())
            .append_pair("response_type", "code")
            .append_pair("scope", &scope)
            .append_pair("state", &request.state)
            .append_pair("code_challenge", &request.code_challenge)
            .append_pair("code_challenge_method", &request.code_challenge_method);
        if request.offline_access {
            url.query_pairs_mut()
                .append_pair("access_type", "offline")
                .append_pair("include_granted_scopes", "true");
        }
        if request.force_consent {
            url.query_pairs_mut().append_pair("prompt", "consent");
        }
        debug!(
            provider = "google",
            oauth_state_id = %fingerprint(&request.state),
            scope = %scope,
            redirect_uri = %self.redirect_uri,
            "oauth upstream authorize URL constructed"
        );
        Ok(url)
    }

    pub fn reauth_url(&self, request: &GoogleReauthRequest) -> Result<Url, AuthError> {
        let mut url = self.authorize_endpoint.clone();
        url.query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", self.redirect_uri.as_str())
            .append_pair("response_type", "code")
            .append_pair("scope", &self.scopes.join(" "))
            .append_pair("state", &request.state)
            .append_pair("nonce", &request.nonce)
            .append_pair("code_challenge", &request.code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("prompt", "select_account")
            .append_pair("claims", r#"{"id_token":{"auth_time":{"essential":true}}}"#);
        Ok(url)
    }

    pub async fn exchange_reauth_code(
        &self,
        code: &str,
        code_verifier: &str,
        expected_nonce: &str,
        expected_subject: &str,
        challenge_issued_at: i64,
    ) -> Result<GoogleFreshAuth, AuthError> {
        let trace = GoogleRequestTrace::start("reauth_code_exchange", "POST", &self.token_endpoint);
        let payload: GoogleTokenResponse = read_json_response(
            trace,
            self.http.post(self.token_endpoint.clone()).form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("code_verifier", code_verifier),
            ]),
            GoogleRequestErrors {
                transport_context: "exchange google reauthentication code",
                transport_log: "oauth upstream reauthentication exchange failed",
                status_context: "google reauthentication token endpoint error",
                status_log: "oauth upstream reauthentication exchange returned error status",
                decode_context: "decode google reauthentication response",
                decode_log: "oauth upstream reauthentication response was unreadable",
                invalid_grant_requires_reauth: false,
            },
        )
        .await?;
        let token = payload.id_token.as_deref().ok_or_else(|| {
            AuthError::InvalidGrant("reauthentication response is missing id_token".to_string())
        })?;
        let claims = self.verify_id_token(token).await?;
        let authenticated_at = claims.auth_time.ok_or_else(|| {
            AuthError::InvalidGrant("reauthentication response is missing auth_time".to_string())
        })?;
        let now = crate::util::now_unix();
        if claims.sub != expected_subject
            || claims.nonce.as_deref() != Some(expected_nonce)
            || authenticated_at <= challenge_issued_at
            || authenticated_at > now
            || authenticated_at <= now - 300
        {
            return Err(AuthError::InvalidGrant(
                "reauthentication evidence does not match the active challenge".to_string(),
            ));
        }
        Ok(GoogleFreshAuth {
            subject: claims.sub,
            authenticated_at,
        })
    }

    pub async fn exchange_code(
        &self,
        code: &str,
        code_verifier: &str,
    ) -> Result<GoogleExchange, AuthError> {
        let trace = GoogleRequestTrace::start("code_exchange", "POST", &self.token_endpoint);
        info!(
            provider = "google",
            oauth_code_id = %fingerprint(code),
            redirect_uri = %self.redirect_uri,
            "oauth upstream code exchange started"
        );
        let payload: GoogleTokenResponse = read_json_response(
            trace,
            self.http.post(self.token_endpoint.clone()).form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("code_verifier", code_verifier),
            ]),
            GoogleRequestErrors {
                transport_context: "exchange google auth code",
                transport_log: "oauth upstream code exchange request failed",
                status_context: "google token endpoint error",
                status_log: "oauth upstream code exchange returned error status",
                decode_context: "decode google token response",
                decode_log: "oauth upstream code exchange returned an unreadable payload",
                invalid_grant_requires_reauth: false,
            },
        )
        .await?;
        let id_token = payload.id_token.as_deref().ok_or_else(|| {
            AuthError::Decode("google code exchange response is missing id_token".to_string())
        })?;
        let claims = self.verify_id_token(id_token).await?;
        info!(
            provider = "google",
            subject_id = %fingerprint(&claims.sub),
            has_refresh_token = payload.refresh_token.is_some(),
            expires_in_secs = payload.expires_in,
            "oauth upstream code exchange succeeded"
        );
        Ok(GoogleExchange {
            subject: claims.sub,
            email: claims.email,
            email_verified: claims.email_verified,
            hosted_domain: claims.hd,
            access_token: payload.access_token,
            refresh_token: payload.refresh_token,
            expires_in: payload.expires_in,
            granted_scopes: payload
                .scope
                .as_deref()
                .map(parse_google_scopes)
                .unwrap_or_else(|| self.scopes.clone()),
            id_token: payload.id_token,
        })
    }

    pub async fn refresh(
        &self,
        refresh_token: &str,
        expected_subject: &str,
        existing_email: Option<&str>,
    ) -> Result<GoogleExchange, AuthError> {
        let trace = GoogleRequestTrace::start("refresh", "POST", &self.token_endpoint);
        info!(
            provider = "google",
            refresh_token_id = %fingerprint(refresh_token),
            "oauth upstream refresh started"
        );
        let payload: GoogleTokenResponse = read_json_response(
            trace,
            self.http.post(self.token_endpoint.clone()).form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ]),
            GoogleRequestErrors {
                transport_context: "refresh google token",
                transport_log: "oauth upstream refresh request failed",
                status_context: "google refresh endpoint error",
                status_log: "oauth upstream refresh returned error status",
                decode_context: "decode google refresh response",
                decode_log: "oauth upstream refresh returned an unreadable payload",
                invalid_grant_requires_reauth: true,
            },
        )
        .await?;
        let (subject, email, email_verified, hosted_domain) =
            if let Some(id_token) = payload.id_token.as_deref() {
                let claims = self.verify_id_token(id_token).await?;
                (claims.sub, claims.email, claims.email_verified, claims.hd)
            } else {
                // Google refresh responses commonly omit `id_token`. The refresh token
                // came from the encrypted row already bound to this verified subject
                // and OAuth client, so preserve that identity instead of rejecting a
                // healthy access-token rollover.
                // No fresh `hd` assertion is available on this path, so domain-based
                // access is not re-established from a reused identity.
                (
                    expected_subject.to_string(),
                    existing_email.map(ToOwned::to_owned),
                    existing_email.map(|_| true),
                    None,
                )
            };
        info!(
            provider = "google",
            subject_id = %fingerprint(&subject),
            identity_reused = payload.id_token.is_none(),
            has_refresh_token = payload.refresh_token.is_some(),
            expires_in_secs = payload.expires_in,
            "oauth upstream refresh succeeded"
        );
        Ok(GoogleExchange {
            subject,
            email,
            email_verified,
            hosted_domain,
            access_token: payload.access_token,
            refresh_token: payload.refresh_token,
            expires_in: payload.expires_in,
            granted_scopes: payload
                .scope
                .as_deref()
                .map(parse_google_scopes)
                .unwrap_or_default(),
            id_token: payload.id_token,
        })
    }

    /// Verify a Google ID token and return its stable account identity.
    pub async fn verify_identity(&self, id_token: &str) -> Result<GoogleIdentity, AuthError> {
        let claims = self.verify_id_token(id_token).await?;
        Ok(GoogleIdentity {
            subject: claims.sub,
            email: claims.email,
            email_verified: claims.email_verified,
            hosted_domain: claims.hd,
        })
    }

    async fn verify_id_token(&self, id_token: &str) -> Result<GoogleIdTokenClaims, AuthError> {
        let header = decode_header(id_token)
            .map_err(|error| AuthError::Storage(format!("verify google id_token: {error}")))?;
        validate_id_token_header(&header)?;
        let kid = header
            .kid
            .ok_or_else(|| AuthError::Storage("google id_token is missing a key id".to_string()))?;
        let key = self.find_jwk_for_kid(&kid).await?;
        if let Some(alg) = key.alg.as_deref()
            && alg != "RS256"
        {
            return Err(AuthError::Storage(format!(
                "google JWKS key `{}` uses unsupported algorithm `{alg}`",
                key.kid
            )));
        }

        let decoding_key = DecodingKey::from_rsa_components(&key.n, &key.e).map_err(|error| {
            AuthError::Storage(format!("build google id_token decoding key: {error}"))
        })?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = true;
        validation.leeway = 0;
        validation.set_audience(&[self.client_id.as_str()]);

        let claims = decode::<GoogleIdTokenClaims>(id_token, &decoding_key, &validation)
            .map(|data| data.claims)
            .map_err(|error| AuthError::Storage(format!("invalid google id_token: {error}")))?;

        if claims.iss != GOOGLE_ISSUER && claims.iss != "accounts.google.com" {
            return Err(AuthError::Storage(format!(
                "invalid google id_token issuer `{}`",
                claims.iss
            )));
        }

        Ok(claims)
    }

    async fn find_jwk_for_kid(&self, kid: &str) -> Result<GoogleJwk, AuthError> {
        let jwks = self.fetch_jwks().await?;
        if let Some(key) = jwks.keys.into_iter().find(|key| key.kid == kid) {
            return Ok(key);
        }

        debug!(
            provider = "google",
            kid, "google jwks cache miss for token key id; refreshing"
        );
        self.refresh_jwks()
            .await?
            .keys
            .into_iter()
            .find(|key| key.kid == kid)
            .ok_or_else(|| {
                AuthError::Storage("google id_token key id was not found in JWKS".to_string())
            })
    }

    async fn fetch_jwks(&self) -> Result<GoogleJwks, AuthError> {
        if let Some(jwks) = self.cached_jwks().await {
            debug!(provider = "google", "google jwks cache hit");
            return Ok(jwks);
        }

        let jwks = {
            let mut cache = self.jwks_cache.write().await;
            if let Some(cached) = cache
                .as_ref()
                .filter(|cached| cached.expires_at > Instant::now())
            {
                debug!(
                    provider = "google",
                    "google jwks cache hit after refresh lock"
                );
                cached.jwks.clone()
            } else {
                Self::refresh_jwks_locked(&self.http, &self.jwks_endpoint, &mut cache).await?
            }
        };
        Ok(jwks)
    }

    async fn refresh_jwks(&self) -> Result<GoogleJwks, AuthError> {
        let jwks = {
            let mut cache = self.jwks_cache.write().await;
            Self::refresh_jwks_locked(&self.http, &self.jwks_endpoint, &mut cache).await?
        };
        Ok(jwks)
    }

    async fn refresh_jwks_locked(
        http: &reqwest::Client,
        jwks_endpoint: &Url,
        cache: &mut Option<CachedGoogleJwks>,
    ) -> Result<GoogleJwks, AuthError> {
        let trace = GoogleRequestTrace::start("fetch_jwks", "GET", jwks_endpoint);
        let response = http
            .get(jwks_endpoint.clone())
            .timeout(GOOGLE_JWKS_FETCH_TIMEOUT)
            .send()
            .await
            .map_err(|error| {
                trace.error(None, &error);
                warn!(provider = "google", error = %error, "google jwks request failed");
                AuthError::Storage(format!("fetch google jwks: {error}"))
            })?;
        let status = response.status();
        let ttl = google_jwks_ttl(response.headers());
        let response = response.error_for_status().map_err(|error| {
            trace.error(Some(status), &error);
            warn!(provider = "google", error = %error, "google jwks request returned error status");
            AuthError::Storage(format!("google jwks endpoint error: {error}"))
        })?;
        trace.finish(status);
        let jwks: GoogleJwks = decode_capped_json(response, "decode google jwks response")
            .await
            .map_err(|error| {
                warn!(provider = "google", error = %error, "google jwks payload unreadable");
                error
            })?;

        *cache = Some(CachedGoogleJwks {
            jwks: jwks.clone(),
            expires_at: Instant::now() + ttl,
        });

        Ok(jwks)
    }

    async fn cached_jwks(&self) -> Option<GoogleJwks> {
        let cache = self.jwks_cache.read().await;
        cache
            .as_ref()
            .filter(|cached| cached.expires_at > Instant::now())
            .map(|cached| cached.jwks.clone())
    }
}

fn google_jwks_ttl(headers: &header::HeaderMap) -> Duration {
    headers
        .get(header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_max_age)
        .map_or(GOOGLE_DEFAULT_JWKS_TTL, Duration::from_secs)
}

fn parse_max_age(cache_control: &str) -> Option<u64> {
    cache_control.split(',').find_map(|directive| {
        let directive = directive.trim();
        let value = directive.strip_prefix("max-age=")?;
        value.parse::<u64>().ok()
    })
}

fn validate_id_token_header(header: &Header) -> Result<(), AuthError> {
    if header.alg != Algorithm::RS256 {
        return Err(AuthError::Storage(format!(
            "verify google id_token: unsupported algorithm `{:?}`",
            header.alg
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        sync::OnceLock,
        time::{Duration, Instant},
    };

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::rand_core::{TryCryptoRng, TryRng, UnwrapErr};
    use rsa::traits::PublicKeyParts;
    use serde_json::json;
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{
        AuthorizeUrlRequest, CachedGoogleJwks, GoogleExchange, GoogleIdentity, GoogleJwk,
        GoogleJwks, GoogleProvider, GoogleReauthRequest, merge_google_scopes,
    };

    #[test]
    fn google_sensitive_debug_output_is_redacted() {
        let request = sample_request();
        let request_debug = format!("{request:?}");
        assert!(!request_debug.contains("state-123"));
        assert!(!request_debug.contains("\"challenge\""));

        let exchange = GoogleExchange {
            subject: "google-subject-secret".to_string(),
            email: Some("admin@example.com".to_string()),
            email_verified: Some(true),
            hosted_domain: None,
            access_token: "access-secret".to_string(),
            refresh_token: Some("refresh-secret".to_string()),
            expires_in: Some(3600),
            granted_scopes: vec!["openid".to_string()],
            id_token: Some("id-secret".to_string()),
        };
        let exchange_debug = format!("{exchange:?}");
        for secret in [
            "google-subject-secret",
            "admin@example.com",
            "access-secret",
            "refresh-secret",
            "id-secret",
        ] {
            assert!(!exchange_debug.contains(secret), "leaked {secret}");
        }

        let identity = GoogleIdentity {
            subject: "google-subject-secret".to_string(),
            email: Some("admin@example.com".to_string()),
            email_verified: Some(true),
            hosted_domain: None,
        };
        let identity_debug = format!("{identity:?}");
        assert!(!identity_debug.contains("google-subject-secret"));
        assert!(!identity_debug.contains("admin@example.com"));
    }

    #[test]
    fn incremental_google_scopes_preserve_existing_grants() {
        assert_eq!(
            merge_google_scopes(
                &["drive.readonly".to_string(), "openid".to_string()],
                &["calendar.readonly".to_string(), "openid".to_string()],
            ),
            vec!["calendar.readonly", "drive.readonly", "openid"]
        );
    }

    #[test]
    fn google_scope_aliases_canonicalize_for_subset_checks() {
        assert_eq!(
            merge_google_scopes(
                &[],
                &[
                    "https://www.googleapis.com/auth/userinfo.email".to_string(),
                    "https://www.googleapis.com/auth/userinfo.profile".to_string(),
                    "openid".to_string(),
                ],
            ),
            vec!["email", "openid", "profile"]
        );
    }

    #[test]
    fn google_authorize_url_includes_offline_access_prompt_and_pkce() {
        let provider = test_google_provider();
        let request = sample_request();
        let url = provider.authorize_url(&request).unwrap();
        assert!(url.as_str().contains("access_type=offline"));
        assert!(url.as_str().contains("prompt=consent"));
        assert!(url.as_str().contains("code_challenge="));
    }

    #[test]
    fn browser_login_authorization_omits_offline_access_and_consent() {
        let provider = test_google_provider();
        let mut request = sample_request();
        request.offline_access = false;
        request.force_consent = false;
        let url = provider.authorize_url(&request).unwrap();
        assert!(!url.as_str().contains("access_type="));
        assert!(!url.as_str().contains("include_granted_scopes="));
        assert!(!url.as_str().contains("prompt="));
    }

    #[test]
    fn google_authorize_url_omits_prompt_when_consent_not_forced() {
        let provider = test_google_provider();
        let mut request = sample_request();
        request.force_consent = false;
        let url = provider.authorize_url(&request).unwrap();
        assert!(url.as_str().contains("access_type=offline"));
        assert!(!url.as_str().contains("prompt="));
    }

    #[test]
    fn google_reauth_url_requests_signed_freshness_and_nonce_without_offline_access() {
        let provider = test_google_provider();
        let url = provider
            .reauth_url(&GoogleReauthRequest {
                state: "reauth-state".to_string(),
                nonce: "reauth-nonce".to_string(),
                code_challenge: "reauth-pkce".to_string(),
            })
            .unwrap();
        let pairs = url
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            pairs.get("nonce").map(|value| value.as_ref()),
            Some("reauth-nonce")
        );
        assert_eq!(
            pairs.get("prompt").map(|value| value.as_ref()),
            Some("select_account")
        );
        assert_eq!(
            pairs.get("claims").map(|value| value.as_ref()),
            Some(r#"{"id_token":{"auth_time":{"essential":true}}}"#)
        );
        assert!(!pairs.contains_key("access_type"));
        assert!(!pairs.contains_key("include_granted_scopes"));
    }

    #[tokio::test]
    async fn google_reauth_accepts_only_matching_recent_signed_nonce_and_subject() {
        let issued_at = unix_now() - 10;
        let token = signed_reauth_token("google-subject-123", "expected-nonce", issued_at + 1);
        let (_server, provider) = mocked_google_provider_with_id_token(token).await;
        let fresh = provider
            .exchange_reauth_code(
                "code",
                "verifier",
                "expected-nonce",
                "google-subject-123",
                issued_at,
            )
            .await
            .unwrap();
        assert_eq!(fresh.authenticated_at(), issued_at + 1);
        assert_eq!(fresh.subject(), "google-subject-123");
        assert!(!format!("{fresh:?}").contains("google-subject-123"));
    }

    #[tokio::test]
    async fn google_reauth_rejects_stale_nonce_and_account_switch() {
        let issued_at = unix_now() - 10;
        for token in [
            signed_reauth_token("google-subject-123", "expected-nonce", issued_at),
            signed_reauth_token("google-subject-123", "wrong-nonce", issued_at + 1),
            signed_reauth_token("different-subject", "expected-nonce", issued_at + 1),
        ] {
            let (_server, provider) = mocked_google_provider_with_id_token(token).await;
            assert!(
                provider
                    .exchange_reauth_code(
                        "code",
                        "verifier",
                        "expected-nonce",
                        "google-subject-123",
                        issued_at
                    )
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn google_reauth_rejects_an_id_token_without_freshness_claims() {
        let (_server, provider) = mocked_google_provider().await;
        let error = provider
            .exchange_reauth_code(
                "code",
                "verifier",
                "expected-nonce",
                "google-subject-123",
                unix_now() - 10,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("auth_time"));
    }

    #[tokio::test]
    async fn google_exchange_parses_subject_and_refresh_token() {
        let (_server, provider) = mocked_google_provider().await;
        let token = provider.exchange_code("code", "verifier").await.unwrap();
        assert_eq!(token.subject, "google-subject-123");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh-token"));
    }

    #[tokio::test]
    async fn google_refresh_maps_invalid_grant_to_oauth_needs_reauth() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": "invalid_grant",
                "error_description": "Token has been expired or revoked."
            })))
            .mount(&server)
            .await;
        let provider = test_google_provider().with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        );

        let error = provider
            .refresh(
                "revoked-refresh-token",
                "google-subject-123",
                Some("user@example.com"),
            )
            .await
            .unwrap_err();

        assert_eq!(error.kind(), "oauth_needs_reauth");
    }

    #[tokio::test]
    async fn google_token_success_body_is_streamed_with_a_hard_cap() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![
                b' ';
                super::GOOGLE_MAX_JSON_RESPONSE_BYTES
                    + 1
            ]))
            .mount(&server)
            .await;
        let provider = test_google_provider().with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        );

        let error = provider
            .refresh("refresh-token", "subject", None)
            .await
            .unwrap_err();

        assert_eq!(error.kind(), "response_too_large");
    }

    #[tokio::test]
    async fn google_token_error_body_is_streamed_with_a_hard_cap() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_bytes(vec![
                b'x';
                super::GOOGLE_MAX_JSON_RESPONSE_BYTES
                    + 1
            ]))
            .mount(&server)
            .await;
        let provider = test_google_provider().with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        );

        let error = provider
            .refresh("refresh-token", "subject", None)
            .await
            .unwrap_err();

        assert_eq!(error.kind(), "response_too_large");
    }

    #[tokio::test]
    async fn google_jwks_body_is_streamed_with_a_hard_cap() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![
                b' ';
                super::GOOGLE_MAX_JSON_RESPONSE_BYTES
                    + 1
            ]))
            .mount(&server)
            .await;
        let provider = test_google_provider()
            .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());

        let error = provider.refresh_jwks().await.unwrap_err();

        assert_eq!(error.kind(), "response_too_large");
    }

    #[tokio::test]
    async fn google_refresh_reuses_verified_identity_when_id_token_is_omitted() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "refreshed-google-access-token",
                "expires_in": 3600,
                "scope": "openid email profile",
            })))
            .mount(&server)
            .await;
        let provider = test_google_provider().with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        );

        let exchange = provider
            .refresh(
                "existing-refresh-token",
                "google-subject-123",
                Some("user@example.com"),
            )
            .await
            .unwrap();

        assert_eq!(exchange.subject, "google-subject-123");
        assert_eq!(exchange.email.as_deref(), Some("user@example.com"));
        assert_eq!(exchange.email_verified, Some(true));
        assert!(exchange.id_token.is_none());
    }

    #[tokio::test]
    async fn google_code_exchange_requires_id_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600,
            })))
            .mount(&server)
            .await;
        let provider = test_google_provider().with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        );

        let error = provider
            .exchange_code("code", "verifier")
            .await
            .unwrap_err();

        assert_eq!(error.kind(), "decode_error");
        assert!(error.to_string().contains("missing id_token"));
    }

    #[tokio::test]
    async fn google_exchange_rejects_unsigned_id_tokens() {
        let (_server, provider) = mocked_google_provider_with_id_token(test_id_token()).await;
        let error = provider
            .exchange_code("code", "verifier")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("verify google id_token"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn google_exchange_rejects_wrong_audience_in_id_token() {
        let (_server, provider) =
            mocked_google_provider_with_id_token(signed_test_id_token("other-client", false, true))
                .await;
        let error = provider
            .exchange_code("code", "verifier")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("invalid google id_token"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn google_exchange_rejects_expired_id_token() {
        let (_server, provider) =
            mocked_google_provider_with_id_token(signed_test_id_token("client-id", true, true))
                .await;
        let error = provider
            .exchange_code("code", "verifier")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("invalid google id_token"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn google_exchange_rejects_wrong_issuer_in_id_token() {
        let (_server, provider) =
            mocked_google_provider_with_id_token(signed_test_id_token("client-id", false, false))
                .await;
        let error = provider
            .exchange_code("code", "verifier")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("issuer"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn google_exchange_reuses_cached_jwks() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token("client-id", false, true),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Cache-Control", "public, max-age=3600")
                    .set_body_json(test_jwks()),
            )
            .mount(&server)
            .await;

        let provider = test_google_provider()
            .with_endpoints(
                server.uri().parse::<Url>().unwrap(),
                server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
            )
            .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());

        provider.exchange_code("code-1", "verifier").await.unwrap();
        provider.exchange_code("code-2", "verifier").await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let jwks_requests = requests
            .iter()
            .filter(|request| request.url.path() == "/certs")
            .count();
        assert_eq!(jwks_requests, 1);
    }

    #[tokio::test]
    async fn google_exchange_refreshes_jwks_when_cached_kid_is_missing() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token("client-id", false, true),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(&server)
            .await;

        let provider = test_google_provider()
            .with_endpoints(
                server.uri().parse::<Url>().unwrap(),
                server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
            )
            .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());
        *provider.jwks_cache.write().await = Some(CachedGoogleJwks {
            jwks: wrong_test_jwks(),
            expires_at: Instant::now() + Duration::from_hours(1),
        });

        let exchange = provider.exchange_code("code", "verifier").await.unwrap();
        assert_eq!(exchange.subject, "google-subject-123");

        let requests = server.received_requests().await.unwrap();
        let jwks_requests = requests
            .iter()
            .filter(|request| request.url.path() == "/certs")
            .count();
        assert_eq!(jwks_requests, 1);
    }

    #[test]
    fn parse_max_age_reads_cache_control_max_age() {
        assert_eq!(super::parse_max_age("public, max-age=3600"), Some(3600));
        assert_eq!(super::parse_max_age("no-cache"), None);
    }

    fn test_google_provider() -> GoogleProvider {
        GoogleProvider::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
        )
        .unwrap()
    }

    async fn mocked_google_provider() -> (MockServer, GoogleProvider) {
        mocked_google_provider_with_id_token(signed_test_id_token("client-id", false, true)).await
    }

    async fn mocked_google_provider_with_id_token(
        id_token: String,
    ) -> (MockServer, GoogleProvider) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600,
                "id_token": id_token,
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(&server)
            .await;

        let provider = test_google_provider()
            .with_endpoints(
                server.uri().parse::<Url>().unwrap(),
                server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
            )
            .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());
        (server, provider)
    }

    fn sample_request() -> AuthorizeUrlRequest {
        AuthorizeUrlRequest {
            state: "state-123".to_string(),
            scope: "lab".to_string(),
            code_challenge: "challenge".to_string(),
            code_challenge_method: "S256".to_string(),
            offline_access: true,
            force_consent: true,
        }
    }

    fn test_id_token() -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"google-subject-123"}"#);
        format!("{header}.{payload}.")
    }

    fn signed_test_id_token(client_id: &str, expired: bool, valid_issuer: bool) -> String {
        let claims = json!({
            "iss": if valid_issuer { "https://accounts.google.com" } else { "https://evil.example.com" },
            "aud": client_id,
            "sub": "google-subject-123",
            "email": "user@example.com",
            "iat": (unix_now() - 10) as usize,
            "exp": if expired { (unix_now() - 3600) as usize } else { (unix_now() + 3600) as usize },
        });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-kid".to_string());
        encode(&header, &claims, &test_encoding_key()).unwrap()
    }

    fn signed_reauth_token(subject: &str, nonce: &str, auth_time: i64) -> String {
        let claims = json!({
            "iss": "https://accounts.google.com",
            "aud": "client-id",
            "sub": subject,
            "email": "user@example.com",
            "iat": (unix_now() - 10) as usize,
            "exp": (unix_now() + 3600) as usize,
            "nonce": nonce,
            "auth_time": auth_time,
        });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-kid".to_string());
        encode(&header, &claims, &test_encoding_key()).unwrap()
    }

    fn test_jwks() -> serde_json::Value {
        let key = test_rsa_key();
        let public_key = key.to_public_key();
        json!({
            "keys": [{
                "kid": "test-kid",
                "alg": "RS256",
                "kty": "RSA",
                "use": "sig",
                "n": URL_SAFE_NO_PAD.encode(public_key.n_bytes()),
                "e": URL_SAFE_NO_PAD.encode(public_key.e_bytes()),
            }]
        })
    }

    fn wrong_test_jwks() -> GoogleJwks {
        GoogleJwks {
            keys: vec![GoogleJwk {
                kid: "stale-kid".to_string(),
                alg: Some("RS256".to_string()),
                n: "stale-modulus".to_string(),
                e: "AQAB".to_string(),
            }],
        }
    }

    fn test_rsa_key() -> &'static RsaPrivateKey {
        static TEST_RSA_KEY: OnceLock<RsaPrivateKey> = OnceLock::new();
        TEST_RSA_KEY.get_or_init(|| {
            let mut rng = UnwrapErr(TestRng);
            RsaPrivateKey::new(&mut rng, 2048).unwrap()
        })
    }

    fn test_encoding_key() -> EncodingKey {
        let pem = test_rsa_key().to_pkcs8_pem(Default::default()).unwrap();
        EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap()
    }

    fn unix_now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    struct TestRng;

    impl TryRng for TestRng {
        type Error = getrandom::Error;

        fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
            let mut bytes = [0u8; 4];
            getrandom::fill(&mut bytes)?;
            Ok(u32::from_le_bytes(bytes))
        }

        fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
            let mut bytes = [0u8; 8];
            getrandom::fill(&mut bytes)?;
            Ok(u64::from_le_bytes(bytes))
        }

        fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
            getrandom::fill(dst)
        }
    }

    impl TryCryptoRng for TestRng {}
}
