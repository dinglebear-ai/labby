//! Presence-aware, bounded Skill dependencies for saved snippets.
//!
//! Skill policy contributes instructions and discovery metadata only. It never
//! grants upstream tool authority; saved snippet execution continues to derive
//! its tool scope exclusively from the caller scope intersected with tools.

use serde::{Deserialize, Serialize};

use super::tool_declarations::SnippetToolDeclarations;
use crate::error::ToolError;

pub const MAX_SKILL_QUERIES: usize = 16;
pub const MAX_PINNED_SKILLS: usize = 16;
pub const MAX_SKILL_CANDIDATES: usize = 64;
pub const MAX_SELECTED_SKILLS: usize = 16;
pub const MAX_SKILL_QUERY_BYTES: usize = 1_024;
pub const MAX_SKILL_URI_BYTES: usize = 4_096;
pub const MAX_SKILL_CONTENT_BYTES: usize = 64 * 1024;
pub const DEFAULT_SKILL_CONTENT_BYTES: usize = 16 * 1024;

const SKILLS_SH_TOOL_SUFFIX: &str = "::depot.skills.search_skills_sh";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnippetSkillResolution {
    Dynamic,
    Pinned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnippetSkillSource {
    Team,
    Public,
    Mounted,
    SkillsSh,
}

impl SnippetSkillSource {
    #[must_use]
    pub const fn trust(self) -> SnippetSkillTrust {
        match self {
            Self::Team => SnippetSkillTrust::Authoritative,
            Self::Public | Self::Mounted => SnippetSkillTrust::Advisory,
            Self::SkillsSh => SnippetSkillTrust::Candidate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnippetSkillTrust {
    Authoritative,
    Advisory,
    Candidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetSkillPolicy {
    pub resolution: SnippetSkillResolution,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queries: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<String>,
    pub sources: Vec<SnippetSkillSource>,
    pub max_candidates: usize,
    pub max_selected: usize,
    pub max_bytes_per_skill: usize,
}

impl SnippetSkillPolicy {
    #[must_use]
    pub fn uses_skills_sh(&self) -> bool {
        self.sources.contains(&SnippetSkillSource::SkillsSh)
    }

    #[must_use]
    pub fn skills_sh_tool<'a>(
        &self,
        tools: Option<&'a SnippetToolDeclarations>,
    ) -> Option<&'a str> {
        if !self.uses_skills_sh() {
            return None;
        }
        tools?
            .as_slice()
            .iter()
            .find(|id| id.ends_with(SKILLS_SH_TOOL_SUFFIX))
            .map(String::as_str)
    }

    pub fn validate_tool_authority(
        &self,
        tools: Option<&SnippetToolDeclarations>,
    ) -> Result<(), ToolError> {
        if self.uses_skills_sh() && self.skills_sh_tool(tools).is_none() {
            return Err(invalid(
                "skills source 'skills_sh' requires an explicit tools declaration for an exact *::depot.skills.search_skills_sh tool; skill policy never grants tool authority",
            ));
        }
        Ok(())
    }
}

pub(super) fn parse(
    lines: &[&str],
    start: usize,
) -> Result<(SnippetSkillPolicy, usize), ToolError> {
    let mut resolution = None;
    let mut queries = Vec::new();
    let mut pinned = Vec::new();
    let mut sources = None;
    let mut max_candidates = 20usize;
    let mut max_selected = 5usize;
    let mut max_bytes_per_skill = DEFAULT_SKILL_CONTENT_BYTES;
    let mut next = start;

    while let Some(raw) = lines.get(next) {
        if raw.trim().is_empty() || raw.trim().starts_with('#') {
            next += 1;
            continue;
        }
        if !raw.starts_with("  ") {
            break;
        }
        if raw.starts_with("    ") {
            return Err(invalid(
                "frontmatter skills accepts scalar fields and inline JSON arrays only",
            ));
        }
        let line = raw.trim();
        let Some((key, value)) = line.split_once(':') else {
            return Err(invalid("invalid frontmatter skills entry"));
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "resolution" => {
                resolution = Some(match value.trim_matches('"') {
                    "dynamic" => SnippetSkillResolution::Dynamic,
                    "pinned" => SnippetSkillResolution::Pinned,
                    _ => {
                        return Err(invalid(
                            "frontmatter skills.resolution must be dynamic or pinned",
                        ));
                    }
                });
            }
            "queries" => queries = parse_string_array(value, "skills.queries")?,
            "pinned" => pinned = parse_string_array(value, "skills.pinned")?,
            "sources" => {
                let values = parse_string_array(value, "skills.sources")?;
                let mut parsed = Vec::with_capacity(values.len());
                for value in values {
                    let source = match value.as_str() {
                        "team" => SnippetSkillSource::Team,
                        "public" => SnippetSkillSource::Public,
                        "mounted" => SnippetSkillSource::Mounted,
                        "skills_sh" => SnippetSkillSource::SkillsSh,
                        _ => {
                            return Err(invalid(
                                "frontmatter skills.sources entries must be team, public, mounted, or skills_sh",
                            ));
                        }
                    };
                    if !parsed.contains(&source) {
                        parsed.push(source);
                    }
                }
                sources = Some(parsed);
            }
            "max_candidates" => {
                max_candidates = parse_usize(value, "skills.max_candidates")?;
            }
            "max_selected" => {
                max_selected = parse_usize(value, "skills.max_selected")?;
            }
            "max_bytes_per_skill" => {
                max_bytes_per_skill = parse_usize(value, "skills.max_bytes_per_skill")?;
            }
            _ => {
                return Err(invalid(&format!("unknown frontmatter skills.{key} field")));
            }
        }
        next += 1;
    }

    let resolution = resolution.ok_or_else(|| {
        invalid("frontmatter skills requires resolution: dynamic or resolution: pinned")
    })?;
    let sources = sources.unwrap_or_else(|| {
        vec![
            SnippetSkillSource::Team,
            SnippetSkillSource::Public,
            SnippetSkillSource::Mounted,
        ]
    });
    let policy = SnippetSkillPolicy {
        resolution,
        queries,
        pinned,
        sources,
        max_candidates,
        max_selected,
        max_bytes_per_skill,
    };
    validate(&policy)?;
    Ok((policy, next))
}

fn validate(policy: &SnippetSkillPolicy) -> Result<(), ToolError> {
    if policy.sources.is_empty() {
        return Err(invalid("frontmatter skills.sources must not be empty"));
    }
    if policy.max_candidates == 0 || policy.max_candidates > MAX_SKILL_CANDIDATES {
        return Err(invalid(&format!(
            "frontmatter skills.max_candidates must be between 1 and {MAX_SKILL_CANDIDATES}"
        )));
    }
    if policy.max_selected == 0
        || policy.max_selected > MAX_SELECTED_SKILLS
        || policy.max_selected > policy.max_candidates
    {
        return Err(invalid(&format!(
            "frontmatter skills.max_selected must be between 1 and min(max_candidates, {MAX_SELECTED_SKILLS})"
        )));
    }
    if policy.max_bytes_per_skill == 0 || policy.max_bytes_per_skill > MAX_SKILL_CONTENT_BYTES {
        return Err(invalid(&format!(
            "frontmatter skills.max_bytes_per_skill must be between 1 and {MAX_SKILL_CONTENT_BYTES}"
        )));
    }
    if policy.queries.len() > MAX_SKILL_QUERIES {
        return Err(invalid("snippet declares too many dynamic Skill queries"));
    }
    for query in &policy.queries {
        if query.trim().is_empty()
            || query.len() > MAX_SKILL_QUERY_BYTES
            || query.chars().any(char::is_control)
        {
            return Err(invalid(
                "frontmatter Skill queries must be non-empty, bounded, and contain no control characters",
            ));
        }
    }
    if policy.pinned.len() > MAX_PINNED_SKILLS {
        return Err(invalid("snippet declares too many pinned Skills"));
    }
    for uri in &policy.pinned {
        if !uri.starts_with("skill://")
            || uri.len() > MAX_SKILL_URI_BYTES
            || uri
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(invalid(
                "frontmatter pinned Skills must be bounded skill:// URIs without whitespace/control characters",
            ));
        }
    }
    match policy.resolution {
        SnippetSkillResolution::Dynamic if policy.queries.is_empty() => {
            return Err(invalid(
                "dynamic Skill resolution requires at least one skills.queries entry",
            ));
        }
        SnippetSkillResolution::Dynamic if !policy.pinned.is_empty() => {
            return Err(invalid(
                "dynamic Skill resolution must not declare skills.pinned; use resolution: pinned",
            ));
        }
        SnippetSkillResolution::Pinned if policy.pinned.is_empty() => {
            return Err(invalid(
                "pinned Skill resolution requires at least one skills.pinned URI",
            ));
        }
        SnippetSkillResolution::Pinned if !policy.queries.is_empty() => {
            return Err(invalid(
                "pinned Skill resolution must not declare skills.queries",
            ));
        }
        _ => {}
    }
    Ok(())
}

fn parse_string_array(value: &str, field: &str) -> Result<Vec<String>, ToolError> {
    serde_json::from_str::<Vec<String>>(value).map_err(|_| {
        invalid(&format!(
            "frontmatter {field} must be an inline JSON string array"
        ))
    })
}

fn parse_usize(value: &str, field: &str) -> Result<usize, ToolError> {
    value
        .parse::<usize>()
        .map_err(|_| invalid(&format!("frontmatter {field} must be an integer")))
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
    fn parses_dynamic_policy_and_applies_safe_defaults() {
        let lines = [
            "  resolution: dynamic",
            r#"  queries: ["{{task}}", "rust security"]"#,
            r#"  sources: ["team", "public", "skills_sh"]"#,
            "  max_candidates: 12",
            "  max_selected: 4",
            "next: value",
        ];
        let (policy, next) = parse(&lines, 0).expect("policy parses");
        assert_eq!(next, 5);
        assert_eq!(policy.resolution, SnippetSkillResolution::Dynamic);
        assert_eq!(policy.queries.len(), 2);
        assert_eq!(policy.max_candidates, 12);
        assert_eq!(policy.max_selected, 4);
        assert_eq!(policy.max_bytes_per_skill, DEFAULT_SKILL_CONTENT_BYTES);
        assert_eq!(
            policy.sources,
            vec![
                SnippetSkillSource::Team,
                SnippetSkillSource::Public,
                SnippetSkillSource::SkillsSh
            ]
        );
    }

    #[test]
    fn skills_sh_requires_predeclared_tool_authority() {
        let policy = SnippetSkillPolicy {
            resolution: SnippetSkillResolution::Dynamic,
            queries: vec!["mcp security".into()],
            pinned: vec![],
            sources: vec![SnippetSkillSource::SkillsSh],
            max_candidates: 8,
            max_selected: 3,
            max_bytes_per_skill: DEFAULT_SKILL_CONTENT_BYTES,
        };
        assert!(policy.validate_tool_authority(None).is_err());

        let tools = SnippetToolDeclarations::try_from(vec![
            "catalog-depot::depot.skills.search_skills_sh".into(),
        ])
        .expect("valid tool");
        assert!(policy.validate_tool_authority(Some(&tools)).is_ok());
    }

    #[test]
    fn pinned_policy_rejects_queries_and_invalid_uris() {
        let lines = [
            "  resolution: pinned",
            r#"  pinned: ["https://example.com/SKILL.md"]"#,
        ];
        assert!(parse(&lines, 0).is_err());

        let lines = [
            "  resolution: pinned",
            r#"  pinned: ["skill://team-depot/skill/demo/SKILL.md"]"#,
            r#"  queries: ["should not exist"]"#,
        ];
        assert!(parse(&lines, 0).is_err());
    }
}
