#![cfg(feature = "openapi-mcp")]

use std::{path::Path, time::Duration};

use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ErrorCode},
    service::ServiceError,
    transport::TokioChildProcess,
};
use serde_json::{Value, json};
use tokio::process::Command;

// The harness never makes an upstream request. Use a reserved hostname so a
// regression cannot turn these preflight cases into a call to a real API.
const FIXTURE_ORIGIN: &str = "https://catalog.example.invalid";
const MCP_TIMEOUT: Duration = Duration::from_secs(10);
const SPEC: &str = r#"{
  "openapi": "3.0.0",
  "paths": {
    "/items/{id}": {
      "parameters": [{"in":"path","name":"id","required":true,"schema":{"type":"string"}}],
      "get": {
        "operationId":"getItem",
        "summary":"Fetch an item",
        "parameters":[
          {"in":"query","name":"page","required":true,"schema":{"type":"integer"}},
          {"in":"query","name":"includeTags","schema":{"type":"boolean"}},
          {"in":"query","name":"minScore","schema":{"type":"number"}}
        ]
      },
      "head": {"operationId":"headItem"},
      "patch": {
        "operationId":"updateItem",
        "requestBody":{"required":true,"content":{"application/json":{"schema":{
          "type":"object",
          "properties":{"title":{"type":"string"},"published":{"type":"boolean"}},
          "required":["title"]
        }}}}
      }
    },
    "/items": {
      "get":{"operationId":"listItems"},
      "delete":{"operationId":"deleteItems"}
    },
    "/duplicate-a": {"get":{"operationId":"duplicateItem"}},
    "/duplicate-b": {"get":{"operationId":"duplicateItem"}}
  }
}"#;

fn command(spec: &Path, operations: &[&str]) -> Command {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby-openapi-mcp"));
    child
        .kill_on_drop(true)
        .arg("--spec-path")
        .arg(spec)
        .args(["--base-url", FIXTURE_ORIGIN]);
    for operation in operations {
        child.args(["--allow-operation", operation]);
    }
    child
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("openapi.json");
    std::fs::write(&path, SPEC).unwrap();
    (tmp, path)
}

fn call(name: &'static str, args: Value) -> CallToolRequestParams {
    CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone())
}

fn assert_invalid_params(error: ServiceError, message: &str) {
    assert!(
        matches!(&error, ServiceError::McpError(data)
            if data.code == ErrorCode::INVALID_PARAMS && data.message == message),
        "expected invalid params ({message}), got {error:?}"
    );
}

#[tokio::test]
async fn reports_the_standalone_executable_name_and_workspace_version() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby-openapi-mcp"));
    let output = tokio::time::timeout(
        MCP_TIMEOUT,
        child.kill_on_drop(true).arg("--version").output(),
    )
    .await
    .expect("version command must exit")
    .expect("run the standalone executable");
    assert!(output.status.success());
    assert_eq!(output.stderr.as_slice(), b"");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        concat!("labby-openapi-mcp ", env!("CARGO_PKG_VERSION")),
    );
}

#[tokio::test]
async fn independent_mcp_stdio_negotiate_discover_and_call() {
    tokio::time::timeout(MCP_TIMEOUT, async {
        let (_tmp, path) = fixture();
        let client =
            ().serve(TokioChildProcess::new(command(&path, &["getItem", "headItem"])).unwrap())
                .await
                .unwrap();
        let info = client.peer_info().unwrap();
        assert_eq!(info.server_info.as_ref().unwrap().name, "labby-openapi-mcp");
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_none());
        assert!(info.capabilities.prompts.is_none());

        let listing = client.list_tools(None).await.unwrap();
        let names: Vec<_> = listing
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect();
        assert_eq!(names, ["getItem", "headItem"]);
        let read = &listing.tools[0];
        assert_eq!(read.description.as_deref(), Some("Fetch an item"));
        assert_eq!(
            json!(*read.input_schema),
            json!({
                "type": "object",
                "properties": {
                    "id": {"type":"string"},
                    "page": {"type":"integer"},
                    "includeTags": {"type":"boolean"},
                    "minScore": {"type":"number"}
                },
                "required": ["id", "page"],
                "additionalProperties": false
            })
        );
        assert_eq!(listing.tools[1].input_schema["required"], json!(["id"]));
        for tool in &listing.tools {
            let annotations = tool.annotations.as_ref().unwrap();
            assert_eq!(annotations.read_only_hint, Some(true));
            assert_eq!(annotations.destructive_hint, Some(false));
            assert_eq!(annotations.open_world_hint, Some(true));
        }

        // Allowlisting is literal and restricts calls, including other GETs.
        for name in ["listItems", "deleteItems", "getitem"] {
            assert_invalid_params(
                client
                    .call_tool(CallToolRequestParams::new(name))
                    .await
                    .unwrap_err(),
                "operation is not exposed",
            );
        }
        for (args, message) in [
            (
                json!({"id":"item-7"}),
                "a required operation argument is missing",
            ),
            (
                json!({"id":"item-7","page":1.5}),
                "operation argument has an unexpected name or type",
            ),
            (
                json!({"id":"item-7","page":1,"includeTags":"true"}),
                "operation argument has an unexpected name or type",
            ),
            (
                json!({"id":"item-7","page":1,"minScore":null}),
                "operation argument has an unexpected name or type",
            ),
            (
                json!({"id":"item-7","page":1,"unlisted":true}),
                "operation argument has an unexpected name or type",
            ),
        ] {
            assert_invalid_params(
                client.call_tool(call("getItem", args)).await.unwrap_err(),
                message,
            );
        }

        // This existing fixture crosses the dispatcher boundary, then fails
        // local path validation before DNS/HTTP rather than at MCP validation.
        let request = call("getItem", json!({"id":"..","page":1}));
        let response = client.call_tool(request).await.unwrap();
        assert_eq!(response.is_error, Some(true));
        assert!(response.structured_content.is_none());
        client.cancel().await.unwrap();
    })
    .await
    .unwrap();
}

async fn assert_startup_rejected(path: &Path, operations: &[&str], message: &str) {
    let out = tokio::time::timeout(MCP_TIMEOUT, command(path, operations).output())
        .await
        .expect("invalid configuration must not leave the stdio child waiting")
        .unwrap();
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "startup must not emit malformed MCP frames"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(message),
        "unexpected startup diagnostic: {stderr}"
    );
}

#[tokio::test]
async fn refuses_mutating_tool_unless_operator_explicitly_enables_it() {
    let (_tmp, path) = fixture();
    assert_startup_rejected(
        &path,
        &["getItem", "deleteItems"],
        "operation deleteItems is not read-only; review it and pass --allow-write",
    )
    .await;
}

#[tokio::test]
async fn explicit_write_keeps_exact_allowlist_and_typed_body_schema() {
    tokio::time::timeout(MCP_TIMEOUT, async {
        let (_tmp, path) = fixture();
        let mut child = command(&path, &["updateItem"]);
        child.arg("--allow-write");
        let client = ().serve(TokioChildProcess::new(child).unwrap()).await.unwrap();
        let listing = client.list_tools(None).await.unwrap();
        assert_eq!(listing.tools.len(), 1);
        let write = &listing.tools[0];
        assert_eq!(write.name, "updateItem");
        let annotations = write.annotations.as_ref().unwrap();
        assert_eq!(annotations.read_only_hint, Some(false));
        assert_eq!(annotations.destructive_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(true));
        assert_eq!(
            json!(*write.input_schema),
            json!({
                "type": "object",
                "properties": {
                    "id": {"type":"string"},
                    "title": {"type":"string"},
                    "published": {"type":"boolean"}
                },
                "required": ["id", "title"],
                "additionalProperties": false
            })
        );
        assert_invalid_params(
            client
                .call_tool(CallToolRequestParams::new("deleteItems"))
                .await
                .unwrap_err(),
            "operation is not exposed",
        );
        for (args, message) in [
            (
                json!({"id":"item-7","published":true}),
                "a required operation argument is missing",
            ),
            (
                json!({"id":"item-7","title":42}),
                "operation argument has an unexpected name or type",
            ),
        ] {
            assert_invalid_params(
                client
                    .call_tool(call("updateItem", args))
                    .await
                    .unwrap_err(),
                message,
            );
        }
        client.cancel().await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn rejects_unknown_or_nonunique_operation_ids_at_startup() {
    let (_tmp, path) = fixture();
    assert_startup_rejected(&path, &["getitem"], "operation getitem is absent").await;
    assert_startup_rejected(
        &path,
        &["duplicateItem"],
        "operation duplicateItem has duplicate IDs",
    )
    .await;
}
