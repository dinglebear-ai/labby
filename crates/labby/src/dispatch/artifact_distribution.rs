//! Cross-store orchestration for managed local Artifact mirrors.
//!
//! AccessStore owns distribution/control state; ArtifactStore owns immutable bytes and
//! local heads. These stores cannot commit atomically, so every operation is staged in
//! AccessStore, applies verified ArtifactStore bytes, then finalizes AccessStore. Retries
//! reuse the same operation identity and are deliberately idempotent.

use std::sync::Arc;

use labby_primitives::access::OwnerScope;
use labby_runtime::artifacts::{
    ArtifactAcquisition, ArtifactDescriptor, ArtifactError, ArtifactForkRequest, ArtifactRecord,
    ArtifactStore,
};
use thiserror::Error;

use crate::access::{
    AccessStore, AccessStoreError, ArtifactAuthorityRecord, ArtifactSubscriptionUpdatePolicy,
    BeginManagedArtifactMirrorUpdate, ManagedArtifactMirror, ManagedArtifactMirrorMode,
    ManagedArtifactMirrorStatus, ManagedArtifactSubscription, StageArtifactAuthority,
    StageManagedArtifactMirror,
};

#[derive(Debug, Error)]
pub(crate) enum ManagedArtifactDistributionError {
    #[error(transparent)]
    Access(#[from] AccessStoreError),
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error("managed Artifact distribution state is invalid: {0}")]
    State(&'static str),
}

#[derive(Clone, Debug)]
pub(crate) struct ManagedArtifactInstallRequest {
    pub(crate) mirror_id: String,
    pub(crate) operation_id: String,
    pub(crate) owner_principal_id: String,
    pub(crate) destination_id: Option<String>,
    pub(crate) source_provider_authority: String,
    pub(crate) source_assignment_id: String,
    pub(crate) source_scope: OwnerScope,
    pub(crate) mode: ManagedArtifactMirrorMode,
    pub(crate) policy_epoch: u64,
    pub(crate) update_policy: Option<ArtifactSubscriptionUpdatePolicy>,
    pub(crate) now: i64,
    pub(crate) acquisition: ArtifactAcquisition,
}

#[derive(Clone, Debug)]
pub(crate) struct ManagedArtifactFollowUpdateRequest {
    pub(crate) mirror_id: String,
    pub(crate) operation_id: String,
    pub(crate) expected_local_revision_id: String,
    pub(crate) policy_epoch: u64,
    pub(crate) now: i64,
    pub(crate) acquisition: ArtifactAcquisition,
}

#[derive(Clone, Debug)]
pub(crate) struct PersonalArtifactForkRequest {
    pub(crate) operation_id: String,
    pub(crate) owner_principal_id: String,
    pub(crate) source_provider_authority: String,
    pub(crate) source_assignment_id: String,
    pub(crate) source_scope: OwnerScope,
    pub(crate) target_name: String,
    pub(crate) title: Option<String>,
    pub(crate) forked_at: Option<String>,
    pub(crate) policy_epoch: u64,
    pub(crate) now: i64,
    pub(crate) acquisition: ArtifactAcquisition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManagedArtifactInstallOutcome {
    pub(crate) mirror: ManagedArtifactMirror,
    pub(crate) subscription: Option<ManagedArtifactSubscription>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PersonalArtifactForkOutcome {
    pub(crate) artifact: ArtifactRecord,
    pub(crate) authority: ArtifactAuthorityRecord,
}

#[derive(Clone)]
pub(crate) struct ManagedArtifactCoordinator {
    access: AccessStore,
    artifacts: Arc<ArtifactStore>,
}

impl ManagedArtifactCoordinator {
    pub(crate) fn new(access: AccessStore, artifacts: Arc<ArtifactStore>) -> Self {
        Self { access, artifacts }
    }

    pub(crate) async fn install(
        &self,
        request: ManagedArtifactInstallRequest,
    ) -> Result<ManagedArtifactInstallOutcome, ManagedArtifactDistributionError> {
        request.acquisition.validate()?;
        match (request.mode, request.update_policy) {
            (ManagedArtifactMirrorMode::Pinned, None)
            | (ManagedArtifactMirrorMode::Followed, Some(_)) => {}
            (ManagedArtifactMirrorMode::Pinned, Some(_)) => {
                return Err(ManagedArtifactDistributionError::State(
                    "pinned_mirror_has_follow_policy",
                ));
            }
            (ManagedArtifactMirrorMode::Followed, None) => {
                return Err(ManagedArtifactDistributionError::State(
                    "followed_mirror_missing_update_policy",
                ));
            }
        }

        let source_artifact_id = request.acquisition.interchange.descriptor.id.clone();
        let source_revision_id = request.acquisition.interchange.revision.id.clone();
        let staged = self
            .access
            .stage_managed_artifact_mirror(StageManagedArtifactMirror {
                mirror_id: request.mirror_id.clone(),
                operation_id: request.operation_id.clone(),
                owner_principal_id: request.owner_principal_id,
                destination_id: request.destination_id,
                source_provider_authority: request.source_provider_authority,
                source_assignment_id: request.source_assignment_id,
                source_scope: request.source_scope,
                source_artifact_id: source_artifact_id.clone(),
                source_revision_id: source_revision_id.clone(),
                local_artifact_id: source_artifact_id,
                mode: request.mode,
                last_authorized_policy_epoch: request.policy_epoch,
                now: request.now,
            })
            .await?;

        let staged = match staged.status {
            ManagedArtifactMirrorStatus::Pending => {
                self.access
                    .mark_managed_artifact_mirror_committing(
                        request.mirror_id.clone(),
                        request.operation_id.clone(),
                        request.now,
                    )
                    .await?
            }
            ManagedArtifactMirrorStatus::Committing | ManagedArtifactMirrorStatus::Active => staged,
            ManagedArtifactMirrorStatus::AccessRevoked
            | ManagedArtifactMirrorStatus::SourceWithdrawn
            | ManagedArtifactMirrorStatus::Removed
            | ManagedArtifactMirrorStatus::Failed => {
                return Err(ManagedArtifactDistributionError::State("mirror_terminal"));
            }
        };
        if staged.source_revision_id != source_revision_id {
            return Err(ManagedArtifactDistributionError::State(
                "source_revision_changed",
            ));
        }

        let local = self
            .artifacts
            .install_acquisition_exact(request.acquisition)?;
        let mirror = self
            .access
            .activate_managed_artifact_mirror(
                request.mirror_id.clone(),
                request.operation_id,
                local.current_revision_id.clone(),
                request.policy_epoch,
                request.now,
            )
            .await?;

        let subscription = if request.mode == ManagedArtifactMirrorMode::Followed {
            let update_policy =
                request
                    .update_policy
                    .ok_or(ManagedArtifactDistributionError::State(
                        "followed_mirror_missing_update_policy",
                    ))?;
            Some(
                self.access
                    .put_artifact_subscription(ManagedArtifactSubscription {
                        mirror_id: request.mirror_id,
                        update_policy,
                        last_observed_revision_id: Some(source_revision_id.clone()),
                        last_applied_revision_id: Some(source_revision_id),
                        last_checked_at: Some(request.now),
                        active: true,
                        updated_at: request.now,
                    })
                    .await?,
            )
        } else {
            None
        };

        Ok(ManagedArtifactInstallOutcome {
            mirror,
            subscription,
        })
    }

    pub(crate) async fn fork_personal(
        &self,
        request: PersonalArtifactForkRequest,
    ) -> Result<PersonalArtifactForkOutcome, ManagedArtifactDistributionError> {
        request.acquisition.validate()?;
        let source_artifact_id = request.acquisition.interchange.descriptor.id.clone();
        let source_revision_id = request.acquisition.interchange.revision.id.clone();
        let assignment = self
            .access
            .artifact_assignment_distribution(
                request.source_assignment_id.clone(),
                request.source_provider_authority.clone(),
                source_artifact_id.clone(),
            )
            .await?
            .ok_or(AccessStoreError::NotAuthorized)?;
        if assignment.source_scope != request.source_scope {
            return Err(AccessStoreError::NotAuthorized.into());
        }

        let descriptor = ArtifactDescriptor::for_identity(
            &request.acquisition.interchange.descriptor.kind,
            "personal",
            &request.target_name,
        )?;
        let principal =
            labby_primitives::access::PrincipalId::new(request.owner_principal_id.clone())
                .map_err(|_| AccessStoreError::InvalidArtifactDistributionInput)?;
        let owner = OwnerScope::Personal(principal);
        let authority = self
            .access
            .stage_artifact_authority(StageArtifactAuthority {
                artifact_id: descriptor.id.clone(),
                operation_id: request.operation_id.clone(),
                owner,
                policy_epoch: request.policy_epoch,
                now: request.now,
            })
            .await?;
        let authority = match authority.status {
            crate::access::ArtifactAuthorityStatus::Pending => {
                self.access
                    .mark_artifact_authority_committing(
                        descriptor.id.clone(),
                        request.operation_id.clone(),
                        request.now,
                    )
                    .await?
            }
            crate::access::ArtifactAuthorityStatus::Committing
            | crate::access::ArtifactAuthorityStatus::Active => authority,
            crate::access::ArtifactAuthorityStatus::Failed => {
                return Err(ManagedArtifactDistributionError::State(
                    "personal_fork_authority_failed",
                ));
            }
        };
        if authority.owner.id() != request.owner_principal_id {
            return Err(ManagedArtifactDistributionError::State(
                "personal_fork_owner_mismatch",
            ));
        }

        let artifact = self.artifacts.fork_acquisition_exact(
            request.acquisition,
            ArtifactForkRequest {
                source_artifact_id,
                namespace: "personal".into(),
                name: request.target_name,
                title: request.title,
                following: false,
                forked_at: request.forked_at,
            },
        )?;
        if artifact.current_revision_id != source_revision_id {
            return Err(ManagedArtifactDistributionError::State(
                "personal_fork_revision_mismatch",
            ));
        }
        let authority = self
            .access
            .activate_artifact_authority(
                artifact.descriptor.id.clone(),
                request.operation_id,
                request.policy_epoch,
                request.now,
            )
            .await?;
        Ok(PersonalArtifactForkOutcome {
            artifact,
            authority,
        })
    }

    pub(crate) async fn apply_follow_update(
        &self,
        request: ManagedArtifactFollowUpdateRequest,
    ) -> Result<ManagedArtifactInstallOutcome, ManagedArtifactDistributionError> {
        self.apply_follow_update_with_policy(request, false).await
    }

    /// Apply one exact revision under an `auto_approved` follow subscription.
    ///
    /// Reauthorization is re-run per revision, so following never becomes standing permission for
    /// future arbitrary revisions. No caller exists yet: the background follow-observation loop that
    /// drives this path is the next distribution slice, so the behavior is covered by tests only.
    #[allow(
        dead_code,
        reason = "awaiting the follow-observation loop that drives it"
    )]
    pub(crate) async fn apply_auto_approved_follow_update(
        &self,
        request: ManagedArtifactFollowUpdateRequest,
    ) -> Result<ManagedArtifactInstallOutcome, ManagedArtifactDistributionError> {
        self.apply_follow_update_with_policy(request, true).await
    }

    async fn apply_follow_update_with_policy(
        &self,
        request: ManagedArtifactFollowUpdateRequest,
        require_auto_approved: bool,
    ) -> Result<ManagedArtifactInstallOutcome, ManagedArtifactDistributionError> {
        request.acquisition.validate()?;
        let source_artifact_id = request.acquisition.interchange.descriptor.id.clone();
        let source_revision_id = request.acquisition.interchange.revision.id.clone();
        let current = self
            .access
            .managed_artifact_mirror(request.mirror_id.clone())
            .await?
            .ok_or(AccessStoreError::ArtifactMirrorUnavailable)?;
        if current.mode != ManagedArtifactMirrorMode::Followed
            || current.source_artifact_id != source_artifact_id
            || current.local_artifact_id != source_artifact_id
        {
            return Err(ManagedArtifactDistributionError::State(
                "follow_mirror_binding_mismatch",
            ));
        }
        let subscription = self
            .access
            .artifact_subscription(request.mirror_id.clone())
            .await?
            .ok_or(ManagedArtifactDistributionError::State(
                "follow_subscription_missing",
            ))?;
        if !subscription.active {
            return Err(ManagedArtifactDistributionError::State(
                "follow_subscription_paused",
            ));
        }
        if subscription.update_policy == ArtifactSubscriptionUpdatePolicy::Pinned {
            return Err(ManagedArtifactDistributionError::State(
                "follow_subscription_pinned",
            ));
        }
        if require_auto_approved
            && subscription.update_policy != ArtifactSubscriptionUpdatePolicy::AutoApproved
        {
            return Err(ManagedArtifactDistributionError::State(
                "follow_subscription_not_auto_approved",
            ));
        }

        let already_finalized = current.status == ManagedArtifactMirrorStatus::Active
            && current.operation_id == request.operation_id
            && current.source_revision_id == source_revision_id
            && current.local_revision_id.as_deref() == Some(source_revision_id.as_str());

        let mirror = if already_finalized {
            let local = self.artifacts.get(&source_artifact_id)?;
            if local.current_revision_id != source_revision_id {
                return Err(ManagedArtifactDistributionError::State(
                    "active_mirror_local_head_mismatch",
                ));
            }
            current
        } else {
            self.access
                .begin_managed_artifact_mirror_update(BeginManagedArtifactMirrorUpdate {
                    mirror_id: request.mirror_id.clone(),
                    operation_id: request.operation_id.clone(),
                    source_revision_id: source_revision_id.clone(),
                    expected_local_revision_id: request.expected_local_revision_id.clone(),
                    policy_epoch: request.policy_epoch,
                    now: request.now,
                })
                .await?;
            let local = self.artifacts.apply_acquisition_update(
                &request.expected_local_revision_id,
                request.acquisition,
            )?;
            self.access
                .activate_managed_artifact_mirror(
                    request.mirror_id.clone(),
                    request.operation_id,
                    local.current_revision_id,
                    request.policy_epoch,
                    request.now,
                )
                .await?
        };

        let subscription = self
            .access
            .put_artifact_subscription(ManagedArtifactSubscription {
                mirror_id: request.mirror_id,
                update_policy: subscription.update_policy,
                last_observed_revision_id: Some(source_revision_id.clone()),
                last_applied_revision_id: Some(source_revision_id),
                last_checked_at: Some(request.now),
                active: true,
                updated_at: request.now,
            })
            .await?;

        Ok(ManagedArtifactInstallOutcome {
            mirror,
            subscription: Some(subscription),
        })
    }

    /// Move an active managed mirror into a terminal restricted state and purge its managed bytes.
    ///
    /// No caller exists yet: revocation reconciliation (access revoked / source withdrawn) is the
    /// next distribution slice, so the behavior is covered by tests only.
    #[allow(
        dead_code,
        reason = "awaiting revocation reconciliation that drives it"
    )]
    pub(crate) async fn restrict_and_purge_managed_mirror(
        &self,
        mirror_id: String,
        restriction: ManagedArtifactMirrorStatus,
        now: i64,
    ) -> Result<ManagedArtifactMirror, ManagedArtifactDistributionError> {
        if !matches!(
            restriction,
            ManagedArtifactMirrorStatus::AccessRevoked
                | ManagedArtifactMirrorStatus::SourceWithdrawn
        ) {
            return Err(ManagedArtifactDistributionError::State(
                "invalid_managed_mirror_restriction",
            ));
        }
        let current = self
            .access
            .managed_artifact_mirror(mirror_id.clone())
            .await?
            .ok_or(AccessStoreError::ArtifactMirrorUnavailable)?;
        if current.status == ManagedArtifactMirrorStatus::Removed {
            return Ok(current);
        }
        let restricted = if current.status == restriction {
            current
        } else {
            self.access
                .restrict_managed_artifact_mirror(
                    mirror_id.clone(),
                    current.operation_id.clone(),
                    restriction,
                    now,
                )
                .await?
        };
        match self.artifacts.get(&restricted.local_artifact_id) {
            Ok(local) => {
                self.artifacts.purge_artifact_exact(
                    &restricted.local_artifact_id,
                    &local.current_revision_id,
                )?;
            }
            Err(ArtifactError::NotFound("record")) => {}
            Err(error) => return Err(error.into()),
        }
        let removed = self
            .access
            .restrict_managed_artifact_mirror(
                mirror_id,
                restricted.operation_id,
                ManagedArtifactMirrorStatus::Removed,
                now,
            )
            .await?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_auth::{Authenticator, VerifiedIdentity};
    use labby_runtime::artifacts::{
        ArtifactImportRequest, ArtifactProvider, ArtifactProviderRequest, LocalArtifactProvider,
    };

    async fn bootstrapped_access() -> (tempfile::TempDir, AccessStore) {
        let directory = crate::access::test_support::secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "owner",
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

    async fn source_acquisition(
        store: &ArtifactStore,
        record: &ArtifactRecord,
    ) -> ArtifactAcquisition {
        LocalArtifactProvider::new(store.clone())
            .acquire(
                &ArtifactProviderRequest::new(
                    record.descriptor.id.clone(),
                    Some(record.current_revision_id.clone()),
                )
                .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn install_authority(access: &AccessStore, artifact_id: &str) -> u64 {
        let owner = OwnerScope::Personal(
            labby_primitives::access::PrincipalId::new("bootstrap-owner").unwrap(),
        );
        access
            .put_artifact_source_policy(crate::access::ArtifactSourcePolicyRecord {
                provider_authority: "depot".into(),
                artifact_id: artifact_id.into(),
                source_scope: owner.clone(),
                policy_epoch: 1,
                ceiling: crate::access::ArtifactDistributionCeiling {
                    sync: true,
                    follow: true,
                    fork: true,
                    export: true,
                    reshare: true,
                },
                updated_at: 10,
            })
            .await
            .unwrap();
        access
            .put_artifact_assignment_distribution(
                crate::access::ArtifactAssignmentDistributionRecord {
                    assignment_id: "assignment-test".into(),
                    provider_authority: "depot".into(),
                    artifact_id: artifact_id.into(),
                    source_scope: owner,
                    policy_epoch: 1,
                    ceiling: crate::access::ArtifactDistributionCeiling {
                        sync: true,
                        follow: true,
                        fork: true,
                        export: true,
                        reshare: true,
                    },
                    active: true,
                    created_at: 11,
                    updated_at: 11,
                },
            )
            .await
            .unwrap();
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "owner",
        )
        .unwrap();
        access
            .artifact_distribution_authority(identity, "bootstrap-default".into(), None)
            .await
            .unwrap()
            .global_revision
    }

    #[tokio::test]
    async fn pinned_install_commits_exact_bytes_and_retry_is_idempotent() {
        let (_access_dir, access) = bootstrapped_access().await;
        let source_dir = tempfile::tempdir().unwrap();
        let source_package = tempfile::tempdir().unwrap();
        std::fs::write(source_package.path().join("a.txt"), b"alpha").unwrap();
        let source = ArtifactStore::new(source_dir.path().join("store")).unwrap();
        let source_record = source
            .import_local(
                ArtifactImportRequest::new("resource", "upstream", "pin-demo"),
                source_package.path(),
            )
            .unwrap();
        let acquisition = source_acquisition(&source, &source_record).await;
        let policy_epoch = install_authority(&access, &source_record.descriptor.id).await;

        let destination_dir = tempfile::tempdir().unwrap();
        let destination =
            Arc::new(ArtifactStore::new(destination_dir.path().join("store")).unwrap());
        let coordinator = ManagedArtifactCoordinator::new(access.clone(), Arc::clone(&destination));
        let request = ManagedArtifactInstallRequest {
            mirror_id: "mirror-pin".into(),
            operation_id: "operation-pin".into(),
            owner_principal_id: "bootstrap-owner".into(),
            destination_id: None,
            source_provider_authority: "depot".into(),
            source_assignment_id: "assignment-test".into(),
            source_scope: OwnerScope::Personal(
                labby_primitives::access::PrincipalId::new("bootstrap-owner").unwrap(),
            ),
            mode: ManagedArtifactMirrorMode::Pinned,
            policy_epoch,
            update_policy: None,
            now: 20,
            acquisition,
        };
        let first = coordinator.install(request.clone()).await.unwrap();
        let retried = coordinator.install(request).await.unwrap();
        assert_eq!(first, retried);
        assert_eq!(first.mirror.status, ManagedArtifactMirrorStatus::Active);
        assert_eq!(first.subscription, None);
        assert_eq!(
            destination
                .get(&source_record.descriptor.id)
                .unwrap()
                .current_revision_id,
            source_record.current_revision_id
        );
    }

    #[tokio::test]
    async fn personal_fork_commits_exact_lineage_and_activates_local_ownership() {
        let (_access_dir, access) = bootstrapped_access().await;
        let source_dir = tempfile::tempdir().unwrap();
        let source_package = tempfile::tempdir().unwrap();
        std::fs::write(source_package.path().join("a.txt"), b"alpha").unwrap();
        let source = ArtifactStore::new(source_dir.path().join("store")).unwrap();
        let source_record = source
            .import_local(
                ArtifactImportRequest::new("resource", "upstream", "fork-coordinator-demo"),
                source_package.path(),
            )
            .unwrap();
        let acquisition = source_acquisition(&source, &source_record).await;
        let policy_epoch = install_authority(&access, &source_record.descriptor.id).await;

        let destination_dir = tempfile::tempdir().unwrap();
        let destination =
            Arc::new(ArtifactStore::new(destination_dir.path().join("store")).unwrap());
        let coordinator = ManagedArtifactCoordinator::new(access, Arc::clone(&destination));
        let request = PersonalArtifactForkRequest {
            operation_id: "fork-operation-a".into(),
            owner_principal_id: "bootstrap-owner".into(),
            source_provider_authority: "depot".into(),
            source_assignment_id: "assignment-test".into(),
            source_scope: OwnerScope::Personal(
                labby_primitives::access::PrincipalId::new("bootstrap-owner").unwrap(),
            ),
            target_name: "my-personal-fork".into(),
            title: Some("My personal fork".into()),
            forked_at: Some("2026-09-17T12:00:00Z".into()),
            policy_epoch,
            now: 20,
            acquisition,
        };
        let outcome = coordinator.fork_personal(request.clone()).await.unwrap();
        assert_eq!(
            outcome.authority.status,
            crate::access::ArtifactAuthorityStatus::Active
        );
        assert_eq!(outcome.authority.owner.id(), "bootstrap-owner");
        assert_ne!(outcome.artifact.descriptor.id, source_record.descriptor.id);
        assert_eq!(
            outcome.artifact.current_revision_id,
            source_record.current_revision_id
        );
        assert_eq!(
            outcome.artifact.lineage.forked_from_artifact_id.as_deref(),
            Some(source_record.descriptor.id.as_str())
        );
        assert_eq!(
            outcome.artifact.lineage.forked_from_revision_id.as_deref(),
            Some(source_record.current_revision_id.as_str())
        );
        assert!(matches!(
            destination.get(&source_record.descriptor.id),
            Err(ArtifactError::NotFound("record"))
        ));
        assert_eq!(coordinator.fork_personal(request).await.unwrap(), outcome);
    }

    #[tokio::test]
    async fn revocation_disables_then_purges_managed_bytes_and_pauses_following() {
        let (_access_dir, access) = bootstrapped_access().await;
        let source_dir = tempfile::tempdir().unwrap();
        let source_package = tempfile::tempdir().unwrap();
        std::fs::write(source_package.path().join("a.txt"), b"alpha").unwrap();
        let source = ArtifactStore::new(source_dir.path().join("store")).unwrap();
        let source_record = source
            .import_local(
                ArtifactImportRequest::new("resource", "upstream", "revocation-demo"),
                source_package.path(),
            )
            .unwrap();
        let acquisition = source_acquisition(&source, &source_record).await;
        let policy_epoch = install_authority(&access, &source_record.descriptor.id).await;
        let destination_dir = tempfile::tempdir().unwrap();
        let destination =
            Arc::new(ArtifactStore::new(destination_dir.path().join("store")).unwrap());
        let coordinator = ManagedArtifactCoordinator::new(access.clone(), Arc::clone(&destination));
        let installed = coordinator
            .install(ManagedArtifactInstallRequest {
                mirror_id: "mirror-revoke".into(),
                operation_id: "operation-revoke-install".into(),
                owner_principal_id: "bootstrap-owner".into(),
                destination_id: None,
                source_provider_authority: "depot".into(),
                source_assignment_id: "assignment-test".into(),
                source_scope: OwnerScope::Personal(
                    labby_primitives::access::PrincipalId::new("bootstrap-owner").unwrap(),
                ),
                mode: ManagedArtifactMirrorMode::Followed,
                policy_epoch,
                update_policy: Some(ArtifactSubscriptionUpdatePolicy::Notify),
                now: 20,
                acquisition,
            })
            .await
            .unwrap();
        assert!(destination.get(&source_record.descriptor.id).is_ok());
        assert!(installed.subscription.as_ref().unwrap().active);

        let removed = coordinator
            .restrict_and_purge_managed_mirror(
                "mirror-revoke".into(),
                ManagedArtifactMirrorStatus::AccessRevoked,
                30,
            )
            .await
            .unwrap();
        assert_eq!(removed.status, ManagedArtifactMirrorStatus::Removed);
        assert!(matches!(
            destination.get(&source_record.descriptor.id),
            Err(ArtifactError::NotFound("record"))
        ));
        let subscription = access
            .artifact_subscription("mirror-revoke".into())
            .await
            .unwrap()
            .unwrap();
        assert!(!subscription.active);
        assert_eq!(
            coordinator
                .restrict_and_purge_managed_mirror(
                    "mirror-revoke".into(),
                    ManagedArtifactMirrorStatus::AccessRevoked,
                    31,
                )
                .await
                .unwrap()
                .status,
            ManagedArtifactMirrorStatus::Removed
        );
    }

    #[tokio::test]
    async fn automatic_follow_update_requires_auto_approved_policy() {
        let (_access_dir, access) = bootstrapped_access().await;
        let source_dir = tempfile::tempdir().unwrap();
        let source_package = tempfile::tempdir().unwrap();
        std::fs::write(source_package.path().join("a.txt"), b"alpha").unwrap();
        let source = ArtifactStore::new(source_dir.path().join("store")).unwrap();
        let first_source = source
            .import_local(
                ArtifactImportRequest::new("resource", "upstream", "auto-follow-demo"),
                source_package.path(),
            )
            .unwrap();
        let first_acquisition = source_acquisition(&source, &first_source).await;
        let policy_epoch = install_authority(&access, &first_source.descriptor.id).await;
        let destination_dir = tempfile::tempdir().unwrap();
        let destination =
            Arc::new(ArtifactStore::new(destination_dir.path().join("store")).unwrap());
        let coordinator = ManagedArtifactCoordinator::new(access.clone(), Arc::clone(&destination));
        let installed = coordinator
            .install(ManagedArtifactInstallRequest {
                mirror_id: "mirror-auto".into(),
                operation_id: "operation-auto-install".into(),
                owner_principal_id: "bootstrap-owner".into(),
                destination_id: None,
                source_provider_authority: "depot".into(),
                source_assignment_id: "assignment-test".into(),
                source_scope: OwnerScope::Personal(
                    labby_primitives::access::PrincipalId::new("bootstrap-owner").unwrap(),
                ),
                mode: ManagedArtifactMirrorMode::Followed,
                policy_epoch,
                update_policy: Some(ArtifactSubscriptionUpdatePolicy::Notify),
                now: 20,
                acquisition: first_acquisition,
            })
            .await
            .unwrap();
        let initial_revision = installed.mirror.local_revision_id.clone().unwrap();
        std::fs::write(source_package.path().join("a.txt"), b"beta").unwrap();
        let second_source = source
            .import_local(
                ArtifactImportRequest::new("resource", "upstream", "auto-follow-demo"),
                source_package.path(),
            )
            .unwrap();
        let second_acquisition = source_acquisition(&source, &second_source).await;
        let request = ManagedArtifactFollowUpdateRequest {
            mirror_id: "mirror-auto".into(),
            operation_id: "operation-auto-update".into(),
            expected_local_revision_id: initial_revision.clone(),
            policy_epoch,
            now: 30,
            acquisition: second_acquisition.clone(),
        };
        assert!(matches!(
            coordinator
                .apply_auto_approved_follow_update(request.clone())
                .await,
            Err(ManagedArtifactDistributionError::State(
                "follow_subscription_not_auto_approved"
            ))
        ));
        assert_eq!(
            destination
                .get(&first_source.descriptor.id)
                .unwrap()
                .current_revision_id,
            initial_revision
        );
        access
            .put_artifact_subscription(ManagedArtifactSubscription {
                mirror_id: "mirror-auto".into(),
                update_policy: ArtifactSubscriptionUpdatePolicy::AutoApproved,
                last_observed_revision_id: Some(initial_revision.clone()),
                last_applied_revision_id: Some(initial_revision.clone()),
                last_checked_at: Some(31),
                active: true,
                updated_at: 31,
            })
            .await
            .unwrap();
        let applied = coordinator
            .apply_auto_approved_follow_update(request)
            .await
            .unwrap();
        assert_eq!(
            applied.mirror.local_revision_id.as_deref(),
            Some(second_source.current_revision_id.as_str())
        );
        assert_eq!(
            destination
                .get(&first_source.descriptor.id)
                .unwrap()
                .current_revision_id,
            second_source.current_revision_id
        );
    }

    #[tokio::test]
    async fn pinned_follow_policy_rejects_updates_without_moving_local_head() {
        let (_access_dir, access) = bootstrapped_access().await;
        let source_dir = tempfile::tempdir().unwrap();
        let source_package = tempfile::tempdir().unwrap();
        std::fs::write(source_package.path().join("a.txt"), b"alpha").unwrap();
        let source = ArtifactStore::new(source_dir.path().join("store")).unwrap();
        let first_source = source
            .import_local(
                ArtifactImportRequest::new("resource", "upstream", "pinned-follow-demo"),
                source_package.path(),
            )
            .unwrap();
        let first_acquisition = source_acquisition(&source, &first_source).await;
        let policy_epoch = install_authority(&access, &first_source.descriptor.id).await;

        let destination_dir = tempfile::tempdir().unwrap();
        let destination =
            Arc::new(ArtifactStore::new(destination_dir.path().join("store")).unwrap());
        let coordinator = ManagedArtifactCoordinator::new(access, Arc::clone(&destination));
        let installed = coordinator
            .install(ManagedArtifactInstallRequest {
                mirror_id: "mirror-pinned-follow".into(),
                operation_id: "operation-initial".into(),
                owner_principal_id: "bootstrap-owner".into(),
                destination_id: None,
                source_provider_authority: "depot".into(),
                source_assignment_id: "assignment-test".into(),
                source_scope: OwnerScope::Personal(
                    labby_primitives::access::PrincipalId::new("bootstrap-owner").unwrap(),
                ),
                mode: ManagedArtifactMirrorMode::Followed,
                policy_epoch,
                update_policy: Some(ArtifactSubscriptionUpdatePolicy::Pinned),
                now: 20,
                acquisition: first_acquisition,
            })
            .await
            .unwrap();
        let initial_revision = installed.mirror.local_revision_id.clone().unwrap();

        std::fs::write(source_package.path().join("a.txt"), b"beta").unwrap();
        let second_source = source
            .import_local(
                ArtifactImportRequest::new("resource", "upstream", "pinned-follow-demo"),
                source_package.path(),
            )
            .unwrap();
        let second_acquisition = source_acquisition(&source, &second_source).await;

        let result = coordinator
            .apply_follow_update(ManagedArtifactFollowUpdateRequest {
                mirror_id: "mirror-pinned-follow".into(),
                operation_id: "operation-update".into(),
                expected_local_revision_id: initial_revision.clone(),
                policy_epoch,
                now: 30,
                acquisition: second_acquisition,
            })
            .await;
        assert!(matches!(
            result,
            Err(ManagedArtifactDistributionError::State(
                "follow_subscription_pinned"
            ))
        ));
        assert_eq!(
            destination
                .get(&first_source.descriptor.id)
                .unwrap()
                .current_revision_id,
            initial_revision
        );
    }

    #[tokio::test]
    async fn followed_update_recovers_when_artifact_commit_precedes_access_finalize() {
        let (_access_dir, access) = bootstrapped_access().await;
        let source_dir = tempfile::tempdir().unwrap();
        let source_package = tempfile::tempdir().unwrap();
        std::fs::write(source_package.path().join("a.txt"), b"alpha").unwrap();
        let source = ArtifactStore::new(source_dir.path().join("store")).unwrap();
        let first_source = source
            .import_local(
                ArtifactImportRequest::new("resource", "upstream", "follow-demo"),
                source_package.path(),
            )
            .unwrap();
        let first_acquisition = source_acquisition(&source, &first_source).await;
        let policy_epoch = install_authority(&access, &first_source.descriptor.id).await;

        let destination_dir = tempfile::tempdir().unwrap();
        let destination =
            Arc::new(ArtifactStore::new(destination_dir.path().join("store")).unwrap());
        let coordinator = ManagedArtifactCoordinator::new(access.clone(), Arc::clone(&destination));
        let initial = coordinator
            .install(ManagedArtifactInstallRequest {
                mirror_id: "mirror-follow".into(),
                operation_id: "operation-initial".into(),
                owner_principal_id: "bootstrap-owner".into(),
                destination_id: None,
                source_provider_authority: "depot".into(),
                source_assignment_id: "assignment-test".into(),
                source_scope: OwnerScope::Personal(
                    labby_primitives::access::PrincipalId::new("bootstrap-owner").unwrap(),
                ),
                mode: ManagedArtifactMirrorMode::Followed,
                policy_epoch,
                update_policy: Some(ArtifactSubscriptionUpdatePolicy::AutoApproved),
                now: 20,
                acquisition: first_acquisition,
            })
            .await
            .unwrap();
        let initial_revision = initial.mirror.local_revision_id.clone().unwrap();

        std::fs::write(source_package.path().join("a.txt"), b"beta").unwrap();
        let second_source = source
            .import_local(
                ArtifactImportRequest::new("resource", "upstream", "follow-demo"),
                source_package.path(),
            )
            .unwrap();
        let second_acquisition = source_acquisition(&source, &second_source).await;

        access
            .begin_managed_artifact_mirror_update(BeginManagedArtifactMirrorUpdate {
                mirror_id: "mirror-follow".into(),
                operation_id: "operation-update".into(),
                source_revision_id: second_source.current_revision_id.clone(),
                expected_local_revision_id: initial_revision.clone(),
                policy_epoch,
                now: 30,
            })
            .await
            .unwrap();
        destination
            .apply_acquisition_update(&initial_revision, second_acquisition.clone())
            .unwrap();

        let recovered = coordinator
            .apply_follow_update(ManagedArtifactFollowUpdateRequest {
                mirror_id: "mirror-follow".into(),
                operation_id: "operation-update".into(),
                expected_local_revision_id: initial_revision,
                policy_epoch,
                now: 31,
                acquisition: second_acquisition,
            })
            .await
            .unwrap();
        assert_eq!(recovered.mirror.status, ManagedArtifactMirrorStatus::Active);
        assert_eq!(
            recovered.mirror.local_revision_id.as_deref(),
            Some(second_source.current_revision_id.as_str())
        );
        let subscription = recovered.subscription.unwrap();
        assert_eq!(
            subscription.last_applied_revision_id.as_deref(),
            Some(second_source.current_revision_id.as_str())
        );
        assert_eq!(
            destination
                .get(&second_source.descriptor.id)
                .unwrap()
                .current_revision_id,
            second_source.current_revision_id
        );
    }
}
