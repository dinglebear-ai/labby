//! Provider-neutral requirements declared by an Agent Skill.
//!
//! Requirements describe source-authored activation context. They are not an
//! authorization grant: in particular, Agent Skills `allowed-tools` values are
//! retained as tool hints while Labby's normal authorization and destructive
//! action policy remain authoritative for every execution.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Compact source-authored requirements used during discovery and activation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRequirementsSummary {
    /// Opaque Agent Skills `compatibility` statement.
    ///
    /// The specification defines this as human-readable environment context,
    /// not a machine-readable dependency expression. Adapters must not infer
    /// dependencies or availability from its contents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    /// Tokens from the experimental Agent Skills `allowed-tools` field.
    ///
    /// These are compatibility hints in their source order. They never grant
    /// access to a Labby tool, shell, network, filesystem, secret, or side
    /// effect, and must be resolved only within the source provider's context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_hints: Vec<String>,
}

impl SkillRequirementsSummary {
    /// Project the requirement-bearing Agent Skills frontmatter fields.
    ///
    /// Callers pass frontmatter that has already crossed the format validation
    /// boundary. Unknown fields, license, and arbitrary metadata are not
    /// requirements and remain preserved by the validated source entry.
    #[must_use]
    pub fn from_frontmatter(frontmatter: &Map<String, Value>) -> Self {
        let compatibility = frontmatter
            .get("compatibility")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let tool_hints = match frontmatter.get("allowed-tools") {
            Some(Value::String(tools)) => tools
                .split_ascii_whitespace()
                .map(ToOwned::to_owned)
                .collect(),
            Some(Value::Array(tools)) => tools
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect(),
            _ => Vec::new(),
        };

        Self {
            compatibility,
            tool_hints,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.compatibility.is_none() && self.tool_hints.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().expect("object").clone()
    }

    #[test]
    fn projects_only_concrete_agent_skills_requirements() {
        let summary = SkillRequirementsSummary::from_frontmatter(&object(json!({
            "name": "review",
            "description": "Review a change",
            "compatibility": "Requires git",
            "allowed-tools": "Read Grep Read",
            "license": "Apache-2.0",
            "metadata": {"vendor.example/channel": "stable"},
            "future-field": "preserved elsewhere"
        })));

        assert_eq!(summary.compatibility.as_deref(), Some("Requires git"));
        assert_eq!(summary.tool_hints, ["Read", "Grep", "Read"]);
        assert_eq!(
            serde_json::to_value(summary).expect("requirements JSON"),
            json!({
                "compatibility": "Requires git",
                "tool_hints": ["Read", "Grep", "Read"]
            })
        );
    }

    #[test]
    fn empty_or_whitespace_tool_hint_grants_nothing() {
        let empty = SkillRequirementsSummary::from_frontmatter(&object(json!({
            "name": "review",
            "description": "Review a change",
            "allowed-tools": "  \t "
        })));

        assert!(empty.is_empty());
        let value = serde_json::to_value(empty).expect("requirements JSON");
        assert_eq!(value, json!({}));
        assert!(value.get("authorized").is_none());
        assert!(value.get("allowed_tools").is_none());
    }

    #[test]
    fn projects_claude_compatible_list_tool_hints_in_source_order() {
        let summary = SkillRequirementsSummary::from_frontmatter(&object(json!({
            "name": "review",
            "description": "Review a change",
            "allowed-tools": ["Read", "Grep", "Read"]
        })));

        assert_eq!(summary.tool_hints, ["Read", "Grep", "Read"]);
    }
}
