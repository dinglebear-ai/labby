use super::super::*;
use super::{MAX_SCOPE_BYTES, MAX_SCOPES, MAX_SUBJECTS};
use rmcp::transport::auth::{CredentialStore, StateStore};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

async fn fixture() -> (UpstreamOauthManager, MockServer, SqliteStore) {
    fixture_with_token_scope(Some("read files:write")).await
}

async fn fixture_with_token_scope(
    token_scope: Option<&str>,
) -> (UpstreamOauthManager, MockServer, SqliteStore) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(401).insert_header(
            "www-authenticate",
            format!("Bearer resource_metadata=\"{}/metadata\"", server.uri()),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/metadata"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "resource": format!("{}/mcp", server.uri()), "authorization_servers": [server.uri()]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET")).and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer":server.uri(), "authorization_endpoint":format!("{}/authorize",server.uri()),
                "token_endpoint":format!("{}/token",server.uri()), "code_challenge_methods_supported":["S256"],
                "scopes_supported":["read", "files:write", "admin"]
            }))).mount(&server).await;
    let mut token_response = serde_json::json!({
        "access_token":"upgraded-access", "token_type":"Bearer", "expires_in":3600,
        "scope":token_scope, "refresh_token":"upgraded-refresh"
    });
    if token_scope.is_none() {
        token_response.as_object_mut().unwrap().remove("scope");
    }
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_response))
        .mount(&server)
        .await;
    let db = SqliteStore::open_with_key(
        tempfile::tempdir().unwrap().keep().join("auth.db"),
        Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
            "scope-tests",
        )),
    )
    .await
    .unwrap();
    let key = crate::upstream::encryption::load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
        .unwrap();
    let config = serde_json::from_value(serde_json::json!({"name":"scoped","url":format!("{}/mcp",server.uri()),
            "oauth":{"mode":"authorization_code_pkce","registration":{"strategy":"preregistered","client_id":"client"},"scopes":["read"]}})).unwrap();
    (
        UpstreamOauthManager::new(db.clone(), key, config, "http://127.0.0.1/callback".into()),
        server,
        db,
    )
}

fn query(url: &str, key: &str) -> String {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

#[tokio::test]
async fn observed_scope_upgrade_uses_subject_bound_persisted_pkce_and_callback() {
    let (manager, _server, db) = fixture().await;
    let existing = SqliteCredentialStore::new(db.clone(), manager.key.clone(), "scoped", "alice");
    CredentialStore::save(&existing, serde_json::from_value(serde_json::json!({
            "client_id":"client", "token_response":{"access_token":"existing-access", "token_type":"Bearer", "expires_in":3600},
            "granted_scopes":["profile"], "token_received_at":1
        })).unwrap()).await.unwrap();
    manager.record_scope_challenge("alice", "files:write");
    let begin = manager.begin_authorization("alice").await.unwrap();
    let scopes = query(&begin.authorization_url, "scope");
    assert!(
        scopes
            .split_whitespace()
            .any(|scope| scope == "files:write")
    );
    assert!(scopes.split_whitespace().any(|scope| scope == "read"));
    assert!(!scopes.split_whitespace().any(|scope| scope == "admin"));
    assert!(
        scopes.split_whitespace().any(|scope| scope == "profile"),
        "preserve permissions from the stored grant"
    );
    assert_eq!(
        query(&begin.authorization_url, "code_challenge_method"),
        "S256"
    );
    let csrf = query(&begin.authorization_url, "state");
    let state = SqliteStateStore::new(db.clone(), "scoped", "alice");
    assert!(
        state.load(&csrf).await.unwrap().is_some(),
        "PKCE state persists through existing state store"
    );
    let wrong = manager
        .complete_authorization_callback("bob", "code", &csrf)
        .await;
    assert!(
        wrong.is_err(),
        "another subject cannot consume the scope upgrade"
    );
    manager
        .complete_authorization_callback("alice", "code", &csrf)
        .await
        .unwrap();
    assert!(manager.required_scope_upgrade("alice").is_empty());
    assert!(
        manager
            .complete_authorization_callback("alice", "code", &csrf)
            .await
            .is_err(),
        "callback replay is rejected"
    );
    let other = manager.begin_authorization("bob").await.unwrap();
    assert_eq!(query(&other.authorization_url, "scope"), "read");
    let stored = SqliteCredentialStore::new(db, manager.key.clone(), "scoped", "alice");
    assert!(CredentialStore::load(&stored).await.unwrap().is_some());
}

#[tokio::test]
async fn observed_scope_upgrade_attempts_remain_bounded_across_fresh_sdk_managers() {
    let (manager, _server, _) = fixture().await;
    manager.record_scope_challenge("alice", "files:write");
    for _ in 0..3 {
        manager.begin_authorization("alice").await.unwrap();
    }
    let error = manager.begin_authorization("alice").await.unwrap_err();
    assert_eq!(error.kind(), "oauth_scope_upgrade_required");
    assert!(manager.begin_authorization("bob").await.is_ok());
    manager.record_scope_challenge("alice", "files:write");
    assert!(
        manager.begin_authorization("alice").await.is_err(),
        "repeated challenges cannot reset the attempt budget"
    );
}
#[test]
fn scope_challenge_parser_accepts_combined_authentication_schemes() {
    for header in [
        r#"Basic realm="gateway", Bearer error="insufficient_scope", scope="write read""#,
        r#"Bearer error="insufficient_scope", scope="write read", Basic realm="gateway""#,
        r#"Bearer error="invalid_token", Bearer error="insufficient_scope", scope="write read""#,
        r#"Negotiate YWJjZA==, Bearer error="insufficient_scope", scope="write read""#,
        r#"Basic, Bearer scope = "write read", error = "insufficient_scope""#,
        r#"Negotiate token, Basic, Bearer scope="write read", error="insufficient_scope""#,
        r#"Basic realm="gateway, Bearer scope=admin", Bearer error="insufficient_scope", scope="write read", error_description="escaped \"quote\", comma""#,
    ] {
        assert_eq!(
            required_scope_from_challenge(header),
            Some("read write".into()),
            "{header}"
        );
    }
}

#[test]
fn scope_challenge_parser_bounds_combined_challenges_and_rejects_injection() {
    for header in [
        r#"Basic error="insufficient_scope", scope="admin", Bearer error="invalid_token""#,
        r#"Basic realm="Bearer error=insufficient_scope, scope=admin", Bearer error="access_denied""#,
        r#"Basic realm="gateway", Bearer error="insufficient_scope", scope="read", scope="admin""#,
        r#"Bearer error="insufficient_scope", scope="read", Basic realm="unterminated"#,
        "Bearer error=\"insufficient_scope\", scope=\"read\"\r\n",
        "Basic realm=\"escaped\\\ncontrol\", Bearer error=\"insufficient_scope\", scope=\"read\"",
    ] {
        assert!(required_scope_from_challenge(header).is_none(), "{header}");
    }
    let excessive = format!(
        "{}Bearer error=\"insufficient_scope\", scope=\"read\"",
        "Basic realm=\"gateway\", ".repeat(16)
    );
    assert!(required_scope_from_challenge(&excessive).is_none());
    let oversized = format!(
        "Basic realm=\"{}\", Bearer error=\"insufficient_scope\", scope=\"read\"",
        "x".repeat(8192)
    );
    assert!(required_scope_from_challenge(&oversized).is_none());
}

#[test]
fn scope_challenge_parser_rejects_ambiguous_and_injected_parameters() {
    assert_eq!(
        required_scope_from_challenge(r#"Bearer scope="write read", error="insufficient_scope""#),
        Some("read write".into())
    );
    for header in [
        r#"Basic error="insufficient_scope", scope="admin""#,
        r#"Bearer error="access_denied", scope="admin""#,
        r#"Bearer realm="x,error=insufficient_scope",scope="admin""#,
        r#"Bearer error="insufficient_scope",scope="read",scope="admin""#,
        r#"Bearer error="insufficient_scope",scope="admin",error="access_denied""#,
        r#"Bearer error="insufficient_scope",scope="""#,
    ] {
        assert!(required_scope_from_challenge(header).is_none(), "{header}");
    }
}

#[tokio::test]
async fn newer_challenge_survives_an_earlier_consents_callback() {
    let (manager, _server, _) = fixture().await;
    manager.record_scope_challenge("alice", "files:write");
    let consent = manager.begin_authorization("alice").await.unwrap();
    manager.record_scope_challenge("alice", "admin");
    manager
        .complete_authorization_callback(
            "alice",
            "code",
            &query(&consent.authorization_url, "state"),
        )
        .await
        .unwrap();
    assert_eq!(manager.required_scope_upgrade("alice"), vec!["admin"]);
}

#[tokio::test]
async fn scope_challenge_registry_bounds_subjects_and_permission_input() {
    let (manager, _server, _) = fixture().await;
    manager.record_scope_challenge("alice", &"x".repeat(MAX_SCOPE_BYTES + 1));
    manager.record_scope_challenge("alice", "read\nadmin");
    let too_many = (0..=MAX_SCOPES)
        .map(|index| format!("s{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    manager.record_scope_challenge("alice", &too_many);
    assert!(manager.required_scope_upgrade("alice").is_empty());
    for index in 0..MAX_SUBJECTS {
        manager.record_scope_challenge(&format!("subject{index}"), "read");
    }
    manager.record_scope_challenge("overflow", "admin");
    assert!(manager.required_scope_upgrade("overflow").is_empty());
}

struct PausedState {
    inner: SqliteStateStore,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl StateStore for PausedState {
    fn save<'life0, 'life1, 'async_trait>(
        &'life0 self,
        csrf: &'life1 str,
        state: rmcp::transport::auth::StoredAuthorizationState,
    ) -> std::pin::Pin<
        Box<
            dyn Future<Output = Result<(), rmcp::transport::auth::AuthError>> + Send + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.started.notify_one();
            self.release.notified().await;
            self.inner.save(csrf, state).await
        })
    }
    fn load<'life0, 'life1, 'async_trait>(
        &'life0 self,
        csrf: &'life1 str,
    ) -> std::pin::Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Option<rmcp::transport::auth::StoredAuthorizationState>,
                        rmcp::transport::auth::AuthError,
                    >,
                > + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { self.inner.load(csrf).await })
    }
    fn delete<'life0, 'life1, 'async_trait>(
        &'life0 self,
        csrf: &'life1 str,
    ) -> std::pin::Pin<
        Box<
            dyn Future<Output = Result<(), rmcp::transport::auth::AuthError>> + Send + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { self.inner.delete(csrf).await })
    }
}

#[tokio::test]
async fn changed_authority_during_pkce_save_cannot_publish_scope_consent() {
    drop(rustls::crypto::ring::default_provider().install_default());
    let (manager, server, db) = fixture().await;
    let cache = crate::upstream::cache::OauthClientCache::new(Arc::new(dashmap::DashMap::new()));
    manager.record_scope_challenge_with_epoch(
        "alice",
        "files:write",
        Some(cache.lifecycle_epoch_for("scoped", "alice")),
    );
    let mut sdk = AuthorizationManager::new(server.uri()).await.unwrap();
    sdk.set_metadata(
        serde_json::from_value(serde_json::json!({
            "authorization_endpoint":format!("{}/authorize",server.uri()),
            "token_endpoint":format!("{}/token",server.uri()),
            "code_challenge_methods_supported":["S256"]
        }))
        .unwrap(),
    );
    sdk.configure_client(OAuthClientConfig::new(
        "client",
        "http://127.0.0.1/callback",
    ))
    .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    sdk.set_state_store(PausedState {
        inner: SqliteStateStore::new(db, "scoped", "alice"),
        started: started.clone(),
        release: release.clone(),
    });
    let flow = tokio::spawn(async move {
        manager
            .authorization_url_with_scope_challenge(&sdk, "alice", &["read"])
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    {
        let _writer = cache.invalidation_barrier().write_owned().await;
        cache.advance_subject_epoch("scoped", "alice");
    }
    release.notify_one();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), flow)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Err(OauthError::NeedsReauth(_))),
        "stale permissions must not produce a consent URL"
    );
}

#[tokio::test]
async fn scope_handoff_survives_status_pruning() {
    let (manager, _server, _) = fixture_with_token_scope(Some("read")).await;
    let cache = crate::upstream::cache::OauthClientCache::new(Arc::new(dashmap::DashMap::new()));
    manager.record_scope_challenge_with_epoch(
        "alice",
        "files:write",
        Some(cache.lifecycle_epoch_for("scoped", "alice")),
    );
    let begin = manager.begin_authorization("alice").await.unwrap();
    let csrf = query(&begin.authorization_url, "state");
    manager
        .complete_authorization_callback("alice", "code", &csrf)
        .await
        .unwrap();
    {
        let _writer = cache.invalidation_barrier().write_owned().await;
        let transfer = manager
            .scope_challenge_transfer_for_callback("alice", &csrf)
            .expect("the partial grant retains this callback's remaining challenge");
        cache.advance_subject_epoch("scoped", "alice");
        assert!(manager.required_scope_upgrade("alice").is_empty());
        manager.rebind_scope_challenge_after_callback(
            transfer,
            cache.lifecycle_epoch_for("scoped", "alice"),
        );
    }
    assert_eq!(manager.required_scope_upgrade("alice"), vec!["files:write"]);
    for _ in 0..2 {
        manager.begin_authorization("alice").await.unwrap();
    }
    assert_eq!(
        manager
            .begin_authorization("alice")
            .await
            .unwrap_err()
            .kind(),
        "oauth_scope_upgrade_required",
        "status pruning must not reset the existing consent attempt budget"
    );
}

#[tokio::test]
async fn narrowed_callback_grant_preserves_required_scope_and_attempt_budget() {
    let (manager, _server, _) = fixture_with_token_scope(Some("read")).await;
    manager.record_scope_challenge("alice", "files:write");
    for _ in 0..3 {
        let begin = manager.begin_authorization("alice").await.unwrap();
        manager
            .complete_authorization_callback(
                "alice",
                "code",
                &query(&begin.authorization_url, "state"),
            )
            .await
            .unwrap();
        let row = manager.credential_row("alice").await.unwrap().unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&row.granted_scopes_json).unwrap(),
            vec!["read"]
        );
        assert_eq!(
            manager.required_scope_upgrade("alice"),
            vec!["files:write"],
            "a successful token exchange is not proof the requested permission was granted"
        );
    }
    assert_eq!(
        manager
            .begin_authorization("alice")
            .await
            .unwrap_err()
            .kind(),
        "oauth_scope_upgrade_required",
        "partial grants cannot reset the three-attempt budget"
    );
}

#[tokio::test]
async fn omitted_token_scope_uses_the_requested_grant_for_scope_completion() {
    let (manager, _server, _) = fixture_with_token_scope(None).await;
    manager.record_scope_challenge("alice", "files:write");
    let begin = manager.begin_authorization("alice").await.unwrap();
    manager
        .complete_authorization_callback("alice", "code", &query(&begin.authorization_url, "state"))
        .await
        .unwrap();
    assert!(manager.required_scope_upgrade("alice").is_empty());
    let row = manager.credential_row("alice").await.unwrap().unwrap();
    let granted = serde_json::from_str::<Vec<String>>(&row.granted_scopes_json).unwrap();
    assert!(
        granted.contains(&"files:write".to_string()),
        "RFC 6749 scope omission preserves requested permissions"
    );
}
