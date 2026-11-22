#![cfg(feature = "openapi-mcp")]

//! Deliberate live check of the built stdio child and its production HTTPS
//! transport. Set LABBY_OPENAPI_MCP_SMOKE_BASE_URL to an operator-maintained,
//! public HTTPS fixture base (an optional path prefix is allowed). It must
//! serve GET <base>/items/labby-standalone-smoke over a publicly trusted
//! certificate, without redirects or credentials, returning HTTP 200 JSON:
//! {"id":"labby-standalone-smoke","status":"ok"}
//!
//! cargo test --locked -p labby --no-default-features --features openapi-mcp \
//!   --test standalone_mcp_https -- live_https_read_returns_structured_result --exact --ignored

use std::time::Duration;

use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Map, json};
use tokio::process::Command;

const FIXTURE_BASE_ENV: &str = "LABBY_OPENAPI_MCP_SMOKE_BASE_URL";
const FIXTURE_ID: &str = "labby-standalone-smoke";
const SPEC: &str = r#"{
  "openapi": "3.0.0",
  "info": {"title":"Labby HTTPS smoke fixture","version":"1"},
  "paths": {
    "/items/{id}": {
      "get": {
        "operationId":"getSmokeItem",
        "parameters":[{"in":"path","name":"id","required":true,"schema":{"type":"string"}}],
        "responses":{"200":{"description":"fixture JSON"}}
      }
    }
  }
}"#;

#[tokio::test]
#[ignore = "requires explicitly configured operator-maintained public HTTPS fixture"]
async fn live_https_read_returns_structured_result() {
    let base = std::env::var(FIXTURE_BASE_ENV)
        .expect("set LABBY_OPENAPI_MCP_SMOKE_BASE_URL to an approved fixture base");
    assert!(!base.is_empty(), "{FIXTURE_BASE_ENV} must not be empty");

    // Read the spec from disk so only the single tool invocation needs
    // network access. The child applies the ordinary production SSRF policy.
    let fixture = tempfile::tempdir().expect("private smoke spec directory");
    let spec_path = fixture.path().join("smoke.json");
    std::fs::write(&spec_path, SPEC).expect("write the smoke spec");
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby-openapi-mcp"));
    child
        .env_clear()
        .kill_on_drop(true)
        .arg("--spec-path")
        .arg(&spec_path)
        .arg(format!("--base-url={base}"))
        .arg("--allow-operation=getSmokeItem");

    tokio::time::timeout(Duration::from_secs(30), async {
        let client =
            ().serve(TokioChildProcess::new(child).expect("launch standalone MCP child"))
                .await
                .expect("negotiate standalone stdio MCP");
        let result = client
            .call_tool(
                CallToolRequestParams::new("getSmokeItem")
                    .with_arguments(Map::from_iter([("id".to_owned(), json!(FIXTURE_ID))])),
            )
            .await;
        client.cancel().await.expect("close standalone MCP child");
        let result = result.expect("call the fixture read operation");
        assert_ne!(result.is_error, Some(true), "fixture request failed");
        assert_eq!(
            result.structured_content,
            Some(json!({"result":{"id":FIXTURE_ID,"status":"ok"}})),
        );
    })
    .await
    .expect("standalone HTTPS smoke timed out");
}
