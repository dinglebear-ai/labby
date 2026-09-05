//! Revisioned, per-turn capability selections.
//!
//! Execution loadouts are not gateway route configuration. They select an
//! authorized subset of one immutable catalog snapshot for a caller/runtime.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::Ordering;

use labby_runtime::error::ToolError;
use serde::{Deserialize, Serialize};

use self::validation::{normalize_members, validate_text};
use super::manager::GatewayManager;

mod error_impl;
mod persistence;
#[cfg(test)]
mod tests;
mod validation;

const MAX_LOADOUTS: usize = 256;
const MAX_TOTAL_LOADOUTS: usize = 4096;
const MAX_MEMBERS: usize = 512;
const MAX_TEXT_BYTES: usize = 256;
const MAX_PUBLISHED_PRINCIPALS: usize = 256;
const MAX_PUBLISHED_MEMBERS: usize = 16_384;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct ExecutionPrincipal(String);

impl ExecutionPrincipal {
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionLoadoutError> {
        let value = value.into();
        validate_text("principal", &value)?;
        if value == "shared" || value.contains('\0') {
            return Err(ExecutionLoadoutError::Invalid {
                field: "principal".into(),
                message: "an explicit, unambiguous principal is required".into(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionLoadoutContext {
    pub principal: ExecutionPrincipal,
    pub allowed_providers: Option<BTreeSet<String>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityFamily {
    Tool,
    Prompt,
    Resource,
    Skill,
    Agent,
    McpApp,
    McpServer,
    Plugin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRef {
    /// Stable provider/server identity. Display names never authorize.
    pub provider: String,
    pub family: CapabilityFamily,
    pub member_id: String,
    pub expected_revision: String,
}

/// One host-authoritative, principal-filtered catalog publication.
///
/// The product host builds this from canonical stores while applying the
/// caller's authorization policy. Callers can select entries from this
/// snapshot, but can never manufacture an entry or its revision.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityCatalogSnapshot {
    pub generation: String,
    pub principal: String,
    pub members: Vec<CapabilityRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
struct RecordKey(String);

impl RecordKey {
    fn new(principal: &ExecutionPrincipal, id: &str) -> Result<Self, ExecutionLoadoutError> {
        validate_text("id", id)?;
        if id.contains('\0') {
            return Err(ExecutionLoadoutError::Invalid {
                field: "id".into(),
                message: "NUL is not allowed".into(),
            });
        }
        Ok(Self(format!(
            "{}:{}{}",
            principal.as_str().len(),
            principal.as_str(),
            id
        )))
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct PublishedCapabilityCatalog {
    snapshots: HashMap<ExecutionPrincipal, CapabilityCatalogSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionLoadoutCreate {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub members: Vec<CapabilityRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionLoadoutPatch {
    pub expected_draft_revision: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub members: Option<Vec<CapabilityRef>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionLoadoutDraft {
    pub id: String,
    pub owner_principal: ExecutionPrincipal,
    #[serde(default)]
    pub runtime_identity: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub members: Vec<CapabilityRef>,
    pub draft_revision: u64,
    pub desired_active_revision: Option<u64>,
    pub effective_runtime_revision: Option<u64>,
    pub restart_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionLoadoutRevision {
    pub loadout_id: String,
    pub revision: u64,
    pub draft_revision: u64,
    pub members: Vec<CapabilityRef>,
    pub catalog_generation: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionStatus {
    Effective,
    Missing,
    Stale,
    Unauthorized,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedCapability {
    pub capability: CapabilityRef,
    pub status: ResolutionStatus,
    pub current_revision: Option<String>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionLoadoutPreview {
    pub loadout_id: String,
    pub draft_revision: u64,
    /// Immutable revision currently effective for this runtime. Zero means the
    /// draft has not been activated yet.
    pub active_revision: u64,
    pub catalog_generation: String,
    pub principal: String,
    pub runtime_identity: String,
    pub resolved: Vec<ResolvedCapability>,
    pub effective: Vec<CapabilityRef>,
    pub missing: Vec<CapabilityRef>,
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionLoadoutActivation {
    pub loadout: ExecutionLoadoutDraft,
    pub revision: ExecutionLoadoutRevision,
    pub preview: ExecutionLoadoutPreview,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionLoadoutSummary {
    pub id: String,
    pub name: String,
    pub draft_revision: u64,
    pub desired_active_revision: Option<u64>,
    pub effective_runtime_revision: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ExecutionLoadoutError {
    NotFound {
        id: String,
    },
    AlreadyExists {
        id: String,
    },
    LimitExceeded {
        limit: usize,
    },
    Invalid {
        field: String,
        message: String,
    },
    StaleRevision {
        expected: u64,
        current: u64,
        changed_fields: Vec<String>,
    },
    Unresolved {
        preview: Box<ExecutionLoadoutPreview>,
    },
    Storage {
        message: String,
    },
    Durability {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Record {
    pub(super) draft: ExecutionLoadoutDraft,
    pub(super) revisions: BTreeMap<u64, ExecutionLoadoutRevision>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(super) struct ExecutionLoadoutStore {
    records: HashMap<RecordKey, Record>,
}

impl ExecutionLoadoutStore {
    fn validate_integrity(&self) -> Result<(), ExecutionLoadoutError> {
        if self.records.len() > MAX_TOTAL_LOADOUTS {
            return Err(ExecutionLoadoutError::LimitExceeded {
                limit: MAX_TOTAL_LOADOUTS,
            });
        }
        let mut per_principal = HashMap::<&ExecutionPrincipal, usize>::new();
        for (key, record) in &self.records {
            ExecutionPrincipal::new(record.draft.owner_principal.as_str())?;
            let expected = RecordKey::new(&record.draft.owner_principal, &record.draft.id)?;
            if key != &expected {
                return Err(invalid_store(
                    "record key does not match its principal and id",
                ));
            }
            let count = per_principal
                .entry(&record.draft.owner_principal)
                .or_default();
            *count += 1;
            if *count > MAX_LOADOUTS {
                return Err(ExecutionLoadoutError::LimitExceeded {
                    limit: MAX_LOADOUTS,
                });
            }
            normalize_members(record.draft.members.clone())?;
            for (number, revision) in &record.revisions {
                if revision.revision != *number
                    || revision.loadout_id != record.draft.id
                    || revision.draft_revision == 0
                {
                    return Err(invalid_store("immutable revision metadata is inconsistent"));
                }
                normalize_members(revision.members.clone())?;
            }
            for active in [
                record.draft.desired_active_revision,
                record.draft.effective_runtime_revision,
            ]
            .into_iter()
            .flatten()
            {
                if !record.revisions.contains_key(&active) {
                    return Err(invalid_store("active revision does not exist"));
                }
            }
        }
        Ok(())
    }
}

fn invalid_store(message: &str) -> ExecutionLoadoutError {
    ExecutionLoadoutError::Storage {
        message: message.into(),
    }
}

impl GatewayManager {
    #[cfg(test)]
    fn fail_next_execution_loadout_persist(&self) {
        self.execution_loadout_fail_persist
            .store(1, Ordering::Release);
    }

    #[cfg(test)]
    fn fail_next_execution_loadout_parent_sync(&self) {
        self.execution_loadout_fail_persist
            .store(2, Ordering::Release);
    }

    #[cfg(test)]
    fn set_execution_loadout_activation_hook(
        &self,
        entered: Arc<std::sync::Barrier>,
        resume: Arc<std::sync::Barrier>,
    ) {
        *self.execution_loadout_activation_hook.lock().unwrap() = Some((entered, resume));
    }

    fn take_execution_loadout_persist_failure(&self) -> u8 {
        #[cfg(test)]
        {
            return self
                .execution_loadout_fail_persist
                .swap(0, Ordering::AcqRel);
        }
        #[cfg(not(test))]
        {
            0
        }
    }
    /// Atomically publish complete principal-filtered snapshots prepared by the
    /// authoritative product host. A single swap prevents mixed generations.
    pub fn publish_execution_capability_snapshots(
        &self,
        snapshots: Vec<CapabilityCatalogSnapshot>,
    ) -> Result<(), ExecutionLoadoutError> {
        let _publication = self.execution_capability_publication.write().map_err(|_| {
            ExecutionLoadoutError::Storage {
                message: "capability publication lock poisoned".into(),
            }
        })?;
        if snapshots.len() > MAX_PUBLISHED_PRINCIPALS {
            return Err(ExecutionLoadoutError::LimitExceeded {
                limit: MAX_PUBLISHED_PRINCIPALS,
            });
        }
        let mut published = PublishedCapabilityCatalog::default();
        let mut publication_generation = None;
        let mut total_members = 0usize;
        for snapshot in snapshots {
            validate_text("generation", &snapshot.generation)?;
            if publication_generation.get_or_insert_with(|| snapshot.generation.clone())
                != &snapshot.generation
            {
                return Err(ExecutionLoadoutError::Invalid {
                    field: "generation".into(),
                    message: "all snapshots in one publication must share one generation".into(),
                });
            }
            let principal = ExecutionPrincipal::new(snapshot.principal.clone())?;
            let snapshot = CapabilityCatalogSnapshot {
                members: normalize_members(snapshot.members)?,
                ..snapshot
            };
            total_members = total_members.saturating_add(snapshot.members.len());
            if total_members > MAX_PUBLISHED_MEMBERS {
                return Err(ExecutionLoadoutError::LimitExceeded {
                    limit: MAX_PUBLISHED_MEMBERS,
                });
            }
            if published.snapshots.insert(principal, snapshot).is_some() {
                return Err(ExecutionLoadoutError::Invalid {
                    field: "principal".into(),
                    message: "duplicate principal capability snapshot".into(),
                });
            }
        }
        self.execution_capabilities.store(Arc::new(published));
        Ok(())
    }

    pub(crate) async fn execution_loadout_revision_contains(
        &self,
        principal: &str,
        runtime_identity: &str,
        id: &str,
        revision: u64,
        tool_id: Option<&str>,
        contract_hash: Option<&str>,
    ) -> Result<(), ToolError> {
        let principal = ExecutionPrincipal::new(principal.to_owned()).map_err(ToolError::from)?;
        let key = RecordKey::new(&principal, id).map_err(ToolError::from)?;
        let store = self.execution_loadouts.read().await;
        let record = store
            .records
            .get(&key)
            .ok_or_else(|| ExecutionLoadoutError::NotFound { id: id.into() })?;
        if record.draft.runtime_identity.as_deref() != Some(runtime_identity) {
            return Err(ExecutionLoadoutError::NotFound { id: id.into() }.into());
        }
        if record.draft.effective_runtime_revision != Some(revision) {
            return Err(ExecutionLoadoutError::StaleRevision {
                expected: revision,
                current: record.draft.effective_runtime_revision.unwrap_or(0),
                changed_fields: vec!["effectiveRuntimeRevision".into()],
            }
            .into());
        }
        if let Some(tool_id) = tool_id {
            let allowed = record.revisions.get(&revision).is_some_and(|value| {
                value.members.iter().any(|member| {
                    member.family == CapabilityFamily::Tool
                        && member.member_id == tool_id
                        && contract_hash.is_none_or(|hash| member.expected_revision == hash)
                })
            });
            if !allowed {
                return Err(ToolError::Sdk {
                    sdk_kind: "forbidden".into(),
                    message: "tool is not bound to the immutable execution loadout revision".into(),
                });
            }
        }
        Ok(())
    }

    pub async fn execution_loadout_list(
        &self,
        context: &ExecutionLoadoutContext,
    ) -> Vec<ExecutionLoadoutSummary> {
        let principal = &context.principal;
        let store = self.execution_loadouts.read().await;
        let mut rows = store
            .records
            .values()
            .filter(|record| &record.draft.owner_principal == principal)
            .map(|record| ExecutionLoadoutSummary {
                id: record.draft.id.clone(),
                name: record.draft.name.clone(),
                draft_revision: record.draft.draft_revision,
                desired_active_revision: record.draft.desired_active_revision,
                effective_runtime_revision: record.draft.effective_runtime_revision,
            })
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        rows
    }

    pub async fn execution_loadout_get(
        &self,
        context: &ExecutionLoadoutContext,
        id: &str,
    ) -> Result<ExecutionLoadoutDraft, ExecutionLoadoutError> {
        let key = RecordKey::new(&context.principal, id)?;
        self.execution_loadouts
            .read()
            .await
            .records
            .get(&key)
            .map(|r| r.draft.clone())
            .ok_or_else(|| ExecutionLoadoutError::NotFound { id: id.into() })
    }

    pub async fn execution_loadout_create(
        &self,
        context: &ExecutionLoadoutContext,
        input: ExecutionLoadoutCreate,
    ) -> Result<ExecutionLoadoutDraft, ExecutionLoadoutError> {
        let members = normalize_members(input.members)?;
        validate_text("id", &input.id)?;
        validate_text("name", &input.name)?;
        if let Some(value) = input.description.as_deref() {
            validate_text("description", value)?;
        }
        let owner_principal = context.principal.clone();
        let key = RecordKey::new(&owner_principal, &input.id)?;
        let fail = self.take_execution_loadout_persist_failure();
        let mut current = self.execution_loadouts.write().await;
        let outcome = ExecutionLoadoutStore::mutate(&self.path, fail, move |store| {
            if store.records.contains_key(&key) {
                return Err(ExecutionLoadoutError::AlreadyExists { id: input.id });
            }
            if store.records.len() >= MAX_TOTAL_LOADOUTS {
                return Err(ExecutionLoadoutError::LimitExceeded {
                    limit: MAX_TOTAL_LOADOUTS,
                });
            }
            let principal_count = store
                .records
                .values()
                .filter(|record| record.draft.owner_principal == owner_principal)
                .count();
            if principal_count >= MAX_LOADOUTS {
                return Err(ExecutionLoadoutError::LimitExceeded {
                    limit: MAX_LOADOUTS,
                });
            }
            let draft = ExecutionLoadoutDraft {
                id: input.id.clone(),
                owner_principal,
                runtime_identity: None,
                name: input.name,
                description: input.description,
                members,
                draft_revision: 1,
                desired_active_revision: None,
                effective_runtime_revision: None,
                restart_required: false,
            };
            store.records.insert(
                key,
                Record {
                    draft: draft.clone(),
                    revisions: BTreeMap::new(),
                },
            );
            Ok(draft)
        })?;
        *current = outcome.store;
        if let Some(error) = outcome.durability_error {
            return Err(error);
        }
        Ok(outcome.result)
    }

    pub async fn execution_loadout_patch(
        &self,
        context: &ExecutionLoadoutContext,
        id: &str,
        patch: ExecutionLoadoutPatch,
    ) -> Result<ExecutionLoadoutDraft, ExecutionLoadoutError> {
        let normalized = patch.members.map(normalize_members).transpose()?;
        if let Some(value) = patch.name.as_deref() {
            validate_text("name", value)?;
        }
        if let Some(Some(value)) = patch.description.as_ref() {
            validate_text("description", value)?;
        }
        let key = RecordKey::new(&context.principal, id)?;
        let fail = self.take_execution_loadout_persist_failure();
        let mut current = self.execution_loadouts.write().await;
        let outcome = ExecutionLoadoutStore::mutate(&self.path, fail, move |store| {
            let record = store
                .records
                .get_mut(&key)
                .ok_or_else(|| ExecutionLoadoutError::NotFound { id: id.into() })?;
            if patch.expected_draft_revision != record.draft.draft_revision {
                return Err(ExecutionLoadoutError::StaleRevision {
                    expected: patch.expected_draft_revision,
                    current: record.draft.draft_revision,
                    changed_fields: vec!["name".into(), "description".into(), "members".into()],
                });
            }
            if let Some(value) = patch.name {
                record.draft.name = value;
            }
            if let Some(value) = patch.description {
                record.draft.description = value;
            }
            if let Some(value) = normalized {
                record.draft.members = value;
            }
            record.draft.draft_revision += 1;
            Ok(record.draft.clone())
        })?;
        *current = outcome.store;
        if let Some(error) = outcome.durability_error {
            return Err(error);
        }
        Ok(outcome.result)
    }

    pub async fn execution_loadout_preview(
        &self,
        context: &ExecutionLoadoutContext,
        id: &str,
        runtime_identity: &str,
    ) -> Result<ExecutionLoadoutPreview, ToolError> {
        validate_text("runtimeIdentity", runtime_identity).map_err(ToolError::from)?;
        let draft = self
            .execution_loadout_get(context, id)
            .await
            .map_err(ToolError::from)?;
        if draft
            .runtime_identity
            .as_deref()
            .is_some_and(|bound| bound != runtime_identity)
        {
            return Err(ExecutionLoadoutError::NotFound { id: id.into() }.into());
        }
        self.resolve_execution_loadout(context, &draft, runtime_identity)
    }

    pub async fn execution_loadout_activate(
        &self,
        context: &ExecutionLoadoutContext,
        id: &str,
        expected_draft_revision: u64,
        runtime_identity: &str,
    ) -> Result<ExecutionLoadoutActivation, ToolError> {
        // Never trust a cached preview: capture and resolve a fresh atomic catalog snapshot.
        let draft = self
            .execution_loadout_get(context, id)
            .await
            .map_err(ToolError::from)?;
        if draft.draft_revision != expected_draft_revision {
            return Err(ExecutionLoadoutError::StaleRevision {
                expected: expected_draft_revision,
                current: draft.draft_revision,
                changed_fields: vec!["name".into(), "description".into(), "members".into()],
            }
            .into());
        }
        let mut current = self.execution_loadouts.write().await;
        let _publication = self.execution_capability_publication.read().map_err(|_| {
            ToolError::from(ExecutionLoadoutError::Storage {
                message: "capability publication lock poisoned".into(),
            })
        })?;
        #[cfg(test)]
        if let Some((entered, resume)) = self
            .execution_loadout_activation_hook
            .lock()
            .unwrap()
            .take()
        {
            entered.wait();
            resume.wait();
        }
        let preview = self.resolve_execution_loadout(context, &draft, runtime_identity)?;
        if preview
            .resolved
            .iter()
            .any(|item| item.status != ResolutionStatus::Effective)
        {
            return Err(ExecutionLoadoutError::Unresolved {
                preview: Box::new(preview),
            }
            .into());
        }
        let key = RecordKey::new(&context.principal, id).map_err(ToolError::from)?;
        let fail = self.take_execution_loadout_persist_failure();
        let outcome = ExecutionLoadoutStore::mutate(&self.path, fail, move |store| {
            let record = store
                .records
                .get_mut(&key)
                .ok_or_else(|| ExecutionLoadoutError::NotFound { id: id.into() })?;
            if record.draft.draft_revision != expected_draft_revision {
                return Err(ExecutionLoadoutError::StaleRevision {
                    expected: expected_draft_revision,
                    current: record.draft.draft_revision,
                    changed_fields: vec!["members".into()],
                });
            }
            if record
                .draft
                .runtime_identity
                .as_deref()
                .is_some_and(|bound| bound != runtime_identity)
            {
                return Err(ExecutionLoadoutError::NotFound { id: id.into() });
            }
            if let Some(existing) = record
                .revisions
                .values()
                .find(|revision| revision.draft_revision == expected_draft_revision)
                .cloned()
            {
                let mut preview = preview;
                preview.active_revision = existing.revision;
                return Ok(ExecutionLoadoutActivation {
                    loadout: record.draft.clone(),
                    revision: existing,
                    preview,
                });
            }
            record.draft.runtime_identity = Some(runtime_identity.into());
            let revision_number = record.revisions.keys().next_back().copied().unwrap_or(0) + 1;
            let revision = ExecutionLoadoutRevision {
                loadout_id: id.into(),
                revision: revision_number,
                draft_revision: expected_draft_revision,
                members: record.draft.members.clone(),
                catalog_generation: preview.catalog_generation.clone(),
            };
            record.revisions.insert(revision_number, revision.clone());
            record.draft.desired_active_revision = Some(revision_number);
            record.draft.effective_runtime_revision = Some(revision_number);
            record.draft.restart_required = false;
            let loadout = record.draft.clone();
            Ok(ExecutionLoadoutActivation {
                loadout,
                revision,
                preview,
            })
        })
        .map_err(ToolError::from)?;
        *current = outcome.store;
        if let Some(error) = outcome.durability_error {
            return Err(error.into());
        }
        Ok(outcome.result)
    }

    pub async fn execution_loadout_rollback(
        &self,
        context: &ExecutionLoadoutContext,
        id: &str,
        expected_draft_revision: u64,
        revision: u64,
    ) -> Result<ExecutionLoadoutDraft, ExecutionLoadoutError> {
        let key = RecordKey::new(&context.principal, id)?;
        let fail = self.take_execution_loadout_persist_failure();
        let mut current = self.execution_loadouts.write().await;
        let outcome =
            ExecutionLoadoutStore::mutate(&self.path, fail, move |store| {
                let record = store
                    .records
                    .get_mut(&key)
                    .ok_or_else(|| ExecutionLoadoutError::NotFound { id: id.into() })?;
                if record.draft.draft_revision != expected_draft_revision {
                    return Err(ExecutionLoadoutError::StaleRevision {
                        expected: expected_draft_revision,
                        current: record.draft.draft_revision,
                        changed_fields: vec!["members".into()],
                    });
                }
                let prior = record.revisions.get(&revision).ok_or_else(|| {
                    ExecutionLoadoutError::Invalid {
                        field: "revision".into(),
                        message: "unknown immutable revision".into(),
                    }
                })?;
                record.draft.members = prior.members.clone();
                record.draft.draft_revision += 1;
                Ok(record.draft.clone())
            })?;
        *current = outcome.store;
        if let Some(error) = outcome.durability_error {
            return Err(error);
        }
        Ok(outcome.result)
    }

    fn resolve_execution_loadout(
        &self,
        context: &ExecutionLoadoutContext,
        draft: &ExecutionLoadoutDraft,
        runtime_identity: &str,
    ) -> Result<ExecutionLoadoutPreview, ToolError> {
        let published = self.execution_capabilities.load_full();
        let principal = context.principal.clone();
        let catalog = published.snapshots.get(&principal);
        let mut available = BTreeMap::new();
        if let Some(snapshot) = catalog {
            for member in &snapshot.members {
                available.insert(
                    (
                        member.provider.clone(),
                        member.family,
                        member.member_id.clone(),
                    ),
                    member.expected_revision.clone(),
                );
            }
        }
        let mut resolved = Vec::with_capacity(draft.members.len());
        let mut effective = Vec::new();
        let mut missing = Vec::new();
        let mut conflicts = Vec::new();
        for capability in &draft.members {
            let key = (
                capability.provider.clone(),
                capability.family,
                capability.member_id.clone(),
            );
            let provider_authorized = context
                .allowed_providers
                .as_ref()
                .is_none_or(|allowed| allowed.contains(&capability.provider))
                || capability.provider == "labby";
            let (status, current, diagnostic) = if !provider_authorized {
                (
                    ResolutionStatus::Unauthorized,
                    None,
                    Some("provider is outside the principal authorization snapshot".into()),
                )
            } else if let Some(current) = available.get(&key) {
                if current == &capability.expected_revision {
                    (ResolutionStatus::Effective, Some(current.clone()), None)
                } else {
                    (
                        ResolutionStatus::Stale,
                        Some(current.clone()),
                        Some("expected revision does not match live catalog".into()),
                    )
                }
            } else {
                (
                    ResolutionStatus::Missing,
                    None,
                    Some("provider-qualified member is not visible to this principal".into()),
                )
            };
            if status == ResolutionStatus::Effective {
                effective.push(capability.clone());
            } else {
                missing.push(capability.clone());
            }
            if let Some(value) = diagnostic.as_ref() {
                conflicts.push(format!("{}: {value}", capability.member_id));
            }
            resolved.push(ResolvedCapability {
                capability: capability.clone(),
                status,
                current_revision: current,
                diagnostic,
            });
        }
        Ok(ExecutionLoadoutPreview {
            loadout_id: draft.id.clone(),
            draft_revision: draft.draft_revision,
            active_revision: draft.effective_runtime_revision.unwrap_or(0),
            catalog_generation: catalog
                .map(|snapshot| snapshot.generation.clone())
                .unwrap_or_default(),
            principal: principal.as_str().to_owned(),
            runtime_identity: runtime_identity.into(),
            resolved,
            effective,
            missing,
            conflicts,
        })
    }
}
