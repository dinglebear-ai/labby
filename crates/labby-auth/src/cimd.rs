use serde::Deserialize;

use crate::error::AuthError;
use crate::state::AuthState;
use crate::types::RegisteredClient;
use crate::util::now_unix;

const DEFAULT_CACHE_SECS: i64 = 300;
const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Deserialize)]
struct ClientMetadataDocument {
    client_id: String,
    client_name: String,
    redirect_uris: Vec<String>,
}

pub async fn resolve_client(
    state: &AuthState,
    client_id: &str,
) -> Result<Option<RegisteredClient>, AuthError> {
    if let Some(client) = state.store.find_client(client_id).await? {
        return Ok(Some(client));
    }
    let url = match labby_primitives::ssrf::parse_validated_https_url(client_id) {
        Ok(url) if url.path() != "/" && !url.path().is_empty() => url,
        _ => return Ok(None),
    };
    let now = now_unix();
    if let Some(entry) = state.cimd_cache.get(client_id)
        && entry.value().1 > now
    {
        return Ok(Some(entry.value().0.clone()));
    }
    let response = crate::remote::secure_get(&url).await?;
    if !response.status().is_success() {
        return Err(AuthError::InvalidGrant(format!(
            "client metadata document returned HTTP {}",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|len| len as usize > MAX_DOCUMENT_BYTES)
    {
        return Err(AuthError::Validation(
            "client metadata document exceeds 1 MiB".to_string(),
        ));
    }
    let cache_secs = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .and_then(cache_max_age)
        .unwrap_or(DEFAULT_CACHE_SECS);
    let bytes = response
        .bytes()
        .await
        .map_err(|error| AuthError::Network(format!("read client metadata document: {error}")))?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(AuthError::Validation(
            "client metadata document exceeds 1 MiB".to_string(),
        ));
    }
    let document: ClientMetadataDocument = serde_json::from_slice(&bytes)
        .map_err(|error| AuthError::Validation(format!("invalid client metadata JSON: {error}")))?;
    let client = validate_document(client_id, document)?;
    state
        .cimd_cache
        .insert(client_id.to_string(), (client.clone(), now + cache_secs));
    Ok(Some(client))
}

fn validate_document(
    expected_client_id: &str,
    document: ClientMetadataDocument,
) -> Result<RegisteredClient, AuthError> {
    if document.client_id != expected_client_id {
        return Err(AuthError::InvalidGrant(
            "client metadata client_id does not match document URL".to_string(),
        ));
    }
    if document.client_name.trim().is_empty() || document.redirect_uris.is_empty() {
        return Err(AuthError::Validation(
            "client metadata requires client_name and redirect_uris".to_string(),
        ));
    }
    if document
        .redirect_uris
        .iter()
        .any(|uri| url::Url::parse(uri).is_err())
    {
        return Err(AuthError::Validation(
            "client metadata contains an invalid redirect URI".to_string(),
        ));
    }
    Ok(RegisteredClient {
        client_id: document.client_id,
        redirect_uris: document.redirect_uris,
        created_at: now_unix(),
    })
}

fn cache_max_age(value: &str) -> Option<i64> {
    value.split(',').find_map(|directive| {
        directive
            .trim()
            .strip_prefix("max-age=")?
            .parse::<i64>()
            .ok()
            .filter(|seconds| *seconds >= 0)
    })
}

#[cfg(test)]
mod tests {
    use super::{ClientMetadataDocument, cache_max_age, validate_document};

    #[test]
    fn rejects_document_whose_client_id_does_not_exactly_match_url() {
        let error = validate_document(
            "https://client.example/client.json",
            ClientMetadataDocument {
                client_id: "https://attacker.example/client.json".to_string(),
                client_name: "Client".to_string(),
                redirect_uris: vec!["http://127.0.0.1:3000/callback".to_string()],
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not match"));
    }

    #[test]
    fn parses_cache_control_max_age() {
        assert_eq!(cache_max_age("public, max-age=600"), Some(600));
    }
}
