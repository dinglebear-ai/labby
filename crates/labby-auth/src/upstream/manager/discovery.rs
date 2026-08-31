//! Authorization-server discovery and issuer/endpoint validation policy.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DynamicClientRegistrationUse {
    BeginAuthorization,
    CompleteAuthorization,
    StoredCredentials,
}

#[derive(Debug, Deserialize)]
pub(super) struct ProtectedResourceMetadata {
    #[serde(default)]
    pub(super) resource: Option<String>,
    #[serde(default)]
    pub(super) authorization_server: Option<String>,
    #[serde(default)]
    pub(super) authorization_servers: Option<Vec<String>>,
}

/// Fetch published OAuth metadata without applying rmcp's discovery-URL
/// issuer equality check.
///
/// Labby validates issuer and endpoint origins itself in
/// `verify_issuer_binding`, including the explicitly allowed Google split
/// token endpoint. rmcp 3 validates the issuer against the metadata URL while
/// fetching, which would reject that policy before Labby can apply it.
pub async fn discover_published_metadata(
    upstream_url: &str,
) -> Result<Option<AuthorizationMetadata>, OauthError> {
    tokio::time::timeout(
        OAUTH_METADATA_DISCOVERY_TIMEOUT,
        discover_published_metadata_inner(upstream_url),
    )
    .await
    .map_err(|_| OauthError::Egress {
        kind: OAuthEgressKind::Timeout,
        message: "OAuth metadata discovery exceeded its overall deadline".to_string(),
    })?
}

async fn discover_published_metadata_inner(
    upstream_url: &str,
) -> Result<Option<AuthorizationMetadata>, OauthError> {
    let upstream = url::Url::parse(upstream_url).map_err(|error| OauthError::Egress {
        kind: OAuthEgressKind::ValidationFailed,
        message: format!("invalid upstream OAuth URL: {error}"),
    })?;
    let client = TrustedOriginOAuthHttpClient::new(upstream_url)?;
    let mut first_error = None;

    for metadata_url in protected_resource_metadata_candidates(&upstream) {
        let Some(resource_metadata) = fetch_metadata::<ProtectedResourceMetadata>(
            &client,
            metadata_url.clone(),
            "protected-resource",
            &mut first_error,
        )
        .await?
        else {
            continue;
        };
        if resource_metadata
            .resource
            .as_deref()
            .is_some_and(|resource| resource != upstream.as_str())
        {
            return Err(OauthError::ResourceMismatch(
                "protected-resource metadata does not match the configured upstream resource"
                    .to_string(),
            ));
        }

        let authorization_servers = bounded_authorization_servers(resource_metadata)?;

        for authorization_server in authorization_servers {
            let selected_issuer = authorization_server.trim().to_string();
            let server_url = match resolve_authorization_server_url(
                &metadata_url,
                authorization_server.trim(),
            ) {
                Ok(url) => url,
                Err(error) => {
                    remember_metadata_error(
                        &mut first_error,
                        OauthError::Egress {
                            kind: OAuthEgressKind::ValidationFailed,
                            message: format!("invalid OAuth authorization server URL: {error}"),
                        },
                    );
                    continue;
                }
            };
            for authorization_metadata_url in authorization_metadata_candidates(&server_url) {
                if let Some(metadata) = fetch_metadata::<AuthorizationMetadata>(
                    &client,
                    authorization_metadata_url,
                    "authorization",
                    &mut first_error,
                )
                .await?
                {
                    let expected_issuer = if url::Url::parse(&selected_issuer).is_ok() {
                        selected_issuer.as_str()
                    } else {
                        server_url.as_str()
                    };
                    validate_discovered_issuer(&metadata, expected_issuer)?;
                    return Ok(Some(metadata));
                }
            }
        }
    }

    for authorization_metadata_url in authorization_metadata_candidates(&upstream) {
        if let Some(metadata) = fetch_metadata::<AuthorizationMetadata>(
            &client,
            authorization_metadata_url,
            "authorization",
            &mut first_error,
        )
        .await?
        {
            validate_discovered_issuer(&metadata, upstream_url)?;
            return Ok(Some(metadata));
        }
    }

    first_error.map_or(Ok(None), Err)
}

async fn fetch_metadata<T: DeserializeOwned>(
    client: &TrustedOriginOAuthHttpClient,
    url: url::Url,
    metadata_kind: &str,
    first_error: &mut Option<OauthError>,
) -> Result<Option<T>, OauthError> {
    let response = match client.get(url).await {
        Ok(response) => response,
        Err(error) => {
            remember_or_return_metadata_error(first_error, error)?;
            return Ok(None);
        }
    };
    if metadata_not_found(&response) {
        return Ok(None);
    }
    if !response.status().is_success() {
        remember_metadata_error(first_error, metadata_http_error(response.status()));
        return Ok(None);
    }
    match serde_json::from_slice(response.body()) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) => {
            remember_metadata_error(first_error, invalid_metadata_error(metadata_kind, error));
            Ok(None)
        }
    }
}

pub(super) fn bounded_authorization_servers(
    metadata: ProtectedResourceMetadata,
) -> Result<Vec<String>, OauthError> {
    let mut servers = metadata.authorization_servers.unwrap_or_default();
    if let Some(server) = metadata.authorization_server {
        servers.insert(0, server);
    }
    let mut seen = std::collections::HashSet::new();
    servers.retain(|server| seen.insert(server.trim().to_string()));
    if servers.len() > MAX_AUTHORIZATION_SERVERS {
        return Err(OauthError::Egress {
            kind: OAuthEgressKind::ValidationFailed,
            message: format!(
                "OAuth protected-resource metadata lists {} authorization servers; maximum is {MAX_AUTHORIZATION_SERVERS}",
                servers.len()
            ),
        });
    }
    Ok(servers)
}

fn remember_metadata_error(first_error: &mut Option<OauthError>, error: OauthError) {
    first_error.get_or_insert(error);
}

fn remember_or_return_metadata_error(
    first_error: &mut Option<OauthError>,
    error: OauthError,
) -> Result<(), OauthError> {
    if terminal_metadata_error(&error) {
        return Err(error);
    }
    remember_metadata_error(first_error, error);
    Ok(())
}

fn terminal_metadata_error(error: &OauthError) -> bool {
    matches!(error, OauthError::Egress { kind, .. } if kind.is_terminal_discovery())
}

fn metadata_not_found(response: &oauth2::HttpResponse) -> bool {
    matches!(response.status().as_u16(), 404 | 410)
}

fn metadata_http_error(status: oauth2::http::StatusCode) -> OauthError {
    OauthError::Egress {
        kind: OAuthEgressKind::UpstreamError,
        message: format!("OAuth metadata returned HTTP {status}"),
    }
}

fn invalid_metadata_error(kind: &str, error: serde_json::Error) -> OauthError {
    OauthError::Egress {
        kind: OAuthEgressKind::ValidationFailed,
        message: format!("invalid OAuth {kind} metadata: {error}"),
    }
}

fn protected_resource_metadata_candidates(upstream: &url::Url) -> Vec<url::Url> {
    let trimmed = upstream
        .path()
        .trim_start_matches('/')
        .trim_end_matches('/');
    let paths = if trimmed.is_empty() {
        vec!["/.well-known/oauth-protected-resource".to_string()]
    } else {
        vec![
            format!("/.well-known/oauth-protected-resource/{trimmed}"),
            format!("/{trimmed}/.well-known/oauth-protected-resource"),
            "/.well-known/oauth-protected-resource".to_string(),
        ]
    };

    paths
        .into_iter()
        .map(|path| {
            let mut candidate = upstream.clone();
            candidate.set_query(None);
            candidate.set_fragment(None);
            candidate.set_path(&path);
            candidate
        })
        .collect()
}

pub(super) fn authorization_metadata_candidates(server: &url::Url) -> Vec<url::Url> {
    if server.path().contains("/.well-known/") {
        return vec![server.clone()];
    }

    let issuer_path = server.path().trim_matches('/');
    let paths = if issuer_path.is_empty() {
        vec![
            "/.well-known/oauth-authorization-server".to_string(),
            "/.well-known/openid-configuration".to_string(),
        ]
    } else {
        vec![
            format!("/.well-known/oauth-authorization-server/{issuer_path}"),
            format!("/.well-known/openid-configuration/{issuer_path}"),
            format!("/{issuer_path}/.well-known/openid-configuration"),
        ]
    };

    paths
        .into_iter()
        .map(|path| {
            let mut candidate = server.clone();
            candidate.set_query(None);
            candidate.set_fragment(None);
            candidate.set_path(&path);
            candidate
        })
        .collect()
}

pub(super) fn validate_discovered_issuer(
    metadata: &AuthorizationMetadata,
    selected_server: &str,
) -> Result<(), OauthError> {
    let issuer = metadata.issuer.as_deref().ok_or_else(|| {
        OauthError::IssuerMismatch("authorization metadata is missing required issuer".to_string())
    })?;
    if issuer != selected_server {
        return Err(OauthError::IssuerMismatch(format!(
            "authorization metadata issuer `{issuer}` does not exactly match selected server `{selected_server}`"
        )));
    }
    Ok(())
}

fn resolve_authorization_server_url(
    metadata_url: &url::Url,
    authorization_server: &str,
) -> Result<url::Url, url::ParseError> {
    url::Url::parse(authorization_server).or_else(|_| metadata_url.join(authorization_server))
}

/// Return the normalized origin (scheme + "://" + lowercased host + optional explicit port)
/// of a URL, or `None` if the URL is invalid or has no host.
///
/// This is stricter than a host-only comparison: it rejects URLs that share a host
/// but differ in scheme or port (e.g. http vs https, or :80 vs :8080).
pub(super) fn url_origin(s: &str) -> Option<String> {
    let u = url::Url::parse(s).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    let scheme = u.scheme();
    match u.port() {
        Some(port) => Some(format!("{scheme}://{host}:{port}")),
        None => Some(format!("{scheme}://{host}")),
    }
}

pub(super) fn is_known_split_endpoint_origin(issuer_origin: &str, endpoint_origin: &str) -> bool {
    issuer_origin == "https://accounts.google.com"
        && endpoint_origin == "https://oauth2.googleapis.com"
}

pub(super) fn extract_state_param(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
}

pub(super) fn google_offline_access_url(url: &str) -> Result<String, OauthError> {
    let mut parsed = url::Url::parse(url).map_err(|error| {
        OauthError::Internal(format!("authorization url generated invalid URL: {error}"))
    })?;
    let is_google_authorize = parsed
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("accounts.google.com"));
    if !is_google_authorize {
        return Ok(url.to_string());
    }

    let existing: std::collections::HashSet<String> = parsed
        .query_pairs()
        .map(|(key, _)| key.into_owned())
        .collect();
    {
        let mut query = parsed.query_pairs_mut();
        if !existing.contains("access_type") {
            query.append_pair("access_type", "offline");
        }
        if !existing.contains("prompt") {
            query.append_pair("prompt", "consent");
        }
        if !existing.contains("include_granted_scopes") {
            query.append_pair("include_granted_scopes", "true");
        }
    }
    Ok(parsed.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(
        issuer: &str,
        authorization_endpoint: &str,
        token_endpoint: &str,
    ) -> AuthorizationMetadata {
        serde_json::from_value(serde_json::json!({
            "issuer": issuer,
            "authorization_endpoint": authorization_endpoint,
            "token_endpoint": token_endpoint,
            "code_challenge_methods_supported": ["S256"]
        }))
        .expect("metadata")
    }

    #[test]
    fn generic_selected_issuer_comparison_is_exact_including_trailing_slash() {
        let exact = metadata(
            "https://auth.example/",
            "https://auth.example/authorize",
            "https://auth.example/token",
        );
        assert!(validate_discovered_issuer(&exact, "https://auth.example/").is_ok());
        assert!(validate_discovered_issuer(&exact, "https://auth.example").is_err());
    }

    #[test]
    fn google_shaped_metadata_cannot_replace_a_different_selected_issuer() {
        let google = metadata(
            "https://accounts.google.com",
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
        );
        assert!(validate_discovered_issuer(&google, "http://127.0.0.1:1234/").is_err());
        assert!(validate_discovered_issuer(&google, "https://accounts.google.com").is_ok());
    }
}
