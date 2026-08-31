#![cfg(feature = "gateway")]

// Integration-test support modules are compiled independently per test binary;
// this binary intentionally exercises only a subset of the shared fixture API.
#[allow(dead_code)]
#[path = "support/live_identity.rs"]
mod live_identity;
#[allow(dead_code)]
#[path = "support/mcp_action_runner.rs"]
mod mcp_action_runner;
#[path = "support/lib.rs"]
mod support;

use axum::http::StatusCode;
use live_identity::{
    LOADOUT_ID, LiveIdentity, PROJECT_ID, RESOURCE, ROUTE_ID, policy, prepare, recover,
    scan_retained_evidence,
};
use mcp_action_runner::{BuiltinMcpRunner, IdentityTuple};

#[tokio::test]
async fn public_first_bootstrap_restart_session_and_cleanup_are_real_and_owned() {
    let mut identity = LiveIdentity::bootstrap("owner@example.test").await.unwrap();
    assert_eq!(identity.identity.project_id, PROJECT_ID);
    assert_eq!(identity.identity.subject, "owner@example.test");
    assert!(
        identity
            .identity
            .issuer
            .starts_with("urn:labby:local-operator:")
    );
    assert_eq!(identity.identity.loadout_id, LOADOUT_ID);
    assert_eq!(identity.identity.route_id, ROUTE_ID);
    assert_eq!(identity.identity.resource, RESOURCE);
    assert_eq!(identity.identity.audience, RESOURCE);
    assert_eq!(identity.identity.credential_generation, 1);
    assert!(identity.identity.expires_at > 0);
    assert_eq!(identity.identity.scopes, ["lab:read"]);
    assert_eq!(identity.introspect().await.unwrap().0, StatusCode::OK);
    assert_eq!(identity.repeat_consume().await.unwrap(), StatusCode::OK);
    assert_eq!(identity.owned_ids().len(), 2);
    assert!(identity.root().join("credential.txt").exists());
    identity.create_session().await.unwrap();
    let first_cookie = identity.session.as_ref().unwrap().cookie.clone();
    let bearer_catalog = identity.bearer_catalog_response().await.unwrap();
    let browser_catalog = identity.browser_catalog_response(None).await.unwrap();
    assert_eq!(bearer_catalog.0, StatusCode::OK);
    assert_eq!(browser_catalog.0, StatusCode::OK);
    assert_eq!(browser_catalog.1, bearer_catalog.1);
    let services = bearer_catalog.1["services"]
        .as_array()
        .expect("catalog services array");
    assert_eq!(
        services
            .iter()
            .filter_map(|service| service["name"].as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["gateway"]),
        "project-bound catalog must expose only the assigned Loadout services"
    );
    assert!(services.iter().all(|service| {
        service["actions"].as_array().is_none_or(|actions| {
            actions
                .iter()
                .all(|action| action["requires_admin"] != true)
        })
    }));
    let expected_tools = services
        .iter()
        .filter_map(|service| service["name"].as_str().map(str::to_owned))
        .collect::<std::collections::BTreeSet<_>>();
    let project_identity = IdentityTuple::from_public(&identity.identity);
    let expected_fingerprint = project_identity.fingerprint();
    let mcp = BuiltinMcpRunner::connect_project(
        identity.base(),
        identity.credential_for_request(),
        project_identity,
    )
    .await
    .unwrap();
    assert_eq!(mcp.identity_fingerprint(), expected_fingerprint);
    assert_eq!(mcp.list_tool_names().await.unwrap(), expected_tools);
    mcp.disconnect().await;
    identity.restart().await.unwrap();
    assert_eq!(identity.introspect().await.unwrap().0, StatusCode::OK);
    let csrf = identity.session.as_ref().unwrap().csrf.clone();
    assert_eq!(
        identity.logout(&csrf).await.unwrap(),
        StatusCode::NO_CONTENT
    );
    identity.create_session().await.unwrap();
    assert_ne!(identity.session.as_ref().unwrap().cookie, first_cookie);
    assert!(identity.cleanup().await.unwrap().is_clean());
}

#[tokio::test]
async fn public_tokens_sessions_and_cross_project_authority_fail_closed() {
    let mut identity = LiveIdentity::bootstrap("owner-negative@example.test")
        .await
        .unwrap();
    let credential = identity.credential_for_request().to_owned();
    let mut tampered = credential.clone();
    tampered.push('x');
    assert_eq!(
        identity.introspect_token(&tampered).await.unwrap(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        identity
            .introspect_token("lby_pc_v1_invalid_short")
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        identity
            .protected_mcp_initialize_with("lby_pc_v1_invalid_short")
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED,
        "reserved malformed product prefix must not fall through to OAuth"
    );
    identity.create_session().await.unwrap();
    assert_eq!(
        identity.browser_catalog(Some("wrong-csrf")).await.unwrap(),
        StatusCode::OK,
        "safe GET does not consume CSRF authority"
    );
    assert_eq!(
        identity.logout("wrong-csrf").await.unwrap(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let csrf = identity.session.as_ref().unwrap().csrf.clone();
    assert_eq!(
        identity.logout(&csrf).await.unwrap(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        identity.browser_catalog(None).await.unwrap(),
        StatusCode::UNAUTHORIZED
    );
    assert!(
        !identity
            .revoke_id("credential-owned-by-another-identity")
            .await
            .unwrap()
            .is_success(),
        "project credential gained non-browser administrative authority"
    );
    assert_eq!(identity.revoke().await.unwrap(), StatusCode::OK);
    assert_eq!(
        identity.introspect_token(&credential).await.unwrap(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        identity
            .protected_mcp_initialize_with(&credential)
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED
    );
    assert!(identity.cleanup().await.unwrap().is_clean());
}

#[tokio::test]
async fn protected_mcp_denies_wrong_project_scope_and_issuer_shaped_tokens() {
    let mut wrong_project = LiveIdentity::bootstrap("wrong-project@example.test")
        .await
        .unwrap();
    let project_policy = policy(&["lab:read"]).replace(
        "project_id = \"bootstrap-default\"",
        "project_id = \"different-project\"",
    );
    wrong_project
        .replace_policy_and_restart(&project_policy)
        .await
        .unwrap();
    assert_eq!(
        wrong_project.protected_mcp_initialize().await.unwrap(),
        StatusCode::UNAUTHORIZED
    );
    assert!(wrong_project.cleanup().await.unwrap().is_clean());

    let mut insufficient = LiveIdentity::bootstrap("insufficient@example.test")
        .await
        .unwrap();
    insufficient
        .replace_policy_and_restart(&policy(&["lab:admin"]))
        .await
        .unwrap();
    assert_eq!(
        insufficient.protected_mcp_initialize().await.unwrap(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        insufficient
            .introspect_token("ordinary.issuer.shaped.token")
            .await
            .unwrap(),
        StatusCode::UNAUTHORIZED
    );
    assert!(insufficient.cleanup().await.unwrap().is_clean());
}

#[test]
fn offline_prepare_rejects_invalid_identity_and_partial_state_is_recoverable() {
    let parent = std::env::temp_dir().join("labby-live-e2e");
    std::fs::create_dir_all(&parent).unwrap();
    let root = tempfile::Builder::new()
        .prefix("identity-invalid-")
        .tempdir_in(parent)
        .unwrap();
    assert!(prepare(root.path(), "invalid\nidentity", 300).is_err());
    assert!(prepare(root.path(), "owner@example.test", 0).is_err());
    assert!(!root.path().join("credential.txt").exists());
    let normalized = prepare(root.path(), " owner@example.test ", 300).unwrap();
    let bundle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("proof.json")).unwrap()).unwrap();
    assert_eq!(bundle["manifest"]["subject"], "owner@example.test");
    recover(
        root.path(),
        normalized["prepare_id"].as_str().unwrap(),
        true,
    )
    .unwrap();
    assert!(!root.path().join("credential.txt").exists());
}

#[tokio::test]
async fn concurrent_bootstraps_are_isolated_and_distinct_input_is_not_idempotent() {
    let (first, second) = tokio::join!(
        LiveIdentity::bootstrap("same-subject@example.test"),
        LiveIdentity::bootstrap("same-subject@example.test")
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.identity.subject, second.identity.subject);
    assert_ne!(first.identity.issuer, second.identity.issuer);
    assert_ne!(first.prepare_id, second.prepare_id);
    assert_ne!(first.identity.credential_id, second.identity.credential_id);

    let mut changed = first.manifest().clone();
    changed["project_name"] = serde_json::json!("Different Project");
    assert_ne!(
        first.consume_with_manifest(changed).await.unwrap(),
        StatusCode::OK
    );
    assert!(first.cleanup().await.unwrap().is_clean());
    assert!(second.cleanup().await.unwrap().is_clean());
}

#[tokio::test]
async fn concurrent_first_prepare_in_one_installation_has_one_owned_winner() {
    let parent = std::env::temp_dir().join("labby-live-e2e");
    std::fs::create_dir_all(&parent).unwrap();
    let root = tempfile::Builder::new()
        .prefix("identity-concurrent-")
        .tempdir_in(parent)
        .unwrap();
    let first_root = root.path().to_owned();
    let second_root = first_root.clone();
    let (first, second) = tokio::join!(
        tokio::task::spawn_blocking(move || prepare(&first_root, "owner@example.test", 300)),
        tokio::task::spawn_blocking(move || prepare(&second_root, "owner@example.test", 300))
    );
    let results = [first.unwrap(), second.unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let winner = results.into_iter().find_map(Result::ok).unwrap();
    let prepare_id = winner["prepare_id"].as_str().unwrap();
    recover(root.path(), prepare_id, true).unwrap();
    assert!(!root.path().join("credential.txt").exists());
    assert!(!root.path().join("proof.json").exists());
}

#[tokio::test]
async fn credential_and_derived_session_expire_at_the_public_ttl() {
    let mut identity = LiveIdentity::bootstrap_with_ttl("expiry@example.test", 10)
        .await
        .unwrap();
    identity.create_session().await.unwrap();
    assert!(identity.session.as_ref().unwrap().expires_at <= identity.identity.expires_at);
    let now = u64::try_from(labby_auth::util::now_unix()).unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(
        identity.identity.expires_at.saturating_sub(now) + 1,
    ))
    .await;
    assert_eq!(
        identity.introspect().await.unwrap().0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        identity.browser_catalog(None).await.unwrap(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::UNAUTHORIZED
    );
    assert!(identity.cleanup().await.unwrap().is_clean());
}

#[test]
fn offline_recovery_revokes_an_interrupted_prepare_and_deletes_outputs() {
    let parent = std::env::temp_dir().join("labby-live-e2e");
    std::fs::create_dir_all(&parent).unwrap();
    let root = tempfile::Builder::new()
        .prefix("identity-recovery-")
        .tempdir_in(parent)
        .unwrap();
    let prepared = prepare(root.path(), "recovery@example.test", 300).unwrap();
    let prepare_id = prepared["prepare_id"].as_str().unwrap();
    let observed = recover(root.path(), prepare_id, false).unwrap();
    assert_eq!(observed["prepare_id"], prepare_id);
    recover(root.path(), prepare_id, true).unwrap();
    assert!(!root.path().join("credential.txt").exists());
    assert!(!root.path().join("proof.json").exists());
}

#[tokio::test]
async fn timeout_and_browser_crash_drop_revoke_before_removing_the_owned_root() {
    let mut identity = LiveIdentity::bootstrap("drop@example.test").await.unwrap();
    identity.create_session().await.unwrap();
    assert!(identity.exercise_timeout().await.is_err());
    let root = identity.root().to_owned();
    let retained = identity.retained_evidence().to_owned();
    let exact_secrets = identity.exact_secret_canaries();
    assert!(!root.join("credential.txt").exists());
    assert!(!root.join("proof.json").exists());
    drop(identity);
    assert!(
        !root.exists(),
        "abnormal Drop retained the installation root"
    );
    assert!(
        retained.exists(),
        "abnormal Drop did not retain sanitized evidence"
    );
    scan_retained_evidence(&retained, &exact_secrets).unwrap();

    let mut browser_crash = LiveIdentity::bootstrap("browser-crash@example.test")
        .await
        .unwrap();
    browser_crash.create_session().await.unwrap();
    let crash_root = browser_crash.root().to_owned();
    let crash_evidence = browser_crash.retained_evidence().to_owned();
    let crash_secrets = browser_crash.exact_secret_canaries();
    drop(browser_crash);
    assert!(
        !crash_root.exists(),
        "browser crash retained the owned root"
    );
    assert!(
        crash_evidence.exists(),
        "browser crash lost cleanup evidence"
    );
    scan_retained_evidence(&crash_evidence, &crash_secrets).unwrap();
}
