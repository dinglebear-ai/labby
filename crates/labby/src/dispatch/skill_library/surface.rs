//! Surface-neutral Skill Library action routing after transport authentication.
//!
//! HTTP and MCP adapters project transport-specific identity/correlation facts
//! into this boundary, then share one implementation for remote controls,
//! imports, idempotency validation, and library dispatch/error mapping.

use std::future::Future;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use labby_runtime::artifacts::{ArtifactDescriptor, canonical_json};
use serde_json::{Value, json};

use crate::access::{
    AccessRuntime, AccessStoreError, ArtifactDestinationPolicy, ArtifactSubscriptionUpdatePolicy,
    ArtifactTransferMode, ManagedArtifactMirrorMode,
};
use crate::dispatch::error::ToolError;

use super::ProcessSkillLibrary;
use super::audit::{
    CanonicalArtifactId, SkillLibraryAuditEvent, SkillLibraryCorrelationId,
    SkillLibraryTerminalAudit, SkillLibraryTerminalOutcome, SkillLibraryTerminalStage,
    record_terminal_mutation,
};
use super::auth::{
    ArtifactDistributionAuthorizationDecision, SkillLibraryAction, SkillLibraryCaller,
    authorize_distribution_at_boundary,
};
use super::import::ImportCoordinator;
use super::params::{
    ArtifactFollowParams, ArtifactFollowPolicyParam, ArtifactFollowUpdateParams,
    ArtifactForkPersonalParams, ArtifactPinParams, ArtifactTransferOptionsParams,
    ImportBatchParams, ImportParams, SourceSelector, validate_idempotency_key,
};
use crate::dispatch::artifact_distribution::{
    ManagedArtifactAuthorization, ManagedArtifactCoordinator, ManagedArtifactFollowUpdateRequest,
    ManagedArtifactInstallOutcome, ManagedArtifactInstallRequest, PersonalArtifactForkOutcome,
    PersonalArtifactForkRequest,
};

pub(crate) async fn dispatch_authorized_action(
    service: &Arc<ProcessSkillLibrary>,
    imports: Option<Arc<ImportCoordinator>>,
    access_runtime: &AccessRuntime,
    caller: SkillLibraryCaller,
    project_id: &str,
    action: &str,
    params: Value,
    correlation: &SkillLibraryCorrelationId,
) -> Result<Value, ToolError> {
    if crate::dispatch::remote_control::REMOTE_ARTIFACT_ACTIONS
        .iter()
        .any(|candidate| candidate.name == action)
    {
        let operation = crate::dispatch::remote_control::operation("artifacts", action)
            .ok_or_else(|| ToolError::UnknownAction {
                message: format!("Unknown action: {action}"),
                valid: Vec::new(),
                hint: None,
            })?;
        let permission = crate::dispatch::artifact_control::operation_permission(operation);
        let authority = crate::dispatch::artifact_control::authorize_authority_context(
            access_runtime,
            caller.identity().clone(),
            project_id,
            caller.selected_team_id(),
            permission,
        )
        .await?;
        return crate::dispatch::remote_control::dispatch_with_context(
            "artifacts",
            action,
            params,
            Some(&authority),
        )
        .await;
    }

    if matches!(
        action,
        "artifacts.transfer_options"
            | "artifacts.pin"
            | "artifacts.follow_managed"
            | "artifacts.follow_update"
            | "artifacts.fork_personal"
    ) {
        return dispatch_distribution_action(
            service,
            imports.as_ref(),
            access_runtime,
            &caller,
            project_id,
            action,
            params,
            correlation,
        )
        .await;
    }

    if action == "artifacts.import" {
        let import_params: ImportParams =
            parse_params(params, "Skill Library import parameters are invalid")?;
        validate_idempotency_key(&import_params.idempotency_key).map_err(|_| {
            ToolError::InvalidParam {
                message: "Skill Library idempotency key is invalid".to_owned(),
                param: "idempotency_key".to_owned(),
            }
        })?;
        let imports = imports.ok_or_else(|| ToolError::Sdk {
            sdk_kind: "source_unavailable".to_owned(),
            message: "Skill import sources are not configured".to_owned(),
        })?;
        return imports
            .import_selected(
                service,
                access_runtime,
                caller,
                project_id,
                import_params.source,
                import_params.expected_library_version,
                import_params.idempotency_key,
                correlation,
            )
            .await
            .map_err(super::map_import_error);
    }

    if action == "artifacts.import_batch" {
        let import_params: ImportBatchParams =
            parse_params(params, "Artifact batch import parameters are invalid")?;
        validate_idempotency_key(&import_params.idempotency_key).map_err(|_| {
            ToolError::InvalidParam {
                message: "Artifact batch idempotency key is invalid".to_owned(),
                param: "idempotency_key".to_owned(),
            }
        })?;
        let imports = imports.ok_or_else(|| ToolError::Sdk {
            sdk_kind: "source_unavailable".to_owned(),
            message: "Artifact import sources are not configured".to_owned(),
        })?;
        return imports
            .import_batch_selected(
                service,
                access_runtime,
                caller,
                project_id,
                import_params.sources,
                import_params.expected_library_version,
                import_params.idempotency_key,
                correlation,
            )
            .await
            .map_err(super::map_import_error);
    }

    service
        .dispatch(
            access_runtime,
            caller,
            project_id,
            action,
            params,
            correlation,
        )
        .await
        .map_err(super::map_dispatch_error)
}

async fn dispatch_distribution_action(
    service: &Arc<ProcessSkillLibrary>,
    imports: Option<&Arc<ImportCoordinator>>,
    access_runtime: &AccessRuntime,
    caller: &SkillLibraryCaller,
    project_id: &str,
    action: &str,
    params: Value,
    correlation: &SkillLibraryCorrelationId,
) -> Result<Value, ToolError> {
    let imports = imports.ok_or_else(|| ToolError::Sdk {
        sdk_kind: "source_unavailable".to_owned(),
        message: "Artifact sources are not configured".to_owned(),
    })?;
    match action {
        "artifacts.transfer_options" => {
            let request: ArtifactTransferOptionsParams =
                parse_params(params, "Artifact transfer options parameters are invalid")?;
            transfer_options(
                imports,
                access_runtime,
                caller,
                project_id,
                request,
                correlation,
            )
            .await
        }
        "artifacts.pin" => {
            let request: ArtifactPinParams =
                parse_params(params, "Artifact pin parameters are invalid")?;
            validate_distribution_idempotency(&request.idempotency_key)?;
            install_managed(
                service,
                imports,
                access_runtime,
                caller,
                project_id,
                request.source,
                request.assignment_id,
                request.idempotency_key,
                ManagedArtifactMirrorMode::Pinned,
                None,
                SkillLibraryAction::Pin,
                correlation,
            )
            .await
        }
        "artifacts.follow_managed" => {
            let request: ArtifactFollowParams =
                parse_params(params, "Artifact follow parameters are invalid")?;
            validate_distribution_idempotency(&request.idempotency_key)?;
            let update_policy = match request.update_policy {
                ArtifactFollowPolicyParam::Notify => ArtifactSubscriptionUpdatePolicy::Notify,
                ArtifactFollowPolicyParam::AutoApproved => {
                    ArtifactSubscriptionUpdatePolicy::AutoApproved
                }
                ArtifactFollowPolicyParam::Pinned => ArtifactSubscriptionUpdatePolicy::Pinned,
            };
            install_managed(
                service,
                imports,
                access_runtime,
                caller,
                project_id,
                request.source,
                request.assignment_id,
                request.idempotency_key,
                ManagedArtifactMirrorMode::Followed,
                Some(update_policy),
                SkillLibraryAction::Follow,
                correlation,
            )
            .await
        }
        "artifacts.follow_update" => {
            let request: ArtifactFollowUpdateParams =
                parse_params(params, "Artifact follow update parameters are invalid")?;
            validate_distribution_idempotency(&request.idempotency_key)?;
            follow_update(
                service,
                imports,
                access_runtime,
                caller,
                project_id,
                request,
                correlation,
            )
            .await
        }
        "artifacts.fork_personal" => {
            let request: ArtifactForkPersonalParams =
                parse_params(params, "Artifact personal fork parameters are invalid")?;
            validate_distribution_idempotency(&request.idempotency_key)?;
            fork_personal(
                service,
                imports,
                access_runtime,
                caller,
                project_id,
                request,
                correlation,
            )
            .await
        }
        _ => Err(ToolError::UnknownAction {
            message: format!("Unknown action: {action}"),
            valid: Vec::new(),
            hint: None,
        }),
    }
}

async fn transfer_options(
    imports: &Arc<ImportCoordinator>,
    access_runtime: &AccessRuntime,
    caller: &SkillLibraryCaller,
    project_id: &str,
    request: ArtifactTransferOptionsParams,
    correlation: &SkillLibraryCorrelationId,
) -> Result<Value, ToolError> {
    let provider_authority = request.source.provider_authority();
    let artifact_id = request.source.artifact_id().to_owned();
    let (decision, acquisition) = acquire_distribution(
        imports,
        access_runtime,
        caller,
        project_id,
        SkillLibraryAction::TransferOptions,
        request.source,
        correlation,
    )
    .await?;
    let authority = decision.authority;
    let store = access_store(access_runtime).await?;
    let options = store
        .artifact_transfer_options(
            provider_authority.clone(),
            artifact_id.clone(),
            request.assignment_id,
            authority.grants,
            acquisition.interchange.publication.clone(),
            acquisition.interchange.license.clone(),
            Some(ArtifactDestinationPolicy::local_personal()),
        )
        .await
        .map_err(map_access_error)?;
    Ok(json!({
        "providerAuthority": provider_authority,
        "artifactId": artifact_id,
        "revisionId": acquisition.interchange.revision.id,
        "policyRevision": authority.global_revision,
        "options": options.decisions().iter().map(|decision| json!({
            "mode": decision.mode.as_wire(),
            "allowed": decision.allowed,
        })).collect::<Vec<_>>(),
    }))
}

#[allow(clippy::too_many_arguments)]
async fn install_managed(
    service: &Arc<ProcessSkillLibrary>,
    imports: &Arc<ImportCoordinator>,
    access_runtime: &AccessRuntime,
    caller: &SkillLibraryCaller,
    project_id: &str,
    source: SourceSelector,
    assignment_id: String,
    idempotency_key: String,
    mode: ManagedArtifactMirrorMode,
    update_policy: Option<ArtifactSubscriptionUpdatePolicy>,
    action: SkillLibraryAction,
    correlation: &SkillLibraryCorrelationId,
) -> Result<Value, ToolError> {
    if mode == ManagedArtifactMirrorMode::Followed
        && update_policy != Some(ArtifactSubscriptionUpdatePolicy::Pinned)
        && source.depot_connection_id().is_none()
    {
        return Err(ToolError::Sdk {
            sdk_kind: "source_unavailable".to_owned(),
            message: "Automatic follow observation requires a configured Depot source".to_owned(),
        });
    }
    let provider_authority = source.provider_authority();
    let artifact_id = source.artifact_id().to_owned();
    let (decision, acquisition) = acquire_distribution(
        imports,
        access_runtime,
        caller,
        project_id,
        action,
        source,
        correlation,
    )
    .await?;
    let revision_id = acquisition.interchange.revision.id.clone();
    let authorization = ManagedArtifactAuthorization {
        identity_ref_json: serde_json::to_string(
            &crate::access::DurableIdentityReference::capture(caller.identity()),
        )
        .map_err(|_| ToolError::Sdk {
            sdk_kind: "internal_error".to_owned(),
            message: "Managed Artifact identity could not be serialized".to_owned(),
        })?,
        project_id: project_id.to_owned(),
        selected_team_id: caller.selected_team_id().map(str::to_owned),
    };
    let ArtifactDistributionAuthorizationDecision { authority, audit } = decision;
    audited_distribution_mutation(&audit, &revision_id, async {
        let store = access_store(access_runtime).await?;
        let options = store
            .artifact_transfer_options(
                provider_authority.clone(),
                artifact_id.clone(),
                Some(assignment_id.clone()),
                authority.grants,
                acquisition.interchange.publication.clone(),
                acquisition.interchange.license.clone(),
                Some(ArtifactDestinationPolicy::local_personal()),
            )
            .await
            .map_err(map_access_error)?;
        let required_mode = match mode {
            ManagedArtifactMirrorMode::Pinned => ArtifactTransferMode::Pin,
            ManagedArtifactMirrorMode::Followed => ArtifactTransferMode::Follow,
        };
        if !options.allows(required_mode) {
            return Err(super::artifact_distribution_denied());
        }
        let assignment = store
            .artifact_assignment_distribution(
                assignment_id.clone(),
                provider_authority.clone(),
                artifact_id.clone(),
            )
            .await
            .map_err(map_access_error)?
            .ok_or_else(super::artifact_distribution_denied)?;
        let mirror_id =
            managed_mirror_id(&provider_authority, &artifact_id, &authority.principal_id)?;
        let operation_id = managed_operation_id(
            action,
            &idempotency_key,
            &mirror_id,
            &authority.principal_id,
            project_id,
        )?;
        let now = unix_seconds()?;
        let coordinator = ManagedArtifactCoordinator::new(store, Arc::clone(&service.store));
        let outcome = coordinator
            .install(ManagedArtifactInstallRequest {
                mirror_id,
                operation_id,
                owner_principal_id: authority.principal_id,
                destination_id: None,
                source_provider_authority: provider_authority,
                source_assignment_id: assignment_id,
                source_scope: assignment.source_scope,
                mode,
                policy_epoch: authority.global_revision,
                update_policy,
                authorization,
                now,
                acquisition,
            })
            .await
            .map_err(super::map_managed_distribution_error)?;
        Ok(managed_receipt(outcome))
    })
    .await
}

async fn follow_update(
    service: &Arc<ProcessSkillLibrary>,
    imports: &Arc<ImportCoordinator>,
    access_runtime: &AccessRuntime,
    caller: &SkillLibraryCaller,
    project_id: &str,
    request: ArtifactFollowUpdateParams,
    correlation: &SkillLibraryCorrelationId,
) -> Result<Value, ToolError> {
    let provider_authority = request.source.provider_authority();
    let artifact_id = request.source.artifact_id().to_owned();
    let (decision, acquisition) = acquire_distribution(
        imports,
        access_runtime,
        caller,
        project_id,
        SkillLibraryAction::FollowUpdate,
        request.source,
        correlation,
    )
    .await?;
    let revision_id = acquisition.interchange.revision.id.clone();
    let ArtifactDistributionAuthorizationDecision { authority, audit } = decision;
    audited_distribution_mutation(&audit, &revision_id, async {
        let store = access_store(access_runtime).await?;
        let mirror_id =
            managed_mirror_id(&provider_authority, &artifact_id, &authority.principal_id)?;
        let mirror = store
            .managed_artifact_mirror(mirror_id.clone())
            .await
            .map_err(map_access_error)?
            .ok_or_else(super::artifact_distribution_denied)?;
        if mirror.owner_principal_id != authority.principal_id
            || mirror.source_provider_authority != provider_authority
            || mirror.source_artifact_id != artifact_id
            || mirror.mode != ManagedArtifactMirrorMode::Followed
        {
            return Err(super::artifact_distribution_denied());
        }
        let options = store
            .artifact_transfer_options(
                provider_authority.clone(),
                artifact_id.clone(),
                Some(mirror.source_assignment_id.clone()),
                authority.grants,
                acquisition.interchange.publication.clone(),
                acquisition.interchange.license.clone(),
                Some(ArtifactDestinationPolicy::local_personal()),
            )
            .await
            .map_err(map_access_error)?;
        if !options.allows(ArtifactTransferMode::Follow) {
            return Err(super::artifact_distribution_denied());
        }
        let expected_local_revision_id =
            mirror
                .local_revision_id
                .clone()
                .ok_or_else(|| ToolError::Conflict {
                    message: "Managed Artifact mirror is not active".to_owned(),
                    existing_id: mirror.mirror_id.clone(),
                })?;
        let operation_id = managed_operation_id(
            SkillLibraryAction::FollowUpdate,
            &request.idempotency_key,
            &mirror_id,
            &authority.principal_id,
            project_id,
        )?;
        let now = unix_seconds()?;
        let coordinator = ManagedArtifactCoordinator::new(store, Arc::clone(&service.store));
        let outcome = coordinator
            .apply_follow_update(ManagedArtifactFollowUpdateRequest {
                mirror_id,
                operation_id,
                expected_local_revision_id,
                policy_epoch: authority.global_revision,
                now,
                acquisition,
            })
            .await
            .map_err(super::map_managed_distribution_error)?;
        Ok(managed_receipt(outcome))
    })
    .await
}

async fn fork_personal(
    service: &Arc<ProcessSkillLibrary>,
    imports: &Arc<ImportCoordinator>,
    access_runtime: &AccessRuntime,
    caller: &SkillLibraryCaller,
    project_id: &str,
    request: ArtifactForkPersonalParams,
    correlation: &SkillLibraryCorrelationId,
) -> Result<Value, ToolError> {
    let provider_authority = request.source.provider_authority();
    let source_artifact_id = request.source.artifact_id().to_owned();
    let (decision, acquisition) = acquire_distribution(
        imports,
        access_runtime,
        caller,
        project_id,
        SkillLibraryAction::ForkPersonal,
        request.source,
        correlation,
    )
    .await?;
    let source_revision_id = acquisition.interchange.revision.id.clone();
    let ArtifactDistributionAuthorizationDecision { authority, audit } = decision;
    audited_distribution_mutation(&audit, &source_revision_id, async {
        let store = access_store(access_runtime).await?;
        let options = store
            .artifact_transfer_options(
                provider_authority.clone(),
                source_artifact_id.clone(),
                Some(request.assignment_id.clone()),
                authority.grants,
                acquisition.interchange.publication.clone(),
                acquisition.interchange.license.clone(),
                Some(ArtifactDestinationPolicy::local_personal()),
            )
            .await
            .map_err(map_access_error)?;
        if !options.allows(ArtifactTransferMode::Fork) {
            return Err(super::artifact_distribution_denied());
        }
        let assignment = store
            .artifact_assignment_distribution(
                request.assignment_id.clone(),
                provider_authority.clone(),
                source_artifact_id.clone(),
            )
            .await
            .map_err(map_access_error)?
            .ok_or_else(super::artifact_distribution_denied)?;
        let target = ArtifactDescriptor::for_identity(
            &acquisition.interchange.descriptor.kind,
            "personal",
            &request.name,
        )
        .map_err(|error| {
            super::map_dispatch_error(super::dispatch::SkillLibraryDispatchError::Artifact(error))
        })?;
        let operation_id = personal_fork_operation_id(
            &request.idempotency_key,
            &target.id,
            &provider_authority,
            &source_artifact_id,
            &source_revision_id,
            &request.assignment_id,
            &authority.principal_id,
            project_id,
        )?;
        let now = unix_seconds()?;
        let coordinator = ManagedArtifactCoordinator::new(store, Arc::clone(&service.store));
        let outcome = coordinator
            .fork_personal(PersonalArtifactForkRequest {
                operation_id,
                owner_principal_id: authority.principal_id,
                source_provider_authority: provider_authority,
                source_assignment_id: request.assignment_id,
                source_scope: assignment.source_scope,
                target_name: request.name,
                title: request.title,
                forked_at: Some(jiff::Timestamp::now().to_string()),
                policy_epoch: authority.global_revision,
                now,
                acquisition,
            })
            .await
            .map_err(super::map_managed_distribution_error)?;
        Ok(personal_fork_receipt(outcome))
    })
    .await
}

async fn acquire_distribution(
    imports: &Arc<ImportCoordinator>,
    access_runtime: &AccessRuntime,
    caller: &SkillLibraryCaller,
    project_id: &str,
    action: SkillLibraryAction,
    source: SourceSelector,
    correlation: &SkillLibraryCorrelationId,
) -> Result<
    (
        ArtifactDistributionAuthorizationDecision,
        labby_runtime::artifacts::ArtifactAcquisition,
    ),
    ToolError,
> {
    let target = CanonicalArtifactId::parse(source.artifact_id().to_owned()).map_err(|error| {
        super::map_dispatch_error(super::dispatch::SkillLibraryDispatchError::Artifact(error))
    })?;
    let requested_revision_id = source.revision_id().to_owned();
    let preflight = authorize_distribution_at_boundary(
        access_runtime,
        caller,
        project_id,
        action,
        &target,
        correlation,
    )
    .await
    .map_err(super::map_distribution_authorization_error)?;
    let acquisition = match imports
        .acquire_selected_for_distribution(access_runtime, caller, project_id, source)
        .await
    {
        Ok(acquisition) => acquisition,
        Err(error) => {
            if action != SkillLibraryAction::TransferOptions {
                record_distribution_terminal_result(
                    &preflight.audit,
                    Some(&requested_revision_id),
                    false,
                );
            }
            return Err(super::map_distribution_acquisition_error(error));
        }
    };
    let actual_revision_id = acquisition.interchange.revision.id.clone();
    let decision = match authorize_distribution_at_boundary(
        access_runtime,
        caller,
        project_id,
        action,
        &target,
        correlation,
    )
    .await
    {
        Ok(decision) => decision,
        Err(error) => {
            if action != SkillLibraryAction::TransferOptions {
                record_distribution_terminal_result(
                    &preflight.audit,
                    Some(&actual_revision_id),
                    false,
                );
            }
            return Err(super::map_distribution_authorization_error(error));
        }
    };
    Ok((decision, acquisition))
}

async fn audited_distribution_mutation<T>(
    audit: &SkillLibraryAuditEvent,
    revision_id: &str,
    future: impl Future<Output = Result<T, ToolError>>,
) -> Result<T, ToolError> {
    let result = future.await;
    record_distribution_terminal_result(audit, Some(revision_id), result.is_ok());
    result
}

fn record_distribution_terminal_result(
    audit: &SkillLibraryAuditEvent,
    revision_id: Option<&str>,
    committed: bool,
) -> bool {
    let terminal = SkillLibraryTerminalAudit::new(
        if committed {
            SkillLibraryTerminalOutcome::Committed
        } else {
            SkillLibraryTerminalOutcome::Failed
        },
        SkillLibraryTerminalStage::Commit,
    );
    let terminal = revision_id.map_or(terminal, |revision| terminal.with_revision_id(revision));
    let retained = record_terminal_mutation(audit, terminal);
    if !retained {
        tracing::warn!(
            service = "skill_library",
            action = ?audit.action,
            "terminal Artifact distribution audit event was not retained"
        );
    }
    retained
}

async fn access_store(
    access_runtime: &AccessRuntime,
) -> Result<crate::access::AccessStore, ToolError> {
    access_runtime
        .store()
        .await
        .map_err(|error| crate::dispatch::access_errors::map_runtime_error("artifacts", error))
}

fn map_access_error(error: AccessStoreError) -> ToolError {
    crate::dispatch::access_errors::map_store_error(
        "artifacts",
        error,
        super::artifact_distribution_denied,
    )
}

fn validate_distribution_idempotency(value: &str) -> Result<(), ToolError> {
    validate_idempotency_key(value).map_err(|_| ToolError::InvalidParam {
        message: "Artifact distribution idempotency key is invalid".to_owned(),
        param: "idempotency_key".to_owned(),
    })
}

fn managed_mirror_id(
    provider_authority: &str,
    artifact_id: &str,
    principal_id: &str,
) -> Result<String, ToolError> {
    stable_id(
        "mirror_",
        &json!({
            "schema": "labby.managed-artifact-mirror/v1",
            "providerAuthority": provider_authority,
            "artifactId": artifact_id,
            "principalId": principal_id,
            "destination": "local",
        }),
    )
}

fn managed_operation_id(
    action: SkillLibraryAction,
    idempotency_key: &str,
    mirror_id: &str,
    principal_id: &str,
    project_id: &str,
) -> Result<String, ToolError> {
    stable_id(
        "operation_",
        &json!({
            "schema": "labby.managed-artifact-operation/v1",
            "action": action.as_str(),
            "idempotencyKey": idempotency_key,
            "mirrorId": mirror_id,
            "principalId": principal_id,
            "projectId": project_id,
        }),
    )
}

fn personal_fork_operation_id(
    idempotency_key: &str,
    target_artifact_id: &str,
    provider_authority: &str,
    source_artifact_id: &str,
    source_revision_id: &str,
    assignment_id: &str,
    principal_id: &str,
    project_id: &str,
) -> Result<String, ToolError> {
    stable_id(
        "operation_",
        &json!({
            "schema": "labby.personal-artifact-fork-operation/v1",
            "action": SkillLibraryAction::ForkPersonal.as_str(),
            "idempotencyKey": idempotency_key,
            "targetArtifactId": target_artifact_id,
            "sourceProviderAuthority": provider_authority,
            "sourceArtifactId": source_artifact_id,
            "sourceRevisionId": source_revision_id,
            "sourceAssignmentId": assignment_id,
            "principalId": principal_id,
            "projectId": project_id,
        }),
    )
}

fn stable_id(prefix: &str, value: &Value) -> Result<String, ToolError> {
    let digest = canonical_json::digest(value).map_err(|error| {
        super::map_dispatch_error(super::dispatch::SkillLibraryDispatchError::Artifact(error))
    })?;
    let hex = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_owned(),
            message: "Artifact identity derivation failed".to_owned(),
        })?;
    Ok(format!("{prefix}{hex}"))
}

fn unix_seconds() -> Result<i64, ToolError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ToolError::Sdk {
            sdk_kind: "internal_error".to_owned(),
            message: "System clock is unavailable".to_owned(),
        })?
        .as_secs();
    i64::try_from(seconds).map_err(|_| ToolError::Sdk {
        sdk_kind: "internal_error".to_owned(),
        message: "System clock is unavailable".to_owned(),
    })
}

fn managed_receipt(outcome: ManagedArtifactInstallOutcome) -> Value {
    let mirror = outcome.mirror;
    json!({
        "mirrorId": mirror.mirror_id,
        "operationId": mirror.operation_id,
        "mode": mirror.mode.as_wire(),
        "status": mirror.status.as_wire(),
        "sourceProviderAuthority": mirror.source_provider_authority,
        "sourceAssignmentId": mirror.source_assignment_id,
        "sourceArtifactId": mirror.source_artifact_id,
        "sourceRevisionId": mirror.source_revision_id,
        "localArtifactId": mirror.local_artifact_id,
        "localRevisionId": mirror.local_revision_id,
        "policyRevision": mirror.last_authorized_policy_epoch,
        "subscription": outcome.subscription.map(|subscription| json!({
            "updatePolicy": subscription.update_policy.as_wire(),
            "lastObservedRevisionId": subscription.last_observed_revision_id,
            "lastAppliedRevisionId": subscription.last_applied_revision_id,
            "lastCheckedAt": subscription.last_checked_at,
            "active": subscription.active,
        })),
    })
}

fn personal_fork_receipt(outcome: PersonalArtifactForkOutcome) -> Value {
    json!({
        "artifactId": outcome.artifact.descriptor.id,
        "revisionId": outcome.artifact.current_revision_id,
        "sourceArtifactId": outcome.artifact.lineage.forked_from_artifact_id,
        "sourceRevisionId": outcome.artifact.lineage.forked_from_revision_id,
        "ownerKind": "personal",
        "ownerId": outcome.authority.owner.id(),
        "authorityStatus": outcome.authority.status.as_wire(),
        "operationId": outcome.authority.operation_id,
        "policyRevision": outcome.authority.policy_epoch,
    })
}

fn parse_params<T: serde::de::DeserializeOwned>(
    params: Value,
    message: &'static str,
) -> Result<T, ToolError> {
    serde_json::from_value(params).map_err(|_| ToolError::InvalidParam {
        message: message.to_owned(),
        param: "params".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use labby_runtime::artifacts::{LibraryActorId, LibraryTenantId};

    use super::super::audit::{SkillLibraryAuditOutcome, SkillLibraryAuditStage};
    use super::super::auth::SkillLibrarySurface;
    use super::*;

    #[test]
    fn distribution_terminal_audit_deduplicates_retry_and_distinguishes_failure() {
        let target = CanonicalArtifactId::parse("artifact-a").unwrap();
        let tenant = LibraryTenantId::from_canonical_projection("tenant-a").unwrap();
        let actor = LibraryActorId::from_canonical_projection("actor-a").unwrap();
        let audit = SkillLibraryAuditEvent::new(
            SkillLibraryCorrelationId::server("distribution-terminal-test"),
            &target,
            SkillLibraryAction::Pin,
            SkillLibrarySurface::Mcp,
            SkillLibraryAuditOutcome::Allow,
            SkillLibraryAuditStage::AccessSnapshot,
        )
        .with_canonical_actor(tenant, actor, 17);

        assert!(record_distribution_terminal_result(
            &audit,
            Some("revision-a"),
            true,
        ));
        assert!(!record_distribution_terminal_result(
            &audit,
            Some("revision-a"),
            true,
        ));
        assert!(record_distribution_terminal_result(
            &audit,
            Some("revision-a"),
            false,
        ));
    }
}
