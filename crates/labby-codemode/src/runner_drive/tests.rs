#![allow(clippy::panic)]
use super::*;
use crate::host::NoopHost;
#[cfg(not(windows))]
use crate::pool::RunnerSpawn;

pub(super) fn test_config(timeout: Duration) -> RunnerConfig {
    RunnerConfig {
        code_to_run: "async () => 1".to_string(),
        proxy: String::new(),
        timeout,
        caller: CodeModeCaller::TrustedLocal,
        surface: CodeModeSurface::Cli,
        max_log_entries: 100,
        max_log_bytes: 4096,
        trace_params: false,
        capability_filter: ToolScope::default(),
        snippet_max_bytes: MAX_SNIPPET_RESOLVED_BYTES_PER_RUN,
        execution_id: None,
        trace_context: None,
        openapi_registry: labby_openapi::OpenApiRegistry::default(),
        openapi_http_client: labby_openapi::http::build_dispatch_client()
            .expect("test dispatch client"),
    }
}

#[cfg(not(windows))]
struct DelayedToolHost {
    inner: NoopHost,
    delay: Duration,
}

#[cfg(not(windows))]
impl CodeModeHost for DelayedToolHost {
    async fn list_tools(
        &self,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
        include_snippets: bool,
        use_cache: bool,
    ) -> Result<crate::host::ToolsRender, ToolError> {
        self.inner
            .list_tools(caller, surface, scope, include_snippets, use_cache)
            .await
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
        tokio::time::sleep(self.delay).await;
        Ok(ToolCallOutcome {
            value: json!({"unexpected": true}),
            ui: None,
        })
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

mod deadlines_and_calls;
mod internal_calls_and_steps;
mod settlement;
mod traces_and_pool;
