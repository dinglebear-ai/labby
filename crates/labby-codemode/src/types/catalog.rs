use super::namespaced_tool_id;
use crate::snippet::store::{SnippetInfo, SnippetInputSpec, SnippetInputType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Source-neutral search/describe entry in the Code Mode capability catalog.
///
/// A descriptor carries discovery metadata only. Its presence never grants
/// execution or access; dispatch/load operations remain separately authorized.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CatalogDescriptor {
    /// Exact upstream-tool declaration for snippets; absent for normal tools
    /// and legacy snippets. An explicit empty declaration remains visible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<crate::snippet::tool_declarations::SnippetToolDeclarations>,
    /// Catalog entry class.
    pub kind: CodeModeCatalogKind,
    /// Stable Code Mode identifier.
    pub id: String,
    /// Unqualified catalog entry name.
    pub name: String,
    /// Source/provider namespace. Reusable snippets use `snippet`.
    pub namespace: String,
    /// Human-readable catalog description.
    pub description: String,
    /// Advisory intrinsic safety facts from the live descriptor. Dispatch
    /// remains authoritative; absence means unknown, never `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety: Option<CodeModeToolSafety>,
    /// JSON Schema for the input payload when this capability has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    /// JSON Schema for the result payload when this capability has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// Compact invocation signature when the capability exposes one.
    pub signature: String,
    /// TypeScript declaration text for callable Tool descriptors.
    pub dts: String,
    /// Optional search/discovery tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Snippet-specific declared input entries; empty for other kinds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<CodeModeSnippetInputEntry>,
}

/// Compact, source-neutral safety facts for discovery presentation.
///
/// Optional booleans preserve fail-closed semantics: an omitted fact is
/// unknown. This type deliberately carries no approval/access policy or raw
/// upstream annotation text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum CodeModeCatalogKind {
    /// Host-provided callable tool.
    Tool,
    /// Reusable Code Mode snippet.
    Snippet,
    /// MCP or host-provided resource metadata.
    Resource,
    /// MCP or host-provided prompt metadata.
    Prompt,
    /// Agent Skill metadata.
    Skill,
    /// Command metadata.
    Command,
    /// Subagent profile metadata.
    #[serde(alias = "agent")]
    Subagent,
}

impl CodeModeCatalogKind {
    /// Stable wire/search spelling used by Code Mode catalog filters.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Snippet => "snippet",
            Self::Resource => "resource",
            Self::Prompt => "prompt",
            Self::Skill => "skill",
            Self::Command => "command",
            Self::Subagent => "subagent",
        }
    }

    /// Parse a catalog kind from its stable wire spelling.
    #[must_use]
    pub fn parse_filter(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "tool" => Some(Self::Tool),
            "snippet" => Some(Self::Snippet),
            "resource" => Some(Self::Resource),
            "prompt" => Some(Self::Prompt),
            "skill" => Some(Self::Skill),
            "command" => Some(Self::Command),
            "subagent" | "agent" => Some(Self::Subagent),
            _ => None,
        }
    }
}

/// Named snippet input plus its validation/default specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CodeModeSnippetInputEntry {
    /// Input parameter name.
    pub name: String,
    /// Input type, requirement, and optional default.
    #[serde(flatten)]
    pub spec: SnippetInputSpec,
}

impl CatalogDescriptor {
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
        let types = crate::ts_signatures::generate_tool_types(
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

    /// Build a metadata-only capability descriptor.
    ///
    /// Metadata descriptors participate in search/describe but deliberately
    /// carry no callable signature or TypeScript declaration. The gateway is
    /// responsible for enforcing authorization again when the capability is
    /// retrieved or invoked through its native surface.
    #[must_use]
    pub fn metadata(
        kind: CodeModeCatalogKind,
        namespace: &str,
        id: &str,
        name: &str,
        description: &str,
        tags: Vec<String>,
    ) -> Self {
        debug_assert!(!matches!(
            kind,
            CodeModeCatalogKind::Tool | CodeModeCatalogKind::Snippet
        ));
        Self {
            kind,
            tools: None,
            id: id.to_string(),
            name: labby_runtime::redact::sanitize_tool_text(name, 256),
            namespace: namespace.to_string(),
            description: labby_runtime::redact::sanitize_tool_text(description, 2048),
            safety: None,
            schema: None,
            output_schema: None,
            signature: String::new(),
            dts: String::new(),
            tags: tags
                .into_iter()
                .take(64)
                .enumerate()
                .map(|(index, tag)| {
                    // The gateway supplies the exact retrieval URI first on these surfaces.
                    if index == 0
                        && matches!(
                            kind,
                            CodeModeCatalogKind::Skill | CodeModeCatalogKind::Resource
                        )
                        && tag.starts_with("uri:")
                    {
                        tag
                    } else {
                        labby_runtime::redact::sanitize_tool_text(&tag, 512)
                    }
                })
                .collect(),
            inputs: Vec::new(),
        }
    }

    /// Stable source-neutral path used by search/describe. Existing tool and
    /// snippet paths are preserved byte-for-byte; metadata-only kinds live
    /// under a kind-prefixed namespace and gain no execution behavior.
    #[must_use]
    pub fn discovery_path(&self) -> String {
        match self.kind {
            CodeModeCatalogKind::Tool => {
                let namespace = crate::preamble::namespace_segment(&self.namespace);
                let name = crate::preamble::tool_name_to_snake(&self.name);
                format!("{namespace}.{name}")
            }
            CodeModeCatalogKind::Snippet => format!("snippet.{}", self.name),
            kind => {
                let kind = kind.as_str();
                let namespace = crate::preamble::namespace_segment(&self.namespace);
                let name = crate::preamble::tool_name_to_snake(&self.name);
                if namespace == kind {
                    format!("{kind}.{name}")
                } else {
                    format!("{kind}.{namespace}.{name}")
                }
            }
        }
    }

    /// Discovery helper text. Metadata entries remain non-callable, but kinds
    /// with explicit progressive-disclosure APIs point at the safe retrieval
    /// operation rather than dead-ending at `describe()`.
    #[must_use]
    pub fn discovery_helper(&self) -> String {
        let tagged_uri = || {
            self.tags
                .iter()
                .find_map(|tag| tag.strip_prefix("uri:"))
                .map(str::to_string)
        };
        match self.kind {
            CodeModeCatalogKind::Tool => format!("codemode.{}", self.discovery_path()),
            CodeModeCatalogKind::Snippet => format!("codemode.run({:?}, input)", self.name),
            CodeModeCatalogKind::Resource => tagged_uri().map_or_else(
                || format!("codemode.describe({:?})", self.discovery_path()),
                |uri| format!("codemode.readResource({uri:?})"),
            ),
            CodeModeCatalogKind::Prompt => format!("codemode.getPrompt({:?}, args)", self.id),
            CodeModeCatalogKind::Skill => tagged_uri().map_or_else(
                || format!("codemode.describe({:?})", self.discovery_path()),
                |uri| format!("codemode.getSkill({uri:?})"),
            ),
            CodeModeCatalogKind::Command | CodeModeCatalogKind::Subagent => {
                format!("codemode.describe({:?})", self.discovery_path())
            }
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
            field.insert(
                "type".to_string(),
                if spec.nullable {
                    json!([json_type, "null"])
                } else {
                    Value::String(json_type.to_string())
                },
            );
        } else if !spec.nullable {
            field.insert("not".to_string(), json!({"type":"null"}));
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
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
}

impl CodeModeDiscoveryEntry {
    #[must_use]
    pub(crate) fn from_catalog(entry: &CatalogDescriptor) -> Self {
        let path = entry.discovery_path();
        let helper = entry.discovery_helper();
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
        }
    }
}
