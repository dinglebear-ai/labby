use super::*;

fn who(actor: &str) -> NoticeRecipient {
    NoticeRecipient {
        actor: Some(actor.into()),
        route: "root".into(),
        consumer: NoticeConsumer::Client {
            id: "client".into(),
            conversation: None,
        },
    }
}
fn producer() -> NoticeProducer {
    NoticeProducer {
        actor: "admin".into(),
        admin: true,
    }
}
fn message(inbox: &str, key: &str) -> PublishNotice {
    PublishNotice {
        inbox_id: inbox.into(),
        source: "test".into(),
        level: NoticeLevel::Info,
        message: "Indexing finished.".into(),
        dedupe_key: key.into(),
        ttl_seconds: 3600,
    }
}
fn db() -> Connection {
    let store = NoticeStore::memory().unwrap();
    Arc::try_unwrap(store.connection.unwrap())
        .ok()
        .unwrap()
        .into_inner()
        .unwrap()
}
fn take(db: &mut Connection, key: &str, now: i64) -> Option<NoticeBatch> {
    deliver_at(db, key, now, |v| Some(v.clone())).unwrap()
}

#[test]
fn notice_production_lease_retry_ack_and_dedupe_are_durable() {
    let dir = tempfile::tempdir().unwrap();
    let store = NoticeStore::open_at(dir.path()).unwrap();
    let key = who("alice").key().unwrap();
    let id;
    let inbox;
    {
        let mut c = store.connection.as_ref().unwrap().lock().unwrap();
        inbox = register_at(&mut c, &key, "authenticated_client", 1000)
            .unwrap()
            .id;
        id = publish_at(&mut c, "admin", &message(&inbox, "job-1"), 1001)
            .unwrap()
            .id;
        let first = take(&mut c, &key, 1002).unwrap();
        assert_eq!(first.notifications[0].id, id);
        assert_eq!(first.notifications[0].delivery_attempt, 1);
        assert!(take(&mut c, &key, 1003).is_none());
    }
    drop(store);
    let store = NoticeStore::open_at(dir.path()).unwrap();
    let mut c = store.connection.as_ref().unwrap().lock().unwrap();
    assert!(take(&mut c, &key, 31_001).is_none());
    let retried = take(&mut c, &key, 31_002).unwrap();
    assert_eq!(retried.notifications[0].id, id);
    assert_eq!(retried.notifications[0].delivery_attempt, 2);
    assert_eq!(
        acknowledge_at(&mut c, &key, std::slice::from_ref(&id), 31_003).unwrap(),
        vec![id.clone()]
    );
    assert_eq!(
        acknowledge_at(&mut c, &key, std::slice::from_ref(&id), 31_004).unwrap(),
        vec![id.clone()]
    );
    assert!(take(&mut c, &key, 100_000).is_none());
    assert!(
        publish_at(&mut c, "admin", &message(&inbox, "job-1"), 100_001)
            .unwrap()
            .duplicate
    );
}

#[test]
fn notice_production_recipient_boundaries_and_no_foreign_ack() {
    let mut c = db();
    let alice = who("alice");
    let key = alice.key().unwrap();
    let inbox = register_at(&mut c, &key, alice.scope(), 1000).unwrap();
    let id = publish_at(&mut c, "admin", &message(&inbox.id, "job"), 1001)
        .unwrap()
        .id;
    let mut variants = vec![who("bob")];
    let mut wrong = alice.clone();
    wrong.route = "other".into();
    variants.push(wrong);
    let mut wrong = alice.clone();
    wrong.consumer = NoticeConsumer::Client {
        id: "other".into(),
        conversation: None,
    };
    variants.push(wrong);
    let mut wrong = alice.clone();
    wrong.consumer = NoticeConsumer::Client {
        id: "client".into(),
        conversation: Some("other".into()),
    };
    variants.push(wrong);
    variants.push(NoticeRecipient {
        actor: None,
        route: "root".into(),
        consumer: NoticeConsumer::Stdio {
            session: "other".into(),
        },
    });
    for wrong in variants {
        let wrong = wrong.key().unwrap();
        assert!(take(&mut c, &wrong, 1002).is_none());
        assert!(
            acknowledge_at(&mut c, &wrong, std::slice::from_ref(&id), 1002)
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        acknowledge_at(&mut c, &key, std::slice::from_ref(&id), 1002)
            .unwrap()
            .is_empty(),
        "cannot ACK before offer"
    );
    assert_eq!(take(&mut c, &key, 1002).unwrap().notifications.len(), 1);
}

#[test]
fn notice_production_budget_rejection_is_transactional_and_count_is_bounded() {
    let mut c = db();
    let key = who("alice").key().unwrap();
    let inbox = register_at(&mut c, &key, "client", 1000).unwrap().id;
    for n in 0..5 {
        publish_at(
            &mut c,
            "admin",
            &message(&inbox, &format!("job-{n}")),
            1001 + n,
        )
        .unwrap();
    }
    assert!(
        deliver_at::<()>(&mut c, &key, 1100, |_| None)
            .unwrap()
            .is_none()
    );
    let batch = take(&mut c, &key, 1101).unwrap();
    assert_eq!(batch.notifications.len(), 3);
    assert_eq!(batch.notifications_remaining, 2);
    assert!(serde_json::to_vec(&batch).unwrap().len() <= MAX_NOTICE_BATCH_BYTES);
    let batch = take(&mut c, &key, 1102).unwrap();
    assert_eq!(batch.notifications.len(), 2);
    assert_eq!(
        batch.notifications_remaining, 3,
        "remaining includes leased, unacknowledged notices"
    );
}

#[test]
fn notice_production_expiry_quota_and_idempotency_conflicts() {
    let mut c = db();
    let key = who("alice").key().unwrap();
    let inbox = register_at(&mut c, &key, "client", 1000).unwrap().id;
    let req = message(&inbox, "job");
    let original = publish_at(&mut c, "admin", &req, 1001).unwrap();
    assert_eq!(
        publish_at(&mut c, "admin", &req, 1002).unwrap().id,
        original.id
    );
    let mut conflict = req.clone();
    conflict.message = "Different payload.".into();
    assert_eq!(
        publish_at(&mut c, "admin", &conflict, 1002).unwrap_err(),
        NoticeError::Conflict
    );
    for n in 1..MAX_PER_RECIPIENT {
        publish_at(&mut c, "admin", &message(&inbox, &format!("job-{n}")), 1002).unwrap();
    }
    assert_eq!(
        publish_at(&mut c, "admin", &message(&inbox, "overflow"), 1003).unwrap_err(),
        NoticeError::Full
    );
    assert!(take(&mut c, &key, 3_601_003).is_none());
    assert!(
        publish_at(&mut c, "admin", &req, 3_601_004).is_ok(),
        "expired entries reclaim quota"
    );
    assert!(take(&mut c, &key, INBOX_TTL_MS + 1001).is_none());
}

#[tokio::test]
async fn notice_production_validation_admin_gate_and_store_failure() {
    let store = NoticeStore::memory().unwrap();
    let who = who("alice");
    let inbox = store.register(who.clone()).await.unwrap();
    let input = message(&inbox.id, "job");
    assert_eq!(
        store
            .publish(
                NoticeProducer {
                    actor: "alice".into(),
                    admin: false
                },
                input.clone()
            )
            .await
            .unwrap_err(),
        NoticeError::Forbidden
    );
    for text in ["".to_string(), "x".repeat(385), "bad\ncontrol".into()] {
        let mut bad = input.clone();
        bad.message = text;
        assert_eq!(
            store.publish(producer(), bad).await.unwrap_err(),
            NoticeError::Invalid
        );
    }
    let mut escaped = input.clone();
    escaped.source = "\\".repeat(64);
    escaped.message = "\\".repeat(384);
    assert_eq!(
        store.publish(producer(), escaped).await.unwrap_err(),
        NoticeError::Invalid
    );
    let mut bad = input.clone();
    bad.ttl_seconds = 0;
    assert_eq!(validate_publish(&bad), Err(NoticeError::Invalid));
    bad.ttl_seconds = 86_401;
    assert_eq!(validate_publish(&bad), Err(NoticeError::Invalid));
    assert_eq!(
        validate_acknowledgments(&["bad".into()]),
        Err(NoticeError::Invalid)
    );
    let ids = vec![format!("notice_{}", ulid::Ulid::new()); MAX_ACKS + 1];
    assert_eq!(validate_acknowledgments(&ids), Err(NoticeError::Invalid));
    let permit = store.permit.acquire().await.unwrap();
    assert_eq!(
        store.register(who.clone()).await.unwrap_err(),
        NoticeError::Busy
    );
    drop(permit);
    assert_eq!(
        NoticeStore::disabled().register(who).await.unwrap_err(),
        NoticeError::Unavailable
    );
}

#[test]
fn notice_production_global_capacity_and_future_schema_fail_closed() {
    let mut c = db();
    for n in 0..MAX_INBOXES {
        register_at(&mut c, &format!("recipient-{n}"), "client", 1000).unwrap();
    }
    assert_eq!(
        register_at(&mut c, "overflow", "client", 1001).unwrap_err(),
        NoticeError::Full
    );
    let c = Connection::open_in_memory().unwrap();
    c.pragma_update(None, "user_version", 999).unwrap();
    assert!(matches!(
        NoticeStore::from_connection(c),
        Err(NoticeError::Unavailable)
    ));
}

#[cfg(unix)]
#[test]
fn notice_production_private_file_and_symlink_refusal() {
    use std::os::unix::fs::{MetadataExt as _, symlink};
    let dir = tempfile::tempdir().unwrap();
    let store = NoticeStore::open_at(dir.path()).unwrap();
    drop(store);
    let path = dir.path().join("agent-notifications/inbox.sqlite3");
    assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    std::fs::remove_file(&path).unwrap();
    let unrelated = dir.path().join("unrelated");
    std::fs::write(&unrelated, b"untouched").unwrap();
    symlink(&unrelated, &path).unwrap();
    assert!(matches!(
        NoticeStore::open_at(dir.path()),
        Err(NoticeError::Unavailable)
    ));
    assert_eq!(std::fs::read(&unrelated).unwrap(), b"untouched");
}

#[tokio::test]
async fn notice_production_default_disabled_and_deadline_keeps_worker_bounded() {
    assert_eq!(
        NoticeStore::default()
            .register(who("alice"))
            .await
            .unwrap_err(),
        NoticeError::Unavailable
    );
    let store = NoticeStore::memory().unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let worker_store = store.clone();
    let operation = tokio::spawn(async move {
        worker_store
            .run(move |_| {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                finished_tx.send(()).unwrap();
                Ok(())
            })
            .await
    });
    tokio::task::spawn_blocking(move || started_rx.recv_timeout(Duration::from_secs(5)).unwrap())
        .await
        .unwrap();
    assert_eq!(operation.await.unwrap(), Err(NoticeError::Deadline));
    assert_eq!(
        store.register(who("alice")).await.unwrap_err(),
        NoticeError::Busy
    );
    release_tx.send(()).unwrap();
    tokio::task::spawn_blocking(move || finished_rx.recv_timeout(Duration::from_secs(5)).unwrap())
        .await
        .unwrap();
}

#[test]
fn notice_production_missing_schema_and_orphaned_rows_fail_closed() {
    let c = db();
    c.execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE agent_inboxes;")
        .unwrap();
    assert!(matches!(
        NoticeStore::from_connection(c),
        Err(NoticeError::Unavailable)
    ));
    let mut c = db();
    let key = who("alice").key().unwrap();
    let inbox = register_at(&mut c, &key, "client", 1000).unwrap();
    publish_at(&mut c, "admin", &message(&inbox.id, "job"), 1001).unwrap();
    c.execute_batch("PRAGMA foreign_keys=OFF; DELETE FROM agent_inboxes;")
        .unwrap();
    assert!(matches!(
        NoticeStore::from_connection(c),
        Err(NoticeError::Unavailable)
    ));
}

#[test]
fn notice_production_concurrent_first_open_is_supported() {
    let dir = tempfile::tempdir().unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..8 {
            let barrier = barrier.clone();
            let root = dir.path();
            handles.push(scope.spawn(move || {
                barrier.wait();
                NoticeStore::open_at(root)
            }));
        }
        for handle in handles {
            assert!(handle.join().unwrap().is_ok());
        }
    });
}

#[tokio::test]
async fn notice_production_combined_controls_roll_back_registration_on_ack_failure() {
    let store = NoticeStore::memory().unwrap();
    let recipient = who("alice");
    let key = recipient.key().unwrap();
    let id;
    {
        let mut c = store.connection.as_ref().unwrap().lock().unwrap();
        let inbox = register_at(&mut c, &key, "client", now_ms()).unwrap();
        id = publish_at(&mut c, "admin", &message(&inbox.id, "job"), now_ms())
            .unwrap()
            .id;
        take(&mut c, &key, now_ms()).unwrap();
        c.execute("UPDATE agent_inboxes SET expires=?", [now_ms() + 60_000])
            .unwrap();
        c.execute_batch("CREATE TRIGGER reject_ack BEFORE UPDATE OF acknowledged ON agent_notices BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    }
    let before: i64 = store
        .connection
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .query_row("SELECT expires FROM agent_inboxes", [], |r| r.get(0))
        .unwrap();
    assert!(
        store
            .controls(recipient.clone(), true, Some(vec![id.clone()]))
            .await
            .is_err()
    );
    let after: i64 = store
        .connection
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .query_row("SELECT expires FROM agent_inboxes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after);
    store
        .connection
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_ack;")
        .unwrap();
    let (address, acks) = store
        .controls(recipient, true, Some(vec![id.clone()]))
        .await
        .unwrap();
    assert!(address.unwrap().expires_at_unix_ms > before);
    assert_eq!(acks.unwrap(), vec![id]);
}

#[cfg(windows)]
#[test]
fn notice_production_windows_existing_file_acl_and_hardlinks_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    drop(NoticeStore::open_at(dir.path()).unwrap());
    let path = dir.path().join("agent-notifications/inbox.sqlite3");
    std::fs::hard_link(&path, dir.path().join("alias.sqlite3")).unwrap();
    assert!(matches!(
        NoticeStore::open_at(dir.path()),
        Err(NoticeError::Unavailable)
    ));
    std::fs::remove_file(dir.path().join("alias.sqlite3")).unwrap();
    std::fs::remove_file(&path).unwrap();
    // Ordinary filesystem creation inherits its parent ACL, rather than the
    // explicitly protected sole-user file ACL required by secure_file.
    std::fs::write(&path, []).unwrap();
    assert!(matches!(
        NoticeStore::open_at(dir.path()),
        Err(NoticeError::Unavailable)
    ));
    assert_eq!(std::fs::metadata(path).unwrap().len(), 0);
}

#[test]
fn notice_production_existing_schema_missing_constraints_is_refused() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(&SCHEMA_SQL.replace(
        "UNIQUE(inbox_id, producer, dedupe_key)",
        "CHECK(attempts >= 0)",
    ))
    .unwrap();
    c.pragma_update(None, "user_version", SCHEMA_VERSION)
        .unwrap();
    c.pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    assert!(matches!(
        NoticeStore::from_connection(c),
        Err(NoticeError::Unavailable)
    ));
}

#[cfg(windows)]
#[test]
fn notice_production_windows_second_open_while_first_connection_is_live() {
    let dir = tempfile::tempdir().unwrap();
    let first = NoticeStore::open_at(dir.path()).unwrap();
    let second = NoticeStore::open_at(dir.path()).unwrap();
    let key = who("alice").key().unwrap();
    let inbox = {
        let mut connection = first.connection.as_ref().unwrap().lock().unwrap();
        register_at(&mut connection, &key, "client", 1000).unwrap()
    };
    let mut connection = second.connection.as_ref().unwrap().lock().unwrap();
    assert_eq!(
        register_at(&mut connection, &key, "client", 1001)
            .unwrap()
            .id,
        inbox.id
    );
    drop(connection);
    drop(first);
    drop(second);
}

#[test]
fn notice_production_storage_diagnostics_exclude_sensitive_error_text() {
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = Capture(Arc::clone(&bytes));
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let sqlite = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
            Some("private notice payload and SQL".into()),
        );
        assert_eq!(NoticeError::from(sqlite), NoticeError::Unavailable);
        let io = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "private database path",
        );
        assert_eq!(
            filesystem_failure("verify_database", &io),
            NoticeError::Unavailable
        );
    });
    let logs = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("DiskFull"));
    assert!(logs.contains("PermissionDenied"));
    assert!(logs.contains("verify_database"));
    assert!(!logs.contains("private notice"));
    assert!(!logs.contains("private database"));
}
