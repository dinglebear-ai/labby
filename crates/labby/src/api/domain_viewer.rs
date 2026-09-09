//! Compose verified domain eligibility with the host-selected Viewer project.
use axum::{
    Json,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse as _, Response},
};
use labby_auth::{
    AuthContext, PrincipalLink, VerifiedIdentity, browser_authority::BrowserAuthority,
};

use crate::api::state::AppState;

fn denied() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({"kind":"forbidden","message":"verified team access required"})),
    )
        .into_response()
}

/// Runs inside AuthLayer: no cookie parsing, email matching or identity synthesis.
pub(super) async fn provision(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let Some(project) = state
        .config
        .auth
        .as_ref()
        .and_then(|config| config.viewer_project_id.as_deref())
    else {
        return next.run(request).await;
    };
    let Some(auth) = request.extensions().get::<AuthContext>() else {
        return next.run(request).await;
    };
    if !auth.via_session {
        return next.run(request).await;
    }
    let Some(authority) = request.extensions().get::<BrowserAuthority>() else {
        return next.run(request).await;
    };
    let proof = match authority.verified_viewer_domain().await {
        Ok(Some(proof)) => proof,
        Ok(None) => return next.run(request).await,
        Err(_) => return denied(),
    };
    let Some(identity) = request.extensions().get::<VerifiedIdentity>() else {
        return denied();
    };
    if project.is_empty()
        || project.len() > 96
        || project.chars().any(char::is_control)
        || project.trim() != project
        || proof.identity() != identity
        || !matches!(identity.principal_link(), PrincipalLink::External { subject, .. } if subject == &auth.sub)
        || proof.revalidate().await.is_err()
    {
        return denied();
    }
    if state
        .access_runtime
        .provision_team_viewer(identity.clone(), project.to_owned())
        .await
        .is_err()
        || proof.revalidate().await.is_err()
    {
        return denied();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{
        AccessRuntime, AccessStore, AuthorizeProjectInput, BootstrapOwnerInput, Permission,
        ProjectRole,
    };
    use axum::{Router, routing::get};
    use std::sync::Arc;
    use tower::ServiceExt as _;

    async fn eligible_fixture(
        email: &str,
        verified: bool,
    ) -> (tempfile::TempDir, Router, AccessStore, VerifiedIdentity) {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let root = directory.path().canonicalize().unwrap();
        let auth = Arc::new(
            labby_auth::state::AuthState::new(labby_auth::config::AuthConfig {
                mode: labby_auth::config::AuthMode::OAuth,
                public_url: Some(url::Url::parse("https://lab.example.com").unwrap()),
                sqlite_path: root.join("auth.db"),
                key_path: root.join("auth-key.pem"),
                admin_email: "owner@admin.example".into(),
                session_cookie_name: "__Host-labby-session".into(),
                viewer_email_domains: vec!["example.org".into()],
                google: labby_auth::config::GoogleConfig {
                    client_id: "client".into(),
                    client_secret: "secret".into(),
                    callback_url: None,
                    callback_path: "/auth/google/callback".into(),
                    scopes: vec!["openid".into(), "email".into()],
                },
                token_encryption_key: Some(
                    labby_auth::at_rest::TokenEncryptionKey::from_encoded(&"11".repeat(32))
                        .unwrap(),
                ),
                ..Default::default()
            })
            .await
            .unwrap(),
        );
        let now = labby_auth::util::now_unix();
        if verified {
            auth.store
                .upsert_bound_verified_inbound_identity(
                    "new-viewer",
                    email,
                    now,
                    auth.inbound_provider_binding(),
                )
                .await
                .unwrap();
        }
        auth.store
            .upsert_bound_browser_session(
                labby_auth::types::BrowserSessionRow {
                    session_id: "viewer-session".into(),
                    subject: "new-viewer".into(),
                    email: Some(email.into()),
                    csrf_token: "viewer-csrf".into(),
                    created_at: now,
                    expires_at: now + 3600,
                    project_binding: None,
                },
                auth.inbound_provider_binding(),
            )
            .await
            .unwrap();
        let store = AccessStore::open(root.join("access.db")).await.unwrap();
        let owner = VerifiedIdentity::external(
            labby_auth::Authenticator::BrowserSession,
            "https://accounts.google.com",
            "original-owner",
        )
        .unwrap();
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner, "Team", "Default").unwrap())
            .await
            .unwrap();
        store.execute_test_statement("INSERT INTO project_loadouts VALUES ('bootstrap-local','bootstrap-default','team','bootstrap-owner',2,2)").await.unwrap();
        let identity = VerifiedIdentity::external(
            labby_auth::Authenticator::BrowserSession,
            "https://accounts.google.com",
            "new-viewer",
        )
        .unwrap();
        let mut state = AppState::new().with_access_runtime(Arc::new(
            AccessRuntime::initialize(root.join("access.db")).await,
        ));
        Arc::make_mut(&mut state.config).auth = Some(crate::config::AuthFileConfig {
            viewer_email_domains: Some(vec!["example.org".into()]),
            viewer_project_id: Some("bootstrap-default".into()),
            ..Default::default()
        });
        let downstream_store = store.clone();
        let router = Router::new()
            .route(
                "/test",
                get(
                    move |axum::extract::Extension(context): axum::extract::Extension<
                        AuthContext,
                    >,
                          axum::extract::Extension(identity): axum::extract::Extension<
                        VerifiedIdentity,
                    >| {
                        let store = downstream_store.clone();
                        async move {
                            assert_eq!(context.scopes, vec!["lab:read"]);
                            for permission in
                                [Permission::AssetDiscover, Permission::ArtifactPublish]
                            {
                                let access = store
                                    .authorize_project(AuthorizeProjectInput::new(
                                        identity.clone(),
                                        "bootstrap-default",
                                        permission,
                                    ))
                                    .await
                                    .unwrap();
                                assert_eq!(access.role, ProjectRole::Viewer);
                            }
                            for permission in [Permission::ProjectManage, Permission::AssetUse] {
                                assert!(
                                    store
                                        .authorize_project(AuthorizeProjectInput::new(
                                            identity.clone(),
                                            "bootstrap-default",
                                            permission
                                        ))
                                        .await
                                        .is_err()
                                );
                            }
                            StatusCode::NO_CONTENT
                        }
                    },
                ),
            )
            .route_layer(axum::middleware::from_fn_with_state(state, provision))
            .route_layer(labby_auth::AuthLayer::from_state(auth).with_allow_session_cookie(true));
        (directory, router, store, identity)
    }

    fn viewer_request() -> Request<Body> {
        Request::builder()
            .uri("/test")
            .header("cookie", "__Host-labby-session=viewer-session")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn verified_domain_browser_is_provisioned_before_read_publish_guards_and_is_idempotent() {
        let (_directory, router, store, identity) = eligible_fixture("new@example.org", true).await;
        assert_eq!(
            router
                .clone()
                .oneshot(viewer_request())
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        let before = store
            .authorize_project(AuthorizeProjectInput::new(
                identity.clone(),
                "bootstrap-default",
                Permission::AssetDiscover,
            ))
            .await
            .unwrap();
        assert_eq!(
            router
                .clone()
                .oneshot(viewer_request())
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        let after = store
            .authorize_project(AuthorizeProjectInput::new(
                identity,
                "bootstrap-default",
                Permission::AssetDiscover,
            ))
            .await
            .unwrap();
        assert_eq!(before, after);
        store
            .execute_test_statement(
                "UPDATE project_memberships SET status='disabled' WHERE role='viewer'",
            )
            .await
            .unwrap();
        assert_eq!(
            router.oneshot(viewer_request()).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn wrong_domain_or_unverified_display_email_cannot_provision() {
        for (email, verified) in [("new@wrong.example", true), ("new@example.org", false)] {
            let (_directory, router, store, identity) = eligible_fixture(email, verified).await;
            assert_eq!(
                router.oneshot(viewer_request()).await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
            assert!(
                store
                    .authorize_project(AuthorizeProjectInput::new(
                        identity,
                        "bootstrap-default",
                        Permission::AssetDiscover
                    ))
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn default_off_does_not_add_an_authentication_bypass_or_require_identity() {
        let state = AppState::new();
        let router = Router::new()
            .route("/test", get(|| async { StatusCode::NO_CONTENT }))
            .route_layer(axum::middleware::from_fn_with_state(state, provision));
        let response = router
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn outer_auth_still_rejects_unsigned_requests_and_static_bearer_is_not_provisioned() {
        let mut state = AppState::new();
        Arc::make_mut(&mut state.config).auth = Some(crate::config::AuthFileConfig {
            viewer_project_id: Some("missing-project".into()),
            viewer_email_domains: Some(vec!["example.org".into()]),
            ..Default::default()
        });
        let router = Router::new()
            .route("/test", get(|| async { StatusCode::NO_CONTENT }))
            .route_layer(axum::middleware::from_fn_with_state(state, provision))
            .route_layer(
                labby_auth::AuthLayer::new().with_static_token(Some(Arc::from("test-operator"))),
            );
        let response = router
            .clone()
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("authorization", "Bearer test-operator")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
}
