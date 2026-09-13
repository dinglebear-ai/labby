//! Pluggable, authority-fenced Agent execution orchestration.

use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use labby_primitives::agent::{
    AgentDefinition, AgentSessionBinding, AgentState, RunningRevocationPolicy,
};
use thiserror::Error;

use crate::authority::{
    AuthorityEpochVector, AuthorityLease, AuthorityLeaseError, AuthoritySafeBoundary,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentResourceBounds {
    pub max_runtime_millis: u64,
    pub max_output_bytes: usize,
    pub max_external_effects: u32,
}
impl AgentResourceBounds {
    pub fn validate(self) -> Result<Self, AgentRuntimeError> {
        if self.max_runtime_millis == 0
            || self.max_runtime_millis > 86_400_000
            || self.max_output_bytes == 0
            || self.max_output_bytes > 64 * 1024 * 1024
            || self.max_external_effects > 10_000
        {
            Err(AgentRuntimeError::InvalidBounds)
        } else {
            Ok(self)
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentExecutionRequest {
    pub definition: AgentDefinition,
    pub session: AgentSessionBinding,
    pub lease: AuthorityLease,
    pub bounds: AgentResourceBounds,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentExecutionOutput {
    pub digest: String,
    pub bytes: usize,
    pub external_effects: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentRecoveryAction {
    ResumeAfterReauthorization,
    MarkInterrupted,
    MarkRevoked,
}
pub fn recovery_action(
    was_running: bool,
    state: AgentState,
    lease_valid: bool,
) -> AgentRecoveryAction {
    if state != AgentState::Active {
        AgentRecoveryAction::MarkRevoked
    } else if was_running && lease_valid {
        AgentRecoveryAction::ResumeAfterReauthorization
    } else {
        AgentRecoveryAction::MarkInterrupted
    }
}

/// Which durable resource an execution lease must name.
///
/// An Agent session lease is bound to the Agent definition; an Agent Task
/// lease is bound to the Task (as its resource or its creation intent).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeaseResourceBinding {
    Agent,
    Task { task_id: String },
}

/// Milliseconds since the Unix epoch from the process clock.
#[must_use]
pub fn system_now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Live authority source. The runtime, not the dispatcher, owns the clock used
/// at every safe boundary: a caller-supplied timestamp can only be earlier than
/// this clock, never later, so a stale dispatch-time value cannot extend a lease.
pub trait AgentAuthority: Send + Sync {
    fn current_epochs(
        &self,
    ) -> impl Future<Output = Result<AuthorityEpochVector, AgentRuntimeError>> + Send;

    /// Current time in milliseconds since the Unix epoch. Defaults to the
    /// process clock; deterministic tests override it.
    fn now_millis(&self) -> u64 {
        system_now_millis()
    }
}
pub trait AgentExecutor: Send + Sync {
    fn execute(
        &self,
        request: AgentExecutionRequest,
        guard: ExecutionGuard<'_>,
    ) -> impl Future<Output = Result<AgentExecutionOutput, AgentRuntimeError>> + Send;
}

#[derive(Clone)]
pub struct Cancellation {
    cancelled: Arc<AtomicBool>,
}
impl Cancellation {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release)
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}
impl Default for Cancellation {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ExecutionGuard<'a> {
    authority: &'a dyn AuthorityDyn,
    lease: AuthorityLease,
    cancellation: Cancellation,
    revocation: RunningRevocationPolicy,
}
trait AuthorityDyn: Send + Sync {
    fn epochs(
        &self,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<AuthorityEpochVector, AgentRuntimeError>> + Send + '_>,
    >;
    fn clock_millis(&self) -> u64;
}
impl<T: AgentAuthority> AuthorityDyn for T {
    fn epochs(
        &self,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<AuthorityEpochVector, AgentRuntimeError>> + Send + '_>,
    > {
        Box::pin(self.current_epochs())
    }
    fn clock_millis(&self) -> u64 {
        self.now_millis()
    }
}
impl ExecutionGuard<'_> {
    /// Revalidate the lease at a safe boundary. `now` is the executor's own
    /// notion of time; the runtime clock is authoritative and an executor can
    /// only move the effective time later, never earlier.
    pub async fn check(
        &self,
        boundary: AuthoritySafeBoundary,
        now: u64,
    ) -> Result<(), AgentRuntimeError> {
        if self.cancellation.is_cancelled() {
            return Err(AgentRuntimeError::Cancelled);
        }
        let now = now.max(self.authority.clock_millis());
        let epochs = self.authority.epochs().await?;
        match self.lease.validate_at(boundary, now, &epochs) {
            Ok(()) => Ok(()),
            Err(AuthorityLeaseError::AuthorityChanged)
                if self.revocation == RunningRevocationPolicy::StopAtSafeBoundary =>
            {
                Err(AgentRuntimeError::Revoked)
            }
            Err(error) => Err(AgentRuntimeError::Lease(error)),
        }
    }
}

/// Require that the lease names exactly the subject, owner, authority
/// fingerprint, and resource this execution claims to act for.
fn validate_lease_binding(
    request: &AgentExecutionRequest,
    resource: &LeaseResourceBinding,
) -> Result<(), AgentRuntimeError> {
    let binding = request.lease.binding();
    let resource_matches = match resource {
        LeaseResourceBinding::Agent => binding.resource_id().as_str() == request.definition.id,
        LeaseResourceBinding::Task { task_id } => {
            binding.resource_id().as_str() == task_id
                || binding
                    .intent_id()
                    .is_some_and(|intent| intent.as_str() == task_id)
        }
    };
    if !resource_matches
        || binding.owner_scope() != &request.definition.owner
        || binding.owner_scope() != &request.session.owner
        || binding.principal_id() != request.session.principal.as_str()
        || request.lease.epoch_fingerprint().as_str() != request.session.authority_fingerprint
    {
        return Err(AgentRuntimeError::BindingMismatch);
    }
    Ok(())
}

pub async fn execute_agent<A: AgentAuthority, E: AgentExecutor>(
    authority: &A,
    executor: &E,
    request: AgentExecutionRequest,
    cancellation: Cancellation,
    now: u64,
) -> Result<AgentExecutionOutput, AgentRuntimeError> {
    execute_agent_bound(
        authority,
        executor,
        request,
        cancellation,
        now,
        &LeaseResourceBinding::Agent,
    )
    .await
}

/// Execute with an explicit statement of which resource the lease must name.
///
/// `now` is the caller's admission timestamp; the effective clock at every
/// boundary is the later of that value and [`AgentAuthority::now_millis`].
pub async fn execute_agent_bound<A: AgentAuthority, E: AgentExecutor>(
    authority: &A,
    executor: &E,
    request: AgentExecutionRequest,
    cancellation: Cancellation,
    now: u64,
    resource: &LeaseResourceBinding,
) -> Result<AgentExecutionOutput, AgentRuntimeError> {
    request
        .definition
        .validate()
        .map_err(|_| AgentRuntimeError::InvalidDefinition)?;
    request.bounds.validate()?;
    if request.definition.state != AgentState::Active
        || request.session.agent_id != request.definition.id
        || request.session.agent_version != request.definition.revision.version
        || request.session.catalog_generation != request.definition.revision.catalog_generation
    {
        return Err(AgentRuntimeError::PinnedDefinitionMismatch);
    }
    validate_lease_binding(&request, resource)?;
    if !request.definition.dispatchable(
        request.definition.authority_epoch,
        &[*request.lease.binding().capability()],
    ) {
        return Err(AgentRuntimeError::NotDispatchable);
    }
    let admission_now = now.max(authority.now_millis());
    let epochs = authority.current_epochs().await?;
    request
        .lease
        .validate_at(
            AuthoritySafeBoundary::BeforeDispatch,
            admission_now,
            &epochs,
        )
        .map_err(AgentRuntimeError::Lease)?;
    let final_lease = request.lease.clone();
    let final_cancellation = cancellation.clone();
    let revocation = request.definition.revocation_policy;
    let guard = ExecutionGuard {
        authority,
        lease: request.lease.clone(),
        cancellation: cancellation.clone(),
        revocation,
    };
    let bounds = request.bounds;
    // `max_runtime_millis` is a hard ceiling owned by the runtime. An executor
    // that overruns it is cancelled and its result is discarded.
    let output = match tokio::time::timeout(
        Duration::from_millis(bounds.max_runtime_millis),
        executor.execute(request, guard),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            cancellation.cancel();
            return Err(AgentRuntimeError::ResourceLimit);
        }
    };
    // Executors may check around their own external effects, but the runtime
    // owns the final commit boundary and never trusts an implementation to do
    // so. This closes the gap for executors that omit or misplace guard.check.
    ExecutionGuard {
        authority,
        lease: final_lease,
        cancellation: final_cancellation,
        revocation,
    }
    .check(AuthoritySafeBoundary::BeforeCommit, admission_now)
    .await?;
    if output.bytes > bounds.max_output_bytes
        || output.external_effects > bounds.max_external_effects
    {
        return Err(AgentRuntimeError::ResourceLimit);
    }
    Ok(output)
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum AgentRuntimeError {
    #[error("invalid agent definition")]
    InvalidDefinition,
    #[error("pinned definition mismatch")]
    PinnedDefinitionMismatch,
    #[error("authority lease is not bound to this execution")]
    BindingMismatch,
    #[error("agent definition is not dispatchable under the granted capability")]
    NotDispatchable,
    #[error("invalid resource bounds")]
    InvalidBounds,
    #[error("resource limit exceeded")]
    ResourceLimit,
    #[error("execution cancelled")]
    Cancelled,
    #[error("authority revoked")]
    Revoked,
    #[error("authority unavailable")]
    AuthorityUnavailable,
    #[error("executor failed")]
    ExecutorFailed,
    #[error("authority lease: {0}")]
    Lease(AuthorityLeaseError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::*;
    use labby_primitives::{access::*, agent::*};
    struct Auth(AuthorityEpochVector);
    impl AgentAuthority for Auth {
        async fn current_epochs(&self) -> Result<AuthorityEpochVector, AgentRuntimeError> {
            Ok(self.0.clone())
        }
        fn now_millis(&self) -> u64 {
            1
        }
    }
    struct SlowExec;
    impl AgentExecutor for SlowExec {
        async fn execute(
            &self,
            _: AgentExecutionRequest,
            _: ExecutionGuard<'_>,
        ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(AgentExecutionOutput {
                digest: dig(),
                bytes: 4,
                external_effects: 0,
            })
        }
    }
    struct Exec;
    impl AgentExecutor for Exec {
        async fn execute(
            &self,
            _: AgentExecutionRequest,
            guard: ExecutionGuard<'_>,
        ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
            guard.check(AuthoritySafeBoundary::BeforeCommit, 2).await?;
            Ok(AgentExecutionOutput {
                digest: dig(),
                bytes: 4,
                external_effects: 0,
            })
        }
    }
    struct ExecWithoutFinalCheck;
    impl AgentExecutor for ExecWithoutFinalCheck {
        async fn execute(
            &self,
            _: AgentExecutionRequest,
            _: ExecutionGuard<'_>,
        ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
            Ok(AgentExecutionOutput {
                digest: dig(),
                bytes: 4,
                external_effects: 0,
            })
        }
    }
    struct ChangingAuth {
        initial: AuthorityEpochVector,
        changed: AuthorityEpochVector,
        reads: std::sync::atomic::AtomicUsize,
    }
    impl AgentAuthority for ChangingAuth {
        async fn current_epochs(&self) -> Result<AuthorityEpochVector, AgentRuntimeError> {
            if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(self.initial.clone())
            } else {
                Ok(self.changed.clone())
            }
        }
        fn now_millis(&self) -> u64 {
            1
        }
    }
    fn dig() -> String {
        format!("sha256:{}", "c".repeat(64))
    }
    fn epochs(n: u64) -> AuthorityEpochVector {
        AuthorityEpochVector::new(AuthorityEpochVectorInput {
            version: 1,
            authority_schema_generation: 1,
            installation_epoch: 1,
            organization_epoch: 1,
            principal_epoch: n,
            team_membership_epochs: vec![],
            team_policy_epoch: None,
            project_membership_epoch: None,
            project_policy_epoch: None,
            resource_policy_epoch: None,
            gateway_catalog_generation: Some(1),
            depot_projection_watermark: None,
            credential_generation: Some(1),
            session_generation: 1,
        })
        .unwrap()
    }
    fn request(e: &AuthorityEpochVector) -> AgentExecutionRequest {
        let owner = OwnerScope::Personal(PrincipalId::new("p-1").unwrap());
        let binding = AuthorityBinding::new(
            PrincipalId::new("p-1").unwrap(),
            owner.clone(),
            Capability::ScopeOperate,
            "EXECUTE",
            ResourceId::new("agent-1").unwrap(),
            None,
        )
        .unwrap();
        AgentExecutionRequest {
            definition: AgentDefinition {
                id: "agent-1".into(),
                owner: owner.clone(),
                revision: AgentRevision {
                    version: 1,
                    content_digest: dig(),
                    repository_digest: dig(),
                    image_digest: dig(),
                    harness_digest: dig(),
                    loadout_digest: dig(),
                    catalog_generation: "catalog-1".into(),
                    credential_references: vec!["credential-ref".into()],
                },
                state: AgentState::Active,
                required_capabilities: vec![Capability::ScopeOperate],
                authority_epoch: 1,
                publication_epoch: 1,
                revocation_policy: RunningRevocationPolicy::StopAtSafeBoundary,
            },
            session: AgentSessionBinding {
                session_id: "session-1".into(),
                agent_id: "agent-1".into(),
                agent_version: 1,
                principal: PrincipalId::new("p-1").unwrap(),
                owner,
                catalog_generation: "catalog-1".into(),
                authority_fingerprint: e.fingerprint().as_str().into(),
                lease_expires_at: 100,
            },
            lease: AuthorityLease::new(
                binding,
                e,
                1,
                100,
                [
                    AuthoritySafeBoundary::BeforeDispatch,
                    AuthoritySafeBoundary::BeforeCommit,
                ],
            )
            .unwrap(),
            bounds: AgentResourceBounds {
                max_runtime_millis: 100,
                max_output_bytes: 100,
                max_external_effects: 1,
            },
        }
    }
    #[tokio::test]
    async fn admission_and_final_boundary_observe_revocation() {
        let initial = epochs(1);
        assert!(
            execute_agent(
                &Auth(initial.clone()),
                &Exec,
                request(&initial),
                Cancellation::new(),
                1
            )
            .await
            .is_ok()
        );
        assert_eq!(
            execute_agent(
                &Auth(epochs(2)),
                &Exec,
                request(&initial),
                Cancellation::new(),
                1
            )
            .await
            .unwrap_err(),
            AgentRuntimeError::Lease(AuthorityLeaseError::AuthorityChanged)
        );
    }

    #[tokio::test]
    async fn lease_must_name_this_agent_owner_principal_and_fingerprint() {
        let initial = epochs(1);
        let mut other_agent = request(&initial);
        other_agent.definition.id = "agent-2".into();
        other_agent.session.agent_id = "agent-2".into();
        assert_eq!(
            execute_agent(
                &Auth(initial.clone()),
                &Exec,
                other_agent,
                Cancellation::new(),
                1
            )
            .await
            .unwrap_err(),
            AgentRuntimeError::BindingMismatch
        );
        let mut other_principal = request(&initial);
        other_principal.session.principal = PrincipalId::new("p-2").unwrap();
        assert_eq!(
            execute_agent(
                &Auth(initial.clone()),
                &Exec,
                other_principal,
                Cancellation::new(),
                1
            )
            .await
            .unwrap_err(),
            AgentRuntimeError::BindingMismatch
        );
        let mut stale_fingerprint = request(&initial);
        stale_fingerprint.session.authority_fingerprint = epochs(7).fingerprint().as_str().into();
        assert_eq!(
            execute_agent(
                &Auth(initial.clone()),
                &Exec,
                stale_fingerprint,
                Cancellation::new(),
                1
            )
            .await
            .unwrap_err(),
            AgentRuntimeError::BindingMismatch
        );
        let mut needs_more = request(&initial);
        needs_more.definition.required_capabilities = vec![Capability::ScopeManage];
        assert_eq!(
            execute_agent(
                &Auth(initial.clone()),
                &Exec,
                needs_more,
                Cancellation::new(),
                1
            )
            .await
            .unwrap_err(),
            AgentRuntimeError::NotDispatchable
        );
    }

    #[tokio::test]
    async fn runtime_bound_cancels_an_overrunning_executor() {
        let initial = epochs(1);
        let cancellation = Cancellation::new();
        let authority = Auth(initial.clone());
        let mut bounded = request(&initial);
        bounded.bounds.max_runtime_millis = 20;
        let execution = execute_agent(&authority, &SlowExec, bounded, cancellation.clone(), 1);
        assert_eq!(
            execution.await.unwrap_err(),
            AgentRuntimeError::ResourceLimit
        );
        assert!(cancellation.is_cancelled());
    }

    #[tokio::test]
    async fn the_runtime_clock_cannot_be_rewound_by_the_dispatcher() {
        struct LateClock(AuthorityEpochVector);
        impl AgentAuthority for LateClock {
            async fn current_epochs(&self) -> Result<AuthorityEpochVector, AgentRuntimeError> {
                Ok(self.0.clone())
            }
            fn now_millis(&self) -> u64 {
                1_000
            }
        }
        let initial = epochs(1);
        // The lease expires at 100; a dispatcher claiming `now == 1` cannot
        // resurrect it once the runtime clock has passed expiry.
        assert_eq!(
            execute_agent(
                &LateClock(initial.clone()),
                &Exec,
                request(&initial),
                Cancellation::new(),
                1
            )
            .await
            .unwrap_err(),
            AgentRuntimeError::Lease(AuthorityLeaseError::Expired)
        );
    }

    #[tokio::test]
    async fn runtime_owns_final_boundary_even_when_executor_omits_it() {
        let initial = epochs(1);
        let authority = ChangingAuth {
            initial: initial.clone(),
            changed: epochs(2),
            reads: std::sync::atomic::AtomicUsize::new(0),
        };
        assert_eq!(
            execute_agent(
                &authority,
                &ExecWithoutFinalCheck,
                request(&initial),
                Cancellation::new(),
                1,
            )
            .await
            .unwrap_err(),
            AgentRuntimeError::Revoked
        );
    }
}
