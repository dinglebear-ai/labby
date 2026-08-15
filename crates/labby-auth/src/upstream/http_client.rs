//! OAuth HTTP client with homelab-aware SSRF enforcement.
//!
//! The operator-selected MCP origin is an explicit trust decision and may
//! resolve to RFC1918, loopback, link-local, CGNAT, or ULA space. Every other
//! OAuth origin remains subject to the public-network SSRF policy. DNS results
//! are pinned into reqwest for each request, and the trusted origin is resolved
//! lazily on its first network operation then reused to close the DNS-rebinding gap.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use rmcp_client::transport::AuthorizationManager;
use rmcp_client::transport::auth::{
    OAuthHttpClient, OAuthHttpClientError, OAuthHttpClientFuture, OAuthHttpRedirectPolicy,
    OAuthHttpRequest,
};
use thiserror::Error;
use tokio::sync::OnceCell;
use url::{Host, Url};

use super::types::OauthError;

const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OAUTH_HTTP_RESPONSE_BODY_BYTES: usize = 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

#[derive(Debug, Error)]
enum OAuthEgressError {
    #[error("invalid OAuth URL: {0}")]
    InvalidUrl(String),
    #[error("OAuth request blocked by SSRF policy: {0}")]
    SsrfBlocked(String),
    #[error("OAuth DNS resolution failed: {0}")]
    Dns(String),
    #[error("OAuth HTTP client failed: {0}")]
    Http(String),
    #[error("OAuth HTTP response body exceeds {0} bytes")]
    ResponseBodyTooLarge(usize),
}

impl OAuthEgressError {
    fn kind(&self) -> &'static str {
        match self {
            Self::InvalidUrl(_) => "validation_failed",
            Self::SsrfBlocked(_) => "ssrf_blocked",
            Self::Dns(_) => "dns_error",
            Self::Http(message) if message.starts_with("timeout: ") => "timeout",
            Self::Http(_) => "network_error",
            Self::ResponseBodyTooLarge(_) => "response_too_large",
        }
    }

    fn into_oauth_error(self, context: &str) -> OauthError {
        OauthError::Egress {
            kind: self.kind(),
            message: format!("{context}: {self}"),
        }
    }
}

fn boxed_error(error: OAuthEgressError) -> OAuthHttpClientError {
    Box::new(error)
}

fn reqwest_error(error: reqwest::Error) -> OAuthEgressError {
    let is_timeout = error.is_timeout();
    let error = error.without_url();
    let message = if is_timeout {
        format!("timeout: {error}")
    } else {
        error.to_string()
    };
    OAuthEgressError::Http(message)
}

/// HTTP policy for an operator-selected upstream OAuth resource.
///
/// The exact resource origin is trusted to use private addressing. Cross-origin
/// destinations must resolve entirely to public addresses.
#[derive(Clone)]
pub(crate) struct TrustedOriginOAuthHttpClient {
    trusted_origin: Url,
    trusted_addresses: Arc<OnceCell<Vec<SocketAddr>>>,
}

impl TrustedOriginOAuthHttpClient {
    pub(crate) fn new(base_url: &str) -> Result<Self, OauthError> {
        drop(rustls::crypto::ring::default_provider().install_default());
        let trusted_origin = parse_oauth_url(base_url).map_err(|error| {
            OauthError::Internal(format!("invalid upstream OAuth URL: {error}"))
        })?;
        Ok(Self {
            trusted_origin,
            trusted_addresses: Arc::new(OnceCell::new()),
        })
    }

    async fn addresses_for(&self, url: &Url) -> Result<Vec<SocketAddr>, OAuthEgressError> {
        validate_request_url(url)?;
        if same_origin(&self.trusted_origin, url) {
            let addresses = self
                .trusted_addresses
                .get_or_try_init(|| async {
                    let addresses = resolve_addresses(&self.trusted_origin).await?;
                    for address in &addresses {
                        check_connectable(address.ip())?;
                    }
                    Ok::<_, OAuthEgressError>(addresses)
                })
                .await?;
            return Ok(addresses.clone());
        }

        let host = url
            .host_str()
            .ok_or_else(|| OAuthEgressError::InvalidUrl("URL has no host".to_string()))?;
        if matches!(url.host(), Some(Host::Domain(_))) {
            labby_primitives::ssrf::check_host_not_private(host)
                .map_err(|error| OAuthEgressError::SsrfBlocked(error.to_string()))?;
        }

        let addresses = resolve_addresses(url).await?;
        for address in &addresses {
            labby_primitives::ssrf::check_ip_not_private(address.ip(), host)
                .map_err(|error| OAuthEgressError::SsrfBlocked(error.to_string()))?;
            check_connectable(address.ip())?;
        }
        Ok(addresses)
    }

    async fn execute_reqwest(
        &self,
        request: reqwest::Request,
        redirect_policy: OAuthHttpRedirectPolicy,
        timeout: Option<Duration>,
    ) -> Result<oauth2::HttpResponse, OAuthHttpClientError> {
        let started = std::time::Instant::now();
        let method = request.method().clone();
        let path = request.url().path().to_string();
        let request_host = request.url().host_str().unwrap_or("<missing>").to_string();
        tracing::info!(
            event = "request.start",
            service = "upstream_oauth",
            method = %method,
            path,
            host = %request_host,
            "outbound OAuth request started"
        );
        let result = self
            .execute_reqwest_inner(request, redirect_policy, timeout)
            .await;
        match &result {
            Ok(response) => tracing::info!(
                event = "request.finish",
                service = "upstream_oauth",
                method = %method,
                path,
                host = %request_host,
                status = response.status().as_u16(),
                elapsed_ms = started.elapsed().as_millis(),
                "outbound OAuth request finished"
            ),
            Err(error) => tracing::warn!(
                event = "request.error",
                service = "upstream_oauth",
                method = %method,
                path,
                host = %request_host,
                kind = oauth_http_error_kind(error),
                message = %error,
                elapsed_ms = started.elapsed().as_millis(),
                "outbound OAuth request failed"
            ),
        }
        result
    }

    async fn execute_reqwest_inner(
        &self,
        request: reqwest::Request,
        redirect_policy: OAuthHttpRedirectPolicy,
        timeout: Option<Duration>,
    ) -> Result<oauth2::HttpResponse, OAuthHttpClientError> {
        let url = request.url().clone();
        let addresses = self
            .addresses_for(&url)
            .await
            .map_err(|error| boxed_error(error))?;
        let host = url
            .host_str()
            .ok_or_else(|| {
                boxed_error(OAuthEgressError::InvalidUrl("URL has no host".to_string()))
            })?
            .to_string();

        let redirect = match redirect_policy {
            OAuthHttpRedirectPolicy::Stop => reqwest::redirect::Policy::none(),
            OAuthHttpRedirectPolicy::Follow => same_origin_redirect_policy(&url),
            _ => reqwest::redirect::Policy::none(),
        };
        let mut builder = configure_oauth_client_builder(
            reqwest::Client::builder(),
            timeout.unwrap_or(DEFAULT_HTTP_TIMEOUT),
            redirect,
        );
        if matches!(url.host(), Some(Host::Domain(_))) {
            builder = builder.resolve_to_addrs(&host, &addresses);
        }
        let client = builder
            .build()
            .map_err(|error| boxed_error(reqwest_error(error)))?;
        let mut response = client
            .execute(request)
            .await
            .map_err(|error| boxed_error(reqwest_error(error)))?;

        let status = response.status();
        let version = response.version();
        let headers = response.headers().clone();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| boxed_error(reqwest_error(error)))?
        {
            if chunk.len() > MAX_OAUTH_HTTP_RESPONSE_BODY_BYTES.saturating_sub(body.len()) {
                return Err(boxed_error(OAuthEgressError::ResponseBodyTooLarge(
                    MAX_OAUTH_HTTP_RESPONSE_BODY_BYTES,
                )));
            }
            body.extend_from_slice(&chunk);
        }

        let mut response_builder = oauth2::http::Response::builder()
            .status(status)
            .version(version);
        for (name, value) in &headers {
            response_builder = response_builder.header(name, value);
        }
        response_builder
            .body(body)
            .map_err(|error| boxed_error(OAuthEgressError::Http(error.to_string())))
    }

    pub(crate) async fn get(&self, url: Url) -> Result<oauth2::HttpResponse, OauthError> {
        let request = reqwest::Request::new(reqwest::Method::GET, url);
        self.execute_reqwest(
            request,
            OAuthHttpRedirectPolicy::Stop,
            Some(DEFAULT_HTTP_TIMEOUT),
        )
        .await
        .map_err(|error| match error.downcast::<OAuthEgressError>() {
            Ok(error) => error.into_oauth_error("fetch OAuth metadata"),
            Err(error) => OauthError::Egress {
                kind: "network_error",
                message: format!("fetch OAuth metadata: {error}"),
            },
        })
    }
}

fn configure_oauth_client_builder(
    builder: reqwest::ClientBuilder,
    timeout: Duration,
    redirect: reqwest::redirect::Policy,
) -> reqwest::ClientBuilder {
    // System proxy discovery would move DNS resolution to the proxy and defeat
    // the address validation and pinning performed for every request.
    builder.timeout(timeout).redirect(redirect).no_proxy()
}

fn oauth_http_error_kind(error: &OAuthHttpClientError) -> &'static str {
    error
        .downcast_ref::<OAuthEgressError>()
        .map_or("network_error", OAuthEgressError::kind)
}

impl OAuthHttpClient for TrustedOriginOAuthHttpClient {
    fn execute(&self, request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
        Box::pin(async move {
            let redirect_policy = request.redirect_policy;
            let timeout = request.timeout;
            let request = reqwest::Request::try_from(request.request)
                .map_err(|error| boxed_error(OAuthEgressError::Http(error.to_string())))?;
            self.execute_reqwest(request, redirect_policy, timeout)
                .await
        })
    }
}

/// Build rmcp's authorization manager with LABBY's egress policy installed for
/// every discovery, registration, token, and refresh request.
pub async fn authorization_manager_for_upstream(
    upstream_url: &str,
) -> Result<AuthorizationManager, OauthError> {
    let client = TrustedOriginOAuthHttpClient::new(upstream_url)?;
    AuthorizationManager::new_with_oauth_http_client(upstream_url, Arc::new(client))
        .await
        .map_err(|_| OauthError::Egress {
            kind: "network_error",
            // rmcp's aggregate error may retain a discovery URL. The custom
            // client already emits a redacted request.error event, so do not
            // echo the third-party wrapper and risk exposing query secrets.
            message: "create auth manager: OAuth discovery failed".to_string(),
        })
}

fn parse_oauth_url(raw: &str) -> Result<Url, OAuthEgressError> {
    let parsed =
        Url::parse(raw).map_err(|error| OAuthEgressError::InvalidUrl(error.to_string()))?;
    validate_request_url(&parsed)?;
    Ok(parsed)
}

fn validate_request_url(url: &Url) -> Result<(), OAuthEgressError> {
    if url.scheme() != "https" && !(url.scheme() == "http" && is_loopback_url(url)) {
        return Err(OAuthEgressError::InvalidUrl(
            "OAuth requests must use https, except for loopback http".to_string(),
        ));
    }
    if url.host().is_none() {
        return Err(OAuthEgressError::InvalidUrl("URL has no host".to_string()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(OAuthEgressError::InvalidUrl(
            "URL must not include userinfo".to_string(),
        ));
    }
    if url.fragment().is_some() {
        return Err(OAuthEgressError::InvalidUrl(
            "URL must not include a fragment".to_string(),
        ));
    }
    Ok(())
}

async fn resolve_addresses(url: &Url) -> Result<Vec<SocketAddr>, OAuthEgressError> {
    let port = url
        .port_or_known_default()
        .ok_or_else(|| OAuthEgressError::InvalidUrl("URL has no resolvable port".to_string()))?;
    let addresses = match url.host() {
        Some(Host::Ipv4(ip)) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
        Some(Host::Ipv6(ip)) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
        Some(Host::Domain(host)) => tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| OAuthEgressError::Dns(format!("resolve {host}: {error}")))?
            .collect(),
        None => Vec::new(),
    };
    if addresses.is_empty() {
        return Err(OAuthEgressError::Dns(
            "OAuth host resolved to no addresses".to_string(),
        ));
    }
    Ok(addresses)
}

fn check_connectable(ip: IpAddr) -> Result<(), OAuthEgressError> {
    let blocked = match ip {
        IpAddr::V4(ip) => ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast(),
        IpAddr::V6(ip) => ip.is_unspecified() || ip.is_multicast(),
    };
    if blocked {
        return Err(OAuthEgressError::SsrfBlocked(format!(
            "address {ip} is not a valid outbound OAuth destination"
        )));
    }
    Ok(())
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        Some(Host::Domain(host)) => {
            host.eq_ignore_ascii_case("localhost")
                || host.to_ascii_lowercase().ends_with(".localhost")
        }
        None => false,
    }
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left
            .host_str()
            .zip(right.host_str())
            .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
        && left.port_or_known_default() == right.port_or_known_default()
}

fn same_origin_redirect_policy(origin: &Url) -> reqwest::redirect::Policy {
    let origin = origin.clone();
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.stop();
        }
        if same_origin(&origin, attempt.url()) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve_once(status: &str, headers: &[(&str, String)], body: Vec<u8>) -> Url {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_string();
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            let mut response = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
            for (name, value) in headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("Connection: close\r\n\r\n");
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        });
        Url::parse(&format!("http://{address}/metadata")).unwrap()
    }

    #[tokio::test]
    async fn trusted_private_origin_is_allowed_and_pinned() {
        let client = TrustedOriginOAuthHttpClient::new("https://10.1.0.8/mcp")
            .expect("trusted private upstream");
        let target = Url::parse("https://10.1.0.8/token").unwrap();
        let addresses = client.addresses_for(&target).await.expect("same origin");
        assert_eq!(addresses, vec!["10.1.0.8:443".parse().unwrap()]);
    }

    #[tokio::test]
    async fn trusted_hostname_reuses_the_first_pinned_resolution() {
        let client = TrustedOriginOAuthHttpClient::new("https://oauth.example.invalid/mcp")
            .expect("trusted upstream");
        let pinned = vec!["203.0.113.10:443".parse().unwrap()];
        client.trusted_addresses.set(pinned.clone()).unwrap();

        let target = Url::parse("https://oauth.example.invalid/token").unwrap();
        assert_eq!(client.addresses_for(&target).await.unwrap(), pinned);
    }

    #[tokio::test]
    async fn different_private_origin_is_blocked() {
        let client = TrustedOriginOAuthHttpClient::new("https://10.1.0.8/mcp")
            .expect("trusted private upstream");
        let target = Url::parse("https://10.1.0.9/token").unwrap();
        let error = client
            .addresses_for(&target)
            .await
            .expect_err("cross-origin private target must be blocked");
        assert!(matches!(error, OAuthEgressError::SsrfBlocked(_)));
    }

    #[tokio::test]
    async fn different_public_origin_is_allowed() {
        let client = TrustedOriginOAuthHttpClient::new("https://10.1.0.8/mcp")
            .expect("trusted private upstream");
        let target = Url::parse("https://8.8.8.8/token").unwrap();
        let addresses = client.addresses_for(&target).await.expect("public target");
        assert_eq!(addresses, vec!["8.8.8.8:443".parse().unwrap()]);
    }

    #[tokio::test]
    async fn transport_errors_do_not_include_request_urls() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let secret = "client_secret=must-not-leak";
        let error = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(10))
            .build()
            .unwrap()
            .get(format!("http://{address}/token?{secret}"))
            .send()
            .await
            .expect_err("server intentionally withholds its response");

        let error = reqwest_error(error);
        assert_eq!(error.kind(), "timeout");
        assert!(!error.to_string().contains(secret));
        assert!(!error.to_string().contains(&address.to_string()));
        server.abort();
    }

    #[test]
    fn origin_comparison_includes_scheme_and_port() {
        let base = Url::parse("https://example.test:8443/mcp").unwrap();
        assert!(same_origin(
            &base,
            &Url::parse("https://EXAMPLE.test:8443/token").unwrap()
        ));
        assert!(!same_origin(
            &base,
            &Url::parse("https://example.test/token").unwrap()
        ));
        assert!(!same_origin(
            &base,
            &Url::parse("http://example.test:8443/token").unwrap()
        ));
    }

    #[test]
    fn request_url_validation_covers_security_boundaries() {
        for raw in [
            "http://example.com/token",
            "https://user:secret@example.com/token",
            "https://example.com/token#fragment",
        ] {
            assert!(parse_oauth_url(raw).is_err(), "{raw}");
        }
        assert!(parse_oauth_url("http://localhost/token").is_ok());
        assert!(parse_oauth_url("http://127.0.0.1/token").is_ok());
    }

    #[tokio::test]
    async fn response_body_limit_accepts_exact_limit_and_rejects_overflow() {
        let exact_url = serve_once(
            "200 OK",
            &[],
            vec![b'x'; MAX_OAUTH_HTTP_RESPONSE_BODY_BYTES],
        )
        .await;
        let exact_client = TrustedOriginOAuthHttpClient::new(exact_url.as_str()).unwrap();
        let response = exact_client.get(exact_url).await.unwrap();
        assert_eq!(response.body().len(), MAX_OAUTH_HTTP_RESPONSE_BODY_BYTES);

        let overflow_url = serve_once(
            "200 OK",
            &[],
            vec![b'x'; MAX_OAUTH_HTTP_RESPONSE_BODY_BYTES + 1],
        )
        .await;
        let overflow_client = TrustedOriginOAuthHttpClient::new(overflow_url.as_str()).unwrap();
        let error = overflow_client.get(overflow_url).await.unwrap_err();
        assert_eq!(error.kind(), "response_too_large");
    }

    #[tokio::test]
    async fn redirect_policy_stops_cross_origin() {
        let final_url = serve_once("200 OK", &[], b"done".to_vec()).await;
        let same_origin_redirect = serve_once(
            "302 Found",
            &[("Location", final_url.to_string())],
            Vec::new(),
        )
        .await;
        // The helpers use distinct ephemeral ports, so this is cross-origin and
        // must be returned without contacting the Location target.
        let client = TrustedOriginOAuthHttpClient::new(same_origin_redirect.as_str()).unwrap();
        let request = reqwest::Request::new(reqwest::Method::GET, same_origin_redirect);
        let response = client
            .execute_reqwest(request, OAuthHttpRedirectPolicy::Follow, None)
            .await
            .unwrap();
        assert_eq!(response.status(), oauth2::http::StatusCode::FOUND);
    }

    #[tokio::test]
    async fn oauth_client_disables_even_an_explicit_proxy() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let origin = serve_once("200 OK", &[], b"direct".to_vec()).await;
        let unreachable_proxy = reqwest::Proxy::all("http://127.0.0.1:9").unwrap();
        let client = configure_oauth_client_builder(
            reqwest::Client::builder().proxy(unreachable_proxy),
            Duration::from_secs(2),
            reqwest::redirect::Policy::none(),
        )
        .build()
        .unwrap();
        let response = client.get(origin).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }
}
