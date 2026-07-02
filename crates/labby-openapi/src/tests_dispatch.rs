//! Dispatch happy/unknown/canary tests.
//!
//! These build a `SpecEntry` DIRECTLY (bypassing `OpenApiRegistry::load`) so the
//! `OperationHandle.base_url` can point at the wiremock server without tripping
//! the SSRF guard (127.0.0.1 is private). The SSRF guard itself is unit-tested in
//! `tests_ssrf.rs`; here we exercise dispatch logic in isolation.

use std::collections::HashMap;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::config::OpenApiCredential;
use crate::dispatch::dispatch_openapi_call;
use crate::registry::{OpenApiRegistry, OperationHandle, SpecEntry};

/// Build a single-op registry whose base_url is `base` (the mock URI) — bypassing
/// the SSRF guard so loopback dispatch can be tested in isolation.
fn registry_from_handle(label: &str, op: OperationHandle) -> OpenApiRegistry {
    let mut operations = HashMap::new();
    operations.insert(op.operation_id.clone(), op);
    let mut inner = HashMap::new();
    inner.insert(label.to_string(), SpecEntry { operations });
    OpenApiRegistry::from_map_for_test(inner)
}

fn get_user_handle(base: &str, credential: Option<OpenApiCredential>) -> OperationHandle {
    OperationHandle {
        operation_id: "getUser".into(),
        method: reqwest::Method::GET,
        path_template: "/users/{id}".into(),
        base_url: base.parse().unwrap(),
        credential,
    }
}

/// A dispatch client that does NOT apply the SSRF pin, so wiremock on 127.0.0.1
/// is reachable. Production uses `build_dispatch_client` + the per-request pin.
fn loopback_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap()
}

#[tokio::test]
async fn happy_path_calls_allowed_operation() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "7" })))
        .mount(&server)
        .await;

    let reg = registry_from_handle("vendor", get_user_handle(&server.uri(), None));
    let out = dispatch_openapi_call(
        &reg,
        &loopback_client(),
        "vendor",
        "getUser",
        json!({ "id": "7" }),
    )
    .await
    .unwrap();
    assert_eq!(out["id"], "7");
}

#[tokio::test]
async fn credential_injected_server_side() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/7"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer tok-abc",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "7" })))
        .mount(&server)
        .await;

    let handle = get_user_handle(
        &server.uri(),
        Some(OpenApiCredential::BearerToken("tok-abc".into())),
    );
    let reg = registry_from_handle("vendor", handle);
    // The params never carry the token — it is injected after the sandbox boundary.
    let out = dispatch_openapi_call(
        &reg,
        &loopback_client(),
        "vendor",
        "getUser",
        json!({ "id": "7" }),
    )
    .await
    .unwrap();
    assert_eq!(out["id"], "7");
}

#[tokio::test]
async fn unknown_operation_returns_unknown_action() {
    let server = MockServer::start().await;
    let reg = registry_from_handle("vendor", get_user_handle(&server.uri(), None));
    let err = dispatch_openapi_call(
        &reg,
        &loopback_client(),
        "vendor",
        "deleteUser",
        json!({ "id": "7" }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind(), "unknown_action");
}

#[tokio::test]
async fn unknown_label_returns_unknown_instance() {
    let err = dispatch_openapi_call(
        &OpenApiRegistry::default(),
        &loopback_client(),
        "nope",
        "getUser",
        json!({}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind(), "unknown_instance");
}

#[tokio::test]
async fn upstream_error_body_never_leaks_into_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/7"))
        .respond_with(
            ResponseTemplate::new(500).set_body_string("CANARY-9f3b-SECRET internal detail"),
        )
        .mount(&server)
        .await;

    let reg = registry_from_handle("vendor", get_user_handle(&server.uri(), None));
    let err = dispatch_openapi_call(
        &reg,
        &loopback_client(),
        "vendor",
        "getUser",
        json!({ "id": "7" }),
    )
    .await
    .unwrap_err();
    let tool_err: labby_runtime::error::ToolError = err.clone().into();
    for s in [
        format!("{err}"),
        format!("{err:?}"),
        format!("{tool_err:?}"),
        serde_json::to_string(&tool_err).unwrap(),
    ] {
        assert!(
            !s.contains("CANARY-9f3b-SECRET"),
            "response body leaked: {s}"
        );
    }
}
