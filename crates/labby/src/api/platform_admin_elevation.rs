//! Durable platform-admin elevation for browser sessions.
//!
//! AuthLayer grants `lab:admin` to a browser session only for the configured
//! admin email; every other allowlisted identity is admitted without it,
//! because an allowlist entry is admission, not an administrative grant.
//! A principal that holds durable `platform.manage` authority (for example
//! one granted by `access.platform_admin.grant`) is elevated here, from the
//! access store, so administrative reach follows durable authority rather
//! than an OAuth scope. Any failure leaves the scopes unchanged (fail closed).
use axum::{body::Body, extract::State, http::Request, middleware::Next, response::Response};
use labby_auth::{AuthContext, VerifiedIdentity};

use crate::api::state::AppState;

const ADMIN_SCOPE: &str = "lab:admin";
const PLATFORM_MANAGE: &str = "platform.manage";

/// Runs inside AuthLayer, after it has attached the caller's identity.
pub(super) async fn elevate(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let needs_check = request
        .extensions()
        .get::<AuthContext>()
        .is_some_and(|auth| auth.via_session && !auth.scopes.iter().any(|s| s == ADMIN_SCOPE));
    if needs_check
        && let Some(identity) = request.extensions().get::<VerifiedIdentity>().cloned()
        && holds_platform_manage(&state, identity).await
        && let Some(auth) = request.extensions_mut().get_mut::<AuthContext>()
    {
        auth.scopes.push(ADMIN_SCOPE.to_owned());
    }
    next.run(request).await
}

async fn holds_platform_manage(state: &AppState, identity: VerifiedIdentity) -> bool {
    // `store()` issues a handle only while the runtime is Ready, so it is the
    // single readiness observation; a separate `status()` check would take the
    // runtime lock twice per request.
    let store = match state.access_runtime.store().await {
        Ok(store) => store,
        Err(error) => {
            // Expected before owner bootstrap; debug only. Identity is never logged.
            tracing::debug!(
                subsystem = "access",
                phase = "elevation",
                error = %error,
                "platform-admin elevation skipped: access runtime unavailable"
            );
            return false;
        }
    };
    match store.session_authority(identity).await {
        Ok(snapshot) => snapshot
            .capabilities
            .iter()
            .any(|capability| capability.as_wire() == PLATFORM_MANAGE),
        Err(error) => {
            tracing::warn!(
                subsystem = "access",
                phase = "elevation",
                error = %error,
                "platform-admin elevation denied: session authority lookup failed"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use labby_auth::Authenticator;
    use std::sync::Arc;
    use tower::ServiceExt as _;

    fn browser_identity(subject: &str) -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            subject,
        )
        .unwrap()
    }

    fn context(subject: &str, via_session: bool) -> AuthContext {
        AuthContext {
            actor_key: None,
            sub: subject.into(),
            scopes: vec!["lab:read".into(), "lab".into()],
            issuer: "browser-session".into(),
            via_session,
            csrf_token: None,
            email: None,
        }
    }

    async fn scopes_after_elevation(
        state: AppState,
        auth: AuthContext,
        identity: VerifiedIdentity,
    ) -> Vec<String> {
        let app = Router::new()
            .route(
                "/",
                get(
                    |axum::Extension(auth): axum::Extension<AuthContext>| async move {
                        auth.scopes.join(" ")
                    },
                ),
            )
            .route_layer(axum::middleware::from_fn_with_state(state, elevate))
            .layer(axum::Extension(identity))
            .layer(axum::Extension(auth));
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        String::from_utf8(body.to_vec())
            .unwrap()
            .split(' ')
            .map(ToOwned::to_owned)
            .collect()
    }

    async fn owner_state() -> (tempfile::TempDir, AppState) {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let path = directory.path().canonicalize().unwrap().join("access.db");
        let runtime = Arc::new(crate::access::AccessRuntime::initialize(path).await);
        runtime
            .bootstrap_owner(
                crate::access::BootstrapOwnerInput::new(
                    browser_identity("owner"),
                    "Local",
                    "Default",
                )
                .unwrap(),
            )
            .await
            .unwrap();
        (directory, AppState::new().with_access_runtime(runtime))
    }

    #[tokio::test]
    async fn durable_platform_admin_session_is_elevated() {
        let (_directory, state) = owner_state().await;
        let scopes =
            scopes_after_elevation(state, context("owner", true), browser_identity("owner")).await;
        assert!(scopes.iter().any(|scope| scope == ADMIN_SCOPE));
    }

    #[tokio::test]
    async fn unprovisioned_allowlisted_session_is_not_elevated() {
        let (_directory, state) = owner_state().await;
        let scopes = scopes_after_elevation(
            state,
            context("stranger", true),
            browser_identity("stranger"),
        )
        .await;
        assert!(!scopes.iter().any(|scope| scope == ADMIN_SCOPE));
    }

    /// Links principal `ops-principal` to the browser identity `ops` inside the
    /// bootstrap organization, without any platform authority.
    const SEED_OPS_PRINCIPAL: &str = "
        INSERT INTO principals(principal_id,organization_id,kind,status,display_name,created_at,updated_at)
          VALUES('ops-principal','bootstrap-local','user','active',NULL,2,2);
        INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,credential_id,status,verification_generation,link_generation,created_at,updated_at)
          VALUES('link-ops-principal','ops-principal','external','https://accounts.google.com','ops',NULL,'active',1,1,2,2);";

    async fn elevated(state: &AppState, subject: &str) -> bool {
        scopes_after_elevation(
            state.clone(),
            context(subject, true),
            browser_identity(subject),
        )
        .await
        .iter()
        .any(|scope| scope == ADMIN_SCOPE)
    }

    fn ops_grant() -> crate::access::PlatformAdministratorInput {
        crate::access::PlatformAdministratorInput::new(browser_identity("owner"), "ops-principal")
            .unwrap()
    }

    #[tokio::test]
    async fn non_owner_platform_admin_is_elevated_until_revoked() {
        let (_directory, state) = owner_state().await;
        let store = state.access_runtime.store().await.unwrap();
        store
            .execute_test_statement(SEED_OPS_PRINCIPAL)
            .await
            .unwrap();
        assert!(!elevated(&state, "ops").await, "no grant, no elevation");

        store
            .grant_platform_administrator(ops_grant())
            .await
            .unwrap();
        assert!(elevated(&state, "ops").await, "granted platform.manage");

        store
            .revoke_platform_administrator(ops_grant())
            .await
            .unwrap();
        assert!(
            !elevated(&state, "ops").await,
            "revocation applies to the next request; nothing is cached"
        );
    }

    #[tokio::test]
    async fn deactivated_principal_with_an_active_grant_is_not_elevated() {
        let (_directory, state) = owner_state().await;
        let store = state.access_runtime.store().await.unwrap();
        store
            .execute_test_statement(SEED_OPS_PRINCIPAL)
            .await
            .unwrap();
        store
            .grant_platform_administrator(ops_grant())
            .await
            .unwrap();
        assert!(elevated(&state, "ops").await);

        store
            .deactivate_principal_for_test("ops-principal")
            .await
            .unwrap();
        assert!(!elevated(&state, "ops").await);
    }

    #[tokio::test]
    async fn blocked_and_setup_required_runtimes_never_elevate() {
        use crate::access::{AccessBlockedReason, AccessRuntime, AccessRuntimeStatus};
        for reason in [
            AccessBlockedReason::Insecure,
            AccessBlockedReason::Corrupt,
            AccessBlockedReason::NewerSchema,
            AccessBlockedReason::Locked,
            AccessBlockedReason::ReadOnly,
            AccessBlockedReason::Unavailable,
        ] {
            let state = AppState::new()
                .with_access_runtime(Arc::new(AccessRuntime::blocked_for_test(reason)));
            assert!(!elevated(&state, "owner").await, "{reason:?}");
        }

        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let path = directory.path().canonicalize().unwrap().join("access.db");
        let runtime = Arc::new(AccessRuntime::initialize(path).await);
        assert!(matches!(
            runtime.status().await,
            AccessRuntimeStatus::SetupRequired(_)
        ));
        let state = AppState::new().with_access_runtime(runtime);
        assert!(!elevated(&state, "owner").await);
    }

    #[tokio::test]
    async fn store_error_never_elevates() {
        let (_directory, state) = owner_state().await;
        let store = state.access_runtime.store().await.unwrap();
        store
            .execute_test_statement(SEED_OPS_PRINCIPAL)
            .await
            .unwrap();
        store
            .grant_platform_administrator(ops_grant())
            .await
            .unwrap();
        assert!(elevated(&state, "ops").await);

        store
            .execute_test_statement("ALTER TABLE principal_links RENAME TO principal_links_hidden;")
            .await
            .unwrap();
        assert!(
            store
                .session_authority(browser_identity("ops"))
                .await
                .is_err(),
            "fixture must make the authority read fail"
        );
        assert!(!elevated(&state, "ops").await);
    }

    #[tokio::test]
    async fn non_session_callers_and_uninitialized_stores_are_never_elevated() {
        let (_directory, state) = owner_state().await;
        let scopes =
            scopes_after_elevation(state, context("owner", false), browser_identity("owner")).await;
        assert!(!scopes.iter().any(|scope| scope == ADMIN_SCOPE));

        let scopes = scopes_after_elevation(
            AppState::new(),
            context("owner", true),
            browser_identity("owner"),
        )
        .await;
        assert!(!scopes.iter().any(|scope| scope == ADMIN_SCOPE));
    }
}
