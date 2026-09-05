//! Presence-aware, bounded exact-tool declarations for saved snippets.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ToolScope;
use crate::error::ToolError;
use crate::types::split_namespaced_id;

/// Maximum declared dependencies in one snippet.
pub const MAX_DECLARED_TOOLS: usize = 128;
/// Bound one exact tool identifier independently of the source-file limit.
pub const MAX_DECLARED_TOOL_ID_BYTES: usize = 1_024;

/// A validated exact-tool allowlist. `Some(empty)` denies all upstream tools;
/// omission is represented by `None` and inherits the caller's existing policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<String>")]
pub struct SnippetToolDeclarations(Vec<String>);

impl TryFrom<Vec<String>> for SnippetToolDeclarations {
    type Error = ToolError;

    fn try_from(ids: Vec<String>) -> Result<Self, Self::Error> {
        if ids.len() > MAX_DECLARED_TOOLS {
            return Err(invalid("snippet declares too many tools"));
        }
        let mut seen = BTreeSet::new();
        for id in &ids {
            let Some((namespace, _)) = split_namespaced_id(id) else {
                return Err(invalid(
                    "snippet tools must use exact <upstream>::<tool> identifiers",
                ));
            };
            if id.len() > MAX_DECLARED_TOOL_ID_BYTES
                || id
                    .chars()
                    .any(|character| character.is_whitespace() || character.is_control())
            {
                return Err(invalid(
                    "snippet tool identifier is oversized or contains whitespace/control characters",
                ));
            }
            if matches!(
                namespace,
                "lab" | "snippet" | "__lab_internal" | "state" | "git" | "openapi"
            ) {
                return Err(invalid(
                    "snippet tools must name upstream tools, not reserved local capabilities",
                ));
            }
            if !seen.insert(id) {
                return Err(invalid(
                    "snippet tool declarations must not contain duplicate identifiers",
                ));
            }
        }
        Ok(Self(ids))
    }
}

impl SnippetToolDeclarations {
    /// The exact identifiers in their declared order.
    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    /// Intersect the declaration with existing caller authority. A declaration
    /// is never a grant and cannot remove the caller's read-only restriction.
    #[must_use]
    pub fn intersect(&self, parent: &ToolScope) -> ToolScope {
        let mut namespaces = Vec::new();
        let mut tools = Vec::new();
        for id in &self.0 {
            if let Some((namespace, tool)) = split_namespaced_id(id)
                && parent.allows(namespace, tool)
            {
                namespaces.push(namespace.to_string());
                tools.push(id.clone());
            }
        }
        let scope = ToolScope::scoped_namespaces(namespaces, tools);
        if parent.is_read_only() {
            scope.read_only()
        } else {
            scope
        }
    }
}

pub(super) fn parse(
    lines: &[&str],
    start: usize,
    value: &str,
) -> Result<(SnippetToolDeclarations, usize), ToolError> {
    if !value.is_empty() {
        let ids = serde_json::from_str::<Vec<String>>(value).map_err(|_| {
            invalid("frontmatter tools must be a block list or a JSON string array")
        })?;
        return Ok((ids.try_into()?, start));
    }
    let mut ids = Vec::new();
    let mut next = start;
    while let Some(line) = lines.get(next) {
        if line.trim().is_empty() || line.trim().starts_with('#') {
            next += 1;
            continue;
        }
        if !line.starts_with("  ") {
            break;
        }
        let value = line
            .trim()
            .strip_prefix("- ")
            .ok_or_else(|| invalid("frontmatter tools must contain string list entries"))?
            .trim();
        let id = if value.starts_with('"') {
            serde_json::from_str::<String>(value)
                .map_err(|_| invalid("frontmatter tool contains an invalid quoted string"))?
        } else if value.starts_with('\'') {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
                .ok_or_else(|| invalid("frontmatter tool contains an invalid quoted string"))?
                .replace("''", "'")
        } else {
            value.to_string()
        };
        ids.push(id);
        if ids.len() > MAX_DECLARED_TOOLS {
            return Err(invalid("snippet declares too many tools"));
        }
        next += 1;
    }
    Ok((ids.try_into()?, next))
}

pub(super) fn invalid(message: &str) -> ToolError {
    ToolError::InvalidParam {
        message: message.to_string(),
        param: "body".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_never_widens_namespaces_tools_or_read_only_access() {
        let declarations = SnippetToolDeclarations::try_from(vec![
            "alpha::read".into(),
            "alpha::write".into(),
            "beta::read".into(),
        ])
        .unwrap();
        let parent =
            ToolScope::scoped_namespaces(vec!["alpha".into()], vec!["read".into()]).read_only();
        let child = declarations.intersect(&parent);
        assert!(child.allows("alpha", "read"));
        assert!(!child.allows("alpha", "write"));
        assert!(!child.allows("beta", "read"));
        assert!(child.is_read_only());
        assert_eq!(declarations.intersect(&child), child);
    }

    #[test]
    fn an_empty_or_disjoint_declaration_is_explicitly_scoped_and_denies_every_tool() {
        for ids in [vec![], vec!["beta::read".into()]] {
            let declaration = SnippetToolDeclarations::try_from(ids).unwrap();
            let parent = ToolScope::scoped_namespaces(vec!["alpha".into()], vec![]);
            let scope = declaration.intersect(&parent);
            assert!(scope.is_scoped());
            assert!(!scope.allows("alpha", "read"));
            assert!(!scope.allows("beta", "read"));
        }
    }

    #[test]
    fn validation_is_bounded_and_cannot_admit_reserved_capabilities() {
        for id in [
            "lab::fs",
            "state::get",
            "git::status",
            "openapi::call",
            "__lab_internal::describe_types",
            "snippet::other",
            "alpha ::read",
            "alpha::\nread",
        ] {
            assert!(SnippetToolDeclarations::try_from(vec![id.into()]).is_err());
        }
        assert!(
            SnippetToolDeclarations::try_from(vec!["alpha::read".into(); MAX_DECLARED_TOOLS + 1])
                .is_err()
        );
        assert!(
            SnippetToolDeclarations::try_from(vec![format!(
                "alpha::{}",
                "x".repeat(MAX_DECLARED_TOOL_ID_BYTES)
            )])
            .is_err()
        );
    }

    #[test]
    fn serde_keeps_explicit_empty_and_rejects_invalid_identifiers() {
        let empty: SnippetToolDeclarations = serde_json::from_str("[]").unwrap();
        assert_eq!(serde_json::to_string(&empty).unwrap(), "[]");
        assert!(serde_json::from_str::<SnippetToolDeclarations>("[\"bare\"]").is_err());
    }
}
