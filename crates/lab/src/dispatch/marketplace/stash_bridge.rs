use std::path::PathBuf;

use lab_apis::stash::StashComponentKind;
use serde::Serialize;

use crate::dispatch::error::ToolError;

pub(super) fn component_name_for_fork(plugin_id: &str, artifact_path: Option<&str>) -> String {
    let raw = match artifact_path {
        Some(path) => format!("{plugin_id}-{path}"),
        None => plugin_id.to_string(),
    };
    let mut out = String::with_capacity(raw.len());
    let mut last_was_dash = false;
    for ch in raw.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };
        if mapped == '-' {
            if !last_was_dash {
                out.push(mapped);
            }
            last_was_dash = true;
        } else {
            out.push(mapped);
            last_was_dash = false;
        }
    }
    out.trim_matches('-').chars().take(128).collect()
}

pub(super) fn kind_for_artifact_path(artifact_path: Option<&str>) -> StashComponentKind {
    let Some(path) = artifact_path else {
        return StashComponentKind::Plugin;
    };
    let first = path.split('/').next().unwrap_or(path);
    match first {
        "agents" => StashComponentKind::Agent,
        "commands" => StashComponentKind::Command,
        "hooks" => StashComponentKind::Hook,
        "monitors" => StashComponentKind::Monitor,
        "output-styles" | "output_styles" => StashComponentKind::OutputStyle,
        "themes" => StashComponentKind::Theme,
        "bin" => StashComponentKind::BinFile,
        "settings.json" => StashComponentKind::Settings,
        path if path.ends_with(".mcp.json") => StashComponentKind::McpConfig,
        path if path.ends_with(".lsp.json") => StashComponentKind::LspConfig,
        "skills" => StashComponentKind::Skill,
        _ => StashComponentKind::Skill,
    }
}

#[derive(Debug, Serialize)]
pub(super) struct ForkResult {
    pub plugin_id: String,
    pub component_id: String,
    pub revision_id: String,
    pub stash_workspace: String,
    pub forked_artifacts: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ForkResponse {
    pub forks: Vec<ForkResult>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ForkedPluginStatus {
    pub plugin_id: String,
    pub component_id: String,
    pub stash_workspace: String,
    pub forked_artifacts: Vec<String>,
    pub status: String,
}

pub(super) fn fork_source_path(
    plugin_id: &str,
    artifact_path: Option<&str>,
) -> Result<PathBuf, ToolError> {
    let (_marketplace_root, source) =
        crate::dispatch::marketplace::update::source_paths_for_bridge(plugin_id)?;
    match artifact_path {
        Some(path) => {
            crate::dispatch::marketplace::stash_meta::validate_rel_path(path)?;
            let candidate = source.join(path);
            if !candidate.exists() {
                return Err(ToolError::Sdk {
                    sdk_kind: "not_found".into(),
                    message: format!("artifact `{path}` not found in `{plugin_id}`"),
                });
            }
            Ok(candidate)
        }
        None => Ok(source),
    }
}

pub(super) fn fork_state_dir(component_id: &str) -> Result<PathBuf, ToolError> {
    let root = crate::dispatch::stash::client::require_stash_root()?.clone();
    Ok(root.join("marketplace").join(component_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stash_component_name_sanitizes_plugin_and_artifact() {
        assert_eq!(
            component_name_for_fork("demo@labby", Some("skills/demo/SKILL.md")),
            "demo-labby-skills-demo-skill-md"
        );
    }

    #[test]
    fn kind_for_artifact_path_maps_plugin_layout_to_stash_kind() {
        assert_eq!(
            kind_for_artifact_path(Some("skills/demo")),
            StashComponentKind::Skill
        );
        assert_eq!(
            kind_for_artifact_path(Some("agents/demo.md")),
            StashComponentKind::Agent
        );
        assert_eq!(
            kind_for_artifact_path(Some("commands/demo.md")),
            StashComponentKind::Command
        );
        assert_eq!(
            kind_for_artifact_path(Some("settings.json")),
            StashComponentKind::Settings
        );
        assert_eq!(kind_for_artifact_path(None), StashComponentKind::Plugin);
    }
}
