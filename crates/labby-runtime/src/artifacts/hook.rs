//! Canonical conversion between bounded Hook declarations and inert Artifacts.
//!
//! A hook command is an executable plus an argument vector. A future host
//! activator must pass `command` and each `arguments` element directly to the
//! operating-system process API. It must never concatenate them into a command
//! line or invoke a shell implicitly.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{
    ArtifactComponent, ArtifactDescriptor, ArtifactInterchange, ArtifactLicenseState,
    ArtifactLineage, ArtifactProvenance, ArtifactPublication, ArtifactRevision, Distribution,
    JsonMap, PublicationState, Visibility,
};
use super::{ArtifactError, invalid};

pub const MAX_HOOK_BYTES: usize = 256 * 1024;
const MAX_HOOK_NAME_BYTES: usize = 128;
const MAX_HOOK_DESCRIPTION_BYTES: usize = 4_096;
const MAX_HOOK_COMMAND_BYTES: usize = 4_096;
const MAX_HOOK_ARGUMENTS: usize = 64;
const MAX_HOOK_ARGUMENT_BYTES: usize = 1_024;

/// Events currently understood by Labby's host integrations. The artifact is only a
/// declaration: authoring, validation, and preview never register or execute it.
pub const SUPPORTED_HOOK_EVENTS: &[&str] = &[
    "config_change",
    "notification",
    "permission_request",
    "post_tool_use",
    "pre_compact",
    "pre_tool_use",
    "session_end",
    "session_start",
    "stop",
    "subagent_stop",
    "user_prompt_submit",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalHookFile {
    pub path: String,
    pub content: String,
}

impl LogicalHookFile {
    #[must_use]
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedHook {
    pub files: BTreeMap<String, Vec<u8>>,
    pub interchange: ArtifactInterchange,
    preview: String,
}

impl MaterializedHook {
    #[must_use]
    pub fn preview_text(&self) -> &str {
        &self.preview
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HookDeclaration {
    name: String,
    description: String,
    event: String,
    command: String,
    #[serde(default)]
    arguments: Vec<String>,
}

pub fn materialize_logical_hook(
    name: &str,
    files: Vec<LogicalHookFile>,
    mut provenance: ArtifactProvenance,
) -> Result<MaterializedHook, ArtifactError> {
    validate_name(name)?;
    if files.len() != 1 {
        return Err(invalid("files", "hook_requires_single_file"));
    }
    let file = files.into_iter().next().expect("length checked");
    if file.path != "HOOK.json" {
        return Err(invalid("files", "hook_file_name"));
    }
    if file.content.len() > MAX_HOOK_BYTES {
        return Err(ArtifactError::LimitExceeded {
            what: "hook_bytes",
            limit: MAX_HOOK_BYTES as u64,
        });
    }
    if file.content.contains('\0') {
        return Err(invalid("content", "nul_content"));
    }
    let declaration: HookDeclaration =
        serde_json::from_str(&file.content).map_err(|_| invalid("hook", "invalid_json"))?;
    if declaration.name != name {
        return Err(invalid("name", "declaration_mismatch"));
    }
    validate_name(&declaration.name)?;
    if declaration.description.trim().is_empty()
        || declaration.description.len() > MAX_HOOK_DESCRIPTION_BYTES
        || declaration.description.chars().any(char::is_control)
    {
        return Err(invalid("description", "invalid"));
    }
    if !SUPPORTED_HOOK_EVENTS.contains(&declaration.event.as_str()) {
        return Err(invalid("event", "unsupported"));
    }
    validate_command(&declaration.command)?;
    validate_arguments(&declaration.arguments)?;

    let preview = serde_json::to_string_pretty(&declaration)?;
    let bytes = file.content.into_bytes();
    let revision = ArtifactRevision::from_components(
        vec![ArtifactComponent::from_bytes("HOOK.json", &bytes, None)?],
        None,
        None,
        None,
        JsonMap::new(),
    )?;
    let mut descriptor = ArtifactDescriptor::for_identity("hook", "labby", name)?;
    descriptor.description = Some(declaration.description);
    descriptor
        .metadata
        .insert("event".to_owned(), Value::String(declaration.event));
    descriptor.metadata.insert(
        "executionPolicy".to_owned(),
        Value::String("explicit_activation".to_owned()),
    );
    provenance
        .adapter
        .get_or_insert_with(|| "labby.hook/v1".to_owned());
    provenance
        .original_format
        .get_or_insert_with(|| "hook-json".to_owned());
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
    Ok(MaterializedHook {
        files: BTreeMap::from([("HOOK.json".to_owned(), bytes)]),
        interchange,
        preview,
    })
}

fn validate_command(command: &str) -> Result<(), ArtifactError> {
    if command.is_empty()
        || command.len() > MAX_HOOK_COMMAND_BYTES
        || command.chars().any(char::is_control)
    {
        return Err(invalid("command", "invalid"));
    }
    if command.contains('/')
        || command.contains('\\')
        || command == "."
        || command == ".."
        || !command
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(invalid("command", "executable_name_required"));
    }
    Ok(())
}

fn validate_arguments(arguments: &[String]) -> Result<(), ArtifactError> {
    if arguments.len() > MAX_HOOK_ARGUMENTS {
        return Err(invalid("arguments", "too_many"));
    }
    for argument in arguments {
        if argument.len() > MAX_HOOK_ARGUMENT_BYTES || argument.chars().any(char::is_control) {
            return Err(invalid("arguments", "invalid"));
        }
    }
    Ok(())
}

fn validate_name(value: &str) -> Result<(), ArtifactError> {
    if value.is_empty()
        || value.len() > MAX_HOOK_NAME_BYTES
        || !value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
    {
        return Err(invalid("name", "invalid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(event: &str, command: &str, arguments: Value) -> Vec<LogicalHookFile> {
        vec![LogicalHookFile::new(
            "HOOK.json",
            serde_json::json!({
                "name":"audit-tools", "description":"Audit tool requests", "event":event,
                "command":command, "arguments":arguments
            })
            .to_string(),
        )]
    }

    #[test]
    fn valid_hook_preview_is_inert_and_policy_marked() {
        let hook = materialize_logical_hook(
            "audit-tools",
            source("pre_tool_use", "labby", serde_json::json!(["doctor"])),
            Default::default(),
        )
        .unwrap();
        assert_eq!(hook.interchange.descriptor.kind, "hook");
        assert_eq!(
            hook.interchange.descriptor.metadata["executionPolicy"],
            "explicit_activation"
        );
        assert!(hook.preview_text().contains("\"command\": \"labby\""));
    }

    #[test]
    fn unsafe_paths_executables_control_characters_and_events_are_rejected() {
        assert!(
            materialize_logical_hook(
                "audit-tools",
                vec![LogicalHookFile::new("../HOOK.json", "{}")],
                Default::default()
            )
            .is_err()
        );
        assert!(
            materialize_logical_hook(
                "audit-tools",
                source("unknown", "labby", serde_json::json!([])),
                Default::default()
            )
            .is_err()
        );
        assert!(
            materialize_logical_hook(
                "audit-tools",
                source("stop", "/bin/sh", serde_json::json!([])),
                Default::default()
            )
            .is_err()
        );
        assert!(
            materialize_logical_hook(
                "audit-tools",
                source("stop", "labby", serde_json::json!(["line\nbreak"])),
                Default::default()
            )
            .is_err()
        );
        assert!(
            materialize_logical_hook(
                "audit-tools",
                source("stop", "labby", serde_json::json!(["tab\tbreak"])),
                Default::default()
            )
            .is_err()
        );
    }

    #[test]
    fn unix_shell_metacharacters_are_literal_argument_values() {
        let arguments = serde_json::json!([
            "$(touch /tmp/labby-hook-must-not-exist)",
            "doctor; rm -rf /tmp/example",
            "`id` | cat",
            "${HOME}",
            "@response-file"
        ]);
        let hook = materialize_logical_hook(
            "audit-tools",
            source("stop", "labby", arguments.clone()),
            Default::default(),
        )
        .expect("shell syntax is inert when retained as argv elements");
        let preview: Value = serde_json::from_str(hook.preview_text()).unwrap();
        assert_eq!(preview["arguments"], arguments);
    }

    #[test]
    fn windows_shell_metacharacters_are_literal_argument_values() {
        let arguments = serde_json::json!([
            "& calc.exe",
            "| whoami",
            "%COMSPEC%",
            "$(Get-Process)",
            "@response-file",
            "quoted value with spaces"
        ]);
        let hook = materialize_logical_hook(
            "audit-tools",
            source("stop", "labby.exe", arguments.clone()),
            Default::default(),
        )
        .expect("shell syntax is inert when retained as argv elements");
        let preview: Value = serde_json::from_str(hook.preview_text()).unwrap();
        assert_eq!(preview["arguments"], arguments);
    }

    #[test]
    fn direct_process_fields_enforce_count_size_nul_and_control_bounds() {
        for command in ["", "labby\0shadow", "labby\nshadow"] {
            assert!(
                materialize_logical_hook(
                    "audit-tools",
                    source("stop", command, serde_json::json!([])),
                    Default::default(),
                )
                .is_err()
            );
        }
        assert!(
            materialize_logical_hook(
                "audit-tools",
                source(
                    "stop",
                    "labby",
                    serde_json::json!(vec!["argument"; MAX_HOOK_ARGUMENTS + 1]),
                ),
                Default::default(),
            )
            .is_err()
        );
        for argument in [
            "x".repeat(MAX_HOOK_ARGUMENT_BYTES + 1),
            "nul\0argument".into(),
            "control\u{7f}argument".into(),
        ] {
            assert!(
                materialize_logical_hook(
                    "audit-tools",
                    source("stop", "labby", serde_json::json!([argument])),
                    Default::default(),
                )
                .is_err()
            );
        }
    }
}
