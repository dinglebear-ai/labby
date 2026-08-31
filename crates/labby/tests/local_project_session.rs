use labby_auth::project_session::ProjectSessionState;
use labby_primitives::product_credential::BoundAccessGrant;

fn grant(expires_at: u64) -> BoundAccessGrant {
    BoundAccessGrant {
        installation_id: "installation".into(),
        issuer: "issuer".into(),
        subject: "subject".into(),
        principal_id: "principal".into(),
        organization_id: "organization".into(),
        project_id: "project".into(),
        loadout_id: "loadout".into(),
        loadout_generation: 2,
        assignment_generation: 3,
        catalog_generation: 4,
        route_id: "route".into(),
        route_generation: 5,
        membership_epoch: 6,
        organization_policy_epoch: 7,
        project_policy_epoch: 8,
        credential_id: "credential".into(),
        credential_generation: 9,
        scopes: vec!["lab:read".into()],
        resource: "lab://project".into(),
        audience: "labby".into(),
        expires_at,
        requires_admin: false,
        destructive: false,
    }
}

#[tokio::test]
async fn project_session_persists_exact_source_binding_across_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.db");
    let state = ProjectSessionState::open(path.clone(), "__Host-labby-project")
        .await
        .unwrap();
    let expected = grant(u64::try_from(labby_auth::util::now_unix() + 300).unwrap());
    let row = state.create(&expected).await.unwrap();
    assert!(
        row.project_binding.as_ref().unwrap()
            == &labby_auth::ProjectSessionBinding::from(&expected)
    );
    assert_ne!(row.session_id, row.csrf_token);
    drop(state);

    let reopened = ProjectSessionState::open(path, "__Host-labby-project")
        .await
        .unwrap();
    let persisted = reopened
        .store
        .find_browser_session(&row.session_id)
        .await
        .unwrap()
        .unwrap();
    assert!(persisted.project_binding == row.project_binding);
    reopened
        .store
        .revoke_browser_session(&row.session_id)
        .await
        .unwrap();
    assert!(
        reopened
            .store
            .find_browser_session(&row.session_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn expired_source_is_denied_without_session_state() {
    let directory = tempfile::tempdir().unwrap();
    let state =
        ProjectSessionState::open(directory.path().join("sessions.db"), "__Host-labby-project")
            .await
            .unwrap();
    let result = state.create(&grant(1)).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn cookies_are_host_only_secure_strict_and_revocable() {
    let directory = tempfile::tempdir().unwrap();
    let state =
        ProjectSessionState::open(directory.path().join("sessions.db"), "__Host-labby-project")
            .await
            .unwrap();
    let set = state.set_cookie("opaque-session", 60);
    assert!(set.contains("Path=/"));
    assert!(set.contains("HttpOnly"));
    assert!(set.contains("Secure"));
    assert!(set.contains("SameSite=Strict"));
    assert!(!set.contains("Domain="));
    assert!(state.clear_cookie().contains("Max-Age=0"));
}

#[test]
fn generation_scope_and_route_drift_change_the_session_authority_tuple() {
    let original = grant(1_000);
    let expected = labby_auth::ProjectSessionBinding::from(&original);

    let mut changed = original.clone();
    changed.membership_epoch += 1;
    assert!(labby_auth::ProjectSessionBinding::from(&changed) != expected);
    changed = original.clone();
    changed.route_generation += 1;
    assert!(labby_auth::ProjectSessionBinding::from(&changed) != expected);
    changed = original.clone();
    changed.scopes.push("lab:admin".into());
    assert!(labby_auth::ProjectSessionBinding::from(&changed) != expected);
    changed = original.clone();
    changed.resource.push_str("/other");
    assert!(labby_auth::ProjectSessionBinding::from(&changed) != expected);
}
