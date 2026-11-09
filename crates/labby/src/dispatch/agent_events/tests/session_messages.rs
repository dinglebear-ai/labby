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

#[tokio::test]
async fn message_session_over_real_http_and_https() {
    use crate::dispatch::agent_messages::{http::HttpMessageRuntime, proof::MessageKey};
    use serde_json::{Value, json};
    use tokio_util::sync::CancellationToken;

    const TOKEN: &str = "fixture-http-event-token";
    const FIRST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SECOND: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    let root = tempfile::tempdir().unwrap();
    let store = NoticeStore::open_at(root.path()).unwrap();
    let events_store = NoticeStore::open_at(root.path()).unwrap();
    let (sender, callback, mut received, https) = webhook::tests::receiver(2).await;
    let events = Arc::new(AgentEvents::new(
        events_store,
        load_key(&STANDARD.encode([b't'; 32])).unwrap(),
        Arc::new(sender),
    ));
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            root.path().join("gateway.toml"),
            Default::default(),
        ),
    );
    let mut state = crate::api::AppState::new().with_gateway_manager(manager);
    state.agent_notifications = store;
    state.agent_events = Some(events.clone());
    state.agent_messages = Some(Arc::new(HttpMessageRuntime::new(
        MessageKey::from_test_bytes([8; 32]),
        "1".repeat(64),
    )));
    let router = crate::cli::serve::build_http_router(
        state,
        Some(TOKEN.to_owned()),
        None,
        &crate::config::McpPreferences::default(),
        &[],
        crate::mcp::peers::PeerNotifier::default(),
        true,
        false,
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let stop = CancellationToken::new();
    let stop_guard = stop.clone().drop_guard();
    let stop_http = stop.clone();
    let http = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(stop_http.cancelled_owned())
        .await
        .unwrap();
    });
    let worker = tokio::spawn(events.run(stop));
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let rpc = |method: &'static str, key: Option<&'static str>, mut params: Value| {
        let mut request = client
            .post(&url)
            .bearer_auth(TOKEN)
            .header("accept", "application/json, text/event-stream")
            .header("mcp-method", method)
            .header("mcp-protocol-version", "2026-07-28");
        if method == "tools/call" {
            request = request.header("mcp-name", "messages");
        }
        if let Some(key) = key {
            request = request.header("labby-session-key", key);
        }
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientInfo":{"name":"http-fixture","version":"1"},
            "io.modelcontextprotocol/clientCapabilities":{},
            "openai/session":"copied-fixture-marker",
        });
        async move {
            let response = request
                .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::OK);
            response.json::<Value>().await.unwrap()
        }
    };
    let tool = |key, action, params| {
        rpc(
            "tools/call",
            Some(key),
            json!({
                "name":"messages","arguments":{"action":action,"params":params}
            }),
        )
    };
    let result = |reply: Value| {
        assert!(
            reply["error"].is_null() && reply["result"]["structuredContent"].is_object(),
            "{reply}"
        );
        reply["result"]["structuredContent"].clone()
    };
    let first = result(tool(FIRST, "register", json!({"label":"sender"})).await);
    let args = json!({"session_messages":true});
    let subscription = json!({
        "name":EVENT_NAME,"arguments":args,
        "delivery":{"mode":"webhook","url":callback,"secret":format!("whsec_{}", STANDARD.encode([b'a';32]))}
    });
    let missing = rpc("events/subscribe", None, subscription.clone()).await;
    assert_eq!(missing["error"]["data"]["kind"], "forbidden", "{missing}");
    let unregistered = rpc("events/subscribe", Some(SECOND), subscription.clone()).await;
    assert_eq!(
        unregistered["error"]["data"]["kind"], "not_found",
        "{unregistered}"
    );

    let second = result(tool(SECOND, "register", json!({"label":"receiver"})).await);
    let listed = rpc("events/list", Some(SECOND), json!({})).await;
    assert_eq!(
        listed["result"]["events"][0]["inputSchema"]["properties"]["session_messages"]["type"],
        "boolean"
    );
    assert_eq!(
        listed["result"]["events"][0]["payloadSchema"]["properties"]["message_id"]["type"],
        "string"
    );
    let subscribed = rpc("events/subscribe", Some(SECOND), subscription.clone()).await;
    assert!(subscribed["result"]["id"].is_string(), "{subscribed}");
    let refreshed = rpc("events/subscribe", Some(SECOND), subscription).await;
    assert_eq!(
        refreshed["result"]["id"], subscribed["result"]["id"],
        "{refreshed}"
    );
    let (_, verification) = tokio::time::timeout(Duration::from_secs(3), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(verification["type"], "verification");

    let unsubscribe = json!({
        "name":EVENT_NAME,"arguments":args,"delivery":{"mode":"webhook","url":callback}
    });
    assert_eq!(
        rpc("events/unsubscribe", Some(FIRST), unsubscribe.clone()).await["result"],
        json!({})
    );
    let body = "The detailed diagnostic body stays in Labby. ".repeat(100);
    let sent = result(
        tool(
            FIRST,
            "send",
            json!({
                "to":{"kind":"session","session_id":second["session_id"]},
                "body":body,"summary":"Diagnostic is ready","idempotency_key":"real-wire"
            }),
        )
        .await,
    );
    let (_, push) = tokio::time::timeout(Duration::from_secs(5), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(push["data"]["message_id"], sent["message_id"]);
    assert_eq!(push["data"]["message"], "Diagnostic is ready");
    assert!(!push.to_string().contains(&body));
    tokio::time::timeout(Duration::from_secs(3), https)
        .await
        .unwrap()
        .unwrap();
    let unread = result(tool(SECOND, "unread", json!({})).await);
    assert_eq!(
        unread["messages"][0]["message_id"], sent["message_id"],
        "{unread}"
    );
    let full = result(tool(SECOND, "read", json!({"message_id":sent["message_id"]})).await);
    assert_eq!(full["sender_session_id"], first["session_id"]);
    assert_eq!(full["body"], body);
    let ack = result(tool(SECOND, "ack", json!({"message_ids":[sent["message_id"]]})).await);
    assert_eq!(ack["acknowledged"], json!([sent["message_id"]]));
    assert_eq!(
        rpc("events/unsubscribe", Some(SECOND), unsubscribe).await["result"],
        json!({})
    );
    drop(stop_guard);
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), http)
        .await
        .unwrap()
        .unwrap();
}
