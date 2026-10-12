//! Cancellation through the real runner protocol must drop in-flight host work.
use super::tests::test_config;
use super::*;
use crate::host::NoopHost;
use std::sync::Arc;
use tokio::sync::{Notify, oneshot};

struct DropSignal(Arc<Notify>);
impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

struct PendingHost {
    inner: NoopHost,
    started: Arc<Notify>,
    dropped: Arc<Notify>,
}
impl CodeModeHost for PendingHost {
    async fn list_tools(
        &self,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
        snippets: bool,
        cache: bool,
    ) -> Result<crate::host::ToolsRender, ToolError> {
        self.inner
            .list_tools(caller, surface, scope, snippets, cache)
            .await
    }
    async fn call_tool(
        &self,
        _: &str,
        _: Value,
        _: &CodeModeCaller,
        _: CodeModeSurface,
        _: &ToolScope,
        _: ExecCtx,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        let _drop_signal = DropSignal(self.dropped.clone());
        self.started.notify_one();
        std::future::pending().await
    }
    async fn resolve_snippet(
        &self,
        name: &str,
        input: Value,
    ) -> Result<crate::host::ResolvedSnippet, ToolError> {
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

#[tokio::test]
async fn aborting_protocol_execution_drops_host_work_and_kills_owned_runner() {
    let started = Arc::new(Notify::new());
    let dropped = Arc::new(Notify::new());
    let (pid_tx, pid_rx) = oneshot::channel();
    let host = PendingHost {
        inner: NoopHost::default(),
        started: started.clone(),
        dropped: dropped.clone(),
    };
    let execution = tokio::spawn(async move {
        let mut runner = PooledRunner::spawn_stub_script(
            r#"
IFS= read -r _
printf '%s\n' '{"type":"tool_call","seq":1,"id":"fixture::pending","params":{}}'
IFS= read -r _
sleep 3600
"#,
        )
        .expect("owned protocol stub");
        pid_tx
            .send(runner.child.id().expect("runner pid"))
            .expect("report pid");
        let config = test_config(Duration::from_secs(20));
        CodeModeBroker::new(Some(&host))
            .drive_runner(
                &mut runner,
                &config,
                tokio::time::Instant::now() + config.timeout,
            )
            .await
    });
    let pid = pid_rx.await.expect("owned pid");
    tokio::time::timeout(Duration::from_secs(3), started.notified())
        .await
        .expect("host call began");
    execution.abort();
    assert!(execution.await.is_err_and(|error| error.is_cancelled()));
    tokio::time::timeout(Duration::from_secs(3), dropped.notified())
        .await
        .expect("pending host future dropped");
    let pid = nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid fits i32"));
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if nix::sys::signal::kill(pid, None) == Err(nix::errno::Errno::ESRCH) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned runner reaped after cancellation");
}

#[tokio::test]
async fn live_runner_closing_stdout_before_activity_times_out_without_retry() {
    let broker: CodeModeBroker<'_, NoopHost> = CodeModeBroker::new(None);
    let mut runner = PooledRunner::spawn_stub_script("IFS= read -r _\nexec 1>&-\nsleep 3600\n")
        .expect("stdout-closing stub");
    let config = test_config(Duration::from_millis(150));
    let outcome = tokio::time::timeout(
        Duration::from_secs(3),
        broker.drive_runner(
            &mut runner,
            &config,
            tokio::time::Instant::now() + config.timeout,
        ),
    )
    .await
    .expect("EOF exit wait and teardown must be bounded");
    let error = match outcome {
        DriveOutcome::RunnerUnhealthy(error) => Some(error),
        _ => None,
    }
    .expect("stdout EOF is not proof of an actual pre-protocol process exit");
    assert_eq!(error.kind(), "timeout");
    assert!(runner.child.try_wait().expect("child status").is_some());
}

#[tokio::test]
async fn stdout_eof_cancels_pending_host_work_before_waiting_for_child_exit() {
    let marker_dir = tempfile::tempdir().expect("marker directory");
    let marker = marker_dir.path().join("host-started");
    let script = format!(
        r#"
IFS= read -r _
printf '%s\n' '{{"type":"tool_call","seq":1,"id":"fixture::pending","params":{{}}}}'
while [ ! -e "{marker}" ]; do sleep 0.01; done
exec 1>&-
sleep 3600
"#,
        marker = marker.display()
    );
    let started = Arc::new(Notify::new());
    let dropped = Arc::new(Notify::new());
    let host = PendingHost {
        inner: NoopHost::default(),
        started: started.clone(),
        dropped: dropped.clone(),
    };
    let broker = CodeModeBroker::new(Some(&host));
    let config = test_config(Duration::from_secs(5));
    let mut runner = PooledRunner::spawn_stub_script(&script).expect("EOF stub");
    let mut execution = Box::pin(broker.drive_runner(
        &mut runner,
        &config,
        tokio::time::Instant::now() + config.timeout,
    ));
    tokio::time::timeout(Duration::from_secs(2), async {
        let host_started = tokio::select! {
            () = started.notified() => true,
            _ = &mut execution => false,
        };
        assert!(host_started, "execution ended before host work started");
    })
    .await
    .expect("host request started");
    std::fs::write(&marker, b"started").expect("release stdout close");
    tokio::time::timeout(Duration::from_secs(2), async {
        let host_dropped = tokio::select! {
            () = dropped.notified() => true,
            _ = &mut execution => false,
        };
        assert!(
            host_dropped,
            "host work must cancel before the live-child exit wait completes"
        );
    })
    .await
    .expect("EOF immediately drops pending host work");
    let outcome = tokio::time::timeout(Duration::from_secs(6), &mut execution)
        .await
        .expect("bounded exit wait");
    drop(execution);
    let error = match outcome {
        DriveOutcome::RunnerUnhealthy(error) => Some(error),
        _ => None,
    }
    .expect("post-activity EOF must not replay");
    assert_eq!(error.kind(), "timeout");
    assert!(runner.child.try_wait().expect("child status").is_some());
}
