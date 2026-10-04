use super::*;

/// The parent-derived `step_ordinal` is a contiguous monotonic count of
/// `step_begin` events (0, 1, …), independent of the runner `seq` spine:
/// even with a `tool_call` interleaved (bumping seq) between two steps, the
/// two recorded steps get ordinals `[0, 1]`. The threaded `execution_id`
/// and step `name` also reach `record_step`.

#[cfg(not(windows))]
#[tokio::test]
async fn step_ordinal_is_contiguous_across_interleaved_tool_calls() {
    use std::sync::Mutex as StdMutex;

    use crate::host::{ResolvedSnippet, ToolCallOutcome, ToolsRender};
    use crate::types::{CodeModeCaller, CodeModeSurface, ToolScope};
    use labby_runtime::CodeModeConfig;

    type Recorded = Vec<(Option<String>, Option<u64>, String)>;

    struct RecordingHost {
        pool: RunnerPool,
        recorded: Arc<StdMutex<Recorded>>,
    }

    impl CodeModeHost for RecordingHost {
        async fn list_tools(
            &self,
            _caller: &CodeModeCaller,
            _surface: CodeModeSurface,
            _scope: &ToolScope,
            _include_snippets: bool,
            _use_cache: bool,
        ) -> Result<ToolsRender, ToolError> {
            Ok(ToolsRender {
                fingerprint: "recording".to_string(),
                embedding_fingerprint: "recording".to_string(),
                entries: Arc::from([]),
                catalog_json: Arc::from("[]"),
                serialized_size: 2,
                withheld: Arc::from([]),
            })
        }

        async fn call_tool(
            &self,
            _id: &str,
            _params: Value,
            _caller: &CodeModeCaller,
            _surface: CodeModeSurface,
            _scope: &ToolScope,
            _ctx: ExecCtx,
        ) -> Result<ToolCallOutcome, CodeModeCallError> {
            Ok(ToolCallOutcome {
                value: json!({"ok": true}),
                ui: None,
            })
        }

        async fn record_step(
            &self,
            ctx: ExecCtx,
            name: &str,
            _value: &Value,
        ) -> Result<(), ToolError> {
            self.recorded.lock().expect("recorded mutex").push((
                ctx.execution_id.as_deref().map(ToString::to_string),
                ctx.step_ordinal,
                name.to_string(),
            ));
            Ok(())
        }

        async fn resolve_snippet(
            &self,
            _name: &str,
            _input: Value,
        ) -> Result<ResolvedSnippet, ToolError> {
            Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: "RecordingHost exposes no snippets".to_string(),
            })
        }

        async fn semantic_rank(
            &self,
            _query: String,
            _top_k: usize,
            _kinds: &[crate::CodeModeCatalogKind],
            _caller: &CodeModeCaller,
            _surface: CodeModeSurface,
            _scope: &ToolScope,
        ) -> Result<Vec<(String, f32)>, ToolError> {
            Ok(Vec::new())
        }

        async fn config(&self) -> CodeModeConfig {
            CodeModeConfig::default()
        }

        fn runner_pool(&self) -> &RunnerPool {
            &self.pool
        }

        fn openapi_registry(&self) -> labby_openapi::OpenApiRegistry {
            labby_openapi::OpenApiRegistry::default()
        }

        fn openapi_http_client(&self) -> reqwest::Client {
            labby_openapi::http::build_dispatch_client().expect("test dispatch client")
        }
    }

    // step_begin(5,"a") -> tool_call(6) -> step_result(5) ->
    // step_begin(7,"b") -> step_result(7). The seq gap (6 between the two
    // steps) must NOT show up in the step ordinals.
    let script = r#"
exec 3<&0
cat <&3 >/dev/null &
printf '{"type":"step_begin","seq":5,"name":"a"}\n'
printf '{"type":"tool_call","seq":6,"id":"stub::tool","params":{}}\n'
printf '{"type":"step_result","seq":5,"value":{"r":1}}\n'
printf '{"type":"step_begin","seq":7,"name":"b"}\n'
printf '{"type":"step_result","seq":7,"value":{"r":2}}\n'
sleep 2
printf '{"type":"done"}\n'
sleep 3600
"#;
    let recorded: Arc<StdMutex<Recorded>> = Arc::new(StdMutex::new(Vec::new()));
    let host = RecordingHost {
        pool: RunnerPool::from_env().expect("test process must expose current executable"),
        recorded: Arc::clone(&recorded),
    };
    let broker = CodeModeBroker::new(Some(&host));
    let mut cfg = test_config(Duration::from_secs(30));
    cfg.execution_id = Some(Arc::<str>::from("exec_test"));
    let mut runner = PooledRunner::spawn_stub_script(script).expect("spawn script stub");
    let outcome = broker
        .drive_runner(&mut runner, &cfg, tokio::time::Instant::now() + cfg.timeout)
        .await;
    match outcome {
        DriveOutcome::Completed(_) => {}
        DriveOutcome::ExecutionError(err)
        | DriveOutcome::RunnerUnavailableBeforeActivity(err)
        | DriveOutcome::RunnerUnhealthy(err) => {
            panic!("run must complete, got error kind `{}`", err.kind())
        }
    }
    let recorded = recorded.lock().expect("recorded mutex").clone();
    assert_eq!(
        recorded
            .iter()
            .map(|(_, ordinal, _)| *ordinal)
            .collect::<Vec<_>>(),
        vec![Some(0), Some(1)],
        "step ordinals must be contiguous regardless of seq gap"
    );
    assert_eq!(
        recorded
            .iter()
            .map(|(_, _, name)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"],
        "step names must be threaded to record_step"
    );
    assert!(
        recorded
            .iter()
            .all(|(exec, _, _)| exec.as_deref() == Some("exec_test")),
        "execution_id must reach record_step"
    );
}

/// External call ordinals are assigned at enqueue time, not completion
/// time. Concurrent calls may settle out of order, but the final
/// `response.calls[]` is seq-sorted and each host context must carry the
/// zero-based ordinal matching that final array position.
#[cfg(not(windows))]
#[tokio::test]
async fn call_ordinal_and_trace_context_align_with_fan_out_response_order() {
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    use crate::host::{ResolvedSnippet, ToolCallOutcome, ToolsRender};
    use crate::types::{CodeModeCaller, CodeModeSurface, ToolScope};
    use labby_primitives::trace::TraceContext;
    use labby_runtime::CodeModeConfig;

    type Recorded = HashMap<String, (Option<u64>, Option<String>, Option<String>)>;

    struct RecordingCallHost {
        pool: RunnerPool,
        recorded: Arc<StdMutex<Recorded>>,
    }

    impl CodeModeHost for RecordingCallHost {
        async fn list_tools(
            &self,
            _caller: &CodeModeCaller,
            _surface: CodeModeSurface,
            _scope: &ToolScope,
            _include_snippets: bool,
            _use_cache: bool,
        ) -> Result<ToolsRender, ToolError> {
            Ok(ToolsRender::empty())
        }

        async fn call_tool(
            &self,
            id: &str,
            _params: Value,
            _caller: &CodeModeCaller,
            _surface: CodeModeSurface,
            _scope: &ToolScope,
            ctx: ExecCtx,
        ) -> Result<ToolCallOutcome, CodeModeCallError> {
            let trace_id = ctx
                .trace_context
                .as_ref()
                .map(|trace| trace.trace_id().to_hex());
            self.recorded.lock().expect("recorded mutex").insert(
                id.to_string(),
                (
                    ctx.call_ordinal,
                    ctx.execution_id.as_deref().map(ToString::to_string),
                    trace_id,
                ),
            );
            if id == "stub::slow" {
                tokio::time::sleep(Duration::from_millis(75)).await;
            }
            Ok(ToolCallOutcome {
                value: json!({"id": id}),
                ui: None,
            })
        }

        async fn resolve_snippet(
            &self,
            _name: &str,
            _input: Value,
        ) -> Result<ResolvedSnippet, ToolError> {
            Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: "RecordingCallHost exposes no snippets".to_string(),
            })
        }

        async fn semantic_rank(
            &self,
            _query: String,
            _top_k: usize,
            _kinds: &[crate::CodeModeCatalogKind],
            _caller: &CodeModeCaller,
            _surface: CodeModeSurface,
            _scope: &ToolScope,
        ) -> Result<Vec<(String, f32)>, ToolError> {
            Ok(Vec::new())
        }

        async fn config(&self) -> CodeModeConfig {
            CodeModeConfig::default()
        }

        fn runner_pool(&self) -> &RunnerPool {
            &self.pool
        }

        fn openapi_registry(&self) -> labby_openapi::OpenApiRegistry {
            labby_openapi::OpenApiRegistry::default()
        }

        fn openapi_http_client(&self) -> reqwest::Client {
            labby_openapi::http::build_dispatch_client().expect("test dispatch client")
        }
    }

    let script = r#"
exec 3<&0
cat <&3 >/dev/null &
printf '{"type":"tool_call","seq":10,"id":"stub::slow","params":{}}
'
printf '{"type":"tool_call","seq":11,"id":"stub::fast","params":{}}
'
sleep 1
printf '{"type":"done"}
'
sleep 3600
"#;
    let recorded: Arc<StdMutex<Recorded>> = Arc::new(StdMutex::new(HashMap::new()));
    let host = RecordingCallHost {
        pool: RunnerPool::from_env().expect("test process must expose current executable"),
        recorded: Arc::clone(&recorded),
    };
    let broker = CodeModeBroker::new(Some(&host));
    let mut cfg = test_config(Duration::from_secs(30));
    cfg.execution_id = Some(Arc::<str>::from("exec_fanout"));
    cfg.trace_context = Some(Arc::new(TraceContext::fresh(0).expect("trace context")));
    let expected_trace_id = cfg
        .trace_context
        .as_ref()
        .expect("trace context")
        .trace_id()
        .to_hex();

    let mut runner = PooledRunner::spawn_stub_script(script).expect("spawn script stub");
    let outcome = broker
        .drive_runner(&mut runner, &cfg, tokio::time::Instant::now() + cfg.timeout)
        .await;
    let response = match outcome {
        DriveOutcome::Completed(response) => response,
        DriveOutcome::ExecutionError(err)
        | DriveOutcome::RunnerUnavailableBeforeActivity(err)
        | DriveOutcome::RunnerUnhealthy(err) => {
            panic!("run must complete, got error kind `{}`", err.kind())
        }
    };

    assert_eq!(
        response
            .calls
            .iter()
            .map(|call| call.id.as_str())
            .collect::<Vec<_>>(),
        vec!["stub::slow", "stub::fast"],
        "final calls remain in runner seq order even when fast settles first"
    );
    let recorded = recorded.lock().expect("recorded mutex");
    for (ordinal, call) in response.calls.iter().enumerate() {
        let (seen_ordinal, execution_id, trace_id) =
            recorded.get(&call.id).expect("recorded host call");
        assert_eq!(*seen_ordinal, Some(ordinal as u64));
        assert_eq!(execution_id.as_deref(), Some("exec_fanout"));
        assert_eq!(trace_id.as_deref(), Some(expected_trace_id.as_str()));
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn drive_runner_marks_pre_protocol_exit_retryable() {
    let broker: CodeModeBroker<'_, NoopHost> = CodeModeBroker::new(None);
    let script = "IFS= read -r _\nexit 17\n";
    let mut runner = PooledRunner::spawn_stub_script(script).expect("spawn exit stub");
    let outcome = broker
        .drive_runner(
            &mut runner,
            &test_config(Duration::from_secs(5)),
            tokio::time::Instant::now() + Duration::from_secs(5),
        )
        .await;

    match outcome {
        DriveOutcome::RunnerUnavailableBeforeActivity(err) => {
            assert_eq!(err.kind(), "server_error");
            assert!(err.to_string().contains("exited before completion"));
        }
        DriveOutcome::Completed(_)
        | DriveOutcome::ExecutionError(_)
        | DriveOutcome::RunnerUnhealthy(_) => {
            panic!("an EOF before the first protocol event must be retryable");
        }
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn run_via_pool_retries_pre_protocol_exit_on_fresh_runner() {
    let marker_dir = tempfile::tempdir().expect("marker tempdir");
    let marker = marker_dir.path().join("first-run");
    let script = format!(
        r#"
if [ ! -e "{marker}" ]; then
  IFS= read -r _
  : > "{marker}"
  exit 17
fi
IFS= read -r _
printf '%s\n' '{{"type":"done","result":{{"state":"json","value":{{"retried":true}}}},"logs":[]}}'
sleep 3600
"#,
        marker = marker.display()
    );
    let pool = RunnerPool::with_spawn(RunnerSpawn {
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_string(), script],
    });
    let broker: CodeModeBroker<'_, NoopHost> = CodeModeBroker::new(None);
    let response = broker
        .run_via_pool(
            &pool,
            test_config(Duration::from_secs(5)),
            tokio::time::Instant::now() + Duration::from_secs(5),
        )
        .await
        .expect("fresh runner retry must succeed");

    assert_eq!(response.result, Some(json!({"retried": true})));
    assert!(marker.exists(), "the first runner must have exited");
}
