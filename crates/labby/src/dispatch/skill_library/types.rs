#![allow(dead_code, reason = "consumed by the concurrent Wave 3 dispatcher")]

use serde::{Deserialize, Serialize};

use labby_runtime::artifacts::{ArtifactAcquisition, ArtifactInterchange, ArtifactPayloadFile};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogicalFileInput {
    pub(crate) path: String,
    pub(crate) content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AcquisitionInput {
    pub(crate) interchange: ArtifactInterchange,
    pub(crate) files: Vec<AcquisitionFileInput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AcquisitionFileInput {
    pub(crate) path: String,
    pub(crate) content: String,
}

impl From<AcquisitionInput> for ArtifactAcquisition {
    fn from(value: AcquisitionInput) -> Self {
        Self {
            interchange: value.interchange,
            files: value
                .files
                .into_iter()
                .map(|file| ArtifactPayloadFile {
                    path: file.path,
                    bytes: file.content.into_bytes(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CursorPage<T> {
    pub(crate) items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SkillLibrarySummary {
    pub(crate) artifact_id: String,
    pub(crate) name: String,
    pub(crate) archived: bool,
    pub(crate) active_revision_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RevisionSummary {
    pub(crate) revision_id: String,
    pub(crate) created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ValidationResponse {
    pub(crate) valid: bool,
    pub(crate) revision_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MutationReceipt {
    pub(crate) outcome: String,
    pub(crate) artifact_id: String,
    pub(crate) active_revision_id: Option<String>,
    pub(crate) canonical_uri: Option<String>,
    pub(crate) old_generation: u64,
    pub(crate) new_generation: u64,
    pub(crate) committed_library_version: u64,
    pub(crate) published_library_version: u64,
    pub(crate) library_digest: String,
    pub(crate) rejected_entries: CursorPage<RejectedEntry>,
    pub(crate) relist_required: bool,
    pub(crate) relist_guidance: &'static str,
    pub(crate) list_changed_notification: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RejectedEntry {
    pub(crate) name: String,
    pub(crate) kind: String,
}

pub(crate) const RELIST_GUIDANCE: &str = "Re-run skills.list or native skills/list; Labby does not emit a Skills list_changed notification.";
