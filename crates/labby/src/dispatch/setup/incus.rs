//! Local-only Incus helpers for host-side Labby gateway bootstrap.
//!
//! These helpers are intentionally CLI-only. They are not in the setup action
//! catalog and must not be exposed through MCP, HTTP, or Code Mode.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde::Deserialize;
use serde_yaml::Value;

use crate::dispatch::error::ToolError;

const SUPPORTED_BACKUP_KEYS: &[&str] = &[
    "snapshots.schedule",
    "snapshots.expiry",
    "snapshots.pattern",
    "snapshots.schedule.stopped",
];

#[derive(Debug, Deserialize)]
struct IncusConfigDocument {
    config: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct BackupConfigEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct BackupConfigApplyOutcome {
    pub container: String,
    pub dry_run: bool,
    pub applied: Vec<BackupConfigEntry>,
}

pub(crate) fn parse_backup_config(path: &Path) -> Result<Vec<BackupConfigEntry>, ToolError> {
    let raw = std::fs::read_to_string(path).map_err(|e| ToolError::Sdk {
        message: format!("failed to read Incus backup config {}: {e}", path.display()),
        sdk_kind: "incus_backup_config_read_failed".into(),
    })?;
    parse_backup_config_str(&raw)
}

pub(crate) fn parse_backup_config_str(raw: &str) -> Result<Vec<BackupConfigEntry>, ToolError> {
    let doc: IncusConfigDocument = serde_yaml::from_str(raw).map_err(|e| ToolError::Sdk {
        message: format!("invalid Incus backup YAML: {e}"),
        sdk_kind: "incus_backup_config_invalid_yaml".into(),
    })?;

    let mut entries = Vec::new();
    for (key, value) in doc.config {
        validate_backup_key(&key)?;
        entries.push(BackupConfigEntry {
            key,
            value: scalar_to_string(value)?,
        });
    }
    if entries.is_empty() {
        return Err(ToolError::Sdk {
            message: "Incus backup config must contain at least one supported config key".into(),
            sdk_kind: "incus_backup_config_empty".into(),
        });
    }
    Ok(entries)
}

pub(crate) fn apply_backup_config(
    container: &str,
    path: &Path,
    dry_run: bool,
) -> Result<BackupConfigApplyOutcome, ToolError> {
    if container.trim().is_empty() {
        return Err(ToolError::MissingParam {
            message: "missing required parameter `container`".into(),
            param: "container".into(),
        });
    }
    let entries = parse_backup_config(path)?;
    if !dry_run {
        for entry in &entries {
            let status = Command::new("incus")
                .arg("config")
                .arg("set")
                .arg(container)
                .arg(&entry.key)
                .arg(&entry.value)
                .status()
                .map_err(|e| ToolError::Sdk {
                    message: format!("failed to run incus config set: {e}"),
                    sdk_kind: "incus_config_set_failed".into(),
                })?;
            if !status.success() {
                return Err(ToolError::Sdk {
                    message: format!(
                        "incus config set failed for {} on container {}",
                        entry.key, container
                    ),
                    sdk_kind: "incus_config_set_failed".into(),
                });
            }
        }
    }
    Ok(BackupConfigApplyOutcome {
        container: container.to_string(),
        dry_run,
        applied: entries,
    })
}

fn validate_backup_key(key: &str) -> Result<(), ToolError> {
    if SUPPORTED_BACKUP_KEYS.contains(&key) {
        return Ok(());
    }
    Err(ToolError::Sdk {
        message: format!("unsupported Incus backup config key: {key}"),
        sdk_kind: "incus_backup_config_unsupported_key".into(),
    })
}

fn scalar_to_string(value: Value) -> Result<String, ToolError> {
    match value {
        Value::String(value) => Ok(value),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Null | Value::Sequence(_) | Value::Mapping(_) | Value::Tagged(_) => {
            Err(ToolError::Sdk {
                message: "Incus backup config values must be scalar strings, booleans, or numbers"
                    .into(),
                sdk_kind: "incus_backup_config_non_scalar".into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_snapshot_keys() {
        let entries = parse_backup_config_str(
            r#"
config:
  snapshots.schedule: "@daily"
  snapshots.expiry: "14d"
  snapshots.pattern: "labby-{{ creation_date|date:'2006-01-02_15-04-05' }}"
  snapshots.schedule.stopped: false
"#,
        )
        .unwrap();
        assert_eq!(entries.len(), 4);
        assert!(
            entries.iter().any(|entry| {
                entry.key == "snapshots.schedule.stopped" && entry.value == "false"
            })
        );
    }

    #[test]
    fn rejects_unknown_keys() {
        let err = parse_backup_config_str(
            r#"
config:
  security.privileged: true
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind(), "incus_backup_config_unsupported_key");
    }

    #[test]
    fn rejects_non_scalar_values() {
        let err = parse_backup_config_str(
            r#"
config:
  snapshots.schedule:
    nested: nope
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind(), "incus_backup_config_non_scalar");
    }
}
