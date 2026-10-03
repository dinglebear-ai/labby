//! Durable, caller-scoped notices piggybacked on Code Mode responses.
//!
//! This inbox is separate from operator notifications. A committed response lease
//! is not a receipt: messages remain retryable until acknowledged or expired.
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::Semaphore;

pub(crate) const MAX_NOTICE_BATCH_BYTES: usize = 1024;
pub(crate) const MAX_ACKS: usize = 32;
const MAX_PENDING: i64 = 2_000;
const MAX_PER_RECIPIENT: i64 = 32;
const MAX_INBOXES: i64 = 2_000;
const MAX_BATCH: usize = 3;
const INBOX_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
const SCHEMA_VERSION: i64 = 1;
const APPLICATION_ID: i64 = 0x4c4e_5446;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct NoticeRecipient {
    pub actor: Option<String>,
    pub route: String,
    pub consumer: NoticeConsumer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum NoticeConsumer {
    Client {
        id: String,
        conversation: Option<String>,
    },
    Credential {
        id: String,
        conversation: Option<String>,
    },
    // Session is a server-generated random ID, never a restartable PID/counter.
    Stdio {
        session: String,
    },
}

impl NoticeRecipient {
    fn key(&self) -> Result<String, NoticeError> {
        let valid = bounded(&self.route, 512)
            && self.actor.as_ref().is_none_or(|v| bounded(v, 512))
            && match &self.consumer {
                NoticeConsumer::Client { id, conversation }
                | NoticeConsumer::Credential { id, conversation } => {
                    self.actor.is_some()
                        && bounded(id, 512)
                        && conversation.as_ref().is_none_or(|v| bounded(v, 128))
                }
                NoticeConsumer::Stdio { session } => bounded(session, 128),
            };
        if !valid {
            return Err(NoticeError::Invalid);
        }
        let encoded = serde_json::to_vec(self).map_err(|_| NoticeError::Invalid)?;
        Ok(hex::encode(Sha256::digest(encoded)))
    }

    pub(crate) fn scope(&self) -> &'static str {
        match &self.consumer {
            NoticeConsumer::Client {
                conversation: Some(_),
                ..
            }
            | NoticeConsumer::Credential {
                conversation: Some(_),
                ..
            } => "conversation_routing",
            NoticeConsumer::Client { .. } => "authenticated_client",
            NoticeConsumer::Credential { .. } => "authenticated_credential",
            NoticeConsumer::Stdio { .. } => "stdio_connection",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum NoticeLevel {
    Info,
    Warning,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CodeModeNotice {
    pub id: String,
    pub source: String,
    pub level: NoticeLevel,
    pub message: String,
    pub delivery_attempt: u32,
    pub expires_at_unix_ms: i64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct NoticeBatch {
    pub notifications: Vec<CodeModeNotice>,
    pub notifications_remaining: usize,
    pub notifications_are_advisory: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct InboxAddress {
    pub id: String,
    pub scope: String,
    pub expires_at_unix_ms: i64,
    pub delivery: &'static str,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublishNotice {
    pub inbox_id: String,
    pub source: String,
    pub level: NoticeLevel,
    pub message: String,
    pub dedupe_key: String,
    #[serde(default = "default_ttl")]
    pub ttl_seconds: u32,
}
fn default_ttl() -> u32 {
    3_600
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublishReceipt {
    pub id: String,
    pub duplicate: bool,
}

/// These values must come from authenticated server context, never JSON params.
#[derive(Clone, Debug)]
pub(crate) struct NoticeProducer {
    pub actor: String,
    pub admin: bool,
}

#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum NoticeError {
    #[error("invalid notification fields, identity, or acknowledgment")]
    Invalid,
    #[error("notification controls require write-capable Code Mode")]
    ReadOnly,
    #[error("notification controls require authenticated actor/client or trusted stdio")]
    Identity,
    #[error("lab:admin scope required to publish agent notifications")]
    Forbidden,
    #[error("notification inbox unavailable or expired")]
    NotFound,
    #[error("dedupe key already belongs to a different notification payload")]
    Conflict,
    #[error("notification capacity reached; retry after acknowledged records expire")]
    Full,
    #[error("notification store busy; retry with the same idempotency key")]
    Busy,
    #[error("durable notification store unavailable; no in-memory fallback")]
    Unavailable,
    #[error(
        "notification deadline exceeded; outcome may be unknown, reconcile with the same IDs before retrying"
    )]
    Deadline,
}

impl NoticeError {
    pub(crate) fn kind(self) -> &'static str {
        match self {
            Self::Invalid => "invalid_param",
            Self::Forbidden | Self::ReadOnly | Self::Identity => "forbidden",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Full | Self::Busy => "rate_limited",
            Self::Unavailable => "unavailable",
            Self::Deadline => "timeout",
        }
    }
}

impl From<rusqlite::Error> for NoticeError {
    fn from(error: rusqlite::Error) -> Self {
        match error {
            rusqlite::Error::SqliteFailure(code, _)
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                Self::Busy
            }
            error => {
                // SQLite messages can include SQL or payloads. Log codes only.
                if let rusqlite::Error::SqliteFailure(code, _) = error {
                    tracing::error!(
                        subsystem = "agent_notifications",
                        operation = "sqlite",
                        sqlite_code = ?code.code,
                        sqlite_extended_code = code.extended_code,
                        "notification storage operation failed"
                    );
                } else {
                    tracing::error!(
                        subsystem = "agent_notifications",
                        operation = "sqlite",
                        "notification storage operation failed without a SQLite status code"
                    );
                }
                Self::Unavailable
            }
        }
    }
}

fn filesystem_failure(operation: &'static str, error: &std::io::Error) -> NoticeError {
    tracing::error!(
        subsystem = "agent_notifications",
        operation,
        io_kind = ?error.kind(),
        os_code = error.raw_os_error(),
        "notification filesystem operation failed"
    );
    NoticeError::Unavailable
}

fn validation_failure(reason: &'static str, observed_value: Option<i64>) -> NoticeError {
    // Schema SQL and integrity diagnostics may contain private names or data.
    tracing::error!(
        subsystem = "agent_notifications",
        operation = "validate_database",
        reason,
        observed_value,
        "notification database validation rejected"
    );
    NoticeError::Unavailable
}

fn validate_integrity(integrity: &str, foreign_violation: bool) -> Result<(), NoticeError> {
    if integrity != "ok" {
        return Err(validation_failure("integrity_check", None));
    }
    if foreign_violation {
        return Err(validation_failure("foreign_keys", None));
    }
    Ok(())
}

#[derive(Clone)]
pub(crate) struct NoticeStore {
    connection: Option<Arc<Mutex<Connection>>>,
    #[cfg(windows)]
    windows_guard: Option<Arc<labby_winjob::fs::SqliteVerificationGuard>>,
    // At most one blocking operation per store can be queued or executing.
    permit: Arc<Semaphore>,
}

impl Default for NoticeStore {
    fn default() -> Self {
        Self::disabled()
    }
}

impl NoticeStore {
    #[cfg(test)]
    pub(crate) fn memory() -> Result<Self, NoticeError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    pub(crate) fn disabled() -> Self {
        Self {
            connection: None,
            #[cfg(windows)]
            windows_guard: None,
            permit: Arc::new(Semaphore::new(1)),
        }
    }

    fn from_connection(mut connection: Connection) -> Result<Self, NoticeError> {
        // Startup contention has a separate bounded budget from request operations.
        connection.busy_timeout(Duration::from_secs(1))?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
        {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
            let application: i64 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
            let schema = schema_snapshot(&tx)?;
            if version == 0 && application == 0 && schema.is_empty() {
                tx.execute_batch(SCHEMA_SQL)?;
                tx.pragma_update(None, "application_id", APPLICATION_ID)?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            } else {
                let expected = Connection::open_in_memory()?;
                expected.execute_batch(SCHEMA_SQL)?;
                if version != SCHEMA_VERSION {
                    return Err(validation_failure("schema_version", Some(version)));
                }
                if application != APPLICATION_ID {
                    return Err(validation_failure("application_id", Some(application)));
                }
                if schema != schema_snapshot(&expected)? {
                    return Err(validation_failure("schema_mismatch", None));
                }
            }
            let integrity: String = tx.query_row("PRAGMA quick_check(1)", [], |r| r.get(0))?;
            let foreign_violation: bool = tx.prepare("PRAGMA foreign_key_check")?.exists([])?;
            validate_integrity(&integrity, foreign_violation)?;
            tx.commit()?;
        }
        connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA max_page_count=4096;")?;
        connection.busy_timeout(Duration::from_millis(25))?;
        Ok(Self {
            connection: Some(Arc::new(Mutex::new(connection))),
            #[cfg(windows)]
            windows_guard: None,
            permit: Arc::new(Semaphore::new(1)),
        })
    }

    /// Open under the validated private installation, before accepting requests.
    pub(crate) async fn open_installation() -> Result<Self, NoticeError> {
        tokio::task::spawn_blocking(|| {
            let paths = crate::installation::InstallationPaths::resolve()
                .map_err(|_| NoticeError::Unavailable)?;
            paths.prepare_root().map_err(|_| NoticeError::Unavailable)?;
            Self::open_at(paths.root())
        })
        .await
        .map_err(|_| NoticeError::Unavailable)?
    }

    fn open_at(root: &Path) -> Result<Self, NoticeError> {
        Self::open_at_attempt(root, false)
    }

    fn open_at_attempt(root: &Path, retried: bool) -> Result<Self, NoticeError> {
        let directory = root.join("agent-notifications");
        crate::installation::secure_file::create_private_dir(&directory)
            .map_err(|error| filesystem_failure("create_directory", &error))?;
        let path = directory.join("inbox.sqlite3");
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.len() > 16 * 1024 * 1024
                    || !metadata.is_file()
                    || metadata.file_type().is_symlink()
                {
                    return Err(NoticeError::Unavailable);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt as _;
                    let parent = std::fs::metadata(&directory)
                        .map_err(|error| filesystem_failure("directory_metadata", &error))?;
                    if metadata.mode() & 0o077 != 0
                        || metadata.nlink() != 1
                        || metadata.uid() != parent.uid()
                    {
                        return Err(NoticeError::Unavailable);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match crate::installation::secure_file::publish_new(&path, &[]) {
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        return if retried {
                            Err(NoticeError::Unavailable)
                        } else {
                            Self::open_at_attempt(root, true)
                        };
                    }
                    Err(error) => return Err(filesystem_failure("create_database", &error)),
                }
            }
            Err(error) => return Err(filesystem_failure("database_metadata", &error)),
        }
        #[cfg(windows)]
        let verified = {
            let guard = labby_winjob::fs::open_sqlite_verification(&path)
                .map_err(|error| filesystem_failure("verify_database", &error))?;
            labby_winjob::fs::verify_private_acl(guard.file())
                .map_err(|error| filesystem_failure("verify_database_acl", &error))?;
            guard
        };
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let store = Self::from_connection(Connection::open_with_flags(path, flags)?)?;
        #[cfg(windows)]
        let store = Self {
            windows_guard: Some(Arc::new(verified)),
            ..store
        };
        Ok(store)
    }

    async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T, NoticeError> + Send + 'static,
    ) -> Result<T, NoticeError> {
        let db = self.connection.clone().ok_or(NoticeError::Unavailable)?;
        #[cfg(windows)]
        let guard = self.windows_guard.clone();
        let permit = Arc::clone(&self.permit)
            .try_acquire_owned()
            .map_err(|_| NoticeError::Busy)?;
        // A deadline bounds response latency, not the durable write. The worker
        // retains its permit so a timeout cannot grow an unbounded work queue.
        let worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            #[cfg(windows)]
            // Tuple fields drop in order, including on early errors/unwinding.
            let resources = (db, guard);
            #[cfg(windows)]
            let mut connection = resources.0.lock().map_err(|_| NoticeError::Unavailable)?;
            #[cfg(not(windows))]
            let mut connection = db.lock().map_err(|_| NoticeError::Unavailable)?;
            let result = work(&mut connection);
            drop(connection);
            // Close the worker's last connection before releasing Windows pins.
            #[cfg(windows)]
            drop(resources);
            #[cfg(not(windows))]
            drop(db);
            result
        });
        tokio::time::timeout(Duration::from_millis(250), worker)
            .await
            .map_err(|_| NoticeError::Deadline)?
            .map_err(|_| NoticeError::Unavailable)?
    }

    pub(crate) async fn controls(
        &self,
        recipient: NoticeRecipient,
        register: bool,
        ids: Option<Vec<String>>,
    ) -> Result<(Option<InboxAddress>, Option<Vec<String>>), NoticeError> {
        if let Some(ids) = &ids {
            validate_acknowledgments(ids)?;
        }
        let key = recipient.key()?;
        self.run(move |db| {
            let now = now_ms();
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let address = if register {
                Some(register_in(&tx, &key, recipient.scope(), now)?)
            } else {
                None
            };
            let acknowledgments = ids
                .as_ref()
                .map(|ids| acknowledge_in(&tx, &key, ids, now))
                .transpose()?;
            tx.commit()?;
            Ok((address, acknowledgments))
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn register(
        &self,
        recipient: NoticeRecipient,
    ) -> Result<InboxAddress, NoticeError> {
        let key = recipient.key()?;
        self.run(move |db| register_at(db, &key, recipient.scope(), now_ms()))
            .await
    }

    pub(crate) async fn publish(
        &self,
        producer: NoticeProducer,
        input: PublishNotice,
    ) -> Result<PublishReceipt, NoticeError> {
        // Authorization before validation or lookup; no recipient enumeration.
        if !producer.admin {
            return Err(NoticeError::Forbidden);
        }
        if !bounded(&producer.actor, 512) {
            return Err(NoticeError::Forbidden);
        }
        validate_publish(&input)?;
        self.run(move |db| publish_at(db, &producer.actor, &input, now_ms()))
            .await
    }

    /// The acceptor builds a candidate envelope. Only commit after it fits; return
    /// that candidate after the durable lease commits, never mutate output early.
    pub(crate) async fn deliver<T: Send + 'static>(
        &self,
        recipient: NoticeRecipient,
        accept: impl FnMut(&NoticeBatch) -> Option<T> + Send + 'static,
    ) -> Result<Option<T>, NoticeError> {
        let key = recipient.key()?;
        self.run(move |db| deliver_at(db, &key, now_ms(), accept))
            .await
    }
}

pub(crate) fn validate_acknowledgments(ids: &[String]) -> Result<(), NoticeError> {
    if ids.len() > MAX_ACKS || ids.iter().any(|id| !valid_id(id, "notice_")) {
        return Err(NoticeError::Invalid);
    }
    Ok(())
}

fn bounded(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn valid_id(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|id| id.len() == 26 && ulid::Ulid::from_string(id).is_ok())
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| i64::try_from(v.as_millis()).unwrap_or(i64::MAX))
}
fn validate_publish(input: &PublishNotice) -> Result<(), NoticeError> {
    if !valid_id(&input.inbox_id, "inbox_")
        || !bounded(&input.source, 64)
        || !bounded(&input.message, 384)
        || !bounded(&input.dedupe_key, 128)
        || !(1..=86_400).contains(&input.ttl_seconds)
    {
        return Err(NoticeError::Invalid);
    }
    let notice = CodeModeNotice {
        id: format!("notice_{}", ulid::Ulid::new()),
        source: input.source.clone(),
        level: input.level,
        message: input.message.clone(),
        delivery_attempt: u32::MAX,
        expires_at_unix_ms: i64::MAX,
    };
    let batch = NoticeBatch {
        notifications: vec![notice],
        notifications_remaining: MAX_PENDING as usize,
        notifications_are_advisory: true,
    };
    if serde_json::to_vec(&batch).map_or(true, |v| v.len() > MAX_NOTICE_BATCH_BYTES) {
        return Err(NoticeError::Invalid);
    }
    Ok(())
}
fn prune(db: &Connection, now: i64) -> Result<(), NoticeError> {
    db.execute("DELETE FROM agent_notices WHERE expires <= ?", [now])?;
    db.execute("DELETE FROM agent_inboxes WHERE expires <= ?", [now])?;
    Ok(())
}
#[cfg(test)]
fn register_at(
    db: &mut Connection,
    key: &str,
    scope: &str,
    now: i64,
) -> Result<InboxAddress, NoticeError> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let address = register_in(&tx, key, scope, now)?;
    tx.commit()?;
    Ok(address)
}
fn register_in(
    tx: &Connection,
    key: &str,
    scope: &str,
    now: i64,
) -> Result<InboxAddress, NoticeError> {
    prune(tx, now)?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM agent_inboxes WHERE recipient=?",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    let id = if let Some(id) = existing {
        id
    } else {
        let count: i64 = tx.query_row("SELECT count(*) FROM agent_inboxes", [], |r| r.get(0))?;
        if count >= MAX_INBOXES {
            return Err(NoticeError::Full);
        }
        format!("inbox_{}", ulid::Ulid::new())
    };
    let expires = now.saturating_add(INBOX_TTL_MS);
    tx.execute(
        "INSERT INTO agent_inboxes(id,recipient,scope,expires) VALUES(?,?,?,?)
        ON CONFLICT(recipient) DO UPDATE SET expires=excluded.expires",
        params![id, key, scope, expires],
    )?;
    Ok(InboxAddress {
        id,
        scope: scope.to_owned(),
        expires_at_unix_ms: expires,
        delivery: "at_least_once_until_ack_or_expiry",
    })
}
fn publish_at(
    db: &mut Connection,
    actor: &str,
    input: &PublishNotice,
    now: i64,
) -> Result<PublishReceipt, NoticeError> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    prune(&tx, now)?;
    let inbox_expires: Option<i64> = tx
        .query_row(
            "SELECT expires FROM agent_inboxes WHERE id=?",
            [&input.inbox_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(inbox_expires) = inbox_expires else {
        return Err(NoticeError::NotFound);
    };
    let payload = serde_json::to_string(&(
        input.source.as_str(),
        input.level,
        input.message.as_str(),
        input.ttl_seconds,
    ))
    .map_err(|_| NoticeError::Invalid)?;
    let prior: Option<(String, String)> = tx
        .query_row(
            "SELECT id,payload FROM agent_notices WHERE inbox_id=? AND producer=? AND dedupe_key=?",
            params![input.inbox_id, actor, input.dedupe_key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((id, prior)) = prior {
        if prior != payload {
            return Err(NoticeError::Conflict);
        }
        tx.commit()?;
        return Ok(PublishReceipt {
            id,
            duplicate: true,
        });
    }
    let total: i64 = tx.query_row("SELECT count(*) FROM agent_notices", [], |r| r.get(0))?;
    let owned: i64 = tx.query_row(
        "SELECT count(*) FROM agent_notices WHERE inbox_id=?",
        [&input.inbox_id],
        |r| r.get(0),
    )?;
    if total >= MAX_PENDING || owned >= MAX_PER_RECIPIENT {
        return Err(NoticeError::Full);
    }
    let id = format!("notice_{}", ulid::Ulid::new());
    let expires = now
        .saturating_add(i64::from(input.ttl_seconds) * 1_000)
        .min(inbox_expires);
    let level = match input.level {
        NoticeLevel::Info => "info",
        NoticeLevel::Warning => "warning",
    };
    tx.execute("INSERT INTO agent_notices(id,inbox_id,producer,dedupe_key,payload,source,level,message,created,expires) VALUES(?,?,?,?,?,?,?,?,?,?)",
        params![id,input.inbox_id,actor,input.dedupe_key,payload,input.source,level,input.message,now,expires])?;
    tx.commit()?;
    tracing::debug!(
        subsystem = "agent_notifications",
        action = "publish",
        duplicate = false,
        "agent notice persisted"
    );
    Ok(PublishReceipt {
        id,
        duplicate: false,
    })
}
#[cfg(test)]
fn acknowledge_at(
    db: &mut Connection,
    key: &str,
    ids: &[String],
    now: i64,
) -> Result<Vec<String>, NoticeError> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let acknowledged = acknowledge_in(&tx, key, ids, now)?;
    tx.commit()?;
    Ok(acknowledged)
}
fn acknowledge_in(
    tx: &Connection,
    key: &str,
    ids: &[String],
    now: i64,
) -> Result<Vec<String>, NoticeError> {
    let mut acknowledged = Vec::new();
    for id in ids {
        // Never reveal whether an unknown/foreign ID exists, and never ACK a notice
        // that this recipient has not been offered. Repeated own ACKs are idempotent.
        if tx.execute(
            "UPDATE agent_notices SET acknowledged=1 WHERE id=? AND attempts>0 AND expires>?
            AND inbox_id IN (SELECT id FROM agent_inboxes WHERE recipient=? AND expires>?)",
            params![id, now, key, now],
        )? > 0
            && !acknowledged.contains(id)
        {
            acknowledged.push(id.clone());
        }
    }
    Ok(acknowledged)
}
fn deliver_at<T>(
    db: &mut Connection,
    key: &str,
    now: i64,
    mut accept: impl FnMut(&NoticeBatch) -> Option<T>,
) -> Result<Option<T>, NoticeError> {
    let inbox: Option<String> = db
        .query_row(
            "SELECT id FROM agent_inboxes WHERE recipient=? AND expires>?",
            params![key, now],
            |r| r.get(0),
        )
        .optional()?;
    let Some(inbox) = inbox else {
        return Ok(None);
    };
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let total: i64 = tx.query_row(
        "SELECT count(*) FROM agent_notices WHERE inbox_id=? AND acknowledged=0 AND expires>?",
        params![inbox, now],
        |r| r.get(0),
    )?;
    let notices = {
        let mut query = tx.prepare(
            "SELECT id,source,level,message,attempts,expires FROM agent_notices
            WHERE inbox_id=? AND acknowledged=0 AND expires>? AND next_attempt<=?
            ORDER BY next_attempt,created,id LIMIT 3",
        )?;
        query
            .query_map(params![inbox, now, now], |r| {
                let level: String = r.get(2)?;
                Ok(CodeModeNotice {
                    id: r.get(0)?,
                    source: r.get(1)?,
                    level: if level == "warning" {
                        NoticeLevel::Warning
                    } else {
                        NoticeLevel::Info
                    },
                    message: r.get(3)?,
                    delivery_attempt: r.get::<_, u32>(4)?.saturating_add(1),
                    expires_at_unix_ms: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for count in (1..=notices.len().min(MAX_BATCH)).rev() {
        let batch = NoticeBatch {
            notifications: notices[..count].to_vec(),
            notifications_remaining: total as usize - count,
            notifications_are_advisory: true,
        };
        if serde_json::to_vec(&batch).map_or(true, |v| v.len() > MAX_NOTICE_BATCH_BYTES) {
            continue;
        }
        let Some(value) = accept(&batch) else {
            continue;
        };
        for notice in &batch.notifications {
            let delay = (30_000_i64 * (1_i64 << notice.delivery_attempt.saturating_sub(1).min(4)))
                .min(300_000);
            tx.execute(
                "UPDATE agent_notices SET attempts=?,next_attempt=? WHERE id=?",
                params![
                    notice.delivery_attempt,
                    now.saturating_add(delay),
                    notice.id
                ],
            )?;
        }
        tx.commit()?;
        tracing::debug!(
            subsystem = "agent_notifications",
            action = "offer",
            count,
            "agent notice delivery lease committed"
        );
        return Ok(Some(value));
    }
    Ok(None)
}

const SCHEMA_SQL: &str = "CREATE TABLE agent_inboxes (
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
fn schema_snapshot(db: &Connection) -> Result<Vec<(String, String, String)>, NoticeError> {
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

#[cfg(test)]
mod tests;
