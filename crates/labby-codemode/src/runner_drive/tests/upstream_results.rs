//! Full-result and automatic advisory projection through the real driver.
use super::*;
use crate::host::{ResolvedSnippet, ToolsRender};
use crate::protocol::CodeModeRunnerInput;
use std::sync::atomic::{AtomicUsize, Ordering};

struct McpHost {
    inner: NoopHost,
    effects: AtomicUsize,
}
impl CodeModeHost for McpHost {
    async fn list_tools(
        &self,
        _: &CodeModeCaller,
        _: CodeModeSurface,
        _: &ToolScope,
        _: bool,
        _: bool,
    ) -> Result<ToolsRender, ToolError> {
        Ok(ToolsRender::empty())
    }
    async fn call_tool(
        &self,
        _: &str,
        params: Value,
        caller: &CodeModeCaller,
        _: CodeModeSurface,
        _: &ToolScope,
        _: ExecCtx,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        self.effects.fetch_add(1, Ordering::SeqCst);
        if params.get("fail") == Some(&json!(true)) {
            return Err(CodeModeCallError::new("invalid_param", "fixture rejection"));
        }
        let value = params.get("value").cloned().unwrap_or(json!({"rows":[1]}));
        if params.get("native") == Some(&json!(true)) {
            return Ok(ToolCallOutcome {
                value,
                full_result: None,
                ui: None,
            });
        }
        let large = if params.get("large") == Some(&json!(true)) {
            "x".repeat(9 * 1024 * 1024)
        } else {
            String::new()
        };
        let notice = caller.subject().unwrap_or("Independent advisory");
        let mut full = json!({"structuredContent":value,"content":[
            {"type":"text","text":value.to_string()},
            {"type":"text","text":notice},
            {"type":"resource_link","uri":"test://resource","name":"reference"}
        ],"_meta":{"fixture":true}});
        if !large.is_empty() {
            full["content"][0] = json!({"type":"image","data":large,"mimeType":"image/png"});
        }
        Ok(ToolCallOutcome {
            value,
            full_result: Some(full),
            ui: None,
        })
    }
    async fn read_resource(
        &self,
        _: String,
        _: &CodeModeCaller,
        _: CodeModeSurface,
        _: &ToolScope,
    ) -> Result<Value, ToolError> {
        self.effects.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"contents":[]}))
    }
    async fn get_prompt(
        &self,
        _: String,
        _: Value,
        _: &CodeModeCaller,
        _: CodeModeSurface,
        _: &ToolScope,
    ) -> Result<Value, ToolError> {
        self.effects.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"messages":[]}))
    }
    async fn resolve_snippet(
        &self,
        name: &str,
        input: Value,
    ) -> Result<ResolvedSnippet, ToolError> {
        self.inner.resolve_snippet(name, input).await
    }
    async fn semantic_rank(
        &self,
        query: String,
        top_k: usize,
        kinds: &[crate::CodeModeCatalogKind],
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> Result<Vec<(String, f32)>, ToolError> {
        self.inner
            .semantic_rank(query, top_k, kinds, caller, surface, scope)
            .await
    }
    async fn config(&self) -> labby_runtime::CodeModeConfig {
        self.inner.config().await
    }
    fn runner_pool(&self) -> &RunnerPool {
        self.inner.runner_pool()
    }
    fn openapi_registry(&self) -> labby_openapi::OpenApiRegistry {
        self.inner.openapi_registry()
    }
    fn openapi_http_client(&self) -> reqwest::Client {
        self.inner.openapi_http_client()
    }
}

async fn run(
    host: &McpHost,
    config: &RunnerConfig,
    requests: Vec<Value>,
    throws: bool,
) -> (DriveOutcome, Vec<CodeModeRunnerInput>) {
    let capture = tempfile::NamedTempFile::new().unwrap();
    let mut script = String::from("IFS= read -r _\n");
    for request in requests {
        script.push_str(&format!("printf '%s\\n' '{}'\nIFS= read -r reply || exit 21\nprintf '%s\\n' \"$reply\" >>'{}'\n", request.to_string().replace('\'', "'\"'\"'"), capture.path().display()));
    }
    let terminal = if throws {
        json!({"type":"error","kind":"execution_error","message":"script failed after write"})
    } else {
        json!({"type":"done","result":{"state":"json","value":{"selected":1}},"logs":[]})
    };
    script.push_str(&format!("printf '%s\\n' '{}'\nIFS= read -r _\n", terminal));
    let mut runner = PooledRunner::spawn_stub_script(&script).unwrap();
    let outcome = CodeModeBroker::new(Some(host))
        .drive_runner(
            &mut runner,
            config,
            tokio::time::Instant::now() + config.timeout,
        )
        .await;
    let replies = std::fs::read_to_string(capture.path())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (outcome, replies)
}
fn request(seq: u64, full: bool, params: Value) -> Value {
    json!({"type":"tool_call","seq":seq,"id":"fixture::lookup","params":params,"full_result":full})
}
fn host() -> McpHost {
    McpHost {
        inner: NoopHost::default(),
        effects: AtomicUsize::new(0),
    }
}

#[tokio::test]
async fn full_results_and_convenient_falsy_values_share_dispatch_and_projection() {
    let host = host();
    let config = test_config(Duration::from_secs(10));
    for value in [
        json!({"rows":[1]}),
        json!(false),
        json!(0),
        json!(null),
        json!(""),
    ] {
        let (outcome, replies) = run(
            &host,
            &config,
            vec![
                request(0, false, json!({"value":value})),
                request(1, true, json!({"value":value})),
            ],
            false,
        )
        .await;
        let DriveOutcome::Completed(response) = outcome else {
            panic!("completed")
        };
        let CodeModeRunnerInput::ToolResult { result: plain, .. } = &replies[0] else {
            panic!("plain result")
        };
        assert_eq!(plain, &value);
        let CodeModeRunnerInput::ToolResult { result: full, .. } = &replies[1] else {
            panic!("full result")
        };
        assert_eq!(full["structuredContent"], value);
        assert_eq!(full["content"].as_array().unwrap().len(), 3);
        assert_eq!(full["_meta"]["fixture"], true);
        assert_eq!(response.result, Some(json!({"selected":1})));
        for row in &response.calls {
            let content = row.upstream_content.as_ref().unwrap();
            assert_eq!(content["content"][0]["text"], "Independent advisory");
            assert_eq!(content["content"][1]["type"], "resource_link");
            assert_eq!(content["omitted_blocks"], 0);
        }
        assert_eq!(
            crate::code_mode_execute_trace(&response)["calls"][0]["upstream_content"]["content"][0]
                ["text"],
            "Independent advisory"
        );
    }
    assert_eq!(host.effects.load(Ordering::SeqCst), 10);
}

#[tokio::test]
async fn success_then_script_throw_preserves_advisory_and_never_replays_write() {
    let host = host();
    let (outcome, _) = run(
        &host,
        &test_config(Duration::from_secs(10)),
        vec![request(0, false, json!({}))],
        true,
    )
    .await;
    let DriveOutcome::ExecutionError(error) = outcome else {
        panic!("script error")
    };
    assert_eq!(host.effects.load(Ordering::SeqCst), 1);
    assert!(error.calls()[0].ok);
    assert_eq!(
        error.calls()[0].upstream_content.as_ref().unwrap()["content"][0]["text"],
        "Independent advisory"
    );
}

#[tokio::test]
async fn full_flag_does_not_bypass_scope_or_reserved_provider_boundaries() {
    let host = host();
    let mut config = test_config(Duration::from_secs(10));
    config.capability_filter = ToolScope::new(vec![], vec!["other::tool".into()]);
    for full in [false, true] {
        let (_, replies) = run(&host, &config, vec![request(0, full, json!({}))], false).await;
        assert!(matches!(&replies[0], CodeModeRunnerInput::ToolError { .. }));
    }
    config.capability_filter = ToolScope::default();
    for id in [
        "__lab_internal::list_artifacts",
        " __lab_internal::read_resource",
        "__lab_internal::get_prompt ",
        "__lab_internal ::read_resource",
        "state::get",
        "git::status",
        "openapi::demo.op",
        "snippet::saved",
    ] {
        let (_, replies) = run(
            &host,
            &config,
            vec![json!({"type":"tool_call","seq":0,"id":id,"params":{"uri":"fixture://read","prompt":"fixture/prompt"},"full_result":true})],
            false,
        )
        .await;
        let CodeModeRunnerInput::ToolError { error, .. } = &replies[0] else {
            panic!("reject {id}")
        };
        assert_eq!(error.kind(), "invalid_param");
        assert_eq!(
            host.effects.load(Ordering::SeqCst),
            0,
            "{id} must not invoke a host"
        );
    }
    assert_eq!(host.effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_tool_results_keep_the_same_error_and_do_not_create_content() {
    let host = host();
    let (outcome, replies) = run(
        &host,
        &test_config(Duration::from_secs(10)),
        vec![
            request(0, false, json!({"fail":true})),
            request(1, true, json!({"fail":true})),
        ],
        false,
    )
    .await;
    let DriveOutcome::Completed(response) = outcome else {
        panic!("completed")
    };
    for reply in replies {
        let CodeModeRunnerInput::ToolError { error, .. } = reply else {
            panic!("error")
        };
        assert_eq!(error.kind(), "invalid_param");
    }
    assert!(
        response
            .calls
            .iter()
            .all(|call| !call.ok && call.upstream_content.is_none())
    );
}

#[tokio::test]
async fn concurrent_caller_content_is_isolated_between_executions() {
    let host = host();
    let mut left = test_config(Duration::from_secs(10));
    let mut right = test_config(Duration::from_secs(10));
    left.caller = CodeModeCaller::Scoped {
        capabilities: crate::CodeModeCallerCapabilities {
            can_read: true,
            can_execute: true,
            ..Default::default()
        },
        sub: Some("caller-left".into()),
    };
    right.caller = CodeModeCaller::Scoped {
        capabilities: crate::CodeModeCallerCapabilities {
            can_read: true,
            can_execute: true,
            ..Default::default()
        },
        sub: Some("caller-right".into()),
    };
    let (left, right) = tokio::join!(
        run(
            &host,
            &left,
            vec![request(0, false, json!({})), request(1, true, json!({}))],
            false
        ),
        run(&host, &right, vec![request(0, true, json!({}))], false)
    );
    for (result, expected, foreign) in [
        (left, "caller-left", "caller-right"),
        (right, "caller-right", "caller-left"),
    ] {
        let DriveOutcome::Completed(response) = result.0 else {
            panic!("completed")
        };
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(encoded.contains(expected));
        assert!(!encoded.contains(foreign));
    }
    assert_eq!(host.effects.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn full_envelope_bytes_share_the_existing_per_call_ceiling() {
    let host = host();
    let (outcome, replies) = run(
        &host,
        &test_config(Duration::from_secs(20)),
        vec![request(0, true, json!({"large":true}))],
        false,
    )
    .await;
    let DriveOutcome::Completed(response) = outcome else {
        panic!("caught byte rejection")
    };
    let CodeModeRunnerInput::ToolError { error, .. } = &replies[0] else {
        panic!("full envelope exceeds byte ceiling")
    };
    assert_eq!(error.kind(), "result_too_large");
    assert_eq!(host.effects.load(Ordering::SeqCst), 1);
    assert_eq!(
        response.calls[0].error_kind.as_deref(),
        Some("result_too_large")
    );
    let projected = response.calls[0].upstream_content.as_ref().unwrap();
    assert_eq!(projected["omitted_blocks"], 1);
    assert_eq!(projected["content"][0]["text"], "Independent advisory");
}

#[tokio::test]
async fn non_mcp_host_values_have_an_explicit_shape_without_a_second_dispatch() {
    let host = host();
    let (outcome, replies) = run(
        &host,
        &test_config(Duration::from_secs(10)),
        vec![request(0, true, json!({"native":true,"value":false}))],
        false,
    )
    .await;
    let DriveOutcome::Completed(response) = outcome else {
        panic!("completed")
    };
    let CodeModeRunnerInput::ToolResult { result, .. } = &replies[0] else {
        panic!("host result")
    };
    assert_eq!(result, &json!({"host_value":false}));
    assert!(response.calls[0].upstream_content.is_none());
    assert_eq!(host.effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn read_only_entrypoint_keeps_full_result_access_and_ordinary_call_admission() {
    let host = host();
    let mut config = test_config(Duration::from_secs(10));
    config.capability_filter = ToolScope::default().read_only();
    config.caller = CodeModeCaller::Scoped {
        capabilities: crate::CodeModeCallerCapabilities {
            can_read: true,
            ..Default::default()
        },
        sub: Some("reader".into()),
    };
    let (outcome, replies) = run(&host, &config, vec![request(0, true, json!({}))], false).await;
    let DriveOutcome::Completed(response) = outcome else {
        panic!("read-only completed")
    };
    let CodeModeRunnerInput::ToolResult { result, .. } = &replies[0] else {
        panic!("full read-only result")
    };
    assert_eq!(result["structuredContent"], json!({"rows":[1]}));
    assert_eq!(response.calls.len(), 1);
}

#[tokio::test]
async fn padded_internal_ids_keep_default_false_dispatch_and_internal_trace_classification() {
    let host = host();
    let config = test_config(Duration::from_secs(10));
    for (index, id) in [
        "__lab_internal::read_resource",
        " __lab_internal::read_resource",
        "__lab_internal ::read_resource",
        "__lab_internal::get_prompt ",
    ]
    .into_iter()
    .enumerate()
    {
        let (outcome,replies) = run(&host,&config,vec![json!({"type":"tool_call","seq":0,"id":id,"params":{"uri":"fixture://read","prompt":"fixture/prompt"}})],false).await;
        assert!(matches!(replies[0], CodeModeRunnerInput::ToolResult { .. }));
        assert_eq!(host.effects.load(Ordering::SeqCst), index + 1);
        let DriveOutcome::Completed(response) = outcome else {
            panic!("default-false internal dispatch")
        };
        assert_eq!(response.calls, Vec::<CodeModeExecutedCall>::new());
    }
}
