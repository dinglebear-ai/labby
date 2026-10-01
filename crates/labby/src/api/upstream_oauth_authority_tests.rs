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
