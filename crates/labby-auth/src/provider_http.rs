use std::borrow::Cow;
use std::time::Duration;
use std::time::Instant;

use reqwest::Url;
use reqwest::header;
use serde::de::DeserializeOwned;
use tracing::{info, warn};

use crate::error::AuthError;
use crate::google::AuthorizeUrlRequest;

/// Installs the process-wide rustls crypto provider, if one isn't already
/// installed.
///
/// rmcp's HTTP transport (and, transitively, reqwest) requires a rustls
/// crypto provider to be installed before the first TLS-capable client is
/// built. The real binary installs one at startup; test binaries never go
/// through that path, so every `OAuthProvider::new` (Google, Authelia,
/// GitHub) also calls this before building its `reqwest::Client`. The
/// `Result` is intentionally discarded — `Err` only means a provider was
/// already installed elsewhere (e.g. by an earlier-constructed provider in
/// the same process), which is safe to ignore.
pub(crate) fn install_rustls_default_once() {
    drop(rustls::crypto::ring::default_provider().install_default());
}

pub(crate) async fn exact_origin_client(
    issuer: &Url,
    allow_private: bool,
    timeout: Duration,
    ca_certificate_path: Option<&std::path::Path>,
) -> Result<reqwest::Client, AuthError> {
    let host = issuer
        .host_str()
        .ok_or_else(|| AuthError::Validation("OIDC issuer has no host".into()))?;
    let port = issuer.port_or_known_default().unwrap_or(443);
    let addresses = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .map_err(|_| AuthError::Network("resolve OIDC issuer host: timed out".into()))?
    .map_err(|error| AuthError::Network(format!("resolve OIDC issuer host: {error}")))?
    .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(AuthError::Network(
            "OIDC issuer resolved to no addresses".into(),
        ));
    }
    if !allow_private {
        for address in &addresses {
            labby_primitives::ssrf::check_ip_not_private(address.ip(), host)
                .map_err(|error| AuthError::Validation(error.to_string()))?;
        }
    }
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve_to_addrs(host, &addresses);
    if let Some(path) = ca_certificate_path {
        let pem = std::fs::read(path)
            .map_err(|_| AuthError::Config("read Authelia CA certificate: unavailable".into()))?;
        let certificate = reqwest::Certificate::from_pem(&pem)
            .map_err(|_| AuthError::Config("parse Authelia CA certificate: invalid PEM".into()))?;
        builder = builder.add_root_certificate(certificate);
    }
    builder
        .build()
        .map_err(|error| AuthError::Config(format!("build OIDC HTTP client: {error}")))
}

pub(crate) async fn bounded_json<T: DeserializeOwned>(
    mut response: reqwest::Response,
    document: &str,
) -> Result<T, AuthError> {
    const MAX_BYTES: usize = 1024 * 1024;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BYTES as u64)
    {
        return Err(AuthError::Validation(format!("{document} exceeds 1 MiB")));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| AuthError::Network(format!("read {document}: {}", error.without_url())))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err(AuthError::Validation(format!("{document} exceeds 1 MiB")));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|error| AuthError::Decode(format!("decode {document}: {error}")))
}

/// Build the fixed Authelia OIDC authorization request.
pub(crate) fn build_authelia_authorize_url(
    authorize_endpoint: &Url,
    client_id: &str,
    redirect_uri: &Url,
    scopes: &[String],
    request: &AuthorizeUrlRequest,
) -> Url {
    let mut url = authorize_endpoint.clone();
    let scope = scopes.join(" ");
    let mut pairs = url.query_pairs_mut();
    pairs
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri.as_str())
        .append_pair("response_type", "code")
        .append_pair("scope", &scope);
    pairs
        .append_pair("state", &request.state)
        .append_pair("code_challenge", &request.code_challenge)
        .append_pair("code_challenge_method", &request.code_challenge_method);
    pairs.append_pair("nonce", &crate::util::fingerprint(&request.state));
    if request.force_consent {
        pairs.append_pair("prompt", "consent");
    }
    drop(pairs);
    url
}

pub(crate) struct RequestTrace<'a> {
    provider_id: &'static str,
    operation: &'static str,
    method: &'static str,
    endpoint: &'a Url,
    started: Instant,
}

impl<'a> RequestTrace<'a> {
    pub(crate) fn start(
        provider_id: &'static str,
        operation: &'static str,
        method: &'static str,
        endpoint: &'a Url,
    ) -> Self {
        info!(
            provider = provider_id,
            operation,
            method,
            host = endpoint.host_str().unwrap_or_default(),
            path = endpoint.path(),
            "request.start"
        );
        Self {
            provider_id,
            operation,
            method,
            endpoint,
            started: Instant::now(),
        }
    }

    pub(crate) fn finish(&self, status: reqwest::StatusCode) {
        info!(
            provider = self.provider_id,
            operation = self.operation,
            method = self.method,
            host = self.endpoint.host_str().unwrap_or_default(),
            path = self.endpoint.path(),
            status = status.as_u16(),
            elapsed_ms = self.started.elapsed().as_millis(),
            "request.finish"
        );
    }

    pub(crate) fn error(&self, status: Option<reqwest::StatusCode>, error: &reqwest::Error) {
        if let Some(status) = status {
            warn!(
                provider = self.provider_id,
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
                provider = self.provider_id,
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

pub(crate) struct RequestErrors {
    provider_id: &'static str,
    transport_context: Cow<'static, str>,
    status_context: Cow<'static, str>,
    decode_context: Cow<'static, str>,
}

impl RequestErrors {
    pub(crate) fn new<T, S, D>(
        provider_id: &'static str,
        transport_context: T,
        status_context: S,
        decode_context: D,
    ) -> Self
    where
        T: Into<Cow<'static, str>>,
        S: Into<Cow<'static, str>>,
        D: Into<Cow<'static, str>>,
    {
        Self {
            provider_id,
            transport_context: transport_context.into(),
            status_context: status_context.into(),
            decode_context: decode_context.into(),
        }
    }
}

pub(crate) async fn read_json_response<T: DeserializeOwned>(
    trace: RequestTrace<'_>,
    request: reqwest::RequestBuilder,
    errors: RequestErrors,
) -> Result<T, AuthError> {
    let response = request.send().await.map_err(|error| {
        let error = error.without_url();
        let auth_error = AuthError::Network(format!("{}: {error}", errors.transport_context));
        trace.error(None, &error);
        warn!(
            provider = errors.provider_id,
            error = %error,
            kind = auth_error.kind(),
            "{}",
            errors.transport_context
        );
        auth_error
    })?;
    let status = response.status();
    let retry_after_ms = response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| seconds.min(300).saturating_mul(1_000));

    if let Err(status_error) = response.error_for_status_ref() {
        let status_error = status_error.without_url();
        let auth_error = if let Some(retry_after_ms) = retry_after_ms {
            // GitHub's secondary rate limit (abuse detection) responds with
            // 403, not 429, but does carry `Retry-After` — trust the header's
            // presence over the exact status code so we don't miss it.
            AuthError::RateLimited {
                message: format!("{}: {status}", errors.status_context),
                retry_after_ms,
            }
        } else if status.is_server_error() {
            AuthError::Server(format!("{}: {status_error}", errors.status_context))
        } else {
            AuthError::AuthFailed(format!("{}: {status_error}", errors.status_context))
        };
        trace.error(Some(status), &status_error);
        warn!(
            provider = errors.provider_id,
            error = %status_error,
            kind = auth_error.kind(),
            "{}",
            errors.status_context
        );
        return Err(auth_error);
    }

    trace.finish(status);
    bounded_json::<T>(response, "OAuth provider response")
        .await
        .map_err(|error| {
            let auth_error = AuthError::Decode(format!("{}: {error}", errors.decode_context));
            warn!(
                provider = errors.provider_id,
                error = %error,
                kind = auth_error.kind(),
                "{}",
                errors.decode_context
            );
            auth_error
        })
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{
        RequestErrors, RequestTrace, build_authelia_authorize_url, install_rustls_default_once,
        read_json_response,
    };
    use crate::error::AuthError;
    use crate::google::AuthorizeUrlRequest;

    #[derive(Debug, Deserialize)]
    struct TestPayload {
        #[allow(dead_code)]
        value: String,
    }

    fn test_errors() -> RequestErrors {
        RequestErrors::new(
            "test-provider",
            "transport failed",
            "status error",
            "decode failed",
        )
    }

    fn test_client() -> reqwest::Client {
        install_rustls_default_once();
        reqwest::Client::new()
    }

    #[test]
    fn authelia_authorize_url_builder_includes_nonce_and_consent() {
        let endpoint = reqwest::Url::parse("https://example.test/authorize").unwrap();
        let redirect = reqwest::Url::parse("https://client.test/auth/callback").unwrap();
        let request = AuthorizeUrlRequest {
            state: "state".to_string(),
            code_challenge: "challenge".to_string(),
            code_challenge_method: "S256".to_string(),
            scope: String::new(),
            offline_access: false,
            force_consent: true,
        };

        let url = build_authelia_authorize_url(
            &endpoint,
            "client-id",
            &redirect,
            &["openid".to_string(), "email".to_string()],
            &request,
        );
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();

        assert_eq!(
            pairs.get("client_id").map(String::as_str),
            Some("client-id")
        );
        assert_eq!(
            pairs.get("redirect_uri").map(String::as_str),
            Some("https://client.test/auth/callback")
        );
        assert_eq!(pairs.get("scope").map(String::as_str), Some("openid email"));
        assert!(pairs.contains_key("nonce"));
        assert_eq!(pairs.get("state").map(String::as_str), Some("state"));
        assert_eq!(pairs.get("prompt").map(String::as_str), Some("consent"));
    }

    /// GitHub's secondary rate limit responds with 403 (not 429) but carries
    /// `Retry-After` — this must classify as `AuthError::RateLimited` with
    /// the header value converted to milliseconds, not `AuthFailed`.
    #[tokio::test]
    async fn a_403_with_retry_after_classifies_as_rate_limited() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rate-limited"))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("Retry-After", "5")
                    .set_body_json(serde_json::json!({"message": "rate limited"})),
            )
            .mount(&server)
            .await;
        let url = server
            .uri()
            .parse::<reqwest::Url>()
            .unwrap()
            .join("/rate-limited")
            .unwrap();
        let client = test_client();
        let trace = RequestTrace::start("test-provider", "op", "GET", &url);

        let error =
            read_json_response::<TestPayload>(trace, client.get(url.clone()), test_errors())
                .await
                .unwrap_err();
        assert!(
            matches!(error, AuthError::RateLimited { .. }),
            "expected RateLimited, got {error:?}"
        );
        if let AuthError::RateLimited { retry_after_ms, .. } = error {
            assert_eq!(retry_after_ms, 5_000);
        }
    }

    /// A generic 4xx with no `Retry-After` header must classify as
    /// `AuthError::AuthFailed`, not `RateLimited` or `Server`.
    #[tokio::test]
    async fn a_generic_4xx_without_retry_after_classifies_as_auth_failed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/bad-request"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(serde_json::json!({"message": "bad request"})),
            )
            .mount(&server)
            .await;
        let url = server
            .uri()
            .parse::<reqwest::Url>()
            .unwrap()
            .join("/bad-request")
            .unwrap();
        let client = test_client();
        let trace = RequestTrace::start("test-provider", "op", "GET", &url);

        let error =
            read_json_response::<TestPayload>(trace, client.get(url.clone()), test_errors())
                .await
                .unwrap_err();
        assert!(
            matches!(error, AuthError::AuthFailed(_)),
            "expected AuthFailed, got {error:?}"
        );
    }

    /// A transport-level failure (connection refused) must classify as
    /// `AuthError::Network`, not surface as a raw `reqwest::Error`.
    #[tokio::test]
    async fn a_transport_failure_classifies_as_network_error() {
        // Bind a listener to grab a free port, then drop it immediately so
        // the port is guaranteed closed — connecting to it fails fast with
        // "connection refused" instead of relying on an arbitrary unused
        // port number that might coincidentally be listened on.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let url = reqwest::Url::parse(&format!("http://{addr}/unreachable")).unwrap();
        let client = test_client();
        let trace = RequestTrace::start("test-provider", "op", "GET", &url);

        let error =
            read_json_response::<TestPayload>(trace, client.get(url.clone()), test_errors())
                .await
                .unwrap_err();
        assert!(
            matches!(error, AuthError::Network(_)),
            "expected Network, got {error:?}"
        );
    }

    /// A 200 response whose body doesn't match the target type must
    /// classify as `AuthError::Decode`, not panic or surface a raw
    /// deserialization error.
    #[tokio::test]
    async fn a_200_response_with_an_unexpected_body_shape_classifies_as_decode_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ok"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"unexpected": "shape"})),
            )
            .mount(&server)
            .await;
        let url = server
            .uri()
            .parse::<reqwest::Url>()
            .unwrap()
            .join("/ok")
            .unwrap();
        let client = test_client();
        let trace = RequestTrace::start("test-provider", "op", "GET", &url);

        let error =
            read_json_response::<TestPayload>(trace, client.get(url.clone()), test_errors())
                .await
                .unwrap_err();
        assert!(
            matches!(error, AuthError::Decode(_)),
            "expected Decode, got {error:?}"
        );
    }
}
