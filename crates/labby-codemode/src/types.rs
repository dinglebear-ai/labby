//! Core Code Mode value types: tool ids, tool descriptors, execution responses,
//! callers, surfaces, and the tool scope.
//!
//! Vocabulary is host-source-neutral. A tool is an opaque `id` of the form
//! `<namespace>::<tool>`; the kernel never learns what backs the namespace.

use std::collections::{BTreeSet, VecDeque};
use std::fmt;

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::CodeModeCallError;
use crate::error::ToolError;
use crate::snippet::store::{SnippetInfo, SnippetInputSpec, SnippetInputType};

use super::artifacts::CodeModeArtifactReceipt;
use super::shape::CodeModeResultShapeMetadata;
use super::util::{invalid_code_mode_id, lab_action_unknown_tool};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeModeToolId {
    pub(crate) raw: String,
    pub(crate) reference: CodeModeToolRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodeModeToolRef {
    Tool { namespace: String, tool: String },
}

impl CodeModeToolId {
    /// Parse a raw `<namespace>::<tool>` string into a `CodeModeToolId`.
    ///
    /// This is an inherent shim over the `FromStr` impl so call sites that
    /// already use `.parse(…)` or `CodeModeToolId::parse(…)` continue to
    /// compile without churn.
    pub(crate) fn parse(raw: &str) -> Result<Self, ToolError> {
        raw.parse()
    }
}

impl std::str::FromStr for CodeModeToolId {
    type Err = ToolError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(invalid_code_mode_id("Code Mode tool id must not be empty"));
        }

        if raw.starts_with("lab::") {
            return Err(lab_action_unknown_tool());
        }

        // Shared `<namespace>::<tool>` splitter — also used by `ToolExecuteSelector`.
        if let Some((namespace, tool)) = split_namespaced_id(raw) {
            return Ok(Self {
                raw: raw.to_string(),
                reference: CodeModeToolRef::Tool {
                    namespace: namespace.to_string(),
                    tool: tool.to_string(),
                },
            });
        }

        Err(invalid_code_mode_id(
            "Code Mode ids must use <namespace>::<tool>",
        ))
    }
}

/// Split a `<namespace>::<tool>` string into its two trimmed parts.
///
/// Returns `None` when the string has a wrong number of `::` separators or
/// when either part is empty after trimming. Used by both `CodeModeToolId` and
/// `ToolExecuteSelector` to avoid duplicating the splitting logic.
pub fn split_namespaced_id(raw: &str) -> Option<(&str, &str)> {
    let mut parts = raw.split("::");
    let namespace = parts.next()?.trim();
    let tool = parts.next()?.trim();
    // Ensure there is no third segment (e.g. `a::b::c` is invalid).
    if parts.next().is_some() {
        return None;
    }
    if namespace.is_empty() || tool.is_empty() {
        return None;
    }
    Some((namespace, tool))
}

/// Build the canonical `<namespace>::<tool>` identifier used by Code Mode.
#[must_use]
pub fn namespaced_tool_id(namespace: &str, tool: &str) -> String {
    format!("{namespace}::{tool}")
}

/// Search/describe catalog entry for a host tool or reusable snippet.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolDescriptor {
    /// Exact upstream-tool declaration for snippets; absent for normal tools
    /// and legacy snippets. An explicit empty declaration remains visible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<crate::snippet::tool_declarations::SnippetToolDeclarations>,
    /// Catalog entry class.
    pub kind: CodeModeCatalogKind,
    /// Stable Code Mode identifier.
    pub id: String,
    /// Unqualified tool or snippet name.
    pub name: String,
    /// Host namespace, or `snippet` for reusable snippets.
    pub namespace: String,
    /// Human-readable catalog description.
    pub description: String,
    /// Advisory intrinsic safety facts from the live descriptor. Dispatch
    /// remains authoritative; absence means unknown, never `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety: Option<CodeModeToolSafety>,
    /// JSON Schema for the input payload when one is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    /// JSON Schema for the result payload when one is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// Compact JavaScript/TypeScript call signature shown by discovery.
    pub signature: String,
    /// TypeScript declaration text emitted by `describe`.
    pub dts: String,
    /// Optional search/discovery tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Declared snippet input entries. Empty for normal tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<CodeModeSnippetInputEntry>,
}

/// Compact, source-neutral safety facts for discovery presentation.
///
/// Optional booleans preserve fail-closed semantics: an omitted fact is
/// unknown. This type deliberately carries no approval/access policy or raw
/// upstream annotation text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeModeToolSafety {
    /// Whether the upstream explicitly classifies the tool as read-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    /// Whether the upstream explicitly classifies the tool as destructive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destructive: Option<bool>,
}

impl CodeModeToolSafety {
    /// Return `true` when neither safety fact is known.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.read_only.is_none() && self.destructive.is_none()
    }
}

/// Kind of object represented by a Code Mode discovery descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum CodeModeCatalogKind {
    /// Host-provided callable tool.
    Tool,
    /// Reusable Code Mode snippet.
    Snippet,
}

/// Named snippet input plus its validation/default specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeModeSnippetInputEntry {
    /// Input parameter name.
    pub name: String,
    /// Input type, requirement, and optional default.
    #[serde(flatten)]
    pub spec: SnippetInputSpec,
}

impl ToolDescriptor {
    /// Build a tool descriptor for a host-provided tool (`<namespace>::<tool>`).
    ///
    /// The host passes already-sanitized JSON Schemas; this constructor only
    /// generates the TypeScript signature / `.d.ts` for the in-sandbox catalog.
    #[must_use]
    pub fn tool(
        namespace: &str,
        tool: &str,
        description: &str,
        schema: Option<Value>,
        output_schema: Option<Value>,
    ) -> Self {
        Self::tool_with_safety(namespace, tool, description, schema, output_schema, None)
    }

    #[must_use]
    pub fn tool_with_safety(
        namespace: &str,
        tool: &str,
        description: &str,
        schema: Option<Value>,
        output_schema: Option<Value>,
        safety: Option<CodeModeToolSafety>,
    ) -> Self {
        let types = super::ts_signatures::generate_tool_types(
            namespace,
            tool,
            description,
            schema.as_ref(),
            output_schema.as_ref(),
        );
        Self {
            kind: CodeModeCatalogKind::Tool,
            tools: None,
            id: namespaced_tool_id(namespace, tool),
            name: tool.to_string(),
            namespace: namespace.to_string(),
            description: description.to_string(),
            safety,
            schema,
            output_schema,
            signature: types.signature,
            dts: types.dts,
            tags: Vec::new(),
            inputs: Vec::new(),
        }
    }

    /// Build a catalog descriptor for a stored or built-in snippet.
    #[must_use]
    pub fn snippet(info: &SnippetInfo) -> Self {
        let description = info
            .description
            .clone()
            .unwrap_or_else(|| format!("Code Mode snippet `{}`", info.name));
        let inputs = info
            .inputs
            .iter()
            .map(|(name, spec)| CodeModeSnippetInputEntry {
                name: name.clone(),
                spec: spec.clone(),
            })
            .collect::<Vec<_>>();
        Self {
            kind: CodeModeCatalogKind::Snippet,
            tools: info.tools.clone(),
            id: format!("snippet::{}", info.name),
            name: info.name.clone(),
            namespace: "snippet".to_string(),
            description,
            safety: None,
            schema: Some(snippet_inputs_schema(&info.inputs)),
            // Deliberate: a snippet returns an arbitrary JavaScript value, so
            // there is no honest output schema to publish. `dts` is likewise
            // empty, so `describe` renders inputs with no type section. This is
            // the current contract in docs/contracts/mcp-tool-output.md.
            output_schema: None,
            signature: format!("codemode.run({:?}, input?)", info.name),
            dts: String::new(),
            tags: info.tags.clone(),
            inputs,
        }
    }
}

fn snippet_inputs_schema(inputs: &std::collections::BTreeMap<String, SnippetInputSpec>) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (name, spec) in inputs {
        if spec.required {
            required.push(Value::String(name.clone()));
        }
        let mut field = serde_json::Map::new();
        if let Some(json_type) = snippet_input_json_type(spec.ty) {
            field.insert("type".to_string(), Value::String(json_type.to_string()));
        }
        if let Some(description) = &spec.description {
            field.insert(
                "description".to_string(),
                Value::String(description.clone()),
            );
        }
        if let Some(default) = &spec.default {
            field.insert("default".to_string(), default.clone());
        }
        properties.insert(name.clone(), Value::Object(field));
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn snippet_input_json_type(ty: SnippetInputType) -> Option<&'static str> {
    match ty {
        SnippetInputType::String => Some("string"),
        SnippetInputType::Integer => Some("integer"),
        SnippetInputType::Number => Some("number"),
        SnippetInputType::Boolean => Some("boolean"),
        SnippetInputType::Object => Some("object"),
        SnippetInputType::Array => Some("array"),
        SnippetInputType::Json => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CodeModeDiscoveryEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tools: Option<crate::snippet::tool_declarations::SnippetToolDeclarations>,
    pub(crate) kind: CodeModeCatalogKind,
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) namespace: String,
    pub(crate) name: String,
    pub(crate) helper: String,
    pub(crate) description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) safety: Option<CodeModeToolSafety>,
    pub(crate) signature: String,
    pub(crate) tags: Vec<String>,
    pub(crate) inputs: Vec<CodeModeSnippetInputEntry>,
    /// Input JSON Schema for the underlying tool. Carried from the catalog for
    /// describe-time type rendering, but skipped from the serialized search
    /// index so the per-execute preamble stays compact.
    #[serde(skip)]
    pub(crate) schema: Option<Value>,
    /// Generated `.d.ts` declaration block for the tool.
    #[serde(skip)]
    pub(crate) dts: String,
}

impl CodeModeDiscoveryEntry {
    #[must_use]
    pub(crate) fn from_catalog(entry: &ToolDescriptor) -> Self {
        let (path, helper) = match entry.kind {
            CodeModeCatalogKind::Tool => {
                let namespace = super::preamble::namespace_segment(&entry.namespace);
                let name = super::preamble::tool_name_to_snake(&entry.name);
                (
                    format!("{namespace}.{name}"),
                    format!("codemode.{namespace}.{name}"),
                )
            }
            CodeModeCatalogKind::Snippet => (
                format!("snippet.{}", entry.name),
                format!("codemode.run({:?}, input)", entry.name),
            ),
        };
        Self {
            kind: entry.kind,
            tools: entry.tools.clone(),
            id: entry.id.clone(),
            path,
            namespace: entry.namespace.clone(),
            name: entry.name.clone(),
            helper,
            description: entry.description.clone(),
            safety: entry.safety,
            signature: entry.signature.clone(),
            tags: entry.tags.clone(),
            inputs: entry.inputs.clone(),
            schema: entry.schema.clone(),
            dts: entry.dts.clone(),
        }
    }
}

/// A captured MCP Apps (mcp-ui) widget link.
///
/// Recorded by the host at the tool-call boundary when a tool result carries
/// `_meta.ui.resourceUri`, before the result envelope is discarded. `ui_meta`
/// holds the `_meta.ui` object verbatim (including `resourceUri`) so the final
/// `execute` response can mirror the widget identically.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiLink {
    /// Raw `_meta.ui` object advertised by the tool result.
    pub ui_meta: Value,
}

/// Serializable result envelope for one Code Mode execution.
#[derive(Debug, Clone, PartialEq, Serialize)]
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
    pub calls: Vec<CodeModeExecutedCall>,
    /// Captured console.log/warn/error lines from the runner. Sourced from the
    /// javy runner subprocess (drained from its stderr); the current javy path
    /// returns no protocol-carried logs, so this is empty until console capture
    /// is wired through.
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

/// Lightweight metadata for one host-brokered tool call. Cloudflare parity:
/// the per-call result payload is NOT carried here — only the model needs the
/// final `result`. Recording full per-call results bloated context and risked
/// leaking secrets through the truncation preview.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeModeExecutedCall {
    /// Fully-qualified `<namespace>::<tool>` identifier.
    pub id: String,
    /// Whether the brokered call completed successfully.
    pub ok: bool,
    /// Tool-call duration in milliseconds.
    pub elapsed_ms: u128,
    /// Offset from execution start to this call's dispatch, in ms. `None` for
    /// synthetic entries (budget rejections, artifact pseudo-calls) that have
    /// no meaningful dispatch time. Lets the inspector render a true waterfall
    /// (sequential vs `Promise.all` fan-out) instead of bare duration bars.
    pub start_ms: Option<u128>,
    /// Redacted/capped params captured at the broker boundary. Raw params must
    /// never be stored in this public trace type.
    pub params: Option<Value>,
    /// Stable error kind when the brokered call failed.
    pub error_kind: Option<String>,
    /// Captured MCP Apps (mcp-ui) widget link for this specific tool call.
    /// Stored as metadata only; the call result payload stays out of the trace.
    pub ui: Option<UiLink>,
}

impl Serialize for CodeModeExecutedCall {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (namespace, tool) = split_code_mode_call_id(&self.id);
        let mut state = serializer.serialize_struct("CodeModeExecutedCall", 9)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("namespace", namespace)?;
        state.serialize_field("tool", tool)?;
        state.serialize_field("ok", &self.ok)?;
        state.serialize_field("elapsed_ms", &self.elapsed_ms)?;
        if let Some(start_ms) = &self.start_ms {
            state.serialize_field("start_ms", start_ms)?;
        }
        if let Some(params) = &self.params {
            state.serialize_field("params", params)?;
        }
        if let Some(error_kind) = &self.error_kind {
            state.serialize_field("error_kind", error_kind)?;
        }
        if let Some(ui) = &self.ui {
            state.serialize_field("ui", &ui.ui_meta)?;
        }
        state.end()
    }
}

#[must_use]
pub(crate) fn split_code_mode_call_id(id: &str) -> (&str, &str) {
    id.split_once("::")
        .map_or(("", id), |(namespace, tool)| (namespace, tool))
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

/// Kind of operation recorded in the bounded Code Mode history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeModeHistoryKind {
    /// JavaScript execution entry.
    Execute,
}

/// Bounded observability record for one Code Mode operation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeHistoryEntry {
    /// Stable execution identifier, when one was assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    /// Monotonic sequence number assigned by the in-memory history.
    pub seq: u64,
    /// Route-scope label under which the operation executed.
    pub route_scope: String,
    /// History operation class.
    pub kind: CodeModeHistoryKind,
    /// Whether the operation completed successfully.
    pub ok: bool,
    /// Operation duration in milliseconds.
    pub elapsed_ms: u128,
    /// Estimated input token count, when measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<usize>,
    /// Estimated output token count, when measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<usize>,
    /// Stable failure kind, when the operation failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    /// Bounded tool-call trace captured for the operation.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<CodeModeExecutedCall>,
    /// Search result count for discovery operations that populate this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_count: Option<usize>,
}

/// In-memory, entry-count- and byte-bounded Code Mode operation history.
#[derive(Debug, Clone)]
pub struct CodeModeHistory {
    entries: VecDeque<CodeModeHistoryEntry>,
    /// Accumulated serialized byte estimate for all entries in `entries`.
    ///
    /// Maintained as a running total to avoid re-serializing the entire deque
    /// on every push or eviction. Updated when entries are added or removed.
    /// The estimate uses the serialized JSON size of each individual entry; the
    /// VecDeque framing bytes (brackets, commas) are a constant ~2 bytes and are
    /// ignored — acceptable given the ~1 KB min entry size and 256 KB default cap.
    running_bytes: usize,
    max_entries: usize,
    max_bytes: usize,
    next_seq: u64,
}

impl Default for CodeModeHistory {
    fn default() -> Self {
        Self::new(50, 256 * 1024)
    }
}

impl CodeModeHistory {
    /// Create a bounded history with minimum-safe entry and byte limits.
    #[must_use]
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            running_bytes: 0,
            max_entries: max_entries.max(1),
            max_bytes: max_bytes.max(1024),
            next_seq: 1,
        }
    }

    /// Append an entry, assign its sequence number, and evict oldest entries as needed.
    pub fn push(&mut self, mut entry: CodeModeHistoryEntry) {
        entry.seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        let entry_bytes = entry_serialized_size(&entry);
        self.entries.push_back(entry);
        self.running_bytes = self.running_bytes.saturating_add(entry_bytes);
        self.trim();
    }

    /// Return the retained history entries in oldest-to-newest order.
    #[must_use]
    pub fn snapshot(&self) -> Vec<CodeModeHistoryEntry> {
        self.entries.iter().cloned().collect()
    }

    /// Return retained history filtered to one route scope, or all entries for `None`.
    #[must_use]
    pub fn snapshot_for_route_scope(&self, route_scope: Option<&str>) -> Vec<CodeModeHistoryEntry> {
        match route_scope {
            None => self.snapshot(),
            Some(route_scope) => self
                .entries
                .iter()
                .filter(|entry| entry.route_scope == route_scope)
                .cloned()
                .collect(),
        }
    }

    fn trim(&mut self) {
        while self.entries.len() > self.max_entries {
            if let Some(evicted) = self.entries.pop_front() {
                self.running_bytes = self
                    .running_bytes
                    .saturating_sub(entry_serialized_size(&evicted));
            }
        }
        while self.running_bytes > self.max_bytes && self.entries.len() > 1 {
            if let Some(evicted) = self.entries.pop_front() {
                self.running_bytes = self
                    .running_bytes
                    .saturating_sub(entry_serialized_size(&evicted));
            }
        }
        if self.running_bytes > self.max_bytes {
            if let Some(entry) = self.entries.pop_back() {
                let old_bytes = entry_serialized_size(&entry);
                let sentinel = Self::oversized_entry_sentinel(entry.seq, entry.kind);
                let sentinel_bytes = entry_serialized_size(&sentinel);
                self.running_bytes = self
                    .running_bytes
                    .saturating_sub(old_bytes)
                    .saturating_add(sentinel_bytes);
                self.entries.push_back(sentinel);
            }
        }
    }

    fn oversized_entry_sentinel(seq: u64, kind: CodeModeHistoryKind) -> CodeModeHistoryEntry {
        CodeModeHistoryEntry {
            execution_id: None,
            seq,
            route_scope: "root".to_string(),
            kind,
            ok: false,
            elapsed_ms: 0,
            input_tokens: None,
            output_tokens: None,
            error_kind: Some("history_entry_too_large".to_string()),
            calls: Vec::new(),
            match_count: None,
        }
    }
}

/// Ephemeral execution source retained so an administrator can promote it into a snippet.
#[derive(Debug, Clone)]
pub struct CodeModeExecutionSource {
    /// Execution identifier used as the promotion lookup key.
    pub execution_id: String,
    /// Source creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Redacted/stable actor key that owns this source.
    pub actor_key: Option<String>,
    /// Whether the original caller had administrative scope.
    pub is_admin: bool,
    /// Route scope in which the source executed.
    pub route_scope: String,
    /// Product surface that initiated the execution.
    pub surface: CodeModeSurface,
    /// Capability-scope fingerprint captured at execution time.
    pub capability_filter_fingerprint: String,
    /// Original JavaScript source eligible for promotion.
    pub code: String,
}

/// Authorization/scope context used when resolving an execution source for promotion.
#[derive(Debug, Clone)]
pub struct CodeModeSourceLookup {
    /// Actor key that must own the source.
    pub actor_key: Option<String>,
    /// Whether the resolving caller has administrative scope.
    pub is_admin: bool,
    /// Current route scope.
    pub route_scope: String,
    /// Current capability-scope fingerprint.
    pub capability_filter_fingerprint: String,
}

/// Bounded in-memory store of ephemeral Code Mode source suitable for snippet promotion.
#[derive(Debug, Clone)]
pub struct CodeModeSourceStore {
    entries: VecDeque<CodeModeExecutionSource>,
    running_bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl Default for CodeModeSourceStore {
    fn default() -> Self {
        Self::new(50, 512 * 1024)
    }
}

impl CodeModeSourceStore {
    /// Create a source store with minimum-safe entry and byte limits.
    #[must_use]
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            running_bytes: 0,
            max_entries: max_entries.max(1),
            max_bytes: max_bytes.max(1024),
        }
    }

    /// Retain a source when it fits the byte budget, evicting oldest entries as needed.
    pub fn push(&mut self, source: CodeModeExecutionSource) {
        let bytes = source.code.len();
        if bytes > self.max_bytes {
            return;
        }
        self.running_bytes = self.running_bytes.saturating_add(bytes);
        self.entries.push_back(source);
        while self.entries.len() > self.max_entries || self.running_bytes > self.max_bytes {
            if let Some(evicted) = self.entries.pop_front() {
                self.running_bytes = self.running_bytes.saturating_sub(evicted.code.len());
            } else {
                break;
            }
        }
    }

    /// Resolve a promotion source after enforcing admin, route, capability, and actor ownership.
    #[must_use]
    pub fn resolve(
        &self,
        execution_id: &str,
        lookup: &CodeModeSourceLookup,
    ) -> Result<CodeModeExecutionSource, ToolError> {
        let Some(source) = self
            .entries
            .iter()
            .find(|entry| entry.execution_id == execution_id)
            .cloned()
        else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_execution".to_string(),
                message: "Code Mode promotion source is ephemeral and may have expired, been evicted, lived in another host process, or disappeared after restart".to_string(),
            });
        };
        if !lookup.is_admin {
            return Err(ToolError::Forbidden {
                message: "promoting Code Mode executions requires lab:admin".to_string(),
                required_scopes: vec!["lab:admin".to_string()],
            });
        }
        if source.route_scope != lookup.route_scope
            || !source_capability_within_lookup(
                &source.capability_filter_fingerprint,
                &lookup.capability_filter_fingerprint,
            )
        {
            return Err(ToolError::Forbidden {
                message: "Code Mode promotion source is outside this route or capability scope"
                    .to_string(),
                required_scopes: vec!["lab:admin".to_string()],
            });
        }
        if source.actor_key != lookup.actor_key {
            return Err(ToolError::Forbidden {
                message: "Code Mode promotion source belongs to a different actor".to_string(),
                required_scopes: vec!["lab:admin".to_string()],
            });
        }
        Ok(source)
    }
}

fn source_capability_within_lookup(source: &str, lookup: &str) -> bool {
    if source == lookup {
        return true;
    }

    let Some(source_namespaces) = capability_fingerprint_namespaces(source) else {
        return false;
    };
    let Some(lookup_namespaces) = capability_fingerprint_namespaces(lookup) else {
        return false;
    };

    match (source_namespaces, lookup_namespaces) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(source), Some(lookup)) => source.is_subset(&lookup),
    }
}

fn capability_fingerprint_namespaces(fingerprint: &str) -> Option<Option<BTreeSet<String>>> {
    if let Ok(value) = serde_json::from_str::<Value>(fingerprint) {
        let namespaces = value.get("namespaces")?;
        if namespaces.is_null() {
            return Some(None);
        }
        let set = namespaces
            .as_array()?
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>();
        return Some(Some(set));
    }

    let namespaces = fingerprint
        .split(';')
        .find_map(|part| part.strip_prefix("namespaces="))?;
    if namespaces == "*" {
        return Some(None);
    }
    let set = namespaces
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    Some(Some(set))
}

/// Serialized byte size of a single history entry.
///
/// Used to maintain the `running_bytes` counter without re-serializing the
/// entire deque on every mutation. Falls back to `usize::MAX` on a (very
/// unlikely) serialization error so the history is conservatively treated as
/// over-budget rather than silently growing without bound.
fn entry_serialized_size(entry: &CodeModeHistoryEntry) -> usize {
    serde_json::to_vec(entry)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX / 2)
}

/// Caller identity and authorization facts presented to the host-neutral Code Mode kernel.
#[derive(Clone, PartialEq, Eq)]
pub enum CodeModeCaller {
    /// Trusted local caller that is granted all Code Mode capabilities.
    TrustedLocal,
    /// Authenticated/scoped caller with host-computed capability booleans.
    Scoped {
        /// Capabilities granted by the host surface.
        capabilities: CodeModeCallerCapabilities,
        /// JWT `sub` claim for the caller, when available. The host decides how
        /// to map this onto its own credential/identity model when resolving
        /// and calling tools; the kernel itself never interprets it.
        sub: Option<String>,
    },
    /// Scoped caller carrying an opaque context minted by the host for a private
    /// in-process hop. The kernel never interprets the token.
    ScopedPrivate {
        capabilities: CodeModeCallerCapabilities,
        sub: Option<String>,
        context_token: String,
    },
    /// Scoped caller carrying a credential for one host-owned external
    /// provider. The kernel never interprets or propagates this credential.
    ScopedHostProvider {
        capabilities: CodeModeCallerCapabilities,
        sub: Option<String>,
        provider_token: String,
        provider_request_id: String,
    },
}

impl fmt::Debug for CodeModeCaller {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustedLocal => formatter.write_str("TrustedLocal"),
            Self::Scoped { capabilities, sub } => formatter
                .debug_struct("Scoped")
                .field("capabilities", capabilities)
                .field("sub", sub)
                .finish(),
            Self::ScopedPrivate {
                capabilities, sub, ..
            } => formatter
                .debug_struct("ScopedPrivate")
                .field("capabilities", capabilities)
                .field("sub", sub)
                .field("context_token", &"[REDACTED]")
                .finish(),
            Self::ScopedHostProvider {
                capabilities,
                sub,
                provider_request_id,
                ..
            } => formatter
                .debug_struct("ScopedHostProvider")
                .field("capabilities", capabilities)
                .field("sub", sub)
                .field("provider_token", &"[REDACTED]")
                .field("provider_request_id", provider_request_id)
                .finish(),
        }
    }
}

/// Host-computed authorization facts for a scoped Code Mode caller.
///
/// The Code Mode kernel stays independent of Lab's OAuth scope names; surface
/// adapters translate their own auth model into these booleans before calling
/// into the kernel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodeModeCallerCapabilities {
    /// Whether read-only Code Mode execution is allowed.
    pub can_read: bool,
    /// Whether full Code Mode execution is allowed.
    pub can_execute: bool,
    /// Whether reusable snippets may be listed/resolved/executed.
    pub can_use_snippets: bool,
    /// Whether the caller carries administrative scope.
    pub is_admin: bool,
}

/// Product surface that initiated a Code Mode execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeModeSurface {
    /// Model Context Protocol surface.
    Mcp,
    /// Local command-line interface.
    Cli,
    Api,
}

impl CodeModeSurface {
    /// Stable lowercase surface tag (`"mcp"`, `"cli"`, or `"api"`) used by hosts when
    /// building their own runtime-owner / logging context.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            CodeModeSurface::Mcp => "mcp",
            CodeModeSurface::Cli => "cli",
            CodeModeSurface::Api => "api",
        }
    }
}

/// Whether a destructive tool call is permitted for this caller.
/// Code Mode execution is already scope-gated; do not add a second host-side
/// confirmation gate based on tool catalog metadata. Hosts call this when
/// applying destructive-tool policy.
#[must_use]
pub fn destructive_permitted(surface: CodeModeSurface, caller: &CodeModeCaller) -> bool {
    match surface {
        CodeModeSurface::Cli => true,
        CodeModeSurface::Mcp | CodeModeSurface::Api => caller.can_execute(),
    }
}

impl CodeModeCaller {
    /// Return whether the caller may use reusable Code Mode snippets.
    #[must_use]
    pub fn can_use_snippets(&self) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { capabilities, .. }
            | Self::ScopedPrivate { capabilities, .. }
            | Self::ScopedHostProvider { capabilities, .. } => capabilities.can_use_snippets,
        }
    }

    /// Return whether the caller may execute full Code Mode workloads.
    #[must_use]
    pub fn can_execute(&self) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { capabilities, .. }
            | Self::ScopedPrivate { capabilities, .. }
            | Self::ScopedHostProvider { capabilities, .. } => capabilities.can_execute,
        }
    }

    /// Return whether the caller may use the read-only Code Mode surface.
    #[must_use]
    pub fn can_read(&self) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { capabilities, .. }
            | Self::ScopedPrivate { capabilities, .. }
            | Self::ScopedHostProvider { capabilities, .. } => capabilities.can_read,
        }
    }

    /// Whether this caller carries the `lab:admin` scope (trusted-local always
    /// counts as admin). Hosts use this when mapping the caller onto their own
    /// credential model.
    #[must_use]
    pub fn is_admin(&self) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { capabilities, .. }
            | Self::ScopedPrivate { capabilities, .. }
            | Self::ScopedHostProvider { capabilities, .. } => capabilities.is_admin,
        }
    }

    /// The caller's `sub` identity, when available. `None` for trusted-local.
    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        match self {
            Self::TrustedLocal => None,
            Self::Scoped { sub, .. }
            | Self::ScopedPrivate { sub, .. }
            | Self::ScopedHostProvider { sub, .. } => sub.as_deref(),
        }
    }

    /// Return the opaque host-provider credential, when the surface supplied
    /// one. It is intentionally unavailable to generic upstream propagation.
    #[must_use]
    pub fn host_provider_token(&self) -> Option<&str> {
        match self {
            Self::ScopedHostProvider { provider_token, .. } => Some(provider_token),
            _ => None,
        }
    }

    /// Return the parent provider correlation identifier verified by the host.
    #[must_use]
    pub fn host_provider_request_id(&self) -> Option<&str> {
        match self {
            Self::ScopedHostProvider {
                provider_request_id,
                ..
            } => Some(provider_request_id),
            _ => None,
        }
    }
}

/// Access mode applied by a Code Mode tool scope.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CodeModeToolAccess {
    /// Permit only tools the host independently verifies as read-only.
    ReadOnly,
    /// Permit normal scoped tool access.
    #[default]
    Full,
}

/// Namespace/tool allowlist and access mode applied to one Code Mode execution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolScope {
    namespaces: Option<BTreeSet<String>>,
    tools: BTreeSet<String>,
    access: CodeModeToolAccess,
}

impl ToolScope {
    /// Create an unscoped-by-default tool scope from optional namespace/tool filters.
    #[must_use]
    pub fn new(namespaces: Vec<String>, tools: Vec<String>) -> Self {
        Self::new_inner(None, namespaces, tools)
    }

    /// Create a scope where an empty namespace list means no namespaces are allowed.
    #[must_use]
    pub fn scoped_namespaces(namespaces: Vec<String>, tools: Vec<String>) -> Self {
        Self::new_inner(Some(BTreeSet::new()), namespaces, tools)
    }

    fn new_inner(
        scoped_default: Option<BTreeSet<String>>,
        namespaces: Vec<String>,
        tools: Vec<String>,
    ) -> Self {
        fn clean_set(values: Vec<String>) -> BTreeSet<String> {
            values
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect()
        }
        let namespaces = clean_set(namespaces);
        Self {
            namespaces: if namespaces.is_empty() {
                scoped_default
            } else {
                Some(namespaces)
            },
            tools: clean_set(tools),
            access: CodeModeToolAccess::Full,
        }
    }

    /// Restrict this execution to tools that the host can prove are read-only.
    /// Hosts must enforce this again against the live tool descriptor at the
    /// invocation boundary; discovery filtering alone is not authorization.
    #[must_use]
    pub fn read_only(mut self) -> Self {
        self.access = CodeModeToolAccess::ReadOnly;
        self
    }

    /// Return whether the scope is restricted to verified read-only tools.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.access == CodeModeToolAccess::ReadOnly
    }

    /// Return whether a namespace/tool pair is included by the configured filters.
    #[must_use]
    pub fn allows(&self, namespace: &str, tool: &str) -> bool {
        (self
            .namespaces
            .as_ref()
            .is_none_or(|namespaces| namespaces.contains(namespace)))
            && (self.tools.is_empty()
                || self.tools.contains(tool)
                || self.tools.contains(&namespaced_tool_id(namespace, tool)))
    }

    /// Return whether any namespace, tool, or read-only restriction is active.
    #[must_use]
    pub fn is_scoped(&self) -> bool {
        self.namespaces.is_some()
            || !self.tools.is_empty()
            || self.access == CodeModeToolAccess::ReadOnly
    }

    /// Borrow the explicit namespace allowlist, if namespace scoping is active.
    #[must_use]
    pub fn allowed_namespaces(&self) -> Option<&BTreeSet<String>> {
        self.namespaces.as_ref()
    }

    /// Return a stable serialized fingerprint of this scope for ownership checks and caches.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        serde_json::json!({
            "access": match self.access {
                CodeModeToolAccess::ReadOnly => "read_only",
                CodeModeToolAccess::Full => "full",
            },
            "namespaces": self.namespaces.as_ref().map(|set| set.iter().cloned().collect::<Vec<_>>()),
            "tools": self.tools.iter().cloned().collect::<Vec<_>>(),
        })
        .to_string()
    }
}
