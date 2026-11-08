use super::*;

pub(super) const V1: &str = "CREATE TABLE agent_inboxes (
                id TEXT PRIMARY KEY, recipient TEXT NOT NULL UNIQUE,
                scope TEXT NOT NULL, expires INTEGER NOT NULL);
            CREATE TABLE agent_notices (
                id TEXT PRIMARY KEY, inbox_id TEXT NOT NULL REFERENCES agent_inboxes(id) ON DELETE CASCADE,
                producer TEXT NOT NULL, dedupe_key TEXT NOT NULL, payload TEXT NOT NULL,
                source TEXT NOT NULL, level TEXT NOT NULL, message TEXT NOT NULL,
                created INTEGER NOT NULL, expires INTEGER NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0, next_attempt INTEGER NOT NULL DEFAULT 0,
                acknowledged INTEGER NOT NULL DEFAULT 0,
                UNIQUE(inbox_id, producer, dedupe_key));
            CREATE INDEX agent_notices_delivery ON agent_notices(inbox_id, acknowledged, next_attempt, created);
";
pub(super) fn schema_snapshot(
    db: &Connection,
) -> Result<Vec<(String, String, String)>, NoticeError> {
    let mut query = db.prepare(
        "SELECT type,name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
    )?;
    Ok(query
        .query_map([], |row| {
            let sql: String = row.get(2)?;
            Ok((
                row.get(0)?,
                row.get(1)?,
                sql.split_whitespace().collect::<String>(),
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

// The existing private DB remains the sole owner of a notice and its deliveries.
// A notice-level webhook witness survives expiry or removal of its subscription.
const ADD_EVENTS: &str = "
    ALTER TABLE agent_notices ADD COLUMN event_body BLOB;
    ALTER TABLE agent_notices ADD COLUMN webhook_offered INTEGER NOT NULL DEFAULT 0 CHECK(webhook_offered IN (0,1));
    CREATE TABLE agent_event_subscriptions (
        id TEXT PRIMARY KEY, recipient TEXT NOT NULL,
        inbox_id TEXT NOT NULL REFERENCES agent_inboxes(id) ON DELETE CASCADE,
        authority_key TEXT NOT NULL, callback TEXT NOT NULL, source TEXT,
        secret BLOB NOT NULL, secret_fingerprint TEXT NOT NULL, previous_secret BLOB,
        previous_until INTEGER NOT NULL DEFAULT 0,
        expires INTEGER NOT NULL, generation INTEGER NOT NULL CHECK(generation>0),
        active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0,1)));
    CREATE INDEX agent_event_subscriptions_inbox ON agent_event_subscriptions(inbox_id,active,expires);
    CREATE INDEX agent_event_subscriptions_authority ON agent_event_subscriptions(authority_key,active);
    CREATE TABLE agent_event_deliveries (
        subscription_id TEXT NOT NULL REFERENCES agent_event_subscriptions(id) ON DELETE CASCADE,
        notice_id TEXT NOT NULL REFERENCES agent_notices(id) ON DELETE CASCADE,
        generation INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','received','stopped')),
        attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts BETWEEN 0 AND 8),
        next_attempt INTEGER NOT NULL, claim_token TEXT, claim_until INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY(subscription_id,notice_id));
    CREATE INDEX agent_event_deliveries_ready ON agent_event_deliveries(status,next_attempt);
    CREATE INDEX agent_event_deliveries_notice ON agent_event_deliveries(notice_id);
    CREATE INDEX agent_event_deliveries_lease ON agent_event_deliveries(claim_until);
";

const ADD_MESSAGES: &str = "
    CREATE TABLE agent_message_sessions (
        id TEXT PRIMARY KEY NOT NULL,
        owner_key TEXT NOT NULL,
        binding_key TEXT NOT NULL UNIQUE,
        inbox_id TEXT NOT NULL UNIQUE REFERENCES agent_inboxes(id) ON DELETE CASCADE,
        label TEXT NOT NULL,
        kind TEXT NOT NULL CHECK(kind IN ('http','stdio')),
        last_seen INTEGER NOT NULL,
        active_until INTEGER NOT NULL,
        retained_until INTEGER NOT NULL,
        closed INTEGER NOT NULL DEFAULT 0 CHECK(closed IN (0,1)));
    CREATE INDEX agent_message_sessions_owner ON agent_message_sessions(owner_key,retained_until);
    CREATE TABLE agent_messages (
        id TEXT PRIMARY KEY NOT NULL,
        owner_key TEXT NOT NULL,
        sender_id TEXT NOT NULL,
        sender_label TEXT NOT NULL,
        body TEXT NOT NULL,
        summary TEXT NOT NULL,
        request_digest TEXT NOT NULL,
        idempotency_key TEXT,
        recipient_ids TEXT NOT NULL,
        created INTEGER NOT NULL,
        expires INTEGER NOT NULL,
        UNIQUE(sender_id,idempotency_key));
    CREATE INDEX agent_messages_owner ON agent_messages(owner_key,expires);
    CREATE INDEX agent_messages_expiry ON agent_messages(expires);
    ALTER TABLE agent_notices ADD COLUMN message_id TEXT REFERENCES agent_messages(id);
    CREATE UNIQUE INDEX agent_notices_message_ref ON agent_notices(message_id,inbox_id)
        WHERE message_id IS NOT NULL;
    ALTER TABLE agent_event_subscriptions ADD COLUMN session_messages INTEGER NOT NULL DEFAULT 0
        CHECK(session_messages IN (0,1));
";

pub(super) fn migrate(db: &mut Connection) -> Result<(), NoticeError> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let application: i64 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let actual = schema_snapshot(&tx)?;
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(V1)?;
    if version == 0 && application == 0 && actual.is_empty() {
        tx.execute_batch(V1)?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)?;
    } else {
        if !matches!(version, 1..=SCHEMA_VERSION) {
            return Err(validation_failure("schema_version", Some(version)));
        }
        if application != APPLICATION_ID {
            return Err(validation_failure("application_id", Some(application)));
        }
        if version >= 2 {
            expected.execute_batch(ADD_EVENTS)?;
        }
        if version >= 3 {
            expected.execute_batch(ADD_MESSAGES)?;
        }
        if actual != schema_snapshot(&expected)? {
            return Err(validation_failure("schema_mismatch", None));
        }
        check_integrity(&tx)?;
    }
    if version < 2 {
        tx.execute_batch(ADD_EVENTS)?;
    }
    if version < 3 {
        tx.execute_batch(ADD_MESSAGES)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        check_integrity(&tx)?;
    }
    tx.commit()?;
    Ok(())
}

fn check_integrity(db: &Connection) -> Result<(), NoticeError> {
    let integrity: String = db.query_row("PRAGMA quick_check(1)", [], |r| r.get(0))?;
    let foreign_violation = db.prepare("PRAGMA foreign_key_check")?.exists([])?;
    validate_integrity(&integrity, foreign_violation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v2_fixture() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        db.execute_batch(V1).unwrap();
        db.execute_batch(ADD_EVENTS).unwrap();
        db.pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        db.pragma_update(None, "user_version", 2).unwrap();
        db.execute_batch(
            "INSERT INTO agent_inboxes VALUES('v2-inbox','v2-owner','authenticated_client',100000);
             INSERT INTO agent_notices
               (id,inbox_id,producer,dedupe_key,payload,source,level,message,created,expires,
                attempts,next_attempt,acknowledged,event_body,webhook_offered)
               VALUES('v2-notice','v2-inbox','admin','dedupe','original','fixture','info','old notice',
                      10,90000,2,3000,1,X'0102',1);
             INSERT INTO agent_event_subscriptions
               (id,recipient,inbox_id,authority_key,callback,source,secret,secret_fingerprint,
                previous_secret,previous_until,expires,generation,active)
               VALUES('v2-sub','v2-owner','v2-inbox','authority','https://example.com/callback',
                      'fixture',X'1122','fingerprint',X'3344',2000,85000,7,1);
             INSERT INTO agent_event_deliveries
               VALUES('v2-sub','v2-notice',7,'pending',2,3100,'claim',5000);",
        )
        .unwrap();
        db
    }

    fn v2_rows(db: &Connection) -> Vec<String> {
        [
            "SELECT json_array(id,recipient,scope,expires) FROM agent_inboxes",
            "SELECT json_array(id,inbox_id,producer,dedupe_key,payload,source,level,message,
                created,expires,attempts,next_attempt,acknowledged,hex(event_body),webhook_offered)
                FROM agent_notices",
            "SELECT json_array(id,recipient,inbox_id,authority_key,callback,source,hex(secret),
                secret_fingerprint,hex(previous_secret),previous_until,expires,generation,active)
                FROM agent_event_subscriptions",
            "SELECT json_array(subscription_id,notice_id,generation,status,attempts,next_attempt,
                claim_token,claim_until) FROM agent_event_deliveries",
        ]
        .into_iter()
        .map(|sql| db.query_row(sql, [], |row| row.get(0)).unwrap())
        .collect()
    }

    #[test]
    fn v2_migration_preserves_subscriptions_and_claims() {
        let mut db = v2_fixture();
        let before = v2_rows(&db);
        migrate(&mut db).unwrap();
        assert_eq!(
            db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(v2_rows(&db), before);
        assert!(
            !db.query_row(
                "SELECT session_messages FROM agent_event_subscriptions",
                [],
                |r| { r.get::<_, bool>(0) }
            )
            .unwrap()
        );
        assert!(
            db.query_row("SELECT message_id FROM agent_notices", [], |r| {
                r.get::<_, Option<String>>(0)
            })
            .unwrap()
            .is_none()
        );
        migrate(&mut db).unwrap();
        assert_eq!(v2_rows(&db), before);
    }

    #[test]
    fn failed_v2_migration_is_atomic() {
        let mut db = v2_fixture();
        let before_schema = schema_snapshot(&db).unwrap();
        let before_rows = v2_rows(&db);
        let pages: u32 = db
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .unwrap();
        db.pragma_update(None, "max_page_count", pages).unwrap();
        assert!(
            migrate(&mut db).is_err(),
            "v3 tables cannot fit in the v2 file"
        );
        assert_eq!(
            db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(schema_snapshot(&db).unwrap(), before_schema);
        assert_eq!(v2_rows(&db), before_rows);
    }

    #[test]
    fn message_schema_retains_live_dependencies() {
        let mut db = v2_fixture();
        migrate(&mut db).unwrap();
        db.execute_batch(
            "INSERT INTO agent_messages
                (id,owner_key,sender_id,sender_label,body,summary,request_digest,
                 idempotency_key,recipient_ids,created,expires)
              VALUES('message','owner','sender','label','full body','preview','digest','key',
                     '[]',10,90000);
             UPDATE agent_notices SET message_id='message' WHERE id='v2-notice';",
        )
        .unwrap();
        assert!(
            db.execute(
                "UPDATE agent_notices SET message_id='missing' WHERE id='v2-notice'",
                [],
            )
            .is_err()
        );
        assert!(db.execute(
            "INSERT INTO agent_notices
                (id,inbox_id,producer,dedupe_key,payload,source,level,message,created,expires,message_id)
              VALUES('duplicate','v2-inbox','admin','dedupe2','original','fixture','info',
                     'old notice',10,90000,'message')",
            [],
        ).is_err());
        db.execute_batch(
            "DELETE FROM agent_notices;
             UPDATE agent_inboxes SET expires=100;
             INSERT INTO agent_inboxes VALUES('local','owner','stdio_connection',100);
             INSERT INTO agent_message_sessions
                (id,owner_key,binding_key,inbox_id,label,kind,last_seen,active_until,retained_until)
              VALUES('session','owner','binding','local','label','stdio',100,200,4000);",
        )
        .unwrap();
        prune(&db, 1000).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM agent_inboxes", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            2,
        );
        prune(&db, 90001).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM agent_inboxes", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            0,
        );
    }
}
