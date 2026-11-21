#![cfg(feature = "standalone-mcp")]

use std::{path::Path, time::Duration};

use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::json;
use tokio::process::Command;

fn command(spec: &Path, origin: &str, operation: &str) -> Command {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby-openapi-mcp"));
    child.kill_on_drop(true).arg("--spec-path").arg(spec).args([
        "--base-url",
        origin,
        "--allow-operation",
        operation,
    ]);
    child
}

const SPEC: &str = r#"{
  "openapi": "3.0.0",
  "paths": {
    "/items/{id}": {"get": {"operationId":"getItem","summary":"Fetch an item",
      "parameters":[{"in":"path","name":"id","required":true,"schema":{"type":"string"}}]}},
    "/items": {"delete":{"operationId":"deleteItems"}}
  }
}"#;

#[tokio::test]
async fn independent_mcp_stdio_negotiate_discover_and_call() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("openapi.json");
        std::fs::write(&path, SPEC).unwrap();
        let client = ()
            .serve(
                TokioChildProcess::new(command(&path, "https://example.com", "getItem")).unwrap(),
            )
            .await
            .unwrap();
        let info = client.peer_info().unwrap();
        assert_eq!(info.server_info.as_ref().unwrap().name, "labby-openapi-mcp");
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_none());
        assert!(info.capabilities.prompts.is_none());
        let listing = client.list_tools(None).await.unwrap();
        assert_eq!(listing.tools.len(), 1);
        assert_eq!(listing.tools[0].name, "getItem");
        assert_eq!(
            listing.tools[0].input_schema["properties"]["id"]["type"],
            "string"
        );
        assert_eq!(listing.tools[0].input_schema["required"], json!(["id"]));
        assert!(
            client
                .call_tool(CallToolRequestParams::new("deleteItems"))
                .await
                .is_err()
        );
        assert!(
            client
                .call_tool(CallToolRequestParams::new("getItem"))
                .await
                .is_err()
        );

        // A decoded request crosses the MCP dispatcher and reaches the real
        // guarded path validation; it fails locally before DNS/HTTP.
        let request = CallToolRequestParams::new("getItem")
            .with_arguments(json!({"id":".."}).as_object().unwrap().clone());
        let response = client.call_tool(request).await.unwrap();
        assert_eq!(response.is_error, Some(true));
        client.cancel().await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn refuses_mutating_tool_unless_operator_explicitly_enables_it() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("openapi.json");
    std::fs::write(&path, SPEC).unwrap();
    let out = command(&path, "https://example.com", "deleteItems")
        .output()
        .await
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(out.stdout, Vec::<u8>::new());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--allow-write"));
}
