//! Canonical conversion between bounded Agent definitions and inert Artifacts.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{
    ArtifactComponent, ArtifactDescriptor, ArtifactInterchange, ArtifactLicenseState,
    ArtifactLineage, ArtifactProvenance, ArtifactPublication, ArtifactRevision, Distribution,
    JsonMap, PublicationState, Visibility,
};
use super::{ArtifactError, invalid};

pub const MAX_AGENT_BYTES: usize = 256 * 1024;
const MAX_AGENT_NAME_BYTES: usize = 128;
const MAX_AGENT_DESCRIPTION_BYTES: usize = 4_096;
const MAX_AGENT_CAPABILITIES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalAgentFile {
    pub path: String,
    pub content: String,
}

impl LogicalAgentFile {
    #[must_use]
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedAgent {
    pub files: BTreeMap<String, Vec<u8>>,
    pub interchange: ArtifactInterchange,
    preview: String,
}

impl MaterializedAgent {
    #[must_use]
    pub fn preview_text(&self) -> &str {
        &self.preview
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct AgentCapability {
    server_id: String,
    family: AgentCapabilityFamily,
    member_id: String,
    expected_revision: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum AgentCapabilityFamily {
    Tool,
    Prompt,
    Resource,
    Skill,
    McpApp,
    McpServer,
    Plugin,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentFrontmatter {
    name: String,
    description: String,
    runtime: String,
    activation: String,
    #[serde(default)]
    capabilities: Vec<AgentCapability>,
}

pub fn materialize_logical_agent(
    name: &str,
    files: Vec<LogicalAgentFile>,
    mut provenance: ArtifactProvenance,
) -> Result<MaterializedAgent, ArtifactError> {
    validate_name(name)?;
    if files.len() != 1 {
        return Err(invalid("files", "agent_requires_single_file"));
    }
    let file = files.into_iter().next().expect("length checked");
    if file.path != "AGENT.md" {
        return Err(invalid("files", "agent_file_name"));
    }
    if file.content.len() > MAX_AGENT_BYTES {
        return Err(ArtifactError::LimitExceeded {
            what: "agent_bytes",
            limit: MAX_AGENT_BYTES as u64,
        });
    }
    if file.content.contains('\0') {
        return Err(invalid("content", "nul_content"));
    }
    let (yaml, body) = split_frontmatter(&file.content)?;
    let frontmatter: AgentFrontmatter =
        serde_yaml::from_str(yaml).map_err(|_| invalid("frontmatter", "invalid_yaml"))?;
    if frontmatter.name != name {
        return Err(invalid("name", "frontmatter_mismatch"));
    }
    validate_name(&frontmatter.name)?;
    if frontmatter.description.trim().is_empty()
        || frontmatter.description.len() > MAX_AGENT_DESCRIPTION_BYTES
        || frontmatter.description.contains('\0')
    {
        return Err(invalid("description", "invalid"));
    }
    if frontmatter.runtime != "labby" {
        return Err(invalid("runtime", "unsupported"));
    }
    if frontmatter.activation != "explicit" {
        return Err(invalid("activation", "explicit_required"));
    }
    if body.trim().is_empty() {
        return Err(invalid("content", "empty"));
    }
    if frontmatter.capabilities.len() > MAX_AGENT_CAPABILITIES {
        return Err(ArtifactError::LimitExceeded {
            what: "agent_capabilities",
            limit: MAX_AGENT_CAPABILITIES as u64,
        });
    }
    let mut unique = BTreeSet::new();
    for capability in &frontmatter.capabilities {
        validate_reference(&capability.server_id, "capabilities")?;
        validate_reference(&capability.member_id, "capabilities")?;
        validate_revision(&capability.expected_revision)?;
        let encoded =
            serde_json::to_string(capability).map_err(|_| invalid("capabilities", "invalid"))?;
        if !unique.insert(encoded) {
            return Err(invalid("capabilities", "duplicate"));
        }
    }

    let capabilities = serde_json::to_value(&frontmatter.capabilities)
        .map_err(|_| invalid("capabilities", "invalid"))?;
    let preview = body.to_owned();
    let bytes = file.content.into_bytes();
    let revision = ArtifactRevision::from_components(
        vec![ArtifactComponent::from_bytes("AGENT.md", &bytes, None)?],
        None,
        None,
        None,
        JsonMap::new(),
    )?;
    let mut descriptor = ArtifactDescriptor::for_identity("agent", "labby", name)?;
    descriptor.description = Some(frontmatter.description);
    descriptor
        .metadata
        .insert("runtime".to_owned(), Value::String(frontmatter.runtime));
    descriptor.metadata.insert(
        "activation".to_owned(),
        Value::String(frontmatter.activation),
    );
    descriptor
        .metadata
        .insert("capabilities".to_owned(), capabilities);
    provenance
        .adapter
        .get_or_insert_with(|| "labby.agent/v1".to_owned());
    provenance
        .original_format
        .get_or_insert_with(|| "agent-markdown".to_owned());
    let interchange = ArtifactInterchange {
        schema_version: super::model::ARTIFACT_INTERCHANGE_SCHEMA.to_owned(),
        descriptor,
        revision,
        provenance,
        license: ArtifactLicenseState::default(),
        lineage: ArtifactLineage::default(),
        publication: ArtifactPublication {
            state: PublicationState::Listed,
            visibility: Visibility::Private,
            distribution: Distribution::Metadata,
            ..ArtifactPublication::default()
        },
        downloads: Vec::new(),
        materialization_hints: JsonMap::new(),
    };
    interchange.validate()?;
    Ok(MaterializedAgent {
        files: BTreeMap::from([("AGENT.md".to_owned(), bytes)]),
        interchange,
        preview,
    })
}

fn split_frontmatter(content: &str) -> Result<(&str, &str), ArtifactError> {
    let rest = content
        .strip_prefix("---\n")
        .ok_or_else(|| invalid("frontmatter", "missing"))?;
    rest.split_once("\n---\n")
        .ok_or_else(|| invalid("frontmatter", "unterminated"))
}

fn validate_name(value: &str) -> Result<(), ArtifactError> {
    if value.is_empty()
        || value.len() > MAX_AGENT_NAME_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
    {
        return Err(invalid("name", "invalid"));
    }
    Ok(())
}

fn validate_reference(value: &str, field: &'static str) -> Result<(), ArtifactError> {
    if value.is_empty()
        || value.len() > 256
        || value.contains('\0')
        || value.chars().any(char::is_whitespace)
    {
        return Err(invalid(field, "invalid_reference"));
    }
    Ok(())
}

fn validate_revision(value: &str) -> Result<(), ArtifactError> {
    if value.is_empty()
        || value.len() > 256
        || value.contains('\0')
        || value.chars().any(char::is_whitespace)
    {
        return Err(invalid("capabilities", "invalid_revision"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(body: &str) -> Vec<LogicalAgentFile> {
        vec![LogicalAgentFile::new(
            "AGENT.md",
            format!(
                "---\nname: release-agent\ndescription: Draft releases\nruntime: labby\nactivation: explicit\ncapabilities:\n  - server_id: gateway\n    family: tool\n    member_id: github::search\n    expected_revision: sha256:abc\n---\n{body}"
            ),
        )]
    }

    #[test]
    fn hostile_instructions_are_returned_only_as_inert_text() {
        let hostile = "<script>alert(1)</script> {{system}} `rm -rf /`";
        let agent =
            materialize_logical_agent("release-agent", valid(hostile), Default::default()).unwrap();
        assert_eq!(agent.interchange.descriptor.kind, "agent");
        assert_eq!(agent.preview_text(), hostile);
        assert_eq!(
            agent.interchange.descriptor.metadata["activation"],
            "explicit"
        );
    }

    #[test]
    fn invalid_capabilities_and_implicit_activation_are_rejected() {
        let invalid_family = valid("body")[0]
            .content
            .replace("family: tool", "family: agent");
        assert!(
            materialize_logical_agent(
                "release-agent",
                vec![LogicalAgentFile::new("AGENT.md", invalid_family)],
                Default::default()
            )
            .is_err()
        );
        let implicit = valid("body")[0]
            .content
            .replace("activation: explicit", "activation: automatic");
        assert!(
            materialize_logical_agent(
                "release-agent",
                vec![LogicalAgentFile::new("AGENT.md", implicit)],
                Default::default()
            )
            .is_err()
        );
    }
}
