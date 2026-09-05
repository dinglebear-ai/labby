use std::time::Duration;

use reqwest::Url;
use serde::Deserialize;

use crate::config::AutheliaConfig;
use crate::error::AuthError;
use crate::google::AuthorizeUrlRequest;
use crate::oauth_provider::ProviderExchange;
use crate::oidc::{OidcVerifier, TokenAuthMethod};
use crate::provider_http::build_authelia_authorize_url;

const AUTHELIA_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// Authelia's OpenID Connect 1.0 Provider mounts its endpoints at these
/// fixed paths off the issuer. The JWKS document is served at `/jwks.json`,
/// matching the provider's `jwks_uri` discovery metadata.
#[cfg(test)]
const AUTHELIA_AUTHORIZE_PATH: &str = "api/oidc/authorization";
#[cfg(test)]
const AUTHELIA_TOKEN_PATH: &str = "api/oidc/token";
#[cfg(test)]
const AUTHELIA_JWKS_PATH: &str = "jwks.json";

#[derive(Debug, Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    #[serde(default)]
    response_types_supported: Vec<String>,
    #[serde(default)]
    grant_types_supported: Vec<String>,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    token_endpoint_auth_methods_supported: Vec<String>,
    #[serde(default)]
    id_token_signing_alg_values_supported: Vec<String>,
}

#[derive(Clone)]
pub struct AutheliaProvider {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: Url,
    pub scopes: Vec<String>,
    pub http: reqwest::Client,
    issuer: Url,
    authorize_endpoint: Url,
    token_endpoint: Url,
    verifier: OidcVerifier,
}

impl std::fmt::Debug for AutheliaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutheliaProvider")
            .field("client_id", &self.client_id)
            .field("issuer_origin", &self.issuer.origin().ascii_serialization())
            .field(
                "redirect_origin",
                &self.redirect_uri.origin().ascii_serialization(),
            )
            .field("scopes", &self.scopes)
            .finish_non_exhaustive()
    }
}

impl AutheliaProvider {
    /// Explicit operator probe using the same discovery and JWKS path as
    /// production callbacks. It is never invoked during Doctor's offline run.
    pub async fn live_probe(config: AutheliaConfig, redirect_uri: Url) -> Result<(), AuthError> {
        let provider = Self::discover(config, redirect_uri).await?;
        provider.verifier.probe_jwks().await
    }
    pub async fn discover(config: AutheliaConfig, redirect_uri: Url) -> Result<Self, AuthError> {
        crate::provider_http::install_rustls_default_once();
        let issuer = normalize_issuer(config.issuer_url)?;
        let discovery_url = discovery_url(&issuer)?;
        let allow_private = config
            .trusted_private_origin
            .as_ref()
            .is_some_and(|trusted| trusted.as_url().origin() == issuer.origin());
        let http = crate::provider_http::exact_origin_client(
            &issuer,
            allow_private,
            AUTHELIA_HTTP_TIMEOUT,
            config.ca_certificate_path.as_deref(),
        )
        .await?;
        let response = http
            .get(discovery_url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|error| {
                AuthError::Network(format!("fetch Authelia discovery: {}", error.without_url()))
            })?;
        let response = response.error_for_status().map_err(|error| {
            AuthError::Network(format!("fetch Authelia discovery: {}", error.without_url()))
        })?;
        let document = crate::provider_http::bounded_json::<DiscoveryDocument>(
            response,
            "Authelia discovery document",
        )
        .await?;
        validate_discovery(&issuer, &document)?;
        let authorize_endpoint = parse_same_origin_endpoint(
            &issuer,
            &document.authorization_endpoint,
            "authorization_endpoint",
        )?;
        let token_endpoint =
            parse_same_origin_endpoint(&issuer, &document.token_endpoint, "token_endpoint")?;
        let jwks_endpoint = parse_same_origin_endpoint(&issuer, &document.jwks_uri, "jwks_uri")?;
        let verifier = OidcVerifier::new(
            "authelia",
            issuer.as_str().trim_end_matches('/').to_string(),
            jwks_endpoint,
            http.clone(),
        )
        .with_token_auth_method(TokenAuthMethod::ClientSecretBasic);
        Ok(Self {
            client_id: config.client_id,
            client_secret: config.client_secret,
            redirect_uri,
            scopes: vec!["openid".into(), "email".into(), "profile".into()],
            http,
            issuer,
            authorize_endpoint,
            token_endpoint,
            verifier,
        })
    }

    #[cfg(test)]
    pub fn new(
        issuer: Url,
        client_id: String,
        client_secret: String,
        redirect_uri: Url,
    ) -> Result<Self, AuthError> {
        crate::provider_http::install_rustls_default_once();
        let http = reqwest::Client::builder()
            .timeout(AUTHELIA_HTTP_TIMEOUT)
            .build()
            .map_err(|error| {
                AuthError::Storage(format!("build authelia oauth http client: {error}"))
            })?;
        // RFC 3986 relative-URL resolution: `Url::join` replaces the LAST
        // path segment of a URL that doesn't end in `/`, not just appends.
        // For a bare-origin issuer (`https://auth.example.com`) that's a
        // no-op, but for a path-prefixed issuer without a trailing slash
        // (`https://example.com/authelia`) it silently drops `/authelia`,
        // producing `https://example.com/api/oidc/token` instead of
        // `https://example.com/authelia/api/oidc/token` — wrong URL, no
        // error. Guarantee a trailing slash before joining so `.join(...)`
        // always appends under the full issuer path.
        let issuer_base = if issuer.as_str().ends_with('/') {
            issuer.clone()
        } else {
            Url::parse(&format!("{}/", issuer.as_str())).map_err(|error| {
                AuthError::Config(format!("normalize authelia issuer url: {error}"))
            })?
        };
        let authorize_endpoint = issuer_base.join(AUTHELIA_AUTHORIZE_PATH).map_err(|error| {
            AuthError::Config(format!("build authelia authorize endpoint: {error}"))
        })?;
        let token_endpoint = issuer_base.join(AUTHELIA_TOKEN_PATH).map_err(|error| {
            AuthError::Config(format!("build authelia token endpoint: {error}"))
        })?;
        let jwks_endpoint = issuer_base
            .join(AUTHELIA_JWKS_PATH)
            .map_err(|error| AuthError::Config(format!("build authelia jwks endpoint: {error}")))?;
        let verifier = OidcVerifier::new(
            "authelia",
            issuer.as_str().trim_end_matches('/').to_string(),
            jwks_endpoint,
            http.clone(),
        )
        .with_token_auth_method(TokenAuthMethod::ClientSecretBasic);

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
            issuer,
            authorize_endpoint,
            token_endpoint,
            verifier,
        })
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_endpoints(
        mut self,
        issuer: Url,
        authorize_endpoint: Url,
        token_endpoint: Url,
        jwks_endpoint: Url,
    ) -> Self {
        self.authorize_endpoint = authorize_endpoint;
        self.token_endpoint = token_endpoint;
        self.verifier = OidcVerifier::new(
            "authelia",
            issuer.as_str().trim_end_matches('/').to_string(),
            jwks_endpoint,
            self.http.clone(),
        )
        .with_token_auth_method(TokenAuthMethod::ClientSecretBasic);
        self.issuer = issuer;
        self
    }

    /// Test-only accessor proving `AutheliaProvider::new`'s endpoint
    /// construction resolves correctly against path-prefixed issuers — see
    /// `authelia_token_endpoint_preserves_issuer_path_prefix_without_trailing_slash`.
    #[cfg(test)]
    pub(crate) fn token_endpoint(&self) -> &Url {
        &self.token_endpoint
    }
}

fn normalize_issuer(mut issuer: Url) -> Result<Url, AuthError> {
    if issuer.scheme() != "https"
        || issuer.host().is_none()
        || !issuer.username().is_empty()
        || issuer.password().is_some()
        || issuer.query().is_some()
        || issuer.fragment().is_some()
    {
        return Err(AuthError::Config(
            "Authelia issuer must be an HTTPS URL without credentials, query, or fragment".into(),
        ));
    }
    let normalized_path = issuer.path().trim_end_matches('/').to_string();
    issuer.set_path(if normalized_path.is_empty() {
        "/"
    } else {
        &normalized_path
    });
    Ok(issuer)
}

fn discovery_url(issuer: &Url) -> Result<Url, AuthError> {
    let issuer_path = issuer.path().trim_matches('/');
    let path = if issuer_path.is_empty() {
        "/.well-known/openid-configuration".to_string()
    } else {
        format!("/.well-known/openid-configuration/{issuer_path}")
    };
    let mut url = issuer.clone();
    url.set_path(&path);
    Ok(url)
}

fn parse_same_origin_endpoint(issuer: &Url, raw: &str, field: &str) -> Result<Url, AuthError> {
    let endpoint = Url::parse(raw)
        .map_err(|_| AuthError::Validation(format!("Authelia {field} is not an absolute URL")))?;
    if endpoint.origin() != issuer.origin()
        || endpoint.scheme() != "https"
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(AuthError::Validation(format!(
            "Authelia {field} must use the exact issuer origin"
        )));
    }
    Ok(endpoint)
}

fn validate_discovery(issuer: &Url, document: &DiscoveryDocument) -> Result<(), AuthError> {
    let discovered = normalize_issuer(
        Url::parse(&document.issuer)
            .map_err(|_| AuthError::Validation("Authelia discovery issuer is invalid".into()))?,
    )?;
    if discovered != *issuer {
        return Err(AuthError::Validation(
            "Authelia discovery issuer does not exactly match configuration".into(),
        ));
    }
    let required = [
        (
            document
                .response_types_supported
                .iter()
                .any(|v| v == "code"),
            "response_type code",
        ),
        (
            document.grant_types_supported.is_empty()
                || document
                    .grant_types_supported
                    .iter()
                    .any(|v| v == "authorization_code"),
            "authorization_code grant",
        ),
        (
            document
                .code_challenge_methods_supported
                .iter()
                .any(|v| v == "S256"),
            "PKCE S256",
        ),
        (
            document
                .token_endpoint_auth_methods_supported
                .iter()
                .any(|v| v == "client_secret_basic"),
            "client_secret_basic",
        ),
        (
            document
                .id_token_signing_alg_values_supported
                .iter()
                .any(|v| v == "RS256"),
            "RS256 signing",
        ),
    ];
    if let Some((_, capability)) = required.into_iter().find(|(present, _)| !present) {
        return Err(AuthError::Validation(format!(
            "Authelia discovery does not advertise required {capability}"
        )));
    }
    Ok(())
}

impl AutheliaProvider {
    pub fn issuer(&self) -> &str {
        self.issuer.as_str().trim_end_matches('/')
    }

    pub fn authorize_url(&self, request: &AuthorizeUrlRequest) -> Result<Url, AuthError> {
        Ok(build_authelia_authorize_url(
            &self.authorize_endpoint,
            &self.client_id,
            &self.redirect_uri,
            &self.scopes,
            request,
        ))
    }

    pub async fn exchange_code(
        &self,
        code: &str,
        code_verifier: &str,
        expected_nonce: &str,
    ) -> Result<ProviderExchange, AuthError> {
        self.verifier
            .exchange_code(
                &self.token_endpoint,
                &self.client_id,
                &self.client_secret,
                &self.redirect_uri,
                code,
                code_verifier,
                Some(expected_nonce),
            )
            .await
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use jsonwebtoken::{Algorithm, Header, encode};
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::rand_core::{TryCryptoRng, TryRng, UnwrapErr};
    use rsa::traits::PublicKeyParts;
    use serde_json::json;
    use std::sync::OnceLock;
    use wiremock::matchers::{basic_auth, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{
        AutheliaProvider, AuthorizeUrlRequest, DiscoveryDocument, discovery_url, validate_discovery,
    };

    fn valid_discovery(issuer: &str) -> DiscoveryDocument {
        DiscoveryDocument {
            issuer: issuer.to_string(),
            authorization_endpoint: format!("{issuer}/api/oidc/authorization"),
            token_endpoint: format!("{issuer}/api/oidc/token"),
            jwks_uri: format!("{issuer}/jwks.json"),
            response_types_supported: vec!["code".into()],
            grant_types_supported: vec!["authorization_code".into()],
            code_challenge_methods_supported: vec!["S256".into()],
            token_endpoint_auth_methods_supported: vec!["client_secret_basic".into()],
            id_token_signing_alg_values_supported: vec!["RS256".into()],
        }
    }

    #[test]
    fn path_prefixed_discovery_uses_oidc_well_known_layout() {
        let issuer = Url::parse("https://auth.example.test/authelia").unwrap();
        assert_eq!(
            discovery_url(&issuer).unwrap().as_str(),
            "https://auth.example.test/.well-known/openid-configuration/authelia"
        );
    }

    #[test]
    fn discovery_requires_exact_issuer_and_security_capabilities() {
        let issuer = Url::parse("https://auth.example.test").unwrap();
        assert!(validate_discovery(&issuer, &valid_discovery(issuer.as_str())).is_ok());
        let mut wrong = valid_discovery("https://other.example.test");
        assert!(validate_discovery(&issuer, &wrong).is_err());
        wrong = valid_discovery(issuer.as_str());
        wrong.code_challenge_methods_supported.clear();
        assert!(validate_discovery(&issuer, &wrong).is_err());
        wrong = valid_discovery(issuer.as_str());
        wrong.token_endpoint_auth_methods_supported = vec!["client_secret_post".into()];
        assert!(validate_discovery(&issuer, &wrong).is_err());
    }

    #[test]
    fn authelia_authorize_url_uses_local_revalidation_scopes_only() {
        let provider = test_authelia_provider();
        let request = AuthorizeUrlRequest {
            state: "state-123".to_string(),
            code_challenge: "challenge".to_string(),
            code_challenge_method: "S256".to_string(),
            scope: String::new(),
            offline_access: false,
            force_consent: true,
        };
        let url = provider.authorize_url(&request).unwrap();
        assert!(url.as_str().contains("scope=openid+email+profile"));
        assert!(!url.as_str().contains("access_type="));
        assert!(url.as_str().contains("prompt=consent"));
    }

    /// Regression test: a path-prefixed issuer WITHOUT a trailing slash must
    /// not lose its path prefix during endpoint construction. RFC 3986
    /// relative-URL resolution replaces the last path segment of a URL that
    /// doesn't end in `/`, so `Url::join` against a bare
    /// `https://example.com/authelia` issuer would silently produce
    /// `https://example.com/api/oidc/token` instead of
    /// `https://example.com/authelia/api/oidc/token`.
    #[test]
    fn authelia_token_endpoint_preserves_issuer_path_prefix_without_trailing_slash() {
        let provider = AutheliaProvider::new(
            Url::parse("https://example.com/authelia").unwrap(),
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/authelia/callback").unwrap(),
        )
        .unwrap();
        assert_eq!(
            provider.token_endpoint().as_str(),
            "https://example.com/authelia/api/oidc/token"
        );
    }

    /// Authelia advertises its signing keys at `<issuer>/jwks.json`. Keep the
    /// provider's default endpoint aligned with the discovery document so a
    /// normal, non-test provider can verify the ID token returned by the
    /// token endpoint.
    #[tokio::test]
    async fn authelia_default_jwks_endpoint_matches_discovery_document() {
        let server = MockServer::start().await;
        let issuer = Url::parse(&server.uri()).unwrap();
        Mock::given(method("POST"))
            .and(path("/api/oidc/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "authelia-access-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token(&issuer, "client-id"),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(&server)
            .await;

        let provider = AutheliaProvider::new(
            issuer,
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://soma.example.com/auth/authelia/callback").unwrap(),
        )
        .unwrap();

        let exchange = provider
            .exchange_code("code", "verifier", "nonce")
            .await
            .unwrap();
        assert_eq!(exchange.subject, "authelia-subject-123");
    }

    #[tokio::test]
    async fn authelia_exchange_parses_subject_from_id_token() {
        let server = MockServer::start().await;
        let issuer = Url::parse(&server.uri()).unwrap();
        Mock::given(method("POST"))
            .and(path("/api/oidc/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "authelia-access-token",
                "refresh_token": "authelia-refresh-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token(&issuer, "client-id"),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/oidc/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(&server)
            .await;

        let provider = test_authelia_provider().with_endpoints(
            issuer.clone(),
            issuer.join("api/oidc/authorization").unwrap(),
            issuer.join("api/oidc/token").unwrap(),
            issuer.join("api/oidc/jwks").unwrap(),
        );

        let exchange = provider
            .exchange_code("code", "verifier", "nonce")
            .await
            .unwrap();
        assert_eq!(exchange.subject, "authelia-subject-123");
        assert_eq!(
            exchange.refresh_token.as_deref(),
            Some("authelia-refresh-token")
        );
    }

    /// Authelia's OIDC provider defaults confidential clients to
    /// `client_secret_basic` unless the operator explicitly opts a client
    /// into `client_secret_post`. This mock only matches a token request
    /// that authenticates via the `Authorization: Basic` header (not a
    /// `client_secret` form field) — if `AutheliaProvider` ever regresses to
    /// posting the secret in the body instead, the request matches no mock,
    /// wiremock returns its default 404, and the exchange fails.
    #[tokio::test]
    async fn authelia_exchange_authenticates_via_http_basic_not_a_body_secret() {
        let server = MockServer::start().await;
        let issuer = Url::parse(&server.uri()).unwrap();
        Mock::given(method("POST"))
            .and(path("/api/oidc/token"))
            .and(basic_auth("client-id", "client-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "authelia-access-token",
                "refresh_token": "authelia-refresh-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token(&issuer, "client-id"),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/oidc/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(&server)
            .await;

        let provider = test_authelia_provider().with_endpoints(
            issuer.clone(),
            issuer.join("api/oidc/authorization").unwrap(),
            issuer.join("api/oidc/token").unwrap(),
            issuer.join("api/oidc/jwks").unwrap(),
        );

        let exchange = provider
            .exchange_code("code", "verifier", "nonce")
            .await
            .expect("token request must authenticate via HTTP Basic to match Authelia's default");
        assert_eq!(exchange.subject, "authelia-subject-123");
    }

    /// Negative-path coverage that only Authelia can exercise: unlike
    /// Google's hardcoded issuer constant, Authelia's issuer is
    /// operator-configured and `.trim_end_matches('/')`-normalized, so a
    /// bug in that normalization or in `OidcVerifier::verify`'s issuer
    /// comparison would not be caught by any of Google's negative tests.
    /// Mirrors `google_exchange_rejects_wrong_issuer_in_id_token`.
    #[tokio::test]
    async fn authelia_exchange_rejects_wrong_issuer_in_id_token() {
        let server = MockServer::start().await;
        let issuer = Url::parse(&server.uri()).unwrap();
        Mock::given(method("POST"))
            .and(path("/api/oidc/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "authelia-access-token",
                "refresh_token": "authelia-refresh-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token_with_raw_issuer(
                    "https://evil.example.com",
                    "client-id",
                ),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/oidc/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(&server)
            .await;

        let provider = test_authelia_provider().with_endpoints(
            issuer.clone(),
            issuer.join("api/oidc/authorization").unwrap(),
            issuer.join("api/oidc/token").unwrap(),
            issuer.join("api/oidc/jwks").unwrap(),
        );

        let error = provider
            .exchange_code("code", "verifier", "nonce")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("issuer"),
            "unexpected error: {error}"
        );
    }

    use reqwest::Url;

    fn test_authelia_provider() -> AutheliaProvider {
        AutheliaProvider::new(
            Url::parse("https://auth.example.com").unwrap(),
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/authelia/callback").unwrap(),
        )
        .unwrap()
    }

    pub(crate) async fn mock_provider_for_nonce(nonce: &str) -> (AutheliaProvider, MockServer) {
        let server = MockServer::start().await;
        let issuer = Url::parse(&server.uri()).unwrap();
        let claims = json!({
            "iss": issuer.as_str().trim_end_matches('/'), "aud": "client-id",
            "sub": "authelia-subject-123", "email": "user@example.com",
            "email_verified": true, "nonce": nonce,
            "iat": (unix_now() - 10) as usize, "exp": (unix_now() + 3600) as usize,
        });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-kid".into());
        let id_token = encode(&header, &claims, &test_encoding_key()).unwrap();
        Mock::given(method("POST"))
            .and(path("/api/oidc/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "authelia-access-token", "expires_in": 3600,
                "id_token": id_token,
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/oidc/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(&server)
            .await;
        let provider = test_authelia_provider().with_endpoints(
            issuer.clone(),
            issuer.join("api/oidc/authorization").unwrap(),
            issuer.join("api/oidc/token").unwrap(),
            issuer.join("api/oidc/jwks").unwrap(),
        );
        (provider, server)
    }

    #[tokio::test]
    async fn one_hundred_cold_jwks_probes_collapse_to_one_fetch() {
        let nonce = crate::util::random_token(18).unwrap();
        let (provider, server) = mock_provider_for_nonce(&nonce).await;
        // One provider instance is one completed discovery generation. These
        // cold verifications share both that generation and its empty JWKS cache.
        let started = std::time::Instant::now();
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..100 {
            let provider = provider.clone();
            tasks.spawn(async move { provider.verifier.probe_jwks().await });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap().unwrap();
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.url.path() == "/api/oidc/jwks")
                .count(),
            1
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    fn signed_test_id_token(issuer: &Url, client_id: &str) -> String {
        signed_test_id_token_with_raw_issuer(issuer.as_str().trim_end_matches('/'), client_id)
    }

    fn signed_test_id_token_with_raw_issuer(issuer: &str, client_id: &str) -> String {
        let claims = json!({
            "iss": issuer,
            "aud": client_id,
            "sub": "authelia-subject-123",
            "email": "user@example.com",
            "email_verified": true,
            "nonce": "nonce",
            "iat": (unix_now() - 10) as usize,
            "exp": (unix_now() + 3600) as usize,
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

    fn test_rsa_key() -> &'static RsaPrivateKey {
        static TEST_RSA_KEY: OnceLock<RsaPrivateKey> = OnceLock::new();
        TEST_RSA_KEY.get_or_init(|| {
            let mut rng = UnwrapErr(TestRng);
            RsaPrivateKey::new(&mut rng, 2048).unwrap()
        })
    }

    fn test_encoding_key() -> jsonwebtoken::EncodingKey {
        let pem = test_rsa_key().to_pkcs8_pem(Default::default()).unwrap();
        jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap()
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
