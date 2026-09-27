//! Finite in-memory tool source. Never falls back to a gateway or filesystem.
use super::model::{MockCall, SnippetTestCase, invalid};
use crate::error::ToolError;
use crate::{
    CatalogDescriptor, CodeModeCallError, CodeModeCaller, CodeModeCatalogKind, CodeModeConfig,
    CodeModeHost, CodeModeSurface, CodeModeToolSafety, ExecCtx, ResolvedSnippet, RunnerPool,
    ToolCallOutcome, ToolScope, ToolsRender,
};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

struct State {
    used: Vec<usize>,
    attempts: usize,
    unexpected: usize,
    simulated_errors: usize,
}
/// A host whose entire authority consists of finite JSON fixture rules.
pub(super) struct FixtureHost {
    rules: Vec<MockCall>,
    state: Mutex<State>,
    pool: RunnerPool,
    client: reqwest::Client,
    pub(super) config: CodeModeConfig,
    pub(super) scope: ToolScope,
    max_calls: usize,
}
impl FixtureHost {
    pub(super) fn new(case: &SnippetTestCase) -> Result<Self, ToolError> {
        case.validate()?;
        let ids: BTreeSet<String> = case.calls.iter().map(|c| c.tool.clone()).collect();
        let namespaces = ids
            .iter()
            .filter_map(|id| id.split_once("::").map(|(n, _)| n.to_string()))
            .collect();
        let scope = ToolScope::scoped_namespaces(namespaces, ids.into_iter().collect()).read_only();
        let config = CodeModeConfig {
            timeout_ms: case.budgets.wall_clock_ms,
            trace_params: false,
            ..CodeModeConfig::default()
        };
        let client = labby_openapi::http::build_dispatch_client()
            .map_err(|_| invalid("unable to initialize isolated fixture host"))?;
        Ok(Self {
            rules: case.calls.clone(),
            state: Mutex::new(State {
                used: vec![0; case.calls.len()],
                attempts: 0,
                unexpected: 0,
                simulated_errors: 0,
            }),
            pool: RunnerPool::from_env()?,
            client,
            config,
            scope,
            max_calls: case.budgets.tool_calls,
        })
    }
    pub(super) fn failures(&self, trace_errors: usize) -> Vec<String> {
        let Ok(state) = self.state.lock() else {
            return vec!["fixture host lock was poisoned".to_string()];
        };
        let mut failures = Vec::new();
        for (i, rule) in self.rules.iter().enumerate() {
            if state.used[i] != rule.times {
                failures.push(format!(
                    "mock rule {i}: consumed {}, expected {}",
                    state.used[i], rule.times
                ));
            }
        }
        if state.unexpected > 0 {
            failures.push(format!("{} unexpected mock calls", state.unexpected));
        }
        if trace_errors > state.simulated_errors {
            failures.push("unexpected failed calls in sandbox trace".to_string());
        }
        failures
    }
    pub(super) async fn shutdown(&self) {
        self.pool.shutdown().await;
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
        let ids: BTreeSet<&str> = self.rules.iter().map(|r| r.tool.as_str()).collect();
        let entries: Vec<_> = ids
            .into_iter()
            .filter_map(|id| {
                let (namespace, tool) = id.split_once("::")?;
                scope.allows(namespace, tool).then(|| {
                    CatalogDescriptor::tool_with_safety(
                        namespace,
                        tool,
                        "Offline fixture tool",
                        Some(serde_json::json!({"type":"object"})),
                        None,
                        Some(CodeModeToolSafety {
                            read_only: Some(true),
                            destructive: Some(false),
                        }),
                    )
                })
            })
            .collect();
        let json = serde_json::to_string(&entries)
            .map_err(|_| invalid("cannot serialize mock catalog"))?;
        Ok(ToolsRender {
            fingerprint: "fixture".to_string(),
            embedding_fingerprint: "fixture".to_string(),
            entries: Arc::from(entries),
            serialized_size: json.len(),
            catalog_json: Arc::from(json),
            withheld: Arc::from([]),
        })
    }
    async fn call_tool(
        &self,
        id: &str,
        params: Value,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        scope: &ToolScope,
        _ctx: ExecCtx,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| invalid("fixture host lock was poisoned"))?;
        state.attempts += 1;
        let allowed = id
            .split_once("::")
            .is_some_and(|(n, t)| scope.allows(n, t) && self.scope.allows(n, t));
        if state.attempts > self.max_calls || !allowed {
            state.unexpected += 1;
            return Err(invalid("mock call exceeded the fixture budget or scope").into());
        }
        let found = self.rules.iter().enumerate().find(|(i, rule)| {
            rule.tool == id
                && state.used[*i] < rule.times
                && rule.params.as_ref().is_none_or(|p| p == &params)
        });
        let Some((index, rule)) = found else {
            state.unexpected += 1;
            return Err(
                invalid("no remaining fixture rule matches this tool and its parameters").into(),
            );
        };
        state.used[index] += 1;
        if let Some(error) = &rule.error {
            state.simulated_errors += 1;
            return Err(ToolError::Sdk {
                sdk_kind: error.kind.clone(),
                message: error.message.clone(),
            }
            .into());
        }
        Ok(ToolCallOutcome {
            value: rule.response.clone(),
            ui: None,
        })
    }
    async fn resolve_snippet(
        &self,
        _name: &str,
        _input: Value,
    ) -> Result<ResolvedSnippet, ToolError> {
        Err(invalid(
            "nested saved snippets are not available in offline fixture tests",
        ))
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
