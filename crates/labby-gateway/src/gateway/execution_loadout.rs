//! Revisioned, per-turn capability selections.
//!
//! Execution loadouts are not gateway route configuration. They select an
//! authorized subset of one immutable catalog snapshot for a caller/runtime.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use labby_runtime::error::ToolError;
use serde::{Deserialize, Serialize};
use sha2::Digest;

use self::validation::{normalize_members, validate_text};
use super::manager::GatewayManager;
use super::palette::{LauncherEntryView, PaletteCaller};

mod error_impl;
mod persistence;
#[cfg(test)]
mod tests;
mod validation;

const MAX_LOADOUTS: usize = 256;
const MAX_MEMBERS: usize = 512;
const MAX_TEXT_BYTES: usize = 256;

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

/// Host-owned canonical catalog source.
///
/// Implementations must read the real owning stores and apply the supplied
/// principal/upstream authorization. The gateway never invents catalog rows or
/// revisions.
pub trait ExecutionCapabilityCatalogProvider: Send + Sync {
    fn snapshot<'a>(
        &'a self,
        manager: &'a GatewayManager,
        principal: &'a str,
        tenant: &'a str,
        allowed_upstreams: Option<&'a BTreeMap<String, ()>>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<CapabilityCatalogSnapshot, ExecutionLoadoutError>>
                + Send
                + 'a,
        >,
    >;
}

#[derive(Debug, Clone, Default)]
pub(super) struct PublishedCapabilityCatalog {
    snapshots: HashMap<String, CapabilityCatalogSnapshot>,
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
    #[serde(default = "shared_principal")]
    pub owner_principal: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Record {
    pub(super) draft: ExecutionLoadoutDraft,
    pub(super) revisions: BTreeMap<u64, ExecutionLoadoutRevision>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(super) struct ExecutionLoadoutStore {
    pub(super) records: HashMap<String, Record>,
}

impl GatewayManager {
    /// Refresh one principal from the canonical production owner.
    pub async fn refresh_execution_capability_snapshot_for(
        &self,
        principal: &str,
        tenant: &str,
        allowed_upstreams: Option<&BTreeMap<String, ()>>,
    ) -> Result<(), ExecutionLoadoutError> {
        let provider = self.execution_capability_provider.as_ref().ok_or_else(|| {
            ExecutionLoadoutError::Storage {
                message: "canonical execution capability catalog is unavailable".into(),
            }
        })?;
        let snapshot = provider
            .snapshot(self, principal, tenant, allowed_upstreams)
            .await?;
        let current = self.execution_capabilities.load_full();
        let mut snapshots = current.snapshots.clone();
        snapshots.insert(principal.to_string(), snapshot);
        self.publish_execution_capability_snapshots(snapshots.into_values().collect())
    }

    /// Canonical upstream-owned non-tool rows for a host catalog provider.
    pub async fn canonical_upstream_execution_capabilities(
        &self,
        allowed_upstreams: Option<&BTreeMap<String, ()>>,
    ) -> Result<Vec<CapabilityRef>, ExecutionLoadoutError> {
        let allowed = |name: &str| allowed_upstreams.is_none_or(|set| set.contains_key(name));
        let mut members = Vec::new();
        if let Some(pool) = self.runtime.published_pool_snapshot().pool() {
            let prompts = pool.published_prompt_catalog().await.map_err(|_| {
                ExecutionLoadoutError::Storage {
                    message: "published prompt catalog is unavailable".into(),
                }
            })?;
            for route in prompts
                .routes()
                .iter()
                .filter(|route| allowed(&route.upstream_name))
            {
                members.push(CapabilityRef {
                    provider: route.upstream_name.to_string(),
                    family: CapabilityFamily::Prompt,
                    member_id: route.native_name.to_string(),
                    expected_revision: canonical_revision(&route.prompt)?,
                });
            }
            let resources = pool.published_resource_catalog().await.map_err(|_| {
                ExecutionLoadoutError::Storage {
                    message: "published resource catalog is unavailable".into(),
                }
            })?;
            for route in resources
                .routes()
                .iter()
                .filter(|route| allowed(&route.upstream_name))
            {
                members.push(CapabilityRef {
                    provider: route.upstream_name.to_string(),
                    family: CapabilityFamily::Resource,
                    member_id: route.native_uri.to_string(),
                    expected_revision: canonical_revision(&route.resource)?,
                });
            }
        }
        let config = self.current_config().await;
        for upstream in config
            .upstream
            .iter()
            .filter(|item| item.enabled && allowed(&item.name))
        {
            members.push(CapabilityRef {
                provider: "labby".into(),
                family: CapabilityFamily::McpServer,
                member_id: upstream.name.clone(),
                expected_revision: canonical_revision(upstream)?,
            });
        }
        Ok(members)
    }

    pub fn with_execution_capability_provider(
        mut self,
        provider: Arc<dyn ExecutionCapabilityCatalogProvider>,
    ) -> Self {
        self.execution_capability_provider = Some(provider);
        self
    }

    async fn refresh_execution_capability_snapshot(
        &self,
        caller: &PaletteCaller,
    ) -> Result<(), ToolError> {
        // Explicit constructors are retained for embedders and focused gateway
        // tests that publish snapshots directly. Production `from_config`
        // always injects the canonical host provider.
        if self.execution_capability_provider.is_none() {
            return Ok(());
        }
        let principal = caller_principal(caller);
        let tenant = caller.catalog_tenant.clone();
        let allowed = caller.allowed_upstreams().map(|names| {
            names
                .iter()
                .cloned()
                .map(|name| (name, ()))
                .collect::<BTreeMap<_, _>>()
        });
        self.refresh_execution_capability_snapshot_for(&principal, &tenant, allowed.as_ref())
            .await
            .map_err(Into::into)
    }

    /// Atomically publish complete principal-filtered snapshots prepared by the
    /// authoritative product host. A single swap prevents mixed generations.
    pub fn publish_execution_capability_snapshots(
        &self,
        snapshots: Vec<CapabilityCatalogSnapshot>,
    ) -> Result<(), ExecutionLoadoutError> {
        let mut published = PublishedCapabilityCatalog::default();
        for snapshot in snapshots {
            validate_text("generation", &snapshot.generation)?;
            validate_text("principal", &snapshot.principal)?;
            let principal = snapshot.principal.clone();
            let snapshot = CapabilityCatalogSnapshot {
                members: normalize_members(snapshot.members)?,
                ..snapshot
            };
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
        let store = self.execution_loadouts.read().await;
        let record = store
            .records
            .get(&record_key(principal, id))
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
        caller: &PaletteCaller,
    ) -> Vec<ExecutionLoadoutSummary> {
        let principal = caller_principal(caller);
        let store = self.execution_loadouts.read().await;
        let mut rows = store
            .records
            .values()
            .filter(|record| record.draft.owner_principal == principal)
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
        caller: &PaletteCaller,
        id: &str,
    ) -> Result<ExecutionLoadoutDraft, ExecutionLoadoutError> {
        self.execution_loadouts
            .read()
            .await
            .records
            .get(&record_key(&caller_principal(caller), id))
            .map(|r| r.draft.clone())
            .ok_or_else(|| ExecutionLoadoutError::NotFound { id: id.into() })
    }

    pub async fn execution_loadout_create(
        &self,
        caller: &PaletteCaller,
        input: ExecutionLoadoutCreate,
    ) -> Result<ExecutionLoadoutDraft, ExecutionLoadoutError> {
        let members = normalize_members(input.members)?;
        validate_text("id", &input.id)?;
        validate_text("name", &input.name)?;
        if let Some(value) = input.description.as_deref() {
            validate_text("description", value)?;
        }
        let mut store = self.execution_loadouts.write().await;
        let owner_principal = caller_principal(caller);
        let key = record_key(&owner_principal, &input.id);
        if store.records.contains_key(&key) {
            return Err(ExecutionLoadoutError::AlreadyExists { id: input.id });
        }
        if store.records.len() >= MAX_LOADOUTS {
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
        store.persist(&self.path)?;
        Ok(draft)
    }

    pub async fn execution_loadout_patch(
        &self,
        caller: &PaletteCaller,
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
        let mut store = self.execution_loadouts.write().await;
        let record = store
            .records
            .get_mut(&record_key(&caller_principal(caller), id))
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
        let draft = record.draft.clone();
        store.persist(&self.path)?;
        Ok(draft)
    }

    pub async fn execution_loadout_preview(
        &self,
        caller: &PaletteCaller,
        id: &str,
        runtime_identity: &str,
    ) -> Result<ExecutionLoadoutPreview, ToolError> {
        validate_text("runtimeIdentity", runtime_identity).map_err(ToolError::from)?;
        let draft = self
            .execution_loadout_get(caller, id)
            .await
            .map_err(ToolError::from)?;
        self.resolve_execution_loadout(caller, &draft, runtime_identity)
            .await
    }

    pub async fn execution_loadout_activate(
        &self,
        caller: &PaletteCaller,
        id: &str,
        expected_draft_revision: u64,
        runtime_identity: &str,
    ) -> Result<ExecutionLoadoutActivation, ToolError> {
        // Never trust a cached preview: capture and resolve a fresh atomic catalog snapshot.
        let draft = self
            .execution_loadout_get(caller, id)
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
        let preview = self
            .resolve_execution_loadout(caller, &draft, runtime_identity)
            .await?;
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
        let mut store = self.execution_loadouts.write().await;
        let record = store
            .records
            .get_mut(&record_key(&caller_principal(caller), id))
            .ok_or_else(|| ExecutionLoadoutError::NotFound { id: id.into() })?;
        if record.draft.draft_revision != expected_draft_revision {
            return Err(ExecutionLoadoutError::StaleRevision {
                expected: expected_draft_revision,
                current: record.draft.draft_revision,
                changed_fields: vec!["members".into()],
            }
            .into());
        }
        if record
            .draft
            .runtime_identity
            .as_deref()
            .is_some_and(|bound| bound != runtime_identity)
        {
            return Err(ExecutionLoadoutError::NotFound { id: id.into() }.into());
        }
        record.draft.runtime_identity = Some(runtime_identity.into());
        let revision_number = record.revisions.keys().next_back().copied().unwrap_or(0) + 1;
        let revision = ExecutionLoadoutRevision {
            loadout_id: id.into(),
            revision: revision_number,
            members: record.draft.members.clone(),
            catalog_generation: preview.catalog_generation.clone(),
        };
        record.revisions.insert(revision_number, revision.clone());
        record.draft.desired_active_revision = Some(revision_number);
        record.draft.effective_runtime_revision = Some(revision_number);
        record.draft.restart_required = false;
        let loadout = record.draft.clone();
        store.persist(&self.path).map_err(ToolError::from)?;
        Ok(ExecutionLoadoutActivation {
            loadout,
            revision,
            preview,
        })
    }

    pub async fn execution_loadout_rollback(
        &self,
        caller: &PaletteCaller,
        id: &str,
        expected_draft_revision: u64,
        revision: u64,
    ) -> Result<ExecutionLoadoutDraft, ExecutionLoadoutError> {
        let mut store = self.execution_loadouts.write().await;
        let record = store
            .records
            .get_mut(&record_key(&caller_principal(caller), id))
            .ok_or_else(|| ExecutionLoadoutError::NotFound { id: id.into() })?;
        if record.draft.draft_revision != expected_draft_revision {
            return Err(ExecutionLoadoutError::StaleRevision {
                expected: expected_draft_revision,
                current: record.draft.draft_revision,
                changed_fields: vec!["members".into()],
            });
        }
        let prior =
            record
                .revisions
                .get(&revision)
                .ok_or_else(|| ExecutionLoadoutError::Invalid {
                    field: "revision".into(),
                    message: "unknown immutable revision".into(),
                })?;
        record.draft.members = prior.members.clone();
        record.draft.draft_revision += 1;
        let draft = record.draft.clone();
        store.persist(&self.path)?;
        Ok(draft)
    }

    async fn resolve_execution_loadout(
        &self,
        caller: &PaletteCaller,
        draft: &ExecutionLoadoutDraft,
        runtime_identity: &str,
    ) -> Result<ExecutionLoadoutPreview, ToolError> {
        self.refresh_execution_capability_snapshot(caller).await?;
        let catalog = self.palette_catalog_snapshot(caller).await?;
        let published = self.execution_capabilities.load_full();
        let principal = caller_principal(caller);
        let non_tool_catalog = published.snapshots.get(&principal);
        let mut available = BTreeMap::new();
        for entry in catalog.entries {
            if let LauncherEntryView::McpTool(tool) = entry {
                available.insert(
                    (
                        tool.upstream.clone(),
                        CapabilityFamily::Tool,
                        tool.id.clone(),
                    ),
                    tool.contract_hash,
                );
            }
        }
        if let Some(snapshot) = non_tool_catalog {
            for member in &snapshot.members {
                if member.family != CapabilityFamily::Tool {
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
            let provider_authorized = caller
                .allowed_upstreams()
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
            catalog_generation: non_tool_catalog
                .map(|snapshot| format!("{}:{}", catalog.fingerprint, snapshot.generation))
                .unwrap_or(catalog.fingerprint),
            principal,
            runtime_identity: runtime_identity.into(),
            resolved,
            effective,
            missing,
            conflicts,
        })
    }
}

fn caller_principal(caller: &PaletteCaller) -> String {
    caller.catalog_principal.clone()
}

fn record_key(principal: &str, id: &str) -> String {
    format!("{principal}\0{id}")
}

fn shared_principal() -> String {
    "shared".into()
}

fn canonical_revision(value: &impl Serialize) -> Result<String, ExecutionLoadoutError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ExecutionLoadoutError::Storage {
        message: "canonical catalog row is not serializable".into(),
    })?;
    Ok(format!(
        "sha256:{}",
        hex::encode(sha2::Sha256::digest(bytes))
    ))
}
