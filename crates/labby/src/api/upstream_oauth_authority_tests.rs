//! Real route regressions for authority loss after credential deletion.
use std::sync::Arc;

use axum::{
    Extension,
    body::{self, Body},
    http::{Request, StatusCode},
};
use labby_auth::{
    at_rest::TokenEncryptionKey,
    sqlite::SqliteStore,
    upstream::{cache::OauthClientCache, encryption::load_key, manager::UpstreamOauthManager},
};
use labby_runtime::gateway_config::{GatewayConfig, UpstreamConfig};
use rmcp::transport::{AuthClient, AuthorizationManager};
use tower::ServiceExt;

use super::*;
use crate::dispatch::gateway::{config_store::test_gateway_manager, manager::GatewayRuntimeHandle};

async fn committed_cleanup_authority_loss(google: bool, initially_denied: bool) {
    #[cfg(target_os = "macos")]
    let directory = tempfile::tempdir_in("/private/tmp").unwrap();
    #[cfg(not(target_os = "macos"))]
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = SqliteStore::open_with_key(
        directory.path().join("auth.db"),
        Some(
            TokenEncryptionKey::from_encoded(
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            )
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    let credential = if google {
        serde_json::json!({"source":"google_provider", "account":"fixture-google-subject"})
    } else {
        serde_json::json!({"source":"dedicated"})
    };
    let config: UpstreamConfig = serde_json::from_value(serde_json::json!({
        "name":"fixture", "enabled":true, "url":"https://fixture.invalid/mcp",
        "oauth": {"mode":"authorization_code_pkce", "credential":credential,
            "registration":{"strategy":"preregistered", "client_id":"fixture-client"}}
    }))
    .unwrap();
    if google {
        store
            .upsert_google_provider_token_bundle(
                labby_auth::types::GoogleProviderCredentialUpdate {
                    subject: "fixture-google-subject".into(),
                    email: None,
                    client_id: "fixture-client".into(),
                    granted_scopes: vec!["openid".into()],
                    access_token: "fixture-access".into(),
                    refresh_token: "fixture-refresh".into(),
                    token_received_at: 1,
                    access_token_expires_at: i64::MAX,
                    issuer: None,
                    refreshed: false,
                    scope_upgraded: false,
                },
            )
            .await
            .unwrap();
    } else {
        store
            .upsert_upstream_oauth_credentials(labby_auth::types::UpstreamOauthCredentialRow {
                upstream_name: "fixture".into(),
                subject: SHARED_GATEWAY_OAUTH_SUBJECT.into(),
                client_id: "fixture-client".into(),
                granted_scopes_json: "[]".into(),
                token_blob: vec![1, 2, 3],
                token_blob_nonce: vec![4, 5, 6],
                token_received_at: 1,
                access_token_expires_at: i64::MAX,
                refresh_token_present: true,
            })
            .await
            .unwrap();
    }
    let key = load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let redirect = "https://fixture.invalid/callback".to_string();
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(
        "fixture".into(),
        UpstreamOauthManager::new(store.clone(), key.clone(), config.clone(), redirect.clone()),
    );
    let cache = OauthClientCache::new(managers.clone());
    drop(rustls::crypto::ring::default_provider().install_default());
    let client = Arc::new(AuthClient::new(
        reqwest::Client::new(),
        AuthorizationManager::new("http://localhost").await.unwrap(),
    ));
    cache
        .publish_prebuilt(&config, SHARED_GATEWAY_OAUTH_SUBJECT, client)
        .unwrap();
    let manager = Arc::new(
        test_gateway_manager(
            directory.path().join("gateway.toml"),
            GatewayRuntimeHandle::default(),
        )
        .with_oauth_resources(store.clone(), key, redirect)
        .with_upstream_oauth_managers(managers)
        .with_oauth_client_cache(cache.clone()),
    );
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            upstream: vec![config],
            ..GatewayConfig::default()
        })
        .await;
    let auth = crate::api::oauth::AuthContext {
        sub: "fixture-admin".into(),
        scopes: vec!["lab:admin".into()],
        actor_key: None,
        issuer: "https://fixture.invalid".into(),
        via_session: true,
        csrf_token: None,
        email: None,
    };
    let identity =
        VerifiedIdentity::local_credential(labby_auth::Authenticator::StaticBearer, &auth.sub)
            .unwrap();
    let runtime = Arc::new(
        crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await,
    );
    runtime
        .bootstrap_owner(
            crate::access::BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
        )
        .await
        .unwrap();
    let access = runtime.store().await.unwrap();
    let state = AppState::new()
        .with_access_runtime(runtime)
        .with_gateway_manager(manager.clone());
    let app = gateway_routes(state.clone())
        .router
        .layer(Extension(auth))
        .layer(Extension(identity))
        .with_state(state);
    if initially_denied {
        access
            .execute_test_statement(
                "UPDATE platform_administrators SET status='revoked', revoked_at=11",
            )
            .await
            .unwrap();
    }
    let cleanup_gate = manager.hold_oauth_status_invalidation_for_test().await;
    let request = if google {
        Request::builder()
            .method("POST")
            .uri("/google/revoke")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"upstream":"fixture","confirm":true}"#))
            .unwrap()
    } else {
        Request::builder()
            .method("POST")
            .uri("/clear?upstream=fixture")
            .body(Body::empty())
            .unwrap()
    };
    let response = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    if initially_denied {
        let response = response.await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let payload: serde_json::Value =
            serde_json::from_slice(&body::to_bytes(response.into_body(), 65536).await.unwrap())
                .unwrap();
        assert_eq!(payload["side_effects"], "none_expected");
        assert!(cache.contains_ready_client("fixture", SHARED_GATEWAY_OAUTH_SUBJECT));
        let retained = store
            .find_upstream_oauth_credentials("fixture", SHARED_GATEWAY_OAUTH_SUBJECT)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.token_blob, vec![1, 2, 3]);
        assert_eq!(retained.token_blob_nonce, vec![4, 5, 6]);
        return;
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let deleted = if google {
                store
                    .find_google_provider_credential("fixture-google-subject")
                    .await
                    .unwrap()
                    .is_none()
            } else {
                store
                    .find_upstream_oauth_credentials("fixture", SHARED_GATEWAY_OAUTH_SUBJECT)
                    .await
                    .unwrap()
                    .is_none()
            };
            if deleted {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("durable deletion must precede the held cleanup boundary");
    access
        .execute_test_statement(
            "UPDATE platform_administrators SET status='revoked', revoked_at=11",
        )
        .await
        .unwrap();
    drop(cleanup_gate);
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), response)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !cache.contains_ready_client("fixture", SHARED_GATEWAY_OAUTH_SUBJECT),
        "cleanup must evict the prepared client before the response"
    );
    let status = response.status();
    let payload: serde_json::Value =
        serde_json::from_slice(&body::to_bytes(response.into_body(), 65536).await.unwrap())
            .unwrap();
    assert_eq!(payload["side_effects"], "possible", "{payload}");
    assert_eq!(payload["kind"], "authority_changed");
    assert_eq!(payload["original_kind"], "forbidden");
    assert_eq!(payload["origin"], "policy");
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(payload["service"], "gateway");
    assert_eq!(
        payload["action"],
        if google {
            "gateway.oauth.google_revoke"
        } else {
            "gateway.oauth.clear"
        }
    );
    assert_eq!(payload["recovery"]["action"], "inspect_and_escalate");
    assert!(
        payload["recovery"]["guidance"]
            .as_str()
            .unwrap()
            .contains("Inspect")
    );
    assert!(payload.get("invalidated").is_none());
    assert!(payload.get("revoked_refresh_tokens").is_none());
    assert!(!payload.to_string().contains("fixture-google-subject"));
}

#[tokio::test]
async fn clear_reports_possible_effects_after_committed_cleanup_and_authority_loss() {
    committed_cleanup_authority_loss(false, false).await;
}

#[tokio::test]
async fn google_revoke_reports_possible_effects_after_committed_cleanup_and_authority_loss() {
    committed_cleanup_authority_loss(true, false).await;
}

#[tokio::test]
async fn initially_denied_clear_retains_credentials_with_no_expected_effects() {
    committed_cleanup_authority_loss(false, true).await;
}

// Gates are registered for one owned manager/action and removed before waiting.
// Dropping the release sender always releases a request, including test failure.
type EffectGate = (
    tokio::sync::oneshot::Sender<Option<String>>,
    tokio::sync::oneshot::Receiver<()>,
);
type EffectGates = std::sync::Mutex<std::collections::HashMap<(usize, &'static str), EffectGate>>;
fn effect_gates() -> &'static EffectGates {
    static GATES: std::sync::OnceLock<EffectGates> = std::sync::OnceLock::new();
    GATES.get_or_init(std::sync::Mutex::default)
}
pub(super) async fn after_operation(
    manager: &Arc<crate::dispatch::gateway::manager::GatewayManager>,
    action: &'static str,
    authorization_url: Option<String>,
) {
    let gate = effect_gates()
        .lock()
        .unwrap()
        .remove(&(Arc::as_ptr(manager) as usize, action));
    if let Some((started, release)) = gate {
        drop(started.send(authorization_url));
        drop(release.await);
    }
}

async fn effectful_operation_authority_loss(action: &'static str, initially_denied: bool) {
    use labby_auth::upstream::store::SqliteCredentialStore;
    use oauth2::{AccessToken, RefreshToken, TokenResponse as _, basic::BasicTokenType};
    use rmcp::transport::auth::{
        CredentialStore, OAuthTokenResponse, StoredCredentials, VendorExtraTokenFields,
    };
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    #[cfg(target_os = "macos")]
    let directory = tempfile::tempdir_in("/private/tmp").unwrap();
    #[cfg(not(target_os = "macos"))]
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    drop(rustls::crypto::ring::default_provider().install_default());
    let store = SqliteStore::open_with_key(
        directory.path().join("auth.db"),
        Some(
            TokenEncryptionKey::from_encoded(
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            )
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    let provider = MockServer::start().await;
    let metadata = serde_json::json!({
        "issuer": format!("{}/mcp", provider.uri()),
        "authorization_endpoint": format!("{}/authorize", provider.uri()),
        "token_endpoint": format!("{}/token", provider.uri()),
        "registration_endpoint": format!("{}/register", provider.uri()),
        "code_challenge_methods_supported":["S256"]
    });
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&metadata))
        .mount(&provider)
        .await;
    Mock::given(method("POST")).and(path("/register")).respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"client_id":"registered-fixture-client", "redirect_uris":["https://fixture.invalid/callback"]}))).mount(&provider).await;
    Mock::given(method("POST")).and(path("/token")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"access_token":"rotated-fixture-access", "refresh_token":"rotated-fixture-refresh", "token_type":"Bearer", "expires_in":3600}))).mount(&provider).await;
    let name = "effect-fixture";
    let config: UpstreamConfig = serde_json::from_value(serde_json::json!({
        "name":name,"enabled":true,"url":format!("{}/mcp",provider.uri()),
        "oauth":{"mode":"authorization_code_pkce", "registration": if action == "start" {serde_json::json!({"strategy":"dynamic"})} else {serde_json::json!({"strategy":"preregistered","client_id":"fixture-client"})}}
    })).unwrap();
    let key = load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let oauth = UpstreamOauthManager::new(
        store.clone(),
        key.clone(),
        config.clone(),
        "https://fixture.invalid/callback".into(),
    );
    let credential_store = SqliteCredentialStore::new(
        store.clone(),
        key.clone(),
        name,
        SHARED_GATEWAY_OAUTH_SUBJECT,
    );
    if action == "status" {
        let mut token = OAuthTokenResponse::new(
            AccessToken::new("expired-fixture-access".into()),
            BasicTokenType::Bearer,
            VendorExtraTokenFields::default(),
        );
        token.set_refresh_token(Some(RefreshToken::new("expired-fixture-refresh".into())));
        token.set_expires_in(Some(&std::time::Duration::from_secs(1)));
        credential_store
            .save(StoredCredentials::new(
                "fixture-client".into(),
                Some(token),
                Vec::new(),
                Some(1),
            ))
            .await
            .unwrap();
    }
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(name.into(), oauth.clone());
    let manager = Arc::new(
        test_gateway_manager(
            directory.path().join("gateway.toml"),
            GatewayRuntimeHandle::default(),
        )
        .with_oauth_resources(
            store.clone(),
            key,
            "https://fixture.invalid/callback".into(),
        )
        .with_upstream_oauth_managers(managers.clone()),
    );
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            upstream: vec![config],
            ..Default::default()
        })
        .await;
    let probe_url = format!(
        "https://{}.fixture.invalid/mcp",
        directory
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_start_matches('.')
            .to_ascii_lowercase()
    );
    let _metadata_guard = manager
        .inject_probe_metadata_for_test(&probe_url, serde_json::from_value(metadata).unwrap());
    let auth = crate::api::oauth::AuthContext {
        sub: "effect-admin".into(),
        scopes: vec!["lab:admin".into()],
        actor_key: None,
        issuer: "https://fixture.invalid".into(),
        via_session: true,
        csrf_token: None,
        email: None,
    };
    let identity =
        VerifiedIdentity::local_credential(labby_auth::Authenticator::StaticBearer, &auth.sub)
            .unwrap();
    let runtime = Arc::new(
        crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await,
    );
    runtime
        .bootstrap_owner(
            crate::access::BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
        )
        .await
        .unwrap();
    let access = runtime.store().await.unwrap();
    let state = AppState::new()
        .with_access_runtime(runtime)
        .with_gateway_manager(manager.clone());
    let app = gateway_routes(state.clone())
        .router
        .layer(Extension(auth))
        .layer(Extension(identity))
        .with_state(state);
    let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
    let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
    let gate_key = (Arc::as_ptr(&manager) as usize, action);
    if initially_denied {
        access
            .execute_test_statement(
                "UPDATE platform_administrators SET status='revoked', revoked_at=11",
            )
            .await
            .unwrap();
    } else {
        assert!(
            effect_gates()
                .lock()
                .unwrap()
                .insert(gate_key, (started_sender, release_receiver))
                .is_none()
        );
    }
    let request = match action {
        "probe" => Request::builder()
            .method("POST")
            .uri("/probe")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({"upstream":"transient-fixture", "url":probe_url,"confirm":true})
                    .to_string(),
            ))
            .unwrap(),
        "start" => Request::builder()
            .method("POST")
            .uri("/start")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::json!({"upstream":name}).to_string()))
            .unwrap(),
        "status" => Request::builder()
            .uri(format!("/status?upstream={name}"))
            .body(Body::empty())
            .unwrap(),
        _ => unreachable!(),
    };
    let response = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    if !initially_denied {
        let url = tokio::time::timeout(std::time::Duration::from_secs(20), started_receiver)
            .await
            .expect("real operation reaches response gate")
            .unwrap();
        match action {
            "start" => {
                assert_eq!(
                    oauth
                        .stored_dynamic_client_id(SHARED_GATEWAY_OAUTH_SUBJECT)
                        .await
                        .unwrap()
                        .as_deref(),
                    Some("registered-fixture-client")
                );
                let url = url::Url::parse(url.as_deref().unwrap()).unwrap();
                let csrf = url
                    .query_pairs()
                    .find(|(key, _)| key == "state")
                    .unwrap()
                    .1
                    .into_owned();
                assert_eq!(
                    oauth.subject_for_state(&csrf).await.unwrap().as_deref(),
                    Some(SHARED_GATEWAY_OAUTH_SUBJECT)
                );
            }
            "status" => {
                let credentials = credential_store.load().await.unwrap().unwrap();
                assert_eq!(
                    credentials.token_response.unwrap().access_token().secret(),
                    "rotated-fixture-access"
                );
            }
            "probe" => assert!(
                managers.contains_key("transient-fixture"),
                "real transient manager registration occurred"
            ),
            _ => unreachable!(),
        }
        access
            .execute_test_statement(
                "UPDATE platform_administrators SET status='revoked', revoked_at=11",
            )
            .await
            .unwrap();
    }
    drop(release_sender);
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), response)
        .await
        .unwrap()
        .unwrap();
    let status = response.status();
    let payload: serde_json::Value =
        serde_json::from_slice(&body::to_bytes(response.into_body(), 65536).await.unwrap())
            .unwrap();
    if initially_denied {
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(payload["side_effects"], "none_expected");
        assert!(provider.received_requests().await.unwrap().is_empty());
        assert!(!managers.contains_key("transient-fixture"));
        assert!(
            oauth
                .stored_dynamic_client_id(SHARED_GATEWAY_OAUTH_SUBJECT)
                .await
                .unwrap()
                .is_none()
        );
        if action == "status" {
            assert_eq!(
                credential_store
                    .load()
                    .await
                    .unwrap()
                    .unwrap()
                    .token_response
                    .unwrap()
                    .access_token()
                    .secret(),
                "expired-fixture-access"
            );
        }
    } else {
        assert_eq!(payload["side_effects"], "possible", "{payload}");
        assert_eq!(payload["kind"], "authority_changed");
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(payload["action"], format!("gateway.oauth.{action}"));
        assert_eq!(payload["recovery"]["action"], "inspect_and_escalate");
        for secret in [
            "registered-fixture-client",
            "rotated-fixture-access",
            "rotated-fixture-refresh",
            "authorization_url",
            "transient-fixture",
        ] {
            assert!(
                !payload.to_string().contains(secret),
                "withhold operation details: {payload}"
            );
        }
    }
}

#[tokio::test]
async fn probe_authority_loss_after_transient_registration_reports_possible_effects() {
    effectful_operation_authority_loss("probe", false).await;
}
#[tokio::test]
async fn start_authority_loss_after_provider_registration_and_pending_state_reports_possible_effects()
 {
    effectful_operation_authority_loss("start", false).await;
}
#[tokio::test]
async fn status_authority_loss_after_token_rotation_reports_possible_effects() {
    effectful_operation_authority_loss("status", false).await;
}
#[tokio::test]
async fn initially_denied_effectful_routes_never_contact_provider_or_register_transient_manager() {
    for action in ["probe", "start", "status"] {
        effectful_operation_authority_loss(action, true).await;
    }
}
