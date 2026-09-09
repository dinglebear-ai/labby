//! Explicit production-owned fixtures for live integration tests.

use std::path::PathBuf;

/// Provision an active, same-organization File Stash recipient using the
/// AccessStore authority and its current-schema validation.
pub async fn provision_file_stash_recipient(
    access_store_path: PathBuf,
    owner_credential_id: String,
    principal_id: String,
    display_name: String,
    recipient_credential_id: String,
) -> Result<(), String> {
    let store = crate::access::AccessStore::open_existing_current(access_store_path)
        .await
        .map_err(|error| error.to_string())?;
    store
        .provision_file_stash_recipient_fixture(
            owner_credential_id,
            principal_id,
            display_name,
            recipient_credential_id,
        )
        .await
        .map_err(|error| error.to_string())
}

/// Bind the explicit static-owner fixture to its local stdio transport only.
/// The durable store must already recognize this identity; the fixture never
/// grants authority or creates a principal at request time.
#[cfg(any(test, feature = "proxy-testkit"))]
pub(crate) async fn local_stdio_fixture_identity(
    runtime: &crate::access::AccessRuntime,
    transport: &str,
    enabled: bool,
) -> Option<labby_auth::VerifiedIdentity> {
    if transport != "stdio" || !enabled {
        return None;
    }
    let identity = labby_auth::VerifiedIdentity::local_credential(
        labby_auth::Authenticator::StaticBearer,
        "static-bearer:primary",
    )
    .ok()?;
    runtime.session_authority(identity.clone()).await.ok()?;
    Some(identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stdio_fixture_requires_opt_in_transport_and_durable_identity() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let runtime =
            crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await;
        assert!(
            local_stdio_fixture_identity(&runtime, "stdio", true)
                .await
                .is_none()
        );
        let identity = labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "static-bearer:primary",
        )
        .unwrap();
        runtime
            .bootstrap_owner(
                crate::access::BootstrapOwnerInput::new(identity, "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        assert!(
            local_stdio_fixture_identity(&runtime, "stdio", false)
                .await
                .is_none()
        );
        assert!(
            local_stdio_fixture_identity(&runtime, "http", true)
                .await
                .is_none()
        );
        assert!(
            local_stdio_fixture_identity(&runtime, "stdio", true)
                .await
                .is_some()
        );
    }
}
