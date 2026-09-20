//! Host-injected canonical Agent Skills access for Code Mode.
//!
//! The gateway owns Code Mode execution but not Labby's canonical Skill Library.
//! The product host injects this seam so discovery and explicit Skill reads can
//! reuse the same caller-authorized registry as native Skills surfaces without
//! creating a second registry in `labby-gateway`.

use std::future::Future;
use std::pin::Pin;

use labby_codemode::{CodeModeCaller, ToolScope};
use labby_runtime::error::ToolError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Compact metadata projected into Code Mode's source-neutral capability catalog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeModeSkillSummary {
    /// Published Skill root URI. This is the stable identity used by get/read.
    pub uri: String,
    /// Human-readable Skill name.
    pub name: String,
    /// Optional concise description from Skill frontmatter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Search terms extracted from canonical metadata.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Product-host seam for canonical, caller-visible Agent Skills.
///
/// Implementations must fail closed when request-bound authorization context is
/// unavailable. Catalog membership is discovery metadata only; get/read repeat
/// authorization through the canonical Skills registry.
pub trait CodeModeSkillProvider: Send + Sync {
    fn list<'a>(
        &'a self,
        caller: &'a CodeModeCaller,
        scope: &'a ToolScope,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<CodeModeSkillSummary>, ToolError>> + Send + 'a>>;

    fn get<'a>(
        &'a self,
        uri: &'a str,
        caller: &'a CodeModeCaller,
        scope: &'a ToolScope,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send + 'a>>;

    fn read<'a>(
        &'a self,
        uri: &'a str,
        caller: &'a CodeModeCaller,
        scope: &'a ToolScope,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send + 'a>>;
}
