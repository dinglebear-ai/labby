//! No-I/O host used only by the fixture harness.
use crate::error::ToolError;
use crate::{
    CodeModeCallError, CodeModeCaller, CodeModeCatalogKind, CodeModeConfig, CodeModeHost,
    CodeModeSurface, ExecCtx, ResolvedSnippet, RunnerPool, ToolCallOutcome, ToolScope, ToolsRender,
};
use serde_json::Value;

pub(super) struct OfflineHost {
    pool: RunnerPool,
    config: CodeModeConfig,
    client: reqwest::Client,
}
impl OfflineHost {
    pub(super) fn new(config: CodeModeConfig) -> Result<Self, ToolError> {
        let client =
            labby_openapi::http::build_dispatch_client().map_err(|error| ToolError::Sdk {
                sdk_kind: "fixture_setup_error".into(),
                message: error.to_string(),
            })?;
        Ok(Self {
            pool: RunnerPool::from_env()?,
            config,
            client,
        })
    }
    pub(super) async fn shutdown(&self) {
        self.pool.shutdown().await;
    }
}
impl CodeModeHost for OfflineHost {
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
        _: Value,
        _: &CodeModeCaller,
        _: CodeModeSurface,
        _: &ToolScope,
        _: ExecCtx,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        Err(ToolError::Sdk {
            sdk_kind: "forbidden".into(),
            message: "Fixture tests have no live tool access".into(),
        }
        .into())
    }
    async fn resolve_snippet(&self, _: &str, _: Value) -> Result<ResolvedSnippet, ToolError> {
        Err(ToolError::Sdk {
            sdk_kind: "forbidden".into(),
            message: "Nested snippet resolution is unavailable in fixture tests".into(),
        })
    }
    async fn semantic_rank(
        &self,
        _: String,
        _: usize,
        _: &[CodeModeCatalogKind],
        _: &CodeModeCaller,
        _: CodeModeSurface,
        _: &ToolScope,
    ) -> Result<Vec<(String, f32)>, ToolError> {
        Ok(Vec::new())
    }
    async fn config(&self) -> CodeModeConfig {
        self.config.clone()
    }
    fn runner_pool(&self) -> &RunnerPool {
        &self.pool
    }
    fn openapi_registry(&self) -> labby_openapi::OpenApiRegistry {
        labby_openapi::OpenApiRegistry::default()
    }
    fn openapi_http_client(&self) -> reqwest::Client {
        self.client.clone()
    }
}
