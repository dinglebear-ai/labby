//! Qualify daemon ownership with the real HTTP and extension socket adapters.
#![cfg(feature = "gateway")]
#![allow(dead_code, clippy::panic)]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;

use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

#[tokio::test]
async fn browser_runtime_is_created_only_by_a_validated_extension_socket() {
    tokio::time::timeout(Duration::from_secs(30), verify_runtime_ownership())
        .await
        .expect("browser ownership deadline");
}

async fn verify_runtime_ownership() {
    let token = "browser-runtime-disposable-token";
    let server = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", token)
        .start()
        .await
        .expect("isolated browser gateway");
    let base = &server.connection().base_url;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let action = |name: &str| {
        client
            .post(format!("{base}/v1/browser"))
            .bearer_auth(token)
            .json(&json!({"action": name, "params": {}}))
    };

    assert!(action("help").send().await.unwrap().status().is_success());
    let unavailable: Value = action("browser.status")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unavailable["kind"], "browser_unavailable", "{unavailable}");

    let socket_url = format!("{}/browser/socket", base.replacen("http://", "ws://", 1));
    let mut rejected = socket_url.clone().into_client_request().unwrap();
    rejected
        .headers_mut()
        .insert("Origin", "https://untrusted.example".parse().unwrap());
    let error = tokio_tungstenite::connect_async(rejected)
        .await
        .unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP rejection");
    };
    assert_eq!(response.status().as_u16(), 403);
    let unavailable: Value = action("browser.status")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unavailable["kind"], "browser_unavailable");

    let mut request = socket_url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Origin",
        "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .parse()
            .unwrap(),
    );
    let (mut socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(response.status().as_u16(), 101);
    let response = action("browser.status").send().await.unwrap();
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["available"], true);
    socket.close(None).await.unwrap();
    assert!(server.finish().await.is_clean());
}
