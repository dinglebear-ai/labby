use super::*;
use crate::host::{ResolvedSnippet, ToolsRender};
use crate::pool::config::PoolConfig;
use crate::types::CodeModeCallerCapabilities;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use tokio::sync::Notify;

struct PendingDrop(Arc<Notify>);

impl Drop for PendingDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

struct FanOutHost {
    inner: NoopHost,
    started: PathBuf,
    pending_dropped: Arc<Notify>,
    effects: AtomicUsize,
}

impl CodeModeHost for FanOutHost {
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
        id: &str,
        _: Value,
        _: &CodeModeCaller,
        _: CodeModeSurface,
        _: &ToolScope,
        _: ExecCtx,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        match id {
            "fixture::fail" => Err(first_failure()),
            "fixture::effect" => {
                self.effects.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(ToolCallOutcome {
                    value: json!({"applied": true}),
                    ui: None,
                })
            }
            "fixture::pending" => {
                let _drop = PendingDrop(self.pending_dropped.clone());
                std::fs::write(&self.started, b"started").expect("owned readiness marker");
                std::future::pending().await
            }
            _ => Err(CodeModeCallError::new(
                "unknown_tool",
                "unexpected fixture call",
            )),
        }
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

fn first_failure() -> CodeModeCallError {
    CodeModeCallError::new("tool_error", "owned first rejection").with_tool("fixture::fail")
}

fn caller_config(subject: &str) -> RunnerConfig {
    let mut cfg = test_config(Duration::from_secs(10));
    cfg.caller = CodeModeCaller::Scoped {
        capabilities: CodeModeCallerCapabilities {
            can_read: true,
            can_execute: true,
            can_use_snippets: false,
            is_admin: false,
        },
        sub: Some(subject.into()),
    };
    cfg.code_to_run = subject.into();
    cfg
}

#[tokio::test]
async fn failed_fan_out_evicts_late_frames_before_other_callers_and_keeps_done_reuse() {
    let dir = tempfile::tempdir().expect("owned protocol fixture");
    let spawned = dir.path().join("spawned");
    let started = dir.path().join("pending-started");
    let replies = dir.path().join("old-replies");
    let first = dir.path().join("first");
    let error = serde_json::to_string(&CodeModeRunnerOutput::Error {
        error: Box::new(first_failure()),
    })
    .expect("structured first error");
    let stale = serde_json::to_string(&CodeModeRunnerOutput::Error {
        error: Box::new(CodeModeCallError::new(
            "server_error",
            "runner expected start message",
        )),
    })
    .expect("stale error");
    // Force the ownership boundary, without a scheduler race: both completed
    // replies are already received and the third call is pending before the
    // first Error plus its late-reply terminal frame are emitted together.
    let script = format!(
        r#"
printf '%s\n' "$$" >>"{spawned}"
if [ ! -e "{first}" ]; then
  : >"{first}"
  IFS= read -r _
  printf '%s\n' '{{"type":"tool_call","seq":0,"id":"fixture::fail","params":{{}}}}'
  printf '%s\n' '{{"type":"tool_call","seq":1,"id":"fixture::effect","params":{{}}}}'
  printf '%s\n' '{{"type":"tool_call","seq":2,"id":"fixture::pending","params":{{}}}}'
  i=0
  while [ "$i" -lt 2 ]; do
    IFS= read -r reply || exit 21
    printf '%s\n' "$reply" >>"{replies}"
    i=$((i+1))
  done
  while [ ! -e "{started}" ]; do sleep 0.01; done
  printf '%s\n' '{error}' '{stale}'
fi
while IFS= read -r start; do
  case "$start" in
    *owner-B*) marker=owner-B ;;
    *owner-C*) marker=owner-C ;;
    *) exit 22 ;;
  esac
  printf '{{"type":"done","result":{{"state":"json","value":{{"marker":"%s","pid":%s}}}},"logs":[],"reuse_ready":true}}\n' "$marker" "$$"
done
"#,
        spawned = spawned.display(),
        first = first.display(),
        replies = replies.display(),
        started = started.display(),
        error = error.replace('\'', "'\"'\"'"),
        stale = stale.replace('\'', "'\"'\"'"),
    );
    let pool = RunnerPool::with_test_spawn(
        PoolConfig {
            size: 1,
            recycle_after: 100,
            max_overflow: 1,
        },
        RunnerSpawn {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), script],
        },
    );
    let dropped = Arc::new(Notify::new());
    let host = FanOutHost {
        inner: NoopHost::default(),
        started,
        pending_dropped: dropped.clone(),
        effects: AtomicUsize::new(0),
    };
    let broker = CodeModeBroker::new(Some(&host));
    let error = broker
        .run_via_pool(
            &pool,
            caller_config("owner-A"),
            tokio::time::Instant::now() + Duration::from_secs(10),
        )
        .await
        .expect_err("uncaught first rejection");
    assert_eq!(error.calls().len(), 2, "completed partial trace survives");
    assert_eq!(error.calls()[0].error_kind.as_deref(), Some("tool_error"));
    assert!(error.calls()[1].ok, "applied late completion is not lost");
    assert_eq!(error.into_call_error(), first_failure());
    assert_eq!(
        host.effects.load(AtomicOrdering::SeqCst),
        1,
        "no replay of mutation"
    );
    tokio::time::timeout(Duration::from_secs(2), dropped.notified())
        .await
        .expect("pending host work dropped");
    let old_replies: Vec<Value> = std::fs::read_to_string(&replies)
        .expect("reply capture")
        .lines()
        .map(|line| serde_json::from_str(line).expect("wire reply"))
        .collect();
    assert_eq!(old_replies.len(), 2);
    assert!(
        old_replies
            .iter()
            .any(|reply| reply["type"] == "tool_error")
    );
    assert!(
        old_replies
            .iter()
            .any(|reply| reply["type"] == "tool_result")
    );
    let b = broker
        .run_via_pool(
            &pool,
            caller_config("owner-B"),
            tokio::time::Instant::now() + Duration::from_secs(10),
        )
        .await
        .expect("B cannot consume A's stale Error");
    let c = broker
        .run_via_pool(
            &pool,
            caller_config("owner-C"),
            tokio::time::Instant::now() + Duration::from_secs(10),
        )
        .await
        .expect("C cannot consume B's terminal result");
    assert_eq!(b.result.as_ref().expect("B result")["marker"], "owner-B");
    assert_eq!(c.result.as_ref().expect("C result")["marker"], "owner-C");
    assert_eq!(
        b.result.as_ref().expect("B result")["pid"],
        c.result.as_ref().expect("C result")["pid"],
        "normal Done reuses its clean runner"
    );
    assert!(b.calls.is_empty() && c.calls.is_empty());
    assert!(b.artifacts.is_empty() && c.artifacts.is_empty());
    assert_eq!(
        std::fs::read_to_string(spawned)
            .expect("spawn capture")
            .lines()
            .count(),
        2,
        "one failed runner replaced; clean Done reused"
    );
    assert_eq!(host.effects.load(AtomicOrdering::SeqCst), 1);
    pool.shutdown().await;
}

async fn caught_done_ownership(proof: &str) {
    let dir = tempfile::tempdir().expect("owned caught-Done fixture");
    let spawned = dir.path().join("spawned");
    let replies = dir.path().join("replies");
    let first = dir.path().join("first");
    // Both host replies have completed before A reports its caught result. The
    // native runner can still have one unread reply and emit a stale Error.
    let script = format!(
        r#"
printf '%s\n' "$$" >>"{spawned}"
if [ ! -e "{first}" ]; then
  : >"{first}"
  IFS= read -r _
  printf '%s\n' '{{"type":"tool_call","seq":0,"id":"fixture::fail","params":{{}}}}'
  printf '%s\n' '{{"type":"tool_call","seq":1,"id":"fixture::effect","params":{{}}}}'
  i=0
  while [ "$i" -lt 2 ]; do
    IFS= read -r reply || exit 21
    printf '%s\n' "$reply" >>"{replies}"
    i=$((i+1))
  done
  printf '%s\n' '{{"type":"done","result":{{"state":"json","value":{{"marker":"owner-A"}}}},"logs":[]{proof}}}' '{{"type":"error","kind":"server_error","message":"runner expected start message"}}'
fi
while IFS= read -r start; do
  case "$start" in
    *owner-B*) marker=owner-B ;;
    *owner-C*) marker=owner-C ;;
    *) exit 22 ;;
  esac
  printf '{{"type":"done","result":{{"state":"json","value":{{"marker":"%s","pid":%s}}}},"logs":[],"reuse_ready":true}}\n' "$marker" "$$"
done
"#,
        spawned = spawned.display(),
        replies = replies.display(),
        first = first.display(),
    );
    let pool = RunnerPool::with_test_spawn(
        PoolConfig {
            size: 1,
            recycle_after: 100,
            max_overflow: 1,
        },
        RunnerSpawn {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), script],
        },
    );
    let host = FanOutHost {
        inner: NoopHost::default(),
        started: dir.path().join("unused-pending-readiness"),
        pending_dropped: Arc::new(Notify::new()),
        effects: AtomicUsize::new(0),
    };
    let broker = CodeModeBroker::new(Some(&host));
    let a = broker
        .run_via_pool(
            &pool,
            caller_config("owner-A"),
            tokio::time::Instant::now() + Duration::from_secs(10),
        )
        .await
        .expect("caught A result remains successful without replay");
    assert_eq!(a.result.as_ref().expect("A result")["marker"], "owner-A");
    assert_eq!(a.calls.len(), 2, "completed fan-out trace survives");
    assert_eq!(a.calls[0].error_kind.as_deref(), Some("tool_error"));
    assert!(a.calls[1].ok);
    let replies: Vec<Value> = std::fs::read_to_string(replies)
        .expect("both completed replies")
        .lines()
        .map(|line| serde_json::from_str(line).expect("reply JSON"))
        .collect();
    assert_eq!(replies.len(), 2);
    assert!(replies.iter().any(|reply| reply["type"] == "tool_error"));
    assert!(replies.iter().any(|reply| reply["type"] == "tool_result"));
    let b = broker
        .run_via_pool(
            &pool,
            caller_config("owner-B"),
            tokio::time::Instant::now() + Duration::from_secs(10),
        )
        .await
        .expect("B cannot consume caught A's stale Error");
    let c = broker
        .run_via_pool(
            &pool,
            caller_config("owner-C"),
            tokio::time::Instant::now() + Duration::from_secs(10),
        )
        .await
        .expect("clean C result");
    assert_eq!(b.result.as_ref().expect("B result")["marker"], "owner-B");
    assert_eq!(c.result.as_ref().expect("C result")["marker"], "owner-C");
    assert_eq!(
        b.result.as_ref().expect("B result")["pid"],
        c.result.as_ref().expect("C result")["pid"],
        "explicit clean Done proof preserves process reuse"
    );
    assert!(b.calls.is_empty() && c.calls.is_empty());
    assert!(b.artifacts.is_empty() && c.artifacts.is_empty());
    assert_eq!(host.effects.load(AtomicOrdering::SeqCst), 1, "no replay");
    assert_eq!(
        std::fs::read_to_string(spawned)
            .expect("spawned processes")
            .lines()
            .count(),
        2,
        "A is evicted; clean B/C reuse one process"
    );
    pool.shutdown().await;
}

#[tokio::test]
async fn caught_fan_out_done_without_settlement_proof_evicts_before_next_caller() {
    caught_done_ownership("").await;
}

#[tokio::test]
async fn caught_fan_out_done_with_pending_reply_evicts_before_next_caller() {
    caught_done_ownership(",\"reuse_ready\":false").await;
}
