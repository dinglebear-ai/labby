//! Durable, surface-neutral Skill Library authority.
//!
//! The Artifact store remains authoritative for immutable bytes and authored heads. This
//! module persists only Labby-local ownership and lifecycle state. Identity values are opaque
//! projections: callers must derive them from the canonical access-control records.

#![allow(
    dead_code,
    reason = "the sealed mutation primitive is wired to the canonical access adapter in a later bead"
)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::canonical_json;
use super::validation::{validate_id, validate_reference_id};
use super::{ArtifactError, ArtifactStore, invalid};

pub const LIBRARY_SCHEMA_VERSION: u8 = 1;
pub const OWNERSHIP_PROJECTION_SCHEMA_VERSION: u8 = 1;
const MAX_ID_BYTES: usize = 256;
const MAX_RECEIPTS: usize = 1024;
const MAX_AUDIT_INTENTS: usize = 1024;
const MAX_TIMESTAMP_BYTES: usize = 64;
pub(crate) const MAX_LIBRARY_STATE_BYTES: u64 = 4 * 1024 * 1024;

macro_rules! opaque_projection_id {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Construct from an accepted canonical access-control identifier projection.
            /// Construct an identifier from the canonical access-runtime projection.
            ///
            /// Product adapters must obtain this value from the accepted AccessRuntime record;
            /// client input, identity-provider claims, email, and display names are not canonical.
            pub fn from_canonical_projection(
                value: impl Into<String>,
            ) -> Result<Self, ArtifactError> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > MAX_ID_BYTES
                    || value.trim() != value
                    || value.chars().any(char::is_control)
                {
                    return Err(invalid($field, "invalid_canonical_projection"));
                }
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

opaque_projection_id!(LibraryTenantId, "tenant_id");
opaque_projection_id!(LibraryActorId, "actor_id");

/// Canonical local ownership projection. It deliberately contains no auth-provider facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryOwnership {
    pub schema_version: u8,
    pub tenant_id: LibraryTenantId,
    pub owner_id: LibraryActorId,
}

impl LibraryOwnership {
    pub fn canonical(tenant_id: LibraryTenantId, owner_id: LibraryActorId) -> Self {
        Self {
            schema_version: OWNERSHIP_PROJECTION_SCHEMA_VERSION,
            tenant_id,
            owner_id,
        }
    }

    fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != OWNERSHIP_PROJECTION_SCHEMA_VERSION {
            return Err(ArtifactError::UnsupportedSchema);
        }
        LibraryTenantId::from_canonical_projection(self.tenant_id.0.clone())?;
        LibraryActorId::from_canonical_projection(self.owner_id.0.clone())?;
        Ok(())
    }
}

/// Authorization decision already made by the canonical product access layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryGrant {
    Owner,
    Admin,
}

/// Canonical request actor plus its already-authorized mutation grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryAuthorization {
    schema_version: u8,
    tenant_id: LibraryTenantId,
    actor_id: LibraryActorId,
    grant: LibraryGrant,
}

impl LibraryAuthorization {
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "constructed by the canonical access adapter in a later bead"
        )
    )]
    /// Construct the sealed authority projection after canonical access authorization.
    ///
    /// This is a dependency-safe seam: `labby-runtime` cannot depend upward on the product's
    /// AccessRuntime. Only the canonical AccessRuntime adapter may call this constructor, and it
    /// must do so immediately after resolving current membership and authorizing this exact
    /// mutation. Transport claims and caller-supplied owner, tenant, role, or grant values must
    /// never reach this constructor.
    pub fn from_authorized_access_projection(
        tenant_id: LibraryTenantId,
        actor_id: LibraryActorId,
        grant: LibraryGrant,
    ) -> Self {
        Self {
            schema_version: OWNERSHIP_PROJECTION_SCHEMA_VERSION,
            tenant_id,
            actor_id,
            grant,
        }
    }

    fn validate_for(&self, ownership: &LibraryOwnership) -> Result<(), ArtifactError> {
        if self.schema_version != OWNERSHIP_PROJECTION_SCHEMA_VERSION {
            return Err(ArtifactError::UnsupportedSchema);
        }
        LibraryTenantId::from_canonical_projection(self.tenant_id.0.clone())?;
        LibraryActorId::from_canonical_projection(self.actor_id.0.clone())?;
        if self.tenant_id != ownership.tenant_id {
            return Err(ArtifactError::NotFound("library_record"));
        }
        if self.grant == LibraryGrant::Owner && self.actor_id != ownership.owner_id {
            return Err(ArtifactError::NotFound("library_record"));
        }
        Ok(())
    }
}

/// Bounded canonical instant used by durable library metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LibraryTimestamp(String);

impl LibraryTimestamp {
    pub fn parse(value: impl Into<String>) -> Result<Self, ArtifactError> {
        let value = value.into();
        let parsed = value
            .parse::<jiff::Timestamp>()
            .map_err(|_| invalid("timestamp", "invalid_timestamp"))?;
        let canonical = parsed.to_string();
        if canonical.len() > MAX_TIMESTAMP_BYTES {
            return Err(invalid("timestamp", "invalid_timestamp"));
        }
        Ok(Self(canonical))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillVisibility {
    Private,
    Tenant,
}

/// One Labby-local Skill record. Revision bytes remain owned by [`ArtifactStore`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillLibraryRecord {
    pub artifact_id: String,
    pub name: String,
    pub ownership: LibraryOwnership,
    pub visibility: SkillVisibility,
    pub archived: bool,
    pub active_revision_id: Option<String>,
    pub created_at: LibraryTimestamp,
    pub updated_at: LibraryTimestamp,
}

impl SkillLibraryRecord {
    fn validate_metadata(&self) -> Result<(), ArtifactError> {
        validate_id(&self.artifact_id, "artifact_id")?;
        validate_skill_name(&self.name)?;
        self.ownership.validate()?;
        if LibraryTimestamp::parse(self.created_at.0.clone())? != self.created_at
            || LibraryTimestamp::parse(self.updated_at.0.clone())? != self.updated_at
        {
            return Err(invalid("timestamp", "not_canonical"));
        }
        if self.archived && self.active_revision_id.is_some() {
            return Err(ArtifactError::LibraryCorrupt("archived_active_record"));
        }
        if let Some(revision) = &self.active_revision_id {
            validate_reference_id(revision, "active_revision_id")?;
        }
        Ok(())
    }

    fn validate(&self, store: &ArtifactStore) -> Result<(), ArtifactError> {
        self.validate_metadata()?;
        let artifact = store.get(&self.artifact_id)?;
        if artifact.descriptor.kind != "skill" {
            return Err(ArtifactError::Conflict("library_artifact_not_skill"));
        }
        if artifact.descriptor.name != self.name {
            return Err(ArtifactError::Conflict("library_name_mismatch"));
        }
        if let Some(revision) = &self.active_revision_id {
            store.revision(&self.artifact_id, revision)?;
        }
        Ok(())
    }
}

/// Security-relevant request binding retained with a terminal receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryIdempotency {
    pub key: String,
    pub request_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryAuditIntent {
    pub sequence: u64,
    pub action: String,
    pub tenant_id: LibraryTenantId,
    pub actor_id: LibraryActorId,
    pub artifact_id: String,
    pub request_digest: String,
    pub committed_at: LibraryTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryReceipt {
    pub sequence: u64,
    pub scope_digest: String,
    pub tenant_id: LibraryTenantId,
    pub actor_id: LibraryActorId,
    pub action: String,
    pub artifact_id: String,
    pub idempotency_key: String,
    pub request_digest: String,
    pub committed_version: u64,
}

/// Complete committed library generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibrarySnapshot {
    pub schema_version: u8,
    pub version: u64,
    pub active_generation_digest: String,
    pub records: BTreeMap<String, SkillLibraryRecord>,
    pub active_names: BTreeMap<String, String>,
    pub receipts: BTreeMap<String, LibraryReceipt>,
    pub audit_intents: Vec<LibraryAuditIntent>,
}

impl Default for LibrarySnapshot {
    fn default() -> Self {
        let mut state = Self {
            schema_version: LIBRARY_SCHEMA_VERSION,
            version: 0,
            active_generation_digest: String::new(),
            records: BTreeMap::new(),
            active_names: BTreeMap::new(),
            receipts: BTreeMap::new(),
            audit_intents: Vec::new(),
        };
        state.active_generation_digest = state
            .compute_digest()
            .expect("empty generation is serializable");
        state
    }
}

impl LibrarySnapshot {
    fn compute_digest(&self) -> Result<String, ArtifactError> {
        #[derive(Serialize)]
        struct Generation<'a> {
            schema_version: u8,
            version: u64,
            records: &'a BTreeMap<String, SkillLibraryRecord>,
            active_names: &'a BTreeMap<String, String>,
            receipts: &'a BTreeMap<String, LibraryReceipt>,
            audit_intents: &'a [LibraryAuditIntent],
        }
        canonical_json::digest(&Generation {
            schema_version: self.schema_version,
            version: self.version,
            records: &self.records,
            active_names: &self.active_names,
            receipts: &self.receipts,
            audit_intents: &self.audit_intents,
        })
    }

    pub(crate) fn validate_metadata(&self) -> Result<(), ArtifactError> {
        if self.schema_version != LIBRARY_SCHEMA_VERSION {
            return Err(ArtifactError::UnsupportedSchema);
        }
        if self.compute_digest()? != self.active_generation_digest {
            return Err(ArtifactError::LibraryCorrupt("generation_digest_mismatch"));
        }
        for (id, record) in &self.records {
            if id != &record.artifact_id {
                return Err(ArtifactError::LibraryCorrupt("record_key_mismatch"));
            }
            record.validate_metadata()?;
        }
        let mut expected = BTreeMap::new();
        for record in self
            .records
            .values()
            .filter(|record| record.active_revision_id.is_some())
        {
            if expected
                .insert(record.name.clone(), record.artifact_id.clone())
                .is_some()
            {
                return Err(ArtifactError::LibraryCorrupt("duplicate_active_name"));
            }
        }
        if expected != self.active_names {
            return Err(ArtifactError::LibraryCorrupt("active_index_mismatch"));
        }
        if self.receipts.len() > MAX_RECEIPTS {
            return Err(ArtifactError::LibraryCorrupt("receipt_limit"));
        }
        if self.audit_intents.len() > MAX_AUDIT_INTENTS {
            return Err(ArtifactError::LibraryCorrupt("audit_limit"));
        }
        let mut receipt_sequences = std::collections::BTreeSet::new();
        for (key, receipt) in &self.receipts {
            validate_digest(key).map_err(|_| ArtifactError::LibraryCorrupt("receipt_key"))?;
            if key != &receipt.scope_digest
                || receipt.sequence == 0
                || !receipt_sequences.insert(receipt.sequence)
                || receipt.committed_version != receipt.sequence
                || receipt.committed_version > self.version
            {
                return Err(ArtifactError::LibraryCorrupt("invalid_receipt"));
            }
            validate_digest(&receipt.request_digest)
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_receipt_digest"))?;
            LibraryTenantId::from_canonical_projection(receipt.tenant_id.0.clone())
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_receipt_actor"))?;
            LibraryActorId::from_canonical_projection(receipt.actor_id.0.clone())
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_receipt_actor"))?;
            validate_action(&receipt.action)
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_receipt_action"))?;
            validate_id(&receipt.artifact_id, "artifact_id")
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_receipt_reference"))?;
            if !self.records.contains_key(&receipt.artifact_id) {
                return Err(ArtifactError::LibraryCorrupt("invalid_receipt_reference"));
            }
            if self.records[&receipt.artifact_id].ownership.tenant_id != receipt.tenant_id {
                return Err(ArtifactError::LibraryCorrupt("receipt_tenant_mismatch"));
            }
            if receipt.idempotency_key.is_empty() || receipt.idempotency_key.len() > 256 {
                return Err(ArtifactError::LibraryCorrupt("invalid_receipt_key"));
            }
            let expected_scope = receipt_scope_digest(
                &receipt.tenant_id,
                &receipt.actor_id,
                &receipt.action,
                &receipt.artifact_id,
                &receipt.idempotency_key,
            )?;
            if expected_scope != *key {
                return Err(ArtifactError::LibraryCorrupt("receipt_scope_mismatch"));
            }
        }
        let mut previous = 0;
        for audit in &self.audit_intents {
            if audit.sequence == 0 || audit.sequence <= previous || audit.sequence > self.version {
                return Err(ArtifactError::LibraryCorrupt("invalid_audit_sequence"));
            }
            previous = audit.sequence;
            validate_id(&audit.artifact_id, "artifact_id")
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_audit_reference"))?;
            if !self.records.contains_key(&audit.artifact_id) {
                return Err(ArtifactError::LibraryCorrupt("invalid_audit_reference"));
            }
            validate_digest(&audit.request_digest)
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_audit_digest"))?;
            let parsed_timestamp = LibraryTimestamp::parse(audit.committed_at.0.clone())
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_audit_timestamp"))?;
            if parsed_timestamp != audit.committed_at {
                return Err(ArtifactError::LibraryCorrupt("invalid_audit_timestamp"));
            }
            LibraryTenantId::from_canonical_projection(audit.tenant_id.0.clone())
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_audit_actor"))?;
            LibraryActorId::from_canonical_projection(audit.actor_id.0.clone())
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_audit_actor"))?;
            validate_action(&audit.action)
                .map_err(|_| ArtifactError::LibraryCorrupt("invalid_audit_action"))?;
            if self.records[&audit.artifact_id].ownership.tenant_id != audit.tenant_id {
                return Err(ArtifactError::LibraryCorrupt("audit_tenant_mismatch"));
            }
        }
        for receipt in self.receipts.values() {
            if !self.audit_intents.iter().any(|audit| {
                audit.sequence == receipt.sequence
                    && audit.tenant_id == receipt.tenant_id
                    && audit.actor_id == receipt.actor_id
                    && audit.action == receipt.action
                    && audit.artifact_id == receipt.artifact_id
                    && audit.request_digest == receipt.request_digest
            }) {
                return Err(ArtifactError::LibraryCorrupt("receipt_audit_mismatch"));
            }
        }
        Ok(())
    }

    pub(crate) fn validate(&self, store: &ArtifactStore) -> Result<(), ArtifactError> {
        self.validate_metadata()?;
        for record in self.records.values() {
            record.validate(store)?;
        }
        Ok(())
    }

    /// Tenant-qualified discoverable records. Archived records are never returned.
    pub fn list_for_tenant(&self, tenant: &LibraryTenantId) -> Vec<&SkillLibraryRecord> {
        self.records
            .values()
            .filter(|record| !record.archived && &record.ownership.tenant_id == tenant)
            .collect()
    }

    pub fn get_for_tenant(
        &self,
        tenant: &LibraryTenantId,
        artifact_id: &str,
    ) -> Option<&SkillLibraryRecord> {
        self.records
            .get(artifact_id)
            .filter(|record| !record.archived && &record.ownership.tenant_id == tenant)
    }
}

/// One durable compare-and-swap mutation.
#[derive(Debug, Clone)]
pub enum LibraryMutation {
    Create {
        record: SkillLibraryRecord,
    },
    SetVisibility {
        artifact_id: String,
        visibility: SkillVisibility,
        updated_at: LibraryTimestamp,
    },
    Activate {
        artifact_id: String,
        revision_id: String,
        updated_at: LibraryTimestamp,
    },
    Deactivate {
        artifact_id: String,
        updated_at: LibraryTimestamp,
    },
    Archive {
        artifact_id: String,
        updated_at: LibraryTimestamp,
    },
}

impl LibraryMutation {
    fn action(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::SetVisibility { .. } => "set_visibility",
            Self::Activate { .. } => "activate",
            Self::Deactivate { .. } => "deactivate",
            Self::Archive { .. } => "archive",
        }
    }
    fn artifact_id(&self) -> &str {
        match self {
            Self::Create { record } => &record.artifact_id,
            Self::SetVisibility { artifact_id, .. }
            | Self::Activate { artifact_id, .. }
            | Self::Deactivate { artifact_id, .. }
            | Self::Archive { artifact_id, .. } => artifact_id,
        }
    }
}

impl ArtifactStore {
    /// Load and fully verify the committed library generation.
    pub fn library_snapshot(&self) -> Result<LibrarySnapshot, ArtifactError> {
        self.read_library_snapshot()
    }

    /// Commit ownership, lifecycle, active-name index, receipt, and audit intent atomically.
    /// Commit one mutation authorized by the canonical AccessRuntime adapter.
    ///
    /// `authorization` must be created with [`LibraryAuthorization::from_authorized_access_projection`]
    /// immediately after final-boundary authorization. This storage layer verifies projection
    /// consistency but deliberately does not recreate product membership policy.
    pub fn mutate_library(
        &self,
        authorization: &LibraryAuthorization,
        target_ownership: &LibraryOwnership,
        expected_version: u64,
        idempotency: LibraryIdempotency,
        mutation: LibraryMutation,
        committed_at: LibraryTimestamp,
    ) -> Result<LibraryReceipt, ArtifactError> {
        target_ownership.validate()?;
        authorization.validate_for(target_ownership)?;
        validate_id(mutation.artifact_id(), "artifact_id")?;
        validate_idempotency(&idempotency)?;
        let scope_digest = receipt_scope_digest(
            &authorization.tenant_id,
            &authorization.actor_id,
            mutation.action(),
            mutation.artifact_id(),
            &idempotency.key,
        )?;
        let action = mutation.action().to_string();
        let target_artifact_id = mutation.artifact_id().to_string();
        // Artifact verification is deliberately outside the library-wide lock. Revisions are
        // immutable, so the verified facts remain valid while the short CAS commit runs.
        let prevalidated = self.read_library_snapshot()?;
        prevalidate_mutation(self, &mutation)?;
        let _lock = self.library_lock()?;
        let current = self.read_library_snapshot_unvalidated()?;
        // A matching receipt is authoritative only inside a completely valid committed metadata
        // generation. Keep Artifact byte/revision verification outside this lock, but never let a
        // forged or torn receipt bypass receipt/audit/index integrity checks.
        current.validate_metadata()?;
        // Resolve a concurrently committed identical request before comparing the stale
        // pre-lock generation. This gives duplicate contenders the winner's terminal receipt.
        if let Some(receipt) = current.receipts.get(&scope_digest) {
            if receipt.request_digest != idempotency.request_digest {
                return Err(ArtifactError::Conflict("idempotency_binding_changed"));
            }
            validate_replay_receipt(
                receipt,
                &scope_digest,
                authorization,
                &action,
                &target_artifact_id,
                &idempotency.key,
                current.version,
            )?;
            return Ok(receipt.clone());
        }
        if current != prevalidated {
            return Err(ArtifactError::Conflict("library_version_changed"));
        }
        let mut state = prevalidated;
        if state.version != expected_version {
            return Err(ArtifactError::Conflict("library_version_changed"));
        }
        apply_mutation(&mut state, authorization, target_ownership, mutation)?;
        state.version = state
            .version
            .checked_add(1)
            .ok_or(ArtifactError::Conflict("library_version_exhausted"))?;
        let receipt = LibraryReceipt {
            sequence: state.version,
            scope_digest: scope_digest.clone(),
            tenant_id: authorization.tenant_id.clone(),
            actor_id: authorization.actor_id.clone(),
            action: action.clone(),
            artifact_id: target_artifact_id.clone(),
            idempotency_key: idempotency.key,
            request_digest: idempotency.request_digest.clone(),
            committed_version: state.version,
        };
        state.receipts.insert(scope_digest, receipt.clone());
        state.audit_intents.push(LibraryAuditIntent {
            sequence: state.version,
            action,
            tenant_id: authorization.tenant_id.clone(),
            actor_id: authorization.actor_id.clone(),
            artifact_id: target_artifact_id,
            request_digest: idempotency.request_digest,
            committed_at,
        });
        enforce_retention(&mut state);
        state.active_generation_digest = state.compute_digest()?;
        state.validate_metadata()?;
        let serialized = canonical_json::to_canonical_vec(&state)?;
        if serialized.len() as u64 > MAX_LIBRARY_STATE_BYTES {
            return Err(ArtifactError::LimitExceeded {
                what: "library_state_bytes",
                limit: MAX_LIBRARY_STATE_BYTES,
            });
        }
        self.persist_library_snapshot(&state)?;
        Ok(receipt)
    }
}

fn validate_replay_receipt(
    receipt: &LibraryReceipt,
    scope_digest: &str,
    authorization: &LibraryAuthorization,
    action: &str,
    artifact_id: &str,
    idempotency_key: &str,
    current_version: u64,
) -> Result<(), ArtifactError> {
    if receipt.scope_digest != scope_digest
        || receipt.tenant_id != authorization.tenant_id
        || receipt.actor_id != authorization.actor_id
        || receipt.action != action
        || receipt.artifact_id != artifact_id
        || receipt.idempotency_key != idempotency_key
        || receipt.sequence == 0
        || receipt.committed_version != receipt.sequence
        || receipt.committed_version > current_version
    {
        return Err(ArtifactError::LibraryCorrupt("invalid_replay_receipt"));
    }
    Ok(())
}

fn enforce_retention(state: &mut LibrarySnapshot) {
    while state.receipts.len() > MAX_RECEIPTS {
        if let Some(key) = state
            .receipts
            .iter()
            .min_by_key(|(_, receipt)| receipt.sequence)
            .map(|(key, _)| key.clone())
        {
            state.receipts.remove(&key);
        }
    }
    if state.audit_intents.len() > MAX_AUDIT_INTENTS {
        state
            .audit_intents
            .drain(..state.audit_intents.len() - MAX_AUDIT_INTENTS);
    }
}

// Helpers avoid persisting arbitrary caller data beyond bounded canonical fields.
fn apply_mutation(
    state: &mut LibrarySnapshot,
    authorization: &LibraryAuthorization,
    target_ownership: &LibraryOwnership,
    mutation: LibraryMutation,
) -> Result<(), ArtifactError> {
    match mutation {
        LibraryMutation::Create { record } => {
            if &record.ownership != target_ownership {
                return Err(ArtifactError::Conflict("ownership_mismatch"));
            }
            record.validate_metadata()?;
            if state.records.contains_key(&record.artifact_id) {
                return Err(ArtifactError::Conflict("library_record_exists"));
            }
            state.records.insert(record.artifact_id.clone(), record);
        }
        LibraryMutation::SetVisibility {
            artifact_id,
            visibility,
            updated_at,
        } => {
            let record = authorized_record(state, authorization, target_ownership, &artifact_id)?;
            record.visibility = visibility;
            record.updated_at = updated_at;
        }
        LibraryMutation::Activate {
            artifact_id,
            revision_id,
            updated_at,
        } => {
            let (name, archived) = {
                let record =
                    authorized_record(state, authorization, target_ownership, &artifact_id)?;
                (record.name.clone(), record.archived)
            };
            if archived {
                return Err(ArtifactError::Conflict("archived_skill"));
            }
            if state
                .active_names
                .get(&name)
                .is_some_and(|owner| owner != &artifact_id)
            {
                return Err(ArtifactError::Conflict("active_name_taken"));
            }
            let record = authorized_record(state, authorization, target_ownership, &artifact_id)?;
            record.active_revision_id = Some(revision_id);
            record.updated_at = updated_at;
            state.active_names.insert(name, artifact_id);
        }
        LibraryMutation::Deactivate {
            artifact_id,
            updated_at,
        } => {
            let name = authorized_record(state, authorization, target_ownership, &artifact_id)?
                .name
                .clone();
            state.active_names.remove(&name);
            let record = authorized_record(state, authorization, target_ownership, &artifact_id)?;
            record.active_revision_id = None;
            record.updated_at = updated_at;
        }
        LibraryMutation::Archive {
            artifact_id,
            updated_at,
        } => {
            let name = authorized_record(state, authorization, target_ownership, &artifact_id)?
                .name
                .clone();
            state.active_names.remove(&name);
            let record = authorized_record(state, authorization, target_ownership, &artifact_id)?;
            record.active_revision_id = None;
            record.archived = true;
            record.updated_at = updated_at;
        }
    }
    Ok(())
}

fn prevalidate_mutation(
    store: &ArtifactStore,
    mutation: &LibraryMutation,
) -> Result<(), ArtifactError> {
    match mutation {
        LibraryMutation::Create { record } => record.validate(store),
        LibraryMutation::Activate {
            artifact_id,
            revision_id,
            ..
        } => store.revision(artifact_id, revision_id).map(|_| ()),
        LibraryMutation::SetVisibility { .. }
        | LibraryMutation::Deactivate { .. }
        | LibraryMutation::Archive { .. } => Ok(()),
    }
}

fn authorized_record<'a>(
    state: &'a mut LibrarySnapshot,
    authorization: &LibraryAuthorization,
    target_ownership: &LibraryOwnership,
    artifact_id: &str,
) -> Result<&'a mut SkillLibraryRecord, ArtifactError> {
    let record = state
        .records
        .get_mut(artifact_id)
        .ok_or(ArtifactError::NotFound("library_record"))?;
    if &record.ownership != target_ownership {
        return Err(ArtifactError::NotFound("library_record"));
    }
    authorization.validate_for(&record.ownership)?;
    Ok(record)
}

fn validate_skill_name(name: &str) -> Result<(), ArtifactError> {
    if name.is_empty()
        || name.len() > 128
        || name.trim() != name
        || name
            .chars()
            .any(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
    {
        return Err(invalid("name", "invalid_skill_name"));
    }
    Ok(())
}
fn validate_idempotency(value: &LibraryIdempotency) -> Result<(), ArtifactError> {
    if value.key.is_empty() || value.key.len() > 256 {
        return Err(invalid("idempotency_key", "invalid"));
    }
    validate_digest(&value.request_digest)?;
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), ArtifactError> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid("digest", "invalid"));
    }
    Ok(())
}

fn validate_action(action: &str) -> Result<(), ArtifactError> {
    if !matches!(
        action,
        "create" | "set_visibility" | "activate" | "deactivate" | "archive"
    ) {
        return Err(invalid("action", "invalid"));
    }
    Ok(())
}

fn receipt_scope_digest(
    tenant_id: &LibraryTenantId,
    actor_id: &LibraryActorId,
    action: &str,
    artifact_id: &str,
    idempotency_key: &str,
) -> Result<String, ArtifactError> {
    canonical_json::digest(&(tenant_id, actor_id, action, artifact_id, idempotency_key))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use tempfile::{TempDir, tempdir};

    use super::*;
    use crate::artifacts::ArtifactImportRequest;
    use crate::artifacts::store::LibraryPersistFault;

    fn ownership(tenant: &str, owner: &str) -> LibraryOwnership {
        LibraryOwnership::canonical(
            LibraryTenantId::from_canonical_projection(tenant).unwrap(),
            LibraryActorId::from_canonical_projection(owner).unwrap(),
        )
    }

    fn owner_auth(owner: &LibraryOwnership) -> LibraryAuthorization {
        LibraryAuthorization::from_authorized_access_projection(
            owner.tenant_id.clone(),
            owner.owner_id.clone(),
            LibraryGrant::Owner,
        )
    }

    fn ts(value: &str) -> LibraryTimestamp {
        LibraryTimestamp::parse(value).unwrap()
    }

    fn idem(key: &str) -> LibraryIdempotency {
        LibraryIdempotency {
            key: key.to_string(),
            request_digest: canonical_json::digest(&key).unwrap(),
        }
    }

    fn add_skill(
        store: &ArtifactStore,
        source: &TempDir,
        namespace: &str,
        name: &str,
        owner: &LibraryOwnership,
        expected: u64,
    ) -> (String, String) {
        std::fs::write(
            source.path().join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Test\n---\nBody\n"),
        )
        .unwrap();
        let record = store
            .import_local(
                ArtifactImportRequest::new("skill", namespace, name),
                source.path(),
            )
            .unwrap();
        let artifact_id = record.descriptor.id.clone();
        let revision_id = record.current_revision_id.clone();
        store
            .mutate_library(
                &owner_auth(owner),
                owner,
                expected,
                idem(&format!("create-{namespace}")),
                LibraryMutation::Create {
                    record: SkillLibraryRecord {
                        artifact_id: artifact_id.clone(),
                        name: name.to_string(),
                        ownership: owner.clone(),
                        visibility: SkillVisibility::Private,
                        archived: false,
                        active_revision_id: None,
                        created_at: ts("2026-08-26T00:00:00Z"),
                        updated_at: ts("2026-08-26T00:00:00Z"),
                    },
                },
                ts("2026-08-26T00:00:00Z"),
            )
            .unwrap();
        (artifact_id, revision_id)
    }

    #[test]
    fn metadata_round_trip_restart_tenant_isolation_and_archive_without_delete() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner_a = ownership("org-a", "alice");
        let (artifact, revision) = add_skill(&store, &source, "team-a", "demo", &owner_a, 0);
        let activated = store
            .mutate_library(
                &owner_auth(&owner_a),
                &owner_a,
                1,
                idem("activate"),
                LibraryMutation::Activate {
                    artifact_id: artifact.clone(),
                    revision_id: revision.clone(),
                    updated_at: ts("2026-08-26T00:01:00Z"),
                },
                ts("2026-08-26T00:01:00Z"),
            )
            .unwrap();
        assert_eq!(activated.committed_version, 2);

        let reopened = ArtifactStore::new(&root).unwrap();
        let snapshot = reopened.library_snapshot().unwrap();
        assert_eq!(
            snapshot
                .get_for_tenant(&owner_a.tenant_id, &artifact)
                .unwrap()
                .active_revision_id
                .as_deref(),
            Some(revision.as_str())
        );
        assert!(
            snapshot
                .get_for_tenant(&ownership("org-b", "bob").tenant_id, &artifact)
                .is_none()
        );

        reopened
            .mutate_library(
                &owner_auth(&owner_a),
                &owner_a,
                2,
                idem("archive"),
                LibraryMutation::Archive {
                    artifact_id: artifact.clone(),
                    updated_at: ts("2026-08-26T00:02:00Z"),
                },
                ts("2026-08-26T00:02:00Z"),
            )
            .unwrap();
        let snapshot = reopened.library_snapshot().unwrap();
        assert!(
            snapshot
                .get_for_tenant(&owner_a.tenant_id, &artifact)
                .is_none()
        );
        assert!(snapshot.records[&artifact].archived);
        reopened
            .revision(&artifact, &revision)
            .expect("archive must retain immutable bytes");
    }

    #[test]
    fn stale_cas_and_changed_idempotency_binding_fail() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let error = store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                0,
                idem("stale"),
                LibraryMutation::Deactivate {
                    artifact_id: artifact.clone(),
                    updated_at: ts("2026-08-26T00:03:00Z"),
                },
                ts("2026-08-26T00:03:00Z"),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::Conflict("library_version_changed")
        ));

        let mutation = LibraryMutation::SetVisibility {
            artifact_id: artifact,
            visibility: SkillVisibility::Tenant,
            updated_at: ts("2026-08-26T00:04:00Z"),
        };
        store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                1,
                idem("same"),
                mutation.clone(),
                ts("2026-08-26T00:04:00Z"),
            )
            .unwrap();
        let mut changed = idem("same");
        changed.request_digest = canonical_json::digest(&"different").unwrap();
        let error = store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                2,
                changed,
                mutation,
                ts("2026-08-26T00:04:00Z"),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::Conflict("idempotency_binding_changed")
        ));
    }

    #[test]
    fn concurrent_identical_idempotency_requests_return_one_equal_receipt() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let barrier = Arc::new(Barrier::new(3));
        let handles = (0..2)
            .map(|_| {
                let root = root.clone();
                let owner = owner.clone();
                let artifact = artifact.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let store = ArtifactStore::new(root).unwrap();
                    barrier.wait();
                    store.mutate_library(
                        &owner_auth(&owner),
                        &owner,
                        1,
                        idem("identical-concurrent-request"),
                        LibraryMutation::SetVisibility {
                            artifact_id: artifact,
                            visibility: SkillVisibility::Tenant,
                            updated_at: ts("2026-08-26T00:04:30Z"),
                        },
                        ts("2026-08-26T00:04:30Z"),
                    )
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let receipts = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(receipts[0], receipts[1]);
        assert_eq!(receipts[0].committed_version, 2);
        let snapshot = ArtifactStore::new(root)
            .unwrap()
            .library_snapshot()
            .unwrap();
        assert_eq!(snapshot.version, 2);
        assert_eq!(snapshot.receipts.len(), 2);
        assert_eq!(snapshot.audit_intents.len(), 2);
    }

    #[test]
    fn concurrent_same_name_activation_has_exactly_one_winner() {
        let data = tempdir().unwrap();
        let source_a = tempdir().unwrap();
        let source_b = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (a, rev_a) = add_skill(&store, &source_a, "team-a", "shared", &owner, 0);
        let (b, rev_b) = add_skill(&store, &source_b, "team-b", "shared", &owner, 1);
        let barrier = Arc::new(Barrier::new(3));
        let handles = [(a, rev_a, "a"), (b, rev_b, "b")]
            .into_iter()
            .map(|(artifact_id, revision_id, key)| {
                let root = root.clone();
                let owner = owner.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let store = ArtifactStore::new(root).unwrap();
                    barrier.wait();
                    store.mutate_library(
                        &owner_auth(&owner),
                        &owner,
                        2,
                        idem(key),
                        LibraryMutation::Activate {
                            artifact_id,
                            revision_id,
                            updated_at: ts("2026-08-26T00:05:00Z"),
                        },
                        ts("2026-08-26T00:05:00Z"),
                    )
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            ArtifactStore::new(&root)
                .unwrap()
                .library_snapshot()
                .unwrap()
                .active_names
                .len(),
            1
        );
    }

    #[test]
    fn corrupt_or_truncated_generation_fails_closed() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        add_skill(
            &store,
            &source,
            "team-a",
            "demo",
            &ownership("org-a", "alice"),
            0,
        );
        std::fs::write(root.join("library/state.json"), b"{\"schemaVersion\":1").unwrap();
        assert!(matches!(
            ArtifactStore::new(&root).unwrap().library_snapshot(),
            Err(ArtifactError::LibraryCorrupt("invalid_json"))
        ));
    }

    fn write_snapshot(root: &std::path::Path, snapshot: &mut LibrarySnapshot) {
        snapshot.active_generation_digest = snapshot.compute_digest().unwrap();
        std::fs::write(
            root.join("library/state.json"),
            canonical_json::to_canonical_vec(snapshot).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn admin_mutation_attributes_receipt_and_audit_to_actual_actor() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let admin = LibraryAuthorization::from_authorized_access_projection(
            owner.tenant_id.clone(),
            LibraryActorId::from_canonical_projection("bob").unwrap(),
            LibraryGrant::Admin,
        );
        let receipt = store
            .mutate_library(
                &admin,
                &owner,
                1,
                idem("admin-change"),
                LibraryMutation::SetVisibility {
                    artifact_id: artifact,
                    visibility: SkillVisibility::Tenant,
                    updated_at: ts("2026-08-26T01:00:00Z"),
                },
                ts("2026-08-26T01:00:00Z"),
            )
            .unwrap();
        assert_eq!(receipt.actor_id.as_str(), "bob");
        let snapshot = store.library_snapshot().unwrap();
        assert_eq!(
            snapshot.audit_intents.last().unwrap().actor_id.as_str(),
            "bob"
        );
        assert_eq!(
            snapshot
                .records
                .values()
                .next()
                .unwrap()
                .ownership
                .owner_id
                .as_str(),
            "alice"
        );
    }

    #[test]
    fn invalid_oversized_timestamp_preserves_last_good_generation() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let error = store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                1,
                idem("oversized-time"),
                LibraryMutation::SetVisibility {
                    artifact_id: artifact,
                    visibility: SkillVisibility::Tenant,
                    updated_at: LibraryTimestamp("x".repeat(MAX_TIMESTAMP_BYTES + 1)),
                },
                ts("2026-08-26T01:30:00Z"),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::InvalidField {
                field: "timestamp",
                ..
            }
        ));
        let reopened = ArtifactStore::new(&root)
            .unwrap()
            .library_snapshot()
            .unwrap();
        assert_eq!(reopened.version, 1);
        assert_eq!(reopened.receipts.len(), 1);
    }

    #[test]
    fn forged_receipt_and_audit_fail_even_with_recomputed_generation_digest() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        add_skill(
            &store,
            &source,
            "team-a",
            "demo",
            &ownership("org-a", "alice"),
            0,
        );

        let mut forged_receipt = store.library_snapshot().unwrap();
        forged_receipt
            .receipts
            .values_mut()
            .next()
            .unwrap()
            .actor_id = LibraryActorId::from_canonical_projection("mallory").unwrap();
        write_snapshot(&root, &mut forged_receipt);
        assert!(matches!(
            store.library_snapshot(),
            Err(ArtifactError::LibraryCorrupt("receipt_scope_mismatch"))
        ));

        let mut forged_audit = forged_receipt;
        let receipt = forged_audit.receipts.values_mut().next().unwrap();
        receipt.actor_id = LibraryActorId::from_canonical_projection("alice").unwrap();
        forged_audit.audit_intents[0].artifact_id = "art_missing".into();
        write_snapshot(&root, &mut forged_audit);
        assert!(matches!(
            store.library_snapshot(),
            Err(ArtifactError::LibraryCorrupt("invalid_audit_reference"))
        ));
    }

    #[test]
    fn matching_replay_receipt_cannot_bypass_missing_audit_validation() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let record = store.library_snapshot().unwrap().records[&artifact].clone();

        let mut forged = store.library_snapshot().unwrap();
        forged.audit_intents.clear();
        write_snapshot(&root, &mut forged);

        let error = store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                0,
                idem("create-team-a"),
                LibraryMutation::Create { record },
                ts("2026-08-26T00:00:00Z"),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::LibraryCorrupt("receipt_audit_mismatch")
        ));
    }

    #[test]
    fn persistence_faults_never_reopen_a_partial_or_older_active_generation() {
        for fault in [
            LibraryPersistFault::Write,
            LibraryPersistFault::FileSync,
            LibraryPersistFault::Commit,
            LibraryPersistFault::DirectorySync,
            LibraryPersistFault::Enospc,
        ] {
            let data = tempdir().unwrap();
            let source = tempdir().unwrap();
            let root = data.path().join("store");
            let store = ArtifactStore::new(&root).unwrap();
            let owner = ownership("org-a", "alice");
            let (artifact, revision) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
            store.inject_library_persist_fault(fault);
            let error = store
                .mutate_library(
                    &owner_auth(&owner),
                    &owner,
                    1,
                    idem("faulted-activate"),
                    LibraryMutation::Activate {
                        artifact_id: artifact.clone(),
                        revision_id: revision.clone(),
                        updated_at: ts("2026-08-26T04:00:00Z"),
                    },
                    ts("2026-08-26T04:00:00Z"),
                )
                .unwrap_err();
            assert!(matches!(error, ArtifactError::Io(_)), "stage {fault:?}");

            let reopened = ArtifactStore::new(&root)
                .unwrap()
                .library_snapshot()
                .unwrap();
            if fault == LibraryPersistFault::DirectorySync {
                assert_eq!(reopened.version, 2);
                assert_eq!(reopened.active_names.get("demo"), Some(&artifact));
                assert_eq!(
                    reopened.records[&artifact].active_revision_id.as_deref(),
                    Some(revision.as_str())
                );
            } else {
                assert_eq!(reopened.version, 1, "stage {fault:?}");
                assert!(reopened.active_names.is_empty(), "stage {fault:?}");
                assert!(reopened.records[&artifact].active_revision_id.is_none());
            }
        }
    }

    #[test]
    fn waiting_writer_makes_bounded_progress_after_library_lock_release() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let lock = store.library_lock().unwrap();
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        let writer = std::thread::spawn({
            let root = root.clone();
            let owner = owner.clone();
            move || {
                let store = ArtifactStore::new(root).unwrap();
                let result = store.mutate_library(
                    &owner_auth(&owner),
                    &owner,
                    1,
                    idem("bounded-writer"),
                    LibraryMutation::SetVisibility {
                        artifact_id: artifact,
                        visibility: SkillVisibility::Tenant,
                        updated_at: ts("2026-08-26T04:30:00Z"),
                    },
                    ts("2026-08-26T04:30:00Z"),
                );
                sent.send(result).unwrap();
            }
        });
        assert!(
            received
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err(),
            "writer must wait while the commit lock is held"
        );
        drop(lock);
        let receipt = received
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("writer makes bounded progress after release")
            .unwrap();
        assert_eq!(receipt.committed_version, 2);
        writer.join().unwrap();
    }

    #[test]
    fn duplicate_active_names_fail_closed_on_reopen() {
        let data = tempdir().unwrap();
        let source_a = tempdir().unwrap();
        let source_b = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (a, rev_a) = add_skill(&store, &source_a, "team-a", "shared", &owner, 0);
        let (b, rev_b) = add_skill(&store, &source_b, "team-b", "shared", &owner, 1);
        let mut snapshot = store.library_snapshot().unwrap();
        snapshot.records.get_mut(&a).unwrap().active_revision_id = Some(rev_a);
        snapshot.records.get_mut(&b).unwrap().active_revision_id = Some(rev_b);
        snapshot.active_names.insert("shared".into(), a);
        write_snapshot(&root, &mut snapshot);
        assert!(matches!(
            ArtifactStore::new(&root).unwrap().library_snapshot(),
            Err(ArtifactError::LibraryCorrupt("duplicate_active_name"))
        ));
    }

    #[test]
    fn archived_active_record_fails_closed_on_reopen() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, revision) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let mut snapshot = store.library_snapshot().unwrap();
        let record = snapshot.records.get_mut(&artifact).unwrap();
        record.archived = true;
        record.active_revision_id = Some(revision);
        snapshot.active_names.insert("demo".into(), artifact);
        write_snapshot(&root, &mut snapshot);
        assert!(matches!(
            ArtifactStore::new(&root).unwrap().library_snapshot(),
            Err(ArtifactError::LibraryCorrupt("archived_active_record"))
        ));
    }

    #[test]
    fn concurrent_deactivate_and_archive_cas_has_one_winner() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, revision) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                1,
                idem("activate-race"),
                LibraryMutation::Activate {
                    artifact_id: artifact.clone(),
                    revision_id: revision,
                    updated_at: ts("2026-08-26T03:00:00Z"),
                },
                ts("2026-08-26T03:00:00Z"),
            )
            .unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let handles = [false, true]
            .into_iter()
            .map(|archive| {
                let root = root.clone();
                let owner = owner.clone();
                let artifact = artifact.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let store = ArtifactStore::new(root).unwrap();
                    let mutation = if archive {
                        LibraryMutation::Archive {
                            artifact_id: artifact,
                            updated_at: ts("2026-08-26T03:01:00Z"),
                        }
                    } else {
                        LibraryMutation::Deactivate {
                            artifact_id: artifact,
                            updated_at: ts("2026-08-26T03:01:00Z"),
                        }
                    };
                    barrier.wait();
                    store.mutate_library(
                        &owner_auth(&owner),
                        &owner,
                        2,
                        idem(if archive {
                            "archive-race"
                        } else {
                            "deactivate-race"
                        }),
                        mutation,
                        ts("2026-08-26T03:01:00Z"),
                    )
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let state = ArtifactStore::new(root)
            .unwrap()
            .library_snapshot()
            .unwrap();
        assert_eq!(state.version, 3);
        assert!(state.active_names.is_empty());
    }

    #[test]
    fn artifact_io_is_not_convoyed_by_the_library_commit_lock() {
        let data = tempdir().unwrap();
        let source_a = tempdir().unwrap();
        let source_b = tempdir().unwrap();
        let store = ArtifactStore::new(data.path().join("store")).unwrap();
        let owner = ownership("org-a", "alice");
        let (artifact, _) = add_skill(&store, &source_a, "team-a", "demo", &owner, 0);

        let _library_lock = store.library_lock().unwrap();
        std::fs::write(
            source_b.path().join("SKILL.md"),
            "---\nname: other\ndescription: Test\n---\nBody\n",
        )
        .unwrap();
        store
            .import_local(
                ArtifactImportRequest::new("skill", "team-b", "other"),
                source_b.path(),
            )
            .expect("unrelated immutable Artifact I/O uses its own artifact lock");

        let error = store
            .mutate_library(
                &owner_auth(&owner),
                &owner,
                1,
                idem("missing-revision"),
                LibraryMutation::Activate {
                    artifact_id: artifact,
                    revision_id: "rev_missing".into(),
                    updated_at: ts("2026-08-26T03:30:00Z"),
                },
                ts("2026-08-26T03:30:00Z"),
            )
            .unwrap_err();
        assert!(matches!(error, ArtifactError::NotFound("revision")));
    }

    #[test]
    fn cap_plus_one_reopens_and_evicts_by_sequence_not_digest_order() {
        let data = tempdir().unwrap();
        let source = tempdir().unwrap();
        let root = data.path().join("store");
        let store = ArtifactStore::new(&root).unwrap();
        let owner = ownership("org-a", "alice");
        let auth = owner_auth(&owner);
        let (artifact, _) = add_skill(&store, &source, "team-a", "demo", &owner, 0);
        let first_key = (0_u64..10_000)
            .map(|index| format!("reverse-{index}"))
            .find(|key| {
                receipt_scope_digest(
                    &auth.tenant_id,
                    &auth.actor_id,
                    "set_visibility",
                    &artifact,
                    key,
                )
                .unwrap()
                .as_bytes()[7]
                    >= b'e'
            })
            .unwrap();
        let first_scope = receipt_scope_digest(
            &auth.tenant_id,
            &auth.actor_id,
            "set_visibility",
            &artifact,
            &first_key,
        )
        .unwrap();
        let mut snapshot = store.library_snapshot().unwrap();
        snapshot.receipts.clear();
        snapshot.audit_intents.clear();
        for index in 0..=MAX_RECEIPTS {
            let sequence = index as u64 + 1;
            let key = if index == 0 {
                first_key.clone()
            } else {
                format!("retained-{index}")
            };
            let scope = receipt_scope_digest(
                &auth.tenant_id,
                &auth.actor_id,
                "set_visibility",
                &artifact,
                &key,
            )
            .unwrap();
            let request_digest = canonical_json::digest(&key).unwrap();
            snapshot.receipts.insert(
                scope.clone(),
                LibraryReceipt {
                    sequence,
                    scope_digest: scope,
                    tenant_id: auth.tenant_id.clone(),
                    actor_id: auth.actor_id.clone(),
                    action: "set_visibility".into(),
                    artifact_id: artifact.clone(),
                    idempotency_key: key,
                    request_digest: request_digest.clone(),
                    committed_version: sequence,
                },
            );
            snapshot.audit_intents.push(LibraryAuditIntent {
                sequence,
                action: "set_visibility".into(),
                tenant_id: auth.tenant_id.clone(),
                actor_id: auth.actor_id.clone(),
                artifact_id: artifact.clone(),
                request_digest,
                committed_at: ts("2026-08-26T02:00:00Z"),
            });
        }
        snapshot.version = MAX_RECEIPTS as u64 + 1;
        enforce_retention(&mut snapshot);
        write_snapshot(&root, &mut snapshot);
        let snapshot = ArtifactStore::new(&root)
            .unwrap()
            .library_snapshot()
            .unwrap();
        assert_eq!(snapshot.receipts.len(), MAX_RECEIPTS);
        assert_eq!(snapshot.audit_intents.len(), MAX_AUDIT_INTENTS);
        assert!(!snapshot.receipts.contains_key(&first_scope));
        assert!(snapshot.receipts.keys().any(|scope| scope < &first_scope));
        assert_eq!(
            snapshot
                .receipts
                .values()
                .map(|receipt| receipt.sequence)
                .min(),
            Some(2)
        );

        // Once the bounded retention window has deliberately forgotten a receipt, replay is a
        // fresh CAS mutation rather than an incorrectly attributed replay of another digest key.
        let replay = store
            .mutate_library(
                &auth,
                &owner,
                MAX_RECEIPTS as u64 + 1,
                idem(&first_key),
                LibraryMutation::SetVisibility {
                    artifact_id: artifact,
                    visibility: SkillVisibility::Tenant,
                    updated_at: ts("2026-08-26T02:01:00Z"),
                },
                ts("2026-08-26T02:01:00Z"),
            )
            .unwrap();
        assert_eq!(replay.committed_version, MAX_RECEIPTS as u64 + 2);
        assert_eq!(replay.idempotency_key, first_key);
    }
}
