use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};

use super::super::codemode_notices::{
    NoticeConsumer, NoticeError, NoticeStore, prune, register_in,
};
use super::types::*;

impl NoticeStore {
    pub(super) async fn check_subscription_capacity(
        &self,
        recipient: String,
        id: String,
        now: i64,
    ) -> Result<(), NoticeError> {
        self.run_event(move |db| check_capacity(db, &recipient, &id, now))
            .await
    }

    #[cfg(test)]
    pub(super) async fn activate_event_subscription(
        &self,
        owner: EventOwnerBinding,
        validated: ValidatedSubscription,
        now: i64,
    ) -> Result<EventSubscription, NoticeError> {
        self.run_event(move |db| activate(db, &owner, &validated, now))
            .await
    }

    pub(super) async fn remove_event_subscription(
        &self,
        owner: EventOwnerBinding,
        target: SubscriptionTarget,
        now: i64,
        complete: impl FnOnce(Option<&str>) + Send + 'static,
    ) -> Result<(), NoticeError> {
        let id = target.id(&owner.recipient)?;
        let key = owner.recipient.key()?;
        self.run_event(move |db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let authority: Option<String> = tx.query_row(
                "SELECT authority_key FROM agent_event_subscriptions WHERE id=? AND recipient=?",
                params![id, key], |row| row.get(0)
            ).optional()?;
            disable(&tx, &id, &key)?;
            prune(&tx, now)?;
            tx.commit()?;
            complete(authority.as_deref());
            Ok(())
        })
        .await
    }
}

pub(super) fn activate(
    db: &mut Connection,
    owner: &EventOwnerBinding,
    validated: &ValidatedSubscription,
    now: i64,
) -> Result<EventSubscription, NoticeError> {
    let id = validated.target.id(&owner.recipient)?;
    let recipient = owner.recipient.key()?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let is_message_inbox = matches!(
        owner.recipient.consumer,
        NoticeConsumer::MessageSession { .. }
    );
    let inbox_id = match (
        &validated.enrollment,
        validated.target.filter.session_messages,
        is_message_inbox,
    ) {
        (Some(enrollment), true, true) if enrollment.binding.recipient == owner.recipient => {
            NoticeStore::retain_message_enrollment_in(&tx, enrollment, now, validated.expires)?
        }
        (None, false, false) => register_in(&tx, &recipient, owner.recipient.scope(), now)?.id,
        _ => return Err(NoticeError::Identity),
    };
    let existing: Option<(bool, String)> = tx.query_row(
        "SELECT active=1 AND expires>?3,authority_key FROM agent_event_subscriptions WHERE id=?1 AND recipient=?2",
        params![id, recipient, now],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    let was_active = existing.as_ref().is_some_and(|(active, _)| *active);
    if !was_active {
        check_capacity(&tx, &recipient, &id, now)?;
    }
    if existing.is_some() && !was_active {
        // An expired/inactive subscription never resumes an old queue. Keep
        // in-flight leases counted until they finish or time out.
        disable(&tx, &id, &recipient)?;
    }
    let generation: i64 = tx.query_row(
        "INSERT INTO agent_event_subscriptions
         (id,recipient,inbox_id,authority_key,callback,source,secret,secret_fingerprint,expires,generation,session_messages)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,1,?12)
         ON CONFLICT(id) DO UPDATE SET
           authority_key=excluded.authority_key, expires=excluded.expires, active=1,
           generation=agent_event_subscriptions.generation+1,
           previous_secret=CASE
             WHEN agent_event_subscriptions.active=0 OR agent_event_subscriptions.expires<=?10 THEN NULL
             WHEN excluded.secret_fingerprint<>agent_event_subscriptions.secret_fingerprint THEN agent_event_subscriptions.secret
             WHEN previous_until>?10 THEN previous_secret ELSE NULL END,
           previous_until=CASE
             WHEN agent_event_subscriptions.active=0 OR agent_event_subscriptions.expires<=?10 THEN 0
             WHEN excluded.secret_fingerprint<>agent_event_subscriptions.secret_fingerprint THEN ?11
             WHEN previous_until>?10 THEN previous_until ELSE 0 END,
           secret=excluded.secret, secret_fingerprint=excluded.secret_fingerprint
         WHERE agent_event_subscriptions.recipient=excluded.recipient
           AND agent_event_subscriptions.inbox_id=excluded.inbox_id
           AND agent_event_subscriptions.session_messages=excluded.session_messages
         RETURNING generation",
        params![id, recipient, inbox_id, owner.authority_key, validated.target.url.as_str(),
                validated.target.filter.source, validated.sealed_key, validated.key_fingerprint,
                validated.expires, now, now.saturating_add(ROTATION_MS), validated.target.filter.session_messages],
        |row| row.get(0),
    ).optional()?.ok_or(NoticeError::NotFound)?;
    tx.execute(
        "UPDATE agent_event_deliveries SET generation=?2 WHERE subscription_id=?1 AND status='pending'",
        params![id, generation],
    )?;
    tx.commit()?;
    Ok(EventSubscription {
        id,
        expires: validated.expires,
        replaced_authority: existing.map(|(_, key)| key),
    })
}

pub(super) fn disable(db: &Connection, id: &str, recipient: &str) -> Result<(), NoticeError> {
    if db.execute(
        "UPDATE agent_event_subscriptions SET active=0
         WHERE id=?1 AND recipient=?2 AND active=1",
        params![id, recipient],
    )? > 0
    {
        db.execute(
            "UPDATE agent_event_deliveries SET status='stopped' WHERE subscription_id=?1 AND status='pending'",
            [id],
        )?;
    }
    Ok(())
}

fn check_capacity(db: &Connection, recipient: &str, id: &str, now: i64) -> Result<(), NoticeError> {
    let (total, owned, existing): (i64, i64, i64) = db.query_row(
        "SELECT count(*), count(CASE WHEN recipient=?1 THEN 1 END),
                          count(CASE WHEN id=?2 AND recipient=?1 THEN 1 END)
         FROM agent_event_subscriptions WHERE active=1 AND expires>?3",
        params![recipient, id, now],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if existing == 0 && (total >= MAX_SUBSCRIPTIONS as i64 || owned >= MAX_PER_INBOX) {
        Err(NoticeError::Full)
    } else {
        Ok(())
    }
}
