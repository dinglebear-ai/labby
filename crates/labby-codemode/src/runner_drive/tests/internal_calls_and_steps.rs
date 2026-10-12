#[cfg(not(windows))]
use super::*;

/// The internal-call ceiling: `__lab_internal::*` calls stay excluded from
/// the ordinary budget and the call trace, but are metered separately
/// against `MAX_INTERNAL_CALLS_PER_RUN`. Over-ceiling internal calls must
/// (a) settle with the fail-open empty `{"ranked": []}` ToolResult (never
/// a ToolError — FAIL-OPEN invariant), (b) never reach
/// `host.semantic_rank` (no further embedding-service round trips),
/// (c) stay absent from the call trace, and (d) leave the ordinary-call
/// budget untouched.

#[cfg(not(windows))]
#[tokio::test]
async fn drive_runner_caps_internal_calls_and_settles_over_ceiling_fail_open() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::host::{ResolvedSnippet, ToolCallOutcome, ToolsRender};
    use crate::types::{CodeModeCaller, CodeModeSurface, ToolScope};
    use labby_runtime::CodeModeConfig;

    struct CountingHost {
        pool: RunnerPool,
        semantic_calls: AtomicUsize,
    }

    impl CodeModeHost for CountingHost {
        async fn list_tools(
            &self,
            _caller: &CodeModeCaller,
            _surface: CodeModeSurface,
            _scope: &ToolScope,
            _include_snippets: bool,
            _use_cache: bool,
        ) -> Result<ToolsRender, ToolError> {
            Ok(ToolsRender {
                fingerprint: "counting".to_string(),
                embedding_fingerprint: "counting".to_string(),
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
            Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "CountingHost exposes no tools".to_string(),
            }
            .into())
        }

        async fn resolve_snippet(
            &self,
            _name: &str,
            _input: Value,
        ) -> Result<ResolvedSnippet, ToolError> {
            Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: "CountingHost exposes no snippets".to_string(),
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
            self.semantic_calls.fetch_add(1, Ordering::SeqCst);
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

    let ceiling = MAX_INTERNAL_CALLS_PER_RUN;
    let internal_total = ceiling + 4;
    let ord1 = internal_total + 1;
    let ord2 = internal_total + 2;
    // Capture everything the host writes back to the stub so the test can
    // assert every internal call settled as a ToolResult (fail-open), not
    // a ToolError.
    let capture = tempfile::NamedTempFile::new().expect("create capture file");
    let capture_path = capture.path().display();
    let script = format!(
        r#"
exec 3<&0
cat <&3 >"{capture_path}" &
i=1
while [ "$i" -le {internal_total} ]; do
  printf '{{"type":"tool_call","seq":%d,"id":"__lab_internal::semantic_rank","params":{{"query":"q","limit":5}}}}\n' "$i"
  i=$((i+1))
done
printf '{{"type":"tool_call","seq":{ord1},"id":"stub::tool","params":{{}}}}\n'
printf '{{"type":"tool_call","seq":{ord2},"id":"stub::tool","params":{{}}}}\n'
sleep 2
printf '{{"type":"done"}}\n'
sleep 3600
"#
    );
    let host = CountingHost {
        pool: RunnerPool::from_env().expect("test process must expose current executable"),
        semantic_calls: AtomicUsize::new(0),
    };
    let broker = CodeModeBroker::new(Some(&host));
    let mut runner = PooledRunner::spawn_stub_script(&script).expect("spawn script stub");
    let outcome = broker
        .drive_runner(
            &mut runner,
            &test_config(Duration::from_secs(30)),
            tokio::time::Instant::now() + Duration::from_secs(30),
        )
        .await;
    let response = match outcome {
        DriveOutcome::Completed(response) => response,
        DriveOutcome::ExecutionError(err)
        | DriveOutcome::RunnerUnavailableBeforeActivity(err)
        | DriveOutcome::RunnerUnhealthy(err) => {
            panic!(
                "over-ceiling internal calls must fail open, got error kind `{}`",
                err.kind()
            )
        }
    };
    // (b) the ceiling actually gates dispatch: only the first
    // MAX_INTERNAL_CALLS_PER_RUN internal calls reached the host.
    assert_eq!(
        host.semantic_calls.load(Ordering::SeqCst),
        ceiling,
        "over-ceiling internal calls must never reach host.semantic_rank"
    );
    // (c) internal calls — under and over ceiling — stay out of the trace.
    assert!(
        response
            .calls
            .iter()
            .all(|call| !call.id.starts_with("__lab_internal::")),
        "internal calls must not appear in the call trace"
    );
    // (d) the ordinary budget is unaffected: both ordinary calls are
    // traced (as unknown_tool failures) and none is budget-rejected.
    assert_eq!(
        response.calls.len(),
        2,
        "both ordinary calls must be traced"
    );
    assert!(
        response
            .calls
            .iter()
            .all(|call| call.error_kind.as_deref() != Some("call_budget_exceeded")),
        "internal calls must not consume ordinary budget slots"
    );
    // (a) every internal call — including the over-ceiling ones — settled
    // its promise with the fail-open `{"ranked": []}` ToolResult. Poll
    // briefly: the background `cat` may still be flushing the last lines
    // to the capture file when the drive loop returns.
    let mut contents = String::new();
    for _ in 0..50 {
        contents = std::fs::read_to_string(capture.path()).unwrap_or_default();
        if contents.matches(r#""ranked":[]"#).count() >= internal_total {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        contents.matches(r#""ranked":[]"#).count(),
        internal_total,
        "every internal call (including over-ceiling) must settle with the fail-open empty ranked result"
    );
}

/// `enqueue_internal_call_over_ceiling`'s fail-open value must match the
/// specific internal tool being throttled, not a hardcoded shape borrowed
/// from `semantic_rank`. Before this was shape-aware, an over-ceiling
/// `describe_types` call settled with `{"ranked": []}` — which happened to
/// look like "no type info" to `codemode.describe()`'s JS
/// (`{"ranked": []}.dts` is `undefined`, same falsy outcome as
/// `{"dts": null}.dts`) only by coincidence, not by design. Regression:
/// drive more than `MAX_INTERNAL_CALLS_PER_RUN` `describe_types` calls and
/// assert every settled result (both under and over the ceiling) is
/// `{"dts": null}` and that `"ranked":[]` never appears in the wire trace.
#[cfg(not(windows))]
#[tokio::test]
async fn drive_runner_describe_types_over_ceiling_settles_with_dts_shape_not_ranked_shape() {
    use crate::host::NoopHost;

    let ceiling = MAX_INTERNAL_CALLS_PER_RUN;
    let internal_total = ceiling + 2;
    let capture = tempfile::NamedTempFile::new().expect("create capture file");
    let capture_path = capture.path().display();
    let script = format!(
        r#"
exec 3<&0
cat <&3 >"{capture_path}" &
i=1
while [ "$i" -le {internal_total} ]; do
  printf '{{"type":"tool_call","seq":%d,"id":"__lab_internal::describe_types","params":{{"id":"nonexistent::tool"}}}}\n' "$i"
  i=$((i+1))
done
sleep 2
printf '{{"type":"done"}}\n'
sleep 3600
"#
    );
    let host = NoopHost::default();
    let broker = CodeModeBroker::new(Some(&host));
    let mut runner = PooledRunner::spawn_stub_script(&script).expect("spawn script stub");
    let outcome = broker
        .drive_runner(
            &mut runner,
            &test_config(Duration::from_secs(30)),
            tokio::time::Instant::now() + Duration::from_secs(30),
        )
        .await;
    match outcome {
        DriveOutcome::Completed(_) => {}
        DriveOutcome::ExecutionError(err)
        | DriveOutcome::RunnerUnavailableBeforeActivity(err)
        | DriveOutcome::RunnerUnhealthy(err) => {
            panic!(
                "over-ceiling describe_types calls must fail open, got error kind `{}`",
                err.kind()
            )
        }
    }
    let mut contents = String::new();
    for _ in 0..50 {
        contents = std::fs::read_to_string(capture.path()).unwrap_or_default();
        if contents.matches(r#""dts":null"#).count() >= internal_total {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        contents.matches(r#""dts":null"#).count(),
        internal_total,
        "every describe_types call — under and over the ceiling — must settle with \
             the {{\"dts\": null}} shape, never {{\"ranked\": []}}"
    );
    assert!(
        !contents.contains(r#""ranked":[]"#),
        "over-ceiling describe_types calls must never settle with semantic_rank's shape"
    );
}
