//! HTTP authorization regression tests. All durable state belongs to the fixture.
use super::*;
use axum::{Router, body::Body, http::Request};
use labby_auth::{Authenticator, VerifiedIdentity};
use std::sync::Arc;
use tower::ServiceExt as _;

struct Fixture {
    directory: tempfile::TempDir,
    state: AppState,
    identity: VerifiedIdentity,
}

impl Fixture {
    async fn new() -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = directory.path().canonicalize().unwrap();
        let runtime =
            Arc::new(crate::access::AccessRuntime::initialize(root.join("access.db")).await);
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "tailcat-http-owner",
        )
        .unwrap();
        runtime
            .bootstrap_owner(
                crate::access::BootstrapOwnerInput::new(identity.clone(), "Local", "Default")
                    .unwrap(),
            )
            .await
            .unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                root.join("gateway.toml"),
                Default::default(),
            ),
        );
        let mut state = AppState::new()
            .with_access_runtime(runtime)
            .with_gateway_manager(manager);
        state.installation_id = Some(Arc::from("tailcat-http-installation"));
        Self {
            directory,
            state,
            identity,
        }
    }

    fn snapshot(&self) -> (Vec<String>, (i64, i64)) {
        let mut files = std::fs::read_dir(self.directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        files.sort();
        let connection =
            rusqlite::Connection::open(self.directory.path().join("access.db")).unwrap();
        let counts = connection.query_row(
            "SELECT (SELECT count(*) FROM project_credentials),(SELECT count(*) FROM credential_idempotency)",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        (files, counts)
    }

    async fn deny(&self, action: &str, case: Denial) {
        let before = self.snapshot();
        let mut auth = AuthContext {
            sub: "tailcat-http-owner".into(),
            actor_key: None,
            issuer: "browser-session".into(),
            scopes: vec!["lab:admin".into()],
            via_session: true,
            csrf_token: Some("fixture-csrf".into()),
            email: Some("owner@example.com".into()),
        };
        let mut router = Router::new()
            .route("/v1/setup", post(handle))
            .with_state(self.state.clone());
        if !matches!(case, Denial::MissingIdentity) {
            let identity = if matches!(case, Denial::StaticBearer) {
                auth.via_session = false;
                VerifiedIdentity::local_credential(Authenticator::StaticBearer, "fixture-static")
                    .unwrap()
            } else {
                self.identity.clone()
            };
            router = router.layer(Extension(identity));
        }
        if matches!(case, Denial::ReadOnly) {
            auth.scopes = vec!["lab:read".into()];
        }
        router = router.layer(Extension(auth));
        if !matches!(case, Denial::Delegated) {
            router = router.layer(Extension(ConnectInfo(
                "127.0.0.1:1234".parse::<SocketAddr>().unwrap(),
            )));
        }
        let params = if action == "tailcat.enable" {
            serde_json::json!({"project_id":"bootstrap-default","credential_id":"fixture-unissued-credential"})
        } else if action == "tailcat.enroll" {
            serde_json::json!({"project_id":"bootstrap-default","idempotency_key":"http-denial-operation-0001"})
        } else {
            serde_json::json!({"project_id":"bootstrap-default","public_resource":"https://labby.example/sandbox",
                "derp_map_url":"https://tailcat.example/derpmap.json","node_path":"/unused/node"})
        };
        let mut request = Request::builder()
            .method("POST")
            .uri("/v1/setup")
            .header("host", "localhost:8765")
            .header("content-type", "application/json");
        if !matches!(case, Denial::MissingCsrf) {
            request = request.header(
                "x-csrf-token",
                if matches!(case, Denial::WrongCsrf) {
                    "wrong"
                } else {
                    "fixture-csrf"
                },
            );
        }
        if matches!(case, Denial::Team) {
            request = request.header("x-labby-team-id", "foreign-team");
        }
        let response = router
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
        let body = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        assert_eq!(
            status,
            axum::http::StatusCode::FORBIDDEN,
            "{action} {case:?}: {}",
            String::from_utf8_lossy(&body)
        );
        assert_eq!(
            self.snapshot(),
            before,
            "{action} {case:?} mutated fixture state"
        );
    }
}

#[derive(Clone, Copy, Debug)]
enum Denial {
    MissingIdentity,
    StaticBearer,
    MissingCsrf,
    WrongCsrf,
    Delegated,
    Team,
    ReadOnly,
    RevokedOwner,
}

#[tokio::test]
async fn tailcat_setup_http_denies_untrusted_context_before_mutation() {
    let fixture = Fixture::new().await;
    for action in ["tailcat.configure", "tailcat.enroll", "tailcat.enable"] {
        for case in [
            Denial::MissingIdentity,
            Denial::StaticBearer,
            Denial::MissingCsrf,
            Denial::WrongCsrf,
            Denial::Delegated,
            Denial::Team,
            Denial::ReadOnly,
        ] {
            fixture.deny(action, case).await;
        }
    }
}

#[tokio::test]
async fn revoked_owner_http_session_cannot_configure_or_enroll() {
    let fixture = Fixture::new().await;
    // Revoke durable authority while retaining the already authenticated session.
    let connection =
        rusqlite::Connection::open(fixture.directory.path().join("access.db")).unwrap();
    assert_eq!(connection.execute("UPDATE project_memberships SET status='disabled' WHERE project_id='bootstrap-default'", []).unwrap(), 1);
    drop(connection);
    for action in ["tailcat.configure", "tailcat.enroll", "tailcat.enable"] {
        fixture.deny(action, Denial::RevokedOwner).await;
    }
}
