use crate::browser_authority::{
    AuthorityError, BrowserAuthority, BrowserPolicy, PermissionState, PolicyFuture,
};
use crate::sqlite::SqliteStore;
use crate::types::BrowserSessionRow;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

struct Policy {
    epoch: AtomicU64,
    allowed: AtomicBool,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            epoch: AtomicU64::new(1),
            allowed: AtomicBool::new(true),
        }
    }
}
impl BrowserPolicy for Policy {
    fn current<'a>(&'a self, _: &'a BrowserSessionRow) -> PolicyFuture<'a> {
        Box::pin(async move {
            if !self.allowed.load(Ordering::SeqCst) {
                return Err(AuthorityError::Denied);
            }
            Ok(PermissionState {
                epoch: self.epoch.load(Ordering::SeqCst).to_string(),
                scopes: vec!["lab:read".into(), "lab:admin".into()],
            })
        })
    }
}

async fn store() -> (tempfile::TempDir, SqliteStore, BrowserSessionRow) {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path().join("auth.db")).await.unwrap();
    let now = crate::util::now_unix();
    let row = BrowserSessionRow {
        session_id: "server-held-cookie".into(),
        subject: "same-principal".into(),
        email: Some("member@example.com".into()),
        csrf_token: "server-held-csrf".into(),
        created_at: now,
        expires_at: now + 3600,
        project_binding: None,
    };
    store.upsert_browser_session(row.clone()).await.unwrap();
    (dir, store, row)
}

#[tokio::test]
async fn same_principal_has_distinct_session_bindings_and_no_secret_debug_output() {
    let (_dir, store, mut row) = store().await;
    let policy = Arc::new(Policy::default());
    let first =
        BrowserAuthority::verify(store.clone(), &row.session_id, "deployment", policy.clone())
            .await
            .unwrap();
    row.session_id = "second-server-cookie".into();
    store.upsert_browser_session(row.clone()).await.unwrap();
    let second = BrowserAuthority::verify(store, &row.session_id, "deployment", policy)
        .await
        .unwrap();
    assert_eq!(first.actor_key(), second.actor_key());
    assert_ne!(first.binding(), second.binding());
    assert_ne!(first.public_epoch(), second.public_epoch());
    let debug = format!("{first:?}");
    for secret in [
        "server-held-cookie",
        "server-held-csrf",
        "same-principal",
        "member@example.com",
    ] {
        assert!(!debug.contains(secret));
    }
    assert!(first.revalidate().await.unwrap().has_scope("lab:admin"));
}

#[tokio::test]
async fn every_use_observes_logout_and_session_replacement() {
    let (_dir, store, mut row) = store().await;
    let authority = BrowserAuthority::verify(
        store.clone(),
        &row.session_id,
        "deployment",
        Arc::new(Policy::default()),
    )
    .await
    .unwrap();
    row.csrf_token = "replacement".into();
    store.upsert_browser_session(row.clone()).await.unwrap();
    assert!(matches!(
        authority.revalidate().await,
        Err(AuthorityError::Changed)
    ));
    store.revoke_browser_session(&row.session_id).await.unwrap();
    assert!(matches!(
        authority.revalidate().await,
        Err(AuthorityError::Denied)
    ));
}

#[tokio::test]
async fn permission_epoch_and_live_denial_invalidate_existing_authority() {
    let (_dir, store, row) = store().await;
    let policy = Arc::new(Policy::default());
    let first =
        BrowserAuthority::verify(store.clone(), &row.session_id, "deployment", policy.clone())
            .await
            .unwrap();
    policy.epoch.store(2, Ordering::SeqCst);
    assert!(matches!(
        first.revalidate().await,
        Err(AuthorityError::Changed)
    ));
    let second = BrowserAuthority::verify(store, &row.session_id, "deployment", policy.clone())
        .await
        .unwrap();
    assert_ne!(first.public_epoch(), second.public_epoch());
    policy.allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        second.revalidate().await,
        Err(AuthorityError::Denied)
    ));
}

#[tokio::test]
async fn forged_expired_or_oversized_session_authority_is_rejected() {
    let (_dir, store, mut row) = store().await;
    let policy = Arc::new(Policy::default());
    assert!(matches!(
        BrowserAuthority::verify(store.clone(), "client-forged", "deployment", policy.clone())
            .await,
        Err(AuthorityError::Denied)
    ));
    assert!(
        BrowserAuthority::verify(
            store.clone(),
            &row.session_id,
            &"x".repeat(2049),
            policy.clone()
        )
        .await
        .is_err()
    );
    row.expires_at = crate::util::now_unix() - 1;
    store.upsert_browser_session(row.clone()).await.unwrap();
    assert!(matches!(
        BrowserAuthority::verify(store, &row.session_id, "deployment", policy).await,
        Err(AuthorityError::Denied)
    ));
}
