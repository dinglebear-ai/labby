#![cfg(feature = "gateway")]
#![allow(clippy::panic, dead_code)]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;

use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::time::Duration;

async fn identity(client: &reqwest::Client, base: &str, token: &str) -> Value {
    let response = client
        .get(format!("{base}/v1/integration/identity"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response.headers()[reqwest::header::CACHE_CONTROL],
        "private, no-store"
    );
    let value = response.json::<Value>().await.unwrap();
    let schema: Value = serde_json::from_str(include_str!(
        "../../../docs/contracts/integration-identity-v1.schema.json"
    ))
    .unwrap();
    assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&value));
    value
}

#[tokio::test]
async fn daemon_owns_identity_and_preserves_it_across_restart() {
    let token = uuid::Uuid::new_v4().to_string();
    let mut guard = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", &token)
        .start()
        .await
        .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let base = guard.connection().base_url.clone();
    let unauthenticated = client
        .get(format!("{base}/v1/integration/identity"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), reqwest::StatusCode::UNAUTHORIZED);
    let first = identity(&client, &base, &token).await;
    let path = guard.root().join("labby-home/installation-id");
    let persisted = std::fs::read(&path).unwrap();
    assert!(!persisted.is_empty());
    let expected = format!("labby_{}", hex::encode(Sha256::digest(&persisted)));
    assert_eq!(first["server_id"], expected);
    assert_eq!(first["auth"]["modes"], serde_json::json!(["static_bearer"]));
    assert!(!first.to_string().contains(&token));
    guard.restart().await.unwrap();
    let second = identity(&client, &guard.connection().base_url, &token).await;
    assert_eq!(second, first);
    assert_eq!(std::fs::read(path).unwrap(), persisted);
    let cleanup = guard.finish().await;
    assert!(
        cleanup.is_clean(),
        "identity daemon cleanup: {:?}",
        cleanup.failures
    );
}
