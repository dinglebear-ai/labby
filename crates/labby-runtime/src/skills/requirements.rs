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

fn split_allowed_tools_string(tools: &str) -> Vec<String> {
    let mut hints = Vec::new();
    let mut start = 0;
    let mut parenthesis_depth = 0_u32;

    let push = |hints: &mut Vec<String>, value: &str| {
        let value = value.trim();
        if !value.is_empty() {
            hints.push(value.to_owned());
        }
    };

    for (index, ch) in tools.char_indices() {
        match ch {
            '(' => parenthesis_depth = parenthesis_depth.saturating_add(1),
            ')' => parenthesis_depth = parenthesis_depth.saturating_sub(1),
            ',' if parenthesis_depth == 0 => {
                push(&mut hints, &tools[start..index]);
                start = index + ch.len_utf8();
            }
            ch if ch.is_ascii_whitespace() && parenthesis_depth == 0 => {
                push(&mut hints, &tools[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    push(&mut hints, &tools[start..]);
    hints
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
            Some(Value::String(tools)) => split_allowed_tools_string(tools),
            Some(Value::Array(tools)) => tools
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|tool| !tool.is_empty())
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
    fn projects_compatibility_allowed_tools_forms_without_granting_authority() {
        let comma = SkillRequirementsSummary::from_frontmatter(&object(json!({
            "name": "review",
            "description": "Review a change",
            "allowed-tools": "Read, Bash(git status *), Grep"
        })));
        assert_eq!(comma.tool_hints, ["Read", "Bash(git status *)", "Grep"]);

        let array = SkillRequirementsSummary::from_frontmatter(&object(json!({
            "name": "review",
            "description": "Review a change",
            "allowed-tools": ["Read", "Bash(git status *)", "Read"]
        })));
        assert_eq!(array.tool_hints, ["Read", "Bash(git status *)", "Read"]);

        for summary in [comma, array] {
            let value = serde_json::to_value(summary).expect("requirements JSON");
            assert!(value.get("authorized").is_none());
            assert!(value.get("allowed_tools").is_none());
        }
    }

    #[test]
    fn whitespace_form_keeps_parenthesized_patterns_intact() {
        let summary = SkillRequirementsSummary::from_frontmatter(&object(json!({
            "name": "review",
            "description": "Review a change",
            "allowed-tools": "Bash(git status *) Read"
        })));

        assert_eq!(summary.tool_hints, ["Bash(git status *)", "Read"]);
    }
}
