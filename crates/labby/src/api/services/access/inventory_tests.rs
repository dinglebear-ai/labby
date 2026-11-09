use super::*;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

#[tokio::test]
async fn invitation_expiry_http_preserves_committed_policy_risk_and_repeat_denial() {
    use sha2::{Digest, Sha256};
    use tracing_subscriber::prelude::*;

    const CHILD_ENV: &str = "LABBY_TEST_INVITATION_EXPIRY_LOG_CHILD";
    const TEST_NAME: &str = "api::services::access::inventory_tests::invitation_expiry_http_preserves_committed_policy_risk_and_repeat_denial";

    // Match the OAuth relay log tests' process isolation for tracing's global callsite cache.
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            tokio::process::Command::new(std::env::current_exe().unwrap())
                .kill_on_drop(true)
                .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=1"])
                .env(CHILD_ENV, "1")
                .output(),
        )
        .await
        .expect("invitation expiry HTTP child exceeded its deadline")
        .unwrap();
        assert!(
            output.status.success(),
            "isolated invitation expiry HTTP test failed. stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed;"));
        return;
    }

    let _lock = crate::test_support::TRACING_TEST_LOCK.lock().unwrap();
    let logs = crate::test_support::SharedBuf::default();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("labby=info"))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_ansi(false)
                .without_time()
                .with_writer(logs.clone()),
        );
    let _subscriber = tracing::subscriber::set_default(subscriber);
    crate::test_support::rebuild_tracing_interest_cache();

    let (_directory, store, app) = fixture().await;
    let token = hex::encode([42_u8; 32]);
    let digest = hex::encode(Sha256::digest([42_u8; 32]));
    let (status, created) = request(
        &app,
        "access.team_invitation.create",
        serde_json::json!({
            "team_id":"bootstrap-initial-team", "principal_id":"bootstrap-owner",
            "role":"member", "token":token, "ttl_seconds":60
        }),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    crate::access::test_support::with_connection(&store, |connection| {
        connection
            .execute(
                "UPDATE team_invitations SET created_at=0,expires_at=1,updated_at=0",
                [],
            )
            .map_err(crate::access::test_support::sqlite_error)?;
        Ok(())
    })
    .await
    .unwrap();
    let (status, first) = request(
        &app,
        "access.team.member.list",
        serde_json::json!({"team_id":"bootstrap-initial-team", "limit":1}),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let mut previous = None;
    for risk in ["possible", "none_expected"] {
        let (status, body) = request(
            &app,
            "access.team_invitation.accept",
            serde_json::json!({"token":token}),
            None,
            true,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["kind"], "forbidden");
        assert_eq!(body["message"], "access denied");
        assert_eq!(body["contract_version"], 1);
        assert_eq!(body["origin"], "policy");
        assert_eq!(body["side_effects"], risk);
        assert_eq!(body["service"], "access");
        assert_eq!(body["action"], "access.team_invitation.accept");
        if risk == "possible" {
            assert_eq!(body["recovery"]["action"], "do_not_retry");
            assert_eq!(body["recovery"]["same_arguments"], "discouraged");
        }
        for value in [&created, &body] {
            let serialized = value.to_string();
            assert!(!serialized.contains(&token));
            assert!(!serialized.contains(&digest));
            assert!(!serialized.contains("InvitationExpiryCommitted"));
        }
        let state = crate::access::test_support::with_connection(&store, |connection| {
            connection
                .query_row(
                    "SELECT (SELECT global_revision FROM access_metadata),
                (SELECT count(*) FROM access_audit WHERE action='access.team_invitation.expire'),
                (SELECT count(*) FROM authority_projection_outbox)",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .map_err(crate::access::test_support::sqlite_error)
        })
        .await
        .unwrap();
        assert_eq!(state.1, 1);
        if let Some(previous) = previous {
            assert_eq!(state, previous);
        }
        previous = Some(state);
    }
    let (status, stale) = request(
        &app,
        "access.team.member.list",
        serde_json::json!({"team_id":"bootstrap-initial-team", "limit":1,
            "offset":1, "snapshot_revision":first["global_revision"]}),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    assert_eq!(stale["kind"], "conflict");
    assert_eq!(stale["existing_id"], "access_inventory");
    assert_eq!(stale["side_effects"], "none_expected");
    for (token, status, kind) in [
        (hex::encode([99_u8; 32]), StatusCode::FORBIDDEN, "forbidden"),
        (
            "bad-token".into(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_param",
        ),
        (
            hex::encode([0_u8; 32]),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_param",
        ),
    ] {
        let (observed, body) = request(
            &app,
            "access.team_invitation.accept",
            serde_json::json!({"token":token}),
            None,
            true,
        )
        .await;
        assert_eq!(observed, status, "{body}");
        assert_eq!(body["kind"], kind);
        assert_eq!(body["side_effects"], "none_expected");
    }
    let captured = crate::test_support::captured_logs(&logs);
    assert!(captured.contains("access.team_invitation.accept"));
    assert!(!captured.contains(&token));
    assert!(!captured.contains(&digest));
}

fn auth() -> AuthContext {
    AuthContext {
        sub: "owner-subject".into(),
        actor_key: None,
        scopes: vec!["lab:admin".into()],
        issuer: "test".into(),
        via_session: true,
        csrf_token: Some("csrf".into()),
        email: None,
    }
}

fn app(mut state: AppState, identity: VerifiedIdentity) -> Router {
    state.installation_id = Some("local-installation".into());
    Router::new()
        .route("/v1/access/admin", post(handle))
        .with_state(state)
        .layer(Extension(auth()))
        .layer(Extension(identity))
}

async fn fixture() -> (tempfile::TempDir, crate::access::AccessStore, Router) {
    let (directory, _store, identity) = crate::access::test_support::fixture().await;
    let runtime = std::sync::Arc::new(
        crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await,
    );
    let store = runtime.store().await.unwrap();
    let router = app(AppState::new().with_access_runtime(runtime), identity);
    (directory, store, router)
}

async fn request(
    app: &Router,
    action: &str,
    params: Value,
    budget: Option<&str>,
    csrf: bool,
) -> (StatusCode, Value) {
    let mut request = Request::post("/v1/access/admin").header("content-type", "application/json");
    if let Some(budget) = budget {
        request = request.header("x-labby-access-read-budget-ms", budget);
    }
    if csrf {
        request = request.header("x-csrf-token", "csrf");
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(
                    serde_json::json!({"action":action,"params":params}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    (status, body)
}

#[tokio::test]
async fn inventory_http_reads_real_persisted_roster_and_assignments() {
    let (_directory, _store, app) = fixture().await;
    for (action, field) in [
        ("access.team.member.list", "members"),
        ("access.team_project.list", "assignments"),
    ] {
        let (status, body) = request(
            &app,
            action,
            serde_json::json!({"team_id":"bootstrap-initial-team"}),
            None,
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body[field].is_array(), "{body}");
        assert_eq!(body["team_status"], "active");
        if field == "members" {
            assert_eq!(body[field][0]["principal_id"], "bootstrap-owner");
        }
    }
}

#[tokio::test]
async fn inventory_http_invalid_header_precedes_unavailable_store_and_csrf_remains_required() {
    let app = app(
        AppState::new(),
        crate::access::test_support::browser("owner-subject"),
    );
    for (action, budget, csrf, expected, kind) in [
        (
            "access.team.member.list",
            "0",
            true,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_param",
        ),
        (
            "access.team_project.list",
            "bad",
            true,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_param",
        ),
        (
            "access.team.member.list",
            "1500",
            false,
            StatusCode::FORBIDDEN,
            "forbidden",
        ),
        (
            "access.team.member.list",
            "1500",
            true,
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
        ),
        (
            "access.team.create",
            "bad",
            true,
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
        ),
    ] {
        let (status, body) = request(
            &app,
            action,
            serde_json::json!({"team_id":"bootstrap-initial-team"}),
            Some(budget),
            csrf,
        )
        .await;
        assert_eq!(status, expected, "{body}");
        assert_eq!(body["kind"], kind, "{body}");
        assert_eq!(body["service"], "access");
        assert_eq!(body["action"], action);
    }
}

#[tokio::test]
async fn inventory_http_revision_budget_and_denial_preserve_typed_read_contract() {
    let (_directory, store, app) = fixture().await;
    let (_, first) = request(
        &app,
        "access.team.member.list",
        serde_json::json!({"team_id":"bootstrap-initial-team"}),
        None,
        true,
    )
    .await;
    store
        .create_team(
            crate::access::CreateTeamInput::new(
                crate::access::test_support::browser("owner-subject"),
                "new-team",
                "New",
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let (status, error) = request(
        &app,
        "access.team.member.list",
        serde_json::json!({
            "team_id":"bootstrap-initial-team",
            "offset":1,
            "snapshot_revision":first["global_revision"]
        }),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["existing_id"], "access_inventory");
    assert_eq!(error["side_effects"], "none_expected");
    let (status, denied) = request(
        &app,
        "access.team_project.list",
        serde_json::json!({"team_id":"absent"}),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    assert_eq!(denied["kind"], "forbidden");
    crate::access::test_support::with_connection(&store, |connection| {
        let principal = "historical-secret-like-value".repeat(4000);
        connection.execute(
            "INSERT INTO principals(principal_id,organization_id,kind,status,created_at,updated_at)
             VALUES(?1,'bootstrap-local','user','active',1,1)",
            [&principal],
        ).map_err(crate::access::test_support::sqlite_error)?;
        connection
            .execute(
                "INSERT INTO team_memberships
             VALUES(?1,'bootstrap-local','bootstrap-initial-team',?1,'member','revoked',3,
             'bootstrap-owner',1,1,1)",
                [&principal],
            )
            .map_err(crate::access::test_support::sqlite_error)?;
        Ok(())
    })
    .await
    .unwrap();
    let (_, first) = request(
        &app,
        "access.team.member.list",
        serde_json::json!({"team_id":"bootstrap-initial-team","limit":1}),
        None,
        true,
    )
    .await;
    let (status, error) = request(
        &app,
        "access.team.member.list",
        serde_json::json!({
            "team_id":"bootstrap-initial-team",
            "offset":1,
            "limit":1,
            "snapshot_revision":first["global_revision"]
        }),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{error}");
    assert_eq!(error["kind"], "response_too_large");
    assert_eq!(error["budget_terminal"], true);
    assert_eq!(error["side_effects"], "none_expected");
    assert!(!error.to_string().contains("historical-secret"));
}

#[tokio::test]
async fn inventory_http_shortened_deadline_covers_admission_and_next_request_succeeds() {
    let (_directory, store, app) = fixture().await;
    let (started, observed_start) = tokio::sync::oneshot::channel();
    let (release, wait_release) = std::sync::mpsc::sync_channel(0);
    let blocker = tokio::spawn(async move {
        crate::access::test_support::with_connection(&store, move |_| {
            started.send(()).unwrap();
            wait_release
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    });
    observed_start.await.unwrap();
    let (status, error) = request(
        &app,
        "access.team.member.list",
        serde_json::json!({"team_id":"bootstrap-initial-team"}),
        Some("5"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{error}");
    assert_eq!(error["kind"], "timeout");
    assert_eq!(error["side_effects"], "none_expected");
    release.send(()).unwrap();
    blocker.await.unwrap();
    let (status, body) = request(
        &app,
        "access.team.member.list",
        serde_json::json!({"team_id":"bootstrap-initial-team"}),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn inventory_http_unbound_installation_is_read_only_unavailable() {
    let (directory, _store, identity) = crate::access::test_support::fixture().await;
    let runtime = std::sync::Arc::new(
        crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await,
    );
    let state = AppState::new().with_access_runtime(runtime);
    let app = Router::new()
        .route("/v1/access/admin", post(handle))
        .with_state(state)
        .layer(Extension(auth()))
        .layer(Extension(identity));
    let (status, error) = request(
        &app,
        "access.team.member.list",
        serde_json::json!({"team_id":"bootstrap-initial-team"}),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["kind"], "service_unavailable");
    assert_eq!(error["side_effects"], "none_expected");
}

#[tokio::test]
async fn secondary_inventory_http_actions_use_real_shared_store() {
    let (_directory, _store, app) = fixture().await;
    for (action, params, field) in [
        (
            "access.team_invitation.list",
            serde_json::json!({"team_id":"bootstrap-initial-team"}),
            "invitations",
        ),
        (
            "access.platform_admin.list",
            serde_json::json!({}),
            "administrators",
        ),
    ] {
        let (status, body) = request(&app, action, params, None, true).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body[field].is_array(), "{body}");
        assert!(body["global_revision"].as_u64().is_some());
    }
}

#[tokio::test]
async fn secondary_inventory_http_deadlines_cleanup_and_csrf_follow_existing_boundary() {
    let (_directory, store, app) = fixture().await;
    for (action, params) in [
        (
            "access.team_invitation.list",
            serde_json::json!({"team_id":"bootstrap-initial-team"}),
        ),
        ("access.platform_admin.list", serde_json::json!({})),
    ] {
        let (status, error) = request(&app, action, params.clone(), Some("0"), true).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
        assert_eq!(error["kind"], "invalid_param");
        let (status, _) = request(&app, action, params.clone(), None, false).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (started, observed_start) = tokio::sync::oneshot::channel();
        let (release, wait_release) = std::sync::mpsc::sync_channel(0);
        let held_store = store.clone();
        let blocker = tokio::spawn(async move {
            crate::access::test_support::with_connection(&held_store, move |_| {
                started.send(()).unwrap();
                wait_release
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
                Ok(())
            })
            .await
            .unwrap();
        });
        observed_start.await.unwrap();
        let (status, error) = request(&app, action, params.clone(), Some("5"), true).await;
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{error}");
        assert_eq!(error["side_effects"], "none_expected");
        release.send(()).unwrap();
        blocker.await.unwrap();
        let (status, body) = request(&app, action, params, None, true).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
}

#[tokio::test]
async fn lifecycle_http_actions_preserve_csrf_and_persist_revoked_assignment() {
    let (_directory, store, app) = fixture().await;
    let (status, body) = request(
        &app,
        "access.team.create",
        serde_json::json!({"team_id":"http-lifecycle","name":"HTTP lifecycle"}),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let params = serde_json::json!({"team_id":"http-lifecycle","project_id":"bootstrap-default"});
    let (status,body)=request(&app,"access.team_project.assign",serde_json::json!({"team_id":"http-lifecycle","project_id":"bootstrap-default","role":"admin"}),None,true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for action in [
        "access.team.member.activate",
        "access.team_project.unassign",
    ] {
        let (status,body)=request(&app,action,serde_json::json!({"team_id":"http-lifecycle","project_id":"bootstrap-default","principal_id":"bootstrap-owner","role":"owner"}),None,false).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["kind"], "forbidden");
        assert_eq!(body["side_effects"], "none_expected");
    }
    let (status, body) = request(
        &app,
        "access.team_project.unassign",
        params.clone(),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "revoked");
    let (status, inventory) = request(
        &app,
        "access.team_project.list",
        serde_json::json!({"team_id":"http-lifecycle","limit":10}),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{inventory}");
    assert_eq!(inventory["assignments"][0]["status"], "revoked");
    let (status, body) = request(&app, "access.team_project.unassign", params, None, true).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["side_effects"], "none_expected");
    let (status,body)=request(&app,"access.team.member.activate",serde_json::json!({"team_id":"http-lifecycle","principal_id":"bootstrap-owner","role":"owner"}),None,true).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["kind"], "forbidden");
    let (_status, body) = request(
        &app,
        "schema",
        serde_json::json!({"action":"access.team.member.activate"}),
        None,
        true,
    )
    .await;
    assert_eq!(body["action"], "access.team.member.activate");
    assert_eq!(body["params"].as_array().unwrap().len(), 3);
    drop(store);
}
