use serde::Deserialize;
use tracing::warn;

use crate::error::AuthError;
use crate::state::AuthState;
use crate::types::RegisteredClient;
use crate::util::now_unix;

const MAX_CACHE_ENTRIES: usize = 1_024;
const MAX_REMOTE_FETCH_LOCKS: usize = 2_048;
const NEGATIVE_CACHE_SECS: i64 = 30;

#[derive(Debug, Deserialize)]
struct ClientMetadataDocument {
    client_id: String,
    client_name: String,
    redirect_uris: Vec<String>,
    #[serde(default = "default_token_endpoint_auth_method")]
    token_endpoint_auth_method: String,
    /// Every method the client is willing to authenticate with.
    ///
    /// `token_endpoint_auth_method` names the client's *preference*; a client
    /// that can also fall back publishes the full set here. ChatGPT's connector
    /// declares `private_key_jwt` as its preference and
    /// `["none", "private_key_jwt"]` as its set, then authenticates with
    /// `none` — reading only the singular field rejects a client for using a
    /// method it published.
    #[serde(default)]
    token_endpoint_auth_methods_supported: Option<Vec<String>>,
    #[serde(default)]
    jwks: Option<serde_json::Value>,
    #[serde(default)]
    jwks_uri: Option<String>,
}

fn default_token_endpoint_auth_method() -> String {
    "none".to_string()
}

pub async fn resolve_client(
    state: &AuthState,
    client_id: &str,
) -> Result<Option<RegisteredClient>, AuthError> {
    tokio::time::timeout(
        crate::remote::REMOTE_FETCH_DEADLINE,
        resolve_client_within_deadline(state, client_id),
    )
    .await
    .map_err(|_| AuthError::Network("client metadata resolution timed out".to_string()))?
}

async fn resolve_client_within_deadline(
    state: &AuthState,
    client_id: &str,
) -> Result<Option<RegisteredClient>, AuthError> {
    let Some(url) = metadata_document_url(client_id) else {
        return state.store.find_client(client_id).await;
    };
    if let Some(client) = cached_client(state, client_id) {
        return Ok(Some(client));
    }
    if negative_cache_hit(state, client_id) {
        return Err(AuthError::Validation(
            "client metadata document is temporarily unavailable".to_string(),
        ));
    }
    let lock_key = format!("cimd:{client_id}");
    let fetch_lock = acquire_remote_fetch_lock(state, &lock_key)?;
    let _guard = fetch_lock.lock().await;
    // Re-read time and both caches after entering the single-flight section:
    // another waiter may have populated either while this task was parked.
    if let Some(client) = cached_client(state, client_id) {
        return Ok(Some(client));
    }
    if negative_cache_hit(state, client_id) {
        return Err(AuthError::Validation(
            "client metadata document is temporarily unavailable".to_string(),
        ));
    }
    let _permit = tokio::time::timeout(
        crate::remote::REMOTE_FETCH_DEADLINE,
        state.remote_fetch_permits.acquire(),
    )
    .await
    .map_err(|_| AuthError::Network("remote metadata permit timed out".to_string()))?
    .map_err(|_| AuthError::Server("remote metadata fetch limiter closed".to_string()))?;
    let (document, cache_policy) =
        match crate::remote::fetch_json::<ClientMetadataDocument>(&url, "client metadata document")
            .await
        {
            Ok(value) => value,
            Err(error) => {
                record_negative_cache(state, client_id);
                return Err(error);
            }
        };
    let client = match validate_document(
        client_id,
        document,
        &state.config.allowed_client_redirect_uris,
    ) {
        Ok(client) => client,
        Err(error) => {
            record_negative_cache(state, client_id);
            return Err(error);
        }
    };
    if cache_policy.cacheable {
        let _maintenance = state.cimd_cache_maintenance.lock().map_err(|_| {
            AuthError::Server("client metadata cache maintenance poisoned".to_string())
        })?;
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
        state.cimd_negative_cache.remove(client_id);
    }
    Ok(Some(client))
}

fn cached_client(state: &AuthState, client_id: &str) -> Option<RegisteredClient> {
    let now = now_unix();
    state
        .cimd_cache
        .get(client_id)
        .and_then(|entry| (entry.value().1 > now).then(|| entry.value().0.clone()))
}

fn negative_cache_hit(state: &AuthState, client_id: &str) -> bool {
    let now = now_unix();
    state
        .cimd_negative_cache
        .get(client_id)
        .is_some_and(|entry| *entry.value() > now)
}

fn record_negative_cache(state: &AuthState, client_id: &str) {
    let Ok(_maintenance) = state.cimd_cache_maintenance.lock() else {
        warn!(
            kind = "internal_error",
            "client metadata negative cache maintenance lock poisoned"
        );
        return;
    };
    state
        .cimd_negative_cache
        .retain(|_, expires_at| *expires_at > now_unix());
    while state.cimd_negative_cache.len() >= MAX_CACHE_ENTRIES {
        let oldest = state
            .cimd_negative_cache
            .iter()
            .min_by_key(|entry| *entry.value())
            .map(|entry| entry.key().clone());
        let Some(oldest) = oldest else { break };
        state.cimd_negative_cache.remove(&oldest);
    }
    state.cimd_negative_cache.insert(
        client_id.to_string(),
        now_unix().saturating_add(NEGATIVE_CACHE_SECS),
    );
}

pub(crate) fn acquire_remote_fetch_lock(
    state: &AuthState,
    lock_key: &str,
) -> Result<std::sync::Arc<tokio::sync::Mutex<()>>, AuthError> {
    let _maintenance = state
        .remote_fetch_lock_maintenance
        .lock()
        .map_err(|_| AuthError::Server("remote fetch lock registry poisoned".to_string()))?;
    if let Some(existing) = state.remote_fetch_locks.get(lock_key) {
        return Ok(existing.value().clone());
    }
    while state.remote_fetch_locks.len() >= MAX_REMOTE_FETCH_LOCKS {
        let idle = state
            .remote_fetch_locks
            .iter()
            .find(|entry| std::sync::Arc::strong_count(entry.value()) == 1)
            .map(|entry| entry.key().clone());
        let Some(idle) = idle else {
            return Err(AuthError::Server(
                "remote metadata fetch capacity exhausted".to_string(),
            ));
        };
        state.remote_fetch_locks.remove(&idle);
    }
    let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    state
        .remote_fetch_locks
        .insert(lock_key.to_string(), lock.clone());
    Ok(lock)
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
        warn!(
            kind = "validation_failed",
            client_id = %expected_client_id,
            "cimd rejected: document client_id does not match the URL it was fetched from"
        );
        return Err(AuthError::InvalidGrant(
            "client metadata client_id does not match document URL".to_string(),
        ));
    }
    if document.client_name.trim().is_empty() || document.redirect_uris.is_empty() {
        warn!(
            kind = "validation_failed",
            client_id = %expected_client_id,
            has_client_name = !document.client_name.trim().is_empty(),
            redirect_uri_count = document.redirect_uris.len(),
            "cimd rejected: document is missing client_name or redirect_uris"
        );
        return Err(AuthError::Validation(
            "client metadata requires client_name and redirect_uris".to_string(),
        ));
    }
    if document
        .redirect_uris
        .iter()
        .any(|uri| !crate::authorize::is_allowed_redirect_uri(uri, allowed_redirect_patterns))
    {
        warn!(
            kind = "validation_failed",
            client_id = %expected_client_id,
            redirect_uri_count = document.redirect_uris.len(),
            "cimd rejected: document contains a redirect URI outside the allowlist"
        );
        return Err(AuthError::Validation(
            "client metadata contains an unsafe redirect URI".to_string(),
        ));
    }
    // The client may authenticate with its declared preference or with any
    // method it additionally published. Both must be methods we implement.
    let mut accepted_methods = vec![document.token_endpoint_auth_method.clone()];
    for method in document
        .token_endpoint_auth_methods_supported
        .iter()
        .flatten()
    {
        if !accepted_methods.contains(method) {
            accepted_methods.push(method.clone());
        }
    }
    if let Some(unsupported) = accepted_methods
        .iter()
        .find(|method| !matches!(method.as_str(), "none" | "private_key_jwt"))
    {
        warn!(
            kind = "validation_failed",
            client_id = %expected_client_id,
            unsupported_auth_method = %unsupported,
            "cimd rejected: document publishes an unimplemented token_endpoint_auth_method"
        );
        return Err(AuthError::Validation(format!(
            "client metadata token_endpoint_auth_method `{unsupported}` must be none or private_key_jwt"
        )));
    }
    // draft-ietf-oauth-client-id-metadata-document section 8.2 lets a
    // `private_key_jwt` client publish its public keys either inline (`jwks`)
    // or by reference (`jwks_uri`) — the draft's own worked example uses
    // `jwks_uri`, and ChatGPT's connector metadata does too. Inline keys win
    // when both are present so we never make an avoidable outbound fetch.
    let jwks_uri = match document.jwks_uri.as_deref() {
        Some(uri) if document.jwks.is_none() => Some(validate_jwks_uri(uri)?),
        _ => None,
    };
    if accepted_methods.iter().any(|m| m == "private_key_jwt")
        && document.jwks.is_none()
        && jwks_uri.is_none()
    {
        warn!(
            kind = "validation_failed",
            client_id = %expected_client_id,
            "cimd rejected: private_key_jwt document publishes neither jwks nor jwks_uri"
        );
        return Err(AuthError::Validation(
            "private_key_jwt client metadata requires jwks or jwks_uri".to_string(),
        ));
    }
    Ok(RegisteredClient {
        client_id: document.client_id,
        redirect_uris: document.redirect_uris,
        created_at: now_unix(),
        token_endpoint_auth_method: document.token_endpoint_auth_method,
        token_endpoint_auth_methods: accepted_methods,
        jwks: document.jwks,
        jwks_uri,
    })
}

/// Reject a `jwks_uri` we would otherwise fetch on the client's behalf.
///
/// The fetch itself re-validates through [`crate::remote::secure_get`], but
/// failing here keeps an unfetchable or internal-network URL out of the CIMD
/// cache and surfaces the problem at `/authorize` rather than at `/token`.
fn validate_jwks_uri(uri: &str) -> Result<String, AuthError> {
    labby_primitives::ssrf::parse_validated_https_url(uri)
        .map(|url| url.to_string())
        .map_err(|error| {
            warn!(
                kind = "validation_failed",
                error = %error,
                "cimd rejected: jwks_uri failed the SSRF preflight"
            );
            AuthError::Validation(format!("client metadata jwks_uri is not usable: {error}"))
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        ClientMetadataDocument, MAX_CACHE_ENTRIES, MAX_REMOTE_FETCH_LOCKS,
        acquire_remote_fetch_lock, negative_cache_hit, record_negative_cache, resolve_client,
        validate_document,
    };
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
                token_endpoint_auth_methods_supported: None,
                jwks: None,
                jwks_uri: None,
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
                    token_endpoint_auth_methods_supported: None,
                    jwks: None,
                    jwks_uri: None,
                },
                &[],
            )
            .unwrap_err();
            assert!(error.to_string().contains("unsafe"));
        }
    }

    /// Shape of the real ChatGPT connector document, which publishes its keys
    /// by reference. Rejecting this form broke `/authorize` with a 422.
    fn chatgpt_shaped_document(
        jwks: Option<serde_json::Value>,
        jwks_uri: Option<&str>,
    ) -> ClientMetadataDocument {
        ClientMetadataDocument {
            client_id: "https://chatgpt.com/oauth/test-client/client.json".to_string(),
            client_name: "ChatGPT".to_string(),
            redirect_uris: vec!["https://chatgpt.com/connector/oauth/test-client".to_string()],
            token_endpoint_auth_method: "private_key_jwt".to_string(),
            token_endpoint_auth_methods_supported: None,
            jwks,
            jwks_uri: jwks_uri.map(str::to_string),
        }
    }

    fn chatgpt_redirect_patterns() -> Vec<String> {
        vec!["https://chatgpt.com/connector/oauth/*".to_string()]
    }

    #[test]
    fn accepts_private_key_jwt_documents_that_publish_keys_by_reference() {
        let client = validate_document(
            "https://chatgpt.com/oauth/test-client/client.json",
            chatgpt_shaped_document(None, Some("https://chatgpt.com/oauth/jwks.json")),
            &chatgpt_redirect_patterns(),
        )
        .unwrap();
        assert_eq!(client.token_endpoint_auth_method, "private_key_jwt");
        assert!(client.jwks.is_none());
        assert_eq!(
            client.jwks_uri.as_deref(),
            Some("https://chatgpt.com/oauth/jwks.json")
        );
    }

    /// The live document, captured from
    /// `https://chatgpt.com/oauth/<id>/client.json` on 2026-08-07 with only
    /// the connector id neutralised. Kept as a recorded artifact rather than
    /// a hand-written string so the premise of this whole change — that a
    /// real client publishes `token_endpoint_auth_methods_supported` in a
    /// *client* document, which is an AS-metadata field name — stays
    /// evidenced rather than asserted.
    #[test]
    fn the_recorded_chatgpt_document_publishes_both_auth_methods() {
        let raw = include_str!("../tests/fixtures/chatgpt-client-metadata.json");
        let document: ClientMetadataDocument = serde_json::from_str(raw).unwrap();
        let client = validate_document(
            "https://chatgpt.com/oauth/test-client/client.json",
            document,
            &chatgpt_redirect_patterns(),
        )
        .unwrap();
        assert_eq!(
            client.token_endpoint_auth_methods,
            vec!["private_key_jwt".to_string(), "none".to_string()]
        );
        assert_eq!(
            client.jwks_uri.as_deref(),
            Some("https://chatgpt.com/oauth/jwks.json")
        );
    }

    /// Byte-for-byte capture of `https://chatgpt.com/oauth/<id>/client.json`
    /// (2026-08-06), with the connector id replaced. Requiring an inline
    /// `jwks` rejected exactly this document, so `/authorize` answered
    /// `422 validation_failed` before the Google redirect.
    #[test]
    fn accepts_the_real_chatgpt_connector_metadata_document() {
        let raw = r#"{
            "client_id": "https://chatgpt.com/oauth/test-client/client.json",
            "client_uri": "https://chatgpt.com/",
            "redirect_uris": ["https://chatgpt.com/connector/oauth/test-client"],
            "token_endpoint_auth_method": "private_key_jwt",
            "token_endpoint_auth_methods_supported": ["none", "private_key_jwt"],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "client_name": "ChatGPT",
            "logo_uri": "https://persistent.oaistatic.com/sonic/misc/openai-logo.png",
            "token_endpoint_auth_signing_alg": "RS256",
            "jwks_uri": "https://chatgpt.com/oauth/jwks.json"
        }"#;
        let document: ClientMetadataDocument = serde_json::from_str(raw).unwrap();
        let client = validate_document(
            "https://chatgpt.com/oauth/test-client/client.json",
            document,
            &chatgpt_redirect_patterns(),
        )
        .unwrap();
        assert_eq!(
            client.jwks_uri.as_deref(),
            Some("https://chatgpt.com/oauth/jwks.json")
        );
    }

    /// ChatGPT declares `private_key_jwt` as its preference *and* publishes
    /// `["none", "private_key_jwt"]` as the set it supports — then
    /// authenticates with `none`. Reading only the singular field rejected it
    /// with `invalid_client` at `/token`, after `/authorize` had already
    /// succeeded, so the connector failed at the last step with no log line.
    #[test]
    fn honours_every_auth_method_the_client_publishes() {
        let raw = r#"{
            "client_id": "https://chatgpt.com/oauth/test-client/client.json",
            "redirect_uris": ["https://chatgpt.com/connector/oauth/test-client"],
            "token_endpoint_auth_method": "private_key_jwt",
            "token_endpoint_auth_methods_supported": ["none", "private_key_jwt"],
            "client_name": "ChatGPT",
            "jwks_uri": "https://chatgpt.com/oauth/jwks.json"
        }"#;
        let document: ClientMetadataDocument = serde_json::from_str(raw).unwrap();
        let client = validate_document(
            "https://chatgpt.com/oauth/test-client/client.json",
            document,
            &chatgpt_redirect_patterns(),
        )
        .unwrap();
        assert_eq!(client.token_endpoint_auth_method, "private_key_jwt");
        assert_eq!(
            client.token_endpoint_auth_methods,
            vec!["private_key_jwt".to_string(), "none".to_string()],
            "the declared preference comes first, then everything else published"
        );
    }

    #[test]
    fn a_client_publishing_only_its_preference_accepts_only_that() {
        let client = validate_document(
            "https://chatgpt.com/oauth/test-client/client.json",
            chatgpt_shaped_document(None, Some("https://chatgpt.com/oauth/jwks.json")),
            &chatgpt_redirect_patterns(),
        )
        .unwrap();
        assert_eq!(
            client.token_endpoint_auth_methods,
            vec!["private_key_jwt".to_string()],
            "absent `token_endpoint_auth_methods_supported` must not widen anything"
        );
    }

    #[test]
    fn rejects_a_published_auth_method_we_do_not_implement() {
        let raw = r#"{
            "client_id": "https://chatgpt.com/oauth/test-client/client.json",
            "redirect_uris": ["https://chatgpt.com/connector/oauth/test-client"],
            "token_endpoint_auth_method": "none",
            "token_endpoint_auth_methods_supported": ["none", "client_secret_basic"],
            "client_name": "ChatGPT"
        }"#;
        let document: ClientMetadataDocument = serde_json::from_str(raw).unwrap();
        let error = validate_document(
            "https://chatgpt.com/oauth/test-client/client.json",
            document,
            &chatgpt_redirect_patterns(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("client_secret_basic"));
    }

    #[test]
    fn inline_keys_win_over_a_jwks_uri_so_no_fetch_is_needed() {
        let client = validate_document(
            "https://chatgpt.com/oauth/test-client/client.json",
            chatgpt_shaped_document(
                Some(serde_json::json!({"keys": []})),
                Some("https://chatgpt.com/oauth/jwks.json"),
            ),
            &chatgpt_redirect_patterns(),
        )
        .unwrap();
        assert!(client.jwks.is_some());
        assert!(client.jwks_uri.is_none());
    }

    #[test]
    fn rejects_private_key_jwt_documents_that_publish_no_keys_at_all() {
        let error = validate_document(
            "https://chatgpt.com/oauth/test-client/client.json",
            chatgpt_shaped_document(None, None),
            &chatgpt_redirect_patterns(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("requires jwks or jwks_uri"));
    }

    #[test]
    fn rejects_a_jwks_uri_that_would_reach_into_the_private_network() {
        for uri in [
            "https://127.0.0.1/jwks.json",
            "https://10.0.0.5/jwks.json",
            "https://gateway.internal/jwks.json",
            "http://chatgpt.com/oauth/jwks.json",
            "not-a-url",
        ] {
            let error = validate_document(
                "https://chatgpt.com/oauth/test-client/client.json",
                chatgpt_shaped_document(None, Some(uri)),
                &chatgpt_redirect_patterns(),
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("jwks_uri is not usable"),
                "expected `{uri}` to be refused, got: {error}"
            );
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
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
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
                    token_endpoint_auth_methods: Vec::new(),
                    jwks: Some(serde_json::json!({"keys": []})),
                    jwks_uri: None,
                },
                now_unix() + 60,
            ),
        );

        let resolved = resolve_client(&state, client_id).await.unwrap().unwrap();
        assert_eq!(resolved.token_endpoint_auth_method, "private_key_jwt");
        assert!(resolved.jwks.is_some());
    }

    #[tokio::test]
    async fn cancelled_waiter_does_not_split_the_single_flight_generation() {
        let state = test_auth_state().await;
        let key = "cimd:https://client.example/client.json";
        let lock = acquire_remote_fetch_lock(&state, key).unwrap();
        let guard = lock.lock().await;

        let waiter_state = state.clone();
        let waiter = tokio::spawn(async move {
            let waiter_lock = acquire_remote_fetch_lock(&waiter_state, key).unwrap();
            let _guard = waiter_lock.lock().await;
        });
        tokio::task::yield_now().await;
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        drop(guard);

        let next = acquire_remote_fetch_lock(&state, key).unwrap();
        assert!(Arc::ptr_eq(&lock, &next));
        assert_eq!(state.remote_fetch_locks.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn absolute_metadata_deadline_includes_single_flight_wait() {
        let state = test_auth_state().await;
        let client_id = "https://client.example/metadata.json";
        let key = format!("cimd:{client_id}");
        let lock = acquire_remote_fetch_lock(&state, &key).unwrap();
        let _held = lock.lock().await;
        let resolving = tokio::spawn({
            let state = state.clone();
            async move { resolve_client(&state, client_id).await }
        });
        tokio::task::yield_now().await;
        tokio::time::advance(crate::remote::REMOTE_FETCH_DEADLINE).await;
        let error = resolving.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn waiter_observes_cache_filled_while_it_was_waiting() {
        let state = test_auth_state().await;
        let client_id = "https://client.example/client.json";
        let lock_key = format!("cimd:{client_id}");
        let lock = acquire_remote_fetch_lock(&state, &lock_key).unwrap();
        let guard = lock.lock().await;

        let waiter_state = state.clone();
        let waiter = tokio::spawn(async move { resolve_client(&waiter_state, client_id).await });
        tokio::task::yield_now().await;
        state.cimd_cache.insert(
            client_id.to_string(),
            (
                RegisteredClient {
                    client_id: client_id.to_string(),
                    redirect_uris: vec!["http://127.0.0.1:3000/callback".to_string()],
                    created_at: now_unix(),
                    token_endpoint_auth_method: "none".to_string(),
                    token_endpoint_auth_methods: vec!["none".to_string()],
                    jwks: None,
                    jwks_uri: None,
                },
                now_unix() + 60,
            ),
        );
        drop(guard);

        let resolved = waiter.await.unwrap().unwrap().unwrap();
        assert_eq!(resolved.client_id, client_id);
    }

    #[tokio::test]
    async fn attacker_controlled_single_flight_keys_stay_bounded() {
        let state = test_auth_state().await;
        for index in 0..=MAX_REMOTE_FETCH_LOCKS {
            let key = format!("cimd:https://client-{index}.example/client.json");
            drop(acquire_remote_fetch_lock(&state, &key).unwrap());
        }
        assert_eq!(state.remote_fetch_locks.len(), MAX_REMOTE_FETCH_LOCKS);
    }

    #[tokio::test]
    async fn negative_cache_is_short_lived_and_bounded() {
        let state = test_auth_state().await;
        for index in 0..=MAX_CACHE_ENTRIES {
            record_negative_cache(
                &state,
                &format!("https://client-{index}.example/client.json"),
            );
        }
        assert_eq!(state.cimd_negative_cache.len(), MAX_CACHE_ENTRIES);
        assert!(negative_cache_hit(
            &state,
            "https://client-1024.example/client.json"
        ));
    }
}
