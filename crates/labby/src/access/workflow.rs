use labby_auth::{Authenticator, PrincipalLink, VerifiedIdentity};
use thiserror::Error;

use super::{
    AccessBlockedReason, AccessRuntime, AccessRuntimeError, BootstrapOutcome, BootstrapOwnerInput,
};

const ADMIN_SCOPE: &str = "lab:admin";

/// Stable, redacted failures suitable for an application-surface adapter.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum OwnerBootstrapError {
    /// Every admission failure collapses here so a caller cannot learn which
    /// rule (session, subject, scope, or configured admin email) refused it.
    #[error("owner bootstrap requires an authenticated browser admin")]
    IdentityNotEligible,
    /// No configured admin exists on this process (not an OAuth deployment).
    #[error("owner bootstrap is only available in OAuth mode")]
    NotConfigured,
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

/// Transport facts an adapter extracts from an authenticated request.
///
/// Adapters build this from `AuthContext` + `VerifiedIdentity` (or from the
/// equivalent browser-session facts); the admission rule itself lives only in
/// [`owner_bootstrap_admission`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct OwnerBootstrapCaller<'a> {
    /// The request was authenticated by a browser session cookie.
    pub(crate) via_session: bool,
    /// Subject the transport authenticated.
    pub(crate) subject: &'a str,
    /// Transport scopes granted to the caller.
    pub(crate) scopes: &'a [String],
    /// Verified email of the caller, when known.
    pub(crate) email: Option<&'a str>,
    /// Verified identity the transport attached.
    pub(crate) identity: &'a VerifiedIdentity,
}

/// The single owner-bootstrap admission rule.
///
/// Admits only a browser-session caller whose verified identity is an external
/// link for the same subject, who holds `lab:admin`, and whose email equals the
/// configured admin email. `configured_admin_email` is `None` outside OAuth mode.
pub(crate) fn owner_bootstrap_admission(
    caller: &OwnerBootstrapCaller<'_>,
    configured_admin_email: Option<&str>,
) -> Result<(), OwnerBootstrapError> {
    let identity_consistent = caller.via_session
        && caller.identity.authenticator() == Authenticator::BrowserSession
        && matches!(
            caller.identity.principal_link(),
            PrincipalLink::External { subject, .. } if subject == caller.subject
        );
    if !identity_consistent || !caller.scopes.iter().any(|scope| scope == ADMIN_SCOPE) {
        return Err(OwnerBootstrapError::IdentityNotEligible);
    }
    let Some(admin_email) = configured_admin_email else {
        return Err(OwnerBootstrapError::NotConfigured);
    };
    if !labby_auth::is_configured_admin_email(admin_email, caller.email) {
        return Err(OwnerBootstrapError::IdentityNotEligible);
    }
    Ok(())
}

/// Run the one-shot owner bootstrap for an admitted browser admin.
///
/// This is intentionally below all transports: it neither authenticates a request nor
/// exposes a generic CLI/MCP operation. It enforces the full admission rule itself, so
/// no adapter can reach the lifecycle with a weaker check. Callers pass the
/// process-scoped runtime owner.
pub(crate) async fn bootstrap_owner(
    runtime: &AccessRuntime,
    caller: OwnerBootstrapCaller<'_>,
    configured_admin_email: Option<&str>,
    organization_name: String,
    project_name: String,
) -> Result<BootstrapOutcome, OwnerBootstrapError> {
    owner_bootstrap_admission(&caller, configured_admin_email)?;
    let input = BootstrapOwnerInput::new(caller.identity.clone(), organization_name, project_name)
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

    const ADMIN: &str = "owner@example.com";

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

    fn admin_scopes() -> Vec<String> {
        vec!["lab:read".into(), "lab:admin".into()]
    }

    fn caller<'a>(
        identity: &'a VerifiedIdentity,
        subject: &'a str,
        scopes: &'a [String],
        email: Option<&'a str>,
    ) -> OwnerBootstrapCaller<'a> {
        OwnerBootstrapCaller {
            via_session: true,
            subject,
            scopes,
            email,
            identity,
        }
    }

    async fn run(
        runtime: &AccessRuntime,
        identity: &VerifiedIdentity,
        subject: &str,
    ) -> Result<BootstrapOutcome, OwnerBootstrapError> {
        let scopes = admin_scopes();
        bootstrap_owner(
            runtime,
            caller(identity, subject, &scopes, Some(ADMIN)),
            Some(ADMIN),
            "Local".into(),
            "Default".into(),
        )
        .await
    }

    #[test]
    fn admission_requires_every_rule_and_hides_which_one_failed() {
        let identity = browser_identity("subject");
        let admin = admin_scopes();
        let read = vec!["lab:read".to_owned()];
        let ok = caller(&identity, "subject", &admin, Some("OWNER@example.com"));
        assert_eq!(owner_bootstrap_admission(&ok, Some(ADMIN)), Ok(()));

        let bearer = VerifiedIdentity::external(
            Authenticator::OauthBearer,
            "https://accounts.google.com",
            "subject",
        )
        .unwrap();
        let local = VerifiedIdentity::local_credential(
            Authenticator::BrowserSession,
            "browser-local-credential",
        )
        .unwrap();
        let refused = [
            OwnerBootstrapCaller {
                via_session: false,
                ..ok
            },
            caller(&bearer, "subject", &admin, Some(ADMIN)),
            caller(&local, "subject", &admin, Some(ADMIN)),
            caller(&identity, "different-subject", &admin, Some(ADMIN)),
            caller(&identity, "subject", &read, Some(ADMIN)),
            caller(&identity, "subject", &admin, Some("colleague@example.com")),
            caller(&identity, "subject", &admin, None),
        ];
        for refused in refused {
            assert_eq!(
                owner_bootstrap_admission(&refused, Some(ADMIN)),
                Err(OwnerBootstrapError::IdentityNotEligible),
                "{refused:?}"
            );
        }
        assert_eq!(
            owner_bootstrap_admission(&ok, None),
            Err(OwnerBootstrapError::NotConfigured)
        );
    }

    #[tokio::test]
    async fn browser_external_identity_creates_then_idempotently_reuses_owner() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let runtime = AccessRuntime::initialize(path).await;
        let identity = browser_identity("owner");

        assert_eq!(
            run(&runtime, &identity, "owner").await,
            Ok(BootstrapOutcome::Created)
        );
        assert_eq!(
            run(&runtime, &identity, "owner").await,
            Ok(BootstrapOutcome::AlreadyApplied)
        );
    }

    #[tokio::test]
    async fn ineligible_callers_are_rejected_before_store_open() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let runtime = AccessRuntime::initialize(path.clone()).await;
        let bearer = VerifiedIdentity::external(
            Authenticator::OauthBearer,
            "https://accounts.google.com",
            "owner",
        )
        .unwrap();
        let local = VerifiedIdentity::local_credential(
            Authenticator::BrowserSession,
            "browser-local-credential",
        )
        .unwrap();
        let identity = browser_identity("owner");
        let scopes = admin_scopes();

        for (identity, email) in [
            (&bearer, Some(ADMIN)),
            (&local, Some(ADMIN)),
            (&identity, Some("colleague@example.com")),
        ] {
            assert_eq!(
                bootstrap_owner(
                    &runtime,
                    caller(identity, "owner", &scopes, email),
                    Some(ADMIN),
                    "Local".into(),
                    "Default".into(),
                )
                .await,
                Err(OwnerBootstrapError::IdentityNotEligible)
            );
        }
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn identity_or_configuration_drift_is_a_redacted_conflict() {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let runtime = AccessRuntime::initialize(path).await;
        run(&runtime, &browser_identity("owner"), "owner")
            .await
            .unwrap();

        assert_eq!(
            run(&runtime, &browser_identity("other"), "other").await,
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
        let error = run(&runtime, &browser_identity("owner"), "owner")
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
