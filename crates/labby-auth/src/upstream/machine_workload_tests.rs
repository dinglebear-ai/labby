//! Workload grants through the real subject-scoped cache, SDK and HTTP boundary.

use std::sync::Arc;

use labby_runtime::gateway_config::{UpstreamConfig, UpstreamOauthMode};
use oauth2::TokenResponse as _;
use rmcp_client::transport::auth::CredentialStore;
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

use super::{
    cache::OauthClientCache, encryption::load_key, machine_grant_tests::isolated,
    manager::UpstreamOauthManager, store::SqliteCredentialStore,
};
use crate::sqlite::SqliteStore;

const ASSERTION: &str = "operator-approved-subject-token";
const SUBJECT_ENV: &str = "LABBY_MACHINE_GRANT_FIXTURE_SUBJECT";
const GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

async fn fixture(
    metadata_fields: serde_json::Value,
    response: serde_json::Value,
) -> (
    MockServer,
    UpstreamConfig,
    OauthClientCache,
    SqliteStore,
    Arc<dashmap::DashMap<String, UpstreamOauthManager>>,
    tempfile::TempDir,
) {
    let server = MockServer::start().await;
    let mut metadata = json!({"issuer":format!("{}/mcp",server.uri()),
        "authorization_endpoint":format!("{}/authorize",server.uri()),
        "token_endpoint":format!("{}/token",server.uri())});
    metadata
        .as_object_mut()
        .unwrap()
        .extend(metadata_fields.as_object().unwrap().clone());
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(metadata))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&server)
        .await;
    let config: UpstreamConfig = serde_json::from_value(json!({"name":"workload",
        "url":format!("{}/mcp",server.uri()),"oauth":{
        "mode":{"workload_identity_federation":{"assertion_env":SUBJECT_ENV}},
        "registration":{"strategy":"preregistered","client_id":"workload-client"},
        "scopes":["read"]}}))
    .unwrap();
    config.validate().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let sqlite = SqliteStore::open(directory.path().join("auth.db"))
        .await
        .unwrap();
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(config.name.clone(), manager(&config, &sqlite));
    (
        server,
        config,
        OauthClientCache::new(Arc::clone(&managers)),
        sqlite,
        managers,
        directory,
    )
}

fn manager(config: &UpstreamConfig, sqlite: &SqliteStore) -> UpstreamOauthManager {
    UpstreamOauthManager::new(
        sqlite.clone(),
        load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap(),
        config.clone(),
        "http://127.0.0.1/callback".into(),
    )
}

fn metadata() -> serde_json::Value {
    json!({"grant_types_supported":[GRANT], "token_endpoint_auth_methods_supported":["none"]})
}

fn token() -> serde_json::Value {
    json!({"access_token":"workload-access","token_type":"Bearer","expires_in":3600,"scope":"read"})
}

async fn post_count(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "POST")
        .count()
}

#[test]
fn workload_cache_uses_real_sdk_and_isolates_callers_without_retaining_assertion() {
    isolated(
        "upstream::machine_workload_tests::workload_cache_uses_real_sdk_and_isolates_callers_without_retaining_assertion",
        async {
            let (server, config, cache, sqlite, _, _directory) = fixture(metadata(), token()).await;
            let client = cache.get_or_build(&config, "caller-one").await.unwrap();
            assert_eq!(client.get_access_token().await.unwrap(), "workload-access");
            assert_eq!(
                cache
                    .get_or_build(&config, "caller-one")
                    .await
                    .unwrap()
                    .get_access_token()
                    .await
                    .unwrap(),
                "workload-access"
            );
            assert_eq!(
                post_count(&server).await,
                1,
                "fresh grant must remain cached"
            );
            assert!(
                sqlite
                    .find_upstream_oauth_credentials("workload", "caller-two")
                    .await
                    .unwrap()
                    .is_none()
            );
            cache.get_or_build(&config, "caller-two").await.unwrap();
            assert_eq!(
                post_count(&server).await,
                2,
                "callers require separately stored authority"
            );
            for subject in ["caller-one", "caller-two"] {
                let credentials = SqliteCredentialStore::new(
                    sqlite.clone(),
                    load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap(),
                    "workload",
                    subject,
                )
                .load()
                .await
                .unwrap()
                .unwrap();
                let retained = serde_json::to_string(&credentials).unwrap();
                assert!(!retained.contains(ASSERTION));
                assert!(
                    credentials
                        .token_response
                        .unwrap()
                        .refresh_token()
                        .is_none()
                );
            }
            for request in server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|r| r.method.as_str() == "POST")
            {
                let form = url::form_urlencoded::parse(&request.body)
                    .into_owned()
                    .collect::<std::collections::HashMap<_, _>>();
                assert_eq!(form["grant_type"], GRANT);
                assert_eq!(form["assertion"], ASSERTION);
                assert_eq!(form["client_id"], "workload-client");
                assert_eq!(form["resource"], config.url.as_deref().unwrap());
                assert_eq!(form["scope"], "read");
                assert!(!form.contains_key("client_secret"));
            }
        },
    );
}

#[test]
fn workload_missing_advertised_grant_or_public_auth_rejects_before_token_post() {
    isolated(
        "upstream::machine_workload_tests::workload_missing_advertised_grant_or_public_auth_rejects_before_token_post",
        async {
            for fields in [
                json!({"token_endpoint_auth_methods_supported":["none"]}),
                json!({"grant_types_supported":["client_credentials"],"token_endpoint_auth_methods_supported":["none"]}),
                json!({"grant_types_supported":[GRANT]}),
                json!({"grant_types_supported":[GRANT],"token_endpoint_auth_methods_supported":["client_secret_basic"]}),
            ] {
                let (server, config, cache, _, _, _directory) = fixture(fields, token()).await;
                let error = cache
                    .get_or_build(&config, "caller")
                    .await
                    .unwrap_err()
                    .to_string();
                assert!(!error.contains(ASSERTION));
                assert_eq!(post_count(&server).await, 0);
            }
        },
    );
}

#[test]
fn workload_token_wrong_resource_or_scope_is_never_persisted() {
    isolated(
        "upstream::machine_workload_tests::workload_token_wrong_resource_or_scope_is_never_persisted",
        async {
            for (field, value) in [
                ("resource", "https://wrong.example/mcp"),
                ("scope", "admin"),
            ] {
                let mut response = token();
                response[field] = value.into();
                let (server, config, cache, sqlite, _, _directory) =
                    fixture(metadata(), response).await;
                assert!(cache.get_or_build(&config, "caller").await.is_err());
                assert_eq!(post_count(&server).await, 1);
                assert!(
                    sqlite
                        .find_upstream_oauth_credentials("workload", "caller")
                        .await
                        .unwrap()
                        .is_none()
                );
            }
        },
    );
}

#[test]
fn workload_changed_assertion_reference_replaces_cached_authority() {
    isolated(
        "upstream::machine_workload_tests::workload_changed_assertion_reference_replaces_cached_authority",
        async {
            let (server, mut config, cache, sqlite, managers, _directory) =
                fixture(metadata(), token()).await;
            cache.get_or_build(&config, "caller").await.unwrap();
            config.oauth.as_mut().unwrap().mode = UpstreamOauthMode::WorkloadIdentityFederation {
                assertion_env: "LABBY_MACHINE_GRANT_FIXTURE_SECRET".into(),
            };
            managers.insert(config.name.clone(), manager(&config, &sqlite));
            cache.get_or_build(&config, "caller").await.unwrap();
            assert_eq!(
                post_count(&server).await,
                2,
                "changed assertion policy must not reuse the old token"
            );
            let requests = server.received_requests().await.unwrap();
            let last = requests
                .iter()
                .rev()
                .find(|r| r.method.as_str() == "POST")
                .unwrap();
            let form = url::form_urlencoded::parse(&last.body)
                .into_owned()
                .collect::<std::collections::HashMap<_, _>>();
            assert_eq!(form["assertion"], "fixture-secret");
        },
    );
}
