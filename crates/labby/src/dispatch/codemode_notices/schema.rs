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
        if !matches!(version, 1 | SCHEMA_VERSION) {
            return Err(validation_failure("schema_version", Some(version)));
        }
        if application != APPLICATION_ID {
            return Err(validation_failure("application_id", Some(application)));
        }
        if version == SCHEMA_VERSION {
            expected.execute_batch(ADD_EVENTS)?;
        }
        if actual != schema_snapshot(&expected)? {
            return Err(validation_failure("schema_mismatch", None));
        }
        check_integrity(&tx)?;
    }
    if version < SCHEMA_VERSION {
        tx.execute_batch(ADD_EVENTS)?;
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
