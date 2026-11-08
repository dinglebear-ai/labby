use rusqlite::{Connection, OptionalExtension as _, Row, params};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::dispatch::codemode_notices::{
    NoticeError, NoticeLevel, PublishNotice, admit_notice_targets, insert_notice_in,
    notice_write_error, prune, register_in, stop_notice_delivery_in,
};

use super::types::*;

const MESSAGE_SOURCE: &str = "labby.messages";
// Account for the structured result and an escaped text copy of the same
// value, with room for the enclosing MCP result fields.
const MAX_READ_VIEW_BYTES: usize = (MAX_MESSAGE_TOOL_BYTES - 1024) / 3;

pub(super) struct MessageOutcome {
    pub(super) value: Value,
    pub(super) wake: bool,
}

impl MessageOutcome {
    fn reply(value: Value) -> Self {
        Self { value, wake: false }
    }
}

struct Session {
    id: String,
    inbox: String,
    label: String,
    last_seen: i64,
    active_until: i64,
    retained_until: i64,
    closed: bool,
}

impl Session {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            inbox: row.get(1)?,
            label: row.get(2)?,
            last_seen: row.get(3)?,
            active_until: row.get(4)?,
            retained_until: row.get(5)?,
            closed: row.get(6)?,
        })
    }

    fn view(&self, now: i64) -> Value {
        json!({
            "session_id": self.id,
            "label": self.label,
            "active": !self.closed && self.active_until > now,
            "last_seen_unix_ms": self.last_seen,
        })
    }
}

fn find(
    db: &Connection,
    binding: &SessionBinding,
    now: i64,
) -> Result<Option<Session>, NoticeError> {
    Ok(db
        .query_row(
            "SELECT s.id,s.inbox_id,s.label,s.last_seen,s.active_until,s.retained_until,s.closed
         FROM agent_message_sessions s JOIN agent_inboxes i ON i.id=s.inbox_id
         WHERE s.owner_key=?1 AND s.binding_key=?2 AND i.recipient=?3
           AND s.kind=?4 AND s.retained_until>?5 AND i.expires>?5",
            params![
                binding.owner_key,
                binding.binding_key,
                binding.recipient.key()?,
                binding.kind.as_str(),
                now
            ],
            Session::from_row,
        )
        .optional()?)
}

fn write(db: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<usize, NoticeError> {
    db.execute(sql, params)
        .map_err(|error| notice_write_error(error, true))
}

fn touch(
    db: &Connection,
    session: &mut Session,
    kind: SessionKind,
    now: i64,
) -> Result<(), NoticeError> {
    session.last_seen = now.max(session.last_seen);
    session.active_until = now
        .saturating_add(kind.lease_ms())
        .max(session.active_until);
    session.retained_until = now.saturating_add(RETENTION_MS).max(session.retained_until);
    write(
        db,
        "UPDATE agent_message_sessions SET label=?2,last_seen=?3,active_until=?4,retained_until=?5
         WHERE id=?1 AND closed=0",
        params![
            session.id,
            session.label,
            session.last_seen,
            session.active_until,
            session.retained_until
        ],
    )?;
    write(
        db,
        "UPDATE agent_inboxes SET expires=max(expires,?2) WHERE id=?1",
        params![session.inbox, session.retained_until],
    )?;
    Ok(())
}

fn register(
    db: &Connection,
    binding: &SessionBinding,
    label: String,
    now: i64,
) -> Result<Session, NoticeError> {
    if let Some(mut prior) = find(db, binding, now)? {
        if prior.closed {
            return Err(NoticeError::NotFound);
        }
        prior.label = label;
        touch(db, &mut prior, binding.kind, now)?;
        return Ok(prior);
    }
    let (total, owned): (i64, i64) = db.query_row(
        "SELECT count(*),count(CASE WHEN owner_key=?1 THEN 1 END) FROM agent_message_sessions",
        [&binding.owner_key],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if total >= MAX_SESSIONS || owned >= MAX_SESSIONS_PER_OWNER {
        return Err(NoticeError::Full);
    }
    let address = register_in(
        db,
        &binding.recipient.key()?,
        binding.recipient.scope(),
        now,
    )?;
    let session = Session {
        id: format!("session_{}", ulid::Ulid::new()),
        inbox: address.id,
        label,
        last_seen: now,
        active_until: now.saturating_add(binding.kind.lease_ms()),
        retained_until: now.saturating_add(RETENTION_MS),
        closed: false,
    };
    write(
        db,
        "INSERT INTO agent_message_sessions
            (id,owner_key,binding_key,inbox_id,label,kind,last_seen,active_until,retained_until)
         VALUES(?,?,?,?,?,?,?,?,?)",
        params![
            session.id,
            binding.owner_key,
            binding.binding_key,
            session.inbox,
            session.label,
            binding.kind.as_str(),
            session.last_seen,
            session.active_until,
            session.retained_until
        ],
    )?;
    Ok(session)
}

pub(super) fn renew_in(
    db: &Connection,
    binding: &SessionBinding,
    close: bool,
    now: i64,
) -> Result<(), NoticeError> {
    if binding.kind != SessionKind::Stdio {
        return Err(NoticeError::Identity);
    }
    if close {
        binding.close();
        write(
            db,
            "UPDATE agent_message_sessions SET active_until=0,closed=1
             WHERE owner_key=?1 AND binding_key=?2 AND kind='stdio'
               AND inbox_id IN (SELECT id FROM agent_inboxes WHERE recipient=?3)",
            params![
                binding.owner_key,
                binding.binding_key,
                binding.recipient.key()?
            ],
        )?;
    } else if !binding.is_closed()
        && let Some(mut session) = find(db, binding, now)?.filter(|session| !session.closed)
    {
        touch(db, &mut session, binding.kind, now)?;
    }
    Ok(())
}

pub(super) fn enrollment(
    db: &Connection,
    binding: SessionBinding,
    now: i64,
) -> Result<MessageEnrollment, NoticeError> {
    let session = find(db, &binding, now)?
        .filter(|session| !session.closed)
        .ok_or(NoticeError::NotFound)?;
    Ok(MessageEnrollment {
        session_id: session.id,
        inbox_id: session.inbox,
        binding,
    })
}

fn sessions(db: &Connection, owner: &str, now: i64) -> Result<Value, NoticeError> {
    let mut query = db.prepare(
        "SELECT s.id,s.inbox_id,s.label,s.last_seen,s.active_until,s.retained_until,s.closed
         FROM agent_message_sessions s JOIN agent_inboxes i ON i.id=s.inbox_id
         WHERE s.owner_key=?1 AND s.retained_until>?2 AND i.expires>?2
         ORDER BY s.label,s.id LIMIT 32",
    )?;
    let values = query
        .query_map(params![owner, now], Session::from_row)?
        .map(|row| row.map(|session| session.view(now)))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({"sessions":values}))
}

pub(super) fn execute_in(
    db: &Connection,
    binding: &SessionBinding,
    request: MessageRequest,
    now: i64,
) -> Result<MessageOutcome, NoticeError> {
    binding.validate()?;
    request.validate()?;
    if binding.is_closed() {
        return Err(NoticeError::NotFound);
    }
    prune(db, now)?;
    if let MessageRequest::Register(params) = request {
        return Ok(MessageOutcome::reply(
            register(db, binding, params.label, now)?.view(now),
        ));
    }
    let mut session = find(db, binding, now)?
        .filter(|session| !session.closed)
        .ok_or(NoticeError::NotFound)?;
    touch(db, &mut session, binding.kind, now)?;
    let owner = &binding.owner_key;
    let value = match request {
        MessageRequest::Sessions(_) => sessions(db, owner, now)?,
        MessageRequest::Send(params) => return send(db, owner, &session, params, now),
        MessageRequest::Unread(params) => unread(db, owner, &session, params.limit, now)?,
        MessageRequest::Read(params) => read(db, owner, &session, &params.message_id, now)?,
        MessageRequest::Ack(params) => ack(db, owner, &session, &params.message_ids, now)?,
        MessageRequest::Register(_) => unreachable!("register handled above"),
    };
    Ok(MessageOutcome::reply(value))
}

fn targets(
    db: &Connection,
    owner: &str,
    target: &MessageTarget,
    now: i64,
) -> Result<Vec<(String, String)>, NoticeError> {
    match target {
        MessageTarget::Session { session_id } => {
            let row = db
                .query_row(
                    "SELECT s.id,s.inbox_id FROM agent_message_sessions s
                 JOIN agent_inboxes i ON i.id=s.inbox_id
                 WHERE s.owner_key=?1 AND s.id=?2 AND s.retained_until>?3 AND i.expires>?3
                   AND s.closed=0 AND (s.kind='http' OR s.active_until>?3)",
                    params![owner, session_id, now],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .ok_or(NoticeError::NotFound)?;
            Ok(vec![row])
        }
        MessageTarget::AllActive {} => {
            let mut query = db.prepare(
                "SELECT s.id,s.inbox_id FROM agent_message_sessions s
                 JOIN agent_inboxes i ON i.id=s.inbox_id
                 WHERE s.owner_key=?1 AND s.retained_until>?2 AND i.expires>?2
                   AND s.closed=0 AND s.active_until>?2
                 ORDER BY s.id LIMIT 33",
            )?;
            let rows = query
                .query_map(params![owner, now], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.len() > MAX_SESSIONS_PER_OWNER as usize {
                return Err(NoticeError::Full);
            }
            Ok(rows)
        }
    }
}

fn send(
    db: &Connection,
    owner: &str,
    sender: &Session,
    input: SendParams,
    now: i64,
) -> Result<MessageOutcome, NoticeError> {
    let request = serde_json::to_vec(&(&input.to, &input.body, &input.summary))
        .map_err(|_| NoticeError::Invalid)?;
    let digest = hex::encode(Sha256::digest(request));
    let old: Option<(String, String, String)> = db
        .query_row(
            "SELECT id,request_digest,recipient_ids FROM agent_messages
         WHERE sender_id=?1 AND idempotency_key=?2 AND owner_key=?3 AND expires>?4",
            params![sender.id, input.idempotency_key, owner, now],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((id, stored_digest, recipients)) = old {
        if stored_digest != digest {
            return Err(NoticeError::Conflict);
        }
        let recipients: Vec<String> =
            serde_json::from_str(&recipients).map_err(|_| NoticeError::Unavailable)?;
        return Ok(MessageOutcome::reply(json!({
            "message_id":id, "recipient_ids":recipients, "duplicate":true
        })));
    }

    let targets = targets(db, owner, &input.to, now)?;
    let inboxes: Vec<_> = targets.iter().map(|(_, inbox)| inbox.clone()).collect();
    let recipients: Vec<_> = targets.iter().map(|(id, _)| id.clone()).collect();
    let mut admission = admit_notice_targets(db, &inboxes)?;
    let (total, owned): (i64, i64) = db.query_row(
        "SELECT count(*),count(CASE WHEN owner_key=?1 THEN 1 END) FROM agent_messages",
        [owner],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if total >= MAX_MESSAGES || owned >= MAX_MESSAGES_PER_OWNER {
        return Err(NoticeError::Full);
    }

    let id = format!("message_{}", ulid::Ulid::new());
    let expires = now.saturating_add(RETENTION_MS);
    let summary = input
        .summary
        .unwrap_or_else(|| format!("Message from {}.", sender.label));
    let view = MessageView {
        header: MessageHeader {
            message_id: id,
            sender_session_id: sender.id.clone(),
            sender_label: sender.label.clone(),
            summary,
            created_at_unix_ms: now,
            expires_at_unix_ms: expires,
        },
        body: input.body,
    };
    check_view(&view)?;
    let id = &view.header.message_id;
    write(
        db,
        "INSERT INTO agent_messages
            (id,owner_key,sender_id,sender_label,body,summary,request_digest,
             idempotency_key,recipient_ids,created,expires)
         VALUES(?,?,?,?,?,?,?,?,?,?,?)",
        params![
            id,
            owner,
            sender.id,
            sender.label,
            view.body,
            view.header.summary,
            digest,
            input.idempotency_key,
            serde_json::to_string(&recipients).map_err(|_| NoticeError::Invalid)?,
            now,
            expires
        ],
    )?;
    for (session_id, inbox) in targets {
        // Retain the address through the body lifetime without making an
        // inactive HTTP target appear active.
        write(
            db,
            "UPDATE agent_message_sessions SET retained_until=max(retained_until,?2) WHERE id=?1",
            params![session_id, expires],
        )?;
        write(
            db,
            "UPDATE agent_inboxes SET expires=max(expires,?2) WHERE id=?1",
            params![inbox, expires],
        )?;
        let notice = PublishNotice {
            inbox_id: inbox,
            source: MESSAGE_SOURCE.to_owned(),
            level: NoticeLevel::Info,
            message: view.header.summary.clone(),
            dedupe_key: id.clone(),
            ttl_seconds: (RETENTION_MS / 1000) as u32,
        };
        insert_notice_in(db, &mut admission, &sender.id, &notice, now, Some(id))?;
    }
    Ok(MessageOutcome {
        value: json!({"message_id":id,"recipient_ids":recipients,"duplicate":false}),
        wake: !inboxes.is_empty(),
    })
}

#[derive(Serialize)]
struct MessageHeader {
    message_id: String,
    sender_session_id: String,
    sender_label: String,
    summary: String,
    created_at_unix_ms: i64,
    expires_at_unix_ms: i64,
}

impl MessageHeader {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            message_id: row.get(0)?,
            sender_session_id: row.get(1)?,
            sender_label: row.get(2)?,
            summary: row.get(3)?,
            created_at_unix_ms: row.get(4)?,
            expires_at_unix_ms: row.get(5)?,
        })
    }
}

#[derive(Serialize)]
struct MessageView {
    #[serde(flatten)]
    header: MessageHeader,
    body: String,
}

fn check_view(view: &MessageView) -> Result<(), NoticeError> {
    let raw = serde_json::to_vec(view).map_err(|_| NoticeError::Invalid)?;
    if raw.len() > MAX_READ_VIEW_BYTES {
        return Err(NoticeError::Invalid);
    }
    Ok(())
}

fn read(
    db: &Connection,
    owner: &str,
    session: &Session,
    id: &str,
    now: i64,
) -> Result<Value, NoticeError> {
    let view = db
        .query_row(
            "SELECT m.id,m.sender_id,m.sender_label,m.summary,m.created,m.expires,m.body
         FROM agent_messages m JOIN agent_notices n ON n.message_id=m.id
         WHERE m.owner_key=?1 AND n.inbox_id=?2 AND m.id=?3 AND m.expires>?4 AND n.expires>?4",
            params![owner, session.inbox, id, now],
            |r| {
                Ok(MessageView {
                    header: MessageHeader::from_row(r)?,
                    body: r.get(6)?,
                })
            },
        )
        .optional()?
        .ok_or(NoticeError::NotFound)?;
    check_view(&view)?;
    serde_json::to_value(view).map_err(|_| NoticeError::Invalid)
}

fn unread(
    db: &Connection,
    owner: &str,
    session: &Session,
    limit: u32,
    now: i64,
) -> Result<Value, NoticeError> {
    let mut query = db.prepare(
        "SELECT m.id,m.sender_id,m.sender_label,m.summary,m.created,m.expires
         FROM agent_messages m JOIN agent_notices n ON n.message_id=m.id
         WHERE m.owner_key=?1 AND n.inbox_id=?2 AND m.expires>?3 AND n.expires>?3
           AND n.acknowledged=0 ORDER BY m.created,m.id LIMIT ?4",
    )?;
    let mut messages = query
        .query_map(
            params![owner, session.inbox, now, limit + 1],
            MessageHeader::from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = messages.len() > limit as usize;
    messages.truncate(limit as usize);
    Ok(json!({"messages":messages,"more":more}))
}

fn ack(
    db: &Connection,
    owner: &str,
    session: &Session,
    ids: &[String],
    now: i64,
) -> Result<Value, NoticeError> {
    let mut acknowledged = Vec::new();
    for id in ids {
        if acknowledged.contains(id) {
            continue;
        }
        let notice: Option<String> = db
            .query_row(
                "UPDATE agent_notices SET acknowledged=1
             WHERE inbox_id=?1 AND message_id=?2 AND expires>?3
               AND EXISTS(SELECT 1 FROM agent_messages WHERE id=?2 AND owner_key=?4 AND expires>?3)
             RETURNING id",
                params![session.inbox, id, now, owner],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(notice) = notice {
            stop_notice_delivery_in(db, &notice)?;
            acknowledged.push(id.clone());
        }
    }
    Ok(json!({"acknowledged":acknowledged}))
}
