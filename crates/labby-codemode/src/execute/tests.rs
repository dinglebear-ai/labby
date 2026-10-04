use super::*;
use crate::host::{NoopHost, ToolsRender};
use crate::types::{CodeModeCallerCapabilities, UiLink};
use serde_json::json;

fn response_with_result(result: Value) -> CodeModeExecutionResponse {
    CodeModeExecutionResponse {
        execution_id: None,
        result: Some(result),
        result_shaping: None,
        ui: None,
        calls: Vec::new(),
        logs: Vec::new(),
        artifacts: Vec::new(),
    }
}

/// A `CodeModeHost` with a configurable tool set, so `describe_types` (and
/// any future test needing real catalog entries) has something to look up.
/// `NoopHost` always returns an empty catalog, which can't exercise this.
///
/// Serializes and Arc-wraps the catalog ONCE in `new()`, matching
/// `CatalogRenderCache`'s real shape — `list_tools()` here is a refcount
/// bump per call, not a re-serialize, so a benchmark against this fixture
/// reflects the real cost of repeated `describe_types` dispatch, not an
/// artifact of a naive test double.
///
/// `list_tools` filters by namespace (`scope.allowed_namespaces()`) but
/// deliberately NOT by `scope.tools` — matching real production, where
/// `code_mode_catalog_tools_allowed` only ever applies namespace-level
/// filtering. Per-tool filtering is the caller's responsibility
/// (`discovery_entry_visible`); a fixture that also filtered by tool would
/// hide exactly the class of bug this is meant to catch.
struct FixtureHost {
    pool: crate::pool::RunnerPool,
    entries: Arc<[CatalogDescriptor]>,
    search_entries: Arc<[CatalogDescriptor]>,
    catalog_json: Arc<str>,
    fail_list_tools: bool,
    list_tools_calls: std::sync::atomic::AtomicUsize,
}

impl FixtureHost {
    fn new(entries: Vec<CatalogDescriptor>) -> Self {
        let catalog_json = serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string());
        Self {
            pool: crate::pool::RunnerPool::from_env()
                .expect("test process must expose current executable"),
            entries: Arc::from(entries),
            search_entries: Arc::from([]),
            catalog_json: Arc::from(catalog_json),
            fail_list_tools: false,
            list_tools_calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// A fixture whose `list_tools` always returns `Err`, for exercising
    /// fail-open degradation paths.
    fn failing() -> Self {
        Self {
            fail_list_tools: true,
            ..Self::new(Vec::new())
        }
    }

    fn list_tools_call_count(&self) -> usize {
        self.list_tools_calls
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn with_search_entries(mut self, entries: Vec<CatalogDescriptor>) -> Self {
        self.search_entries = Arc::from(entries);
        self
    }
}

impl CodeModeHost for FixtureHost {
    async fn list_tools(
        &self,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        scope: &ToolScope,
        _include_snippets: bool,
        _use_cache: bool,
    ) -> Result<ToolsRender, ToolError> {
        self.list_tools_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.fail_list_tools {
            return Err(ToolError::Sdk {
                sdk_kind: "upstream_connect_error".to_string(),
                message: "FixtureHost: simulated list_tools failure".to_string(),
            });
        }
        let entries = match scope.allowed_namespaces() {
            Some(allowed) => Arc::from(
                self.entries
                    .iter()
                    .filter(|entry| allowed.contains(&entry.namespace))
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            None => Arc::clone(&self.entries),
        };
        Ok(ToolsRender {
            fingerprint: "fixture".to_string(),
            embedding_fingerprint: "fixture".to_string(),
            entries,
            catalog_json: Arc::clone(&self.catalog_json),
            serialized_size: self.catalog_json.len(),
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
            message: "FixtureHost does not dispatch real tool calls".to_string(),
        }
        .into())
    }

    async fn list_resources(
        &self,
        upstream: String,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
    ) -> Result<Value, ToolError> {
        Ok(
            json!({ "resources": [{ "uri": format!("lab://upstream/{upstream}/fixture://skill") }] }),
        )
    }

    async fn read_resource(
        &self,
        uri: String,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
    ) -> Result<Value, ToolError> {
        Ok(json!({ "contents": [{ "uri": uri }] }))
    }

    async fn get_prompt(
        &self,
        prompt: String,
        arguments: Value,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
    ) -> Result<Value, ToolError> {
        Ok(json!({
            "prompt": prompt,
            "arguments": arguments,
            "messages": [{ "role": "user", "content": "fixture prompt" }],
        }))
    }

    async fn list_skills(
        &self,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
    ) -> Result<Value, ToolError> {
        Ok(json!({
            "skills": [{
                "uri": "skill://labby/fixture",
                "name": "fixture",
                "description": "fixture skill",
            }],
        }))
    }

    async fn get_skill(
        &self,
        uri: String,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
    ) -> Result<Value, ToolError> {
        Ok(json!({
            "skill": {
                "uri": uri,
                "name": "fixture",
            },
        }))
    }

    async fn read_skill(
        &self,
        uri: String,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
    ) -> Result<Value, ToolError> {
        Ok(json!({
            "contents": [{
                "uri": uri,
                "text": "fixture body",
            }],
        }))
    }

    async fn resolve_snippet(
        &self,
        _name: &str,
        _input: Value,
    ) -> Result<crate::host::ResolvedSnippet, ToolError> {
        Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: "FixtureHost exposes no snippets".to_string(),
        })
    }

    async fn semantic_rank(
        &self,
        _query: String,
        _top_k: usize,
        _kinds: &[CodeModeCatalogKind],
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
    ) -> Result<Vec<(String, f32)>, ToolError> {
        Ok(Vec::new())
    }

    async fn search_artifacts(
        &self,
        _query: String,
        limit: usize,
        kinds: &[CodeModeCatalogKind],
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
    ) -> Result<crate::ArtifactSearchResult, ToolError> {
        Ok(crate::ArtifactSearchResult {
            entries: self
                .search_entries
                .iter()
                .filter(|entry| kinds.is_empty() || kinds.contains(&entry.kind))
                .take(limit)
                .cloned()
                .collect(),
            incomplete_sources: Vec::new(),
        })
    }

    async fn config(&self) -> CodeModeConfig {
        CodeModeConfig::default()
    }

    fn runner_pool(&self) -> &crate::pool::RunnerPool {
        &self.pool
    }

    fn openapi_registry(&self) -> labby_openapi::OpenApiRegistry {
        labby_openapi::OpenApiRegistry::default()
    }

    fn openapi_http_client(&self) -> reqwest::Client {
        labby_openapi::http::build_dispatch_client().expect("test dispatch client")
    }
}

mod capability_dispatch;
mod execution_and_discovery;
mod schema_and_provider_policy;
