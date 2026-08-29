#![allow(dead_code, reason = "consumed by the concurrent Wave 3 dispatcher")]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CreateVisibility {
    #[default]
    Private,
    Shared,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogicalFileInput {
    pub(crate) path: String,
    pub(crate) content: String,
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
    pub(crate) latest_revision_id: String,
    pub(crate) visibility: &'static str,
    pub(crate) access_label: &'static str,
    pub(crate) can_mutate: bool,
    pub(crate) owner: OwnerSummary,
    pub(crate) provenance: ProvenanceSummary,
    pub(crate) materialized: bool,
    pub(crate) canonical_uri: Option<String>,
    pub(crate) current_generation: u64,
    pub(crate) published_library_version: u64,
    pub(crate) allowed_actions: Vec<&'static str>,
    pub(crate) latest_revision_files: Vec<RevisionFileSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct OwnerSummary {
    /// Privacy-preserving relationship, never a principal/provider identifier.
    pub(crate) relationship: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProvenanceSummary {
    /// Stable source family only. Source URIs, repository names, refs, and registries are omitted.
    pub(crate) source: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RevisionFileSummary {
    pub(crate) path: String,
    pub(crate) digest: String,
    pub(crate) size: u64,
    pub(crate) media_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VersionedSkillLibraryPage {
    pub(crate) library_version: u64,
    pub(crate) published_library_version: u64,
    pub(crate) can_create: bool,
    pub(crate) create_visibilities: Vec<&'static str>,
    pub(crate) allowed_actions: Vec<&'static str>,
    pub(crate) items: Vec<SkillLibrarySummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VersionedSkillLibrarySummary {
    pub(crate) library_version: u64,
    #[serde(flatten)]
    pub(crate) item: SkillLibrarySummary,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RevisionSummary {
    pub(crate) revision_id: String,
    pub(crate) created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VersionedRevisionPage {
    pub(crate) library_version: u64,
    pub(crate) items: Vec<RevisionSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VersionedRevisionFile {
    pub(crate) library_version: u64,
    pub(crate) artifact_id: String,
    pub(crate) revision_id: String,
    pub(crate) path: String,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ValidationResponse {
    pub(crate) valid: bool,
    pub(crate) artifact_id: Option<String>,
    pub(crate) revision_id: Option<String>,
    pub(crate) rejections: Vec<ValidationRejection>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ValidationRejection {
    pub(crate) field: &'static str,
    pub(crate) code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SkillPreview {
    pub(crate) artifact_id: String,
    pub(crate) revision_id: String,
    /// Explicitly tells clients that all bodies are inert text, never rendered markup.
    pub(crate) render_mode: &'static str,
    pub(crate) files: Vec<SkillPreviewFile>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SkillPreviewFile {
    pub(crate) path: String,
    pub(crate) media_type: &'static str,
    pub(crate) text: String,
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
