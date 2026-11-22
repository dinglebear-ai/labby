//! Core Code Mode value types: tool ids, tool descriptors, execution responses,
//! callers, surfaces, and the tool scope.
//!
//! Vocabulary is host-source-neutral. A tool is an opaque `id` of the form
//! `<namespace>::<tool>`; the kernel never learns what backs the namespace.

mod caller;
mod catalog;
mod history;
mod source_store;

pub use caller::{
    CodeModeCaller, CodeModeCallerCapabilities, CodeModeHostContext, CodeModeProviderContext,
    CodeModeSurface, destructive_permitted,
};
pub(crate) use catalog::CodeModeDiscoveryEntry;
pub use catalog::{
    CatalogDescriptor, CodeModeCatalogKind, CodeModeSnippetInputEntry, CodeModeToolSafety,
};
pub use history::{CodeModeHistory, CodeModeHistoryEntry, CodeModeHistoryKind};
pub use source_store::{CodeModeExecutionSource, CodeModeSourceLookup, CodeModeSourceStore};

use super::artifacts::CodeModeArtifactReceipt;
pub use super::execute::{CodeModeToolAccess, ToolScope};
pub(crate) use super::namespace_alias::{CodeModeToolId, CodeModeToolRef};
pub use super::namespace_alias::{namespaced_tool_id, split_namespaced_id};
use super::shape::CodeModeResultShapeMetadata;
use super::trace::CodeModeExecutedCallSchema;
pub use super::trace::{CodeModeExecutedCall, UiLink};
use crate::CodeModeCallError;
use crate::error::ToolError;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use std::fmt;

/// Serializable result envelope for one Code Mode execution.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CodeModeExecutionResponse {
    /// Stable execution identifier used for journals, artifacts, and promotion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    /// The final return value of the async function. None when the function
    /// returns undefined or throws (the throw case surfaces via ToolError).
    /// Explicit JavaScript `null` is represented as `Some(Value::Null)` and
    /// serializes as `"result": null`; undefined omits the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Metadata describing any model-facing result shaping that was applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_shaping: Option<CodeModeResultShapeMetadata>,
    /// Captured mcp-ui widget link (last-wins across the run). The MCP boundary
    /// attaches this as `_meta.ui` on the returned `CallToolResult` so the host
    /// renders the native widget. `None` when no widget-bearing call ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<UiLink>,
    /// Metadata for every host-brokered call attempted during execution.
    #[schemars(with = "Vec<CodeModeExecutedCallSchema>")]
    pub calls: Vec<CodeModeExecutedCall>,
    /// Console lines captured by this execution and returned in its completion frame.
    pub logs: Vec<String>,
    /// Artifacts written by the execution through the brokered artifact API.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<CodeModeArtifactReceipt>,
}

/// Pair of the unshaped execution response and the model-facing display response.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeModeExecutionOutcome {
    /// Full execution response before final result shaping.
    pub raw_response: CodeModeExecutionResponse,
    /// Response after configured model-facing result shaping.
    pub display_response: CodeModeExecutionResponse,
}

/// Code Mode execution failure plus the bounded call trace accumulated before failure.
#[derive(Debug, Clone)]
pub struct CodeModeExecutionError {
    error: CodeModeCallError,
    calls: Vec<CodeModeExecutedCall>,
}

impl CodeModeExecutionError {
    /// Construct an execution error while preserving the completed call trace.
    #[must_use]
    pub fn with_trace(
        error: impl Into<CodeModeCallError>,
        calls: Vec<CodeModeExecutedCall>,
    ) -> Self {
        Self {
            error: error.into(),
            calls,
        }
    }

    /// Return the stable error kind from the underlying call error.
    #[must_use]
    pub fn kind(&self) -> &str {
        self.error.kind()
    }

    /// Borrow the tool calls completed before the execution failed.
    #[must_use]
    pub fn calls(&self) -> &[CodeModeExecutedCall] {
        &self.calls
    }

    /// Consume the wrapper and return the underlying Code Mode call error.
    #[must_use]
    pub fn into_call_error(self) -> CodeModeCallError {
        self.error
    }

    /// Convert into the surface-neutral tool error representation.
    #[must_use]
    pub fn into_tool_error(self) -> ToolError {
        self.error.into_tool_error()
    }

    /// Contract-preserving collapse: keeps the inner [`CodeModeCallError`]'s
    /// refined metadata and evidence via `ToolError::Contract`. The executed
    /// `calls` trace stays dispatch-internal (it is not part of the error
    /// contract) — read it via [`Self::calls`] before converting when needed.
    #[must_use]
    pub fn into_contract_tool_error(self) -> ToolError {
        self.error.into_contract_tool_error()
    }
}

impl fmt::Display for CodeModeExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for CodeModeExecutionError {}

impl From<ToolError> for CodeModeExecutionError {
    fn from(error: ToolError) -> Self {
        Self::with_trace(error, Vec::new())
    }
}

impl From<CodeModeCallError> for CodeModeExecutionError {
    fn from(error: CodeModeCallError) -> Self {
        Self::with_trace(error, Vec::new())
    }
}
