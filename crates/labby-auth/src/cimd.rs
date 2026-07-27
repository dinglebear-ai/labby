use serde::Deserialize;

use crate::error::AuthError;
use crate::state::AuthState;
use crate::types::RegisteredClient;
use crate::util::now_unix;

const MAX_CACHE_ENTRIES: usize = 1_024;

#[derive(Debug, Deserialize)]
struct ClientMetadataDocument {
    client_id: String,
    client_name: String,
    redirect_uris: Vec<String>,
    #[serde(default = "default_token_endpoint_auth_method")]
    token_endpoint_auth_method: String,
    #[serde(default)]
    jwks: Option<serde_json::Value>,
}

fn default_token_endpoint_auth_method() -> String {
    "none".to_string()
}

pub async fn resolve_client(
    state: &AuthState,
    client_id: &str,
) -> Result<Option<RegisteredClient>, AuthError> {
    let Some(url) = metadata_document_url(client_id) else {
        return state.store.find_client(client_id).await;
    };
    let now = now_unix();
    let lock_key = format!("cimd:{client_id}");
    let fetch_lock = state
        .remote_fetch_locks
        .entry(lock_key.clone())
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = fetch_lock.lock().await;
    if let Some(entry) = state.cimd_cache.get(client_id)
        && entry.value().1 > now
    {
        return Ok(Some(entry.value().0.clone()));
    }
    if let Some(entry) = state.cimd_cache.get(client_id)
        && entry.value().1 > now_unix()
    {
        let client = entry.value().0.clone();
        drop(entry);
        drop(_guard);
        state.remote_fetch_locks.remove(&lock_key);
        return Ok(Some(client));
    }
    let _permit = state
        .remote_fetch_permits
        .acquire()
        .await
        .map_err(|_| AuthError::Server("remote metadata fetch limiter closed".to_string()))?;
    let (document, cache_policy) =
        match crate::remote::fetch_json::<ClientMetadataDocument>(&url, "client metadata document")
            .await
        {
            Ok(value) => value,
            Err(error) => {
                drop(_guard);
                state.remote_fetch_locks.remove(&lock_key);
                return Err(error);
            }
        };
    let client = validate_document(
        client_id,
        document,
        &state.config.allowed_client_redirect_uris,
    )?;
    if cache_policy.cacheable {
        state
            .cimd_cache
            .retain(|_, (_, expires_at)| *expires_at > now_unix());
        if state.cimd_cache.len() >= MAX_CACHE_ENTRIES
            && let Some(oldest) = state
                .cimd_cache
                .iter()
                .min_by_key(|entry| entry.value().1)
                .map(|entry| entry.key().clone())
        {
            state.cimd_cache.remove(&oldest);
        }
        state.cimd_cache.insert(
            client_id.to_string(),
            (
                client.clone(),
                now_unix().saturating_add(cache_policy.max_age_secs),
            ),
        );
    }
    drop(_guard);
    state.remote_fetch_locks.remove(&lock_key);
    Ok(Some(client))
}

/// Whether a client identifier names a Client ID Metadata Document (CIMD).
///
/// URL-based clients must always be resolved from their metadata document,
/// rather than the local `registered_clients` reference row.  That row exists
/// only to satisfy the refresh-token foreign key; it deliberately does not
/// replace the document's authentication method or JWK set.
pub fn is_metadata_document_client_id(client_id: &str) -> bool {
    metadata_document_url(client_id).is_some()
}

fn metadata_document_url(client_id: &str) -> Option<url::Url> {
    match labby_primitives::ssrf::parse_validated_https_url(client_id) {
        Ok(url) if url.path() != "/" && !url.path().is_empty() => Some(url),
        _ => None,
    }
}

fn validate_document(
    expected_client_id: &str,
    document: ClientMetadataDocument,
    allowed_redirect_patterns: &[String],
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
        .any(|uri| !crate::authorize::is_allowed_redirect_uri(uri, allowed_redirect_patterns))
    {
        return Err(AuthError::Validation(
            "client metadata contains an unsafe redirect URI".to_string(),
        ));
    }
    if !matches!(
        document.token_endpoint_auth_method.as_str(),
        "none" | "private_key_jwt"
    ) {
        return Err(AuthError::Validation(
            "client metadata token_endpoint_auth_method must be none or private_key_jwt"
                .to_string(),
        ));
    }
    if document.token_endpoint_auth_method == "private_key_jwt" && document.jwks.is_none() {
        return Err(AuthError::Validation(
            "private_key_jwt client metadata requires inline jwks".to_string(),
        ));
    }
    Ok(RegisteredClient {
        client_id: document.client_id,
        redirect_uris: document.redirect_uris,
        created_at: now_unix(),
        token_endpoint_auth_method: document.token_endpoint_auth_method,
        jwks: document.jwks,
    })
}

#[cfg(test)]
mod tests {
    use super::{ClientMetadataDocument, resolve_client, validate_document};
    use crate::authorize::tests::test_auth_state;
    use crate::types::RegisteredClient;
    use crate::util::now_unix;

    #[test]
    fn rejects_document_whose_client_id_does_not_exactly_match_url() {
        let error = validate_document(
            "https://client.example/client.json",
            ClientMetadataDocument {
                client_id: "https://attacker.example/client.json".to_string(),
                client_name: "Client".to_string(),
                redirect_uris: vec!["http://127.0.0.1:3000/callback".to_string()],
                token_endpoint_auth_method: "none".to_string(),
                jwks: None,
            },
            &[],
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not match"));
    }

    #[test]
    fn rejects_unsafe_redirect_schemes() {
        for redirect_uri in [
            "javascript:alert(1)",
            "data:text/html,boom",
            "http://example.com/callback",
        ] {
            let error = validate_document(
                "https://client.example/client.json",
                ClientMetadataDocument {
                    client_id: "https://client.example/client.json".to_string(),
                    client_name: "Client".to_string(),
                    redirect_uris: vec![redirect_uri.to_string()],
                    token_endpoint_auth_method: "none".to_string(),
                    jwks: None,
                },
                &[],
            )
            .unwrap_err();
            assert!(error.to_string().contains("unsafe"));
        }
    }

    #[tokio::test]
    async fn cimd_client_resolution_does_not_downgrade_to_its_local_reference() {
        let state = test_auth_state().await;
        let client_id = "https://chatgpt.com/oauth/test-client/client.json";
        // The persisted row exists solely as the refresh-token foreign-key
        // parent. Its older SQLite representation cannot carry the CIMD JWKs.
        state
            .store
            .register_client(RegisteredClient {
                client_id: client_id.to_string(),
                redirect_uris: vec!["https://chatgpt.com/connector/oauth/test-client".to_string()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                jwks: None,
            })
            .await
            .unwrap();
        state.cimd_cache.insert(
            client_id.to_string(),
            (
                RegisteredClient {
                    client_id: client_id.to_string(),
                    redirect_uris: vec![
                        "https://chatgpt.com/connector/oauth/test-client".to_string(),
                    ],
                    created_at: now_unix(),
                    token_endpoint_auth_method: "private_key_jwt".to_string(),
                    jwks: Some(serde_json::json!({"keys": []})),
                },
                now_unix() + 60,
            ),
        );

        let resolved = resolve_client(&state, client_id).await.unwrap().unwrap();
        assert_eq!(resolved.token_endpoint_auth_method, "private_key_jwt");
        assert!(resolved.jwks.is_some());
    }
}
