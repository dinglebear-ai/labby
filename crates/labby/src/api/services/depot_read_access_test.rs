use super::*;
use crate::access::{AccessRuntime, AccessStore, BootstrapOwnerInput};
use crate::dispatch::depot::cursor::{Binding, CursorError, PageInput};
use std::sync::Arc;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};

async fn fixture() -> (
    tempfile::TempDir,
    AppState,
    AccessStore,
    BrowserAuthority,
    AuthContext,
    VerifiedIdentity,
) {
    let (directory, authority, auth, identity) =
        super::super::tests::browser_context(&["lab:read"]).await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = directory.path().canonicalize().unwrap().join("access.db");
    let store = AccessStore::open(path.clone()).await.unwrap();
    store
        .bootstrap_owner(BootstrapOwnerInput::new(identity.clone(), "Team", "Default").unwrap())
        .await
        .unwrap();
    store.execute_test_statement("INSERT INTO project_loadouts VALUES ('bootstrap-local','bootstrap-default','team','bootstrap-owner',2,2)").await.unwrap();
    let mut state =
        AppState::new().with_access_runtime(Arc::new(AccessRuntime::initialize(path).await));
    Arc::make_mut(&mut state.config).depot.read_project_id = Some("bootstrap-default".into());
    (directory, state, store, authority, auth, identity)
}

#[tokio::test]
async fn every_active_role_can_read_without_gaining_management_or_execution() {
    let (_directory, state, store, authority, auth, identity) = fixture().await;
    for statement in [
        "UPDATE project_memberships SET role='owner'",
        "UPDATE project_memberships SET role='admin'",
        "UPDATE project_memberships SET role='member'",
        "UPDATE project_memberships SET role='viewer'",
    ] {
        store.execute_test_statement(statement).await.unwrap();
        let access = ReadAccess::begin(&state, &authority, Some(&auth), Some(&identity))
            .await
            .unwrap();
        assert!(access.epoch().is_some());
        access
            .finish(&state, &authority, Some(&auth), Some(&identity), Ok(()))
            .await
            .unwrap();
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
}

#[tokio::test]
async fn outsider_is_denied_on_all_reads_before_any_upstream_request() {
    let (_directory, mut state, _store, _authority, _auth, _identity) = fixture().await;
    let (_other_directory, authority, auth, identity) =
        super::super::tests::browser_context_for_subject(&["lab:read"], "outsider").await;
    let upstream = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&upstream)
        .await;
    state.depot = crate::dispatch::depot::DepotClient::for_test(
        url::Url::parse(&upstream.uri()).unwrap(),
        "private-team-read",
    )
    .into();
    let a = Some(Extension(auth.clone()));
    let i = Some(Extension(identity.clone()));
    for result in [
        status(
            State(state.clone()),
            Extension(authority.clone()),
            a.clone(),
            i.clone(),
        )
        .await,
        session(
            State(state.clone()),
            Extension(authority.clone()),
            a.clone(),
            i.clone(),
        )
        .await,
        operations(
            State(state.clone()),
            Extension(authority.clone()),
            a.clone(),
            i.clone(),
        )
        .await,
        providers(
            State(state.clone()),
            Extension(authority.clone()),
            a.clone(),
            i.clone(),
        )
        .await,
        discover(
            State(state.clone()),
            Extension(authority.clone()),
            a.clone(),
            i.clone(),
            Json(DiscoveryRequest {
                provider: None,
                query: String::new(),
                limit: 10,
                cursor: None,
            }),
        )
        .await,
        detail(
            State(state.clone()),
            Extension(authority.clone()),
            a.clone(),
            i.clone(),
            Json(DetailRequest {
                provider_id: "team".into(),
                artifact_id: "private-artifact".into(),
            }),
        )
        .await,
        call(
            State(state.clone()),
            Extension(authority.clone()),
            Extension(auth.clone()),
            i.clone(),
            HeaderMap::new(),
            Json(OperationRequest {
                operation: "depot.artifacts.list".into(),
                params: json!({}),
                destructive_intent: None,
            }),
        )
        .await,
    ] {
        assert_eq!(result.unwrap_err().0, StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn viewer_reads_upstream_but_revoked_or_changed_membership_cannot_release_data() {
    let (_directory, mut state, store, authority, auth, identity) = fixture().await;
    store
        .execute_test_statement("UPDATE project_memberships SET role='viewer'")
        .await
        .unwrap();
    let upstream = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"operations":[]})))
        .expect(1)
        .mount(&upstream)
        .await;
    state.depot = crate::dispatch::depot::DepotClient::for_test(
        url::Url::parse(&upstream.uri()).unwrap(),
        "private-team-read",
    )
    .into();
    let response = operations(
        State(state.clone()),
        Extension(authority.clone()),
        Some(Extension(auth.clone())),
        Some(Extension(identity.clone())),
    )
    .await
    .unwrap();
    assert_eq!(response.0["operations"], json!([]));
    let access = ReadAccess::begin(&state, &authority, Some(&auth), Some(&identity))
        .await
        .unwrap();
    store
        .execute_test_statement("UPDATE project_memberships SET updated_at=updated_at+1")
        .await
        .unwrap();
    assert!(
        access
            .finish(
                &state,
                &authority,
                Some(&auth),
                Some(&identity),
                Ok(json!({"private":"data"}))
            )
            .await
            .is_err()
    );
    for statement in [
        "UPDATE project_memberships SET status='disabled'",
        "UPDATE project_memberships SET status='active'; UPDATE principal_links SET status='revoked'",
    ] {
        store.execute_test_statement(statement).await.unwrap();
        assert!(
            ReadAccess::begin(&state, &authority, Some(&auth), Some(&identity))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn cached_private_cursor_is_denied_after_revocation_and_invalid_after_regrant() {
    let (_directory, state, store, authority, auth, identity) = fixture().await;
    let access = ReadAccess::begin(&state, &authority, Some(&auth), Some(&identity))
        .await
        .unwrap();
    let mut binding = Binding::for_browser(
        &authority,
        "lab:read",
        "all".into(),
        String::new(),
        "discovery/v1:10".into(),
        "registry".into(),
        vec![],
    )
    .await
    .unwrap();
    binding.bind_access_epoch(access.epoch().as_deref());
    let now = std::time::Instant::now();
    let cursor = state
        .depot_manager
        .cursors
        .create(binding.clone(), vec![], now)
        .await
        .unwrap();
    let PageInput::Compute(lease) = state
        .depot_manager
        .cursors
        .begin(&cursor, &binding, now)
        .await
        .unwrap()
    else {
        panic!("expected new cursor")
    };
    lease
        .complete(br#"{"private":"cached"}"#.to_vec(), None, now)
        .await
        .unwrap();
    assert!(matches!(
        state
            .depot_manager
            .cursors
            .begin(&cursor, &binding, now)
            .await
            .unwrap(),
        PageInput::Replay(_)
    ));
    store
        .execute_test_statement(
            "UPDATE project_memberships SET status='disabled', updated_at=updated_at+1",
        )
        .await
        .unwrap();
    let response = discover(
        State(state.clone()),
        Extension(authority.clone()),
        Some(Extension(auth.clone())),
        Some(Extension(identity.clone())),
        Json(DiscoveryRequest {
            provider: None,
            query: String::new(),
            limit: 10,
            cursor: Some(cursor.clone()),
        }),
    )
    .await;
    assert_eq!(response.unwrap_err().0, StatusCode::FORBIDDEN);
    store
        .execute_test_statement(
            "UPDATE project_memberships SET status='active', updated_at=updated_at+1",
        )
        .await
        .unwrap();
    let restored = ReadAccess::begin(&state, &authority, Some(&auth), Some(&identity))
        .await
        .unwrap();
    let mut current = Binding::for_browser(
        &authority,
        "lab:read",
        "all".into(),
        String::new(),
        "discovery/v1:10".into(),
        "registry".into(),
        vec![],
    )
    .await
    .unwrap();
    current.bind_access_epoch(restored.epoch().as_deref());
    assert!(matches!(
        state
            .depot_manager
            .cursors
            .begin(&cursor, &current, now)
            .await,
        Err(CursorError::Expired)
    ));
}

#[tokio::test]
async fn unset_read_project_preserves_existing_instance_read_behavior() {
    let (_directory, authority, _auth, _identity) =
        super::super::tests::browser_context(&["lab:read"]).await;
    let state = AppState::new();
    let access = ReadAccess::begin(&state, &authority, None, None)
        .await
        .unwrap();
    assert!(access.epoch().is_none());
}

#[tokio::test]
async fn bound_public_search_and_detail_use_project_gate_and_protected_credential() {
    use wiremock::matchers::{header, path};
    let (_directory, mut state, store, authority, auth, identity) = fixture().await;
    let upstream = MockServer::start().await;
    let preferences: crate::config::depot::DepotPreferences = toml::from_str(&format!(
        r#"
read_project_id = "bootstrap-default"
[public_read_binding]
endpoint = "{}"
bearer_token_env = "LABBY_DEPOT_CATALOG_READ_TOKEN"
deployment_id = "catalog"
"#,
        upstream.uri()
    ))
    .unwrap();
    Arc::make_mut(&mut state.config).depot = preferences.clone();
    state.depot_manager = Arc::new(crate::dispatch::depot::manager::Manager::new(
        &preferences,
        crate::dispatch::depot::manager::SecretSnapshot::from_values(
            std::collections::BTreeMap::from([(
                "LABBY_DEPOT_CATALOG_READ_TOKEN".into(),
                "protected-read".into(),
            )]),
        ),
        Default::default(),
    ));
    let metadata = json!({"contractVersion":"depot.discovery/v1","deploymentId":"catalog","deploymentEpoch":"boot","authorityEpoch":"private-read","listingEpoch":"1","snapshotContinuations":true,"maxPageSize":200});
    Mock::given(path("/api/discovery"))
        .and(header("authorization", "Bearer protected-read"))
        .respond_with(ResponseTemplate::new(200).set_body_json(metadata.clone()))
        .expect(1)
        .mount(&upstream)
        .await;
    let mut listing = metadata.clone();
    listing["result"] = json!({"artifacts":[{"id":"private-skill","kind":"skill","name":"Python","currentRevisionId":"revision-exact"}],"total":1});
    Mock::given(path("/api/discovery/list"))
        .and(header("authorization", "Bearer protected-read"))
        .respond_with(ResponseTemplate::new(200).set_body_json(listing))
        .expect(1)
        .mount(&upstream)
        .await;
    let mut artifact = metadata;
    artifact["result"] = json!({"artifact":{"descriptor":{"id":"private-skill","kind":"skill","name":"Python"},"currentRevisionId":"revision-exact","currentRevision":{"id":"revision-exact","contentDigest":"sha256:known"}}});
    Mock::given(path("/api/discovery/get"))
        .and(header("authorization", "Bearer protected-read"))
        .respond_with(ResponseTemplate::new(200).set_body_json(artifact))
        .expect(1)
        .mount(&upstream)
        .await;
    let request = || {
        serde_json::from_value(json!({"provider":"public","query":"python","limit":20})).unwrap()
    };
    assert!(
        discover(
            State(state.clone()),
            Extension(authority.clone()),
            None,
            None,
            Json(request())
        )
        .await
        .is_err()
    );
    let result = discover(
        State(state.clone()),
        Extension(authority.clone()),
        Some(Extension(auth.clone())),
        Some(Extension(identity.clone())),
        Json(request()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(result["items"][0]["providerId"], "public");
    assert_eq!(result["items"][0]["currentRevisionId"], "revision-exact");
    assert!(!result.to_string().contains("protected-read"));
    let result = detail(
        State(state.clone()),
        Extension(authority.clone()),
        Some(Extension(auth.clone())),
        Some(Extension(identity.clone())),
        Json(DetailRequest {
            provider_id: "public".into(),
            artifact_id: "private-skill".into(),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(result["artifact"]["currentRevisionId"], "revision-exact");
    store
        .execute_test_statement("UPDATE project_memberships SET status='disabled'")
        .await
        .unwrap();
    assert!(
        discover(
            State(state.clone()),
            Extension(authority.clone()),
            Some(Extension(auth)),
            Some(Extension(identity)),
            Json(request())
        )
        .await
        .is_err()
    );
    Arc::make_mut(&mut state.config).depot.read_project_id = None;
    assert!(
        discover(
            State(state),
            Extension(authority),
            None,
            None,
            Json(request())
        )
        .await
        .is_err()
    );
    upstream.verify().await;
}
