use super::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Origin of a reusable Code Mode snippet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnippetSource {
    /// Snippet shipped with Labby.
    Builtin,
    /// Operator-created snippet stored under the Labby home directory.
    User,
}

/// Discovery metadata for a built-in or user Code Mode snippet.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnippetInfo {
    /// SHA-256 of the exact stored source bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
    /// Optional exact-tool declaration used to scope native saved-snippet execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<SnippetToolDeclarations>,
    /// Stable snippet name.
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Search/discovery tags.
    pub tags: Vec<String>,
    /// Named input specifications.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, SnippetInputSpec>,
    /// Snippet origin.
    pub source: SnippetSource,
    /// Source file path.
    pub path: PathBuf,
    /// Whether this entry is shadowed by a user snippet with the same name.
    pub shadowed: bool,
}

/// Fully resolved snippet including its source body.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedSnippet {
    /// SHA-256 of the exact stored source bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
    /// Optional declaration; an empty list expresses deny-all upstream access.
    /// Host saved-snippet execution intersects this with the caller policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<SnippetToolDeclarations>,
    /// Stable snippet name.
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Search/discovery tags.
    pub tags: Vec<String>,
    /// Named input specifications.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, SnippetInputSpec>,
    /// Snippet origin.
    pub source: SnippetSource,
    /// Source file path.
    pub path: PathBuf,
    /// Complete snippet file contents.
    pub body: String,
}

/// Parsed YAML frontmatter from a Markdown snippet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetFrontmatter {
    /// Optional exact-tool declaration, distinct from omitted metadata.
    pub tools: Option<SnippetToolDeclarations>,
    /// Declared snippet name.
    pub name: String,
    /// Declared human-readable description.
    pub description: String,
    /// Declared discovery tags.
    pub tags: Vec<String>,
    /// Declared named inputs.
    pub inputs: BTreeMap<String, SnippetInputSpec>,
}

/// Validation/default specification for one snippet input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SnippetInputSpec {
    /// Expected input value type.
    pub ty: SnippetInputType,
    /// Whether callers must provide the input when no default exists.
    #[serde(default)]
    pub required: bool,
    /// Whether an explicitly supplied JSON null is accepted.
    #[serde(default)]
    pub nullable: bool,
    /// Optional JSON default value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// Optional human-readable input description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Supported validation types for declared snippet inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnippetInputType {
    /// JSON string.
    String,
    /// Integer-valued JSON number.
    Integer,
    /// Any JSON number.
    Number,
    /// JSON boolean.
    Boolean,
    /// JSON object.
    Object,
    /// JSON array.
    Array,
    /// Any JSON value.
    Json,
}

/// Result returned after removing a user snippet.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnippetRemoveResult {
    /// Snippet name requested for removal.
    pub name: String,
    /// Whether a user snippet file was removed.
    pub removed: bool,
}

/// A skipped snippet file and the reason it could not be listed.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnippetDiagnostic {
    /// Filename-derived identifier.
    pub name: String,
    /// Source path.
    pub path: PathBuf,
    /// Source authority.
    pub source: SnippetSource,
    /// Validation or read failure.
    pub message: String,
}

/// Complete discovery result, including invalid files.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnippetList {
    /// Valid discovery entries.
    pub snippets: Vec<SnippetInfo>,
    /// Files omitted due to validation or read errors.
    pub diagnostics: Vec<SnippetDiagnostic>,
    /// Number of per-file diagnostics omitted after the 128-entry response cap.
    pub diagnostics_omitted: usize,
}
