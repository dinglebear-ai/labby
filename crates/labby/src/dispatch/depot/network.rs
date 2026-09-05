//! Pinned HTTPS transport for every Depot discovery and probe request.
//! Hyper is used directly to bound HTTP/1 framing before header allocation.

use std::collections::{BTreeMap, BTreeSet};
use std::future::{Ready, ready};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Bytes;
use axum::http::{HeaderValue, Method, Request, StatusCode};
use http_body_util::{BodyExt as _, Full};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::{
    Client,
    connect::{HttpConnector, dns::Name},
};
use hyper_util::rt::TokioExecutor;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tower::Service;
use url::Url;

use crate::config::depot::canonical_endpoint;

pub const MAX_BODY: usize = 1024 * 1024;
const MAX_HEADERS: usize = 32 * 1024;
const DNS_LEASE: Duration = Duration::from_mins(1);
const ATTEMPT: Duration = Duration::from_secs(3);
const IO_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NetworkError {
    #[error("Depot endpoint is invalid")]
    InvalidEndpoint,
    #[error("Depot address is blocked by host policy")]
    Blocked,
    #[error("Depot credential does not match endpoint")]
    CredentialBinding,
    #[error("Depot request deadline exceeded")]
    Timeout,
    #[error("Depot transport unavailable")]
    Unavailable,
    #[error("Depot response exceeds limit")]
    TooLarge,
    #[error("Depot response is invalid")]
    InvalidResponse,
    #[error("Depot rejected request ({0})")]
    Status(u16),
}

#[derive(Clone)]
pub struct Secret {
    endpoint: Url,
    value: HeaderValue,
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl Secret {
    pub fn bearer(endpoint: &str, value: &str) -> Result<Self, NetworkError> {
        let endpoint = canonical_endpoint(endpoint).map_err(|_| NetworkError::InvalidEndpoint)?;
        if value.is_empty() || value.len() > 8192 {
            return Err(NetworkError::CredentialBinding);
        }
        let mut value = HeaderValue::from_str(&format!("Bearer {value}"))
            .map_err(|_| NetworkError::CredentialBinding)?;
        value.set_sensitive(true);
        Ok(Self { endpoint, value })
    }
}

/// Only the server composition root can supply exact private-host grants.
/// Link-local, mapped IPv6, multicast, loopback and metadata are never grants.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkPolicy {
    pub private_hosts: BTreeMap<String, BTreeSet<IpAddr>>,
    #[cfg(test)]
    pub(crate) allow_test_loopback: bool,
}

pub fn validate_addresses(
    host: &str,
    addresses: &[IpAddr],
    policy: &NetworkPolicy,
) -> Result<(), NetworkError> {
    if addresses.is_empty() || addresses.len() > 32 {
        return Err(NetworkError::Blocked);
    }
    for address in addresses {
        // AWS and GCP metadata IPv6 endpoints are ULA, but must never be
        // admitted by the host's private-address grants.
        if matches!(address, IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some()
            || matches!(ip.segments(), [0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254]
                | [0xfd20, 0xce, 0, 0, 0, 0, 0, 0x254]))
        {
            return Err(NetworkError::Blocked);
        }
        #[cfg(test)]
        if policy.allow_test_loopback && address.is_loopback() {
            continue;
        }
        let private = match address {
            IpAddr::V4(ip) => ip.is_private(),
            IpAddr::V6(ip) => ip.is_unique_local(),
        };
        if private
            && policy
                .private_hosts
                .get(host)
                .is_some_and(|allowed| allowed.contains(address))
        {
            continue;
        }
        labby_primitives::ssrf::check_ip_not_private(*address, "Depot")
            .map_err(|_| NetworkError::Blocked)?;
    }
    Ok(())
}

#[derive(Clone)]
struct PinnedResolver {
    host: Arc<str>,
    addresses: Vec<SocketAddr>,
}

impl Service<Name> for PinnedResolver {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = std::io::Error;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn call(&mut self, name: Name) -> Self::Future {
        ready(if name.as_str() == self.host.as_ref() {
            Ok(self.addresses.clone().into_iter())
        } else {
            Err(std::io::Error::other("unapproved Depot host"))
        })
    }
}

type PinnedClient = Client<HttpsConnector<HttpConnector<PinnedResolver>>, Full<Bytes>>;
struct Lease {
    client: PinnedClient,
    expires: Instant,
}

#[derive(Debug, Clone, Copy)]
pub enum Operation {
    Identity,
    List,
    Get,
}

impl Operation {
    fn path(self) -> &'static str {
        match self {
            Self::Identity => "api/discovery",
            Self::List => "api/discovery/list",
            Self::Get => "api/discovery/get",
        }
    }
}

pub struct NetworkClient {
    endpoint: Url,
    secret: Option<Secret>,
    policy: NetworkPolicy,
    lease: Mutex<Option<Lease>>,
    #[cfg(test)]
    test_addresses: Option<Vec<IpAddr>>,
    #[cfg(test)]
    test_tls: Option<rustls::ClientConfig>,
}

impl NetworkClient {
    #[cfg(test)]
    pub(super) async fn expire_test_lease(&mut self, addresses: Vec<IpAddr>) {
        self.test_addresses = Some(addresses);
        if let Some(lease) = self.lease.get_mut() {
            lease.expires = Instant::now();
        }
    }
    #[cfg(test)]
    pub(super) fn with_test_connection(
        mut self,
        addresses: Vec<IpAddr>,
        tls: rustls::ClientConfig,
    ) -> Self {
        self.test_addresses = Some(addresses);
        self.test_tls = Some(tls);
        self
    }

    /// Pure construction: no DNS, sockets, certificate reads, or requests.
    pub fn new(
        endpoint: &str,
        secret: Option<Secret>,
        policy: NetworkPolicy,
    ) -> Result<Self, NetworkError> {
        let endpoint = canonical_endpoint(endpoint).map_err(|_| NetworkError::InvalidEndpoint)?;
        if secret
            .as_ref()
            .is_some_and(|secret| secret.endpoint != endpoint)
        {
            return Err(NetworkError::CredentialBinding);
        }
        Ok(Self {
            endpoint,
            secret,
            policy,
            lease: Mutex::new(None),
            #[cfg(test)]
            test_addresses: None,
            #[cfg(test)]
            test_tls: None,
        })
    }

    pub async fn call(
        &self,
        operation: Operation,
        body: Option<Value>,
        deadline: Instant,
    ) -> Result<Value, NetworkError> {
        let deadline = deadline.min(Instant::now() + ATTEMPT);
        tokio::time::timeout_at(deadline, self.request(operation, body))
            .await
            .map_err(|_| NetworkError::Timeout)?
    }

    async fn request(
        &self,
        operation: Operation,
        body: Option<Value>,
    ) -> Result<Value, NetworkError> {
        let client = self.client().await?;
        let url = self
            .endpoint
            .join(operation.path())
            .map_err(|_| NetworkError::InvalidEndpoint)?;
        let bytes = body
            .map(|body| serde_json::to_vec(&body))
            .transpose()
            .map_err(|_| NetworkError::InvalidResponse)?
            .unwrap_or_default();
        if bytes.len() > MAX_BODY {
            return Err(NetworkError::TooLarge);
        }
        let mut builder = Request::builder()
            .uri(url.as_str())
            .method(if matches!(operation, Operation::Identity) {
                Method::GET
            } else {
                Method::POST
            })
            .header("accept", "application/json")
            .header("accept-encoding", "identity")
            .header("content-type", "application/json");
        if let Some(secret) = &self.secret {
            builder = builder.header("authorization", secret.value.clone());
        }
        let request = builder
            .body(Full::new(Bytes::from(bytes)))
            .map_err(|_| NetworkError::InvalidResponse)?;
        let mut response = tokio::time::timeout(IO_TIMEOUT, client.request(request))
            .await
            .map_err(|_| NetworkError::Timeout)?
            .map_err(|_| NetworkError::Unavailable)?;
        let header_bytes: usize = response
            .headers()
            .iter()
            .map(|(key, value)| key.as_str().len() + value.len() + 4)
            .sum();
        if header_bytes > MAX_HEADERS {
            return Err(NetworkError::TooLarge);
        }
        if response
            .headers()
            .get("content-encoding")
            .is_some_and(|value| value != "identity")
        {
            return Err(NetworkError::InvalidResponse);
        }
        if response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|size| size > MAX_BODY as u64)
        {
            return Err(NetworkError::TooLarge);
        }
        let status = response.status();
        // No redirects and no upstream error bodies, which can contain secrets.
        if status != StatusCode::OK {
            return Err(NetworkError::Status(status.as_u16()));
        }
        let mut bytes = Vec::new();
        while let Some(frame) = tokio::time::timeout(IO_TIMEOUT, response.body_mut().frame())
            .await
            .map_err(|_| NetworkError::Timeout)?
        {
            let frame = frame.map_err(|_| NetworkError::Unavailable)?;
            if let Ok(data) = frame.into_data() {
                if bytes.len().saturating_add(data.len()) > MAX_BODY {
                    return Err(NetworkError::TooLarge);
                }
                bytes.extend_from_slice(&data);
            }
        }
        validate_json_depth(&bytes)?;
        serde_json::from_slice(&bytes).map_err(|_| NetworkError::InvalidResponse)
    }

    async fn client(&self) -> Result<PinnedClient, NetworkError> {
        let mut lease = self.lease.lock().await;
        if let Some(current) = lease
            .as_ref()
            .filter(|lease| lease.expires > Instant::now())
        {
            return Ok(current.client.clone());
        }
        *lease = None; // expired pools cannot acquire new requests
        let host = self
            .endpoint
            .host_str()
            .ok_or(NetworkError::InvalidEndpoint)?;
        let port = self
            .endpoint
            .port_or_known_default()
            .ok_or(NetworkError::InvalidEndpoint)?;
        let addresses = self.resolve(host, port).await?;
        validate_addresses(host, &addresses, &self.policy)?;
        let resolver = PinnedResolver {
            host: Arc::from(host),
            addresses: addresses
                .into_iter()
                .map(|ip| SocketAddr::new(ip, port))
                .collect(),
        };
        let mut connector = HttpConnector::new_with_resolver(resolver);
        connector.enforce_http(false);
        connector.set_connect_timeout(Some(IO_TIMEOUT));
        let tls = self.tls()?;
        let connector = tls
            .https_only()
            .enable_http1()
            .enable_http2()
            .wrap_connector(connector);
        let mut builder = Client::builder(TokioExecutor::new());
        builder
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(Duration::from_secs(30))
            .http1_max_buf_size(MAX_HEADERS)
            .http1_max_headers(100)
            .http2_max_header_list_size(MAX_HEADERS as u32);
        let client = builder.build(connector);
        *lease = Some(Lease {
            client: client.clone(),
            expires: Instant::now() + DNS_LEASE,
        });
        Ok(client)
    }

    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, NetworkError> {
        #[cfg(test)]
        if let Some(addresses) = &self.test_addresses {
            return Ok(addresses.clone());
        }
        if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        let addresses = tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| NetworkError::Unavailable)?;
        Ok(addresses.take(33).map(|address| address.ip()).collect())
    }

    fn tls(
        &self,
    ) -> Result<
        hyper_rustls::HttpsConnectorBuilder<hyper_rustls::builderstates::WantsSchemes>,
        NetworkError,
    > {
        #[cfg(test)]
        if let Some(tls) = &self.test_tls {
            return Ok(hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(tls.clone()));
        }
        hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_native_roots(Arc::new(rustls::crypto::ring::default_provider()))
            .map_err(|_| NetworkError::Unavailable)
    }
}

/// Bound nesting before serde allocates the parsed projection. Strings and
/// escapes are skipped; malformed JSON is subsequently rejected by serde.
fn validate_json_depth(bytes: &[u8]) -> Result<(), NetworkError> {
    let (mut depth, mut string, mut escaped) = (0_u8, false, false);
    for byte in bytes {
        if string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                string = false;
            }
        } else {
            match byte {
                b'"' => string = true,
                b'{' | b'[' => {
                    depth = depth.checked_add(1).ok_or(NetworkError::TooLarge)?;
                    if depth > 64 {
                        return Err(NetworkError::TooLarge);
                    }
                }
                b'}' | b']' => depth = depth.checked_sub(1).ok_or(NetworkError::InvalidResponse)?,
                _ => {}
            }
        }
    }
    Ok(())
}
