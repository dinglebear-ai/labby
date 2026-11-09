use super::*;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

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
