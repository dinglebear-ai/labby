//! Pure data types and validation helpers for the `setup` Bootstrap service.
//!
//! Kept local to the binary crate so gateway-host builds do not pull in the SDK.
//! Keep these types independent of the retired service SDK surface.

use labby_primitives::plugin_ui::{FieldKind, UiSchema};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

/// Sentinel returned by `setup.draft.get` in place of any value whose owning
/// `UiSchema.secret == true` flag is set.
pub const SECRET_SENTINEL: &str = "***";

/// Fully normalized, replay-stable bootstrap request. This contains no secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessBootstrapManifest {
    pub version: u8,
    pub installation_id: String,
    pub canonical_issuer: String,
    pub organization_name: String,
    pub project_name: String,
    pub subject: String,
    pub loadout_id: String,
    pub route_id: String,
    pub resource: String,
    pub scopes: Vec<String>,
    pub ttl_seconds: u64,
    pub credential_id: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone)]
pub struct AccessBootstrapPrepare {
    pub proof_file: PathBuf,
    pub credential_file: PathBuf,
    pub organization_name: String,
    pub project_name: String,
    pub subject: String,
    pub loadout_id: String,
    pub route_id: String,
    pub resource: String,
    pub scopes: Vec<String>,
    pub ttl_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessBootstrapPrepareOutcome {
    pub prepare_id: String,
    pub proof_id: String,
    pub credential_id: String,
    pub journal_state: PrepareJournalState,
    pub proof_file: PathBuf,
    pub credential_file: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrepareJournalState {
    Allocating,
    FilesPublished,
    ProofActive,
    Consumed,
    Revoked,
    Cleaned,
    ManualFileCleanupRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareFileIdentity {
    pub path: PathBuf,
    pub digest_hex: String,
    #[cfg(unix)]
    pub device: u64,
    #[cfg(unix)]
    pub inode: u64,
    #[cfg(unix)]
    pub parent_device: u64,
    #[cfg(unix)]
    pub parent_inode: u64,
    #[cfg(unix)]
    pub owner: u32,
    #[cfg(unix)]
    pub mode: u32,
    #[cfg(unix)]
    pub links: u64,
}

/// Secret-free, installation-owned crash recovery authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareJournal {
    pub version: u8,
    pub prepare_id: String,
    pub installation_id: String,
    pub proof_id: String,
    pub proof_digest_hex: String,
    pub credential_id: String,
    pub credential_digest_hex: String,
    pub manifest: AccessBootstrapManifest,
    pub manifest_digest_hex: String,
    pub request_digest_hex: String,
    pub idempotency_digest_hex: String,
    pub state: PrepareJournalState,
    pub created_at: i64,
    pub expires_at: i64,
    pub proof_path: PathBuf,
    pub credential_path: PathBuf,
    pub proof_file: Option<PrepareFileIdentity>,
    pub credential_file: Option<PrepareFileIdentity>,
}

/// First-run setup state machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SetupState {
    /// `~/.labby/.env` does not exist at all.
    Uninitialized,
    /// `.env` exists but is missing one or more required core env vars.
    ConfigMissing { envars: Vec<String> },
    /// `.env` is partially populated; some service env keys are missing.
    PartiallyConfigured { missing: Vec<String> },
    /// All required keys present; running health probes.
    HealthChecking { services: Vec<String> },
    /// Probes complete; configuration is committed and healthy.
    Ready,
}

/// Snapshot returned by `setup.state` to the wizard / settings UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupSnapshot {
    pub first_run: bool,
    pub env_path: PathBuf,
    pub draft_path: PathBuf,
    pub last_completed_step: u8,
    pub draft_stale: bool,
    pub has_draft: bool,
    pub draft_entry_count: usize,
    pub env_mtime_unix_seconds: Option<u64>,
    pub draft_mtime_unix_seconds: Option<u64>,
    pub state: SetupState,
}

/// Single key=value entry within a draft mutation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftEntry {
    pub key: String,
    pub value: String,
}

/// Outcome envelope for `setup.draft.commit` and `setup.finalize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitOutcome {
    pub written: usize,
    pub skipped: Vec<String>,
    pub backup_path: Option<PathBuf>,
    pub audit_pass_count: usize,
    pub audit_total_count: usize,
}

#[derive(Debug, Error)]
pub enum SetupError {
    #[error("invalid value for {field}: {reason}")]
    InvalidValue { field: String, reason: String },
}

/// Marker type for setup validation helpers.
#[derive(Debug, Default, Clone, Copy)]
pub struct SetupClient;

impl SetupClient {
    pub const fn new() -> Self {
        Self
    }

    pub fn validate_against_ui_schema(
        field: &str,
        value: &str,
        schema: &UiSchema,
    ) -> Result<(), SetupError> {
        let validation = &schema.validation;
        if validation.required && value.is_empty() {
            return Err(SetupError::InvalidValue {
                field: field.to_owned(),
                reason: "required".into(),
            });
        }
        if let Some(min) = validation.min_length
            && value.chars().count() < min
        {
            return Err(SetupError::InvalidValue {
                field: field.to_owned(),
                reason: format!("shorter than min_length={min}"),
            });
        }
        if let Some(max) = validation.max_length
            && value.chars().count() > max
        {
            return Err(SetupError::InvalidValue {
                field: field.to_owned(),
                reason: format!("longer than max_length={max}"),
            });
        }

        match schema.kind {
            FieldKind::Text | FieldKind::Secret => Ok(()),
            FieldKind::Url => {
                let parsed = url::Url::parse(value).map_err(|e| SetupError::InvalidValue {
                    field: field.to_owned(),
                    reason: format!("not a URL: {e}"),
                })?;
                let scheme = parsed.scheme();
                if scheme != "http" && scheme != "https" {
                    return Err(SetupError::InvalidValue {
                        field: field.to_owned(),
                        reason: format!("scheme must be http or https, got {scheme}"),
                    });
                }
                Ok(())
            }
            FieldKind::Bool => match value {
                "true" | "false" | "1" | "0" => Ok(()),
                _ => Err(SetupError::InvalidValue {
                    field: field.to_owned(),
                    reason: "not a boolean (true|false|1|0)".into(),
                }),
            },
            FieldKind::Number => {
                value.parse::<f64>().map_err(|e| SetupError::InvalidValue {
                    field: field.to_owned(),
                    reason: format!("not a number: {e}"),
                })?;
                Ok(())
            }
            FieldKind::FilePath => validate_file_path(field, value, schema),
            FieldKind::Enum { values } => {
                if values.iter().any(|allowed| *allowed == value) {
                    Ok(())
                } else {
                    Err(SetupError::InvalidValue {
                        field: field.to_owned(),
                        reason: format!("must be one of: {}", values.join(", ")),
                    })
                }
            }
        }
    }
}

fn validate_file_path(field: &str, value: &str, schema: &UiSchema) -> Result<(), SetupError> {
    let path = Path::new(value);
    let mut saw_root = false;
    let mut saw_prefix = false;
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(SetupError::InvalidValue {
                    field: field.to_owned(),
                    reason: "path traversal (..) is not allowed".into(),
                });
            }
            Component::RootDir => saw_root = true,
            Component::Prefix(_) => saw_prefix = true,
            _ => {}
        }
    }
    if (saw_root || saw_prefix) && schema.validation.safe_root.is_none() {
        return Err(SetupError::InvalidValue {
            field: field.to_owned(),
            reason: "absolute paths require a configured safe_root".into(),
        });
    }
    Ok(())
}
