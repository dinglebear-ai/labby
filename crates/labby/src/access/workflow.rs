use std::path::PathBuf;

use labby_auth::{Authenticator, PrincipalLink, VerifiedIdentity};
use thiserror::Error;

use super::{AccessStore, BootstrapOutcome, BootstrapOwnerInput};
use crate::access::error::AccessStoreError;

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
/// exposes a generic CLI/MCP operation. Callers must pass the explicit access-store path.
pub(crate) async fn bootstrap_owner_at(
    store_path: PathBuf,
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
        .map_err(map_store_error)?;
    let store = AccessStore::open(store_path)
        .await
        .map_err(map_store_error)?;
    store.bootstrap_owner(input).await.map_err(map_store_error)
}

fn map_store_error(error: AccessStoreError) -> OwnerBootstrapError {
    match error {
        AccessStoreError::InvalidBootstrapInput => OwnerBootstrapError::InvalidInput,
        AccessStoreError::BootstrapConflict => OwnerBootstrapError::Conflict,
        AccessStoreError::Locked => OwnerBootstrapError::Busy,
        AccessStoreError::Corrupt
        | AccessStoreError::UnsupportedSchema { .. }
        | AccessStoreError::IntegrityViolation { .. }
        | AccessStoreError::ForeignKeyViolation
        | AccessStoreError::MalformedVocabulary => OwnerBootstrapError::Integrity,
        AccessStoreError::DiskFull
        | AccessStoreError::ReadOnly
        | AccessStoreError::InsecurePath { .. }
        | AccessStoreError::MissingParent { .. }
        | AccessStoreError::InsecurePermissions { .. }
        | AccessStoreError::IdentityUnavailable
        | AccessStoreError::ProjectAccessUnavailable
        | AccessStoreError::Unavailable(_) => OwnerBootstrapError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use labby_auth::{Authenticator, VerifiedIdentity};

    use super::*;

    fn secure_tempdir() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        directory
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

        assert_eq!(
            bootstrap_owner_at(
                path.clone(),
                browser_identity("owner"),
                "Local".into(),
                "Default".into()
            )
            .await,
            Ok(BootstrapOutcome::Created)
        );
        assert_eq!(
            bootstrap_owner_at(
                path,
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
            bootstrap_owner_at(path.clone(), identity, "Local".into(), "Default".into()).await,
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
            bootstrap_owner_at(path.clone(), identity, "Local".into(), "Default".into()).await,
            Err(OwnerBootstrapError::IdentityNotEligible)
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn identity_or_configuration_drift_is_a_redacted_conflict() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        bootstrap_owner_at(
            path.clone(),
            browser_identity("owner"),
            "Local".into(),
            "Default".into(),
        )
        .await
        .unwrap();

        assert_eq!(
            bootstrap_owner_at(
                path,
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

        let error = bootstrap_owner_at(
            missing_parent.clone(),
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
