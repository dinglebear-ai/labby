//! Real rmcp token exchanges against a local, barrier-controlled HTTP provider.

use std::sync::Arc;
use std::time::Duration;

use oauth2::{AccessToken, RefreshToken, TokenResponse as _, basic::BasicTokenType};
use rmcp_client::transport::AuthorizationManager;
use rmcp_client::transport::auth::{
    AuthError, AuthorizationMetadata, CredentialStore, OAuthClientConfig, OAuthTokenResponse,
    StoredCredentials, VendorExtraTokenFields,
};
use tokio::sync::Notify;

use super::encryption::load_key;
use super::google_store::GoogleProviderCredentialStore;
use super::store::SqliteCredentialStore;
use crate::sqlite::SqliteStore;

fn credentials(access: &str) -> StoredCredentials {
    let mut token = OAuthTokenResponse::new(
        AccessToken::new(access.into()),
        BasicTokenType::Bearer,
        VendorExtraTokenFields::default(),
    );
    token.set_refresh_token(Some(RefreshToken::new("old-refresh".into())));
    token.set_expires_in(Some(&Duration::from_secs(1)));
    StoredCredentials::new("client".into(), Some(token), vec!["openid".into()], Some(1))
}

async fn database() -> SqliteStore {
    SqliteStore::open_with_key(
        tempfile::tempdir().unwrap().keep().join("auth.db"),
        Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
            "refresh-tests",
        )),
    )
    .await
    .unwrap()
}

fn ordinary(store: &SqliteStore) -> SqliteCredentialStore {
    SqliteCredentialStore::new(
        store.clone(),
        load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap(),
        "upstream",
        "subject",
    )
}

async fn fixture() -> (
    String,
    Arc<Notify>,
    Arc<Notify>,
    tokio::task::JoinHandle<()>,
) {
    fixture_with_rejection(false).await
}

async fn fixture_with_rejection(
    rejected: bool,
) -> (
    String,
    Arc<Notify>,
    Arc<Notify>,
    tokio::task::JoinHandle<()>,
) {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let start = Arc::clone(&started);
    let resume = Arc::clone(&release);
    let app = axum::Router::new().route("/token", axum::routing::post(move || {
        let start = Arc::clone(&start);
        let resume = Arc::clone(&resume);
        async move {
            start.notify_one();
            resume.notified().await;
            if rejected {
                return (axum::http::StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error":"invalid_grant"})));
            }
            (axum::http::StatusCode::OK, axum::Json(serde_json::json!({"access_token":"refreshed-access","refresh_token":"rotated-refresh","token_type":"Bearer","expires_in":3600,"scope":"openid"})))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let uri = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (uri, started, release, task)
}

async fn manager(uri: &str, store: impl CredentialStore + 'static) -> AuthorizationManager {
    // The workspace intentionally disables rustls' implicit provider selection.
    // Install the pinned ring provider before rmcp builds its reqwest client.
    drop(rustls::crypto::ring::default_provider().install_default());
    let mut manager = AuthorizationManager::new(uri).await.unwrap();
    let mut metadata = AuthorizationMetadata::default();
    metadata.authorization_endpoint = format!("{uri}/authorize");
    metadata.token_endpoint = format!("{uri}/token");
    manager.set_metadata(metadata);
    manager.set_credential_store(store);
    manager
        .configure_client(OAuthClientConfig::new(
            "client",
            "http://127.0.0.1/callback",
        ))
        .unwrap();
    assert!(manager.initialize_from_store().await.unwrap());
    manager
}

async fn refresh_racing_identity_change(replace: bool) {
    let db = database().await;
    ordinary(&db)
        .save(credentials("initial-access"))
        .await
        .unwrap();
    let (uri, started, release, server) = fixture().await;
    let manager = manager(&uri, ordinary(&db)).await;
    let refresh = tokio::spawn(async move { manager.refresh_token().await });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    db.clear_upstream_oauth_identity("upstream", "subject")
        .await
        .unwrap();
    if replace {
        // Reinstall the same plaintext: randomized encryption still represents
        // new authority, so an old refresh cannot pass the durable CAS.
        ordinary(&db)
            .save(credentials("initial-access"))
            .await
            .unwrap();
    }
    release.notify_one();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), refresh)
            .await
            .unwrap()
            .unwrap(),
        Err(AuthError::AuthorizationRequired)
    ));
    let current = ordinary(&db).load().await.unwrap();
    if replace {
        assert_eq!(
            current
                .unwrap()
                .token_response
                .unwrap()
                .access_token()
                .secret(),
            "initial-access"
        );
    } else {
        assert!(
            current.is_none(),
            "a late refresh must not resurrect a cleared identity"
        );
    }
    server.abort();
}

#[tokio::test]
async fn ordinary_refresh_cannot_recreate_cleared_identity() {
    refresh_racing_identity_change(false).await;
}

#[tokio::test]
async fn ordinary_refresh_cannot_overwrite_reauthorized_identity_with_identical_plaintext() {
    refresh_racing_identity_change(true).await;
}

#[tokio::test]
async fn existing_ordinary_credential_refreshes_without_schema_migration() {
    let db = database().await;
    ordinary(&db)
        .save(credentials("initial-access"))
        .await
        .unwrap();
    let (uri, started, release, server) = fixture().await;
    let manager = manager(&uri, ordinary(&db)).await;
    let refresh = tokio::spawn(async move { manager.refresh_token().await });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), refresh)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        ordinary(&db)
            .load()
            .await
            .unwrap()
            .unwrap()
            .token_response
            .unwrap()
            .access_token()
            .secret(),
        "refreshed-access"
    );
    server.abort();
}

#[tokio::test]
async fn google_refresh_operation_guard_is_distinct_from_persistence_lock() {
    let db = database().await;
    db.upsert_google_provider_token_bundle(crate::types::GoogleProviderCredentialUpdate {
        subject: "account".into(),
        email: Some("operator@example.com".into()),
        client_id: "client".into(),
        granted_scopes: vec!["openid".into()],
        access_token: "initial-access".into(),
        refresh_token: "old-refresh".into(),
        token_received_at: 1,
        access_token_expires_at: 2,
        issuer: Some("https://accounts.google.com".into()),
        refreshed: false,
        scope_upgraded: true,
    })
    .await
    .unwrap();
    let provider = crate::google::GoogleProvider::new(
        "client".into(),
        "secret".into(),
        url::Url::parse("http://127.0.0.1/callback").unwrap(),
    )
    .unwrap();
    let store = GoogleProviderCredentialStore::new(
        db.clone(),
        Arc::new(provider),
        Some("account".into()),
        "client".into(),
        vec!["openid".into()],
    );
    let (uri, started, release, server) = fixture().await;
    let manager = manager(&uri, store).await;
    let refresh = tokio::spawn(async move { manager.refresh_token().await });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();

    let persistence_guard = crate::google_refresh::lock("account")
        .try_lock_owned()
        .expect("rmcp's refresh-operation guard must not reuse the persistence mutex");
    drop(persistence_guard);

    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), refresh)
        .await
        .expect("refresh must persist after the provider exchange completes")
        .unwrap()
        .unwrap();
    assert_eq!(
        db.find_google_provider_credential("account")
            .await
            .unwrap()
            .unwrap()
            .access_token
            .as_deref(),
        Some("refreshed-access")
    );
    assert!(
        crate::google_refresh::lock("account")
            .try_lock_owned()
            .is_ok()
    );
    server.abort();
}

async fn rejected_refresh(replace: bool) {
    let db = database().await;
    ordinary(&db)
        .save(credentials("initial-access"))
        .await
        .unwrap();
    let other = SqliteCredentialStore::new(
        db.clone(),
        load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap(),
        "upstream",
        "other",
    );
    other.save(credentials("other-access")).await.unwrap();
    let (uri, started, release, server) = fixture_with_rejection(true).await;
    let notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let recorded = Arc::clone(&notifications);
    let store = ordinary(&db).with_refresh_rejection_handler(Arc::new(move |provider_subject| {
        assert!(provider_subject.is_none());
        recorded.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    let manager = Arc::new(manager(&uri, store).await);
    let refreshing = Arc::clone(&manager);
    let refresh = tokio::spawn(async move { refreshing.refresh_token().await });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    if replace {
        ordinary(&db).save(credentials("new-access")).await.unwrap();
    }
    release.notify_one();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), refresh)
            .await
            .unwrap()
            .unwrap(),
        Err(AuthError::TokenRefreshRejected(_))
    ));
    let current = ordinary(&db).load().await.unwrap();
    if replace {
        assert_eq!(
            current
                .unwrap()
                .token_response
                .unwrap()
                .access_token()
                .secret(),
            "new-access"
        );
    } else {
        assert!(
            current.is_none(),
            "the rejected generation must not be retried"
        );
        assert!(
            matches!(
                tokio::time::timeout(Duration::from_secs(5), manager.refresh_token())
                    .await
                    .unwrap(),
                Err(AuthError::AuthorizationRequired)
            ),
            "a retained manager must require sign-in before another provider request"
        );
    }
    assert!(
        other.load().await.unwrap().is_some(),
        "another subject must retain its credential"
    );
    assert_eq!(
        notifications.load(std::sync::atomic::Ordering::SeqCst),
        usize::from(!replace),
        "only successful deletion can invalidate runtime sessions"
    );
    server.abort();
}

#[tokio::test]
async fn rejected_refresh_removes_only_its_subject_credential() {
    rejected_refresh(false).await;
}

#[tokio::test]
async fn rejected_refresh_preserves_newer_authorization() {
    rejected_refresh(true).await;
}

async fn initial_scopes(
    configured: Option<Vec<String>>,
    resource_scopes: Option<Vec<&str>>,
    challenge_scope: Option<&str>,
    expected: &str,
) {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(401).insert_header(
            "www-authenticate",
            format!(
                "Bearer resource_metadata=\"{}/metadata\"{}", server.uri(),
                challenge_scope.map(|scope| format!(", scope=\"{scope}\"" )).unwrap_or_default()
            ),
        ))
        .mount(&server)
        .await;
    let mut resource = serde_json::json!({"resource":format!("{}/mcp",server.uri()),"authorization_servers":[server.uri()]});
    if let Some(scopes) = resource_scopes {
        resource["scopes_supported"] = serde_json::json!(scopes);
    }
    Mock::given(method("GET"))
        .and(path("/metadata"))
        .respond_with(ResponseTemplate::new(200).set_body_json(resource))
        .mount(&server)
        .await;
    Mock::given(method("GET")).and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer":server.uri(), "authorization_endpoint":format!("{}/authorize",server.uri()),
            "token_endpoint":format!("{}/token",server.uri()), "code_challenge_methods_supported":["S256"],
            "scopes_supported":["admin","offline_access"]
        }))).mount(&server).await;
    let upstream = serde_json::from_value(serde_json::json!({"name":"scope-tests","url":format!("{}/mcp",server.uri()),"oauth":{
        "mode":"authorization_code_pkce","registration":{"strategy":"preregistered","client_id":"client"},"scopes":configured
    }})).unwrap();
    let manager = super::manager::UpstreamOauthManager::new(
        database().await,
        load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap(),
        upstream,
        "http://127.0.0.1/callback".into(),
    );
    for subject in ["alice", "bob"] {
        let result = manager.begin_authorization(subject).await.unwrap();
        let url = url::Url::parse(&result.authorization_url).unwrap();
        let actual = url
            .query_pairs()
            .find(|(key, _)| key == "scope")
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default();
        assert_eq!(
            actual, expected,
            "scope policy must survive the metadata cache for {subject}"
        );
    }
}

#[tokio::test]
async fn initial_scopes_use_resource_metadata_on_first_and_cached_login() {
    initial_scopes(None, Some(vec!["read"]), None, "read offline_access").await;
}

#[tokio::test]
async fn initial_scopes_preserve_configured_override() {
    initial_scopes(
        Some(vec!["configured".into()]),
        Some(vec!["read"]),
        None,
        "configured",
    )
    .await;
}

#[tokio::test]
async fn initial_scopes_do_not_request_authorization_server_admin_scope() {
    initial_scopes(None, None, None, "").await;
}

#[tokio::test]
async fn initial_scopes_prefer_the_resource_challenge() {
    initial_scopes(
        None,
        Some(vec!["read", "write"]),
        Some("read"),
        "read offline_access",
    )
    .await;
}

#[tokio::test]
async fn initial_scopes_select_defaults_for_an_empty_configuration() {
    initial_scopes(
        Some(vec![]),
        Some(vec!["read"]),
        None,
        "read offline_access",
    )
    .await;
}
