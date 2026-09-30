//! Durable auto-approved follow reconciliation for managed Artifacts.
//!
//! One process-owned loop observes bounded due subscriptions. Durable identity references are
//! restored only from AccessStore state and every external read/apply is fenced by fresh local
//! authorization. No transport credentials, endpoints, or remote bytes are persisted here.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use labby_apis::artifact_control::Operation;
use labby_runtime::artifacts::{ArtifactError, canonical_json};
use serde_json::{Value, json};
use thiserror::Error;

use crate::access::{
    AccessRuntime, AccessRuntimeError, AccessStoreError, ArtifactDestinationPolicy,
    ArtifactTransferDenyReason, ArtifactTransferMode, ManagedArtifactMirror,
    ManagedArtifactMirrorStatus, ManagedArtifactSubscription,
};
use crate::dispatch::artifact_distribution::{
    ManagedArtifactCoordinator, ManagedArtifactDistributionError,
    ManagedArtifactFollowUpdateRequest,
};
use crate::dispatch::error::ToolError;

use super::ProcessSkillLibraryRuntime;
use super::audit::{
    CanonicalArtifactId, SkillLibraryAuditEvent, SkillLibraryCorrelationId,
    SkillLibraryTerminalAudit, SkillLibraryTerminalOutcome, SkillLibraryTerminalStage,
    record_terminal_mutation,
};
use super::auth::{
    SkillLibraryAction, SkillLibraryAuthorizationError, authorize_durable_distribution_at_boundary,
};
use super::import::ImportAdapterError;
use super::params::SourceSelector;

const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);
const RECONCILE_BATCH_LIMIT: usize = 16;

#[derive(Debug, Error)]
enum FollowReconcileError {
    #[error(transparent)]
    Runtime(#[from] AccessRuntimeError),
    #[error(transparent)]
    Access(#[from] AccessStoreError),
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error(transparent)]
    Import(#[from] ImportAdapterError),
    #[error(transparent)]
    Distribution(#[from] ManagedArtifactDistributionError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error("durable Artifact follow authorization is unavailable")]
    AuthorizationUnavailable,
    #[error("managed Artifact follow state is invalid: {0}")]
    State(&'static str),
}

/// Start exactly one reconciler for the process-owned Skill Library runtime.
pub(crate) fn start(
    access_runtime: &Arc<AccessRuntime>,
    runtime: &Arc<ProcessSkillLibraryRuntime>,
) {
    let access_runtime = Arc::downgrade(access_runtime);
    let runtime = Arc::downgrade(runtime);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(RECONCILE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let Some(access_runtime) = access_runtime.upgrade() else {
                break;
            };
            let Some(runtime) = runtime.upgrade() else {
                break;
            };
            if let Err(error) = tick(&access_runtime, &runtime).await {
                tracing::warn!(
                    kind = reconcile_error_kind(&error),
                    "managed Artifact follow reconciliation tick failed"
                );
            }
        }
    });
}

/// Before explicit owner setup there can be no authorized follow work. This
/// is a waiting state, not a failed reconciliation or permission decision.
fn reconciliation_store(
    result: Result<crate::access::AccessStore, AccessRuntimeError>,
) -> Result<Option<crate::access::AccessStore>, FollowReconcileError> {
    match result {
        Ok(store) => Ok(Some(store)),
        Err(AccessRuntimeError::SetupRequired(_)) => {
            tracing::debug!(
                kind = "access_setup_required",
                "managed Artifact follow reconciliation waits for owner setup"
            );
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

async fn tick(
    access_runtime: &AccessRuntime,
    runtime: &ProcessSkillLibraryRuntime,
) -> Result<(), FollowReconcileError> {
    let now = unix_seconds()?;
    let checked_before = now.saturating_sub(
        i64::try_from(RECONCILE_INTERVAL.as_secs())
            .map_err(|_| FollowReconcileError::State("follow_interval_out_of_range"))?,
    );
    let Some(store) = reconciliation_store(access_runtime.store().await)? else {
        return Ok(());
    };

    for mirror in store
        .managed_artifact_mirrors_for_reconciliation(checked_before, 64)
        .await?
    {
        let mirror_id = mirror.mirror_id.clone();
        if let Err(error) = reconcile_managed_authority(access_runtime, runtime, mirror, now).await
        {
            tracing::warn!(
                mirror_id,
                kind = reconcile_error_kind(&error),
                "managed Artifact authority reconciliation failed"
            );
        }
    }

    // Restriction commits before the purge; retry any purge an earlier tick could not finish.
    for mirror in store.managed_artifact_mirrors_pending_purge(64).await? {
        let mirror_id = mirror.mirror_id.clone();
        let restriction = mirror.status;
        if let Err(error) = restrict(&store, runtime, &mirror, restriction, now).await {
            tracing::warn!(
                mirror_id,
                kind = reconcile_error_kind(&error),
                "managed Artifact purge retry failed"
            );
        }
    }

    let subscriptions = store
        .auto_approved_artifact_subscriptions_due(checked_before, RECONCILE_BATCH_LIMIT)
        .await?;
    for subscription in subscriptions {
        let mirror_id = subscription.mirror_id.clone();
        if let Err(error) = reconcile_subscription(access_runtime, runtime, subscription, now).await
        {
            tracing::warn!(
                mirror_id,
                kind = reconcile_error_kind(&error),
                "managed Artifact auto-follow reconciliation failed"
            );
        }
    }
    Ok(())
}

async fn reconcile_managed_authority(
    access_runtime: &AccessRuntime,
    runtime: &ProcessSkillLibraryRuntime,
    mirror: ManagedArtifactMirror,
    now: i64,
) -> Result<(), FollowReconcileError> {
    let store = access_runtime.store().await?;
    let mirror = store
        .mark_managed_artifact_mirror_checked(mirror.mirror_id, now)
        .await?;
    let target = CanonicalArtifactId::parse(mirror.source_artifact_id.clone())?;
    let correlation = SkillLibraryCorrelationId::server("artifact-managed-reconcile");
    let (action, mode) = match mirror.mode {
        crate::access::ManagedArtifactMirrorMode::Pinned => {
            (SkillLibraryAction::Pin, ArtifactTransferMode::Pin)
        }
        crate::access::ManagedArtifactMirrorMode::Followed => (
            SkillLibraryAction::FollowUpdate,
            ArtifactTransferMode::Follow,
        ),
    };
    let (identity, decision) = match authorize_durable_distribution_at_boundary(
        access_runtime,
        &mirror.identity_ref_json,
        &mirror.authorization_project_id,
        mirror.authorization_team_id.clone(),
        action,
        &target,
        &correlation,
    )
    .await
    {
        Ok(decision) => decision,
        Err(SkillLibraryAuthorizationError::Denied) => {
            restrict(
                &store,
                runtime,
                &mirror,
                ManagedArtifactMirrorStatus::AccessRevoked,
                now,
            )
            .await?;
            return Ok(());
        }
        Err(SkillLibraryAuthorizationError::Unavailable) => {
            return Err(FollowReconcileError::AuthorizationUnavailable);
        }
    };
    if decision.authority.principal_id != mirror.owner_principal_id {
        return Err(FollowReconcileError::State("managed_mirror_owner_mismatch"));
    }

    let policy = store
        .managed_artifact_policy_decision(
            mirror.source_provider_authority.clone(),
            mirror.source_artifact_id.clone(),
            mirror.source_assignment_id.clone(),
            decision.authority.grants,
            mode,
        )
        .await?;
    if !policy.allowed {
        record_terminal(&decision.audit, &mirror.source_revision_id, false);
        restrict(
            &store,
            runtime,
            &mirror,
            restriction_for_denial(policy.denied_by),
            now,
        )
        .await?;
        return Ok(());
    }

    let Some(connection_id) = mirror
        .source_provider_authority
        .strip_prefix("depot:")
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };

    let control_context = match crate::dispatch::artifact_control::authorize_authority_context(
        access_runtime,
        identity,
        &mirror.authorization_project_id,
        mirror.authorization_team_id.as_deref(),
        crate::access::Permission::AssetUse,
    )
    .await
    {
        Ok(context) => context,
        Err(error) if error.kind() == "forbidden" => {
            record_terminal(&decision.audit, &mirror.source_revision_id, false);
            restrict(
                &store,
                runtime,
                &mirror,
                ManagedArtifactMirrorStatus::AccessRevoked,
                now,
            )
            .await?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };

    let head = match runtime
        .controls
        .execute(
            Some(connection_id),
            Operation::ArtifactsGet,
            &json!({ "artifactId": mirror.source_artifact_id.clone() }),
            Some(&control_context),
        )
        .await
    {
        Ok(head) => head,
        Err(error) if error.kind() == "not_found" => {
            record_terminal(&decision.audit, &mirror.source_revision_id, false);
            restrict(
                &store,
                runtime,
                &mirror,
                ManagedArtifactMirrorStatus::SourceWithdrawn,
                now,
            )
            .await?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    validate_head_identity(&head, &mirror.source_artifact_id)?;
    if !head_allows_managed_bytes(&head) {
        record_terminal(&decision.audit, &mirror.source_revision_id, false);
        restrict(
            &store,
            runtime,
            &mirror,
            ManagedArtifactMirrorStatus::SourceWithdrawn,
            now,
        )
        .await?;
    }
    Ok(())
}

async fn reconcile_subscription(
    access_runtime: &AccessRuntime,
    runtime: &ProcessSkillLibraryRuntime,
    subscription: ManagedArtifactSubscription,
    now: i64,
) -> Result<(), FollowReconcileError> {
    let store = access_runtime.store().await?;
    let mirror = store
        .managed_artifact_mirror(subscription.mirror_id.clone())
        .await?
        .ok_or(FollowReconcileError::State("follow_mirror_missing"))?;
    if mirror.mode != crate::access::ManagedArtifactMirrorMode::Followed
        || !matches!(
            mirror.status,
            ManagedArtifactMirrorStatus::Active | ManagedArtifactMirrorStatus::Committing
        )
    {
        return Err(FollowReconcileError::State(
            "follow_mirror_not_reconcilable",
        ));
    }

    let target = CanonicalArtifactId::parse(mirror.source_artifact_id.clone())?;
    let correlation = SkillLibraryCorrelationId::server("artifact-follow-auto");
    let (identity, preflight) = match authorize_durable_distribution_at_boundary(
        access_runtime,
        &mirror.identity_ref_json,
        &mirror.authorization_project_id,
        mirror.authorization_team_id.clone(),
        SkillLibraryAction::FollowUpdate,
        &target,
        &correlation,
    )
    .await
    {
        Ok(decision) => decision,
        Err(SkillLibraryAuthorizationError::Denied) => {
            restrict(
                &store,
                runtime,
                &mirror,
                ManagedArtifactMirrorStatus::AccessRevoked,
                now,
            )
            .await?;
            return Ok(());
        }
        Err(SkillLibraryAuthorizationError::Unavailable) => {
            return Err(FollowReconcileError::AuthorizationUnavailable);
        }
    };
    if preflight.authority.principal_id != mirror.owner_principal_id {
        return Err(FollowReconcileError::State("managed_mirror_owner_mismatch"));
    }

    let connection_id = mirror
        .source_provider_authority
        .strip_prefix("depot:")
        .filter(|value| !value.is_empty())
        .ok_or(FollowReconcileError::State(
            "follow_source_has_no_trusted_head_resolver",
        ))?;

    let control_context = match crate::dispatch::artifact_control::authorize_authority_context(
        access_runtime,
        identity.clone(),
        &mirror.authorization_project_id,
        mirror.authorization_team_id.as_deref(),
        crate::access::Permission::AssetUse,
    )
    .await
    {
        Ok(context) => context,
        Err(error) if error.kind() == "forbidden" => {
            restrict(
                &store,
                runtime,
                &mirror,
                ManagedArtifactMirrorStatus::AccessRevoked,
                now,
            )
            .await?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };

    let head = match runtime
        .controls
        .execute(
            Some(connection_id),
            Operation::ArtifactsGet,
            &json!({ "artifactId": mirror.source_artifact_id }),
            Some(&control_context),
        )
        .await
    {
        Ok(head) => head,
        Err(error) if error.kind() == "not_found" => {
            record_terminal(&preflight.audit, &mirror.source_revision_id, false);
            restrict(
                &store,
                runtime,
                &mirror,
                ManagedArtifactMirrorStatus::SourceWithdrawn,
                now,
            )
            .await?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    validate_head_identity(&head, &mirror.source_artifact_id)?;
    if !head_allows_managed_bytes(&head) {
        record_terminal(&preflight.audit, &mirror.source_revision_id, false);
        restrict(
            &store,
            runtime,
            &mirror,
            ManagedArtifactMirrorStatus::SourceWithdrawn,
            now,
        )
        .await?;
        return Ok(());
    }
    let head_revision = remote_head_revision(&head)?;

    let (target_revision, operation_id) =
        if mirror.status == ManagedArtifactMirrorStatus::Committing {
            (
                mirror.source_revision_id.clone(),
                mirror.operation_id.clone(),
            )
        } else {
            let local_revision =
                mirror
                    .local_revision_id
                    .as_deref()
                    .ok_or(FollowReconcileError::State(
                        "active_follow_has_no_local_revision",
                    ))?;
            if local_revision == head_revision {
                store
                    .observe_artifact_subscription(subscription.mirror_id, Some(head_revision), now)
                    .await?;
                return Ok(());
            }
            (
                head_revision.clone(),
                auto_follow_operation_id(&mirror.mirror_id, &head_revision)?,
            )
        };

    let source = SourceSelector::Depot {
        connection_id: connection_id.to_owned(),
        artifact_id: mirror.source_artifact_id.clone(),
        revision_id: target_revision.clone(),
    };
    let acquisition = runtime
        .imports
        .acquire_selected_for_durable_distribution(
            access_runtime,
            identity,
            &mirror.authorization_project_id,
            mirror.authorization_team_id.as_deref(),
            source,
        )
        .await?;

    let (_identity, postflight) = match authorize_durable_distribution_at_boundary(
        access_runtime,
        &mirror.identity_ref_json,
        &mirror.authorization_project_id,
        mirror.authorization_team_id.clone(),
        SkillLibraryAction::FollowUpdate,
        &target,
        &correlation,
    )
    .await
    {
        Ok(decision) => decision,
        Err(SkillLibraryAuthorizationError::Denied) => {
            record_terminal(&preflight.audit, &target_revision, false);
            restrict(
                &store,
                runtime,
                &mirror,
                ManagedArtifactMirrorStatus::AccessRevoked,
                now,
            )
            .await?;
            return Ok(());
        }
        Err(SkillLibraryAuthorizationError::Unavailable) => {
            return Err(FollowReconcileError::AuthorizationUnavailable);
        }
    };
    if postflight.authority.principal_id != mirror.owner_principal_id {
        return Err(FollowReconcileError::State("managed_mirror_owner_mismatch"));
    }

    let current_store = access_runtime.store().await?;
    let options = current_store
        .artifact_transfer_options(
            mirror.source_provider_authority.clone(),
            mirror.source_artifact_id.clone(),
            Some(mirror.source_assignment_id.clone()),
            postflight.authority.grants,
            acquisition.interchange.publication.clone(),
            acquisition.interchange.license.clone(),
            Some(ArtifactDestinationPolicy::local_personal()),
        )
        .await?;
    let follow = options.decision(ArtifactTransferMode::Follow);
    if !follow.allowed {
        record_terminal(&postflight.audit, &target_revision, false);
        restrict(
            &current_store,
            runtime,
            &mirror,
            restriction_for_denial(follow.denied_by),
            now,
        )
        .await?;
        return Ok(());
    }

    let expected_local_revision_id = mirror
        .local_revision_id
        .clone()
        .ok_or(FollowReconcileError::State("follow_has_no_local_revision"))?;
    let coordinator =
        ManagedArtifactCoordinator::new(current_store, Arc::clone(&runtime.service.store));
    let result = coordinator
        .apply_auto_approved_follow_update(ManagedArtifactFollowUpdateRequest {
            mirror_id: mirror.mirror_id,
            operation_id,
            expected_local_revision_id,
            policy_epoch: postflight.authority.global_revision,
            now,
            acquisition,
        })
        .await;
    record_terminal(&postflight.audit, &target_revision, result.is_ok());
    result?;
    Ok(())
}

async fn restrict(
    store: &crate::access::AccessStore,
    runtime: &ProcessSkillLibraryRuntime,
    mirror: &ManagedArtifactMirror,
    restriction: ManagedArtifactMirrorStatus,
    now: i64,
) -> Result<(), FollowReconcileError> {
    let coordinator =
        ManagedArtifactCoordinator::new(store.clone(), Arc::clone(&runtime.service.store));
    coordinator
        .restrict_and_purge_managed_mirror(mirror.mirror_id.clone(), restriction, now)
        .await?;
    Ok(())
}

fn restriction_for_denial(
    reason: Option<ArtifactTransferDenyReason>,
) -> ManagedArtifactMirrorStatus {
    match reason {
        Some(
            ArtifactTransferDenyReason::PublisherPolicy
            | ArtifactTransferDenyReason::PublicationState
            | ArtifactTransferDenyReason::LicensePolicy
            | ArtifactTransferDenyReason::Takedown,
        ) => ManagedArtifactMirrorStatus::SourceWithdrawn,
        Some(
            ArtifactTransferDenyReason::CallerPermission
            | ArtifactTransferDenyReason::AssignmentPolicy
            | ArtifactTransferDenyReason::DestinationPolicy,
        )
        | None => ManagedArtifactMirrorStatus::AccessRevoked,
    }
}

fn validate_head_identity(
    head: &Value,
    expected_artifact_id: &str,
) -> Result<(), FollowReconcileError> {
    if head.get("id").and_then(Value::as_str) != Some(expected_artifact_id) {
        return Err(FollowReconcileError::State("remote_head_identity_mismatch"));
    }
    Ok(())
}

fn remote_head_revision(head: &Value) -> Result<String, FollowReconcileError> {
    head.get("currentRevisionId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(FollowReconcileError::State(
            "remote_head_revision_unavailable",
        ))
}

fn head_allows_managed_bytes(head: &Value) -> bool {
    let publication = head.get("publication");
    let license = head.get("license");
    publication
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
        == Some("published")
        && publication
            .and_then(|value| value.get("distribution"))
            .and_then(Value::as_str)
            == Some("bytes")
        && matches!(
            license
                .and_then(|value| value.get("redistribution"))
                .and_then(Value::as_str),
            Some("redistributable" | "forkable")
        )
        && !matches!(
            license
                .and_then(|value| value.get("takedownState"))
                .and_then(Value::as_str),
            Some("restricted" | "removed")
        )
}

fn auto_follow_operation_id(
    mirror_id: &str,
    revision_id: &str,
) -> Result<String, FollowReconcileError> {
    let digest = canonical_json::digest(&json!({
        "schema": "labby.managed-artifact-auto-follow/v1",
        "mirrorId": mirror_id,
        "sourceRevisionId": revision_id,
    }))?;
    let suffix = digest
        .strip_prefix("sha256:")
        .ok_or(FollowReconcileError::State("auto_follow_digest_invalid"))?;
    Ok(format!("operation_auto_follow_{suffix}"))
}

fn record_terminal(audit: &SkillLibraryAuditEvent, revision_id: &str, committed: bool) {
    let terminal = SkillLibraryTerminalAudit::new(
        if committed {
            SkillLibraryTerminalOutcome::Committed
        } else {
            SkillLibraryTerminalOutcome::Failed
        },
        SkillLibraryTerminalStage::Commit,
    )
    .with_revision_id(revision_id);
    if !record_terminal_mutation(audit, terminal) {
        tracing::warn!(
            service = "skill_library",
            action = ?audit.action,
            "terminal auto-follow Artifact audit event was not retained"
        );
    }
}

fn unix_seconds() -> Result<i64, FollowReconcileError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| FollowReconcileError::State("system_clock_unavailable"))?
        .as_secs();
    i64::try_from(seconds).map_err(|_| FollowReconcileError::State("system_clock_out_of_range"))
}

fn reconcile_error_kind(error: &FollowReconcileError) -> &'static str {
    match error {
        FollowReconcileError::Runtime(_) => "access_runtime",
        FollowReconcileError::Access(_) => "access_store",
        FollowReconcileError::Artifact(_) => "artifact",
        FollowReconcileError::Import(_) => "source",
        FollowReconcileError::Distribution(_) => "distribution",
        FollowReconcileError::Tool(error) => match error.kind() {
            "not_found" => "not_found",
            "forbidden" => "forbidden",
            "service_unavailable" => "service_unavailable",
            "rate_limited" => "rate_limited",
            _ => "tool",
        },
        FollowReconcileError::AuthorizationUnavailable => "authorization_unavailable",
        FollowReconcileError::State(_) => "state",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coco_follow_reconciliation_waits_only_for_setup() {
        use crate::access::{AccessBlockedReason, AccessSetupReason};
        for reason in [
            AccessSetupReason::Missing,
            AccessSetupReason::Uninitialized,
            AccessSetupReason::ProofPending,
        ] {
            assert!(
                reconciliation_store(Err(AccessRuntimeError::SetupRequired(reason)))
                    .unwrap()
                    .is_none()
            );
        }
        for reason in [
            AccessBlockedReason::Insecure,
            AccessBlockedReason::Corrupt,
            AccessBlockedReason::NewerSchema,
            AccessBlockedReason::Locked,
            AccessBlockedReason::ReadOnly,
            AccessBlockedReason::Unavailable,
        ] {
            assert!(matches!(
                reconciliation_store(Err(AccessRuntimeError::Blocked(reason))),
                Err(FollowReconcileError::Runtime(AccessRuntimeError::Blocked(
                    _
                )))
            ));
        }
    }

    #[test]
    fn denial_restriction_distinguishes_source_withdrawal_from_access_revocation() {
        for reason in [
            ArtifactTransferDenyReason::PublisherPolicy,
            ArtifactTransferDenyReason::PublicationState,
            ArtifactTransferDenyReason::LicensePolicy,
            ArtifactTransferDenyReason::Takedown,
        ] {
            assert_eq!(
                restriction_for_denial(Some(reason)),
                ManagedArtifactMirrorStatus::SourceWithdrawn
            );
        }
        for reason in [
            ArtifactTransferDenyReason::CallerPermission,
            ArtifactTransferDenyReason::AssignmentPolicy,
            ArtifactTransferDenyReason::DestinationPolicy,
        ] {
            assert_eq!(
                restriction_for_denial(Some(reason)),
                ManagedArtifactMirrorStatus::AccessRevoked
            );
        }
    }

    #[test]
    fn remote_head_contract_requires_exact_identity_and_followable_bytes() {
        let artifact = "resource:upstream:demo";
        let head = json!({
            "id": artifact,
            "currentRevisionId": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "publication": {"state":"published","distribution":"bytes"},
            "license": {
                "redistribution":"redistributable",
                "takedownState":"none"
            }
        });
        validate_head_identity(&head, artifact).unwrap();
        assert!(head_allows_managed_bytes(&head));
        assert_eq!(
            remote_head_revision(&head).unwrap(),
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );

        let withdrawn = json!({
            "id": artifact,
            "currentRevisionId": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "publication": {"state":"withdrawn","distribution":"bytes"},
            "license": {
                "redistribution":"redistributable",
                "takedownState":"none"
            }
        });
        assert!(!head_allows_managed_bytes(&withdrawn));
    }

    #[test]
    fn auto_follow_operation_is_revision_stable_and_revision_specific() {
        let first = auto_follow_operation_id(
            "mirror-a",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        assert_eq!(
            first,
            auto_follow_operation_id(
                "mirror-a",
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap()
        );
        assert_ne!(
            first,
            auto_follow_operation_id(
                "mirror-a",
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )
            .unwrap()
        );
    }
}
