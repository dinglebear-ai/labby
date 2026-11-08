use super::*;

fn message_binding(key: char) -> crate::dispatch::agent_messages::SessionBinding {
    use crate::dispatch::agent_messages::{SessionBinding, SessionKind};
    let binding_key = key.to_string().repeat(64);
    let owner_key = "1".repeat(64);
    SessionBinding {
        recipient: NoticeRecipient {
            actor: Some(owner_key.clone()),
            route: "root".into(),
            consumer: NoticeConsumer::MessageSession {
                binding: binding_key.clone(),
            },
        },
        owner_key,
        binding_key,
        kind: SessionKind::Http,
        stdio_closed: None,
    }
}

async fn register_message_session(
    store: &NoticeStore,
    binding: crate::dispatch::agent_messages::SessionBinding,
    label: &str,
) -> serde_json::Value {
    store
        .message_request(
            binding,
            serde_json::from_value(serde_json::json!({
                "action":"register", "params":{"label":label}
            }))
            .unwrap(),
        )
        .await
        .unwrap()
}

#[test]
fn normal_event_subscription_identity_unchanged() {
    let recipient = owner("existing").recipient;
    let url = "https://example.com/callback";
    let old = serde_json::to_vec(&(
        "labby.event.subscription.v1",
        recipient.key().unwrap(),
        EVENT_NAME,
        url,
        serde_json::json!({"source":"ci"}),
    ))
    .unwrap();
    for args in [
        serde_json::json!({"source":"ci"}),
        serde_json::json!({"source":"ci","session_messages":false}),
    ] {
        let target = SubscriptionTarget::new(
            EVENT_NAME,
            serde_json::from_value(args).unwrap(),
            "webhook",
            url,
        )
        .unwrap();
        assert_eq!(
            target.id(&recipient).unwrap(),
            format!("sub_{}", hex::encode(Sha256::digest(&old)))
        );
    }
}

#[tokio::test]
async fn message_subscription_filters_typed_references() {
    use crate::dispatch::codemode_notices::{NoticeLevel, NoticeProducer, PublishNotice};
    use serde_json::{Value, json};

    let store = NoticeStore::memory().unwrap();
    let events = Arc::new(AgentEvents::new(
        store.clone(),
        load_key(&STANDARD.encode([b't'; 32])).unwrap(),
        Arc::new(HeldSender::default()),
    ));
    let sender = message_binding('a');
    let target = message_binding('b');
    register_message_session(&store, sender.clone(), "sender").await;
    let target_session = register_message_session(&store, target.clone(), "receiver").await;
    let enrollment = store
        .registered_message_recipient(target.clone())
        .await
        .unwrap();

    let url = "https://example.com/callback";
    let mut request = subscribe_request(url);
    request["arguments"] = json!({"session_messages":true});
    let parse = |request: Value| serde_json::from_value::<EventsSubscribeParams>(request).unwrap();
    assert!(
        events
            .subscribe(
                target.recipient.clone(),
                Arc::new(EventAuthority::StaticBearer),
                parse(request.clone()),
                None,
            )
            .await
            .is_err()
    );

    let mut stale = enrollment.clone();
    stale.session_id = format!("session_{}", ulid::Ulid::new());
    assert!(
        events
            .subscribe(
                target.recipient.clone(),
                Arc::new(EventAuthority::StaticBearer),
                parse(request.clone()),
                Some(stale),
            )
            .await
            .is_err()
    );
    assert!(
        events.authority_keys().is_none(),
        "failed activation must discard its authorizer"
    );

    let subscribed = events
        .subscribe(
            target.recipient.clone(),
            Arc::new(EventAuthority::StaticBearer),
            parse(request),
            Some(enrollment.clone()),
        )
        .await
        .unwrap();
    let normal = owner("legacy-operator").recipient;
    let normal_sub = events
        .subscribe(
            normal.clone(),
            Arc::new(EventAuthority::StaticBearer),
            parse(subscribe_request(url)),
            None,
        )
        .await
        .unwrap();
    let normal_inbox = store.register(normal).await.unwrap();
    let ordinary = PublishNotice {
        inbox_id: enrollment.inbox_id,
        source: "ci".into(),
        level: NoticeLevel::Info,
        message: "Legacy notice".into(),
        dedupe_key: "legacy".into(),
        ttl_seconds: 3600,
    };
    let producer = NoticeProducer {
        actor: "fixture".into(),
        admin: true,
    };
    assert!(
        store
            .publish(producer.clone(), ordinary.clone())
            .await
            .is_err()
    );
    // Opposite-flag rows exercise the filter even with deliberately mixed SQL fixtures.
    store.run_event(|db| {
        db.execute_batch(
            "INSERT INTO agent_event_subscriptions
             (id,recipient,inbox_id,authority_key,callback,source,secret,secret_fingerprint,expires,generation,session_messages)
             SELECT id||'-opposite',recipient,inbox_id,authority_key,callback,source,secret,secret_fingerprint,expires,generation,NOT session_messages
             FROM agent_event_subscriptions;",
        )?;
        Ok(())
    }).await.unwrap();
    let normal_notice = store
        .publish(
            producer,
            PublishNotice {
                inbox_id: normal_inbox.id,
                ..ordinary
            },
        )
        .await
        .unwrap();

    let sent = store.message_request(sender, serde_json::from_value(json!({
        "action":"send", "params":{
            "to":{"kind":"session","session_id":target_session["session_id"]},
            "body":"Full body stays private", "summary":"Wake the session", "idempotency_key":"event",
        }
    })).unwrap()).await.unwrap();
    let rows = store
        .run_event(|db| {
            let mut statement = db.prepare(
                "SELECT d.subscription_id,n.event_body FROM agent_event_deliveries d
             JOIN agent_notices n ON n.id=d.notice_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    for (id, bytes) in rows {
        let payload: Value = serde_json::from_slice(&bytes).unwrap();
        if id == subscribed["id"] {
            assert_eq!(payload["data"]["message_id"], sent["message_id"]);
            assert_eq!(payload["data"]["message"], "Wake the session");
            assert!(
                !String::from_utf8(bytes)
                    .unwrap()
                    .contains("Full body stays private")
            );
        } else {
            assert_eq!(id, normal_sub["id"]);
            assert_eq!(payload["data"]["id"], normal_notice.id);
            assert!(payload["data"].get("message_id").is_none());
        }
    }
    let removed = store
        .message_request(
            target,
            serde_json::from_value(json!({
                "action":"ack", "params":{"message_ids":[sent["message_id"]]}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(removed["acknowledged"], json!([sent["message_id"]]));
    let stopped = store
        .run_event(move |db| {
            Ok(db.query_row(
                "SELECT status FROM agent_event_deliveries WHERE subscription_id=?",
                [subscribed["id"].as_str().unwrap()],
                |row| row.get::<_, String>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(stopped, "stopped");
}

#[tokio::test]
async fn late_subscription_commit_keeps_current_authority() {
    for rollback in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = NoticeStore::open_at(root.path()).unwrap();
        store
            .run_event(move |db| {
                db.busy_timeout(Duration::from_secs(1))?;
                if rollback {
                    db.execute_batch(
                        "CREATE TRIGGER forced_rejection BEFORE INSERT ON agent_event_subscriptions
                     BEGIN SELECT RAISE(ABORT,'fixture activation failure'); END;",
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();
        let lock = Connection::open(root.path().join("agent-notifications/inbox.sqlite3")).unwrap();
        lock.execute_batch("BEGIN IMMEDIATE").unwrap();
        let events = Arc::new(AgentEvents::new(
            store.clone(),
            load_key(&STANDARD.encode([b't'; 32])).unwrap(),
            Arc::new(HeldSender::default()),
        ));
        let params =
            serde_json::from_value(subscribe_request("https://example.com/callback")).unwrap();
        assert_eq!(
            events
                .subscribe(
                    owner("late-commit").recipient,
                    Arc::new(EventAuthority::StaticBearer),
                    params,
                    None,
                )
                .await,
            Err(EventError::Store(NoticeError::Deadline))
        );
        lock.execute_batch("COMMIT").unwrap();
        let active = store
            .run_event(|db| {
                use rusqlite::OptionalExtension as _;
                Ok(db
                    .query_row(
                        "SELECT authority_key FROM agent_event_subscriptions WHERE active=1",
                        [],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?)
            })
            .await
            .unwrap();
        if rollback {
            assert!(active.is_none() && events.authority_keys().is_none());
        } else {
            let authority = active.expect("transaction committed after response deadline");
            assert_eq!(
                events.check_authority(&authority).await,
                EventAuthorityDecision::Current
            );
        }
    }
}

#[derive(Default)]
struct PausedChallenge {
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl WebhookSender for PausedChallenge {
    fn verify<'a>(
        &'a self,
        _: &'a url::Url,
        _: &'a str,
        _: &'a WebhookSecret,
    ) -> futures::future::BoxFuture<'a, Result<(), EventError>> {
        Box::pin(async move {
            self.started.notify_one();
            self.release.notified().await;
            Ok(())
        })
    }

    fn send(
        &self,
        _: webhook::WebhookAttempt,
    ) -> futures::future::BoxFuture<'_, webhook::DeliveryOutcome> {
        Box::pin(async { panic!("no delivery should be admitted") })
    }
}

struct SwitchableAuthority(std::sync::atomic::AtomicBool);

impl EventAuthorizer for SwitchableAuthority {
    fn check(&self) -> futures::future::BoxFuture<'_, EventAuthorityDecision> {
        use std::sync::atomic::Ordering;
        let current = self.0.load(Ordering::Acquire);
        Box::pin(async move {
            if current {
                EventAuthorityDecision::Current
            } else {
                EventAuthorityDecision::Revoked
            }
        })
    }
}

#[tokio::test]
async fn message_subscription_rechecks_enrollment_and_grant_after_challenge() {
    use std::sync::atomic::{AtomicBool, Ordering};
    for revoke in [false, true] {
        let store = NoticeStore::memory().unwrap();
        let challenge = Arc::new(PausedChallenge::default());
        let events = Arc::new(AgentEvents::new(
            store.clone(),
            load_key(&STANDARD.encode([b't'; 32])).unwrap(),
            challenge.clone(),
        ));
        let binding = message_binding('b');
        register_message_session(&store, binding.clone(), "recipient").await;
        let enrollment = store
            .registered_message_recipient(binding.clone())
            .await
            .unwrap();
        let mut request = subscribe_request("https://example.com/callback");
        request["arguments"] = serde_json::json!({"session_messages":true});
        let authority = Arc::new(SwitchableAuthority(AtomicBool::new(true)));
        let subscriber = events.clone();
        let check = authority.clone();
        let inflight = tokio::spawn(async move {
            subscriber
                .subscribe(
                    binding.recipient,
                    check,
                    serde_json::from_value(request).unwrap(),
                    Some(enrollment),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), challenge.started.notified())
            .await
            .unwrap();
        let expected = if revoke {
            authority.0.store(false, Ordering::Release);
            NoticeError::Identity
        } else {
            store
                .run_event(|db| {
                    db.execute("UPDATE agent_message_sessions SET retained_until=0", [])?;
                    Ok(())
                })
                .await
                .unwrap();
            NoticeError::NotFound
        };
        challenge.release.notify_one();
        let result = tokio::time::timeout(Duration::from_secs(3), inflight)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result, Err(EventError::Store(expected)));
        assert!(events.authority_keys().is_none());
        let persisted = store
            .run_event(|db| {
                Ok(
                    db.query_row("SELECT count(*) FROM agent_event_subscriptions", [], |r| {
                        r.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert_eq!(persisted, 0);
    }
}
