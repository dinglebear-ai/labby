use std::future::Future;

use labby_auth::VerifiedIdentity;
use labby_gateway::gateway::manager::GatewayManager;
use thiserror::Error;

use super::error::AccessStoreError;
use super::{
    AccessRuntime, AccessRuntimeError, AssignProjectLoadoutInput, AssignProjectLoadoutOutcome,
};

/// Redacted failure contract for the unmounted Gateway compatibility adapter.
#[derive(Debug, Error)]
pub(crate) enum GatewayLoadoutAssignmentError {
    #[error("access runtime is unavailable")]
    RuntimeUnavailable,
    #[error("project loadout assignment input is invalid")]
    InvalidInput,
    #[error("project access is unavailable")]
    ProjectAccessUnavailable,
    #[error("project already has a different loadout assignment")]
    ProjectLoadoutConflict,
    #[error("access persistence is unavailable")]
    AccessUnavailable,
    #[error("gateway loadout is unavailable")]
    LoadoutUnavailable,
}

/// Admits one point-in-time desired-config Loadout name before atomically assigning it.
///
/// This is intentionally crate-private and unmounted. Validation is compatibility checking, not
/// authorization: access is checked before Gateway lookup and again by the store mutation.
pub(crate) async fn assign_admitted_project_loadout(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    identity: VerifiedIdentity,
    project_id: impl Into<String>,
    loadout_name: impl Into<String>,
) -> Result<AssignProjectLoadoutOutcome, GatewayLoadoutAssignmentError> {
    assign_with_validator(
        runtime,
        identity,
        project_id.into(),
        loadout_name.into(),
        |name| async move { manager.loadout_get(&name).await.map(|_| ()) },
    )
    .await
}

async fn assign_with_validator<F, Fut, E>(
    runtime: &AccessRuntime,
    identity: VerifiedIdentity,
    project_id: String,
    loadout_name: String,
    validator: F,
) -> Result<AssignProjectLoadoutOutcome, GatewayLoadoutAssignmentError>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<(), E>>,
{
    let store = runtime
        .store()
        .await
        .map_err(|_error: AccessRuntimeError| GatewayLoadoutAssignmentError::RuntimeUnavailable)?;
    // Authorize the opaque Project selector before validating any caller-controlled Loadout fact.
    // The store query safely treats malformed or unknown selectors as the same denial.
    store
        .authorize_project_management_without_loadout(identity.clone(), project_id.clone())
        .await
        .map_err(map_access_error)?;
    let input = AssignProjectLoadoutInput::new(identity, project_id, loadout_name)
        .map_err(map_access_error)?;
    validator(input.loadout_name().to_owned())
        .await
        .map_err(|_| GatewayLoadoutAssignmentError::LoadoutUnavailable)?;
    // This immediate transaction resolves identity, membership, role, and status again. The
    // preflight above is not a grant and cannot survive a concurrent revocation.
    store
        .assign_project_loadout(input)
        .await
        .map_err(map_access_error)
}

fn map_access_error(error: AccessStoreError) -> GatewayLoadoutAssignmentError {
    match error {
        AccessStoreError::InvalidProjectLoadoutInput => GatewayLoadoutAssignmentError::InvalidInput,
        AccessStoreError::ProjectAccessUnavailable
        | AccessStoreError::IdentityUnavailable
        | AccessStoreError::NotAuthorized => {
            GatewayLoadoutAssignmentError::ProjectAccessUnavailable
        }
        AccessStoreError::ProjectLoadoutConflict => {
            GatewayLoadoutAssignmentError::ProjectLoadoutConflict
        }
        AccessStoreError::Locked
        | AccessStoreError::Corrupt
        | AccessStoreError::DiskFull
        | AccessStoreError::ReadOnly
        | AccessStoreError::InsecurePath { .. }
        | AccessStoreError::MissingParent { .. }
        | AccessStoreError::InsecurePermissions { .. }
        | AccessStoreError::UnsupportedSchema { .. }
        | AccessStoreError::IntegrityViolation { .. }
        | AccessStoreError::ForeignKeyViolation
        | AccessStoreError::BootstrapConflict
        | AccessStoreError::InvalidBootstrapInput
        | AccessStoreError::MalformedVocabulary
        | AccessStoreError::Unavailable(_) => GatewayLoadoutAssignmentError::AccessUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use labby_auth::{Authenticator, VerifiedIdentity};
    #[cfg(feature = "proxy-testkit")]
    use labby_gateway::gateway::config_store::FsGatewayConfigStore;
    #[cfg(feature = "proxy-testkit")]
    use labby_gateway::gateway::manager::GatewayRuntimeHandle;
    #[cfg(feature = "proxy-testkit")]
    use labby_runtime::gateway_config::{GatewayConfig, GatewayLoadoutConfig};

    use super::*;
    use crate::access::BootstrapOwnerInput;

    fn identity(credential: &str) -> VerifiedIdentity {
        VerifiedIdentity::local_credential(Authenticator::StaticBearer, credential).unwrap()
    }

    async fn fixture() -> (tempfile::TempDir, AccessRuntime, VerifiedIdentity) {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let runtime = AccessRuntime::initialize(directory.path().join("access.db")).await;
        let owner = identity("static-bearer:gateway-adapter-owner");
        runtime
            .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        (directory, runtime, owner)
    }

    #[tokio::test]
    async fn validator_is_not_invoked_for_unauthorized_identity() {
        let (_directory, runtime, _owner) = fixture().await;
        let invoked = Arc::new(AtomicBool::new(false));
        let marker = Arc::clone(&invoked);
        let result = assign_with_validator(
            &runtime,
            identity("static-bearer:unknown"),
            "bootstrap-default".into(),
            "production".into(),
            move |_| async move {
                marker.store(true, Ordering::SeqCst);
                Ok::<_, ()>(())
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(GatewayLoadoutAssignmentError::ProjectAccessUnavailable)
        ));
        assert!(!invoked.load(Ordering::SeqCst));

        let result = assign_with_validator(
            &runtime,
            identity("static-bearer:unknown"),
            "bad\nproject".into(),
            "bad\nloadout".into(),
            |_| async { Err::<(), ()>(()) },
        )
        .await;
        assert!(matches!(
            result,
            Err(GatewayLoadoutAssignmentError::ProjectAccessUnavailable)
        ));
    }

    #[tokio::test]
    async fn validation_failure_has_no_state_change() {
        let (_directory, runtime, owner) = fixture().await;
        let store = runtime.store().await.unwrap();
        let before = store.loadout_state_for_test().await.unwrap();
        let result = assign_with_validator(
            &runtime,
            owner,
            "bootstrap-default".into(),
            "missing".into(),
            |_| async { Err::<(), _>("raw gateway detail") },
        )
        .await;

        assert!(matches!(
            &result,
            Err(GatewayLoadoutAssignmentError::LoadoutUnavailable)
        ));
        assert_eq!(
            result.unwrap_err().to_string(),
            "gateway loadout is unavailable"
        );
        assert_eq!(store.loadout_state_for_test().await.unwrap(), before);
    }

    #[test]
    fn persistence_error_details_are_redacted() {
        let error = map_access_error(AccessStoreError::Unavailable(
            "secret path and sqlite detail".into(),
        ));

        assert_eq!(error.to_string(), "access persistence is unavailable");
    }

    #[tokio::test]
    async fn successful_validation_assigns_and_audits() {
        let (_directory, runtime, owner) = fixture().await;
        let store = runtime.store().await.unwrap();
        let outcome = assign_with_validator(
            &runtime,
            owner,
            "bootstrap-default".into(),
            "production".into(),
            |name| async move {
                assert_eq!(name, "production");
                Ok::<_, ()>(())
            },
        )
        .await
        .unwrap();

        assert_eq!(outcome, AssignProjectLoadoutOutcome::Assigned);
        assert_eq!(
            store.loadout_state_for_test().await.unwrap(),
            (1, 2, 1, 1, 2)
        );
        assert_eq!(
            store.loadout_audit_for_test().await.unwrap().0,
            "access.project_loadout.assign"
        );
    }

    #[tokio::test]
    #[cfg(feature = "proxy-testkit")]
    async fn concrete_adapter_reads_desired_gateway_loadouts() {
        let (directory, runtime, owner) = fixture().await;
        let gateway_path = directory.path().join("gateway.toml");
        let manager = GatewayManager::with_store(
            gateway_path.clone(),
            GatewayRuntimeHandle::default(),
            Arc::new(FsGatewayConfigStore::new(gateway_path)),
        );
        manager
            .try_seed_config(GatewayConfig {
                loadouts: vec![GatewayLoadoutConfig {
                    name: "production".into(),
                    ..GatewayLoadoutConfig::default()
                }],
                ..GatewayConfig::default()
            })
            .await
            .unwrap();

        assert!(matches!(
            assign_admitted_project_loadout(
                &runtime,
                &manager,
                owner.clone(),
                "bootstrap-default",
                "missing",
            )
            .await,
            Err(GatewayLoadoutAssignmentError::LoadoutUnavailable)
        ));
        assert_eq!(
            assign_admitted_project_loadout(
                &runtime,
                &manager,
                owner,
                "bootstrap-default",
                "production",
            )
            .await
            .unwrap(),
            AssignProjectLoadoutOutcome::Assigned
        );
    }

    #[tokio::test]
    async fn mutation_reauthorizes_after_validation() {
        let (_directory, runtime, owner) = fixture().await;
        let store = runtime.store().await.unwrap();
        let validator_store = store.clone();
        let result = assign_with_validator(&runtime, owner, "bootstrap-default".into(), "production".into(), move |_| async move {
            validator_store.execute_test_statement("UPDATE project_memberships SET status='suspended' WHERE project_id='bootstrap-default'").await.unwrap();
            Ok::<_, ()>(())
        }).await;

        assert!(matches!(
            result,
            Err(GatewayLoadoutAssignmentError::ProjectAccessUnavailable)
        ));
        assert_eq!(
            store.loadout_state_for_test().await.unwrap(),
            (0, 1, 0, 0, 1)
        );
    }
}
