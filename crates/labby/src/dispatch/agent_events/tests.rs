use super::super::codemode_notices::{NoticeConsumer, NoticeError, NoticeRecipient, NoticeStore};
use super::subscriptions::activate;
use super::*;
use rusqlite::Connection;

pub(super) fn owner(actor: &str) -> EventOwnerBinding {
    EventOwnerBinding {
        recipient: NoticeRecipient {
            actor: Some(actor.into()),
            route: "root".into(),
            consumer: NoticeConsumer::Client {
                id: "client".into(),
                conversation: None,
            },
        },
        authority_key: "test-process".into(),
    }
}

pub(super) fn input(source: &str, endpoint: usize) -> ValidatedSubscription {
    ValidatedSubscription {
        target: SubscriptionTarget::new(
            EVENT_NAME,
            EventFilter {
                source: Some(source.into()),
                session_messages: false,
            },
            "webhook",
            &format!("https://example.com/callback/{endpoint}"),
        )
        .unwrap(),
        enrollment: None,
        sealed_key: vec![1],
        key_fingerprint: "test".into(),
        expires: 3_600_001,
    }
}

#[tokio::test]
async fn foreign_subscription_is_not_visible() {
    let store = NoticeStore::memory().unwrap();
    let sub = store
        .activate_event_subscription(owner("alice"), input("CI", 1), 1)
        .await
        .unwrap();
    store
        .remove_event_subscription(owner("bob"), input("CI", 1).target, 2, |_| {})
        .await
        .unwrap();
    assert!(
        store
            .run(move |db| Ok(db.query_row(
                "SELECT active=1 AND expires=3600001 FROM agent_event_subscriptions WHERE id=?",
                [sub.id],
                |row| row.get::<_, bool>(0),
            )?))
            .await
            .unwrap()
    );
    store
        .remove_event_subscription(owner("alice"), input("CI", 1).target, 3, |_| {})
        .await
        .unwrap();
    store
        .remove_event_subscription(owner("alice"), input("CI", 1).target, 4, |_| {})
        .await
        .unwrap();
    assert_eq!(
        store
            .run(|db| Ok(db.query_row(
                "SELECT count(*) FROM agent_event_subscriptions",
                [],
                |row| row.get::<_, u32>(0)
            )?))
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn quota_is_enforced_per_inbox_and_globally() {
    let store = NoticeStore::memory().unwrap();
    // One DB permit, one blocking task. Each activate still uses the same
    // transaction as a production request.
    store.run(|db: &mut Connection| {
        for n in 0..8 {
            activate(db, &owner("actor-0"), &input("ci", n), 1)?;
        }
        let error = activate(db, &owner("actor-0"), &input("ci", 8), 2).unwrap_err();
        assert_eq!(error, NoticeError::Full);
        // Seed unrelated inboxes in bulk, then exercise the production
        // insertion at the global boundary without a test-only timeout.
        let tx = db.transaction()?;
        tx.execute_batch(
            "WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<1991)
             INSERT INTO agent_inboxes
             SELECT 'inbox-'||n,'owner-'||n,'authenticated_client',3600001 FROM seq;
             INSERT INTO agent_event_subscriptions
             (id,recipient,inbox_id,authority_key,callback,secret,secret_fingerprint,expires,generation)
             SELECT 'sub-'||id,recipient,id,'fixture','https://example.com/',X'00','fixture',3600001,1
             FROM agent_inboxes WHERE id LIKE 'inbox-%';",
        )?;
        tx.commit()?;
        activate(db, &owner("last"), &input("ci", 0), 2)?;
        let error = activate(db, &owner("another"), &input("ci", 0), 2).unwrap_err();
        assert_eq!(error, NoticeError::Full);
        Ok(())
    }).await.unwrap();
}

#[test]
fn canonical_subscription_identity_is_recipient_scoped() {
    let owner = owner("alice");
    let target =
        |url| SubscriptionTarget::new(EVENT_NAME, EventFilter::default(), "webhook", url).unwrap();
    assert_eq!(
        target("https://EXAMPLE.COM:443")
            .id(&owner.recipient)
            .unwrap(),
        target("https://example.com/").id(&owner.recipient).unwrap()
    );
    assert_ne!(
        target("https://example.com/").id(&owner.recipient).unwrap(),
        input("ci", 0).target.id(&owner.recipient).unwrap()
    );
}

#[test]
fn subscription_ttl_distinguishes_omitted_null_and_integer() {
    let mut params = subscribe_request("https://example.com/notice");
    let ttl = |params: serde_json::Value| {
        serde_json::from_value::<EventsSubscribeParams>(params)
            .unwrap()
            .validate()
            .unwrap()
            .2
    };
    assert_eq!(ttl(params.clone()), 3_600_000);
    for (requested, expected) in [
        (serde_json::json!(1), 60_000),
        (serde_json::json!(86_400_001), 86_400_000),
        (serde_json::Value::Null, 86_400_000),
    ] {
        params["ttlMs"] = requested;
        assert_eq!(ttl(params.clone()), expected);
    }
    let mut null_filter = params.clone();
    null_filter["arguments"]["source"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<EventsSubscribeParams>(null_filter).is_err());
    params["ttlMs"] = serde_json::json!(-1);
    assert!(serde_json::from_value::<EventsSubscribeParams>(params.clone()).is_err());
    params["ttlMs"] = serde_json::json!(0);
    assert!(
        serde_json::from_value::<EventsSubscribeParams>(params)
            .unwrap()
            .validate()
            .is_err()
    );
}

fn subscribe_request(url: &str) -> serde_json::Value {
    subscribe_request_with_secret(url, &format!("whsec_{}", STANDARD.encode([b'a'; 32])))
}

fn subscribe_request_with_secret(url: &str, secret: &str) -> serde_json::Value {
    serde_json::json!({
        "name": EVENT_NAME,
        "arguments":{"source":"ci"},
        "delivery":{
            "mode":"webhook", "url": url,
            "secret":secret
        },
    })
}

#[tokio::test]
async fn mcp_events_through_http_auth_and_signed_delivery() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use serde_json::{Value, json};
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt as _;

    let directory = tempfile::tempdir().unwrap();
    let publishing = NoticeStore::open_at(directory.path()).unwrap();
    let working = NoticeStore::open_at(directory.path()).unwrap();
    let (sender, callback, secret, mut received, https) = webhook::tests::receiver(3).await;
    let encryption = load_key(&STANDARD.encode([b't'; 32])).unwrap();
    let events = Arc::new(AgentEvents::new(working, encryption, Arc::new(sender)));
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            directory.path().join("gateway.toml"),
            Default::default(),
        ),
    );
    let mut state = crate::api::AppState::new().with_gateway_manager(manager);
    state.agent_notifications = publishing.clone();
    let token = ulid::Ulid::new().to_string();
    let make_app = |state| {
        crate::cli::serve::build_http_router(
            state,
            Some(token.clone()),
            None,
            &crate::config::McpPreferences::default(),
            &[],
            crate::mcp::peers::PeerNotifier::default(),
            true,
            false,
        )
        .unwrap()
    };
    let disabled = make_app(state.clone());
    state.agent_events = Some(events.clone());
    let enabled = make_app(state);
    let send = |app: axum::Router,
                path: &'static str,
                method: &'static str,
                params: Value,
                authenticated: bool,
                modern: bool| {
        let token = token.clone();
        async move {
            let mut request = Request::builder()
                .method("POST")
                .uri(path)
                .header("host", "localhost")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream");
            if authenticated {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            if modern {
                request = request
                    .header("mcp-protocol-version", "2026-07-28")
                    .header("mcp-method", method);
            }
            let body = if path == "/mcp" {
                json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
            } else {
                params
            };
            let response = app
                .oneshot(request.body(Body::from(body.to_string())).unwrap())
                .await
                .unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        }
    };
    let init = json!({"protocolVersion":"2025-11-25","clientInfo":{"name":"fixture","version":"1"},"capabilities":{}});
    let (status, legacy) = send(
        enabled.clone(),
        "/mcp",
        "initialize",
        init.clone(),
        true,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{legacy}");
    assert_eq!(
        legacy["result"]["capabilities"]["events"],
        json!({}),
        "{legacy}"
    );
    let (_, baseline) = send(disabled, "/mcp", "initialize", init, true, false).await;
    assert!(baseline["result"]["capabilities"].get("events").is_none());
    let modern_meta = |conversation: &str| {
        json!({
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientInfo":{"name":"fixture","version":"1"},
            "io.modelcontextprotocol/clientCapabilities":{},
            "openai/session":conversation,
        })
    };
    let (_, discover) = send(
        enabled.clone(),
        "/mcp",
        "server/discover",
        json!({"_meta":modern_meta("c1")}),
        true,
        true,
    )
    .await;
    assert_eq!(
        discover["result"]["capabilities"]["events"],
        json!({}),
        "{discover}"
    );
    let (_, list) = send(
        enabled.clone(),
        "/mcp",
        "events/list",
        json!({"_meta":modern_meta("c1")}),
        true,
        true,
    )
    .await;
    assert_eq!(list["result"]["events"][0]["name"], EVENT_NAME, "{list}");
    assert_eq!(list["result"]["events"][0]["delivery"], json!(["webhook"]));
    let mut subscribe = subscribe_request_with_secret(&callback, &secret);
    subscribe["_meta"] = modern_meta("c1");
    let (status, _) = send(
        enabled.clone(),
        "/mcp",
        "events/subscribe",
        subscribe.clone(),
        false,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let mut foreign_argument = subscribe.clone();
    foreign_argument["arguments"]["inbox_id"] = json!("inbox_forged");
    let (_, denied) = send(
        enabled.clone(),
        "/mcp",
        "events/subscribe",
        foreign_argument,
        true,
        true,
    )
    .await;
    assert_eq!(denied["error"]["code"], -32602, "{denied}");
    let mut private_callback = subscribe.clone();
    private_callback["delivery"]["url"] = json!("https://[::ffff:127.0.0.1]/");
    let (_, denied) = send(
        enabled.clone(),
        "/mcp",
        "events/subscribe",
        private_callback,
        true,
        true,
    )
    .await;
    assert_eq!(denied["error"]["code"], -32015, "{denied}");
    assert!(received.try_recv().is_err());
    let (_, accepted) = send(
        enabled.clone(),
        "/mcp",
        "events/subscribe",
        subscribe.clone(),
        true,
        true,
    )
    .await;
    let id = accepted["result"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("subscribe failed: {accepted}"));
    assert_eq!(accepted["result"]["truncated"], false);
    assert!(
        accepted["result"]["refreshBefore"]
            .as_str()
            .unwrap()
            .parse::<jiff::Timestamp>()
            .is_ok()
    );
    let (_, verification) = received.try_recv().unwrap();
    assert_eq!(verification["type"], "verification");
    let (_, refreshed) = send(
        enabled.clone(),
        "/mcp",
        "events/subscribe",
        subscribe.clone(),
        true,
        true,
    )
    .await;
    assert_eq!(refreshed["result"]["id"], id, "{refreshed}");
    assert!(
        received.try_recv().is_err(),
        "same subscription and key reused verification"
    );
    let mut changed_filter = subscribe;
    changed_filter["arguments"]["source"] = json!("security");
    let (_, distinct) = send(
        enabled.clone(),
        "/mcp",
        "events/subscribe",
        changed_filter.clone(),
        true,
        true,
    )
    .await;
    let distinct_id = distinct["result"]["id"]
        .as_str()
        .expect("distinct subscription");
    assert_ne!(distinct_id, id);
    let (_, verification) = received
        .try_recv()
        .expect("new subscription ID was challenged");
    assert_eq!(verification["type"], "verification");
    changed_filter["delivery"]
        .as_object_mut()
        .unwrap()
        .remove("secret");
    let (_, closed_filter) = send(
        enabled.clone(),
        "/mcp",
        "events/unsubscribe",
        changed_filter,
        true,
        true,
    )
    .await;
    assert_eq!(closed_filter["result"], json!({}));
    let mut unsubscribe = json!({
        "name":EVENT_NAME, "arguments":{"source":"ci"}, "delivery":{"mode":"webhook","url":callback},
        "_meta":modern_meta("other-conversation"),
    });
    let (_, foreign) = send(
        enabled.clone(),
        "/mcp",
        "events/unsubscribe",
        unsubscribe.clone(),
        true,
        true,
    )
    .await;
    assert_eq!(foreign["result"], json!({}));
    let inbox = publishing
        .run(|db| {
            Ok(db.query_row(
                "SELECT inbox_id FROM agent_event_subscriptions WHERE active=1 AND generation=2",
                [],
                |row| row.get::<_, String>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(
        accepted["result"]["_meta"]["ai.dinglebear.labby/inboxId"],
        inbox
    );
    let cancel = CancellationToken::new();
    let guard = cancel.clone().drop_guard();
    let worker = tokio::spawn(events.run(cancel));
    let publish = |source, key| {
        json!({
            "inbox_id":inbox,"source":source,"level":"info","message":"CI completed.","dedupe_key":key
        })
    };
    let (status, filtered) = send(
        enabled.clone(),
        "/v1/notifications/agent",
        "",
        publish("logs", "filtered"),
        true,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{filtered}");
    let (status, published) = send(
        enabled.clone(),
        "/v1/notifications/agent",
        "",
        publish("ci", "matching"),
        true,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{published}");
    let (_, delivered) = tokio::time::timeout(Duration::from_secs(5), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivered["eventId"], published["id"]);
    assert_eq!(delivered["data"]["source"], "ci");
    assert_eq!(delivered["data"]["message"], "CI completed.");
    https.await.unwrap();
    unsubscribe["_meta"] = modern_meta("c1");
    let (_, removed) = send(
        enabled.clone(),
        "/mcp",
        "events/unsubscribe",
        unsubscribe,
        true,
        true,
    )
    .await;
    assert_eq!(removed["result"], json!({}), "{removed}");
    let (status, _) = send(
        enabled,
        "/v1/notifications/agent",
        "",
        publish("ci", "after"),
        true,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    drop(guard);
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        publishing
            .run(|db| Ok(db.query_row(
                "SELECT count(*) FROM agent_event_subscriptions WHERE active=1",
                [],
                |row| row.get::<_, u32>(0),
            )?))
            .await
            .unwrap(),
        0
    );
}

#[derive(Default)]
struct HeldSender {
    started: tokio::sync::Notify,
    dropped: tokio::sync::Notify,
}

impl WebhookSender for HeldSender {
    fn verify<'a>(
        &'a self,
        _: &'a url::Url,
        _: &'a str,
        _: &'a WebhookSecret,
    ) -> futures::future::BoxFuture<'a, Result<(), EventError>> {
        Box::pin(async { Ok(()) })
    }

    fn send(
        &self,
        _: webhook::WebhookAttempt,
    ) -> futures::future::BoxFuture<'_, webhook::DeliveryOutcome> {
        Box::pin(async move {
            struct OnDrop<'a>(&'a tokio::sync::Notify);
            impl Drop for OnDrop<'_> {
                fn drop(&mut self) {
                    self.0.notify_one();
                }
            }
            let _notify = OnDrop(&self.dropped);
            self.started.notify_one();
            std::future::pending().await
        })
    }
}

#[tokio::test]
async fn unsubscribe_cancels_in_flight_request_and_joins_worker() {
    let directory = tempfile::tempdir().unwrap();
    let publisher = NoticeStore::open_at(directory.path()).unwrap();
    let working = NoticeStore::open_at(directory.path()).unwrap();
    let sender = Arc::new(HeldSender::default());
    let events = Arc::new(AgentEvents::new(
        working,
        load_key(&STANDARD.encode([b't'; 32])).unwrap(),
        sender.clone(),
    ));
    let who = owner("cancel-owner");
    let url = "https://example.com/callback";
    let params = serde_json::from_value(subscribe_request(url)).unwrap();
    events
        .subscribe(
            who.recipient.clone(),
            Arc::new(EventAuthority::StaticBearer),
            params,
            None,
        )
        .await
        .unwrap();
    let inbox = publisher.register(who.recipient.clone()).await.unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let guard = cancel.clone().drop_guard();
    let worker = tokio::spawn(events.clone().run(cancel));
    publisher
        .publish(
            super::super::codemode_notices::NoticeProducer {
                actor: "fixture".into(),
                admin: true,
            },
            super::super::codemode_notices::PublishNotice {
                inbox_id: inbox.id,
                source: "ci".into(),
                level: super::super::codemode_notices::NoticeLevel::Info,
                message: "CI ready.".into(),
                dedupe_key: "cancel".into(),
                ttl_seconds: 3600,
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), sender.started.notified())
        .await
        .unwrap();
    events.unsubscribe(who.recipient, serde_json::from_value(serde_json::json!({
        "name":EVENT_NAME, "arguments":{"source":"ci"}, "delivery":{"mode":"webhook","url":url}
    })).unwrap()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), sender.dropped.notified())
        .await
        .unwrap();
    drop(guard);
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn receiver_can_ack_before_http_response() {
    use super::super::codemode_notices::{NoticeLevel, NoticeProducer, PublishNotice};
    let directory = tempfile::tempdir().unwrap();
    let publisher = NoticeStore::open_at(directory.path()).unwrap();
    let worker = NoticeStore::open_at(directory.path()).unwrap();
    let recipient = owner("early-ack").recipient;
    let (sender, callback, secret, mut requests, receiver) =
        webhook::tests::receiver_with_offer_check(2, Some((publisher.clone(), recipient.clone())))
            .await;
    let events = Arc::new(AgentEvents::new(
        worker,
        load_key(&STANDARD.encode([b't'; 32])).unwrap(),
        Arc::new(sender),
    ));
    let params = serde_json::from_value(subscribe_request_with_secret(&callback, &secret)).unwrap();
    events
        .subscribe(
            recipient.clone(),
            Arc::new(EventAuthority::StaticBearer),
            params,
            None,
        )
        .await
        .unwrap();
    let (_, verification) = requests.recv().await.unwrap();
    assert_eq!(verification["type"], "verification");
    let inbox = publisher.register(recipient.clone()).await.unwrap();
    let notice = publisher
        .publish(
            NoticeProducer {
                actor: "fixture".into(),
                admin: true,
            },
            PublishNotice {
                inbox_id: inbox.id,
                source: "ci".into(),
                level: NoticeLevel::Info,
                message: "Build finished.".into(),
                dedupe_key: "early".into(),
                ttl_seconds: 3600,
            },
        )
        .await
        .unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let guard = cancel.clone().drop_guard();
    let work = tokio::spawn(events.run(cancel));
    tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .unwrap()
        .unwrap();
    let (_, event) = requests.recv().await.unwrap();
    assert_eq!(event["eventId"], notice.id);
    drop(guard);
    tokio::time::timeout(Duration::from_secs(3), work)
        .await
        .unwrap()
        .unwrap();
    assert!(
        publisher
            .deliver(recipient, |v| Some(v.notifications.clone()))
            .await
            .unwrap()
            .is_none()
    );
}

mod session_messages;

#[tokio::test]
async fn revocation_between_attempt_and_body_release_blocks_the_offer() {
    use super::super::codemode_notices::{NoticeLevel, NoticeProducer, PublishNotice};
    use super::webhook::{DeliveryOutcome, WebhookAttempt, WebhookSender};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio_util::sync::CancellationToken;

    struct Grant(AtomicBool);
    impl EventAuthorizer for Grant {
        fn check(&self) -> futures::future::BoxFuture<'_, EventAuthorityDecision> {
            Box::pin(async {
                if self.0.load(Ordering::Acquire) {
                    EventAuthorityDecision::Current
                } else {
                    EventAuthorityDecision::Revoked
                }
            })
        }
    }
    #[derive(Default)]
    struct BodyGate {
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
        settled: tokio::sync::Notify,
        exposed: AtomicBool,
    }
    impl WebhookSender for BodyGate {
        fn verify<'a>(
            &'a self,
            _: &'a url::Url,
            _: &'a str,
            _: &'a WebhookSecret,
        ) -> futures::future::BoxFuture<'a, Result<(), EventError>> {
            Box::pin(async { Ok(()) })
        }

        fn send(
            &self,
            mut attempt: WebhookAttempt,
        ) -> futures::future::BoxFuture<'_, DeliveryOutcome> {
            Box::pin(async move {
                self.started.notify_one();
                self.release.notified().await;
                let authorized = attempt.offer.take().unwrap()(CancellationToken::new())
                    .await
                    .is_ok();
                self.exposed.store(authorized, Ordering::Release);
                self.settled.notify_one();
                DeliveryOutcome::Stop
            })
        }
    }

    let store = NoticeStore::memory().unwrap();
    let gate = Arc::new(BodyGate::default());
    let grant = Arc::new(Grant(AtomicBool::new(true)));
    let events = Arc::new(AgentEvents::new(
        store.clone(),
        load_key(&STANDARD.encode([b't'; 32])).unwrap(),
        gate.clone(),
    ));
    let recipient = owner("revoked-during-connect").recipient;
    let params = serde_json::from_value(subscribe_request("https://example.com/events")).unwrap();
    let enrolled = events
        .subscribe(recipient.clone(), grant.clone(), params, None)
        .await
        .unwrap();
    let notice = store
        .publish(
            NoticeProducer {
                actor: "fixture".into(),
                admin: true,
            },
            PublishNotice {
                inbox_id: enrolled["_meta"]["ai.dinglebear.labby/inboxId"]
                    .as_str()
                    .unwrap()
                    .into(),
                source: "ci".into(),
                level: NoticeLevel::Info,
                message: "Private build notice.".into(),
                dedupe_key: "revoked".into(),
                ttl_seconds: 60,
            },
        )
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let guard = cancel.clone().drop_guard();
    let worker = tokio::spawn(events.run(cancel));
    tokio::time::timeout(Duration::from_secs(3), gate.started.notified())
        .await
        .unwrap();
    grant.0.store(false, Ordering::Release);
    gate.release.notify_one();
    tokio::time::timeout(Duration::from_secs(3), gate.settled.notified())
        .await
        .unwrap();
    assert!(!gate.exposed.load(Ordering::Acquire));
    drop(guard);
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    let (_, ack) = store
        .controls(recipient, false, Some(vec![notice.id]))
        .await
        .unwrap();
    assert_eq!(ack, Some(vec![]));
}
