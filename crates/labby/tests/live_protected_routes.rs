#![allow(clippy::panic, dead_code)]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_identity.rs"]
mod live_identity;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/state_snapshot.rs"]
mod state_snapshot;

mod support {
    pub(crate) use crate::live_labby::{
        CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, isolated_command,
    };
}

use reqwest::StatusCode;
use serde_json::json;
use state_snapshot::{NarrowStorageObservation, OwnedProcessObservation};

const HIDDEN_FAMILIES: &[(&str, &str)] = &[
    ("gateway.list", "/v1/gateway"),
    ("gateway.get", "/v1/gateway"),
    ("gateway.test", "/v1/gateway"),
    ("gateway.update", "/v1/gateway"),
    ("gateway.remove", "/v1/gateway"),
    ("gateway.lifecycle", "/v1/gateway"),
    ("gateway.oauth.status", "/v1/gateway/oauth/status"),
    ("gateway.import_pending.list", "/v1/gateway"),
    ("gateway.import_tombstones.list", "/v1/gateway"),
];

async fn hidden_batch(
    base: &str,
    token: Option<&str>,
) -> Result<Vec<(String, StatusCode, usize)>, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())?;
    let mut outcomes = Vec::new();
    for (action, path) in HIDDEN_FAMILIES {
        let mut request = client
            .post(format!("{base}{path}"))
            .header("content-type", "application/json")
            .json(&json!({"action": action, "params": {"name":"not-owned"}}));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), request.send())
            .await
            .map_err(|_| format!("{action} timed out"))?
            .map_err(|e| e.to_string())?;
        let status = response.status();
        let body = response.bytes().await.map_err(|e| e.to_string())?;
        outcomes.push(((*action).to_owned(), status, body.len()));
    }
    Ok(outcomes)
}

#[tokio::test]
async fn missing_malformed_and_foreign_project_denials_are_non_enumerating_and_side_effect_free() {
    let first = live_identity::LiveIdentity::bootstrap("protected-subject-a")
        .await
        .expect("first identity");
    let foreign = live_identity::LiveIdentity::bootstrap("protected-subject-b")
        .await
        .expect("foreign identity");
    assert_ne!(first.identity.subject, foreign.identity.subject);
    assert_ne!(
        first.identity.credential_id, foreign.identity.credential_id,
        "independent subjects shared a credential"
    );

    let process_before = OwnedProcessObservation::read(first.root()).unwrap();
    let storage_before = NarrowStorageObservation::read(
        &first.root().join("labby-home"),
        &["config.toml", "access.sqlite"],
    )
    .unwrap();
    let (missing, malformed, wrong_project) = tokio::join!(
        hidden_batch(first.base(), None),
        hidden_batch(first.base(), Some("not-a-labby-credential")),
        hidden_batch(first.base(), Some(foreign.credential_for_request())),
    );
    for batch in [missing.unwrap(), malformed.unwrap(), wrong_project.unwrap()] {
        assert_eq!(batch.len(), HIDDEN_FAMILIES.len());
        for (action, status, body_len) in batch {
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{action} disclosed state");
            assert!(body_len < 64 * 1024, "{action} denial was unbounded");
        }
    }
    assert_eq!(
        OwnedProcessObservation::read(first.root()).unwrap(),
        process_before
    );
    assert_eq!(
        NarrowStorageObservation::read(
            &first.root().join("labby-home"),
            &["config.toml", "access.sqlite"]
        )
        .unwrap(),
        storage_before,
        "hidden denials mutated public configuration or access state"
    );
    assert_eq!(
        first.protected_mcp_initialize().await.unwrap(),
        StatusCode::OK,
        "allowed route stopped working after denial batch"
    );

    let first_cleanup = first.cleanup().await.expect("first cleanup");
    let foreign_cleanup = foreign.cleanup().await.expect("foreign cleanup");
    assert!(first_cleanup.is_clean(), "{:?}", first_cleanup.failures);
    assert!(foreign_cleanup.is_clean(), "{:?}", foreign_cleanup.failures);
}

#[tokio::test]
async fn revocation_during_discovery_invalidates_stale_api_mcp_and_browser_clients() {
    let mut identity = live_identity::LiveIdentity::bootstrap("revocation-race-subject")
        .await
        .expect("identity");
    identity.create_session().await.expect("session");
    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::OK
    );
    let csrf = identity.session.as_ref().unwrap().csrf.clone();
    let (_, revoke_status) = tokio::join!(identity.bearer_catalog_response(), identity.revoke());
    assert_eq!(revoke_status.unwrap(), StatusCode::OK);

    assert_eq!(
        identity.introspect().await.unwrap().0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        identity.browser_catalog(Some(&csrf)).await.unwrap(),
        StatusCode::UNAUTHORIZED,
        "credential revocation left its browser session active"
    );
    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::UNAUTHORIZED,
        "stale MCP credential remained authorized after revocation"
    );

    let cleanup = identity.cleanup().await.expect("retryable cleanup");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn same_raw_subject_is_isolated_by_two_issuers_projects_and_disjoint_loadouts() {
    let subject = "same-raw-parity-subject";
    let first = live_identity::LiveIdentity::bootstrap_with_binding(
        subject,
        "issuer-a.example.test",
        "loadout-a",
    )
    .await
    .expect("issuer A bootstrap");
    let second = live_identity::LiveIdentity::bootstrap_with_binding(
        subject,
        "issuer-b.example.test",
        "loadout-b",
    )
    .await
    .expect("issuer B bootstrap");
    assert_eq!(first.identity.subject, second.identity.subject);
    assert_ne!(first.identity.issuer, second.identity.issuer);
    assert_ne!(first.identity.loadout_id, second.identity.loadout_id);
    assert_ne!(
        (&first.identity.issuer, &first.identity.project_id),
        (&second.identity.issuer, &second.identity.project_id),
        "issuer-scoped project identities collapsed"
    );
    assert_eq!(
        first
            .protected_mcp_initialize_with(second.credential_for_request())
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED,
        "credential crossed installation/issuer/project boundary"
    );
    assert_eq!(
        second
            .protected_mcp_initialize_with(first.credential_for_request())
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED,
        "reverse credential crossing was accepted"
    );

    let first_cleanup = first.cleanup().await.expect("first cleanup");
    let second_cleanup = second.cleanup().await.expect("second cleanup");
    assert!(first_cleanup.is_clean(), "{:?}", first_cleanup.failures);
    assert!(second_cleanup.is_clean(), "{:?}", second_cleanup.failures);
}

#[tokio::test]
async fn legitimate_narrowing_and_policy_restart_prevent_older_broader_republication() {
    let mut identity = live_identity::LiveIdentity::bootstrap_with_scopes(
        "narrow-race-subject",
        &["lab:admin", "lab:read"],
    )
    .await
    .expect("identity");
    let (narrow_id, narrow_token) = identity
        .issue_narrower_credential(&["lab:read"])
        .await
        .expect("narrow child credential");
    assert_eq!(
        identity.introspect_token(&narrow_token).await.unwrap(),
        StatusCode::OK
    );
    assert_eq!(
        identity
            .protected_mcp_initialize_with(&narrow_token)
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED,
        "narrowed credential amplified to route scope"
    );

    let base = identity.base().to_owned();
    let source = identity.credential_for_request().to_owned();
    let facts = identity.identity.clone();
    let older_completion = tokio::spawn(async move {
        live_identity::LiveIdentity::issue_narrower_at(
            &base,
            &source,
            &facts,
            &["lab:read".to_owned()],
        )
        .await
    });
    identity
        .replace_policy_and_restart(&live_identity::policy(&["lab:admin"]))
        .await
        .expect("narrowing policy restart");
    let older = older_completion.await.expect("issue task joined");
    if let Ok((older_id, older_token)) = older {
        assert_eq!(
            identity.introspect_token(&older_token).await.unwrap(),
            StatusCode::UNAUTHORIZED,
            "older completion republished broader pre-restart authority"
        );
        assert_eq!(
            identity.revoke_id(&older_id).await.unwrap(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        identity.introspect_token(&narrow_token).await.unwrap(),
        StatusCode::UNAUTHORIZED,
        "assignment/policy generation change left a child token reusable"
    );
    assert_eq!(
        identity.introspect().await.unwrap().0,
        StatusCode::UNAUTHORIZED,
        "source token remained reusable after policy reassignment"
    );
    // Cleanup is proof-authorized and remains retryable even after all product
    // credentials were invalidated by reassignment.
    drop(narrow_id);
    let cleanup = identity.cleanup().await.expect("proof cleanup");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}
