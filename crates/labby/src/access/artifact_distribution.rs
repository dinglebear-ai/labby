use labby_auth::VerifiedIdentity;
use labby_primitives::access::{OwnerScope, PrincipalId, ProjectId, TeamId};
use labby_runtime::artifacts::{
    ArtifactLicenseState, ArtifactPublication, Distribution, PublicationState, Redistribution,
    TakedownState,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::authorization::authorize_library_in_transaction;
use super::domain::Permission;
use super::error::{AccessStoreError, AccessStoreResult};
use super::store::{AccessStore, map_sqlite_error};

pub(super) const SCHEMA: &str = r"
CREATE TABLE artifact_authorities (
    artifact_id TEXT PRIMARY KEY CHECK(length(trim(artifact_id)) BETWEEN 1 AND 2048),
    operation_id TEXT NOT NULL UNIQUE CHECK(length(trim(operation_id)) BETWEEN 1 AND 256),
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('personal','team','project')),
    owner_id TEXT NOT NULL CHECK(length(trim(owner_id)) BETWEEN 1 AND 256),
    status TEXT NOT NULL CHECK(status IN ('pending','committing','active','failed')),
    policy_epoch INTEGER NOT NULL CHECK(policy_epoch > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE artifact_publisher_policies (
    artifact_id TEXT PRIMARY KEY,
    policy_epoch INTEGER NOT NULL CHECK(policy_epoch > 0),
    allow_sync INTEGER NOT NULL CHECK(allow_sync IN (0,1)),
    allow_follow INTEGER NOT NULL CHECK(allow_follow IN (0,1)),
    allow_fork INTEGER NOT NULL CHECK(allow_fork IN (0,1)),
    allow_export INTEGER NOT NULL CHECK(allow_export IN (0,1)),
    allow_reshare INTEGER NOT NULL CHECK(allow_reshare IN (0,1)),
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(artifact_id) REFERENCES artifact_authorities(artifact_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE artifact_source_policies (
    provider_authority TEXT NOT NULL CHECK(length(trim(provider_authority)) BETWEEN 1 AND 512),
    artifact_id TEXT NOT NULL CHECK(length(trim(artifact_id)) BETWEEN 1 AND 2048),
    source_scope_kind TEXT NOT NULL CHECK(source_scope_kind IN ('personal','team','project')),
    source_scope_id TEXT NOT NULL CHECK(length(trim(source_scope_id)) BETWEEN 1 AND 256),
    policy_epoch INTEGER NOT NULL CHECK(policy_epoch > 0),
    allow_sync INTEGER NOT NULL CHECK(allow_sync IN (0,1)),
    allow_follow INTEGER NOT NULL CHECK(allow_follow IN (0,1)),
    allow_fork INTEGER NOT NULL CHECK(allow_fork IN (0,1)),
    allow_export INTEGER NOT NULL CHECK(allow_export IN (0,1)),
    allow_reshare INTEGER NOT NULL CHECK(allow_reshare IN (0,1)),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(provider_authority, artifact_id)
) STRICT;

CREATE TABLE artifact_assignment_distributions (
    assignment_id TEXT PRIMARY KEY CHECK(length(trim(assignment_id)) BETWEEN 1 AND 256),
    provider_authority TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    source_scope_kind TEXT NOT NULL CHECK(source_scope_kind IN ('personal','team','project')),
    source_scope_id TEXT NOT NULL CHECK(length(trim(source_scope_id)) BETWEEN 1 AND 256),
    policy_epoch INTEGER NOT NULL CHECK(policy_epoch > 0),
    allow_sync INTEGER NOT NULL CHECK(allow_sync IN (0,1)),
    allow_follow INTEGER NOT NULL CHECK(allow_follow IN (0,1)),
    allow_fork INTEGER NOT NULL CHECK(allow_fork IN (0,1)),
    allow_export INTEGER NOT NULL CHECK(allow_export IN (0,1)),
    allow_reshare INTEGER NOT NULL CHECK(allow_reshare IN (0,1)),
    status TEXT NOT NULL CHECK(status IN ('active','revoked')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(provider_authority, artifact_id)
      REFERENCES artifact_source_policies(provider_authority, artifact_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX artifact_assignment_distributions_lookup
    ON artifact_assignment_distributions(provider_authority, artifact_id, status);

CREATE TABLE artifact_mirrors (
    mirror_id TEXT PRIMARY KEY CHECK(length(trim(mirror_id)) BETWEEN 1 AND 256),
    operation_id TEXT NOT NULL UNIQUE CHECK(length(trim(operation_id)) BETWEEN 1 AND 256),
    owner_principal_id TEXT NOT NULL,
    identity_ref_json TEXT NOT NULL CHECK(json_valid(identity_ref_json) AND length(identity_ref_json) <= 16384),
    authorization_project_id TEXT NOT NULL CHECK(length(trim(authorization_project_id)) BETWEEN 1 AND 256),
    authorization_team_id TEXT CHECK(length(trim(authorization_team_id)) BETWEEN 1 AND 256),
    destination_id TEXT,
    source_provider_authority TEXT NOT NULL,
    source_assignment_id TEXT NOT NULL,
    source_scope_kind TEXT NOT NULL CHECK(source_scope_kind IN ('personal','team','project')),
    source_scope_id TEXT NOT NULL CHECK(length(trim(source_scope_id)) BETWEEN 1 AND 256),
    source_artifact_id TEXT NOT NULL,
    source_revision_id TEXT NOT NULL CHECK(length(trim(source_revision_id)) BETWEEN 1 AND 2048),
    local_artifact_id TEXT NOT NULL CHECK(length(trim(local_artifact_id)) BETWEEN 1 AND 2048),
    local_revision_id TEXT,
    mode TEXT NOT NULL CHECK(mode IN ('pinned','followed')),
    status TEXT NOT NULL CHECK(status IN ('pending','committing','active','access_revoked','source_withdrawn','removed','failed')),
    last_authorized_policy_epoch INTEGER NOT NULL CHECK(last_authorized_policy_epoch > 0),
    last_checked_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK(destination_id IS NULL OR length(trim(destination_id)) BETWEEN 1 AND 256),
    CHECK(local_revision_id IS NULL OR length(trim(local_revision_id)) BETWEEN 1 AND 2048),
    CHECK(status != 'active' OR local_revision_id IS NOT NULL),
    FOREIGN KEY(owner_principal_id) REFERENCES principals(principal_id) ON DELETE RESTRICT,
    FOREIGN KEY(source_assignment_id) REFERENCES artifact_assignment_distributions(assignment_id) ON DELETE RESTRICT
) STRICT;
CREATE INDEX artifact_mirrors_owner_status
    ON artifact_mirrors(owner_principal_id, status);
CREATE INDEX artifact_mirrors_source
    ON artifact_mirrors(source_provider_authority, source_artifact_id, source_revision_id);
CREATE INDEX artifact_mirrors_reconcile
    ON artifact_mirrors(status, last_checked_at, mirror_id);

CREATE TABLE artifact_subscriptions (
    mirror_id TEXT PRIMARY KEY,
    update_policy TEXT NOT NULL CHECK(update_policy IN ('notify','auto_approved','pinned')),
    last_observed_revision_id TEXT,
    last_applied_revision_id TEXT,
    last_checked_at INTEGER,
    status TEXT NOT NULL CHECK(status IN ('active','paused','revoked')),
    updated_at INTEGER NOT NULL,
    CHECK(last_observed_revision_id IS NULL OR length(trim(last_observed_revision_id)) BETWEEN 1 AND 2048),
    CHECK(last_applied_revision_id IS NULL OR length(trim(last_applied_revision_id)) BETWEEN 1 AND 2048),
    FOREIGN KEY(mirror_id) REFERENCES artifact_mirrors(mirror_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX artifact_subscriptions_reconcile
    ON artifact_subscriptions(status, update_policy, last_checked_at, mirror_id);
";

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum ArtifactDistributionPermission {
    Use,
    Sync,
    Follow,
    Fork,
    Export,
    Reshare,
}

impl ArtifactDistributionPermission {
    const ALL: [Self; 6] = [
        Self::Use,
        Self::Sync,
        Self::Follow,
        Self::Fork,
        Self::Export,
        Self::Reshare,
    ];

    pub(crate) const fn as_wire(self) -> &'static str {
        match self {
            Self::Use => "artifact.use",
            Self::Sync => "artifact.sync",
            Self::Follow => "artifact.follow",
            Self::Fork => "artifact.fork",
            Self::Export => "artifact.export",
            Self::Reshare => "artifact.reshare",
        }
    }

    const fn project_permission(self) -> Permission {
        match self {
            Self::Use => Permission::ArtifactUse,
            Self::Sync => Permission::ArtifactSync,
            Self::Follow => Permission::ArtifactFollow,
            Self::Fork => Permission::ArtifactFork,
            Self::Export => Permission::ArtifactExport,
            Self::Reshare => Permission::ArtifactReshare,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactDistributionAuthoritySnapshot {
    pub(crate) principal_id: String,
    pub(crate) organization_id: String,
    pub(crate) project_id: String,
    pub(crate) grants: ArtifactDistributionGrants,
    pub(crate) global_revision: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ArtifactDistributionGrants {
    pub(crate) use_remote: bool,
    pub(crate) sync: bool,
    pub(crate) follow: bool,
    pub(crate) fork: bool,
    pub(crate) export: bool,
    pub(crate) reshare: bool,
}

impl ArtifactDistributionGrants {
    fn set(&mut self, permission: ArtifactDistributionPermission, allowed: bool) {
        match permission {
            ArtifactDistributionPermission::Use => self.use_remote = allowed,
            ArtifactDistributionPermission::Sync => self.sync = allowed,
            ArtifactDistributionPermission::Follow => self.follow = allowed,
            ArtifactDistributionPermission::Fork => self.fork = allowed,
            ArtifactDistributionPermission::Export => self.export = allowed,
            ArtifactDistributionPermission::Reshare => self.reshare = allowed,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ArtifactDistributionCeiling {
    pub(crate) sync: bool,
    pub(crate) follow: bool,
    pub(crate) fork: bool,
    pub(crate) export: bool,
    pub(crate) reshare: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactDestinationPolicy {
    pub(crate) active: bool,
    pub(crate) pin: bool,
    pub(crate) follow: bool,
    pub(crate) fork: bool,
    pub(crate) reshare: bool,
}

impl ArtifactDestinationPolicy {
    pub(crate) const fn local_personal() -> Self {
        Self {
            active: true,
            pin: true,
            follow: true,
            fork: true,
            reshare: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArtifactTransferMode {
    UseRemote,
    Pin,
    Follow,
    Fork,
    Export,
    Reshare,
}

impl ArtifactTransferMode {
    pub(crate) const ALL: [Self; 6] = [
        Self::UseRemote,
        Self::Pin,
        Self::Follow,
        Self::Fork,
        Self::Export,
        Self::Reshare,
    ];

    pub(crate) const fn as_wire(self) -> &'static str {
        match self {
            Self::UseRemote => "use_remote",
            Self::Pin => "pin",
            Self::Follow => "follow",
            Self::Fork => "fork",
            Self::Export => "export",
            Self::Reshare => "reshare",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArtifactTransferDenyReason {
    CallerPermission,
    PublisherPolicy,
    AssignmentPolicy,
    PublicationState,
    LicensePolicy,
    Takedown,
    DestinationPolicy,
}

impl ArtifactTransferDenyReason {
    pub(crate) const fn as_wire(self) -> &'static str {
        match self {
            Self::CallerPermission => "caller_permission",
            Self::PublisherPolicy => "publisher_policy",
            Self::AssignmentPolicy => "assignment_policy",
            Self::PublicationState => "publication_state",
            Self::LicensePolicy => "license_policy",
            Self::Takedown => "takedown",
            Self::DestinationPolicy => "destination_policy",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactTransferDecision {
    pub(crate) mode: ArtifactTransferMode,
    pub(crate) allowed: bool,
    pub(crate) denied_by: Option<ArtifactTransferDenyReason>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactTransferOptions {
    decisions: Vec<ArtifactTransferDecision>,
}

impl ArtifactTransferOptions {
    pub(crate) fn decision(&self, mode: ArtifactTransferMode) -> ArtifactTransferDecision {
        self.decisions
            .iter()
            .copied()
            .find(|candidate| candidate.mode == mode)
            .expect("all transfer modes are always evaluated")
    }

    pub(crate) fn allows(&self, mode: ArtifactTransferMode) -> bool {
        self.decision(mode).allowed
    }

    pub(crate) fn decisions(&self) -> &[ArtifactTransferDecision] {
        &self.decisions
    }
}

pub(crate) struct ArtifactTransferFacts<'a> {
    pub(crate) grants: ArtifactDistributionGrants,
    pub(crate) publisher: ArtifactDistributionCeiling,
    pub(crate) assignment: Option<ArtifactDistributionCeiling>,
    pub(crate) publication: &'a ArtifactPublication,
    pub(crate) license: &'a ArtifactLicenseState,
    pub(crate) destination: Option<ArtifactDestinationPolicy>,
}

pub(crate) fn evaluate_transfer_options(
    facts: ArtifactTransferFacts<'_>,
) -> ArtifactTransferOptions {
    let decisions = ArtifactTransferMode::ALL
        .into_iter()
        .map(|mode| evaluate_transfer_mode(mode, &facts))
        .collect();
    ArtifactTransferOptions { decisions }
}

fn evaluate_transfer_mode(
    mode: ArtifactTransferMode,
    facts: &ArtifactTransferFacts<'_>,
) -> ArtifactTransferDecision {
    let deny = |reason| ArtifactTransferDecision {
        mode,
        allowed: false,
        denied_by: Some(reason),
    };
    let allow = || ArtifactTransferDecision {
        mode,
        allowed: true,
        denied_by: None,
    };

    if matches!(
        facts.license.takedown_state,
        TakedownState::Restricted | TakedownState::Removed
    ) {
        return deny(ArtifactTransferDenyReason::Takedown);
    }
    if facts.publication.state == PublicationState::Withdrawn {
        return deny(ArtifactTransferDenyReason::PublicationState);
    }

    if mode == ArtifactTransferMode::UseRemote {
        return if facts.grants.use_remote {
            allow()
        } else {
            deny(ArtifactTransferDenyReason::CallerPermission)
        };
    }

    if facts.publication.state != PublicationState::Published
        || facts.publication.distribution != Distribution::Bytes
    {
        return deny(ArtifactTransferDenyReason::PublicationState);
    }

    let caller_allows = match mode {
        ArtifactTransferMode::UseRemote => unreachable!("handled above"),
        ArtifactTransferMode::Pin => facts.grants.sync,
        ArtifactTransferMode::Follow => facts.grants.sync && facts.grants.follow,
        ArtifactTransferMode::Fork => facts.grants.fork,
        ArtifactTransferMode::Export => facts.grants.export,
        ArtifactTransferMode::Reshare => facts.grants.reshare,
    };
    if !caller_allows {
        return deny(ArtifactTransferDenyReason::CallerPermission);
    }

    let publisher_allows = ceiling_allows(facts.publisher, mode);
    if !publisher_allows {
        return deny(ArtifactTransferDenyReason::PublisherPolicy);
    }

    let Some(assignment) = facts.assignment else {
        return deny(ArtifactTransferDenyReason::AssignmentPolicy);
    };
    if !ceiling_allows(assignment, mode) {
        return deny(ArtifactTransferDenyReason::AssignmentPolicy);
    }

    let license_allows = match mode {
        ArtifactTransferMode::Fork => facts.license.redistribution == Redistribution::Forkable,
        ArtifactTransferMode::Pin
        | ArtifactTransferMode::Follow
        | ArtifactTransferMode::Export
        | ArtifactTransferMode::Reshare => matches!(
            facts.license.redistribution,
            Redistribution::Redistributable | Redistribution::Forkable
        ),
        ArtifactTransferMode::UseRemote => true,
    };
    if !license_allows {
        return deny(ArtifactTransferDenyReason::LicensePolicy);
    }

    if matches!(
        mode,
        ArtifactTransferMode::Pin
            | ArtifactTransferMode::Follow
            | ArtifactTransferMode::Fork
            | ArtifactTransferMode::Reshare
    ) {
        let Some(destination) = facts.destination else {
            return deny(ArtifactTransferDenyReason::DestinationPolicy);
        };
        if !destination.active || !destination_allows(destination, mode) {
            return deny(ArtifactTransferDenyReason::DestinationPolicy);
        }
    }

    allow()
}

const fn ceiling_allows(ceiling: ArtifactDistributionCeiling, mode: ArtifactTransferMode) -> bool {
    match mode {
        ArtifactTransferMode::UseRemote => true,
        ArtifactTransferMode::Pin => ceiling.sync,
        ArtifactTransferMode::Follow => ceiling.sync && ceiling.follow,
        ArtifactTransferMode::Fork => ceiling.fork,
        ArtifactTransferMode::Export => ceiling.export,
        ArtifactTransferMode::Reshare => ceiling.reshare,
    }
}

const fn destination_allows(
    destination: ArtifactDestinationPolicy,
    mode: ArtifactTransferMode,
) -> bool {
    match mode {
        ArtifactTransferMode::UseRemote | ArtifactTransferMode::Export => true,
        ArtifactTransferMode::Pin => destination.pin,
        ArtifactTransferMode::Follow => destination.follow,
        ArtifactTransferMode::Fork => destination.fork,
        ArtifactTransferMode::Reshare => destination.reshare,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArtifactAuthorityStatus {
    Pending,
    Committing,
    Active,
    Failed,
}

impl ArtifactAuthorityStatus {
    pub(crate) const fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Committing => "committing",
            Self::Active => "active",
            Self::Failed => "failed",
        }
    }

    fn from_wire(value: &str) -> AccessStoreResult<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "committing" => Ok(Self::Committing),
            "active" => Ok(Self::Active),
            "failed" => Ok(Self::Failed),
            _ => Err(AccessStoreError::MalformedVocabulary),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StageArtifactAuthority {
    pub(crate) artifact_id: String,
    pub(crate) operation_id: String,
    pub(crate) owner: OwnerScope,
    pub(crate) policy_epoch: u64,
    pub(crate) now: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactAuthorityRecord {
    pub(crate) artifact_id: String,
    pub(crate) operation_id: String,
    pub(crate) owner: OwnerScope,
    pub(crate) status: ArtifactAuthorityStatus,
    pub(crate) policy_epoch: u64,
    pub(crate) created_at: i64,
    pub(crate) updated_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactPublisherPolicyRecord {
    pub(crate) artifact_id: String,
    pub(crate) policy_epoch: u64,
    pub(crate) ceiling: ArtifactDistributionCeiling,
    pub(crate) updated_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactSourcePolicyRecord {
    pub(crate) provider_authority: String,
    pub(crate) artifact_id: String,
    pub(crate) source_scope: OwnerScope,
    pub(crate) policy_epoch: u64,
    pub(crate) ceiling: ArtifactDistributionCeiling,
    pub(crate) updated_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactAssignmentDistributionRecord {
    pub(crate) assignment_id: String,
    pub(crate) provider_authority: String,
    pub(crate) artifact_id: String,
    pub(crate) source_scope: OwnerScope,
    pub(crate) policy_epoch: u64,
    pub(crate) ceiling: ArtifactDistributionCeiling,
    pub(crate) active: bool,
    pub(crate) created_at: i64,
    pub(crate) updated_at: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedArtifactMirrorMode {
    Pinned,
    Followed,
}

impl ManagedArtifactMirrorMode {
    pub(crate) const fn as_wire(self) -> &'static str {
        match self {
            Self::Pinned => "pinned",
            Self::Followed => "followed",
        }
    }

    fn from_wire(value: &str) -> AccessStoreResult<Self> {
        match value {
            "pinned" => Ok(Self::Pinned),
            "followed" => Ok(Self::Followed),
            _ => Err(AccessStoreError::MalformedVocabulary),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedArtifactMirrorStatus {
    Pending,
    Committing,
    Active,
    AccessRevoked,
    SourceWithdrawn,
    Removed,
    Failed,
}

impl ManagedArtifactMirrorStatus {
    pub(crate) const fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Committing => "committing",
            Self::Active => "active",
            Self::AccessRevoked => "access_revoked",
            Self::SourceWithdrawn => "source_withdrawn",
            Self::Removed => "removed",
            Self::Failed => "failed",
        }
    }

    fn from_wire(value: &str) -> AccessStoreResult<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "committing" => Ok(Self::Committing),
            "active" => Ok(Self::Active),
            "access_revoked" => Ok(Self::AccessRevoked),
            "source_withdrawn" => Ok(Self::SourceWithdrawn),
            "removed" => Ok(Self::Removed),
            "failed" => Ok(Self::Failed),
            _ => Err(AccessStoreError::MalformedVocabulary),
        }
    }

    const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::AccessRevoked | Self::SourceWithdrawn | Self::Removed | Self::Failed
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StageManagedArtifactMirror {
    pub(crate) mirror_id: String,
    pub(crate) operation_id: String,
    pub(crate) owner_principal_id: String,
    pub(crate) identity_ref_json: String,
    pub(crate) authorization_project_id: String,
    pub(crate) authorization_team_id: Option<String>,
    pub(crate) destination_id: Option<String>,
    pub(crate) source_provider_authority: String,
    pub(crate) source_assignment_id: String,
    pub(crate) source_scope: OwnerScope,
    pub(crate) source_artifact_id: String,
    pub(crate) source_revision_id: String,
    pub(crate) local_artifact_id: String,
    pub(crate) mode: ManagedArtifactMirrorMode,
    pub(crate) last_authorized_policy_epoch: u64,
    pub(crate) now: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BeginManagedArtifactMirrorUpdate {
    pub(crate) mirror_id: String,
    pub(crate) operation_id: String,
    pub(crate) source_revision_id: String,
    pub(crate) expected_local_revision_id: String,
    pub(crate) policy_epoch: u64,
    pub(crate) now: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManagedArtifactMirror {
    pub(crate) mirror_id: String,
    pub(crate) operation_id: String,
    pub(crate) owner_principal_id: String,
    pub(crate) identity_ref_json: String,
    pub(crate) authorization_project_id: String,
    pub(crate) authorization_team_id: Option<String>,
    pub(crate) destination_id: Option<String>,
    pub(crate) source_provider_authority: String,
    pub(crate) source_assignment_id: String,
    pub(crate) source_scope: OwnerScope,
    pub(crate) source_artifact_id: String,
    pub(crate) source_revision_id: String,
    pub(crate) local_artifact_id: String,
    pub(crate) local_revision_id: Option<String>,
    pub(crate) mode: ManagedArtifactMirrorMode,
    pub(crate) status: ManagedArtifactMirrorStatus,
    pub(crate) last_authorized_policy_epoch: u64,
    pub(crate) last_checked_at: Option<i64>,
    pub(crate) created_at: i64,
    pub(crate) updated_at: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArtifactSubscriptionUpdatePolicy {
    Notify,
    AutoApproved,
    Pinned,
}

impl ArtifactSubscriptionUpdatePolicy {
    pub(crate) const fn as_wire(self) -> &'static str {
        match self {
            Self::Notify => "notify",
            Self::AutoApproved => "auto_approved",
            Self::Pinned => "pinned",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManagedArtifactSubscription {
    pub(crate) mirror_id: String,
    pub(crate) update_policy: ArtifactSubscriptionUpdatePolicy,
    pub(crate) last_observed_revision_id: Option<String>,
    pub(crate) last_applied_revision_id: Option<String>,
    pub(crate) last_checked_at: Option<i64>,
    pub(crate) active: bool,
    pub(crate) updated_at: i64,
}

fn validate_text(value: &str, max: usize) -> AccessStoreResult<()> {
    if value.is_empty()
        || value.len() > max
        || value != value.trim()
        || value.chars().any(char::is_control)
    {
        return Err(AccessStoreError::InvalidArtifactDistributionInput);
    }
    Ok(())
}

fn bump_global_revision(
    transaction: &rusqlite::Transaction<'_>,
    updated_at: i64,
) -> AccessStoreResult<u64> {
    let revision = transaction
        .query_row(
            "UPDATE access_metadata SET global_revision=global_revision+1,updated_at=?1 WHERE singleton=1 RETURNING global_revision",
            [updated_at],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_sqlite_error)?;
    u64::try_from(revision).map_err(|_| AccessStoreError::MalformedVocabulary)
}

fn validate_epoch(epoch: u64) -> AccessStoreResult<i64> {
    let value =
        i64::try_from(epoch).map_err(|_| AccessStoreError::InvalidArtifactDistributionInput)?;
    if value <= 0 {
        return Err(AccessStoreError::InvalidArtifactDistributionInput);
    }
    Ok(value)
}

fn owner_parts(owner: &OwnerScope) -> AccessStoreResult<(&'static str, &str)> {
    match owner {
        OwnerScope::Personal(id) => Ok(("personal", id.as_str())),
        OwnerScope::Team(id) => Ok(("team", id.as_str())),
        OwnerScope::Project(id) => Ok(("project", id.as_str())),
        OwnerScope::Installation(_) => Err(AccessStoreError::InvalidArtifactDistributionInput),
    }
}

fn owner_from_parts(kind: &str, id: String) -> AccessStoreResult<OwnerScope> {
    match kind {
        "personal" => PrincipalId::new(id).map(OwnerScope::Personal),
        "team" => TeamId::new(id).map(OwnerScope::Team),
        "project" => ProjectId::new(id).map(OwnerScope::Project),
        _ => return Err(AccessStoreError::MalformedVocabulary),
    }
    .map_err(|_| AccessStoreError::MalformedVocabulary)
}

#[derive(Debug)]
struct RawMirror {
    mirror_id: String,
    operation_id: String,
    owner_principal_id: String,
    identity_ref_json: String,
    authorization_project_id: String,
    authorization_team_id: Option<String>,
    destination_id: Option<String>,
    source_provider_authority: String,
    source_assignment_id: String,
    source_scope_kind: String,
    source_scope_id: String,
    source_artifact_id: String,
    source_revision_id: String,
    local_artifact_id: String,
    local_revision_id: Option<String>,
    mode: String,
    status: String,
    last_authorized_policy_epoch: i64,
    last_checked_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

impl RawMirror {
    fn into_record(self) -> AccessStoreResult<ManagedArtifactMirror> {
        Ok(ManagedArtifactMirror {
            mirror_id: self.mirror_id,
            operation_id: self.operation_id,
            owner_principal_id: self.owner_principal_id,
            identity_ref_json: self.identity_ref_json,
            authorization_project_id: self.authorization_project_id,
            authorization_team_id: self.authorization_team_id,
            destination_id: self.destination_id,
            source_provider_authority: self.source_provider_authority,
            source_assignment_id: self.source_assignment_id,
            source_scope: owner_from_parts(&self.source_scope_kind, self.source_scope_id)?,
            source_artifact_id: self.source_artifact_id,
            source_revision_id: self.source_revision_id,
            local_artifact_id: self.local_artifact_id,
            local_revision_id: self.local_revision_id,
            mode: ManagedArtifactMirrorMode::from_wire(&self.mode)?,
            status: ManagedArtifactMirrorStatus::from_wire(&self.status)?,
            last_authorized_policy_epoch: u64::try_from(self.last_authorized_policy_epoch)
                .map_err(|_| AccessStoreError::MalformedVocabulary)?,
            last_checked_at: self.last_checked_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

fn query_artifact_authority(
    connection: &rusqlite::Connection,
    artifact_id: &str,
) -> AccessStoreResult<Option<ArtifactAuthorityRecord>> {
    connection
        .query_row(
            "SELECT artifact_id,operation_id,owner_kind,owner_id,status,policy_epoch,created_at,updated_at FROM artifact_authorities WHERE artifact_id=?1",
            [artifact_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?
        .map(|(artifact_id, operation_id, owner_kind, owner_id, status, policy_epoch, created_at, updated_at)| {
            Ok(ArtifactAuthorityRecord {
                artifact_id,
                operation_id,
                owner: owner_from_parts(&owner_kind, owner_id)?,
                status: ArtifactAuthorityStatus::from_wire(&status)?,
                policy_epoch: u64::try_from(policy_epoch)
                    .map_err(|_| AccessStoreError::MalformedVocabulary)?,
                created_at,
                updated_at,
            })
        })
        .transpose()
}

fn query_mirror(
    connection: &rusqlite::Connection,
    mirror_id: &str,
) -> AccessStoreResult<Option<ManagedArtifactMirror>> {
    let raw = connection
        .query_row(
            "SELECT mirror_id,operation_id,owner_principal_id,identity_ref_json,authorization_project_id,authorization_team_id,destination_id,source_provider_authority,source_assignment_id,source_scope_kind,source_scope_id,source_artifact_id,source_revision_id,local_artifact_id,local_revision_id,mode,status,last_authorized_policy_epoch,last_checked_at,created_at,updated_at FROM artifact_mirrors WHERE mirror_id=?1",
            [mirror_id],
            |row| {
                Ok(RawMirror {
                    mirror_id: row.get(0)?, operation_id: row.get(1)?, owner_principal_id: row.get(2)?,
                    identity_ref_json: row.get(3)?, authorization_project_id: row.get(4)?, authorization_team_id: row.get(5)?,
                    destination_id: row.get(6)?, source_provider_authority: row.get(7)?, source_assignment_id: row.get(8)?,
                    source_scope_kind: row.get(9)?, source_scope_id: row.get(10)?, source_artifact_id: row.get(11)?, source_revision_id: row.get(12)?,
                    local_artifact_id: row.get(13)?, local_revision_id: row.get(14)?, mode: row.get(15)?, status: row.get(16)?,
                    last_authorized_policy_epoch: row.get(17)?, last_checked_at: row.get(18)?, created_at: row.get(19)?, updated_at: row.get(20)?,
                })
            },
        )
        .optional()
        .map_err(map_sqlite_error)?;
    raw.map(RawMirror::into_record).transpose()
}

impl ArtifactSubscriptionUpdatePolicy {
    fn from_wire(value: &str) -> AccessStoreResult<Self> {
        match value {
            "notify" => Ok(Self::Notify),
            "auto_approved" => Ok(Self::AutoApproved),
            "pinned" => Ok(Self::Pinned),
            _ => Err(AccessStoreError::MalformedVocabulary),
        }
    }
}

impl AccessStore {
    pub(crate) async fn artifact_distribution_authority(
        &self,
        identity: VerifiedIdentity,
        project_id: String,
        selected_team_id: Option<String>,
    ) -> AccessStoreResult<ArtifactDistributionAuthoritySnapshot> {
        validate_text(&project_id, 256)?;
        if let Some(team_id) = selected_team_id.as_deref() {
            validate_text(team_id, 256)?;
        }
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(map_sqlite_error)?;
            let discover = authorize_library_in_transaction(
                &transaction,
                &identity,
                &project_id,
                Permission::AssetDiscover,
            )?;
            if selected_team_id
                .as_deref()
                .is_some_and(|team_id| !discover.team_ids.iter().any(|id| id == team_id))
            {
                return Err(AccessStoreError::NotAuthorized);
            }

            let mut grants = ArtifactDistributionGrants::default();
            for permission in ArtifactDistributionPermission::ALL {
                match authorize_library_in_transaction(
                    &transaction,
                    &identity,
                    &project_id,
                    permission.project_permission(),
                ) {
                    Ok(snapshot) => {
                        if snapshot.principal_id != discover.principal_id
                            || snapshot.organization_id != discover.organization_id
                            || snapshot.project_id != discover.project_id
                            || snapshot.global_revision != discover.global_revision
                        {
                            return Err(AccessStoreError::IntegrityViolation {
                                check: "artifact_distribution_authority_snapshot",
                            });
                        }
                        grants.set(permission, true);
                    }
                    Err(AccessStoreError::NotAuthorized) => grants.set(permission, false),
                    Err(error) => return Err(error),
                }
            }
            let snapshot = ArtifactDistributionAuthoritySnapshot {
                principal_id: discover.principal_id,
                organization_id: discover.organization_id,
                project_id: discover.project_id,
                grants,
                global_revision: discover.global_revision,
            };
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(snapshot)
        })
        .await
    }

    pub(crate) async fn stage_artifact_authority(
        &self,
        input: StageArtifactAuthority,
    ) -> AccessStoreResult<ArtifactAuthorityRecord> {
        validate_text(&input.artifact_id, 2048)?;
        validate_text(&input.operation_id, 256)?;
        let epoch = validate_epoch(input.policy_epoch)?;
        let (owner_kind, owner_id) = owner_parts(&input.owner)?;
        validate_text(owner_id, 256)?;
        let artifact_id = input.artifact_id.clone();
        let operation_id = input.operation_id.clone();
        let owner_kind = owner_kind.to_owned();
        let owner_id = owner_id.to_owned();
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let existing_id = transaction
                .query_row(
                    "SELECT artifact_id FROM artifact_authorities WHERE artifact_id=?1 OR operation_id=?2 LIMIT 1",
                    params![artifact_id, operation_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_error)?;
            if let Some(existing_id) = existing_id {
                let existing = query_artifact_authority(&transaction, &existing_id)?
                    .ok_or(AccessStoreError::ArtifactDistributionConflict)?;
                let same = existing.artifact_id == artifact_id
                    && existing.operation_id == operation_id
                    && owner_parts(&existing.owner)? == (owner_kind.as_str(), owner_id.as_str())
                    && existing.policy_epoch == input.policy_epoch;
                if !same {
                    return Err(AccessStoreError::ArtifactDistributionConflict);
                }
                transaction.commit().map_err(map_sqlite_error)?;
                return Ok(existing);
            }
            let current_revision: i64 = transaction
                .query_row(
                    "SELECT global_revision FROM access_metadata WHERE singleton=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(map_sqlite_error)?;
            if current_revision != epoch {
                return Err(AccessStoreError::NotAuthorized);
            }
            transaction
                .execute(
                    "INSERT INTO artifact_authorities(artifact_id,operation_id,owner_kind,owner_id,status,policy_epoch,created_at,updated_at) VALUES(?1,?2,?3,?4,'pending',?5,?6,?6)",
                    params![artifact_id, operation_id, owner_kind, owner_id, epoch, input.now],
                )
                .map_err(map_sqlite_error)?;
            let authority = query_artifact_authority(&transaction, &artifact_id)?
                .ok_or(AccessStoreError::ArtifactDistributionConflict)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(authority)
        })
        .await
    }

    pub(crate) async fn artifact_authority(
        &self,
        artifact_id: String,
    ) -> AccessStoreResult<Option<ArtifactAuthorityRecord>> {
        validate_text(&artifact_id, 2048)?;
        self.with_connection(move |connection| query_artifact_authority(connection, &artifact_id))
            .await
    }

    pub(crate) async fn mark_artifact_authority_committing(
        &self,
        artifact_id: String,
        operation_id: String,
        now: i64,
    ) -> AccessStoreResult<ArtifactAuthorityRecord> {
        transition_artifact_authority(
            self,
            artifact_id,
            operation_id,
            None,
            ArtifactAuthorityStatus::Committing,
            now,
        )
        .await
    }

    pub(crate) async fn activate_artifact_authority(
        &self,
        artifact_id: String,
        operation_id: String,
        policy_epoch: u64,
        now: i64,
    ) -> AccessStoreResult<ArtifactAuthorityRecord> {
        let epoch = validate_epoch(policy_epoch)?;
        transition_artifact_authority(
            self,
            artifact_id,
            operation_id,
            Some(epoch),
            ArtifactAuthorityStatus::Active,
            now,
        )
        .await
    }

    pub(crate) async fn fail_artifact_authority(
        &self,
        artifact_id: String,
        operation_id: String,
        now: i64,
    ) -> AccessStoreResult<ArtifactAuthorityRecord> {
        transition_artifact_authority(
            self,
            artifact_id,
            operation_id,
            None,
            ArtifactAuthorityStatus::Failed,
            now,
        )
        .await
    }

    pub(crate) async fn put_artifact_publisher_policy(
        &self,
        record: ArtifactPublisherPolicyRecord,
    ) -> AccessStoreResult<ArtifactPublisherPolicyRecord> {
        validate_text(&record.artifact_id, 2048)?;
        let epoch = validate_epoch(record.policy_epoch)?;
        let artifact_id = record.artifact_id.clone();
        let ceiling = record.ceiling;
        let updated_at = record.updated_at;
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let authority = query_artifact_authority(&transaction, &artifact_id)?
                .ok_or(AccessStoreError::ArtifactDistributionConflict)?;
            if authority.status != ArtifactAuthorityStatus::Active {
                return Err(AccessStoreError::ArtifactMirrorStateConflict);
            }
            let current = transaction
                .query_row(
                    "SELECT policy_epoch FROM artifact_publisher_policies WHERE artifact_id=?1",
                    [&artifact_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(map_sqlite_error)?;
            if current.is_some_and(|current| epoch < current) {
                return Err(AccessStoreError::ArtifactDistributionConflict);
            }
            transaction
                .execute(
                    "INSERT INTO artifact_publisher_policies(artifact_id,policy_epoch,allow_sync,allow_follow,allow_fork,allow_export,allow_reshare,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(artifact_id) DO UPDATE SET policy_epoch=excluded.policy_epoch,allow_sync=excluded.allow_sync,allow_follow=excluded.allow_follow,allow_fork=excluded.allow_fork,allow_export=excluded.allow_export,allow_reshare=excluded.allow_reshare,updated_at=excluded.updated_at",
                    params![artifact_id, epoch, ceiling.sync, ceiling.follow, ceiling.fork, ceiling.export, ceiling.reshare, updated_at],
                )
                .map_err(map_sqlite_error)?;
            let _global_revision = bump_global_revision(&transaction, updated_at)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(ArtifactPublisherPolicyRecord {
                artifact_id,
                policy_epoch: record.policy_epoch,
                ceiling,
                updated_at,
            })
        })
        .await
    }

    pub(crate) async fn put_artifact_source_policy(
        &self,
        record: ArtifactSourcePolicyRecord,
    ) -> AccessStoreResult<ArtifactSourcePolicyRecord> {
        validate_text(&record.provider_authority, 512)?;
        validate_text(&record.artifact_id, 2048)?;
        let epoch = validate_epoch(record.policy_epoch)?;
        let (scope_kind, scope_id) = owner_parts(&record.source_scope)?;
        validate_text(scope_id, 256)?;
        let provider = record.provider_authority.clone();
        let artifact = record.artifact_id.clone();
        let scope_kind = scope_kind.to_owned();
        let scope_id = scope_id.to_owned();
        let ceiling = record.ceiling;
        let updated_at = record.updated_at;
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let current = transaction
                .query_row(
                    "SELECT source_scope_kind,source_scope_id,policy_epoch FROM artifact_source_policies WHERE provider_authority=?1 AND artifact_id=?2",
                    params![provider, artifact],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
                )
                .optional()
                .map_err(map_sqlite_error)?;
            if let Some((existing_kind, existing_id, existing_epoch)) = current
                && (existing_kind != scope_kind || existing_id != scope_id || epoch < existing_epoch)
            {
                return Err(AccessStoreError::ArtifactDistributionConflict);
            }
            transaction
                .execute(
                    "INSERT INTO artifact_source_policies(provider_authority,artifact_id,source_scope_kind,source_scope_id,policy_epoch,allow_sync,allow_follow,allow_fork,allow_export,allow_reshare,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(provider_authority,artifact_id) DO UPDATE SET policy_epoch=excluded.policy_epoch,allow_sync=excluded.allow_sync,allow_follow=excluded.allow_follow,allow_fork=excluded.allow_fork,allow_export=excluded.allow_export,allow_reshare=excluded.allow_reshare,updated_at=excluded.updated_at",
                    params![provider, artifact, scope_kind, scope_id, epoch, ceiling.sync, ceiling.follow, ceiling.fork, ceiling.export, ceiling.reshare, updated_at],
                )
                .map_err(map_sqlite_error)?;
            let _global_revision = bump_global_revision(&transaction, updated_at)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(ArtifactSourcePolicyRecord {
                provider_authority: provider,
                artifact_id: artifact,
                source_scope: owner_from_parts(&scope_kind, scope_id)?,
                policy_epoch: record.policy_epoch,
                ceiling,
                updated_at,
            })
        })
        .await
    }

    pub(crate) async fn put_artifact_assignment_distribution(
        &self,
        record: ArtifactAssignmentDistributionRecord,
    ) -> AccessStoreResult<ArtifactAssignmentDistributionRecord> {
        validate_text(&record.assignment_id, 256)?;
        validate_text(&record.provider_authority, 512)?;
        validate_text(&record.artifact_id, 2048)?;
        let epoch = validate_epoch(record.policy_epoch)?;
        let (scope_kind, scope_id) = owner_parts(&record.source_scope)?;
        validate_text(scope_id, 256)?;
        let assignment_id = record.assignment_id.clone();
        let provider = record.provider_authority.clone();
        let artifact = record.artifact_id.clone();
        let scope_kind = scope_kind.to_owned();
        let scope_id = scope_id.to_owned();
        let ceiling = record.ceiling;
        let active = record.active;
        let created_at = record.created_at;
        let updated_at = record.updated_at;
        self.with_connection(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let existing = transaction.query_row(
                "SELECT provider_authority,artifact_id,source_scope_kind,source_scope_id,policy_epoch,created_at FROM artifact_assignment_distributions WHERE assignment_id=?1",
                [&assignment_id],
                |row| Ok((row.get::<_, String>(0)?,row.get::<_, String>(1)?,row.get::<_, String>(2)?,row.get::<_, String>(3)?,row.get::<_, i64>(4)?,row.get::<_, i64>(5)?)),
            ).optional().map_err(map_sqlite_error)?;
            let persisted_created_at = if let Some((ep,ea,esk,esi,ee,created)) = existing {
                if ep != provider || ea != artifact || esk != scope_kind || esi != scope_id || epoch < ee {
                    return Err(AccessStoreError::ArtifactDistributionConflict);
                }
                created
            } else { created_at };
            transaction.execute(
                "INSERT INTO artifact_assignment_distributions(assignment_id,provider_authority,artifact_id,source_scope_kind,source_scope_id,policy_epoch,allow_sync,allow_follow,allow_fork,allow_export,allow_reshare,status,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) ON CONFLICT(assignment_id) DO UPDATE SET policy_epoch=excluded.policy_epoch,allow_sync=excluded.allow_sync,allow_follow=excluded.allow_follow,allow_fork=excluded.allow_fork,allow_export=excluded.allow_export,allow_reshare=excluded.allow_reshare,status=excluded.status,updated_at=excluded.updated_at",
                params![assignment_id,provider,artifact,scope_kind,scope_id,epoch,ceiling.sync,ceiling.follow,ceiling.fork,ceiling.export,ceiling.reshare,if active {"active"} else {"revoked"},persisted_created_at,updated_at],
            ).map_err(map_sqlite_error)?;
            let _global_revision = bump_global_revision(&transaction, updated_at)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(ArtifactAssignmentDistributionRecord { assignment_id, provider_authority: provider, artifact_id: artifact, source_scope: owner_from_parts(&scope_kind, scope_id)?, policy_epoch: record.policy_epoch, ceiling, active, created_at: persisted_created_at, updated_at })
        }).await
    }

    pub(crate) async fn artifact_assignment_distribution(
        &self,
        assignment_id: String,
        provider_authority: String,
        artifact_id: String,
    ) -> AccessStoreResult<Option<ArtifactAssignmentDistributionRecord>> {
        validate_text(&assignment_id, 256)?;
        validate_text(&provider_authority, 512)?;
        validate_text(&artifact_id, 2048)?;
        self.with_connection(move |connection| {
            connection
                .query_row(
                    "SELECT source_scope_kind,source_scope_id,policy_epoch,allow_sync,allow_follow,allow_fork,allow_export,allow_reshare,created_at,updated_at FROM artifact_assignment_distributions WHERE assignment_id=?1 AND provider_authority=?2 AND artifact_id=?3 AND status='active'",
                    params![assignment_id, provider_authority, artifact_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            ArtifactDistributionCeiling {
                                sync: row.get(3)?,
                                follow: row.get(4)?,
                                fork: row.get(5)?,
                                export: row.get(6)?,
                                reshare: row.get(7)?,
                            },
                            row.get::<_, i64>(8)?,
                            row.get::<_, i64>(9)?,
                        ))
                    },
                )
                .optional()
                .map_err(map_sqlite_error)?
                .map(|(scope_kind, scope_id, policy_epoch, ceiling, created_at, updated_at)| {
                    Ok(ArtifactAssignmentDistributionRecord {
                        assignment_id,
                        provider_authority,
                        artifact_id,
                        source_scope: owner_from_parts(&scope_kind, scope_id)?,
                        policy_epoch: u64::try_from(policy_epoch)
                            .map_err(|_| AccessStoreError::MalformedVocabulary)?,
                        ceiling,
                        active: true,
                        created_at,
                        updated_at,
                    })
                })
                .transpose()
        })
        .await
    }

    pub(crate) async fn artifact_transfer_options(
        &self,
        provider_authority: String,
        artifact_id: String,
        assignment_id: Option<String>,
        grants: ArtifactDistributionGrants,
        publication: ArtifactPublication,
        license: ArtifactLicenseState,
        destination: Option<ArtifactDestinationPolicy>,
    ) -> AccessStoreResult<ArtifactTransferOptions> {
        validate_text(&provider_authority, 512)?;
        validate_text(&artifact_id, 2048)?;
        if let Some(value) = assignment_id.as_deref() {
            validate_text(value, 256)?;
        }
        self.with_connection(move |connection| {
            let publisher = connection.query_row(
                "SELECT allow_sync,allow_follow,allow_fork,allow_export,allow_reshare FROM artifact_source_policies WHERE provider_authority=?1 AND artifact_id=?2",
                params![provider_authority,artifact_id],
                |row| Ok(ArtifactDistributionCeiling { sync: row.get(0)?, follow: row.get(1)?, fork: row.get(2)?, export: row.get(3)?, reshare: row.get(4)? }),
            ).optional().map_err(map_sqlite_error)?.unwrap_or_default();
            let assignment = if let Some(assignment_id) = assignment_id {
                connection.query_row(
                    "SELECT allow_sync,allow_follow,allow_fork,allow_export,allow_reshare FROM artifact_assignment_distributions WHERE assignment_id=?1 AND provider_authority=?2 AND artifact_id=?3 AND status='active'",
                    params![assignment_id,provider_authority,artifact_id],
                    |row| Ok(ArtifactDistributionCeiling { sync: row.get(0)?, follow: row.get(1)?, fork: row.get(2)?, export: row.get(3)?, reshare: row.get(4)? }),
                ).optional().map_err(map_sqlite_error)?
            } else { None };
            Ok(evaluate_transfer_options(ArtifactTransferFacts { grants, publisher, assignment, publication: &publication, license: &license, destination }))
        }).await
    }

    pub(crate) async fn managed_artifact_policy_decision(
        &self,
        provider_authority: String,
        artifact_id: String,
        assignment_id: String,
        grants: ArtifactDistributionGrants,
        mode: ArtifactTransferMode,
    ) -> AccessStoreResult<ArtifactTransferDecision> {
        let publication = ArtifactPublication {
            state: PublicationState::Published,
            distribution: Distribution::Bytes,
            ..ArtifactPublication::default()
        };
        let license = ArtifactLicenseState {
            redistribution: Redistribution::Redistributable,
            ..ArtifactLicenseState::default()
        };
        self.artifact_transfer_options(
            provider_authority,
            artifact_id,
            Some(assignment_id),
            grants,
            publication,
            license,
            Some(ArtifactDestinationPolicy::local_personal()),
        )
        .await
        .map(|options| options.decision(mode))
    }

    pub(crate) async fn stage_managed_artifact_mirror(
        &self,
        input: StageManagedArtifactMirror,
    ) -> AccessStoreResult<ManagedArtifactMirror> {
        validate_text(&input.mirror_id, 256)?;
        validate_text(&input.operation_id, 256)?;
        validate_text(&input.owner_principal_id, 256)?;
        validate_text(&input.authorization_project_id, 256)?;
        if let Some(team_id) = input.authorization_team_id.as_deref() {
            validate_text(team_id, 256)?;
        }
        if input.identity_ref_json.is_empty() || input.identity_ref_json.len() > 16_384 {
            return Err(AccessStoreError::InvalidArtifactDistributionInput);
        }
        serde_json::from_str::<super::DurableIdentityReference>(&input.identity_ref_json)
            .map_err(|_| AccessStoreError::InvalidArtifactDistributionInput)?;
        if let Some(destination) = input.destination_id.as_deref() {
            validate_text(destination, 256)?;
        }
        validate_text(&input.source_provider_authority, 512)?;
        validate_text(&input.source_assignment_id, 256)?;
        validate_text(&input.source_artifact_id, 2048)?;
        validate_text(&input.source_revision_id, 2048)?;
        validate_text(&input.local_artifact_id, 2048)?;
        let epoch = validate_epoch(input.last_authorized_policy_epoch)?;
        let (scope_kind, scope_id) = owner_parts(&input.source_scope)?;
        validate_text(scope_id, 256)?;
        let scope_kind = scope_kind.to_owned();
        let scope_id = scope_id.to_owned();
        self.with_connection(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let current_global_revision: i64 = transaction
                .query_row("SELECT global_revision FROM access_metadata WHERE singleton=1", [], |row| row.get(0))
                .map_err(map_sqlite_error)?;
            if current_global_revision != epoch {
                return Err(AccessStoreError::NotAuthorized);
            }
            let assignment_matches: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM artifact_assignment_distributions WHERE assignment_id=?1 AND provider_authority=?2 AND artifact_id=?3 AND source_scope_kind=?4 AND source_scope_id=?5 AND status='active')",
                params![input.source_assignment_id,input.source_provider_authority,input.source_artifact_id,scope_kind,scope_id],
                |row| row.get(0),
            ).map_err(map_sqlite_error)?;
            if !assignment_matches {
                return Err(AccessStoreError::NotAuthorized);
            }
            let existing_id = transaction.query_row(
                "SELECT mirror_id FROM artifact_mirrors WHERE mirror_id=?1 OR operation_id=?2 LIMIT 1",
                params![input.mirror_id,input.operation_id], |row| row.get::<_, String>(0)
            ).optional().map_err(map_sqlite_error)?;
            if let Some(existing_id) = existing_id {
                let existing = query_mirror(&transaction, &existing_id)?.ok_or(AccessStoreError::ArtifactMirrorUnavailable)?;
                let same = existing.mirror_id == input.mirror_id
                    && existing.operation_id == input.operation_id
                    && existing.owner_principal_id == input.owner_principal_id
                    && existing.identity_ref_json == input.identity_ref_json
                    && existing.authorization_project_id == input.authorization_project_id
                    && existing.authorization_team_id == input.authorization_team_id
                    && existing.destination_id == input.destination_id
                    && existing.source_provider_authority == input.source_provider_authority
                    && existing.source_assignment_id == input.source_assignment_id
                    && existing.source_scope == input.source_scope
                    && existing.source_artifact_id == input.source_artifact_id
                    && existing.source_revision_id == input.source_revision_id
                    && existing.local_artifact_id == input.local_artifact_id
                    && existing.mode == input.mode;
                if !same { return Err(AccessStoreError::ArtifactDistributionConflict); }
                transaction.commit().map_err(map_sqlite_error)?;
                return Ok(existing);
            }
            transaction.execute(
                "INSERT INTO artifact_mirrors(mirror_id,operation_id,owner_principal_id,identity_ref_json,authorization_project_id,authorization_team_id,destination_id,source_provider_authority,source_assignment_id,source_scope_kind,source_scope_id,source_artifact_id,source_revision_id,local_artifact_id,local_revision_id,mode,status,last_authorized_policy_epoch,last_checked_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,NULL,?15,'pending',?16,NULL,?17,?17)",
                params![input.mirror_id,input.operation_id,input.owner_principal_id,input.identity_ref_json,input.authorization_project_id,input.authorization_team_id,input.destination_id,input.source_provider_authority,input.source_assignment_id,scope_kind,scope_id,input.source_artifact_id,input.source_revision_id,input.local_artifact_id,input.mode.as_wire(),epoch,input.now],
            ).map_err(map_sqlite_error)?;
            let mirror = query_mirror(&transaction, &input.mirror_id)?.ok_or(AccessStoreError::ArtifactMirrorUnavailable)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(mirror)
        }).await
    }

    pub(crate) async fn managed_artifact_mirror(
        &self,
        mirror_id: String,
    ) -> AccessStoreResult<Option<ManagedArtifactMirror>> {
        validate_text(&mirror_id, 256)?;
        self.with_connection(move |connection| query_mirror(connection, &mirror_id))
            .await
    }

    pub(crate) async fn managed_artifact_mirrors_for_reconciliation(
        &self,
        checked_before: i64,
        limit: usize,
    ) -> AccessStoreResult<Vec<ManagedArtifactMirror>> {
        if !(1..=64).contains(&limit) {
            return Err(AccessStoreError::InvalidArtifactDistributionInput);
        }
        let limit =
            i64::try_from(limit).map_err(|_| AccessStoreError::InvalidArtifactDistributionInput)?;
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT mirror_id,operation_id,owner_principal_id,identity_ref_json,authorization_project_id,authorization_team_id,destination_id,source_provider_authority,source_assignment_id,source_scope_kind,source_scope_id,source_artifact_id,source_revision_id,local_artifact_id,local_revision_id,mode,status,last_authorized_policy_epoch,last_checked_at,created_at,updated_at
                     FROM artifact_mirrors
                     WHERE status IN ('active','committing')
                       AND (last_checked_at IS NULL OR last_checked_at<=?1)
                     ORDER BY COALESCE(last_checked_at,-9223372036854775808),mirror_id
                     LIMIT ?2",
                )
                .map_err(map_sqlite_error)?;
            let rows = statement
                .query_map(params![checked_before, limit], |row| {
                    Ok(RawMirror {
                        mirror_id: row.get(0)?,
                        operation_id: row.get(1)?,
                        owner_principal_id: row.get(2)?,
                        identity_ref_json: row.get(3)?,
                        authorization_project_id: row.get(4)?,
                        authorization_team_id: row.get(5)?,
                        destination_id: row.get(6)?,
                        source_provider_authority: row.get(7)?,
                        source_assignment_id: row.get(8)?,
                        source_scope_kind: row.get(9)?,
                        source_scope_id: row.get(10)?,
                        source_artifact_id: row.get(11)?,
                        source_revision_id: row.get(12)?,
                        local_artifact_id: row.get(13)?,
                        local_revision_id: row.get(14)?,
                        mode: row.get(15)?,
                        status: row.get(16)?,
                        last_authorized_policy_epoch: row.get(17)?,
                        last_checked_at: row.get(18)?,
                        created_at: row.get(19)?,
                        updated_at: row.get(20)?,
                    })
                })
                .map_err(map_sqlite_error)?;
            let mut mirrors = Vec::new();
            for row in rows {
                mirrors.push(row.map_err(map_sqlite_error)?.into_record()?);
            }
            Ok(mirrors)
        })
        .await
    }

    pub(crate) async fn mark_managed_artifact_mirror_checked(
        &self,
        mirror_id: String,
        checked_at: i64,
    ) -> AccessStoreResult<ManagedArtifactMirror> {
        validate_text(&mirror_id, 256)?;
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let changed = transaction
                .execute(
                    "UPDATE artifact_mirrors
                     SET last_checked_at=?2
                     WHERE mirror_id=?1 AND status IN ('active','committing')",
                    params![mirror_id, checked_at],
                )
                .map_err(map_sqlite_error)?;
            if changed != 1 {
                return Err(AccessStoreError::ArtifactMirrorStateConflict);
            }
            let mirror = query_mirror(&transaction, &mirror_id)?
                .ok_or(AccessStoreError::ArtifactMirrorUnavailable)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(mirror)
        })
        .await
    }

    pub(crate) async fn mark_managed_artifact_mirror_committing(
        &self,
        mirror_id: String,
        operation_id: String,
        now: i64,
    ) -> AccessStoreResult<ManagedArtifactMirror> {
        transition_mirror(
            self,
            mirror_id,
            operation_id,
            None,
            None,
            ManagedArtifactMirrorStatus::Committing,
            now,
        )
        .await
    }

    pub(crate) async fn begin_managed_artifact_mirror_update(
        &self,
        input: BeginManagedArtifactMirrorUpdate,
    ) -> AccessStoreResult<ManagedArtifactMirror> {
        validate_text(&input.mirror_id, 256)?;
        validate_text(&input.operation_id, 256)?;
        validate_text(&input.source_revision_id, 2048)?;
        validate_text(&input.expected_local_revision_id, 2048)?;
        let epoch = validate_epoch(input.policy_epoch)?;
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let current_global_revision: i64 = transaction
                .query_row("SELECT global_revision FROM access_metadata WHERE singleton=1", [], |row| row.get(0))
                .map_err(map_sqlite_error)?;
            if current_global_revision != epoch {
                return Err(AccessStoreError::NotAuthorized);
            }
            let current = query_mirror(&transaction, &input.mirror_id)?
                .ok_or(AccessStoreError::ArtifactMirrorUnavailable)?;

            if current.status == ManagedArtifactMirrorStatus::Committing
                && current.mode == ManagedArtifactMirrorMode::Followed
                && current.operation_id == input.operation_id
                && current.source_revision_id == input.source_revision_id
                && current.local_revision_id.as_deref()
                    == Some(input.expected_local_revision_id.as_str())
                && current.last_authorized_policy_epoch == input.policy_epoch
            {
                transaction.commit().map_err(map_sqlite_error)?;
                return Ok(current);
            }

            if current.status == ManagedArtifactMirrorStatus::Active
                && current.operation_id == input.operation_id
            {
                return Err(AccessStoreError::ArtifactDistributionConflict);
            }
            if current.mode != ManagedArtifactMirrorMode::Followed
                || current.status != ManagedArtifactMirrorStatus::Active
                || current.local_revision_id.as_deref()
                    != Some(input.expected_local_revision_id.as_str())
                || input.policy_epoch < current.last_authorized_policy_epoch
            {
                return Err(AccessStoreError::ArtifactMirrorStateConflict);
            }
            let conflicting_operation: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM artifact_mirrors WHERE operation_id=?1 AND mirror_id<>?2)",
                    params![input.operation_id, input.mirror_id],
                    |row| row.get(0),
                )
                .map_err(map_sqlite_error)?;
            if conflicting_operation {
                return Err(AccessStoreError::ArtifactDistributionConflict);
            }
            transaction
                .execute(
                    "UPDATE artifact_mirrors SET operation_id=?2,source_revision_id=?3,status='committing',last_authorized_policy_epoch=?4,updated_at=?5 WHERE mirror_id=?1",
                    params![input.mirror_id,input.operation_id,input.source_revision_id,epoch,input.now],
                )
                .map_err(map_sqlite_error)?;
            let mirror = query_mirror(&transaction, &input.mirror_id)?
                .ok_or(AccessStoreError::ArtifactMirrorUnavailable)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(mirror)
        })
        .await
    }

    pub(crate) async fn activate_managed_artifact_mirror(
        &self,
        mirror_id: String,
        operation_id: String,
        local_revision_id: String,
        policy_epoch: u64,
        now: i64,
    ) -> AccessStoreResult<ManagedArtifactMirror> {
        validate_text(&local_revision_id, 2048)?;
        let epoch = validate_epoch(policy_epoch)?;
        transition_mirror(
            self,
            mirror_id,
            operation_id,
            Some(local_revision_id),
            Some(epoch),
            ManagedArtifactMirrorStatus::Active,
            now,
        )
        .await
    }

    pub(crate) async fn restrict_managed_artifact_mirror(
        &self,
        mirror_id: String,
        operation_id: String,
        status: ManagedArtifactMirrorStatus,
        now: i64,
    ) -> AccessStoreResult<ManagedArtifactMirror> {
        if !status.is_terminal() {
            return Err(AccessStoreError::ArtifactMirrorStateConflict);
        }
        transition_mirror(self, mirror_id, operation_id, None, None, status, now).await
    }

    pub(crate) async fn put_artifact_subscription(
        &self,
        subscription: ManagedArtifactSubscription,
    ) -> AccessStoreResult<ManagedArtifactSubscription> {
        validate_text(&subscription.mirror_id, 256)?;
        if let Some(value) = subscription.last_observed_revision_id.as_deref() {
            validate_text(value, 2048)?;
        }
        if let Some(value) = subscription.last_applied_revision_id.as_deref() {
            validate_text(value, 2048)?;
        }
        self.with_connection(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let mirror = query_mirror(&transaction, &subscription.mirror_id)?.ok_or(AccessStoreError::ArtifactMirrorUnavailable)?;
            if mirror.mode != ManagedArtifactMirrorMode::Followed || mirror.status != ManagedArtifactMirrorStatus::Active {
                return Err(AccessStoreError::ArtifactMirrorStateConflict);
            }
            transaction.execute(
                "INSERT INTO artifact_subscriptions(mirror_id,update_policy,last_observed_revision_id,last_applied_revision_id,last_checked_at,status,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(mirror_id) DO UPDATE SET update_policy=excluded.update_policy,last_observed_revision_id=excluded.last_observed_revision_id,last_applied_revision_id=excluded.last_applied_revision_id,last_checked_at=excluded.last_checked_at,status=excluded.status,updated_at=excluded.updated_at",
                params![subscription.mirror_id,subscription.update_policy.as_wire(),subscription.last_observed_revision_id,subscription.last_applied_revision_id,subscription.last_checked_at,if subscription.active {"active"} else {"paused"},subscription.updated_at],
            ).map_err(map_sqlite_error)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(subscription)
        }).await
    }

    pub(crate) async fn artifact_subscription(
        &self,
        mirror_id: String,
    ) -> AccessStoreResult<Option<ManagedArtifactSubscription>> {
        validate_text(&mirror_id, 256)?;
        self.with_connection(move |connection| {
            connection.query_row(
                "SELECT update_policy,last_observed_revision_id,last_applied_revision_id,last_checked_at,status,updated_at FROM artifact_subscriptions WHERE mirror_id=?1",
                [&mirror_id],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                )),
            ).optional().map_err(map_sqlite_error)?.map(|(policy,observed,applied,checked,status,updated_at)| {
                Ok(ManagedArtifactSubscription {
                    mirror_id,
                    update_policy: ArtifactSubscriptionUpdatePolicy::from_wire(&policy)?,
                    last_observed_revision_id: observed,
                    last_applied_revision_id: applied,
                    last_checked_at: checked,
                    active: status == "active",
                    updated_at,
                })
            }).transpose()
        }).await
    }

    pub(crate) async fn auto_approved_artifact_subscriptions_due(
        &self,
        checked_before: i64,
        limit: usize,
    ) -> AccessStoreResult<Vec<ManagedArtifactSubscription>> {
        if !(1..=64).contains(&limit) {
            return Err(AccessStoreError::InvalidArtifactDistributionInput);
        }
        let limit =
            i64::try_from(limit).map_err(|_| AccessStoreError::InvalidArtifactDistributionInput)?;
        self.with_connection(move |connection| {
            let mut statement = connection.prepare(
                "SELECT s.mirror_id,s.update_policy,s.last_observed_revision_id,s.last_applied_revision_id,s.last_checked_at,s.status,s.updated_at
                 FROM artifact_subscriptions s
                 JOIN artifact_mirrors m ON m.mirror_id=s.mirror_id
                 WHERE s.status='active'
                   AND s.update_policy='auto_approved'
                   AND m.mode='followed'
                   AND m.status IN ('active','committing')
                   AND (s.last_checked_at IS NULL OR s.last_checked_at<=?1)
                 ORDER BY COALESCE(s.last_checked_at,-9223372036854775808),s.mirror_id
                 LIMIT ?2",
            ).map_err(map_sqlite_error)?;
            let rows = statement.query_map(params![checked_before,limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            }).map_err(map_sqlite_error)?;
            let mut subscriptions = Vec::new();
            for row in rows {
                let (mirror_id,policy,observed,applied,checked,status,updated_at) = row.map_err(map_sqlite_error)?;
                subscriptions.push(ManagedArtifactSubscription {
                    mirror_id,
                    update_policy: ArtifactSubscriptionUpdatePolicy::from_wire(&policy)?,
                    last_observed_revision_id: observed,
                    last_applied_revision_id: applied,
                    last_checked_at: checked,
                    active: status == "active",
                    updated_at,
                });
            }
            Ok(subscriptions)
        }).await
    }

    pub(crate) async fn observe_artifact_subscription(
        &self,
        mirror_id: String,
        observed_revision_id: Option<String>,
        checked_at: i64,
    ) -> AccessStoreResult<ManagedArtifactSubscription> {
        validate_text(&mirror_id, 256)?;
        if let Some(revision_id) = observed_revision_id.as_deref() {
            validate_text(revision_id, 2048)?;
        }
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let mirror = query_mirror(&transaction, &mirror_id)?
                .ok_or(AccessStoreError::ArtifactMirrorUnavailable)?;
            if mirror.mode != ManagedArtifactMirrorMode::Followed
                || mirror.status != ManagedArtifactMirrorStatus::Active
            {
                return Err(AccessStoreError::ArtifactMirrorStateConflict);
            }
            let changed = transaction
                .execute(
                    "UPDATE artifact_subscriptions
                     SET last_observed_revision_id=COALESCE(?2,last_observed_revision_id),
                         last_checked_at=?3,
                         updated_at=?3
                     WHERE mirror_id=?1 AND status='active'",
                    params![mirror_id, observed_revision_id, checked_at],
                )
                .map_err(map_sqlite_error)?;
            if changed != 1 {
                return Err(AccessStoreError::ArtifactMirrorStateConflict);
            }
            let row = transaction
                .query_row(
                    "SELECT update_policy,last_observed_revision_id,last_applied_revision_id,last_checked_at,status,updated_at FROM artifact_subscriptions WHERE mirror_id=?1",
                    [&mirror_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .map_err(map_sqlite_error)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(ManagedArtifactSubscription {
                mirror_id,
                update_policy: ArtifactSubscriptionUpdatePolicy::from_wire(&row.0)?,
                last_observed_revision_id: row.1,
                last_applied_revision_id: row.2,
                last_checked_at: row.3,
                active: row.4 == "active",
                updated_at: row.5,
            })
        })
        .await
    }
}

async fn transition_artifact_authority(
    store: &AccessStore,
    artifact_id: String,
    operation_id: String,
    policy_epoch: Option<i64>,
    target: ArtifactAuthorityStatus,
    now: i64,
) -> AccessStoreResult<ArtifactAuthorityRecord> {
    validate_text(&artifact_id, 2048)?;
    validate_text(&operation_id, 256)?;
    store
        .with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let current = query_artifact_authority(&transaction, &artifact_id)?
                .ok_or(AccessStoreError::ArtifactDistributionConflict)?;
            if current.operation_id != operation_id {
                return Err(AccessStoreError::ArtifactDistributionConflict);
            }
            if current.status == target {
                transaction.commit().map_err(map_sqlite_error)?;
                return Ok(current);
            }
            let allowed = match target {
                ArtifactAuthorityStatus::Committing => {
                    current.status == ArtifactAuthorityStatus::Pending
                }
                ArtifactAuthorityStatus::Active => {
                    current.status == ArtifactAuthorityStatus::Committing && policy_epoch.is_some()
                }
                ArtifactAuthorityStatus::Failed => matches!(
                    current.status,
                    ArtifactAuthorityStatus::Pending | ArtifactAuthorityStatus::Committing
                ),
                ArtifactAuthorityStatus::Pending => false,
            };
            if !allowed {
                return Err(AccessStoreError::ArtifactMirrorStateConflict);
            }
            if target == ArtifactAuthorityStatus::Active {
                let epoch = policy_epoch.ok_or(AccessStoreError::ArtifactDistributionConflict)?;
                let current_epoch = i64::try_from(current.policy_epoch)
                    .map_err(|_| AccessStoreError::MalformedVocabulary)?;
                if epoch != current_epoch {
                    return Err(AccessStoreError::ArtifactDistributionConflict);
                }
                let global_revision: i64 = transaction
                    .query_row(
                        "SELECT global_revision FROM access_metadata WHERE singleton=1",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite_error)?;
                if global_revision != epoch {
                    return Err(AccessStoreError::NotAuthorized);
                }
            }
            transaction
                .execute(
                    "UPDATE artifact_authorities SET status=?2,updated_at=?3 WHERE artifact_id=?1",
                    params![artifact_id, target.as_wire(), now],
                )
                .map_err(map_sqlite_error)?;
            if target == ArtifactAuthorityStatus::Active {
                let _global_revision = bump_global_revision(&transaction, now)?;
            }
            let updated = query_artifact_authority(&transaction, &artifact_id)?
                .ok_or(AccessStoreError::ArtifactDistributionConflict)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(updated)
        })
        .await
}

async fn transition_mirror(
    store: &AccessStore,
    mirror_id: String,
    operation_id: String,
    local_revision_id: Option<String>,
    policy_epoch: Option<i64>,
    target: ManagedArtifactMirrorStatus,
    now: i64,
) -> AccessStoreResult<ManagedArtifactMirror> {
    validate_text(&mirror_id, 256)?;
    validate_text(&operation_id, 256)?;
    store.with_connection(move |connection| {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
        let current = query_mirror(&transaction, &mirror_id)?.ok_or(AccessStoreError::ArtifactMirrorUnavailable)?;
        if current.operation_id != operation_id { return Err(AccessStoreError::ArtifactDistributionConflict); }
        if current.status == target {
            if target == ManagedArtifactMirrorStatus::Active && current.local_revision_id != local_revision_id {
                return Err(AccessStoreError::ArtifactDistributionConflict);
            }
            transaction.commit().map_err(map_sqlite_error)?;
            return Ok(current);
        }
        let allowed = match target {
            ManagedArtifactMirrorStatus::Committing => current.status == ManagedArtifactMirrorStatus::Pending,
            ManagedArtifactMirrorStatus::Active => current.status == ManagedArtifactMirrorStatus::Committing && local_revision_id.is_some(),
            ManagedArtifactMirrorStatus::Removed => current.status != ManagedArtifactMirrorStatus::Removed,
            ManagedArtifactMirrorStatus::AccessRevoked | ManagedArtifactMirrorStatus::SourceWithdrawn | ManagedArtifactMirrorStatus::Failed => !current.status.is_terminal(),
            ManagedArtifactMirrorStatus::Pending => false,
        };
        if !allowed { return Err(AccessStoreError::ArtifactMirrorStateConflict); }
        if let Some(epoch) = policy_epoch {
            let authorized_epoch = i64::try_from(current.last_authorized_policy_epoch)
                .map_err(|_| AccessStoreError::MalformedVocabulary)?;
            if epoch != authorized_epoch {
                return Err(AccessStoreError::ArtifactDistributionConflict);
            }
            if target == ManagedArtifactMirrorStatus::Active {
                let global_revision: i64 = transaction
                    .query_row(
                        "SELECT global_revision FROM access_metadata WHERE singleton=1",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite_error)?;
                if global_revision != epoch {
                    return Err(AccessStoreError::NotAuthorized);
                }
                let (scope_kind, scope_id) = owner_parts(&current.source_scope)?;
                let assignment_matches: bool = transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM artifact_assignment_distributions WHERE assignment_id=?1 AND provider_authority=?2 AND artifact_id=?3 AND source_scope_kind=?4 AND source_scope_id=?5 AND status='active')",
                        params![current.source_assignment_id,current.source_provider_authority,current.source_artifact_id,scope_kind,scope_id],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite_error)?;
                if !assignment_matches {
                    return Err(AccessStoreError::NotAuthorized);
                }
            }
        }
        transaction.execute(
            "UPDATE artifact_mirrors SET status=?2,local_revision_id=COALESCE(?3,local_revision_id),last_authorized_policy_epoch=COALESCE(?4,last_authorized_policy_epoch),updated_at=?5 WHERE mirror_id=?1",
            params![mirror_id,target.as_wire(),local_revision_id,policy_epoch,now],
        ).map_err(map_sqlite_error)?;
        if target.is_terminal() {
            transaction
                .execute(
                    "UPDATE artifact_subscriptions SET status='paused',updated_at=?2 WHERE mirror_id=?1",
                    params![mirror_id, now],
                )
                .map_err(map_sqlite_error)?;
        }
        let updated = query_mirror(&transaction, &mirror_id)?.ok_or(AccessStoreError::ArtifactMirrorUnavailable)?;
        transaction.commit().map_err(map_sqlite_error)?;
        Ok(updated)
    }).await
}

#[cfg(test)]
mod tests {
    use labby_auth::Authenticator;

    use super::*;

    fn durable_identity_json() -> String {
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "artifact-owner",
        )
        .unwrap();
        serde_json::to_string(&crate::access::DurableIdentityReference::capture(&identity)).unwrap()
    }

    fn transferable() -> (ArtifactPublication, ArtifactLicenseState) {
        let publication = ArtifactPublication {
            state: PublicationState::Published,
            distribution: Distribution::Bytes,
            ..ArtifactPublication::default()
        };
        let license = ArtifactLicenseState {
            redistribution: Redistribution::Forkable,
            ..ArtifactLicenseState::default()
        };
        (publication, license)
    }

    fn open_ceiling() -> ArtifactDistributionCeiling {
        ArtifactDistributionCeiling {
            sync: true,
            follow: true,
            fork: true,
            export: true,
            reshare: true,
        }
    }

    fn all_grants() -> ArtifactDistributionGrants {
        ArtifactDistributionGrants {
            use_remote: true,
            sync: true,
            follow: true,
            fork: true,
            export: true,
            reshare: true,
        }
    }

    fn facts<'a>(
        publication: &'a ArtifactPublication,
        license: &'a ArtifactLicenseState,
    ) -> ArtifactTransferFacts<'a> {
        ArtifactTransferFacts {
            grants: all_grants(),
            publisher: open_ceiling(),
            assignment: Some(open_ceiling()),
            publication,
            license,
            destination: Some(ArtifactDestinationPolicy {
                active: true,
                pin: true,
                follow: true,
                fork: true,
                reshare: true,
            }),
        }
    }

    #[test]
    fn artifact_permission_wire_names_are_frozen() {
        assert_eq!(
            ArtifactDistributionPermission::Use.as_wire(),
            "artifact.use"
        );
        assert_eq!(
            ArtifactDistributionPermission::Sync.as_wire(),
            "artifact.sync"
        );
        assert_eq!(
            ArtifactDistributionPermission::Follow.as_wire(),
            "artifact.follow"
        );
        assert_eq!(
            ArtifactDistributionPermission::Fork.as_wire(),
            "artifact.fork"
        );
        assert_eq!(
            ArtifactDistributionPermission::Export.as_wire(),
            "artifact.export"
        );
        assert_eq!(
            ArtifactDistributionPermission::Reshare.as_wire(),
            "artifact.reshare"
        );
    }

    #[test]
    fn use_without_sync_only_allows_remote_use() {
        let (publication, license) = transferable();
        let mut input = facts(&publication, &license);
        input.grants = ArtifactDistributionGrants {
            use_remote: true,
            ..ArtifactDistributionGrants::default()
        };
        let options = evaluate_transfer_options(input);
        assert!(options.allows(ArtifactTransferMode::UseRemote));
        assert!(!options.allows(ArtifactTransferMode::Pin));
        assert!(!options.allows(ArtifactTransferMode::Follow));
        assert!(!options.allows(ArtifactTransferMode::Fork));
        assert!(!options.allows(ArtifactTransferMode::Export));
        assert!(!options.allows(ArtifactTransferMode::Reshare));
    }

    #[test]
    fn sync_without_fork_never_creates_detached_fork() {
        let (publication, license) = transferable();
        let mut input = facts(&publication, &license);
        input.grants.fork = false;
        let options = evaluate_transfer_options(input);
        assert!(options.allows(ArtifactTransferMode::Pin));
        assert!(!options.allows(ArtifactTransferMode::Fork));
        assert_eq!(
            options.decision(ArtifactTransferMode::Fork).denied_by,
            Some(ArtifactTransferDenyReason::CallerPermission)
        );
    }

    #[test]
    fn follow_requires_both_follow_and_sync_permission() {
        let (publication, license) = transferable();
        let mut input = facts(&publication, &license);
        input.grants.sync = false;
        let options = evaluate_transfer_options(input);
        assert!(!options.allows(ArtifactTransferMode::Follow));
        assert_eq!(
            options.decision(ArtifactTransferMode::Follow).denied_by,
            Some(ArtifactTransferDenyReason::CallerPermission)
        );
    }

    #[test]
    fn absent_assignment_policy_fails_closed_for_byte_movement() {
        let (publication, license) = transferable();
        let mut input = facts(&publication, &license);
        input.assignment = None;
        let options = evaluate_transfer_options(input);
        assert!(options.allows(ArtifactTransferMode::UseRemote));
        for mode in [
            ArtifactTransferMode::Pin,
            ArtifactTransferMode::Follow,
            ArtifactTransferMode::Fork,
            ArtifactTransferMode::Export,
            ArtifactTransferMode::Reshare,
        ] {
            assert!(!options.allows(mode));
            assert_eq!(
                options.decision(mode).denied_by,
                Some(ArtifactTransferDenyReason::AssignmentPolicy)
            );
        }
    }

    #[test]
    fn unknown_redistribution_blocks_all_byte_transfer() {
        let (publication, mut license) = transferable();
        license.redistribution = Redistribution::Unknown;
        let options = evaluate_transfer_options(facts(&publication, &license));
        for mode in [
            ArtifactTransferMode::Pin,
            ArtifactTransferMode::Follow,
            ArtifactTransferMode::Fork,
            ArtifactTransferMode::Export,
            ArtifactTransferMode::Reshare,
        ] {
            assert!(!options.allows(mode));
            assert_eq!(
                options.decision(mode).denied_by,
                Some(ArtifactTransferDenyReason::LicensePolicy)
            );
        }
    }

    #[test]
    fn redistributable_is_not_implicitly_forkable() {
        let (publication, mut license) = transferable();
        license.redistribution = Redistribution::Redistributable;
        let options = evaluate_transfer_options(facts(&publication, &license));
        assert!(options.allows(ArtifactTransferMode::Pin));
        assert!(options.allows(ArtifactTransferMode::Export));
        assert!(!options.allows(ArtifactTransferMode::Fork));
        assert_eq!(
            options.decision(ArtifactTransferMode::Fork).denied_by,
            Some(ArtifactTransferDenyReason::LicensePolicy)
        );
    }

    #[test]
    fn restricted_takedown_wins_over_every_other_allow() {
        let (publication, mut license) = transferable();
        license.takedown_state = TakedownState::Restricted;
        let options = evaluate_transfer_options(facts(&publication, &license));
        for mode in ArtifactTransferMode::ALL {
            assert!(!options.allows(mode));
            assert_eq!(
                options.decision(mode).denied_by,
                Some(ArtifactTransferDenyReason::Takedown)
            );
        }
    }

    #[test]
    fn withdrawn_artifact_cannot_be_used_or_transferred() {
        let (mut publication, license) = transferable();
        publication.state = PublicationState::Withdrawn;
        let options = evaluate_transfer_options(facts(&publication, &license));
        for mode in ArtifactTransferMode::ALL {
            assert!(!options.allows(mode));
            assert_eq!(
                options.decision(mode).denied_by,
                Some(ArtifactTransferDenyReason::PublicationState)
            );
        }
    }

    #[test]
    fn destination_capability_can_narrow_an_otherwise_allowed_transfer() {
        let (publication, license) = transferable();
        let mut input = facts(&publication, &license);
        input.destination = Some(ArtifactDestinationPolicy {
            active: true,
            pin: true,
            follow: false,
            fork: true,
            reshare: true,
        });
        let options = evaluate_transfer_options(input);
        assert!(options.allows(ArtifactTransferMode::Pin));
        assert!(!options.allows(ArtifactTransferMode::Follow));
        assert_eq!(
            options.decision(ArtifactTransferMode::Follow).denied_by,
            Some(ArtifactTransferDenyReason::DestinationPolicy)
        );
    }

    async fn bootstrapped_store() -> (tempfile::TempDir, AccessStore) {
        use labby_auth::{Authenticator, VerifiedIdentity};

        let directory = crate::access::test_support::secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "artifact-owner",
        )
        .unwrap();
        store
            .bootstrap_owner(
                crate::access::BootstrapOwnerInput::new(identity, "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        (directory, store)
    }

    fn personal_owner() -> OwnerScope {
        OwnerScope::Personal(PrincipalId::new("bootstrap-owner").unwrap())
    }

    fn external_identity(subject: &str) -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            subject,
        )
        .unwrap()
    }

    async fn install_distribution_role_fixture(store: &AccessStore) {
        store
            .execute_test_statement(
                "INSERT INTO principals VALUES('distribution-user','bootstrap-local','user','active','Distribution User',30,30);
                 INSERT INTO principal_links VALUES('distribution-user-link','distribution-user','external','https://accounts.google.com','distribution-user',NULL,'active',1,1,30,30);
                 INSERT INTO projects VALUES
                   ('distribution-admin','bootstrap-local','Distribution Admin','active',0,30,30),
                   ('distribution-member','bootstrap-local','Distribution Member','active',0,30,30),
                   ('distribution-viewer','bootstrap-local','Distribution Viewer','active',0,30,30);
                 INSERT INTO project_memberships VALUES
                   ('distribution-admin-membership','bootstrap-local','distribution-admin','distribution-user','admin','active','bootstrap-owner',30,30),
                   ('distribution-member-membership','bootstrap-local','distribution-member','distribution-user','member','active','bootstrap-owner',30,30),
                   ('distribution-viewer-membership','bootstrap-local','distribution-viewer','distribution-user','viewer','active','bootstrap-owner',30,30);
                 INSERT INTO project_loadouts VALUES
                   ('bootstrap-local','distribution-admin','production','bootstrap-owner',30,30),
                   ('bootstrap-local','distribution-member','production','bootstrap-owner',30,30),
                   ('bootstrap-local','distribution-viewer','production','bootstrap-owner',30,30);",
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn distribution_grants_are_derived_from_current_project_authority() {
        let (_directory, store) = bootstrapped_store().await;
        install_distribution_role_fixture(&store).await;
        let identity = external_identity("distribution-user");

        let admin = store
            .artifact_distribution_authority(identity.clone(), "distribution-admin".into(), None)
            .await
            .unwrap();
        assert_eq!(
            admin.grants,
            ArtifactDistributionGrants {
                use_remote: true,
                sync: true,
                follow: true,
                fork: true,
                export: false,
                reshare: false,
            }
        );

        let member = store
            .artifact_distribution_authority(identity.clone(), "distribution-member".into(), None)
            .await
            .unwrap();
        assert_eq!(
            member.grants,
            ArtifactDistributionGrants {
                use_remote: true,
                ..ArtifactDistributionGrants::default()
            }
        );

        let viewer = store
            .artifact_distribution_authority(identity.clone(), "distribution-viewer".into(), None)
            .await
            .unwrap();
        assert_eq!(viewer.grants, ArtifactDistributionGrants::default());

        assert!(matches!(
            store
                .artifact_distribution_authority(
                    identity,
                    "distribution-admin".into(),
                    Some("not-my-team".into()),
                )
                .await,
            Err(AccessStoreError::NotAuthorized)
        ));
    }

    async fn install_source_policy(store: &AccessStore) {
        store
            .put_artifact_source_policy(ArtifactSourcePolicyRecord {
                provider_authority: "depot".into(),
                artifact_id: "artifact-a".into(),
                source_scope: personal_owner(),
                policy_epoch: 1,
                ceiling: open_ceiling(),
                updated_at: 11,
            })
            .await
            .unwrap();
    }

    async fn current_policy_epoch(store: &AccessStore) -> u64 {
        store
            .artifact_distribution_authority(
                external_identity("artifact-owner"),
                "bootstrap-default".into(),
                None,
            )
            .await
            .unwrap()
            .global_revision
    }

    async fn install_assignment(store: &AccessStore) {
        store
            .put_artifact_assignment_distribution(ArtifactAssignmentDistributionRecord {
                assignment_id: "assignment-a".into(),
                provider_authority: "depot".into(),
                artifact_id: "artifact-a".into(),
                source_scope: personal_owner(),
                policy_epoch: 1,
                ceiling: open_ceiling(),
                active: true,
                created_at: 12,
                updated_at: 12,
            })
            .await
            .unwrap();
    }

    fn stage_input(
        mirror_id: &str,
        operation_id: &str,
        mode: ManagedArtifactMirrorMode,
        policy_epoch: u64,
    ) -> StageManagedArtifactMirror {
        StageManagedArtifactMirror {
            mirror_id: mirror_id.into(),
            operation_id: operation_id.into(),
            owner_principal_id: "bootstrap-owner".into(),
            identity_ref_json: durable_identity_json(),
            authorization_project_id: "bootstrap-default".into(),
            authorization_team_id: None,
            destination_id: None,
            source_provider_authority: "depot".into(),
            source_assignment_id: "assignment-a".into(),
            source_scope: personal_owner(),
            source_artifact_id: "artifact-a".into(),
            source_revision_id: "revision-a".into(),
            local_artifact_id: "artifact-a".into(),
            mode,
            last_authorized_policy_epoch: policy_epoch,
            now: 20,
        }
    }

    #[tokio::test]
    async fn persisted_transfer_policy_defaults_assignment_distribution_to_deny() {
        let (_directory, store) = bootstrapped_store().await;
        install_source_policy(&store).await;
        let (publication, license) = transferable();

        let before = store
            .artifact_transfer_options(
                "depot".into(),
                "artifact-a".into(),
                Some("assignment-a".into()),
                all_grants(),
                publication.clone(),
                license.clone(),
                Some(ArtifactDestinationPolicy::local_personal()),
            )
            .await
            .unwrap();
        assert!(!before.allows(ArtifactTransferMode::Pin));
        assert_eq!(
            before.decision(ArtifactTransferMode::Pin).denied_by,
            Some(ArtifactTransferDenyReason::AssignmentPolicy)
        );

        install_assignment(&store).await;
        let after = store
            .artifact_transfer_options(
                "depot".into(),
                "artifact-a".into(),
                Some("assignment-a".into()),
                all_grants(),
                publication,
                license,
                Some(ArtifactDestinationPolicy::local_personal()),
            )
            .await
            .unwrap();
        assert!(after.allows(ArtifactTransferMode::Pin));
        assert!(after.allows(ArtifactTransferMode::Follow));
    }

    #[tokio::test]
    async fn source_policy_scope_is_immutable_and_policy_epoch_never_regresses() {
        let (_directory, store) = bootstrapped_store().await;
        install_source_policy(&store).await;

        let changed_scope = store
            .put_artifact_source_policy(ArtifactSourcePolicyRecord {
                provider_authority: "depot".into(),
                artifact_id: "artifact-a".into(),
                source_scope: OwnerScope::Project(ProjectId::new("bootstrap-default").unwrap()),
                policy_epoch: 2,
                ceiling: open_ceiling(),
                updated_at: 13,
            })
            .await;
        assert!(matches!(
            changed_scope,
            Err(AccessStoreError::ArtifactDistributionConflict)
        ));

        let regressed = store
            .put_artifact_source_policy(ArtifactSourcePolicyRecord {
                provider_authority: "depot".into(),
                artifact_id: "artifact-a".into(),
                source_scope: personal_owner(),
                policy_epoch: 0,
                ceiling: open_ceiling(),
                updated_at: 13,
            })
            .await;
        assert!(matches!(
            regressed,
            Err(AccessStoreError::InvalidArtifactDistributionInput)
        ));
    }

    #[tokio::test]
    async fn local_artifact_authority_is_a_reconciled_exact_epoch_saga() {
        let (_directory, store) = bootstrapped_store().await;
        let policy_epoch = current_policy_epoch(&store).await;
        let input = StageArtifactAuthority {
            artifact_id: "local-artifact-a".into(),
            operation_id: "authority-operation-a".into(),
            owner: personal_owner(),
            policy_epoch,
            now: 14,
        };
        let staged = store.stage_artifact_authority(input.clone()).await.unwrap();
        assert_eq!(staged.status, ArtifactAuthorityStatus::Pending);
        assert_eq!(store.stage_artifact_authority(input).await.unwrap(), staged);

        let committing = store
            .mark_artifact_authority_committing(
                "local-artifact-a".into(),
                "authority-operation-a".into(),
                15,
            )
            .await
            .unwrap();
        assert_eq!(committing.status, ArtifactAuthorityStatus::Committing);

        let active = store
            .activate_artifact_authority(
                "local-artifact-a".into(),
                "authority-operation-a".into(),
                policy_epoch,
                16,
            )
            .await
            .unwrap();
        assert_eq!(active.status, ArtifactAuthorityStatus::Active);
        assert_eq!(active.owner, personal_owner());
        assert_eq!(
            store
                .activate_artifact_authority(
                    "local-artifact-a".into(),
                    "authority-operation-a".into(),
                    policy_epoch,
                    17,
                )
                .await
                .unwrap(),
            active
        );
    }

    #[tokio::test]
    async fn local_artifact_authority_activation_fails_after_policy_drift() {
        let (_directory, store) = bootstrapped_store().await;
        let policy_epoch = current_policy_epoch(&store).await;
        store
            .stage_artifact_authority(StageArtifactAuthority {
                artifact_id: "local-artifact-drift".into(),
                operation_id: "authority-operation-drift".into(),
                owner: personal_owner(),
                policy_epoch,
                now: 18,
            })
            .await
            .unwrap();
        store
            .mark_artifact_authority_committing(
                "local-artifact-drift".into(),
                "authority-operation-drift".into(),
                19,
            )
            .await
            .unwrap();
        store
            .put_artifact_source_policy(ArtifactSourcePolicyRecord {
                provider_authority: "depot:drift".into(),
                artifact_id: "other-artifact".into(),
                source_scope: personal_owner(),
                policy_epoch: 1,
                ceiling: open_ceiling(),
                updated_at: 20,
            })
            .await
            .unwrap();
        assert!(matches!(
            store
                .activate_artifact_authority(
                    "local-artifact-drift".into(),
                    "authority-operation-drift".into(),
                    policy_epoch,
                    21,
                )
                .await,
            Err(AccessStoreError::NotAuthorized)
        ));
        assert_eq!(
            store
                .artifact_authority("local-artifact-drift".into())
                .await
                .unwrap()
                .unwrap()
                .status,
            ArtifactAuthorityStatus::Committing
        );
    }

    #[tokio::test]
    async fn managed_mirror_state_machine_is_idempotent_and_cannot_reactivate_revoked_state() {
        let (_directory, store) = bootstrapped_store().await;
        install_source_policy(&store).await;
        install_assignment(&store).await;
        let policy_epoch = current_policy_epoch(&store).await;

        let input = stage_input(
            "mirror-a",
            "operation-a",
            ManagedArtifactMirrorMode::Pinned,
            policy_epoch,
        );
        let staged = store
            .stage_managed_artifact_mirror(input.clone())
            .await
            .unwrap();
        assert_eq!(staged.status, ManagedArtifactMirrorStatus::Pending);
        assert_eq!(
            store.stage_managed_artifact_mirror(input).await.unwrap(),
            staged
        );

        let conflicting = store
            .stage_managed_artifact_mirror(stage_input(
                "mirror-b",
                "operation-a",
                ManagedArtifactMirrorMode::Pinned,
                policy_epoch,
            ))
            .await;
        assert!(matches!(
            conflicting,
            Err(AccessStoreError::ArtifactDistributionConflict)
        ));

        let committing = store
            .mark_managed_artifact_mirror_committing("mirror-a".into(), "operation-a".into(), 21)
            .await
            .unwrap();
        assert_eq!(committing.status, ManagedArtifactMirrorStatus::Committing);

        let active = store
            .activate_managed_artifact_mirror(
                "mirror-a".into(),
                "operation-a".into(),
                "revision-a".into(),
                policy_epoch,
                22,
            )
            .await
            .unwrap();
        assert_eq!(active.status, ManagedArtifactMirrorStatus::Active);
        assert_eq!(active.local_revision_id.as_deref(), Some("revision-a"));

        let revoked = store
            .restrict_managed_artifact_mirror(
                "mirror-a".into(),
                "operation-a".into(),
                ManagedArtifactMirrorStatus::AccessRevoked,
                23,
            )
            .await
            .unwrap();
        assert_eq!(revoked.status, ManagedArtifactMirrorStatus::AccessRevoked);
        let reactivation = store
            .activate_managed_artifact_mirror(
                "mirror-a".into(),
                "operation-a".into(),
                "revision-a".into(),
                policy_epoch,
                24,
            )
            .await;
        assert!(matches!(
            reactivation,
            Err(AccessStoreError::ArtifactMirrorStateConflict)
        ));
    }

    #[tokio::test]
    async fn followed_mirror_update_enters_committing_and_is_idempotent() {
        let (_directory, store) = bootstrapped_store().await;
        install_source_policy(&store).await;
        install_assignment(&store).await;
        let policy_epoch = current_policy_epoch(&store).await;

        store
            .stage_managed_artifact_mirror(stage_input(
                "mirror-update",
                "operation-initial",
                ManagedArtifactMirrorMode::Followed,
                policy_epoch,
            ))
            .await
            .unwrap();
        store
            .mark_managed_artifact_mirror_committing(
                "mirror-update".into(),
                "operation-initial".into(),
                21,
            )
            .await
            .unwrap();
        store
            .activate_managed_artifact_mirror(
                "mirror-update".into(),
                "operation-initial".into(),
                "revision-a".into(),
                policy_epoch,
                22,
            )
            .await
            .unwrap();

        let input = BeginManagedArtifactMirrorUpdate {
            mirror_id: "mirror-update".into(),
            operation_id: "operation-update-b".into(),
            source_revision_id: "revision-b".into(),
            expected_local_revision_id: "revision-a".into(),
            policy_epoch,
            now: 23,
        };
        let committing = store
            .begin_managed_artifact_mirror_update(input.clone())
            .await
            .unwrap();
        assert_eq!(committing.status, ManagedArtifactMirrorStatus::Committing);
        assert_eq!(committing.operation_id, "operation-update-b");
        assert_eq!(committing.source_revision_id, "revision-b");
        assert_eq!(committing.local_revision_id.as_deref(), Some("revision-a"));
        assert_eq!(committing.last_authorized_policy_epoch, policy_epoch);
        assert_eq!(
            store
                .begin_managed_artifact_mirror_update(input)
                .await
                .unwrap(),
            committing
        );

        let conflicting = store
            .begin_managed_artifact_mirror_update(BeginManagedArtifactMirrorUpdate {
                mirror_id: "mirror-update".into(),
                operation_id: "operation-update-c".into(),
                source_revision_id: "revision-c".into(),
                expected_local_revision_id: "revision-a".into(),
                policy_epoch,
                now: 24,
            })
            .await;
        assert!(matches!(
            conflicting,
            Err(AccessStoreError::ArtifactMirrorStateConflict)
        ));

        let active = store
            .activate_managed_artifact_mirror(
                "mirror-update".into(),
                "operation-update-b".into(),
                "revision-b".into(),
                policy_epoch,
                25,
            )
            .await
            .unwrap();
        assert_eq!(active.status, ManagedArtifactMirrorStatus::Active);
        assert_eq!(active.source_revision_id, "revision-b");
        assert_eq!(active.local_revision_id.as_deref(), Some("revision-b"));
    }

    #[tokio::test]
    async fn follow_subscription_requires_an_active_followed_mirror() {
        let (_directory, store) = bootstrapped_store().await;
        install_source_policy(&store).await;
        install_assignment(&store).await;
        let policy_epoch = current_policy_epoch(&store).await;

        store
            .stage_managed_artifact_mirror(stage_input(
                "mirror-follow",
                "operation-follow",
                ManagedArtifactMirrorMode::Followed,
                policy_epoch,
            ))
            .await
            .unwrap();
        store
            .mark_managed_artifact_mirror_committing(
                "mirror-follow".into(),
                "operation-follow".into(),
                21,
            )
            .await
            .unwrap();
        store
            .activate_managed_artifact_mirror(
                "mirror-follow".into(),
                "operation-follow".into(),
                "revision-a".into(),
                policy_epoch,
                22,
            )
            .await
            .unwrap();

        let subscription = ManagedArtifactSubscription {
            mirror_id: "mirror-follow".into(),
            update_policy: ArtifactSubscriptionUpdatePolicy::Notify,
            last_observed_revision_id: Some("revision-b".into()),
            last_applied_revision_id: Some("revision-a".into()),
            last_checked_at: Some(23),
            active: true,
            updated_at: 23,
        };
        assert_eq!(
            store
                .put_artifact_subscription(subscription.clone())
                .await
                .unwrap(),
            subscription
        );
        assert_eq!(
            store
                .artifact_subscription("mirror-follow".into())
                .await
                .unwrap(),
            Some(subscription)
        );

        store
            .stage_managed_artifact_mirror(stage_input(
                "mirror-pin",
                "operation-pin",
                ManagedArtifactMirrorMode::Pinned,
                policy_epoch,
            ))
            .await
            .unwrap();
        store
            .mark_managed_artifact_mirror_committing(
                "mirror-pin".into(),
                "operation-pin".into(),
                24,
            )
            .await
            .unwrap();
        store
            .activate_managed_artifact_mirror(
                "mirror-pin".into(),
                "operation-pin".into(),
                "revision-a".into(),
                policy_epoch,
                25,
            )
            .await
            .unwrap();
        let invalid = store
            .put_artifact_subscription(ManagedArtifactSubscription {
                mirror_id: "mirror-pin".into(),
                update_policy: ArtifactSubscriptionUpdatePolicy::AutoApproved,
                last_observed_revision_id: None,
                last_applied_revision_id: None,
                last_checked_at: None,
                active: true,
                updated_at: 26,
            })
            .await;
        assert!(matches!(
            invalid,
            Err(AccessStoreError::ArtifactMirrorStateConflict)
        ));
    }

    #[tokio::test]
    async fn auto_follow_due_scan_is_bounded_filters_policy_and_recovers_committing_work() {
        let (_directory, store) = bootstrapped_store().await;
        install_source_policy(&store).await;
        install_assignment(&store).await;
        let policy_epoch = current_policy_epoch(&store).await;

        for (mirror_id, operation_id, policy) in [
            (
                "mirror-auto-active",
                "operation-auto-active",
                ArtifactSubscriptionUpdatePolicy::AutoApproved,
            ),
            (
                "mirror-notify",
                "operation-notify",
                ArtifactSubscriptionUpdatePolicy::Notify,
            ),
            (
                "mirror-auto-recover",
                "operation-auto-recover",
                ArtifactSubscriptionUpdatePolicy::AutoApproved,
            ),
        ] {
            store
                .stage_managed_artifact_mirror(stage_input(
                    mirror_id,
                    operation_id,
                    ManagedArtifactMirrorMode::Followed,
                    policy_epoch,
                ))
                .await
                .unwrap();
            store
                .mark_managed_artifact_mirror_committing(mirror_id.into(), operation_id.into(), 21)
                .await
                .unwrap();
            store
                .activate_managed_artifact_mirror(
                    mirror_id.into(),
                    operation_id.into(),
                    "revision-a".into(),
                    policy_epoch,
                    22,
                )
                .await
                .unwrap();
            store
                .put_artifact_subscription(ManagedArtifactSubscription {
                    mirror_id: mirror_id.into(),
                    update_policy: policy,
                    last_observed_revision_id: Some("revision-a".into()),
                    last_applied_revision_id: Some("revision-a".into()),
                    last_checked_at: Some(5),
                    active: true,
                    updated_at: 5,
                })
                .await
                .unwrap();
        }

        store
            .stage_managed_artifact_mirror(stage_input(
                "mirror-pin-reconcile",
                "operation-pin-reconcile",
                ManagedArtifactMirrorMode::Pinned,
                policy_epoch,
            ))
            .await
            .unwrap();
        store
            .mark_managed_artifact_mirror_committing(
                "mirror-pin-reconcile".into(),
                "operation-pin-reconcile".into(),
                21,
            )
            .await
            .unwrap();
        store
            .activate_managed_artifact_mirror(
                "mirror-pin-reconcile".into(),
                "operation-pin-reconcile".into(),
                "revision-a".into(),
                policy_epoch,
                22,
            )
            .await
            .unwrap();

        store
            .begin_managed_artifact_mirror_update(BeginManagedArtifactMirrorUpdate {
                mirror_id: "mirror-auto-recover".into(),
                operation_id: "operation-auto-recover-b".into(),
                source_revision_id: "revision-b".into(),
                expected_local_revision_id: "revision-a".into(),
                policy_epoch,
                now: 23,
            })
            .await
            .unwrap();

        let mirrors = store
            .managed_artifact_mirrors_for_reconciliation(10, 64)
            .await
            .unwrap();
        assert_eq!(mirrors.len(), 4);
        for expected in [
            "mirror-auto-active",
            "mirror-notify",
            "mirror-auto-recover",
            "mirror-pin-reconcile",
        ] {
            assert!(mirrors.iter().any(|mirror| mirror.mirror_id == expected));
        }
        assert!(mirrors.iter().all(|mirror| {
            mirror.authorization_project_id == "bootstrap-default"
                && mirror.authorization_team_id.is_none()
                && !mirror.identity_ref_json.is_empty()
                && mirror.last_checked_at.is_none()
        }));
        let checked_pin = store
            .mark_managed_artifact_mirror_checked("mirror-pin-reconcile".into(), 30)
            .await
            .unwrap();
        assert_eq!(checked_pin.last_checked_at, Some(30));
        let remaining_due = store
            .managed_artifact_mirrors_for_reconciliation(10, 64)
            .await
            .unwrap();
        assert_eq!(remaining_due.len(), 3);
        assert!(
            remaining_due
                .iter()
                .all(|mirror| mirror.mirror_id != "mirror-pin-reconcile")
        );
        assert!(matches!(
            store
                .managed_artifact_mirrors_for_reconciliation(10, 0)
                .await,
            Err(AccessStoreError::InvalidArtifactDistributionInput)
        ));
        assert!(matches!(
            store
                .managed_artifact_mirrors_for_reconciliation(10, 65)
                .await,
            Err(AccessStoreError::InvalidArtifactDistributionInput)
        ));

        let due = store
            .auto_approved_artifact_subscriptions_due(10, 64)
            .await
            .unwrap();
        assert_eq!(
            due.iter()
                .map(|subscription| subscription.mirror_id.as_str())
                .collect::<Vec<_>>(),
            vec!["mirror-auto-active", "mirror-auto-recover"]
        );
        for subscription in &due {
            let mirror = store
                .managed_artifact_mirror(subscription.mirror_id.clone())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(mirror.authorization_project_id, "bootstrap-default");
            assert!(mirror.authorization_team_id.is_none());
            assert!(!mirror.identity_ref_json.is_empty());
        }

        let observed = store
            .observe_artifact_subscription(
                "mirror-auto-active".into(),
                Some("revision-c".into()),
                30,
            )
            .await
            .unwrap();
        assert_eq!(
            observed.last_observed_revision_id.as_deref(),
            Some("revision-c")
        );
        assert_eq!(
            observed.last_applied_revision_id.as_deref(),
            Some("revision-a")
        );
        assert_eq!(observed.last_checked_at, Some(30));
        let observed_mirror = store
            .managed_artifact_mirror("mirror-auto-active".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            observed_mirror.authorization_project_id,
            "bootstrap-default"
        );
        assert!(!observed_mirror.identity_ref_json.is_empty());

        let due_after_observation = store
            .auto_approved_artifact_subscriptions_due(10, 64)
            .await
            .unwrap();
        assert_eq!(
            due_after_observation
                .iter()
                .map(|subscription| subscription.mirror_id.as_str())
                .collect::<Vec<_>>(),
            vec!["mirror-auto-recover"]
        );

        assert!(matches!(
            store.auto_approved_artifact_subscriptions_due(10, 0).await,
            Err(AccessStoreError::InvalidArtifactDistributionInput)
        ));
        assert!(matches!(
            store.auto_approved_artifact_subscriptions_due(10, 65).await,
            Err(AccessStoreError::InvalidArtifactDistributionInput)
        ));
    }
}
