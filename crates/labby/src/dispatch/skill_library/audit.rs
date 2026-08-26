//! Bounded, deduplicating Skill Library authorization audit sink.

#![allow(
    dead_code,
    reason = "shared audit sink is consumed by the Wave 2 Skill Library dispatcher"
)]

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use labby_runtime::artifacts::canonical_json;
use labby_runtime::artifacts::validation::validate_id;
use labby_runtime::artifacts::{ArtifactError, LibraryActorId, LibraryTenantId};

use super::auth::{SkillLibraryAction, SkillLibrarySurface};

const MAX_AUDIT_EVENTS: usize = 1024;
const MAX_CORRELATION_BYTES: usize = 128;

/// Validated canonical Artifact identity. Audit retains only its bounded digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CanonicalArtifactId(String);

impl CanonicalArtifactId {
    pub(crate) fn parse(value: impl Into<String>) -> Result<Self, ArtifactError> {
        let value = value.into();
        validate_id(&value, "artifact_id")?;
        Ok(Self(value))
    }

    fn audit_digest(&self) -> String {
        canonical_json::sha256_bytes(self.0.as_bytes())
    }
}

/// Bounded request correlation identifier established by the transport adapter.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SkillLibraryCorrelationId(String);

impl SkillLibraryCorrelationId {
    pub(crate) fn parse(value: impl Into<String>) -> Result<Self, ()> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_CORRELATION_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
        {
            return Err(());
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SkillLibraryAuditOutcome {
    Allow,
    Deny,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SkillLibraryAuditStage {
    Transport,
    AccessSnapshot,
    Ownership,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SkillLibraryAuditKey {
    correlation_id: SkillLibraryCorrelationId,
    action: SkillLibraryAction,
    target_digest: String,
    outcome: SkillLibraryAuditOutcome,
    stage: SkillLibraryAuditStage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillLibraryAuditEvent {
    key: SkillLibraryAuditKey,
    pub(crate) tenant_id: Option<LibraryTenantId>,
    pub(crate) actor_id: Option<LibraryActorId>,
    pub(crate) action: SkillLibraryAction,
    target_digest: String,
    pub(crate) surface: SkillLibrarySurface,
    pub(crate) outcome: SkillLibraryAuditOutcome,
    pub(crate) stage: SkillLibraryAuditStage,
    pub(crate) policy_revision: Option<u64>,
}

impl SkillLibraryAuditEvent {
    pub(crate) fn new(
        correlation_id: SkillLibraryCorrelationId,
        target: &CanonicalArtifactId,
        action: SkillLibraryAction,
        surface: SkillLibrarySurface,
        outcome: SkillLibraryAuditOutcome,
        stage: SkillLibraryAuditStage,
    ) -> Self {
        let target_digest = target.audit_digest();
        Self {
            key: SkillLibraryAuditKey {
                correlation_id,
                action,
                target_digest: target_digest.clone(),
                outcome,
                stage,
            },
            tenant_id: None,
            actor_id: None,
            action,
            target_digest,
            surface,
            outcome,
            stage,
            policy_revision: None,
        }
    }

    pub(crate) fn with_canonical_actor(
        mut self,
        tenant_id: LibraryTenantId,
        actor_id: LibraryActorId,
        policy_revision: u64,
    ) -> Self {
        self.tenant_id = Some(tenant_id);
        self.actor_id = Some(actor_id);
        self.policy_revision = Some(policy_revision);
        self
    }
}

#[derive(Default)]
struct AuditState {
    order: VecDeque<SkillLibraryAuditKey>,
    keys: HashSet<SkillLibraryAuditKey>,
    events: VecDeque<SkillLibraryAuditEvent>,
}

/// Process-shared bounded audit sink. Recording the same terminal decision is idempotent.
#[derive(Clone, Default)]
pub(crate) struct SkillLibraryAuditSink {
    state: Arc<Mutex<AuditState>>,
}

impl SkillLibraryAuditSink {
    /// Returns true only when this decision was newly retained and emitted.
    pub(crate) fn record(&self, event: SkillLibraryAuditEvent) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.keys.contains(&event.key) {
            return false;
        }
        while state.order.len() >= MAX_AUDIT_EVENTS {
            if let Some(expired) = state.order.pop_front() {
                state.keys.remove(&expired);
                state.events.pop_front();
            }
        }
        state.keys.insert(event.key.clone());
        state.order.push_back(event.key.clone());
        state.events.push_back(event.clone());
        drop(state);
        tracing::info!(
            correlation_id = event.key.correlation_id.as_str(),
            action = event.action.as_str(),
            target_digest = event.target_digest,
            surface = event.surface.as_str(),
            outcome = ?event.outcome,
            stage = ?event.stage,
            policy_revision = event.policy_revision,
            tenant_id = event.tenant_id.as_ref().map(LibraryTenantId::as_str),
            actor_id = event.actor_id.as_ref().map(LibraryActorId::as_str),
            "skill library authorization decision"
        );
        true
    }

    #[cfg(test)]
    fn events(&self) -> Vec<SkillLibraryAuditEvent> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .events
            .iter()
            .cloned()
            .collect()
    }
}

pub(super) fn skill_library_audit_sink() -> &'static SkillLibraryAuditSink {
    static SINK: OnceLock<SkillLibraryAuditSink> = OnceLock::new();
    SINK.get_or_init(SkillLibraryAuditSink::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_sink_deduplicates_and_retains_correlation_without_raw_target() {
        let sink = SkillLibraryAuditSink::default();
        let target = CanonicalArtifactId::parse("secret-canary").unwrap();
        let correlation = SkillLibraryCorrelationId::parse("request-1").unwrap();
        let event = SkillLibraryAuditEvent::new(
            correlation,
            &target,
            SkillLibraryAction::Activate,
            SkillLibrarySurface::Mcp,
            SkillLibraryAuditOutcome::Allow,
            SkillLibraryAuditStage::Ownership,
        );
        assert!(sink.record(event.clone()));
        assert!(!sink.record(event));
        assert_eq!(sink.events().len(), 1);
        let debug = format!("{:?}", sink.events());
        assert!(debug.contains("request-1"));
        assert!(!debug.contains("secret-canary"));
    }

    #[test]
    fn invalid_newline_and_oversized_identifiers_never_reach_the_sink() {
        assert!(CanonicalArtifactId::parse("secret\ncanary").is_err());
        assert!(CanonicalArtifactId::parse("x".repeat(1024)).is_err());
        assert!(SkillLibraryCorrelationId::parse("request\nforged").is_err());
        assert!(SkillLibraryCorrelationId::parse("x".repeat(129)).is_err());
    }
}
