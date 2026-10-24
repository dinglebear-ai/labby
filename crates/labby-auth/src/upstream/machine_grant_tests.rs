//! Real unattended grants through the public subject-scoped auth cache.

use std::sync::Arc;

use super::{
    cache::OauthClientCache, encryption::load_key, manager::UpstreamOauthManager,
    store::SqliteCredentialStore,
};
use crate::sqlite::SqliteStore;
use labby_runtime::gateway_config::UpstreamConfig;
use rmcp_client::transport::auth::CredentialStore;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

#[test]
fn client_credentials_cache_bootstraps_and_renews_without_a_refresh_token() {
    const CHILD: &str = "LABBY_MACHINE_GRANT_FIXTURE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "upstream::machine_grant_tests::client_credentials_cache_bootstraps_and_renews_without_a_refresh_token", "--nocapture"])
            .env(CHILD, "1").env("LABBY_MACHINE_GRANT_FIXTURE_SECRET", "fixture-secret")
            .status().unwrap();
        assert!(status.success(), "isolated grant fixture must pass");
        return;
    }
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer":format!("{}/mcp",server.uri()), "authorization_endpoint":format!("{}/authorize",server.uri()),
            "token_endpoint":format!("{}/token",server.uri()), "grant_types_supported":["client_credentials"],
            "token_endpoint_auth_methods_supported":["client_secret_post"]
        }))).mount(&server).await;
        Mock::given(method("POST")).and(path("/token")).respond_with(ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({"access_token":"machine-access","token_type":"Bearer","expires_in":3600,"scope":"read"})))
            .mount(&server).await;
        let config: UpstreamConfig = serde_json::from_value(serde_json::json!({
            "name":"machine", "url":format!("{}/mcp",server.uri()), "oauth":{
                "mode":{"client_credentials":{"authentication":"client_secret"}},
                "registration":{"strategy":"preregistered","client_id":"machine-client","client_secret_env":"LABBY_MACHINE_GRANT_FIXTURE_SECRET"},
                "scopes":["read"]
            }
        })).expect("unattended mode must deserialize without changing legacy PKCE configuration");
        let directory = tempfile::tempdir().unwrap();
        let sqlite = SqliteStore::open(directory.path().join("auth.db")).await.unwrap();
        let key = load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
        let managers = Arc::new(dashmap::DashMap::new());
        managers.insert(config.name.clone(), UpstreamOauthManager::new(sqlite.clone(),key.clone(),config.clone(),"http://127.0.0.1/callback".into()));
        let cache = OauthClientCache::new(managers);
        let client = cache.get_or_build(&config,"service-caller").await.expect("configured machine grant must bootstrap through the actual cache");
        assert_eq!(client.get_access_token().await.unwrap(),"machine-access");
        let credentials = SqliteCredentialStore::new(sqlite.clone(),key,"machine","service-caller");
        let mut expired = credentials.load().await.unwrap().unwrap();
        expired.token_received_at = Some(1);
        credentials.save(expired).await.unwrap();
        let (one,two) = tokio::join!(client.get_access_token(),client.get_access_token());
        assert_eq!(one.unwrap(),"machine-access");
        assert_eq!(two.unwrap(),"machine-access");
        let requests = server.received_requests().await.unwrap();
        let grants = requests.iter().filter(|request| request.method.as_str() == "POST").collect::<Vec<_>>();
        assert_eq!(grants.len(),2,"initial grant and exactly one renewal; no refresh_token grant");
        for request in grants {
            let form = url::form_urlencoded::parse(&request.body).collect::<std::collections::HashMap<_,_>>();
            assert_eq!(form.get("grant_type").map(|v|v.as_ref()),Some("client_credentials"));
            assert_eq!(form.get("resource").map(|v|v.as_ref()),config.url.as_deref());
            assert!(!form.contains_key("refresh_token"));
        }
        assert!(sqlite.find_upstream_oauth_credentials("machine","another-caller").await.unwrap().is_none());
    });
}

pub(super) fn isolated(name: &str, future: impl Future<Output = ()>) {
    if std::env::var_os("LABBY_MACHINE_GRANT_FIXTURE_CHILD").is_none() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        let test_name = if name.contains("::") {
            name.to_owned()
        } else {
            format!("upstream::machine_grant_tests::{name}")
        };
        command
            .args(["--exact", &test_name, "--nocapture"])
            .env("LABBY_MACHINE_GRANT_FIXTURE_CHILD", "1")
            .env("LABBY_MACHINE_GRANT_FIXTURE_SECRET", "fixture-secret")
            .env(
                "LABBY_MACHINE_GRANT_FIXTURE_SUBJECT",
                "operator-approved-subject-token",
            );
        if name.contains("private_key") {
            use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
            let key = rsa::RsaPrivateKey::new(&mut rand::rng(), 2048).unwrap();
            command
                .env(
                    "LABBY_MACHINE_GRANT_FIXTURE_KEY",
                    key.to_pkcs8_pem(Default::default()).unwrap().as_str(),
                )
                .env(
                    "LABBY_MACHINE_GRANT_FIXTURE_PUBLIC_KEY",
                    key.to_public_key()
                        .to_public_key_pem(Default::default())
                        .unwrap(),
                );
        }
        assert!(
            command.status().unwrap().success(),
            "isolated grant fixture must pass"
        );
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future);
}

pub(super) async fn client_fixture(
    authentication: serde_json::Value,
) -> (
    MockServer,
    SqliteStore,
    UpstreamConfig,
    OauthClientCache,
    SqliteCredentialStore,
    Arc<dashmap::DashMap<String, UpstreamOauthManager>>,
) {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "issuer":format!("{}/mcp",server.uri()),"authorization_endpoint":format!("{}/authorize",server.uri()),
        "token_endpoint":format!("{}/token",server.uri()),"grant_types_supported":["client_credentials"],
        "token_endpoint_auth_methods_supported":["client_secret_post","private_key_jwt"],"token_endpoint_auth_signing_alg_values_supported":["RS256"]
    }))).mount(&server).await;
    let config:UpstreamConfig=serde_json::from_value(serde_json::json!({"name":"machine","url":format!("{}/mcp",server.uri()),"oauth":{
        "mode":{"client_credentials":{"authentication":authentication}},"registration":{"strategy":"preregistered","client_id":"machine-client","client_secret_env":"LABBY_MACHINE_GRANT_FIXTURE_SECRET"},"scopes":["read"]
    }})).unwrap();
    let sqlite = SqliteStore::open(tempfile::tempdir().unwrap().keep().join("auth.db"))
        .await
        .unwrap();
    let key = load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let credentials = SqliteCredentialStore::new(sqlite.clone(), key.clone(), "machine", "caller");
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(
        "machine".into(),
        UpstreamOauthManager::new(
            sqlite.clone(),
            key,
            config.clone(),
            "http://127.0.0.1/callback".into(),
        ),
    );
    (
        server,
        sqlite,
        config,
        OauthClientCache::new(Arc::clone(&managers)),
        credentials,
        managers,
    )
}

pub(super) async fn access_response(server: &MockServer) {
    Mock::given(method("POST")).and(path("/token")).respond_with(ResponseTemplate::new(200).set_body_json(
        serde_json::json!({"access_token":"machine-access","token_type":"Bearer","expires_in":3600,"scope":"read"})
    )).mount(server).await;
}

#[test]
fn private_key_jwt_uses_a_signed_registered_client_assertion() {
    isolated(
        "private_key_jwt_uses_a_signed_registered_client_assertion",
        async {
            for audience in [None, Some("urn:example:registered-token-service")] {
                let mut authentication = serde_json::json!({"private_key_jwt":{"signing_key_env":"LABBY_MACHINE_GRANT_FIXTURE_KEY","algorithm":"RS256"}});
                if let Some(audience) = audience {
                    authentication["private_key_jwt"]["token_endpoint_audience"] = audience.into();
                }
                let (server, _, config, cache, _, _) = client_fixture(authentication).await;
                access_response(&server).await;
                let client = cache.get_or_build(&config, "caller").await.unwrap();
                assert_eq!(client.get_access_token().await.unwrap(), "machine-access");
                let requests = server.received_requests().await.unwrap();
                let request = requests
                    .iter()
                    .find(|request| request.method.as_str() == "POST")
                    .unwrap();
                let form = url::form_urlencoded::parse(&request.body)
                    .collect::<std::collections::HashMap<_, _>>();
                assert!(!form.contains_key("client_id"));
                assert!(!form.contains_key("client_secret"));
                assert_eq!(
                    form.get("client_assertion_type").unwrap(),
                    "urn:ietf:params:oauth:client-assertion-type:jwt-bearer"
                );
                let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
                validation.set_audience(&[audience
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{}/mcp", server.uri()))]);
                validation.set_issuer(&["machine-client"]);
                validation.sub = Some("machine-client".into());
                let claims = jsonwebtoken::decode::<serde_json::Value>(
                    form.get("client_assertion").unwrap().as_ref(),
                    &jsonwebtoken::DecodingKey::from_rsa_pem(
                        std::env::var("LABBY_MACHINE_GRANT_FIXTURE_PUBLIC_KEY")
                            .unwrap()
                            .as_bytes(),
                    )
                    .unwrap(),
                    &validation,
                )
                .unwrap()
                .claims;
                assert_eq!(claims["sub"], "machine-client");
                assert!(
                    claims["jti"]
                        .as_str()
                        .is_some_and(|value| !value.is_empty())
                );
            }
        },
    );
}

#[test]
fn machine_grant_rejection_deletes_only_current_authority_and_fails_closed() {
    isolated(
        "machine_grant_rejection_deletes_only_current_authority_and_fails_closed",
        async {
            let (server, _, config, cache, credentials, _) =
                client_fixture(serde_json::json!("client_secret")).await;
            access_response(&server).await;
            let client = cache.get_or_build(&config, "caller").await.unwrap();
            let mut expired = credentials.load().await.unwrap().unwrap();
            expired.token_received_at = Some(1);
            credentials.save(expired).await.unwrap();
            server.reset().await;
            Mock::given(method("POST")).respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({"error":"invalid_grant","error_description":"fixture-secret"}))).mount(&server).await;
            let failure = client.get_access_token().await.unwrap_err().to_string();
            assert!(!failure.contains("fixture-secret"));
            assert!(credentials.load().await.unwrap().is_none());
            assert!(
                cache.has_pending_invalidations(),
                "successful rejection CAS must notify runtime invalidation"
            );
        },
    );
}

#[test]
fn changed_grant_policy_cannot_reuse_old_cached_authority() {
    isolated(
        "changed_grant_policy_cannot_reuse_old_cached_authority",
        async {
            let (server, sqlite, config, cache, credentials, managers) =
                client_fixture(serde_json::json!("client_secret")).await;
            access_response(&server).await;
            let _before = cache.get_or_build(&config, "caller").await.unwrap();
            let mut changed = config.clone();
            changed.oauth.as_mut().unwrap().mode =
                labby_runtime::gateway_config::UpstreamOauthMode::AuthorizationCodePkce;
            assert!(credentials.load().await.unwrap().is_some());
            managers.insert(
                "machine".into(),
                UpstreamOauthManager::new(
                    sqlite,
                    load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap(),
                    changed.clone(),
                    "http://127.0.0.1/callback".into(),
                ),
            );
            assert!(
                cache.get_or_build(&changed, "caller").await.is_err(),
                "mode change must not return the old cached client or use its token"
            );
        },
    );
}

async fn enterprise_exchange(kind: &str, wrong_resource: bool) {
    use base64::Engine as _;
    let resource = MockServer::start().await;
    let idp = MockServer::start().await;
    for (server, issuer) in [
        (&resource, format!("{}/mcp", resource.uri())),
        (&idp, idp.uri()),
    ] {
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer":issuer,"authorization_endpoint":format!("{}/authorize",server.uri()),"token_endpoint":format!("{}/token",server.uri())
        }))).mount(server).await;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let encode = |value: serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap())
    };
    let jag = format!(
        "{}.{}.{}",
        encode(serde_json::json!({"alg":"RS256","typ":"oauth-id-jag+jwt"})),
        encode(serde_json::json!({
            "iss":idp.uri(),"sub":"approved-employee","aud":format!("{}/mcp",resource.uri()),"client_id":"machine-client","jti":"one-use-fixture",
            "iat":now,"exp":now+300,"resource":if wrong_resource {"https://wrong.example/mcp".into()}else{format!("{}/mcp",resource.uri())},"scope":"read"
        })),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"fixture-signature")
    );
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "access_token":jag,"issued_token_type":"urn:ietf:params:oauth:token-type:id-jag","token_type":"N_A","scope":"read"
    }))).mount(&idp).await;
    access_response(&resource).await;
    let config:UpstreamConfig=serde_json::from_value(serde_json::json!({"name":"enterprise","url":format!("{}/mcp",resource.uri()),"oauth":{
        "mode":{"enterprise_managed":{"identity_provider":{"issuer":idp.uri(),"token_endpoint":format!("{}/token",idp.uri()),"client_id":"enterprise-client","client_secret_env":"LABBY_MACHINE_GRANT_FIXTURE_SECRET","authentication":"client_secret_post"},
            "subject_token":{"token_env":"LABBY_MACHINE_GRANT_FIXTURE_SUBJECT","kind":kind},"resource_authentication":"client_secret_basic"}},
        "registration":{"strategy":"preregistered","client_id":"machine-client","client_secret_env":"LABBY_MACHINE_GRANT_FIXTURE_SECRET"},"scopes":["read"]
    }})).unwrap();
    let sqlite = SqliteStore::open(tempfile::tempdir().unwrap().keep().join("auth.db"))
        .await
        .unwrap();
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(
        "enterprise".into(),
        UpstreamOauthManager::new(
            sqlite.clone(),
            load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap(),
            config.clone(),
            "http://127.0.0.1/callback".into(),
        ),
    );
    let cache = OauthClientCache::new(managers);
    let result = cache.get_or_build(&config, "enterprise-caller").await;
    if wrong_resource {
        assert!(result.is_err());
        assert!(
            resource
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.method.as_str() != "POST"),
            "invalid ID-JAG must never reach resource redemption"
        );
        assert!(
            sqlite
                .find_upstream_oauth_credentials("enterprise", "enterprise-caller")
                .await
                .unwrap()
                .is_none()
        );
    } else {
        assert_eq!(
            result.unwrap().get_access_token().await.unwrap(),
            "machine-access"
        );
        let requests = idp.received_requests().await.unwrap();
        let request = requests
            .iter()
            .find(|request| request.method.as_str() == "POST")
            .unwrap();
        let form =
            url::form_urlencoded::parse(&request.body).collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            form.get("subject_token_type").unwrap(),
            &format!("urn:ietf:params:oauth:token-type:{kind}")
        );
        assert_eq!(form.get("client_id").unwrap(), "enterprise-client");
        assert_eq!(form.get("client_secret").unwrap(), "fixture-secret");
        assert_eq!(
            form.get("resource").map(|value| value.as_ref()),
            config.url.as_deref()
        );
        assert_eq!(
            form.get("audience").map(|value| value.as_ref()),
            Some(format!("{}/mcp", resource.uri()).as_str())
        );
        let requests = resource.received_requests().await.unwrap();
        let request = requests
            .iter()
            .find(|request| request.method.as_str() == "POST")
            .unwrap();
        assert!(
            request
                .headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("Basic ")
        );
        let form =
            url::form_urlencoded::parse(&request.body).collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            form.get("grant_type").unwrap(),
            "urn:ietf:params:oauth:grant-type:jwt-bearer"
        );
        // The resource binding is inside the validated ID-JAG assertion;
        // SDK redemption uses the RFC 7523 assertion grant without resource.
        assert_eq!(
            form.get("assertion").map(|value| value.as_ref()),
            Some(jag.as_str())
        );
        assert!(!form.contains_key("resource"));
    }
}

#[test]
fn enterprise_managed_exchanges_approved_refresh_and_id_tokens() {
    isolated(
        "enterprise_managed_exchanges_approved_refresh_and_id_tokens",
        async {
            enterprise_exchange("refresh_token", false).await;
            enterprise_exchange("id_token", false).await;
        },
    );
}

#[test]
fn enterprise_managed_rejects_an_id_jag_for_another_resource() {
    isolated(
        "enterprise_managed_rejects_an_id_jag_for_another_resource",
        async {
            enterprise_exchange("id_token", true).await;
        },
    );
}

#[test]
fn private_key_jwt_audience_replacement_cannot_reuse_cached_or_persisted_authority() {
    isolated(
        "private_key_jwt_audience_replacement_cannot_reuse_cached_or_persisted_authority",
        async {
            let (server, sqlite, config, cache, _, managers) = client_fixture(serde_json::json!({"private_key_jwt":{"signing_key_env":"LABBY_MACHINE_GRANT_FIXTURE_KEY","algorithm":"RS256"}})).await;
            access_response(&server).await;
            cache.get_or_build(&config, "caller").await.unwrap();
            let mut changed = serde_json::to_value(&config).unwrap();
            changed["oauth"]["mode"]["client_credentials"]["authentication"]["private_key_jwt"]["token_endpoint_audience"] =
                "urn:example:replacement".into();
            let changed: UpstreamConfig = serde_json::from_value(changed).unwrap();
            managers.insert(
                "machine".into(),
                UpstreamOauthManager::new(
                    sqlite,
                    load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap(),
                    changed.clone(),
                    "http://127.0.0.1/callback".into(),
                ),
            );
            cache.get_or_build(&changed, "caller").await.unwrap();
            let requests = server.received_requests().await.unwrap();
            assert_eq!(
                requests
                    .iter()
                    .filter(|request| request.method.as_str() == "POST")
                    .count(),
                2,
                "audience replacement must obtain a new grant instead of reusing either cache or persisted token"
            );
        },
    );
}

#[test]
fn private_key_jwt_audience_validation_accepts_identifiers_and_rejects_unsafe_values() {
    for audience in [
        "urn:example:registered-token-service",
        "https://issuer.example/token",
        "opaque-registration-id",
        "",
        "  ",
        " leading",
        "trailing ",
        "line\nfeed",
        "null\0byte",
    ] {
        let config: UpstreamConfig = serde_json::from_value(serde_json::json!({"name":"machine", "url":"https://mcp.example/mcp", "oauth":{"mode":{"client_credentials":{"authentication":{"private_key_jwt":{"signing_key_env":"SIGNING_KEY", "algorithm":"RS256", "token_endpoint_audience":audience}}}},"registration":{"strategy":"preregistered", "client_id":"registered-client"}}})).unwrap();
        let valid = !audience.trim().is_empty()
            && audience.trim() == audience
            && !audience.chars().any(char::is_control);
        assert_eq!(
            config.validate().is_ok(),
            valid,
            "audience validation must preserve opaque identifiers and reject blank, padded or control-bearing values"
        );
    }
}
