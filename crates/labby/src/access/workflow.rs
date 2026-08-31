use labby_auth::{Authenticator, PrincipalLink, VerifiedIdentity};
use thiserror::Error;

use super::{
    AccessBlockedReason, AccessRuntime, AccessRuntimeError, BootstrapOutcome, BootstrapOwnerInput,
};

/// Stable, redacted failures suitable for an application-surface adapter.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum OwnerBootstrapError {
    #[error("owner bootstrap requires a browser-authenticated external identity")]
    IdentityNotEligible,
    #[error("owner bootstrap input is invalid")]
    InvalidInput,
    #[error("owner bootstrap conflicts with existing access state")]
    Conflict,
    #[error("owner bootstrap storage is busy")]
    Busy,
    #[error("owner bootstrap storage failed integrity validation")]
    Integrity,
    #[error("owner bootstrap storage is unavailable")]
    Unavailable,
}

/// Run the one-shot owner bootstrap from an already-authenticated browser identity.
///
/// This is intentionally below all transports: it neither authenticates a request nor
/// exposes a generic CLI/MCP operation. Callers pass the process-scoped runtime owner.
pub(crate) async fn bootstrap_owner(
    runtime: &AccessRuntime,
    identity: VerifiedIdentity,
    organization_name: String,
    project_name: String,
) -> Result<BootstrapOutcome, OwnerBootstrapError> {
    if identity.authenticator() != Authenticator::BrowserSession
        || !matches!(identity.principal_link(), PrincipalLink::External { .. })
    {
        return Err(OwnerBootstrapError::IdentityNotEligible);
    }

    let input = BootstrapOwnerInput::new(identity, organization_name, project_name)
        .map_err(|_| OwnerBootstrapError::InvalidInput)?;
    runtime
        .bootstrap_owner(input)
        .await
        .map_err(map_runtime_error)
}

fn map_runtime_error(error: AccessRuntimeError) -> OwnerBootstrapError {
    match error {
        AccessRuntimeError::InvalidBootstrapInput => OwnerBootstrapError::InvalidInput,
        AccessRuntimeError::BootstrapConflict => OwnerBootstrapError::Conflict,
        AccessRuntimeError::Blocked(AccessBlockedReason::Locked) => OwnerBootstrapError::Busy,
        AccessRuntimeError::Blocked(
            AccessBlockedReason::Corrupt | AccessBlockedReason::NewerSchema,
        ) => OwnerBootstrapError::Integrity,
        AccessRuntimeError::SetupRequired(_)
        | AccessRuntimeError::Blocked(_)
        | AccessRuntimeError::LifecycleUnavailable => OwnerBootstrapError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use labby_auth::{Authenticator, VerifiedIdentity};

    use super::*;

    fn secure_tempdir() -> tempfile::TempDir {
        super::super::test_support::secure_tempdir()
    }

    fn browser_identity(subject: &str) -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            subject,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn browser_external_identity_creates_then_idempotently_reuses_owner() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let runtime = AccessRuntime::initialize(path).await;

        assert_eq!(
            bootstrap_owner(
                &runtime,
                browser_identity("owner"),
                "Local".into(),
                "Default".into()
            )
            .await,
            Ok(BootstrapOutcome::Created)
        );
        assert_eq!(
            bootstrap_owner(
                &runtime,
                browser_identity("owner"),
                "Local".into(),
                "Default".into()
            )
            .await,
            Ok(BootstrapOutcome::AlreadyApplied)
        );
    }

    #[tokio::test]
    async fn bearer_external_identity_is_rejected_before_store_open() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let identity = VerifiedIdentity::external(
            Authenticator::OauthBearer,
            "https://accounts.google.com",
            "owner",
        )
        .unwrap();

        assert_eq!(
            bootstrap_owner(
                &AccessRuntime::initialize(path.clone()).await,
                identity,
                "Local".into(),
                "Default".into(),
            )
            .await,
            Err(OwnerBootstrapError::IdentityNotEligible)
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn browser_local_identity_is_rejected_before_store_open() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let identity = VerifiedIdentity::local_credential(
            Authenticator::BrowserSession,
            "browser-local-credential",
        )
        .unwrap();

        assert_eq!(
            bootstrap_owner(
                &AccessRuntime::initialize(path.clone()).await,
                identity,
                "Local".into(),
                "Default".into(),
            )
            .await,
            Err(OwnerBootstrapError::IdentityNotEligible)
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn identity_or_configuration_drift_is_a_redacted_conflict() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let runtime = AccessRuntime::initialize(path).await;
        bootstrap_owner(
            &runtime,
            browser_identity("owner"),
            "Local".into(),
            "Default".into(),
        )
        .await
        .unwrap();

        assert_eq!(
            bootstrap_owner(
                &runtime,
                browser_identity("other"),
                "Local".into(),
                "Default".into()
            )
            .await,
            Err(OwnerBootstrapError::Conflict)
        );
    }

    #[tokio::test]
    async fn storage_errors_do_not_expose_paths_or_sqlite_details() {
        let directory = secure_tempdir();
        let missing_parent = directory
            .path()
            .join("missing-ancestor")
            .join("access")
            .join("access.db");

        let runtime = AccessRuntime::initialize(missing_parent.clone()).await;
        let error = bootstrap_owner(
            &runtime,
            browser_identity("owner"),
            "Local".into(),
            "Default".into(),
        )
        .await
        .unwrap_err();
        assert_eq!(error, OwnerBootstrapError::Unavailable);
        assert!(
            !error
                .to_string()
                .contains(&missing_parent.display().to_string())
        );
        assert!(!error.to_string().to_ascii_lowercase().contains("sqlite"));
    }
}
