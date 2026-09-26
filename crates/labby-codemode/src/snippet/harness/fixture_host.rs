//! Deterministic fake tools behind the same host seam as live Code Mode.

use super::{FixtureCall, FixtureResponse, invalid};
use crate::error::ToolError;
use crate::host::{CodeModeHost, ExecCtx, ResolvedSnippet, ToolCallOutcome, ToolsRender};
use crate::pool::RunnerPool;
use crate::{
    CatalogDescriptor, CodeModeCallError, CodeModeCaller, CodeModeCatalogKind, CodeModeSurface,
    CodeModeToolSafety, ToolScope,
};
use labby_runtime::CodeModeConfig;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

pub(super) struct FixtureHost {
    rules: Vec<FixtureCall>,
    used: Mutex<Vec<usize>>,
    pool: RunnerPool,
    config: CodeModeConfig,
    http: reqwest::Client,
}

impl FixtureHost {
    pub(super) fn new(rules: Vec<FixtureCall>, config: CodeModeConfig) -> Result<Self, ToolError> {
        Ok(Self {
            used: Mutex::new(vec![0; rules.len()]),
            rules,
            pool: RunnerPool::from_env()?,
            config,
            http: labby_openapi::http::build_dispatch_client()
                .map_err(|_| invalid("unable to construct fixture host"))?,
        })
    }

    pub(super) fn scope(&self) -> ToolScope {
        let namespaces = self
            .rules
            .iter()
            .filter_map(|r| r.tool.split_once("::"))
            .map(|(namespace, _)| namespace.to_owned())
            .collect();
        let tools = self.rules.iter().map(|r| r.tool.clone()).collect();
        // Even an empty fixture must be scoped: empty ordinary scope means all.
        ToolScope::scoped_namespaces(namespaces, tools).read_only()
    }

    pub(super) fn remaining(&self) -> Result<Vec<String>, ToolError> {
        let used = self
            .used
            .lock()
            .map_err(|_| invalid("fixture state lock poisoned"))?;
        Ok(self
            .rules
            .iter()
            .zip(used.iter())
            .enumerate()
            .filter(|(_, (rule, count))| **count != rule.times)
            .map(|(i, (rule, count))| {
                format!(
                    "rule {i} ({}): expected {}, observed {count}",
                    rule.tool, rule.times
                )
            })
            .collect())
    }

    fn respond(&self, id: &str, params: &Value) -> Result<Value, CodeModeCallError> {
        let mut used = self
            .used
            .lock()
            .map_err(|_| invalid("fixture state lock poisoned"))?;
        let index = self.rules.iter().enumerate().position(|(i, rule)| {
            rule.tool == id && used[i] < rule.times && subset(&rule.params, params)
        });
        let Some(index) = index else {
            return Err(ToolError::Sdk {
                sdk_kind: "fixture_unexpected_call".to_owned(),
                message: format!("no unused fixture rule matches {id}; live fallback is disabled"),
            }
            .into());
        };
        used[index] += 1;
        match &self.rules[index].response {
            FixtureResponse::Returns(value) => Ok(value.clone()),
            FixtureResponse::Error(error) => Err(ToolError::Sdk {
                sdk_kind: error.kind.clone(),
                message: error.message.clone(),
            }
            .into()),
        }
    }
}

pub(super) fn subset(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => expected
            .iter()
            .all(|(key, value)| actual.get(key).is_some_and(|other| subset(value, other))),
        _ => expected == actual,
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
            .filter_map(|id| id.split_once("::"))
            .filter(|(namespace, tool)| scope.allows(namespace, tool))
            .map(|(namespace, tool)| {
                CatalogDescriptor::tool_with_safety(
                    namespace,
                    tool,
                    "Offline fixture, not a live upstream contract",
                    None,
                    None,
                    Some(CodeModeToolSafety {
                        read_only: Some(true),
                        destructive: Some(false),
                    }),
                )
            })
            .collect();
        let catalog_json = serde_json::to_string(&entries)
            .map_err(|_| invalid("unable to serialize fixture catalog"))?;
        Ok(ToolsRender {
            fingerprint: "snippet-fixture".to_owned(),
            embedding_fingerprint: "snippet-fixture".to_owned(),
            entries: Arc::from(entries),
            serialized_size: catalog_json.len(),
            catalog_json: Arc::from(catalog_json),
            withheld: Arc::from([]),
        })
    }

    async fn call_tool(
        &self,
        id: &str,
        params: Value,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
        _ctx: ExecCtx,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        Ok(ToolCallOutcome {
            value: self.respond(id, &params)?,
            ui: None,
        })
    }

    async fn resolve_snippet(
        &self,
        _name: &str,
        _input: Value,
    ) -> Result<ResolvedSnippet, ToolError> {
        Err(invalid(
            "nested snippets are not supported by offline fixtures",
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
        self.http.clone()
    }
}
