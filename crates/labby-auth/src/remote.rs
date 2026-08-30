use std::net::IpAddr;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use tracing::{info, warn};

use crate::error::AuthError;

pub(crate) const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
pub(crate) const DEFAULT_CACHE_SECS: i64 = 300;
pub(crate) const MAX_CACHE_SECS: i64 = 60 * 60;
pub(crate) const REMOTE_FETCH_DEADLINE: Duration = Duration::from_secs(5);

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            !ip.is_broadcast()
                && !ip.is_documentation()
                && !ip.is_multicast()
                && a != 0
                && !(a == 192 && b == 0)
                && !(a == 198 && (18..=19).contains(&b))
                && a < 240
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            !ip.is_multicast()
                && (segments[0] & 0xffc0) != 0xfec0
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CachePolicy {
    pub cacheable: bool,
    pub max_age_secs: i64,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            cacheable: true,
            max_age_secs: DEFAULT_CACHE_SECS,
        }
    }
}

pub(crate) fn cache_policy(value: Option<&str>) -> CachePolicy {
    let Some(value) = value else {
        return CachePolicy::default();
    };
    let mut policy = CachePolicy::default();
    for directive in value.split(',').map(str::trim) {
        if directive.eq_ignore_ascii_case("no-store") || directive.eq_ignore_ascii_case("no-cache")
        {
            policy.cacheable = false;
        }
        let Some((name, raw_value)) = directive.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("max-age")
            && let Ok(seconds) = raw_value.trim().trim_matches('"').parse::<i64>()
        {
            policy.max_age_secs = seconds.clamp(0, MAX_CACHE_SECS);
        }
    }
    policy
}

/// Fetch a remote OAuth metadata document with redirects disabled and DNS
/// pinned to addresses that passed the shared private-network deny policy.
pub(crate) async fn secure_get(url: &url::Url) -> Result<reqwest::Response, AuthError> {
    tokio::time::timeout(REMOTE_FETCH_DEADLINE, secure_get_inner(url))
        .await
        .map_err(|_| AuthError::Network("remote OAuth document fetch timed out".to_string()))?
}

async fn secure_get_inner(url: &url::Url) -> Result<reqwest::Response, AuthError> {
    let validated = labby_primitives::ssrf::parse_validated_https_url(url.as_str())
        .map_err(|error| AuthError::Validation(error.to_string()))?;
    let host = validated
        .host_str()
        .ok_or_else(|| AuthError::Validation("metadata URL has no host".to_string()))?
        .to_string();
    let port = validated.port_or_known_default().unwrap_or(443);
    let addresses = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|error| AuthError::Network(format!("resolve OAuth metadata host: {error}")))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(AuthError::Network(
            "OAuth metadata host resolved to no addresses".to_string(),
        ));
    }
    for address in &addresses {
        labby_primitives::ssrf::check_ip_not_private(address.ip(), &host)
            .map_err(|error| AuthError::Validation(error.to_string()))?;
        if !is_public_ip(address.ip()) {
            return Err(AuthError::Validation(format!(
                "`{host}` resolves to non-global address {}; blocked to prevent SSRF",
                address.ip()
            )));
        }
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REMOTE_FETCH_DEADLINE)
        .resolve_to_addrs(&host, &addresses)
        .build()
        .map_err(|error| AuthError::Config(format!("build OAuth metadata client: {error}")))?;
    let started = Instant::now();
    info!(
        event = "request.start",
        method = "GET",
        host = %host,
        path = %validated.path(),
        "remote OAuth document request started"
    );
    let response = client
        .get(validated)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| {
            warn!(
                event = "request.error",
                method = "GET",
                host = %host,
                path = %url.path(),
                elapsed_ms = started.elapsed().as_millis(),
                error = %error,
                "remote OAuth document request failed"
            );
            AuthError::Network(format!("fetch OAuth metadata: {error}"))
        })?;
    info!(
        event = "request.finish",
        method = "GET",
        host = %host,
        path = %url.path(),
        status = response.status().as_u16(),
        elapsed_ms = started.elapsed().as_millis(),
        "remote OAuth document request completed"
    );
    Ok(response)
}

pub(crate) async fn fetch_json<T>(
    url: &url::Url,
    document_name: &str,
) -> Result<(T, CachePolicy), AuthError>
where
    T: DeserializeOwned,
{
    let mut response = secure_get(url)
        .await?
        .error_for_status()
        .map_err(|error| AuthError::Network(format!("fetch {document_name}: {error}")))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_DOCUMENT_BYTES as u64)
    {
        return Err(AuthError::Validation(format!(
            "{document_name} exceeds 1 MiB"
        )));
    }
    let policy = cache_policy(
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| AuthError::Network(format!("read {document_name}: {error}")))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_DOCUMENT_BYTES {
            return Err(AuthError::Validation(format!(
                "{document_name} exceeds 1 MiB"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map(|value| (value, policy))
        .map_err(|error| AuthError::Validation(format!("invalid {document_name} JSON: {error}")))
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::{MAX_CACHE_SECS, cache_policy, is_public_ip};

    #[test]
    fn cache_policy_is_case_insensitive_and_bounded() {
        let policy = cache_policy(Some("Public, MAX-AGE=9223372036854775807"));
        assert_eq!(policy.max_age_secs, MAX_CACHE_SECS);
        assert!(policy.cacheable);
        assert!(!cache_policy(Some("max-age=30, No-Store")).cacheable);
        assert!(!cache_policy(Some("NO-CACHE")).cacheable);
    }

    #[test]
    fn public_ip_filter_rejects_special_use_ranges() {
        for ip in [
            "192.0.2.1",
            "198.18.0.1",
            "224.0.0.1",
            "240.0.0.1",
            "2001:db8::1",
            "ff02::1",
        ] {
            assert!(!is_public_ip(ip.parse::<IpAddr>().unwrap()), "{ip}");
        }
        assert!(is_public_ip("8.8.8.8".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }
}
