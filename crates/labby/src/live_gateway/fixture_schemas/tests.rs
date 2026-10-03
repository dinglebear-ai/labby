use super::*;
#[test]
fn schema_reads_are_metadata_only_and_names_are_json_literals() {
    let code = schema_read_code("github", &["quoted\"name".into()]).unwrap();
    assert!(code.contains("codemode.readResource(\"lab://gateway/github/schema\")"));
    assert!(code.contains("quoted\\\"name"));
    assert!(!code.contains("callTool"));
}
#[test]
fn decoding_preserves_missing_output_and_rejects_partial_or_wrong_authority() {
    let mut trace = json!({"result":{"name":"github","health":"healthy","tools":[{"name":"lookup","input_schema":{"type":"object"}}]}});
    let names = vec!["lookup".into()];
    assert!(
        decode_schema_trace(trace.clone(), "github", &names).unwrap()["github::lookup"]
            .output_schema
            .is_none()
    );
    trace["result"]["name"] = json!("other");
    assert!(decode_schema_trace(trace, "github", &names).is_err());
    assert!(decode_schema_trace(json!({"error_kind":"forbidden"}), "github", &names).is_err());
    assert!(
        decode_schema_trace(json!({"result_shape":{"truncated":true}}), "github", &names).is_err()
    );
}
#[tokio::test]
async fn selected_public_mcp_endpoint_supplies_both_schemas_without_http_actions() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    drop(rustls::crypto::ring::default_provider().install_default());
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/mcp"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(|request: &wiremock::Request| {
                let rpc: Value = serde_json::from_slice(&request.body).unwrap();
                let result = match rpc["method"].as_str().unwrap() {
                    "server/discover" => json!({"resultType":"complete", "supportedVersions":["2026-07-28"], "capabilities":{"tools":{}}, "serverInfo":{"name":"selected-mcp","version":"1"}, "ttlMs":0, "cacheScope":"private"}),
                    "initialize" => json!({"protocolVersion":"2026-07-28","capabilities":{"tools":{}},"serverInfo":{"name":"selected-mcp","version":"1"}}),
                    "notifications/initialized" => return ResponseTemplate::new(202),
                    "tools/call" => {
                        assert_eq!(rpc["params"]["name"], "codemode_read");
                        let code = rpc["params"]["arguments"]["code"].as_str().unwrap();
                        assert!(code.contains("lab://gateway/example/schema"));
                        assert!(!code.contains("callTool"));
                        json!({"resultType":"complete","content":[],"structuredContent":{"result":{"name":"example","health":"healthy","tools":[{
                            "name":"lookup","input_schema":{"type":"object","required":["query"],"properties":{"query":{"type":"string"}}},
                            "output_schema":{"type":"object","required":["items"],"properties":{"items":{"type":"array","items":{"type":"string"}}}}
                        }]}}})
                    },
                    other => panic!("unexpected RPC {other}"),
                };
                ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":rpc["id"],"result":result}))
            }).mount(&server).await;
    let live = LiveGateway {
        base_url: normalize_base_url(&server.uri()).unwrap(),
        explicit: true,
        source: "test",
        token: Some("test-token".into()),
        team_id: None,
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        dispatch_timeout: SCHEMA_DEADLINE,
        actions: None,
    };
    let contracts = live
        .fixture_tool_schemas(&["example::lookup".into()])
        .await
        .unwrap();
    let contract = &contracts["example::lookup"];
    assert!(contract.input_schema.is_some());
    let response = labby_codemode::snippet::schemas::generate_response(
        contract.output_schema.as_ref().unwrap(),
        labby_codemode::snippet::schemas::FixtureVariant::Populated,
    )
    .unwrap();
    assert_eq!(response, json!({"items":["synthetic"]}));
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| request.url.path() == "/mcp")
    );
}

/// Opt-in live proof: inspect native MCP contracts without invoking tools.
#[tokio::test]
#[ignore = "requires an explicitly selected authenticated live gateway"]
async fn live_native_output_schema_generates_and_validates_synthetic_response() {
    use labby_codemode::snippet::schemas::{FixtureVariant, generate_response, validate_value};
    drop(rustls::crypto::ring::default_provider().install_default());
    let mut config = crate::config::load().unwrap();
    let server =
        std::env::var("LABBY_SCHEMA_LIVE_SERVER").expect("explicit live schema target required");
    config.cli_target =
        crate::config::cli::select(&config.cli, Some(&server), None, false).unwrap();
    let live = detect(&config, "cli")
        .await
        .unwrap()
        .expect("selected live gateway");
    let service = live.connect_service_bounded(()).await.unwrap();
    let peer = service.peer().clone();
    let (result, cleanup) = bounded_codemode_call_and_cleanup(
        async {
            let mut cursor = None;
            let mut seen = BTreeSet::new();
            let mut total_bytes = 0;
            let mut discovered = Vec::new();
            for _ in 0..32 {
                let page = peer
                    .list_tools(cursor.clone().map(|cursor| {
                        rmcp::model::PaginatedRequestParams::default().with_cursor(Some(cursor))
                    }))
                    .await?;
                total_bytes += serde_json::to_vec(&page).unwrap().len();
                assert!(total_bytes <= 8 * 1024 * 1024, "native catalog byte budget");
                for tool in page.tools {
                    let Some(schema) = tool.output_schema.as_ref() else {
                        continue;
                    };
                    let schema = Value::Object((**schema).clone());
                    let Ok(response) = generate_response(&schema, FixtureVariant::Minimal) else {
                        continue;
                    };
                    validate_value(&response, &schema).unwrap();
                    discovered.push((
                        tool.name.to_string(),
                        Value::Object((*tool.input_schema).clone()),
                        schema,
                        response,
                    ));
                }
                cursor = page.next_cursor;
                if cursor.is_none() {
                    return Ok::<_, rmcp::ServiceError>(discovered);
                }
                assert!(
                    seen.insert(cursor.clone()),
                    "repeated native catalog cursor"
                );
            }
            panic!("native catalog page budget")
        },
        SCHEMA_DEADLINE,
        MCP_CLEANUP_TIMEOUT,
        service.cancel(),
    )
    .await;
    cleanup.unwrap();
    let discovered = result.unwrap();
    assert!(
        !discovered.is_empty(),
        "no live typed output contract supported by generator"
    );
    for (name, input, output, response) in discovered {
        println!(
            "live native MCP tool={name}; input_contract=true; output_contract=true; synthetic_response_validated=true"
        );
        // Preserve non-secret contracts as qualification evidence in the caller's requested directory.
        if let Ok(dir) = std::env::var("LABBY_SCHEMA_EVIDENCE_DIR") {
            let file = std::path::Path::new(&dir).join(format!("{name}.schema.json"));
            if name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                std::fs::write(file, serde_json::to_vec_pretty(&json!({"input_schema":input,"output_schema":output,"synthetic_response":response})).unwrap()).unwrap();
            }
        }
    }
}

#[tokio::test]
async fn legacy_resource_output_is_supplemented_only_for_verified_native_identity() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    drop(rustls::crypto::ring::default_provider().install_default());
    for (resolved_id, expected_output) in [("example::lookup", true), ("other::lookup", false)] {
        let server = MockServer::start().await;
        let resolved_id = resolved_id.to_owned();
        Mock::given(method("POST")).and(path("/mcp"))
            .respond_with(move |request: &wiremock::Request| {
                let rpc: Value = serde_json::from_slice(&request.body).unwrap();
                let input = json!({"type":"object"});
                let result = match rpc["method"].as_str().unwrap() {
                    "server/discover" => json!({"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"serverInfo":{"name":"selected","version":"1"},"ttlMs":0,"cacheScope":"private"}),
                    "tools/list" => json!({"tools":[{"name":"lookup","inputSchema":input,"outputSchema":{"type":"object","required":["count"],"properties":{"count":{"type":"integer","minimum":0}}},"annotations":{"readOnlyHint":true}}]}),
                    "tools/call" => {
                        assert_eq!(rpc["params"]["name"],"codemode_read");
                        let code=rpc["params"]["arguments"]["code"].as_str().unwrap();
                        assert!(!code.contains("callTool"));
                        let payload = if code.contains("codemode.readResource") {
                            json!({"name":"example","health":"healthy","tools":[{"name":"lookup","input_schema":input}]})
                        } else {
                            assert!(code.contains(r#"codemode.describe("lookup")"#));
                            json!({"id":resolved_id,"schema_status":"complete"})
                        };
                        json!({"resultType":"complete","content":[],"structuredContent":{"result":payload}})
                    },
                    other => panic!("unexpected RPC {other}"),
                };
                ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":rpc["id"],"result":result}))
            }).mount(&server).await;
        let live = LiveGateway {
            base_url: normalize_base_url(&server.uri()).unwrap(),
            explicit: true,
            source: "test",
            token: None,
            team_id: None,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            dispatch_timeout: SCHEMA_DEADLINE,
            actions: None,
        };
        let contracts = live
            .fixture_tool_schemas(&["example::lookup".into()])
            .await
            .unwrap();
        assert_eq!(
            contracts["example::lookup"].output_schema.is_some(),
            expected_output
        );
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|r| r.url.path() == "/mcp")
        );
    }
}

#[tokio::test]
async fn native_schema_listing_rejects_incomplete_and_over_budget_catalogs() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    drop(rustls::crypto::ring::default_provider().install_default());
    for (case, expected) in [
        ("cursor", "native MCP schema pagination incomplete"),
        ("duplicate", "duplicate native tool identity"),
        ("bytes", "native MCP schema catalog exceeds bounds"),
        ("items", "native MCP schema catalog exceeds bounds"),
        (
            "permission",
            "native tool listing for selected gateway failed: MCP -32603: permission denied",
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/mcp"))
            .respond_with(move |request: &wiremock::Request| {
                let rpc: Value = serde_json::from_slice(&request.body).unwrap();
                let result = match rpc["method"].as_str().unwrap() {
                    "server/discover" => json!({"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"serverInfo":{"name":"selected","version":"1"},"ttlMs":0,"cacheScope":"private"}),
                    "tools/call" => {
                        assert_eq!(rpc["params"]["name"], "codemode_read");
                        assert!(rpc["params"]["arguments"]["code"].as_str().unwrap().contains("codemode.readResource"));
                        json!({"resultType":"complete","content":[],"structuredContent":{"result":{"name":"example","health":"healthy","tools":[{"name":"lookup","input_schema":{"type":"object"}}]}}})
                    },
                    "tools/list" => match case {
                        "permission" => return ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":rpc["id"],"error":{"code":-32603,"message":"permission denied","data":{"source":"must not be exposed"}}})),
                        "cursor" => json!({"tools":[],"nextCursor":"repeat"}),
                        "duplicate" => json!({"tools":[{"name":"lookup","inputSchema":{}},{"name":"lookup","inputSchema":{}}]}),
                        "bytes" => {
                            let second = rpc["params"]["cursor"] == "next";
                            json!({"tools":[{"name":if second {"second"} else {"first"},"inputSchema":{},"description":"x".repeat(4 * 1024 * 1024 + 1024)}],"nextCursor":if second {None} else {Some("next")}})
                        },
                        "items" => json!({"tools":(0..4097).map(|i| json!({"name":format!("tool_{i}"),"inputSchema":{}})).collect::<Vec<_>>()}),
                        _ => unreachable!(),
                    },
                    other => panic!("unexpected RPC {other}"),
                };
                ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":rpc["id"],"result":result}))
            }).mount(&server).await;
        let live = LiveGateway {
            base_url: normalize_base_url(&server.uri()).unwrap(),
            explicit: true,
            source: "test",
            token: None,
            team_id: None,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            dispatch_timeout: SCHEMA_DEADLINE,
            actions: None,
        };
        let error = live
            .fixture_tool_schemas(&["example::lookup".into()])
            .await
            .unwrap_err();
        match error {
            ToolError::Sdk { sdk_kind, message } => {
                assert_eq!(sdk_kind, "schema_unavailable");
                assert_eq!(message, expected, "case {case}");
            }
            other => panic!("case {case} returned unexpected error: {other:?}"),
        }
        let requests = server.received_requests().await.unwrap();
        assert!(requests.iter().all(|request| request.url.path() == "/mcp"));
        assert!(
            requests.len() <= 4,
            "discovery must stop at the failed bound"
        );
    }
}

#[test]
fn schema_transport_diagnostics_are_contextual_bounded_and_redacted() {
    let error = rmcp::ServiceError::McpError(rmcp::model::ErrorData::new(
        rmcp::model::ErrorCode(-32603),
        "permission rejected Authorization: Bearer synthetic-secret",
        Some(
            json!({"source":"private request source", "params":{"secret":"opaque private value"}}),
        ),
    ));
    let ToolError::Sdk { sdk_kind, message } =
        schema_transport_error("schema read", "example", &error)
    else {
        panic!("expected schema error")
    };
    assert_eq!(sdk_kind, "schema_unavailable");
    assert!(message.contains("schema read for example"));
    assert!(message.contains("MCP -32603: permission rejected"));
    assert!(!message.contains("synthetic-secret"));
    assert!(!message.contains("private request source"));
    assert!(!message.contains("opaque private value"));
    let error = rmcp::ServiceError::McpError(rmcp::model::ErrorData::new(
        rmcp::model::ErrorCode(-32603),
        "x".repeat(2048),
        None,
    ));
    let ToolError::Sdk { message, .. } = schema_transport_error("schema read", "example", &error)
    else {
        panic!("expected schema error")
    };
    assert_eq!(message.chars().count(), 1024);
}
