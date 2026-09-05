use crate::browser_authority::{BrowserAuthority, BrowserPolicy, PermissionState, PolicyFuture};
use crate::browser_authority_tests::{Policy, store};
use crate::reauth::{Outcome, ProofError, Proofs, Purpose, ReservationState, TrustedAuthEvent};
use crate::util::now_unix;
use serde_json::json;
use std::sync::Arc;

fn purpose(operation: &str, payload: serde_json::Value) -> Purpose {
    Purpose::new(
        "provider.update",
        "team",
        "version-1",
        operation,
        "lab:admin",
        &payload,
    )
    .unwrap()
}
fn event(authority: &BrowserAuthority, authenticated_at: i64) -> TrustedAuthEvent {
    TrustedAuthEvent {
        binding: authority.binding(),
        authenticated_at,
    }
}
async fn authority(store: crate::sqlite::SqliteStore, session: &str) -> BrowserAuthority {
    BrowserAuthority::verify(store, session, "deployment", Arc::new(Policy::default()))
        .await
        .unwrap()
}

#[tokio::test]
async fn proof_binds_exact_action_payload_version_operation_and_session() {
    let (_dir, store, mut row) = store().await;
    let first = authority(store.clone(), &row.session_id).await;
    let proofs = Proofs::new(store.clone());
    let intent = purpose(
        "operation-1",
        json!({"name":"Team", "credential":"never-log-me"}),
    );
    let issued = proofs
        .issue(&first, &event(&first, now_unix()), &intent)
        .await
        .unwrap();
    assert_eq!(issued.proof.as_str().len(), 43);
    assert!(!format!("{issued:?} {intent:?}").contains("never-log-me"));
    assert!(issued.expires_at <= now_unix() + 120);
    for changed in [
        purpose(
            "operation-2",
            json!({"name":"Team", "credential":"never-log-me"}),
        ),
        purpose(
            "operation-1",
            json!({"name":"Changed", "credential":"never-log-me"}),
        ),
        Purpose::new(
            "provider.remove",
            "team",
            "version-1",
            "operation-1",
            "lab:admin",
            &json!({}),
        )
        .unwrap(),
        Purpose::new(
            "provider.update",
            "team",
            "version-2",
            "operation-1",
            "lab:admin",
            &json!({}),
        )
        .unwrap(),
    ] {
        assert!(matches!(
            proofs.reserve(&issued.proof, &first, &changed).await,
            Err(ProofError::Replayed)
        ));
    }
    row.session_id = "other-session".into();
    store.upsert_browser_session(row.clone()).await.unwrap();
    let other = authority(store.clone(), &row.session_id).await;
    assert!(
        proofs
            .reserve(&issued.proof, &other, &intent)
            .await
            .is_err()
    );
    store
        .revoke_browser_session("server-held-cookie")
        .await
        .unwrap();
    assert!(
        proofs
            .reserve(&issued.proof, &first, &intent)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn reserve_finalize_and_reopen_preserve_one_logical_operation() {
    let (dir, store, row) = store().await;
    let authority = authority(store.clone(), &row.session_id).await;
    let proofs = Proofs::new(store);
    let first = Purpose::new(
        "provider.update",
        "team",
        "v1",
        "operation",
        "lab:admin",
        &json!({"z":2,"a":1}),
    )
    .unwrap();
    let same = Purpose::new(
        "provider.update",
        "team",
        "v1",
        "operation",
        "lab:admin",
        &json!({"a":1,"z":2}),
    )
    .unwrap();
    let issued = proofs
        .issue(&authority, &event(&authority, now_unix()), &first)
        .await
        .unwrap();
    let reserved = proofs
        .reserve(&issued.proof, &authority, &same)
        .await
        .unwrap();
    assert_eq!(reserved.state(), ReservationState::Reserved);
    let reopened = Proofs::new(
        crate::sqlite::SqliteStore::open(dir.path().join("auth.db"))
            .await
            .unwrap(),
    );
    assert_eq!(
        reopened
            .reserve(&issued.proof, &authority, &same)
            .await
            .unwrap()
            .state(),
        ReservationState::Reserved
    );
    reopened
        .finalize(&reserved, Outcome::Committed)
        .await
        .unwrap();
    reopened
        .finalize(&reserved, Outcome::Committed)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .reserve(&issued.proof, &authority, &same)
            .await
            .unwrap()
            .state(),
        ReservationState::Finalized(Outcome::Committed)
    );
    assert_eq!(
        reopened.finalize(&reserved, Outcome::Aborted).await,
        Err(ProofError::Replayed)
    );
}

#[tokio::test]
async fn stale_future_and_expired_proofs_cannot_be_redeemed() {
    let (_dir, store, row) = store().await;
    let authority = authority(store.clone(), &row.session_id).await;
    let proofs = Proofs::new(store.clone());
    let intent = purpose("operation", json!({}));
    for time in [now_unix() - 301, now_unix() + 60] {
        assert!(matches!(
            proofs
                .issue(&authority, &event(&authority, time), &intent)
                .await,
            Err(ProofError::Required)
        ));
    }
    let issued = proofs
        .issue(&authority, &event(&authority, now_unix() - 290), &intent)
        .await
        .unwrap();
    assert!(issued.expires_at <= now_unix() + 10);
    store
        .execute_test_statement("UPDATE reauth_proofs SET expires_at = 0")
        .await
        .unwrap();
    assert!(matches!(
        proofs.reserve(&issued.proof, &authority, &intent).await,
        Err(ProofError::Expired)
    ));
}

#[tokio::test]
async fn actor_issuance_and_verification_limits_are_enforced() {
    let (_dir, store, row) = store().await;
    let authority = authority(store.clone(), &row.session_id).await;
    let proofs = Proofs::new(store.clone());
    let intent = purpose("operation", json!({}));
    let mut issued = Vec::new();
    for _ in 0..5 {
        issued.push(
            proofs
                .issue(&authority, &event(&authority, now_unix()), &intent)
                .await
                .unwrap(),
        );
    }
    assert!(matches!(
        proofs
            .issue(&authority, &event(&authority, now_unix()), &intent)
            .await,
        Err(ProofError::RateLimited)
    ));
    for _ in 0..30 {
        proofs
            .reserve(&issued[0].proof, &authority, &intent)
            .await
            .unwrap();
    }
    assert!(matches!(
        proofs.reserve(&issued[0].proof, &authority, &intent).await,
        Err(ProofError::RateLimited)
    ));
    store
        .execute_test_statement("DELETE FROM reauth_attempts")
        .await
        .unwrap();
    for _ in 0..3 {
        proofs
            .issue(&authority, &event(&authority, now_unix()), &intent)
            .await
            .unwrap();
    }
    assert!(matches!(
        proofs
            .issue(&authority, &event(&authority, now_unix()), &intent)
            .await,
        Err(ProofError::Capacity)
    ));
}

#[test]
fn purpose_is_bounded_and_cannot_normalize_away_payload_identity() {
    assert!(Purpose::new("", "team", "v1", "operation", "lab:admin", &json!({})).is_err());
    assert!(
        Purpose::new(
            "update",
            "team",
            "v1",
            "operation",
            "lab:admin",
            &json!({"token":"x".repeat(65_537)})
        )
        .is_err()
    );
}

#[tokio::test]
async fn global_issuance_verification_and_storage_caps_are_independent() {
    let (_dir, store, mut row) = store().await;
    let proofs = Proofs::new(store.clone());
    let intent = purpose("operation", json!({}));
    let mut actors = Vec::new();
    for n in 0..129 {
        row.subject = format!("actor-{n}");
        row.session_id = format!("session-{n}");
        store.upsert_browser_session(row.clone()).await.unwrap();
        actors.push(authority(store.clone(), &row.session_id).await);
    }
    let mut issued = Vec::new();
    for actor in &actors[..30] {
        issued.push(
            proofs
                .issue(actor, &event(actor, now_unix()), &intent)
                .await
                .unwrap(),
        );
    }
    assert!(matches!(
        proofs
            .issue(&actors[30], &event(&actors[30], now_unix()), &intent)
            .await,
        Err(ProofError::RateLimited)
    ));
    for n in 0..4 {
        for _ in 0..30 {
            proofs
                .reserve(&issued[n].proof, &actors[n], &intent)
                .await
                .unwrap();
        }
    }
    assert!(matches!(
        proofs.reserve(&issued[4].proof, &actors[4], &intent).await,
        Err(ProofError::RateLimited)
    ));
    for actor in &actors[30..128] {
        // Isolate the storage cap from the independently asserted time windows.
        store
            .execute_test_statement("DELETE FROM reauth_attempts")
            .await
            .unwrap();
        proofs
            .issue(actor, &event(actor, now_unix()), &intent)
            .await
            .unwrap();
    }
    store
        .execute_test_statement("DELETE FROM reauth_attempts")
        .await
        .unwrap();
    assert!(matches!(
        proofs
            .issue(&actors[128], &event(&actors[128], now_unix()), &intent)
            .await,
        Err(ProofError::Capacity)
    ));
}

#[tokio::test]
async fn logout_and_expiry_while_waiting_for_the_proof_transaction_deny_redemption() {
    for expires_during_wait in [false, true] {
        struct NotifyingPolicy(Arc<tokio::sync::Notify>);
        impl BrowserPolicy for NotifyingPolicy {
            fn current<'a>(&'a self, _: &'a crate::types::BrowserSessionRow) -> PolicyFuture<'a> {
                Box::pin(async move {
                    self.0.notify_one();
                    Ok(PermissionState {
                        epoch: "1".into(),
                        scopes: vec!["lab:admin".into()],
                    })
                })
            }
        }
        let (dir, store, row) = store().await;
        let notify = Arc::new(tokio::sync::Notify::new());
        let actor = BrowserAuthority::verify(
            store.clone(),
            &row.session_id,
            "deployment",
            Arc::new(NotifyingPolicy(notify.clone())),
        )
        .await
        .unwrap();
        let proofs = Proofs::new(store);
        let intent = purpose("operation", json!({}));
        let issued = proofs
            .issue(&actor, &event(&actor, now_unix()), &intent)
            .await
            .unwrap();
        notify.notified().await;
        let mut connection = rusqlite::Connection::open(dir.path().join("auth.db")).unwrap();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let expiry = now_unix() + 1;
        if expires_during_wait {
            transaction
                .execute(
                    "UPDATE reauth_proofs SET expires_at = ?1",
                    rusqlite::params![expiry],
                )
                .unwrap();
        } else {
            transaction
                .execute(
                    "DELETE FROM browser_sessions WHERE session_id = ?1",
                    rusqlite::params![row.session_id],
                )
                .unwrap();
        }
        let task =
            tokio::spawn(async move { proofs.reserve(&issued.proof, &actor, &intent).await });
        notify.notified().await;
        if expires_during_wait {
            while now_unix() <= expiry {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
        transaction.commit().unwrap();
        let expected = if expires_during_wait {
            ProofError::Expired
        } else {
            ProofError::Denied
        };
        assert!(matches!(task.await.unwrap(), Err(error) if error == expected));
    }
}

#[tokio::test]
async fn a_new_process_cannot_redeem_a_previous_process_proof() {
    if let Ok(path) = std::env::var("LABBY_TEST_REAUTH_DB") {
        let store = crate::sqlite::SqliteStore::open(path.into()).await.unwrap();
        let actor = authority(store.clone(), "server-held-cookie").await;
        let proof =
            crate::reauth::ProofHandle::parse(std::env::var("LABBY_TEST_REAUTH_PROOF").unwrap())
                .unwrap();
        assert!(
            Proofs::new(store)
                .reserve(&proof, &actor, &purpose("operation", json!({})))
                .await
                .is_err()
        );
        return;
    }
    let (dir, store, row) = store().await;
    let actor = authority(store.clone(), &row.session_id).await;
    let issued = Proofs::new(store)
        .issue(
            &actor,
            &event(&actor, now_unix()),
            &purpose("operation", json!({})),
        )
        .await
        .unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "reauth_tests::a_new_process_cannot_redeem_a_previous_process_proof",
        ])
        .env("LABBY_TEST_REAUTH_DB", dir.path().join("auth.db"))
        .env("LABBY_TEST_REAUTH_PROOF", issued.proof.as_str())
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}
