//! Local Code Mode providers reserved by the Lab runtime.
//!
//! These providers are not upstream MCP tools. The runner still emits them
//! through the existing `ToolCall` protocol so promise settlement and tracing
//! stay uniform, but the parent intercepts the reserved namespaces before
//! `CodeModeHost::call_tool` can route to an upstream named `state` or `git`.

use serde_json::Value;

use crate::error::ToolError;
use crate::types::split_namespaced_id;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalProviderName {
    State,
    Git,
    /// OpenAPI-derived operations dispatched by `labby-openapi`. Unlike `State`/
    /// `Git`, `Openapi` carries a `<label>.<operationId>` method and is
    /// dispatched WITHOUT the `LOCAL_PROVIDER_LOCK`.
    Openapi,
}

impl LocalProviderName {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Git => "git",
            Self::Openapi => "openapi",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LocalProviderCall {
    pub(crate) provider: LocalProviderName,
    pub(crate) method: String,
    pub(crate) params: Value,
}

pub(crate) fn is_reserved_provider_namespace(namespace: &str) -> bool {
    matches!(namespace, "state" | "git" | "openapi")
}

pub(crate) fn try_parse_local_provider_call(
    id: &str,
) -> Result<Option<LocalProviderCall>, ToolError> {
    let trimmed = id.trim();
    let Some((namespace, method)) = split_namespaced_id(trimmed) else {
        if let Some((namespace, _)) = trimmed.split_once("::")
            && is_reserved_provider_namespace(namespace.trim())
        {
            return Err(ToolError::InvalidParam {
                message: "local provider method must not be empty".to_string(),
                param: "id".to_string(),
            });
        }
        return Ok(None);
    };

    let provider = match namespace {
        "state" => LocalProviderName::State,
        "git" => LocalProviderName::Git,
        "openapi" => LocalProviderName::Openapi,
        _ => return Ok(None),
    };

    if method.trim().is_empty() {
        return Err(ToolError::InvalidParam {
            message: "local provider method must not be empty".to_string(),
            param: "id".to_string(),
        });
    }

    Ok(Some(LocalProviderCall {
        provider,
        method: method.to_string(),
        params: Value::Null,
    }))
}
