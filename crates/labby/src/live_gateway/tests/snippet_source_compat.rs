//! A receipt-capable older daemon ignores unknown fields; negotiate before effects.
use super::test_gateway;
use crate::dispatch::snippets::store::SnippetSourceFormat;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

struct ReceiptDaemon {
    body_supported: bool,
    format_supported: bool,
    executions: Arc<AtomicUsize>,
}

impl Respond for ReceiptDaemon {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let request: Value = request.body_json().unwrap();
        let Some(id) = request.get("id") else {
            return ResponseTemplate::new(202);
        };
        let result = match request["method"].as_str().unwrap() {
            "server/discover" => json!({
                "resultType":"complete", "supportedVersions":["2026-07-28"],
                "capabilities":{"tools":{}},
                "serverInfo":{"name":"receipt-capable-daemon","version":"legacy"},
                "ttlMs":0,"cacheScope":"private"
            }),
            "initialize" => json!({
                "protocolVersion":request["params"]["protocolVersion"],
                "capabilities":{"tools":{}},
                "serverInfo":{"name":"receipt-capable-daemon","version":"legacy"}
            }),
            "ping" => json!({}),
            "tools/call" => {
                assert_eq!(request["params"]["name"], "snippets");
                let arguments = &request["params"]["arguments"];
                let data = match arguments["action"].as_str().unwrap() {
                    "schema" => {
                        assert_eq!(arguments["params"]["action"], "snippets.exec");
                        let mut schema = crate::dispatch::helpers::action_schema(
                            crate::dispatch::snippets::ACTIONS,
                            "snippets.exec",
                        )
                        .unwrap();
                        schema["params"].as_array_mut().unwrap().retain(|param| {
                            (param["name"] != "body" || self.body_supported)
                                && (param["name"] != "source_format" || self.format_supported)
                        });
                        schema
                    }
                    "snippets.exec" => {
                        self.executions.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(arguments["params"]["name"], "same-name");
                        // An older daemon accepts the request but silently runs its
                        // conflicting saved source; receipt_status alone proves nothing.
                        let source = if self.body_supported && self.format_supported {
                            assert!(
                                arguments["params"]["body"]
                                    .as_str()
                                    .unwrap()
                                    .contains("local-selected")
                            );
                            "local-selected"
                        } else {
                            "daemon-conflicting"
                        };
                        json!({"execution_id":"snippet_compat","receipt_status":"persisted","result":{"source":source},"calls":[]})
                    }
                    action => panic!("unexpected fixture action {action}"),
                };
                json!({"resultType":"complete","content":[],"structuredContent":{"ok":true,"service":"snippets","data":data},"isError":false})
            }
            method => panic!("unexpected fixture request {method}"),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":id,"result":result}))
    }
}

async fn daemon(body_supported: bool, format_supported: bool) -> (MockServer, Arc<AtomicUsize>) {
    let server = MockServer::start().await;
    let executions = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("authorization", "Bearer synthetic-compat-token"))
        .respond_with(ReceiptDaemon {
            body_supported,
            format_supported,
            executions: executions.clone(),
        })
        .mount(&server)
        .await;
    (server, executions)
}

#[tokio::test]
async fn old_daemon_cannot_execute_conflicting_saved_source_for_local_run() {
    for (body, format) in [(false, false), (true, false)] {
        let (server, executions) = daemon(body, format).await;
        let gateway = test_gateway(server.uri(), Some("synthetic-compat-token".into()));
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            gateway.call_snippet_source(
                "same-name",
                "async () => ({source:'local-selected'})",
                SnippetSourceFormat::JavaScript,
                json!({}),
            ),
        )
        .await
        .expect("compatibility negotiation must be bounded");
        assert!(
            result.is_err(),
            "older daemon silently executed different source: {result:?}"
        );
        assert_eq!(
            executions.load(Ordering::SeqCst),
            0,
            "compatibility rejection must precede execution"
        );
    }
}

#[tokio::test]
async fn current_daemon_accepts_local_source_after_inline_schema_negotiation() {
    let (server, executions) = daemon(true, true).await;
    let gateway = test_gateway(server.uri(), Some("synthetic-compat-token".into()));
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        gateway.call_snippet_source(
            "same-name",
            "async () => ({source:'local-selected'})",
            SnippetSourceFormat::JavaScript,
            json!({}),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response["result"]["source"], "local-selected");
    assert_eq!(response["receipt_status"], "persisted");
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}
