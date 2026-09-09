//! Surface-neutral Dev Container execution and recovery orchestration.

use std::{collections::BTreeSet, future::Future, pin::Pin};

use labby_primitives::dev_container::{
    ApprovedTemplate, DevContainerId, HostCapability, ImageDigest, LifecycleNonce,
};
use thiserror::Error;

use crate::authority::{
    AuthorityEpochVector, AuthorityLease, AuthorityLeaseError, AuthoritySafeBoundary,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineState {
    Missing,
    Running,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryAction {
    None,
    Start,
    Stop,
    Destroy,
    MarkFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurableIntent {
    Running,
    Stopped,
    Deleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineHandle {
    pub instance_id: DevContainerId,
    pub lifecycle_nonce: LifecycleNonce,
}

/// Engine launch request. `image_digest` must be the canonical `sha256:` image
/// of the approved template; [`create`] re-parses and pins it before any engine
/// call, and [`EngineCreateRequest::for_template`] fills it from the template
/// so callers never copy the digest by hand.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineCreateRequest {
    pub handle: EngineHandle,
    pub image_digest: String,
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub lifetime_seconds: u64,
    pub host_capabilities: BTreeSet<HostCapability>,
}

impl EngineCreateRequest {
    /// Build a request whose image is pinned to the approved template.
    #[must_use]
    pub fn for_template(
        handle: EngineHandle,
        template: &ApprovedTemplate,
        resources: crate::dev_container::LaunchResources,
        host_capabilities: BTreeSet<HostCapability>,
    ) -> Self {
        Self {
            handle,
            image_digest: template.image().as_str().to_owned(),
            cpu_millis: resources.cpu_millis,
            memory_bytes: resources.memory_bytes,
            disk_bytes: resources.disk_bytes,
            lifetime_seconds: resources.lifetime_seconds,
            host_capabilities,
        }
    }

    /// The typed image digest, when canonical.
    pub fn image(&self) -> Result<ImageDigest, crate::dev_container::DevContainerAdmissionError> {
        ImageDigest::new(self.image_digest.clone())
            .map_err(|_| crate::dev_container::DevContainerAdmissionError::ImageDigestMismatch)
    }
}

pub trait ContainerRuntime: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn create<'a>(
        &'a self,
        request: EngineCreateRequest,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>>;
    fn inspect<'a>(
        &'a self,
        handle: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<EngineState, Self::Error>> + Send + 'a>>;
    fn start<'a>(
        &'a self,
        handle: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>>;
    fn stop<'a>(
        &'a self,
        handle: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>>;
    fn destroy<'a>(
        &'a self,
        handle: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>>;
}

#[derive(Clone, Copy, Debug, Error)]
#[error("Dev Container runtime is disabled")]
pub struct DisabledRuntimeError;

#[derive(Default)]
pub struct DisabledContainerRuntime;

impl ContainerRuntime for DisabledContainerRuntime {
    type Error = DisabledRuntimeError;
    fn create<'a>(
        &'a self,
        _: EngineCreateRequest,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(DisabledRuntimeError) })
    }
    fn inspect<'a>(
        &'a self,
        _: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<EngineState, Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(DisabledRuntimeError) })
    }
    fn start<'a>(
        &'a self,
        _: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(DisabledRuntimeError) })
    }
    fn stop<'a>(
        &'a self,
        _: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(DisabledRuntimeError) })
    }
    fn destroy<'a>(
        &'a self,
        _: &'a EngineHandle,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
        Box::pin(async { Err(DisabledRuntimeError) })
    }
}

#[derive(Debug, Error)]
pub enum RuntimeError<E> {
    #[error("Dev Container authority lease is no longer valid")]
    Authority(#[source] AuthorityLeaseError),
    #[error("Dev Container authority could not be refreshed")]
    AuthorityUnavailable,
    #[error("Dev Container launch was denied by its approved template")]
    Admission(#[source] crate::dev_container::DevContainerAdmissionError),
    #[error("container runtime operation failed")]
    Engine(#[source] E),
}

pub fn recovery_action(intent: DurableIntent, engine: EngineState) -> RecoveryAction {
    match (intent, engine) {
        (DurableIntent::Running, EngineState::Stopped) => RecoveryAction::Start,
        (DurableIntent::Running, EngineState::Missing) => RecoveryAction::MarkFailed,
        (DurableIntent::Stopped, EngineState::Running) => RecoveryAction::Stop,
        (DurableIntent::Deleted, EngineState::Running | EngineState::Stopped) => {
            RecoveryAction::Destroy
        }
        _ => RecoveryAction::None,
    }
}

pub async fn create<E: ContainerRuntime + ?Sized>(
    engine: &E,
    lease: &AuthorityLease,
    epochs: &AuthorityEpochVector,
    now_millis: u64,
    template: &ApprovedTemplate,
    request: EngineCreateRequest,
) -> Result<(), RuntimeError<E::Error>> {
    lease
        .validate_at(
            AuthoritySafeBoundary::BeforeExternalEffect,
            now_millis,
            epochs,
        )
        .map_err(RuntimeError::Authority)?;
    // The engine only ever sees the template's approved image.
    let image = request.image().map_err(RuntimeError::Admission)?;
    if &image != template.image() {
        return Err(RuntimeError::Admission(
            crate::dev_container::DevContainerAdmissionError::ImageDigestMismatch,
        ));
    }
    crate::dev_container::validate_launch(
        template,
        &crate::dev_container::LaunchRequest {
            resources: crate::dev_container::LaunchResources {
                cpu_millis: request.cpu_millis,
                memory_bytes: request.memory_bytes,
                disk_bytes: request.disk_bytes,
                lifetime_seconds: request.lifetime_seconds,
            },
            host_capabilities: request.host_capabilities.clone(),
        },
    )
    .map_err(RuntimeError::Admission)?;
    engine.create(request).await.map_err(RuntimeError::Engine)
}

/// Re-read both policy and time before inspection and again after inspection,
/// immediately before a mutating engine call.
pub async fn reconcile<E, F, Fut>(
    engine: &E,
    lease: &AuthorityLease,
    mut current_authority: F,
    handle: &EngineHandle,
    intent: DurableIntent,
) -> Result<RecoveryAction, RuntimeError<E::Error>>
where
    E: ContainerRuntime + ?Sized,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(AuthorityEpochVector, u64), RuntimeError<E::Error>>>,
{
    let (epochs, now_millis) = current_authority().await?;
    lease
        .validate_at(
            AuthoritySafeBoundary::BeforeExternalEffect,
            now_millis,
            &epochs,
        )
        .map_err(RuntimeError::Authority)?;
    let action = recovery_action(
        intent,
        engine.inspect(handle).await.map_err(RuntimeError::Engine)?,
    );
    if matches!(action, RecoveryAction::None | RecoveryAction::MarkFailed) {
        return Ok(action);
    }
    let (epochs, now_millis) = current_authority().await?;
    lease
        .validate_at(
            AuthoritySafeBoundary::BeforeExternalEffect,
            now_millis,
            &epochs,
        )
        .map_err(RuntimeError::Authority)?;
    match action {
        RecoveryAction::Start => engine.start(handle).await,
        RecoveryAction::Stop => engine.stop(handle).await,
        RecoveryAction::Destroy => engine.destroy(handle).await,
        RecoveryAction::None | RecoveryAction::MarkFailed => unreachable!(),
    }
    .map_err(RuntimeError::Engine)?;
    Ok(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{
        AUTHORITY_EPOCH_VECTOR_VERSION, AuthorityBinding, AuthorityEpochVectorInput,
    };
    use labby_primitives::access::{Capability, OwnerScope, PrincipalId, ResourceId};
    use labby_primitives::dev_container::{
        DevContainerQuota, DevContainerTemplateId, HostCapabilityPolicy, ImageDigest,
    };
    use std::sync::Mutex;

    #[derive(Debug, Error)]
    #[error("fake engine error")]
    struct FakeError;

    struct FakeEngine {
        state: Mutex<EngineState>,
        calls: Mutex<Vec<&'static str>>,
    }
    impl ContainerRuntime for FakeEngine {
        type Error = FakeError;
        fn create<'a>(
            &'a self,
            _: EngineCreateRequest,
        ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push("create");
                Ok(())
            })
        }
        fn inspect<'a>(
            &'a self,
            _: &'a EngineHandle,
        ) -> Pin<Box<dyn Future<Output = Result<EngineState, Self::Error>> + Send + 'a>> {
            Box::pin(async move { Ok(*self.state.lock().unwrap()) })
        }
        fn start<'a>(
            &'a self,
            _: &'a EngineHandle,
        ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push("start");
                *self.state.lock().unwrap() = EngineState::Running;
                Ok(())
            })
        }
        fn stop<'a>(
            &'a self,
            _: &'a EngineHandle,
        ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push("stop");
                *self.state.lock().unwrap() = EngineState::Stopped;
                Ok(())
            })
        }
        fn destroy<'a>(
            &'a self,
            _: &'a EngineHandle,
        ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push("destroy");
                *self.state.lock().unwrap() = EngineState::Missing;
                Ok(())
            })
        }
    }

    fn authority() -> (AuthorityLease, AuthorityEpochVector) {
        let epochs = AuthorityEpochVector::new(AuthorityEpochVectorInput {
            version: AUTHORITY_EPOCH_VECTOR_VERSION,
            authority_schema_generation: 1,
            installation_epoch: 1,
            organization_epoch: 1,
            principal_epoch: 1,
            team_membership_epochs: vec![],
            team_policy_epoch: None,
            project_membership_epoch: None,
            project_policy_epoch: None,
            resource_policy_epoch: Some(1),
            gateway_catalog_generation: None,
            depot_projection_watermark: None,
            credential_generation: None,
            session_generation: 1,
        })
        .unwrap();
        let binding = AuthorityBinding::new(
            PrincipalId::new("p1").unwrap(),
            OwnerScope::Personal(PrincipalId::new("p1").unwrap()),
            Capability::ScopeOperate,
            "dev_containers.reconcile",
            ResourceId::new("dc-1").unwrap(),
            None,
        )
        .unwrap();
        let lease = AuthorityLease::new(
            binding,
            &epochs,
            100,
            1_000,
            [AuthoritySafeBoundary::BeforeExternalEffect],
        )
        .unwrap();
        (lease, epochs)
    }

    fn handle() -> EngineHandle {
        EngineHandle {
            instance_id: DevContainerId::new("dc-1").unwrap(),
            lifecycle_nonce: LifecycleNonce::new("11111111111111111111111111111111").unwrap(),
        }
    }

    #[test]
    fn recovery_is_restart_safe_and_cleanup_is_idempotent() {
        assert_eq!(
            recovery_action(DurableIntent::Running, EngineState::Stopped),
            RecoveryAction::Start
        );
        assert_eq!(
            recovery_action(DurableIntent::Running, EngineState::Missing),
            RecoveryAction::MarkFailed
        );
        assert_eq!(
            recovery_action(DurableIntent::Deleted, EngineState::Running),
            RecoveryAction::Destroy
        );
        assert_eq!(
            recovery_action(DurableIntent::Deleted, EngineState::Missing),
            RecoveryAction::None
        );
    }

    #[tokio::test]
    async fn fake_engine_reconciles_restart_and_idempotent_destroy() {
        let engine = FakeEngine {
            state: Mutex::new(EngineState::Stopped),
            calls: Mutex::new(vec![]),
        };
        let (lease, epochs) = authority();
        assert_eq!(
            reconcile(
                &engine,
                &lease,
                || std::future::ready(Ok((epochs.clone(), 200))),
                &handle(),
                DurableIntent::Running
            )
            .await
            .unwrap(),
            RecoveryAction::Start
        );
        assert_eq!(
            reconcile(
                &engine,
                &lease,
                || std::future::ready(Ok((epochs.clone(), 200))),
                &handle(),
                DurableIntent::Deleted
            )
            .await
            .unwrap(),
            RecoveryAction::Destroy
        );
        assert_eq!(
            reconcile(
                &engine,
                &lease,
                || std::future::ready(Ok((epochs.clone(), 200))),
                &handle(),
                DurableIntent::Deleted
            )
            .await
            .unwrap(),
            RecoveryAction::None
        );
        assert_eq!(*engine.calls.lock().unwrap(), vec!["start", "destroy"]);
    }

    #[tokio::test]
    async fn reconciliation_rechecks_revocation_and_expiry_after_inspection() {
        for expire in [false, true] {
            let engine = FakeEngine {
                state: Mutex::new(EngineState::Stopped),
                calls: Mutex::new(vec![]),
            };
            let (lease, epochs) = authority();
            let mut checks = 0;
            let result = reconcile(
                &engine,
                &lease,
                || {
                    checks += 1;
                    std::future::ready(if checks == 2 && !expire {
                        Err(RuntimeError::Authority(
                            AuthorityLeaseError::AuthorityChanged,
                        ))
                    } else {
                        Ok((epochs.clone(), if checks == 2 { 1_000 } else { 200 }))
                    })
                },
                &handle(),
                DurableIntent::Running,
            )
            .await;
            assert!(matches!(result, Err(RuntimeError::Authority(_))));
            assert_eq!(checks, 2);
            assert!(engine.calls.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn create_denies_unapproved_host_access_before_engine_effect() {
        let engine = FakeEngine {
            state: Mutex::new(EngineState::Missing),
            calls: Mutex::new(vec![]),
        };
        let template = ApprovedTemplate::new(
            DevContainerTemplateId::new("safe").unwrap(),
            ImageDigest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
            DevContainerQuota {
                max_active_instances: 1,
                cpu_millis: 1_000,
                memory_bytes: 2_000,
                disk_bytes: 3_000,
                max_lifetime_seconds: 60,
            },
            HostCapabilityPolicy::deny_all(),
        )
        .unwrap();
        let (lease, epochs) = authority();
        let result = create(
            &engine,
            &lease,
            &epochs,
            200,
            &template,
            EngineCreateRequest {
                handle: handle(),
                image_digest: template.image().as_str().into(),
                cpu_millis: 500,
                memory_bytes: 1_000,
                disk_bytes: 2_000,
                lifetime_seconds: 30,
                host_capabilities: BTreeSet::from([HostCapability::HostNetwork]),
            },
        )
        .await;
        assert!(matches!(result, Err(RuntimeError::Admission(_))));
        assert!(engine.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_pins_the_image_to_the_approved_template() {
        let engine = FakeEngine {
            state: Mutex::new(EngineState::Missing),
            calls: Mutex::new(vec![]),
        };
        let template = ApprovedTemplate::new(
            DevContainerTemplateId::new("safe").unwrap(),
            ImageDigest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
            DevContainerQuota {
                max_active_instances: 1,
                cpu_millis: 1_000,
                memory_bytes: 2_000,
                disk_bytes: 3_000,
                max_lifetime_seconds: 60,
            },
            HostCapabilityPolicy::deny_all(),
        )
        .unwrap();
        let (lease, epochs) = authority();
        let mut request = EngineCreateRequest::for_template(
            handle(),
            &template,
            crate::dev_container::LaunchResources {
                cpu_millis: 500,
                memory_bytes: 1_000,
                disk_bytes: 2_000,
                lifetime_seconds: 30,
            },
            BTreeSet::new(),
        );
        assert_eq!(request.image().unwrap(), *template.image());
        request.image_digest = format!("sha256:{}", "b".repeat(64));
        let result = create(&engine, &lease, &epochs, 200, &template, request).await;
        assert!(matches!(
            result,
            Err(RuntimeError::Admission(
                crate::dev_container::DevContainerAdmissionError::ImageDigestMismatch
            ))
        ));
        assert!(engine.calls.lock().unwrap().is_empty());
    }
}
