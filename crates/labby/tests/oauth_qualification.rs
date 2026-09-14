//! Q2 bearer and OAuth qualification through public contracts.
#![cfg(feature = "gateway")]
#![allow(clippy::panic)]

#[allow(dead_code)]
#[path = "support/live_identity.rs"]
mod live_identity;
#[path = "support/oauth_qualification.rs"]
mod oauth_support;
#[path = "support/lib.rs"]
mod support;

use std::collections::BTreeMap;

use axum::http::StatusCode;
#[cfg(feature = "proxy-testkit")]
use oauth_support::GoogleFixture;
use oauth_support::{
    ActualProviderGate, AutheliaFixture, actual_provider_gate, assert_log_secret_absent,
    assert_secret_absent, authority_state_digest, denied_revoke, github_inbound_rejection,
    protected_initialize, support_decisions,
};
use support::LiveLabbyBuilder;

fn unique_query(url: &reqwest::Url) -> BTreeMap<String, String> {
    let mut query = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        assert!(
            query.insert(key.into_owned(), value.into_owned()).is_none(),
            "duplicate authorization query parameter"
        );
    }
    query
}

fn session_cookie_secret(response: &reqwest::Response) -> String {
    let prefix = format!("{}=", labby_auth::session::BROWSER_SESSION_COOKIE_NAME);
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            value
                .split(';')
                .next()
                .and_then(|cookie| cookie.strip_prefix(&prefix))
        })
        .filter(|value| !value.is_empty())
        .expect("successful callback session cookie")
        .to_owned()
}

#[tokio::test]
async fn bearer_denials_are_typed_isolated_side_effect_free_and_persist_across_restart() {
    let mut owner = live_identity::LiveIdentity::bootstrap("q2-owner@example.test")
        .await
        .expect("owner");
    let foreign = live_identity::LiveIdentity::bootstrap("q2-foreign@example.test")
        .await
        .expect("foreign");
    let owner_token = owner.credential_for_request().to_owned();
    let foreign_token = foreign.credential_for_request().to_owned();
    assert_ne!(owner.identity.subject, foreign.identity.subject);
    assert_eq!(
        owner.protected_mcp_initialize().await.unwrap(),
        StatusCode::OK
    );

    let before = authority_state_digest(owner.root()).unwrap();
    for (case, token) in [
        ("missing", None),
        ("malformed", Some("malformed-q2-bearer")),
        ("foreign", Some(foreign_token.as_str())),
    ] {
        let (status, body) = protected_initialize(owner.base(), token).await.unwrap();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.len() < 64 * 1024);
        assert_secret_absent(&body, &[&owner_token, &foreign_token]);
        assert_eq!(
            authority_state_digest(owner.root()).unwrap(),
            before,
            "{case} bearer mutated authority state during protected read denial"
        );
        let (status, body) = denied_revoke(owner.base(), token, &owner.identity.credential_id)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_secret_absent(&body, &[&owner_token, &foreign_token]);
        assert_eq!(
            authority_state_digest(owner.root()).unwrap(),
            before,
            "{case} bearer mutated authority state during destructive denial"
        );
    }
    assert_eq!(
        authority_state_digest(owner.root()).unwrap(),
        before,
        "denied read/destructive requests mutated authority state"
    );

    assert_eq!(owner.revoke().await.unwrap(), StatusCode::OK);
    assert_eq!(
        owner.protected_mcp_initialize().await.unwrap(),
        StatusCode::UNAUTHORIZED
    );
    owner.restart().await.unwrap();
    assert_eq!(
        owner.protected_mcp_initialize().await.unwrap(),
        StatusCode::UNAUTHORIZED,
        "restart resurrected revoked authority"
    );

    assert_log_secret_absent(
        owner.root(),
        &[&owner_token, &foreign_token, "malformed-q2-bearer"],
    )
    .unwrap();
    assert_log_secret_absent(foreign.root(), &[&foreign_token]).unwrap();

    assert!(owner.cleanup().await.unwrap().is_clean());
    assert!(foreign.cleanup().await.unwrap().is_clean());
}

#[tokio::test]
#[cfg(feature = "proxy-testkit")]
async fn google_real_daemon_consumes_bound_state_and_pkce_once_across_restart() {
    const CLIENT_ID: &str = "q2-google-client";
    const CLIENT_SECRET: &str = "q2-google-client-secret";
    const EMAIL: &str = "q2-google-user@example.test";
    const SUBJECT: &str = "q2-google-subject";
    const ENCRYPTION_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    let fixture = GoogleFixture::start(CLIENT_ID, SUBJECT, EMAIL).await;
    let mut daemon = LiveLabbyBuilder::new()
        .env("LABBY_AUTH_MODE", "oauth")
        .env("LABBY_PUBLIC_URL", "http://127.0.0.1:7777")
        .env("LABBY_AUTH_ADMIN_EMAIL", EMAIL)
        .env("LABBY_GOOGLE_CLIENT_ID", CLIENT_ID)
        .env("LABBY_GOOGLE_CLIENT_SECRET", CLIENT_SECRET)
        .env("LABBY_TOKEN_ENCRYPTION_KEY", ENCRYPTION_KEY)
        .env("LABBY_TEST_GOOGLE_TOKEN_ENDPOINT", fixture.token_endpoint())
        .env("LABBY_TEST_GOOGLE_JWKS_ENDPOINT", fixture.jwks_endpoint())
        .start()
        .await
        .expect("OAuth daemon");
    let base = daemon.connection().base_url.clone();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let missing = client
        .get(format!(
            "{base}/auth/google/callback?state=unknown&code=fixture-code"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);

    let login = client
        .get(format!("{base}/auth/login?return_to=%2Fgateway"))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::FOUND);
    let upstream = reqwest::Url::parse(login.headers()["location"].to_str().unwrap()).unwrap();
    let query = unique_query(&upstream);
    let state = query["state"].clone();
    assert_eq!(query["code_challenge_method"], "S256");
    let challenge = query["code_challenge"].clone();
    assert!(!challenge.is_empty());
    fixture.expect_exchange(
        CLIENT_ID,
        CLIENT_SECRET,
        "http://127.0.0.1:7777/auth/google/callback",
        &challenge,
    );

    let callback_url = format!(
        "{base}/auth/google/callback?state={state}&code=fixture-code&iss={}",
        percent_encoding::utf8_percent_encode(
            "https://accounts.google.com",
            percent_encoding::NON_ALPHANUMERIC
        )
    );
    let callback = client
        .get(&callback_url)
        .send()
        .await
        .unwrap_or_else(|error| panic!("Google callback request failed: {error}"));
    assert_eq!(callback.status(), StatusCode::SEE_OTHER);
    let session_cookie = session_cookie_secret(&callback);

    let replay = client.get(&callback_url).send().await.unwrap();
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
    daemon.restart().await.unwrap();
    let replay_after_restart = client.get(&callback_url).send().await.unwrap();
    assert_eq!(replay_after_restart.status(), StatusCode::BAD_REQUEST);

    let requests = fixture.server.received_requests().await.unwrap();
    let token_requests = requests
        .iter()
        .filter(|request| request.url.path() == "/token")
        .collect::<Vec<_>>();
    assert_eq!(
        token_requests.len(),
        1,
        "state replay repeated token exchange"
    );
    let token_body = String::from_utf8_lossy(&token_requests[0].body);
    assert!(token_body.contains("code=fixture-code"));
    assert!(token_body.contains("code_verifier="));
    assert!(!token_body.contains("code_verifier=&"));

    assert_log_secret_absent(
        daemon.root(),
        &[
            CLIENT_SECRET,
            &state,
            &session_cookie,
            "fixture-code",
            "q2-google-access-token",
            "q2-google-refresh-token",
            ENCRYPTION_KEY,
        ],
    )
    .unwrap();

    assert!(daemon.finish().await.is_clean());
}

#[tokio::test]
async fn authelia_real_daemon_discovers_and_consumes_bound_pkce_state_once() {
    const CLIENT_ID: &str = "q2-authelia-client";
    const CLIENT_SECRET: &str = "q2-authelia-client-secret";
    const EMAIL: &str = "q2-authelia-user@example.test";
    const SUBJECT: &str = "q2-authelia-subject";
    const ENCRYPTION_KEY: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    let fixture = AutheliaFixture::start(CLIENT_ID, SUBJECT, EMAIL).await;
    let mut daemon = LiveLabbyBuilder::new()
        .env("LABBY_AUTH_MODE", "oauth")
        .env("LABBY_AUTH_PROVIDER", "authelia")
        .env("LABBY_PUBLIC_URL", "http://127.0.0.1:7777")
        .env("LABBY_AUTH_ADMIN_EMAIL", EMAIL)
        .env("LABBY_AUTHELIA_ISSUER_URL", &fixture.issuer)
        .env("LABBY_AUTHELIA_CLIENT_ID", CLIENT_ID)
        .env("LABBY_AUTHELIA_CLIENT_SECRET", CLIENT_SECRET)
        .env("LABBY_AUTHELIA_TRUSTED_PRIVATE_ORIGIN", &fixture.issuer)
        .env("LABBY_AUTHELIA_CA_CERT_PATH", &fixture.ca_path)
        .env("LABBY_TOKEN_ENCRYPTION_KEY", ENCRYPTION_KEY)
        .start()
        .await
        .expect("Authelia OAuth daemon");
    let base = daemon.connection().base_url.clone();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let login = client
        .get(format!("{base}/auth/login?return_to=%2Fgateway"))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::FOUND);
    let upstream = reqwest::Url::parse(login.headers()["location"].to_str().unwrap()).unwrap();
    assert_eq!(upstream.origin().ascii_serialization(), fixture.issuer);
    let query = unique_query(&upstream);
    let state = query["state"].clone();
    assert_eq!(query["code_challenge_method"], "S256");
    assert!(!query["code_challenge"].is_empty());
    assert!(!query["nonce"].is_empty());

    let wrong_issuer = client
        .get(format!(
            "{base}/auth/oidc/callback?state={state}&code=fixture-code&iss=https%3A%2F%2Fevil.example"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_issuer.status(), StatusCode::UNAUTHORIZED);

    let second_login = client
        .get(format!("{base}/auth/login?return_to=%2Fgateway"))
        .send()
        .await
        .unwrap();
    let second_upstream =
        reqwest::Url::parse(second_login.headers()["location"].to_str().unwrap()).unwrap();
    let second_query = unique_query(&second_upstream);
    let second_state = second_query["state"].clone();
    let second_nonce = second_query["nonce"].clone();
    let second_challenge = second_query["code_challenge"].clone();
    fixture.set_nonce(&second_nonce);
    fixture.expect_exchange(
        CLIENT_ID,
        CLIENT_SECRET,
        "http://127.0.0.1:7777/auth/oidc/callback",
        &second_challenge,
    );
    let callback_url = format!(
        "{base}/auth/oidc/callback?state={second_state}&code=fixture-code&iss={}",
        percent_encoding::utf8_percent_encode(&fixture.issuer, percent_encoding::NON_ALPHANUMERIC)
    );
    let callback = client
        .get(&callback_url)
        .send()
        .await
        .unwrap_or_else(|error| panic!("Authelia callback failed: {error}"));
    assert_eq!(callback.status(), StatusCode::SEE_OTHER);
    let session_cookie = session_cookie_secret(&callback);
    assert_eq!(
        client.get(&callback_url).send().await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    daemon.restart().await.unwrap();
    assert_eq!(
        client.get(&callback_url).send().await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );

    let requests = fixture.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("POST /api/oidc/token "))
            .count(),
        1
    );
    let exchange = requests
        .iter()
        .find(|request| request.starts_with("POST /api/oidc/token "))
        .unwrap();
    assert!(
        exchange
            .to_ascii_lowercase()
            .contains("authorization: basic ")
    );
    assert!(exchange.contains("code_verifier="));
    assert!(!exchange.contains("code_verifier=&"));
    assert_log_secret_absent(
        daemon.root(),
        &[
            CLIENT_SECRET,
            &state,
            &second_state,
            &second_nonce,
            &session_cookie,
            "fixture-code",
            "q2-authelia-access",
            "q2-authelia-refresh",
            ENCRYPTION_KEY,
        ],
    )
    .unwrap();
    fixture.finish().await.unwrap();
    assert!(daemon.finish().await.is_clean());
}

#[test]
fn actual_provider_rows_are_explicitly_unavailable_without_credentials() {
    let empty = BTreeMap::new();
    for provider in ["google", "authelia", "github-upstream"] {
        let ActualProviderGate::Unavailable { missing } = actual_provider_gate(provider, &empty)
        else {
            panic!("missing actual-provider credentials cannot qualify {provider}");
        };
        assert!(!missing.is_empty());
    }
}

#[test]
fn github_inbound_and_upstream_support_are_decided_separately() {
    let rejection = github_inbound_rejection();
    assert!(
        rejection.contains("must be `google` or `authelia`"),
        "{rejection}"
    );
    let decisions = support_decisions();
    assert_eq!(decisions["google_deterministic"]["flow"], "browser_session");
    assert_eq!(
        decisions["google_deterministic"]["refresh"],
        "not_applicable"
    );
    assert_eq!(
        decisions["authelia_deterministic"]["flow"],
        "browser_session"
    );
    assert_eq!(
        decisions["authelia_deterministic"]["refresh"],
        "not_applicable"
    );
    assert_eq!(decisions["github_inbound"]["status"], "unsupported");
    assert_eq!(decisions["github_upstream"]["status"], "unknown");
}
