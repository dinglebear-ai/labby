//! Canonical conversion between bounded Prompt source and inert Artifacts.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;

use super::model::{
    ArtifactComponent, ArtifactDescriptor, ArtifactInterchange, ArtifactLicenseState,
    ArtifactLineage, ArtifactProvenance, ArtifactPublication, ArtifactRevision, Distribution,
    JsonMap, PublicationState, Visibility,
};
use super::{ArtifactError, invalid};

pub const MAX_PROMPT_BYTES: usize = 256 * 1024;
const MAX_PROMPT_NAME_BYTES: usize = 128;
const MAX_PROMPT_DESCRIPTION_BYTES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalPromptFile {
    pub path: String,
    pub content: String,
}

impl LogicalPromptFile {
    #[must_use]
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedPrompt {
    pub files: BTreeMap<String, Vec<u8>>,
    pub interchange: ArtifactInterchange,
    preview: String,
}

impl MaterializedPrompt {
    #[must_use]
    pub fn preview_text(&self) -> &str {
        &self.preview
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    arguments: Vec<String>,
}

pub fn materialize_logical_prompt(
    name: &str,
    files: Vec<LogicalPromptFile>,
    mut provenance: ArtifactProvenance,
) -> Result<MaterializedPrompt, ArtifactError> {
    validate_name(name)?;
    if files.len() != 1 {
        return Err(invalid("files", "prompt_requires_single_file"));
    }
    let file = files.into_iter().next().expect("length checked");
    if file.path != "PROMPT.md" {
        return Err(invalid("files", "prompt_file_name"));
    }
    if file.content.len() > MAX_PROMPT_BYTES {
        return Err(ArtifactError::LimitExceeded {
            what: "prompt_bytes",
            limit: MAX_PROMPT_BYTES as u64,
        });
    }
    if file.content.contains('\0') {
        return Err(invalid("content", "nul_content"));
    }
    let (yaml, body) = split_frontmatter(&file.content)?;
    let frontmatter: PromptFrontmatter =
        serde_yaml_ng::from_str(yaml).map_err(|_| invalid("frontmatter", "invalid_yaml"))?;
    if frontmatter.name != name {
        return Err(invalid("name", "frontmatter_mismatch"));
    }
    validate_name(&frontmatter.name)?;
    if frontmatter.description.trim().is_empty()
        || frontmatter.description.len() > MAX_PROMPT_DESCRIPTION_BYTES
        || frontmatter.description.contains('\0')
    {
        return Err(invalid("description", "invalid"));
    }
    if body.trim().is_empty() {
        return Err(invalid("content", "empty"));
    }
    let preview = body.to_owned();
    let mut arguments = BTreeSet::new();
    for argument in &frontmatter.arguments {
        validate_name(argument)?;
        if !arguments.insert(argument) {
            return Err(invalid("arguments", "duplicate"));
        }
    }

    let bytes = file.content.into_bytes();
    let revision = ArtifactRevision::from_components(
        vec![ArtifactComponent::from_bytes("PROMPT.md", &bytes, None)?],
        None,
        None,
        None,
        JsonMap::new(),
    )?;
    let mut descriptor = ArtifactDescriptor::for_identity("prompt", "labby", name)?;
    descriptor.description = Some(frontmatter.description);
    descriptor.metadata.insert(
        "arguments".to_owned(),
        Value::Array(
            frontmatter
                .arguments
                .into_iter()
                .map(Value::String)
                .collect(),
        ),
    );
    provenance
        .adapter
        .get_or_insert_with(|| "labby.prompt/v1".to_owned());
    provenance
        .original_format
        .get_or_insert_with(|| "prompt-markdown".to_owned());
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
    Ok(MaterializedPrompt {
        files: BTreeMap::from([("PROMPT.md".to_owned(), bytes)]),
        interchange,
        preview,
    })
}

fn split_frontmatter(content: &str) -> Result<(&str, &str), ArtifactError> {
    let (rest, closing) = if let Some(rest) = content.strip_prefix("---\r\n") {
        (rest, "\r\n---\r\n")
    } else if let Some(rest) = content.strip_prefix("---\n") {
        (rest, "\n---\n")
    } else {
        return Err(invalid("frontmatter", "missing"));
    };
    let (yaml, body) = rest
        .split_once(closing)
        .ok_or_else(|| invalid("frontmatter", "unterminated"))?;
    Ok((yaml, body))
}

fn validate_name(value: &str) -> Result<(), ArtifactError> {
    if value.is_empty()
        || value.len() > MAX_PROMPT_NAME_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
    {
        return Err(invalid("name", "invalid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(body: &str) -> Vec<LogicalPromptFile> {
        vec![LogicalPromptFile::new(
            "PROMPT.md",
            format!("---\nname: release-notes\ndescription: Draft release notes\n---\n{body}"),
        )]
    }

    #[test]
    fn valid_prompt_is_inert_and_content_addressed() {
        let hostile = "<script>alert('no')</script> {{system}}";
        let prompt =
            materialize_logical_prompt("release-notes", valid(hostile), Default::default())
                .unwrap();
        assert_eq!(prompt.interchange.descriptor.kind, "prompt");
        assert_eq!(prompt.interchange.descriptor.name, "release-notes");
        assert!(prompt.files["PROMPT.md"].ends_with(hostile.as_bytes()));
        assert_eq!(prompt.preview_text(), hostile);
    }

    #[test]
    fn windows_line_endings_preserve_content_addressed_source() {
        let source =
            "---\r\nname: release-notes\r\ndescription: Draft release notes\r\n---\r\nBody\r\n";
        let prompt = materialize_logical_prompt(
            "release-notes",
            vec![LogicalPromptFile::new("PROMPT.md", source)],
            Default::default(),
        )
        .unwrap();
        assert_eq!(prompt.files["PROMPT.md"], source.as_bytes());
        assert_eq!(prompt.preview_text(), "Body\r\n");
    }

    #[test]
    fn invalid_prompt_descriptor_and_extra_files_are_rejected() {
        let wrong_name = vec![LogicalPromptFile::new(
            "PROMPT.md",
            "---\nname: other\ndescription: Nope\n---\nbody",
        )];
        assert!(materialize_logical_prompt("expected", wrong_name, Default::default()).is_err());
        let mut extra = valid("body");
        extra.push(LogicalPromptFile::new("HOOK.md", "not allowed"));
        assert!(materialize_logical_prompt("release-notes", extra, Default::default()).is_err());
    }

    #[test]
    fn prompt_content_is_bounded() {
        let oversized = "x".repeat(MAX_PROMPT_BYTES + 1);
        assert!(
            materialize_logical_prompt("release-notes", valid(&oversized), Default::default())
                .is_err()
        );
    }
}
