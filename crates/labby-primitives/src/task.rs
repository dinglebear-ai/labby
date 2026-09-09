//! Agent Task contracts. These are not Depot ingestion/artifact jobs.

use crate::access::{OwnerScope, PrincipalId, ProjectId};
use crate::digest::Sha256Digest;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Created,
    Queued,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    Expired,
}

impl TaskState {
    pub const fn terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Expired
        )
    }
    pub const fn permits(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Created | Self::Failed | Self::Cancelled | Self::Expired,
                Self::Queued
            ) | (Self::Queued, Self::Running)
                | (
                    Self::Created | Self::Queued | Self::Running,
                    Self::Cancelling
                )
                | (Self::Running, Self::Succeeded | Self::Failed)
                | (Self::Cancelling, Self::Cancelled | Self::Failed)
                | (
                    Self::Queued | Self::Running | Self::Cancelling,
                    Self::Expired
                )
        )
    }
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
}

/// A durable Task intent.
///
/// Invariant: `project` is the Project that owns the Task and is therefore
/// present exactly when `owner` is [`OwnerScope::Project`], carrying the same
/// identifier. A Team-, Personal-, or Installation-owned Task never names a
/// Project, because Project authority is not implied by any other owner scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskIntent {
    pub id: String,
    pub idempotency_key: String,
    pub owner: OwnerScope,
    pub project: Option<ProjectId>,
    pub creator: PrincipalId,
    pub agent_id: String,
    pub agent_version: u64,
    pub agent_revision_digest: String,
    pub input_digest: String,
    pub catalog_generation: String,
    pub authority_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskAttemptLease {
    pub attempt: u32,
    pub fencing_token: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskSettlement {
    pub state: TaskState,
    pub output_digest: Option<String>,
    pub error_code: Option<String>,
    pub settled_at: i64,
}

pub fn validate_intent(intent: &TaskIntent) -> bool {
    let project_bound = match (&intent.owner, &intent.project) {
        (OwnerScope::Project(owner), Some(project)) => owner == project,
        (OwnerScope::Project(_), None) | (_, Some(_)) => false,
        (_, None) => true,
    };
    project_bound
        && intent.agent_version > 0
        && [
            &intent.id,
            &intent.idempotency_key,
            &intent.agent_id,
            &intent.catalog_generation,
            &intent.authority_fingerprint,
        ]
        .iter()
        .all(|v| valid(v))
        && [&intent.agent_revision_digest, &intent.input_digest]
            .iter()
            .all(|v| Sha256Digest::is_canonical(v))
}
fn valid(v: &str) -> bool {
    !v.is_empty() && v.len() <= 256 && v == v.trim() && !v.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn intent_project_binding_follows_the_owner_scope() {
        use crate::access::TeamId;
        let digest = format!("sha256:{}", "b".repeat(64));
        let mut intent = TaskIntent {
            id: "task-1".into(),
            idempotency_key: "key-1".into(),
            owner: OwnerScope::Project(ProjectId::new("proj-1").unwrap()),
            project: Some(ProjectId::new("proj-1").unwrap()),
            creator: PrincipalId::new("p-1").unwrap(),
            agent_id: "agent-1".into(),
            agent_version: 1,
            agent_revision_digest: digest.clone(),
            input_digest: digest,
            catalog_generation: "catalog-1".into(),
            authority_fingerprint: "authority-1".into(),
        };
        assert!(validate_intent(&intent));
        intent.project = Some(ProjectId::new("proj-2").unwrap());
        assert!(!validate_intent(&intent));
        intent.project = None;
        assert!(!validate_intent(&intent));
        intent.owner = OwnerScope::Team(TeamId::new("team-1").unwrap());
        assert!(validate_intent(&intent));
        intent.project = Some(ProjectId::new("proj-1").unwrap());
        assert!(!validate_intent(&intent));
    }

    #[test]
    fn lifecycle_is_closed_and_terminal_settlement_is_final() {
        assert!(TaskState::Created.permits(TaskState::Queued));
        assert!(TaskState::Running.permits(TaskState::Succeeded));
        assert!(!TaskState::Succeeded.permits(TaskState::Running));
        assert!(TaskState::Succeeded.terminal());
        assert!(!TaskState::Running.terminal());
    }
}
