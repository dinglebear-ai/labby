use super::*;
use crate::dispatch::agent_events::subscriptions::activate;
use crate::dispatch::agent_events::tests::{input, owner};
use crate::dispatch::codemode_notices::{NoticeLevel, NoticeProducer, NoticeStore};

const OWNER_KEYS: &str = r#"["test-process"]"#;

fn producer() -> NoticeProducer {
    NoticeProducer {
        actor: "fixture-publisher".into(),
        admin: true,
    }
}

fn message(inbox_id: String, source: &str, dedupe: &str) -> PublishNotice {
    PublishNotice {
        inbox_id,
        source: source.into(),
        level: NoticeLevel::Info,
        message: "CI completed.".into(),
        dedupe_key: dedupe.into(),
        ttl_seconds: 3_600,
    }
}

#[tokio::test]
async fn atomic_enqueue_and_retry_survive_independent_connections() {
    let directory = tempfile::tempdir().unwrap();
    let publisher = NoticeStore::open_at(directory.path()).unwrap();
    let worker = NoticeStore::open_at(directory.path()).unwrap();
    let who = owner("worker-owner");
    let now = now_ms();
    let mut spec = input("ci", 1);
    spec.expires = now + 3_600_000;
    let sub = worker
        .activate_event_subscription(who.clone(), spec, now)
        .await
        .unwrap();
    let inbox = publisher.register(who.recipient.clone()).await.unwrap().id;
    publisher
        .publish(producer(), message(inbox.clone(), "logs", "not-matched"))
        .await
        .unwrap();

    // Fail the enqueue in SQLite. Neither the inbox notice nor its outbox row
    // may commit separately.
    worker
        .run(|db| {
            db.execute_batch(
                "CREATE TRIGGER reject_enqueue BEFORE INSERT ON agent_event_deliveries
            BEGIN SELECT RAISE(ABORT,'fixture'); END;",
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        publisher
            .publish(producer(), message(inbox.clone(), "ci", "event"))
            .await
            .unwrap_err(),
        NoticeError::Unavailable
    );
    worker
        .run(|db| {
            db.execute_batch("DROP TRIGGER reject_enqueue;")?;
            Ok(())
        })
        .await
        .unwrap();
    let notice = publisher
        .publish(producer(), message(inbox.clone(), "ci", "event"))
        .await
        .unwrap();
    assert!(!notice.duplicate);
    let duplicate = publisher
        .publish(producer(), message(inbox, "ci", "event"))
        .await
        .unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(notice.id, duplicate.id);
    let id = notice.id.clone();
    let (_, premature) = publisher
        .controls(who.recipient.clone(), false, Some(vec![id.clone()]))
        .await
        .unwrap();
    assert_eq!(premature, Some(vec![]));

    let now = now_ms();
    let first = worker
        .run(move |db| {
            let claim =
                claim_next(db, OWNER_KEYS, now)?.expect("independent worker observed publish");
            assert!(begin(db, &claim, now)?);
            assert!(
                claim_next(db, OWNER_KEYS, now)?.is_none(),
                "same subscription cannot send twice"
            );
            Ok(claim)
        })
        .await
        .unwrap();
    assert_eq!(first.notice, id);
    assert_eq!(first.subscription, sub.id);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&first.body).unwrap()["eventId"],
        id
    );
    drop(worker);
    let restarted = NoticeStore::open_at(directory.path()).unwrap();
    let retry = restarted
        .run(move |db| {
            assert!(
                claim_next(db, "[]", now + LEASE_MS)?.is_none(),
                "restart has no live authority"
            );
            let retry = claim_next(db, OWNER_KEYS, now + LEASE_MS)?.expect("reclaimed lease");
            assert_eq!(retry.notice, first.notice);
            assert_eq!(retry.body, first.body);
            assert_ne!(retry.token, first.token);
            assert!(begin(db, &retry, now + LEASE_MS)?);
            assert!(mark_offered(db, &retry, now + LEASE_MS)?);
            finish(
                db,
                &retry,
                Finish::Outcome(DeliveryOutcome::Received),
                now + LEASE_MS,
            )?;
            Ok(retry)
        })
        .await
        .unwrap();
    assert_eq!(retry.attempts, 1);
    let id_for_check = id.clone();
    assert_eq!(
        restarted
            .run(move |db| Ok(db.query_row(
                "SELECT acknowledged,webhook_offered FROM agent_notices WHERE id=?",
                [id_for_check],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?))
            .await
            .unwrap(),
        (0, 1)
    );
    let (_, foreign) = publisher
        .controls(owner("other").recipient, false, Some(vec![id.clone()]))
        .await
        .unwrap();
    assert_eq!(foreign, Some(vec![]));
    let (_, ack) = publisher
        .controls(who.recipient, false, Some(vec![id.clone()]))
        .await
        .unwrap();
    assert_eq!(ack, Some(vec![id]));
}

#[tokio::test]
async fn retry_budget_pause_and_generation_fencing() {
    let store = NoticeStore::memory().unwrap();
    let who = owner("retry-owner");
    let inbox = store.register(who.recipient.clone()).await.unwrap().id;
    let now = now_ms();
    let mut spec = input("ci", 1);
    spec.expires = now + 3_600_000;
    store
        .activate_event_subscription(who.clone(), spec, now)
        .await
        .unwrap();
    store
        .publish(producer(), message(inbox, "ci", "retries"))
        .await
        .unwrap();
    let now = now_ms();
    store
        .run(move |db| {
            let first = claim_next(db, OWNER_KEYS, now)?.unwrap();
            finish(db, &first, Finish::Pause, now)?;
            assert!(claim_next(db, OWNER_KEYS, now)?.is_none());
            let now = now + 1_000;
            let stale = claim_next(db, OWNER_KEYS, now)?.unwrap();
            assert_eq!(stale.attempts, 0);
            assert!(begin(db, &stale, now)?);
            let mut refresh = input("ci", 1);
            refresh.expires = now + 3_600_000;
            activate(db, &who, &refresh, now)?;
            assert!(
                claim_next(db, OWNER_KEYS, now + 1_000)?.is_none(),
                "rotation cannot overlap the old lease"
            );
            finish(db, &stale, Finish::Outcome(DeliveryOutcome::Gone), now)?;
            let mut now = now + 1_000;
            for attempt in 1..8 {
                let current = claim_next(db, OWNER_KEYS, now)?.unwrap();
                assert_eq!(current.attempts, attempt);
                assert_eq!(current.generation, stale.generation + 1);
                assert!(begin(db, &current, now)?);
                finish(
                    db,
                    &current,
                    Finish::Outcome(DeliveryOutcome::Retry { after_ms: 60 }),
                    now,
                )?;
                assert!(claim_next(db, OWNER_KEYS, now)?.is_none());
                now += retry_delay(&current).max(60);
            }
            assert!(claim_next(db, OWNER_KEYS, now + LEASE_MS)?.is_none());
            let (attempts, status): (i64, String) = db.query_row(
                "SELECT attempts,status FROM agent_event_deliveries",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!((attempts, status.as_str()), (8, "stopped"));
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn eight_leases_globally_across_store_handles() {
    let directory = tempfile::tempdir().unwrap();
    let first = NoticeStore::open_at(directory.path()).unwrap();
    let second = NoticeStore::open_at(directory.path()).unwrap();
    let now = now_ms();
    for i in 0..9 {
        let who = owner(&format!("claim-owner-{i}"));
        let mut spec = input("ci", i);
        spec.expires = now + 3_600_000;
        first
            .activate_event_subscription(who.clone(), spec, now)
            .await
            .unwrap();
        let inbox = first.register(who.recipient.clone()).await.unwrap().id;
        first
            .publish(producer(), message(inbox, "ci", "global"))
            .await
            .unwrap();
    }
    let now = now_ms();
    for _ in 0..4 {
        for store in [&first, &second] {
            assert!(
                store
                    .run(move |db| claim_next(db, OWNER_KEYS, now))
                    .await
                    .unwrap()
                    .is_some()
            );
        }
    }
    assert!(
        second
            .run(move |db| claim_next(db, OWNER_KEYS, now))
            .await
            .unwrap()
            .is_none()
    );
}
