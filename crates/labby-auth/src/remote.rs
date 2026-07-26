use std::time::Duration;

use crate::error::AuthError;

/// Fetch a remote OAuth metadata document with redirects disabled and DNS
/// pinned to addresses that passed the shared private-network deny policy.
pub(crate) async fn secure_get(url: &url::Url) -> Result<reqwest::Response, AuthError> {
    let validated = labby_primitives::ssrf::parse_validated_https_url(url.as_str())
        .map_err(|error| AuthError::Validation(error.to_string()))?;
    let host = validated
        .host_str()
        .ok_or_else(|| AuthError::Validation("metadata URL has no host".to_string()))?;
    let port = validated.port_or_known_default().unwrap_or(443);
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| AuthError::Network(format!("resolve OAuth metadata host: {error}")))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(AuthError::Network(
            "OAuth metadata host resolved to no addresses".to_string(),
        ));
    }
    for address in &addresses {
        labby_primitives::ssrf::check_ip_not_private(address.ip(), host)
            .map_err(|error| AuthError::Validation(error.to_string()))?;
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .resolve_to_addrs(host, &addresses)
        .build()
        .map_err(|error| AuthError::Config(format!("build OAuth metadata client: {error}")))?;
    client
        .get(validated)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| AuthError::Network(format!("fetch OAuth metadata: {error}")))
}
