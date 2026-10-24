use super::{
    OAUTH_STATUS_DISCOVERY_FAILURE_COOLDOWN, OAUTH_STATUS_DISCOVERY_FRESHNESS,
    OauthStatusDiscoverySnapshot, oauth_status_discovery_is_fresh,
    probe::{probe_manager_key, validate_probe_upstream_name, validate_probe_url},
    should_use_dynamic_registration,
};

fn discovery_snapshot(age: std::time::Duration, failed: bool) -> OauthStatusDiscoverySnapshot {
    OauthStatusDiscoverySnapshot {
        config_fingerprint: String::new(),
        lifecycle_epoch: None,
        pool_identity: None,
        observation: Default::default(),
        completed_at: tokio::time::Instant::now() - age,
        summary: None,
        tool_error: failed.then(|| "unavailable".to_string()),
        error: None,
    }
}

#[test]
fn successful_oauth_status_discovery_has_short_freshness_window() {
    assert!(oauth_status_discovery_is_fresh(&discovery_snapshot(
        OAUTH_STATUS_DISCOVERY_FRESHNESS
            .checked_sub(std::time::Duration::from_secs(1))
            .expect("freshness exceeds one second"),
        false,
    )));
    assert!(!oauth_status_discovery_is_fresh(&discovery_snapshot(
        OAUTH_STATUS_DISCOVERY_FRESHNESS + std::time::Duration::from_secs(1),
        false,
    )));
}

#[test]
fn failed_oauth_status_discovery_observes_retry_cooldown() {
    assert!(oauth_status_discovery_is_fresh(&discovery_snapshot(
        OAUTH_STATUS_DISCOVERY_FAILURE_COOLDOWN
            .checked_sub(std::time::Duration::from_secs(1))
            .expect("cooldown exceeds one second"),
        true,
    )));
    assert!(!oauth_status_discovery_is_fresh(&discovery_snapshot(
        OAUTH_STATUS_DISCOVERY_FAILURE_COOLDOWN + std::time::Duration::from_secs(1),
        true,
    )));
}
use labby_runtime::gateway_config::{
    GatewayConfig, UpstreamConfig, UpstreamOauthConfig, UpstreamOauthCredentialSource,
    UpstreamOauthMode, UpstreamOauthRegistration,
};

fn lifecycle_test_upstream(name: &str, oauth: bool) -> UpstreamConfig {
    UpstreamConfig {
        display_name: None,
        lifecycle: None,
        enabled: true,
        name: name.to_string(),
        url: Some(format!("https://{name}.example/mcp")),
        transport: None,
        socket_path: None,
        headers: Default::default(),
        bearer_token_env: None,
        command: None,
        args: vec![],
        env: Default::default(),
        proxy_resources: false,
        proxy_prompts: false,
        proxy_skills: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        expose_skills: None,
        code_mode_hint: None,
        oauth: oauth.then(|| UpstreamOauthConfig {
            mode: UpstreamOauthMode::AuthorizationCodePkce,
            registration: UpstreamOauthRegistration::Dynamic,
            scopes: None,
            credential: Default::default(),
            additional_endpoint_origins: vec![],
            prefer_client_metadata_document: None,
        }),
        imported_from: None,
        priority: 1.0,
    }
}

#[test]
fn shared_provider_scope_excludes_dedicated_and_non_oauth_upstreams() {
    let mut shared = lifecycle_test_upstream("shared", true);
    shared.oauth.as_mut().unwrap().credential =
        UpstreamOauthCredentialSource::GoogleProvider { account: None };
    let dedicated = lifecycle_test_upstream("dedicated", true);
    let raw = lifecycle_test_upstream("raw", false);

    assert_eq!(
        super::GatewayManager::google_provider_upstream_names(&GatewayConfig {
            upstream: vec![shared, dedicated, raw],
            ..GatewayConfig::default()
        }),
        vec!["shared".to_string()]
    );
}

#[test]
fn validate_probe_url_rejects_userinfo() {
    let result = validate_probe_url("https://user:pass@example.com/mcp");
    assert!(result.is_err(), "expected error for URL with userinfo");
}

#[test]
fn validate_probe_url_rejects_http() {
    let result = validate_probe_url("http://10.1.0.8/mcp");
    assert!(
        result.is_err(),
        "expected error for non-HTTPS OAuth probe URL"
    );
}

#[test]
fn validate_probe_url_rejects_query_and_fragment() {
    let with_query = validate_probe_url("https://example.com/mcp?foo=bar");
    assert!(
        with_query.is_err(),
        "expected error for URL with query string"
    );
    let with_fragment = validate_probe_url("https://example.com/mcp#section");
    assert!(
        with_fragment.is_err(),
        "expected error for URL with fragment"
    );
}

#[test]
fn probe_manager_key_includes_port_and_path() {
    let url = url::Url::parse("https://example.com:9000/mcp").unwrap();
    let key = probe_manager_key(&url);
    assert!(
        key.contains("example.com"),
        "key should contain hostname: {key}"
    );
    assert!(key.contains("9000"), "key should contain port: {key}");
    assert!(
        key.contains("mcp"),
        "key should contain path segment: {key}"
    );
}

#[test]
fn probe_manager_key_distinguishes_colliding_paths() {
    let url_a = url::Url::parse("https://example.com/mcp/a").unwrap();
    let url_b = url::Url::parse("https://example.com/mcp/b").unwrap();
    let key_a = probe_manager_key(&url_a);
    let key_b = probe_manager_key(&url_b);
    assert_ne!(
        key_a, key_b,
        "different paths should produce different keys"
    );
}

#[test]
fn validate_probe_upstream_name_rejects_path_like_values() {
    let with_slash = validate_probe_upstream_name("my/server");
    assert!(
        with_slash.is_err(),
        "expected error for name containing '/'"
    );
    let with_backslash = validate_probe_upstream_name("my\\server");
    assert!(
        with_backslash.is_err(),
        "expected error for name containing '\\'"
    );
    let empty = validate_probe_upstream_name("  ");
    assert!(empty.is_err(), "expected error for whitespace-only name");
}

// ── should_use_dynamic_registration coverage ─────────────────────────────────

#[test]
fn swag_uses_client_metadata_document_even_when_dynamic_registration_is_advertised() {
    // Legacy default: "swag" always uses CIMD regardless of what the server supports.
    assert!(
        !should_use_dynamic_registration("swag", true, None),
        "swag + supports_dynamic + no override → should NOT use dynamic"
    );
    // Other upstreams that support dynamic registration should use it.
    assert!(
        should_use_dynamic_registration("github", true, None),
        "github + supports_dynamic + no override → should use dynamic"
    );
    // No supports_dynamic → always false regardless of upstream name.
    assert!(
        !should_use_dynamic_registration("github", false, None),
        "no dynamic support → should NOT use dynamic"
    );
}

#[test]
fn prefer_client_metadata_document_true_overrides_dynamic_registration() {
    // When the operator explicitly sets prefer_client_metadata_document = true,
    // dynamic registration is suppressed even when the server supports it.
    assert!(
        !should_use_dynamic_registration("github", true, Some(true)),
        "explicit prefer_cimd=true + supports_dynamic → should NOT use dynamic"
    );
    assert!(
        !should_use_dynamic_registration("github", false, Some(true)),
        "explicit prefer_cimd=true + no support → should NOT use dynamic"
    );
}

#[test]
fn prefer_client_metadata_document_false_opts_in_to_dynamic_registration() {
    // When the operator explicitly sets prefer_client_metadata_document = false,
    // dynamic registration is used even for "swag".
    assert!(
        should_use_dynamic_registration("swag", true, Some(false)),
        "explicit prefer_cimd=false + supports_dynamic → should use dynamic"
    );
    // No dynamic support → still false (hardware constraint, not a preference).
    assert!(
        !should_use_dynamic_registration("swag", false, Some(false)),
        "explicit prefer_cimd=false + no support → should NOT use dynamic"
    );
}

#[tokio::test]
async fn status_refresh_rejection_invalidates_cached_runtime_authority() {
    use labby_auth::upstream::{
        cache::OauthClientCache, encryption::load_key, manager::UpstreamOauthManager,
        store::SqliteCredentialStore,
    };
    use rmcp::transport::auth::{CredentialStore, StoredCredentials};
    use std::sync::Arc;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer": format!("{}/mcp", server.uri()),
            "authorization_endpoint":format!("{}/authorize",server.uri()),
            "token_endpoint":format!("{}/token",server.uri()),
            "code_challenge_methods_supported":["S256"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(serde_json::json!({"error":"invalid_grant"})),
        )
        .mount(&server)
        .await;
    let directory = tempfile::tempdir().unwrap();
    let sqlite = labby_auth::sqlite::SqliteStore::open(directory.path().join("auth.db"))
        .await
        .unwrap();
    let key = load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let mut config = lifecycle_test_upstream("status-rejection", true);
    config.url = Some(format!("{}/mcp", server.uri()));
    config.oauth.as_mut().unwrap().registration = UpstreamOauthRegistration::Preregistered {
        client_id: "status-client".into(),
        client_secret_env: None,
    };
    let client_id = match &config.oauth.as_ref().unwrap().registration {
        UpstreamOauthRegistration::Preregistered { client_id, .. } => client_id.clone(),
        _ => unreachable!(),
    };
    let credentials: StoredCredentials = serde_json::from_value(serde_json::json!({
        "client_id":client_id,"token_response":{"access_token":"expired-access",
        "refresh_token":"rejected-refresh","token_type":"Bearer","expires_in":1},
        "granted_scopes":[],"token_received_at":1
    }))
    .unwrap();
    let store = SqliteCredentialStore::new(sqlite.clone(), key.clone(), &config.name, "alice");
    store.save(credentials).await.unwrap();
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(
        config.name.clone(),
        UpstreamOauthManager::new(
            sqlite.clone(),
            key,
            config.clone(),
            "http://127.0.0.1/callback".into(),
        ),
    );
    let cache = OauthClientCache::new(Arc::clone(&managers));
    drop(rustls::crypto::ring::default_provider().install_default());
    let client = rmcp::transport::AuthClient::new(
        reqwest::Client::new(),
        rmcp::transport::AuthorizationManager::new(config.url.as_deref().unwrap())
            .await
            .unwrap(),
    );
    cache
        .publish_prebuilt(&config, "alice", Arc::new(client))
        .unwrap();
    let rejected = cache.lifecycle_epoch_for(&config.name, "alice");
    let unrelated = cache.lifecycle_epoch_for(&config.name, "bob");
    let runtime = crate::gateway::runtime::GatewayRuntimeHandle::default();
    runtime
        .swap(Some(Arc::new(
            crate::upstream::pool::UpstreamPool::new().with_oauth_client_cache(cache.clone()),
        )))
        .await;
    let gateway =
        crate::gateway::manager::GatewayManager::new(directory.path().join("config.toml"), runtime)
            .with_oauth_client_cache(cache.clone())
            .with_upstream_oauth_managers(managers);
    let status = gateway
        .upstream_oauth_status(&config.name, "alice")
        .await
        .unwrap();
    assert!(status.refresh_attempted);
    assert_eq!(
        status.refresh_error_kind.as_deref(),
        Some("oauth_needs_reauth")
    );
    assert!(!status.authenticated);
    assert!(!status.refresh_token_present);
    assert!(status.access_token_expires_at.is_none());
    assert!(status.seconds_until_expiry.is_none());
    assert_eq!(
        status.state,
        crate::gateway::oauth::UpstreamOauthConnectionState::Disconnected
    );
    assert!(store.load().await.unwrap().is_none());
    assert!(
        !rejected.is_current(),
        "status rejection must fence retained runtime authority"
    );
    assert!(
        unrelated.is_current(),
        "another subject must retain its authority"
    );
    assert_eq!(cache.ready_client_count(), 0);
}

#[tokio::test]
async fn successful_callback_keeps_newly_observed_scope_after_runtime_epoch_replacement() {
    use labby_auth::upstream::{
        cache::OauthClientCache, encryption::load_key, manager::UpstreamOauthManager,
    };
    use std::sync::Arc;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
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
            "resource":format!("{}/mcp",server.uri()), "authorization_servers":[server.uri()]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET")).and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer":server.uri(),"authorization_endpoint":format!("{}/authorize",server.uri()),
            "token_endpoint":format!("{}/token",server.uri()), "code_challenge_methods_supported":["S256"]
        }))).mount(&server).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token":"upgraded-access","token_type":"Bearer","expires_in":3600,
            "scope":"read files:write","refresh_token":"upgraded-refresh"
        })))
        .mount(&server)
        .await;
    let directory = tempfile::tempdir().unwrap();
    let sqlite = labby_auth::sqlite::SqliteStore::open(directory.path().join("auth.db"))
        .await
        .unwrap();
    let mut config = lifecycle_test_upstream("callback-scopes", true);
    config.url = Some(format!("{}/mcp", server.uri()));
    config.oauth.as_mut().unwrap().scopes = Some(vec!["read".into()]);
    config.oauth.as_mut().unwrap().registration = UpstreamOauthRegistration::Preregistered {
        client_id: "scope-client".into(),
        client_secret_env: None,
    };
    let oauth = UpstreamOauthManager::new(
        sqlite,
        load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap(),
        config.clone(),
        "http://127.0.0.1/callback".into(),
    );
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(config.name.clone(), oauth.clone());
    let cache = OauthClientCache::new(Arc::clone(&managers));
    let gateway = super::GatewayManager::new(
        directory.path().join("config.toml"),
        crate::gateway::runtime::GatewayRuntimeHandle::default(),
    )
    .with_oauth_client_cache(cache.clone())
    .with_upstream_oauth_managers(managers);
    let observed = cache.scope_challenge_handler(&config, "alice").unwrap();
    observed("files:write".into());
    let before = cache.lifecycle_epoch_for(&config.name, "alice");
    let consent = gateway
        .begin_upstream_authorization(&config.name, "alice")
        .await
        .unwrap();
    let csrf = url::Url::parse(&consent.authorization_url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    observed("admin".into());
    gateway
        .complete_upstream_authorization_callback(&config.name, "alice", "code", &csrf)
        .await
        .unwrap();
    assert!(
        !before.is_current(),
        "callback must replace runtime authority"
    );
    assert_eq!(
        oauth.required_scope_upgrade("alice"),
        vec!["admin"],
        "a newer ungranted challenge must survive the authorized callback epoch change"
    );
    let status = gateway
        .upstream_oauth_status(&config.name, "alice")
        .await
        .unwrap();
    assert_eq!(
        status.state,
        crate::gateway::oauth::UpstreamOauthConnectionState::ScopeUpgradeRequired
    );
    assert_eq!(status.required_scopes, vec!["admin"]);
    for _ in 0..2 {
        let consent = gateway
            .begin_upstream_authorization(&config.name, "alice")
            .await
            .unwrap();
        let csrf = url::Url::parse(&consent.authorization_url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        gateway
            .complete_upstream_authorization_callback(&config.name, "alice", "code", &csrf)
            .await
            .unwrap();
        assert_eq!(oauth.required_scope_upgrade("alice"), vec!["admin"]);
    }
    assert!(
        gateway
            .begin_upstream_authorization(&config.name, "alice")
            .await
            .is_err(),
        "partial grants and replacement epochs cannot reset the attempt budget"
    );
    gateway
        .clear_upstream_credentials(&config.name, "alice")
        .await
        .unwrap();
    assert!(
        oauth.required_scope_upgrade("alice").is_empty(),
        "credential clear must not transfer or resurrect a prior challenge"
    );
    observed("admin".into());
    assert!(
        oauth.required_scope_upgrade("alice").is_empty(),
        "the prior transport remains fenced"
    );
}
