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
