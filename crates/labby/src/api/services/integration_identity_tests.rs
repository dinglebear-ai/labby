use super::*;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

use crate::api::router::build_router_with_bearer;

#[test]
fn persisted_server_id_is_stable_and_schema_shaped() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("server-id");

    let first = load_or_create_server_id(&path).unwrap();
    let second = load_or_create_server_id(&path).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.len(), "labby_".len() + 32);
    assert!(first.starts_with("labby_"));
}

#[tokio::test]
async fn identity_is_v1_authenticated_schema_conformant_and_redacted() {
    let state = AppState::new().with_integration_server_id("labby_0123456789abcdef");
    let app = build_router_with_bearer(state, Some("secret-token".into()), None);

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/integration/identity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/integration/identity")
                .header("authorization", "Bearer secret-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["contract_version"], "1.0.0");
    assert_eq!(json["product"], "labby");
    assert_eq!(json["server_id"], "labby_0123456789abcdef");
    assert_eq!(json["product_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        json["api_version"],
        serde_json::json!({"major": 1, "minor": 0})
    );
    assert_eq!(json["auth"]["modes"], serde_json::json!(["static_bearer"]));
    assert_eq!(json["auth"]["issuer"], Value::Null);
    assert_eq!(json["auth"]["audience"], Value::Null);
    assert_eq!(json["auth"]["token_endpoint_origin"], Value::Null);
    assert_eq!(json["auth"]["principal_cache_scope"], "static-bearer");
    assert_eq!(json["auth"]["credential_generation"], "redacted");
    assert_eq!(
        json["streams"],
        serde_json::json!({"transport": "none", "resume": "none"})
    );
    assert_eq!(json.as_object().unwrap().len(), 8);
    assert!(
        !String::from_utf8(body.to_vec())
            .unwrap()
            .contains("secret-token")
    );
}
