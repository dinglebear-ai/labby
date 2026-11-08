use super::store::{execute_in, renew_in};
use super::*;
use crate::dispatch::codemode_notices::{NoticeConsumer, NoticeRecipient, NoticeStore};
use rusqlite::{Connection, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc, sync::atomic::AtomicBool};

struct Fixture {
    root: tempfile::TempDir,
    db: Connection,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        drop(NoticeStore::open_at(root.path()).unwrap());
        let db = connect(root.path());
        Self { root, db }
    }

    fn request(
        &mut self,
        binding: &SessionBinding,
        action: &str,
        params: Value,
        now: i64,
    ) -> Result<Value, NoticeError> {
        let request = serde_json::from_value(json!({ "action": action, "params": params }))
            .map_err(|_| NoticeError::Invalid)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = execute_in(&tx, binding, request, now)?;
        tx.commit()?;
        Ok(result.value)
    }

    fn register(&mut self, binding: &SessionBinding, label: &str, now: i64) -> String {
        self.request(binding, "register", json!({"label": label}), now)
            .unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn inbox(&self, session_id: &str) -> String {
        self.db
            .query_row(
                "SELECT inbox_id FROM agent_message_sessions WHERE id=?",
                [session_id],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn counts(&self) -> (i64, i64, i64) {
        self.db
            .query_row(
                "SELECT (SELECT count(*) FROM agent_messages),
            (SELECT count(*) FROM agent_notices), (SELECT count(*) FROM agent_event_deliveries)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    }
}

fn connect(root: &Path) -> Connection {
    let db = Connection::open(root.join("agent-notifications/inbox.sqlite3")).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    db
}

fn identity(owner: &str, session: &str, kind: SessionKind) -> SessionBinding {
    let owner_key = hex::encode(Sha256::digest(owner));
    let binding_key = hex::encode(Sha256::digest(format!("{owner}/{session}")));
    let (actor, consumer, stdio_closed) = match kind {
        SessionKind::Http => (
            Some(owner_key.clone()),
            NoticeConsumer::MessageSession {
                binding: binding_key.clone(),
            },
            None,
        ),
        SessionKind::Stdio => (
            None,
            NoticeConsumer::Stdio {
                session: binding_key.clone(),
            },
            Some(Arc::new(AtomicBool::new(false))),
        ),
    };
    SessionBinding {
        owner_key,
        binding_key,
        kind,
        recipient: NoticeRecipient {
            actor,
            route: "root".into(),
            consumer,
        },
        stdio_closed,
    }
}

fn target(id: &str) -> Value {
    json!({"kind":"session","session_id":id})
}

fn send(to: Value, key: &str, body: &str) -> Value {
    json!({"to":to,"body":body,"idempotency_key":key})
}

fn receipt_id(receipt: &Value) -> String {
    receipt["message_id"].as_str().unwrap().to_owned()
}

#[test]
fn register_is_stable_for_exact_binding() {
    let mut fixture = Fixture::new();
    let a = identity("owner", "a", SessionKind::Http);
    let b = identity("owner", "b", SessionKind::Http);
    let first = fixture.register(&a, "debug", 100);
    assert!(first.starts_with("session_"));
    assert_eq!(fixture.register(&a, "renamed", 200), first);
    let second = fixture.register(&b, "renamed", 200);
    assert_ne!(first, second);
    let dir = fixture.request(&a, "sessions", json!({}), 201).unwrap();
    assert_eq!(dir["sessions"].as_array().unwrap().len(), 2);
    assert!(fixture.request(&a, "send", json!({
        "to":target(&second), "body":"spoof", "idempotency_key":"fake", "from_session_id":second,
    }), 202).is_err());
}

#[test]
fn broadcast_snapshots_my_active_sessions_atomically() {
    let mut fixture = Fixture::new();
    let a = identity("owner", "a", SessionKind::Http);
    let b = identity("owner", "b", SessionKind::Stdio);
    let old = identity("owner", "old", SessionKind::Http);
    let closed = identity("owner", "closed", SessionKind::Stdio);
    let foreign = identity("foreign", "x", SessionKind::Http);
    let now = 1_000_000;
    let a_id = fixture.register(&a, "sending session", now - 300_001);
    let b_id = fixture.register(&b, "connected peer", now - 1);
    let old_id = fixture.register(&old, "only callback remains", now - 300_001);
    let closed_id = fixture.register(&closed, "disconnected", now - 1);
    let foreign_id = fixture.register(&foreign, "other owner", now);
    let old_inbox = fixture.inbox(&old_id);
    fixture.db.execute(
        "INSERT INTO agent_event_subscriptions
         (id,recipient,inbox_id,authority_key,callback,secret,secret_fingerprint,expires,generation,session_messages)
         SELECT 'idle-callback',recipient,id,'old-authority','https://example.com',X'11','fp',?2,1,1
         FROM agent_inboxes WHERE id=?1",
        params![old_inbox, now + 600_000],
    ).unwrap();
    let tx = fixture.db.transaction().unwrap();
    renew_in(&tx, &closed, true, now).unwrap();
    tx.commit().unwrap();
    let receipt = fixture
        .request(
            &a,
            "send",
            send(json!({"kind":"all_active"}), "hello", "test"),
            now,
        )
        .unwrap();
    let mut expected = vec![a_id, b_id];
    expected.sort();
    assert_eq!(receipt["recipient_ids"], json!(expected));
    assert!(
        receipt["recipient_ids"]
            .as_array()
            .unwrap()
            .iter()
            .all(|id| id != &json!(old_id) && id != &json!(closed_id) && id != &json!(foreign_id))
    );
    let id = receipt_id(&receipt);
    fixture
        .request(&a, "ack", json!({"message_ids":[id]}), now + 1)
        .unwrap();
    assert_eq!(
        fixture.request(&b, "unread", json!({}), now + 1).unwrap()["messages"][0]["message_id"],
        id
    );
}

#[test]
fn direct_recipients_own_body_and_ack() {
    let mut fixture = Fixture::new();
    let a = identity("owner", "a", SessionKind::Http);
    let b = identity("owner", "b", SessionKind::Http);
    let sibling = identity("owner", "c", SessionKind::Http);
    let foreign = identity("foreign", "b", SessionKind::Http);
    let from = fixture.register(&a, "build terminal", 100);
    let to = fixture.register(&b, "debug terminal", 100);
    fixture.register(&sibling, "sibling", 100);
    fixture.register(&foreign, "foreign", 100);
    let text = "🦀\n\t".repeat(2730) + "abcd";
    assert_eq!(text.len(), 16_384);
    let id = receipt_id(
        &fixture
            .request(&a, "send", send(target(&to), "direct", &text), 200)
            .unwrap(),
    );
    let arg = json!({"message_id":id});
    for denied in [&a, &sibling, &foreign] {
        assert_eq!(
            fixture.request(denied, "read", arg.clone(), 201),
            Err(NoticeError::NotFound)
        );
        let ack = fixture
            .request(denied, "ack", json!({"message_ids":[id]}), 201)
            .unwrap();
        assert_eq!(ack["acknowledged"], json!([]));
    }
    let read = fixture.request(&b, "read", arg, 201).unwrap();
    assert_eq!(read["body"], text);
    assert_eq!(read["sender_session_id"], from);
    assert_eq!(read["sender_label"], "build terminal");
    let unread = fixture
        .request(&b, "unread", json!({"limit":1}), 202)
        .unwrap();
    assert_eq!(unread["messages"][0]["message_id"], id);
    assert!(unread["messages"][0].get("body").is_none());
    let ack = fixture
        .request(&b, "ack", json!({"message_ids":[id]}), 203)
        .unwrap();
    assert_eq!(ack["acknowledged"], json!([id]));
    assert_eq!(
        fixture.request(&b, "unread", json!({}), 204).unwrap()["messages"],
        json!([])
    );
    assert_eq!(
        fixture
            .request(&b, "read", json!({"message_id":id}), 205)
            .unwrap()["body"],
        text
    );
}

#[test]
fn retry_keeps_original_targets() {
    let mut fixture = Fixture::new();
    let a = identity("owner", "a", SessionKind::Http);
    let b = identity("owner", "b", SessionKind::Http);
    let c = identity("owner", "c", SessionKind::Http);
    let a_id = fixture.register(&a, "sender", 100);
    fixture.register(&b, "peer", 100);
    let request = send(json!({"kind":"all_active"}), "stable", "original body");
    let original = fixture.request(&a, "send", request.clone(), 200).unwrap();
    assert_eq!(original["duplicate"], false);
    fixture.db = connect(fixture.root.path());
    assert_eq!(fixture.register(&a, "renamed after restart", 300), a_id);
    let c_id = fixture.register(&c, "new peer", 300);
    let retry = fixture.request(&a, "send", request.clone(), 300).unwrap();
    assert_eq!(retry["duplicate"], true);
    assert_eq!(retry["message_id"], original["message_id"]);
    assert_eq!(retry["recipient_ids"], original["recipient_ids"]);
    assert_eq!(fixture.counts(), (1, 2, 0));
    for changed in [
        send(target(&c_id), "stable", "original body"),
        send(json!({"kind":"all_active"}), "stable", "changed body"),
        json!({"to":{"kind":"all_active"},"idempotency_key":"stable",
            "body":"original body","summary":"different"}),
    ] {
        assert_eq!(
            fixture.request(&a, "send", changed, 301),
            Err(NoticeError::Conflict)
        );
    }
    assert_eq!(fixture.counts(), (1, 2, 0));
}

#[test]
fn failed_broadcast_has_no_side_effects() {
    let mut fixture = Fixture::new();
    let a = identity("owner", "a", SessionKind::Http);
    let b = identity("owner", "b", SessionKind::Http);
    let a_id = fixture.register(&a, "sender", 100);
    let b_id = fixture.register(&b, "peer", 100);
    let later = fixture.inbox(if a_id < b_id { &b_id } else { &a_id });
    for (subscription, session) in [("first", &a_id), ("second", &b_id)] {
        let inbox = fixture.inbox(session);
        fixture.db.execute(
            "INSERT INTO agent_event_subscriptions
              (id,recipient,inbox_id,authority_key,callback,secret,secret_fingerprint,expires,generation,session_messages)
             SELECT ?1,recipient,id,'auth','https://example.com',X'11','fp',30000,1,1
             FROM agent_inboxes WHERE id=?2",
            params![subscription, inbox],
        ).unwrap();
    }
    fixture.db.execute_batch(&format!(
        "CREATE TRIGGER reject_second BEFORE INSERT ON agent_event_deliveries
         WHEN (SELECT inbox_id FROM agent_event_subscriptions WHERE id=NEW.subscription_id)='{later}'
         BEGIN SELECT RAISE(ABORT, 'fixture rollback'); END;"
    )).unwrap();
    let broadcast = || send(json!({"kind":"all_active"}), "atomic", "body");
    assert!(fixture.request(&a, "send", broadcast(), 200).is_err());
    assert_eq!(fixture.counts(), (0, 0, 0));
    fixture
        .db
        .execute_batch("DROP TRIGGER reject_second;")
        .unwrap();
    for i in 0..32 {
        fixture
            .request(
                &a,
                "send",
                send(target(&b_id), &format!("fill-{i}"), "fill"),
                200,
            )
            .unwrap();
    }
    let before = fixture.counts();
    assert_eq!(
        fixture.request(&a, "send", broadcast(), 201),
        Err(NoticeError::Full)
    );
    assert_eq!(fixture.counts(), before);

    let mut near_full = Fixture::new();
    let id = near_full.register(&a, "only sender", 100);
    let pages = near_full
        .db
        .pragma_query_value(None, "page_count", |r| r.get::<_, u32>(0))
        .unwrap();
    near_full
        .db
        .pragma_update(None, "max_page_count", pages)
        .unwrap();
    let text = "x".repeat(16_384);
    assert_eq!(
        near_full.request(&a, "send", send(target(&id), "full", &text), 200),
        Err(NoticeError::Full)
    );
    assert_eq!(near_full.counts(), (0, 0, 0));
}

#[test]
fn logical_expiry_releases_key_while_lease_pins_body() {
    let mut fixture = Fixture::new();
    let a = identity("owner", "a", SessionKind::Http);
    let b = identity("owner", "b", SessionKind::Http);
    let a_id = fixture.register(&a, "sender", 100);
    let b_id = fixture.register(&b, "peer", 100);
    let expiry = 200 + 86_400_000;
    let inbox = fixture.inbox(&b_id);
    fixture.db.execute(
        "INSERT INTO agent_event_subscriptions
          (id,recipient,inbox_id,authority_key,callback,secret,secret_fingerprint,expires,generation,session_messages)
         SELECT 'held',recipient,id,'auth','https://example.com',X'11','fp',?2,1,1
         FROM agent_inboxes WHERE id=?1",
        params![inbox, expiry],
    ).unwrap();
    let old = receipt_id(
        &fixture
            .request(&a, "send", send(target(&b_id), "key", "first"), 200)
            .unwrap(),
    );
    assert_eq!(fixture.counts(), (1, 1, 1));
    fixture
        .db
        .execute(
            "UPDATE agent_event_deliveries SET claim_token='live',claim_until=?",
            [expiry + 1000],
        )
        .unwrap();
    assert_eq!(fixture.register(&a, "sender", expiry - 1), a_id);
    assert_eq!(fixture.register(&b, "peer", expiry - 1), b_id);
    let new = receipt_id(
        &fixture
            .request(&a, "send", send(target(&b_id), "key", "second"), expiry)
            .unwrap(),
    );
    assert_ne!(old, new);
    assert_eq!(fixture.counts(), (2, 2, 1));
    assert_eq!(
        fixture.request(&b, "read", json!({"message_id":old}), expiry),
        Err(NoticeError::NotFound)
    );
    assert!(
        fixture
            .db
            .query_row(
                "SELECT idempotency_key IS NULL FROM agent_messages WHERE id=?",
                [&old],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
    fixture
        .request(&b, "unread", json!({}), expiry + 1001)
        .unwrap();
    assert_eq!(fixture.counts(), (1, 1, 0));
}

#[test]
fn close_fences_stdio_renewal() {
    let mut fixture = Fixture::new();
    let old = identity("owner", "old", SessionKind::Stdio);
    let later = identity("owner", "next", SessionKind::Stdio);
    let old_id = fixture.register(&old, "terminal", 100);
    let tx = fixture.db.transaction().unwrap();
    renew_in(&tx, &old, true, 101).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        fixture.request(&old, "register", json!({"label":"stale"}), 102),
        Err(NoticeError::NotFound)
    );
    let tx = fixture.db.transaction().unwrap();
    renew_in(&tx, &old, false, 103).unwrap();
    tx.commit().unwrap();
    let new_id = fixture.register(&later, "terminal", 104);
    assert_ne!(old_id, new_id);
    let dir = fixture.request(&later, "sessions", json!({}), 105).unwrap();
    assert!(
        dir["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["session_id"] == old_id && s["active"] == false)
    );
    assert_eq!(
        fixture.request(&later, "send", send(target(&old_id), "stale", "text"), 106),
        Err(NoticeError::NotFound)
    );
}

#[test]
fn limits_cover_serialized_tool_result() {
    let mut fixture = Fixture::new();
    let a = identity("owner", "a", SessionKind::Http);
    let id = fixture.register(&a, &"\"".repeat(64), 100);
    let body = "\\".repeat(16_384);
    let receipt = fixture
        .request(&a, "send", send(target(&id), "escape", &body), 200)
        .unwrap();
    let view = fixture
        .request(&a, "read", json!({"message_id":receipt["message_id"]}), 201)
        .unwrap();
    assert_eq!(view["body"], body);
    let encoded = serde_json::to_vec(&json!({
        "content":[{"type":"text","text":view.to_string()}],
        "structuredContent":view
    }))
    .unwrap();
    assert!(encoded.len() <= 128 * 1024);
    assert_eq!(
        fixture.request(
            &a,
            "send",
            send(target(&id), "oversize", &(body + "x")),
            202
        ),
        Err(NoticeError::Invalid)
    );
    assert_eq!(fixture.counts(), (1, 1, 0));
}

#[tokio::test]
async fn concurrent_retry_has_one_receipt_across_independent_connections() {
    let mut fixture = Fixture::new();
    let now = crate::dispatch::codemode_notices::now_ms();
    let a = identity("owner", "a", SessionKind::Http);
    let b = identity("owner", "b", SessionKind::Http);
    fixture.register(&a, "sender", now);
    let b_id = fixture.register(&b, "peer", now);
    let first = NoticeStore::open_at(fixture.root.path()).unwrap();
    let second = NoticeStore::open_at(fixture.root.path()).unwrap();
    let request = || {
        serde_json::from_value(
            json!({"action":"send","params":send(target(&b_id), "race", "text")}),
        )
        .unwrap()
    };
    let (left, right) = tokio::join!(
        first.message_request(a.clone(), request()),
        second.message_request(a.clone(), request()),
    );
    assert!(left.is_ok() || right.is_ok());
    let old = left.as_ref().or(right.as_ref()).unwrap();
    if let (Ok(left), Ok(right)) = (&left, &right) {
        assert_ne!(left["duplicate"], right["duplicate"]);
        assert_eq!(left["message_id"], right["message_id"]);
    } else {
        assert!(matches!(left, Err(NoticeError::Busy)) || matches!(right, Err(NoticeError::Busy)));
    }
    let replay = first.message_request(a, request()).await.unwrap();
    assert_eq!(replay["duplicate"], true);
    assert_eq!(replay["message_id"], old["message_id"]);
    assert_eq!(fixture.counts(), (1, 1, 0));
}

#[test]
fn owner_and_global_quotas_are_independent() {
    let mut fixture = Fixture::new();
    let a = identity("owner", "a", SessionKind::Http);
    let b = identity("owner", "b", SessionKind::Http);
    let outsider = identity("outsider", "a", SessionKind::Http);
    let a_id = fixture.register(&a, "sender", 100);
    let b_id = fixture.register(&b, "peer", 100);
    let mut third = None;
    for n in 0..30 {
        let binding = identity("owner", &format!("peer-{n}"), SessionKind::Http);
        let id = fixture.register(&binding, "extra peer", 100);
        third.get_or_insert(id);
    }
    let overflow = identity("owner", "overflow", SessionKind::Http);
    assert_eq!(
        fixture.request(&overflow, "register", json!({"label":"overflow"}), 101),
        Err(NoticeError::Full)
    );
    let outsider_id = fixture.register(&outsider, "different owner", 101);
    for id in [&a_id, &b_id] {
        for i in 0..32 {
            fixture
                .request(
                    &a,
                    "send",
                    send(target(id), &format!("{id}-{i}"), "test"),
                    200,
                )
                .unwrap();
        }
    }
    assert_eq!(
        fixture.request(
            &a,
            "send",
            send(target(third.as_deref().unwrap()), "overflow", "test"),
            201
        ),
        Err(NoticeError::Full)
    );
    fixture
        .request(
            &outsider,
            "send",
            send(target(&outsider_id), "accepted", "test"),
            201,
        )
        .unwrap();

    fixture.db.execute_batch(
        "WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<191)
         INSERT INTO agent_messages(id,owner_key,sender_id,sender_label,body,summary,
                                    request_digest,recipient_ids,created,expires)
         SELECT 'fixture-'||n,'fixture','fixture','test','test','test','digest','[]',1,10000
         FROM seq;
         WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<479)
         INSERT INTO agent_inboxes SELECT 'fixture-'||n,'fixture-'||n,'message_session',10000 FROM seq;
         WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<479)
         INSERT INTO agent_message_sessions(id,owner_key,binding_key,inbox_id,label,kind,
                 last_seen,active_until,retained_until)
         SELECT 'fixture-'||n,'owner-'||n,'binding-'||n,'fixture-'||n,'fixture','http',1,500,10000
         FROM seq;",
    ).unwrap();
    assert_eq!(
        fixture.request(
            &outsider,
            "send",
            send(target(&outsider_id), "global", "test"),
            202
        ),
        Err(NoticeError::Full)
    );
    let new_owner = identity("new owner", "a", SessionKind::Http);
    assert_eq!(
        fixture.request(&new_owner, "register", json!({"label":"global"}), 202),
        Err(NoticeError::Full)
    );
    assert_eq!(fixture.counts(), (256, 65, 0));
}
