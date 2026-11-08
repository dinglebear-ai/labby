use std::{sync::Arc, time::Duration};

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::super::codemode_notices::{NoticeError, PublishNotice, now_ms, prune};
use super::subscriptions::disable;
use super::types::{EVENT_NAME, EventError, WebhookSecret, parse_callback, timestamp};
use super::webhook::{DeliveryOutcome, WebhookAttempt};
use super::{AgentEvents, EventAuthorityDecision};

const MAX_IN_FLIGHT: usize = 8;
const LEASE_MS: i64 = 30_000;
const RETRY_DELAYS_MS: [i64; 7] = [1_000, 5_000, 15_000, 30_000, 60_000, 120_000, 300_000];

pub(in crate::dispatch) fn enqueue(
    tx: &Connection,
    notice_id: &str,
    input: &PublishNotice,
    now: i64,
    expires: i64,
) -> Result<(), NoticeError> {
    let queued = tx.execute(
        "INSERT INTO agent_event_deliveries(subscription_id,notice_id,generation,next_attempt)
         SELECT id,?1,generation,?2 FROM agent_event_subscriptions
         WHERE inbox_id=?3 AND active=1 AND expires>?2 AND (source IS NULL OR source=?4)",
        params![notice_id, now, input.inbox_id, input.source],
    )?;
    if queued != 0 {
        let body = serde_json::to_vec(&serde_json::json!({
            "eventId": notice_id,
            "name": EVENT_NAME,
            "timestamp": timestamp(now)?,
            "cursor": null,
            "data": {"id": notice_id, "source": input.source, "level": input.level,
                     "message": input.message, "expires_at_unix_ms": expires}
        }))
        .map_err(|_| NoticeError::Invalid)?;
        tx.execute(
            "UPDATE agent_notices SET event_body=? WHERE id=?",
            params![body, notice_id],
        )?;
    }
    Ok(())
}

struct Claim {
    subscription: String,
    notice: String,
    recipient: String,
    authority: String,
    generation: i64,
    token: String,
    callback: String,
    key: Vec<u8>,
    previous: Option<Vec<u8>>,
    previous_until: i64,
    body: Vec<u8>,
    expires: i64,
    attempts: usize,
}

fn claim_next(
    db: &mut Connection,
    authority_keys_json: &str,
    now: i64,
) -> Result<Option<Claim>, NoticeError> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    prune(&tx, now)?;
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM agent_event_deliveries WHERE claim_until>?1",
        [now],
        |row| row.get(0),
    )?;
    if count >= MAX_IN_FLIGHT as i64 {
        tx.commit()?;
        return Ok(None);
    }
    let claim = tx
        .query_row(
            "SELECT s.id,n.id,s.recipient,s.authority_key,s.generation,s.callback,s.secret,
                CASE WHEN s.previous_until>?1 THEN s.previous_secret END,n.event_body,
                min(s.expires,n.expires,i.expires),d.attempts,s.previous_until
         FROM agent_event_deliveries d JOIN agent_event_subscriptions s ON s.id=d.subscription_id
         JOIN agent_notices n ON n.id=d.notice_id AND n.inbox_id=s.inbox_id
         JOIN agent_inboxes i ON i.id=s.inbox_id AND i.recipient=s.recipient
         WHERE d.status='pending' AND d.generation=s.generation AND d.attempts<8
           AND d.next_attempt<=?1 AND s.active=1 AND s.expires>?1 AND n.expires>?1
           AND i.expires>?1 AND n.acknowledged=0
           AND s.authority_key IN (SELECT value FROM json_each(?2))
           AND NOT EXISTS (SELECT 1 FROM agent_event_deliveries lease
                 WHERE lease.subscription_id=s.id AND lease.claim_until>?1)
         ORDER BY d.next_attempt,n.created,s.id,n.id LIMIT 1",
            params![now, authority_keys_json],
            |row| {
                Ok(Claim {
                    subscription: row.get(0)?,
                    notice: row.get(1)?,
                    recipient: row.get(2)?,
                    authority: row.get(3)?,
                    generation: row.get(4)?,
                    token: ulid::Ulid::new().to_string(),
                    callback: row.get(5)?,
                    key: row.get(6)?,
                    previous: row.get(7)?,
                    body: row.get(8)?,
                    expires: row.get(9)?,
                    attempts: usize::from(row.get::<_, u8>(10)?),
                    previous_until: row.get(11)?,
                })
            },
        )
        .optional()?;
    if let Some(claim) = &claim {
        tx.execute(
            "UPDATE agent_event_deliveries SET claim_token=?3,claim_until=?4
             WHERE subscription_id=?1 AND notice_id=?2",
            params![
                claim.subscription,
                claim.notice,
                claim.token,
                now.saturating_add(LEASE_MS)
            ],
        )?;
    }
    tx.commit()?;
    Ok(claim)
}

fn retry_delay(claim: &Claim) -> i64 {
    RETRY_DELAYS_MS.get(claim.attempts).copied().unwrap_or(0)
}

fn begin(db: &Connection, claim: &Claim, now: i64) -> Result<bool, NoticeError> {
    let next_attempt = now.saturating_add(retry_delay(claim));
    let changed = db.execute(
        "UPDATE agent_event_deliveries SET attempts=attempts+1,next_attempt=?6
         WHERE subscription_id=?1 AND notice_id=?2 AND claim_token=?3 AND generation=?4
           AND claim_until>?5 AND status='pending' AND attempts<8
           AND EXISTS (SELECT 1 FROM agent_event_subscriptions s
                WHERE s.id=?1 AND s.active=1 AND s.expires>?5 AND s.generation=?4)
           AND EXISTS (SELECT 1 FROM agent_notices n WHERE n.id=?2 AND n.expires>?5 AND n.acknowledged=0)",
        params![claim.subscription, claim.notice, claim.token, claim.generation, now, next_attempt],
    )?;
    Ok(changed == 1)
}

fn mark_offered(db: &Connection, claim: &Claim, now: i64) -> Result<bool, NoticeError> {
    Ok(db.execute(
        "UPDATE agent_notices SET webhook_offered=1
         WHERE id=?1 AND expires>?4 AND acknowledged=0 AND EXISTS (
             SELECT 1 FROM agent_event_deliveries d
             JOIN agent_event_subscriptions s ON s.id=d.subscription_id
             WHERE d.notice_id=?1 AND d.subscription_id=?2 AND d.claim_token=?3
               AND d.claim_until>?4 AND d.generation=?5 AND d.status='pending'
               AND s.active=1 AND s.expires>?4 AND s.generation=?5
         )",
        params![
            claim.notice,
            claim.subscription,
            claim.token,
            now,
            claim.generation
        ],
    )? == 1)
}

#[derive(Clone, Copy)]
enum Finish {
    Outcome(DeliveryOutcome),
    Pause,
    Revoke,
    Release,
}

fn finish(db: &mut Connection, claim: &Claim, result: Finish, now: i64) -> Result<(), NoticeError> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = tx
        .query_row(
            "UPDATE agent_event_deliveries SET claim_token=NULL,claim_until=0
         WHERE subscription_id=?1 AND notice_id=?2 AND claim_token=?3
         RETURNING generation=?4 AND status='pending' AND EXISTS
             (SELECT 1 FROM agent_event_subscriptions s
              WHERE s.id=?1 AND s.active=1 AND s.generation=?4 AND s.expires>?5)",
            params![
                claim.subscription,
                claim.notice,
                claim.token,
                claim.generation,
                now
            ],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false);
    if current {
        match result {
            Finish::Revoke | Finish::Outcome(DeliveryOutcome::Gone) => {
                disable(&tx, &claim.subscription, &claim.recipient)?;
            }
            Finish::Outcome(DeliveryOutcome::Received) => mark_status(&tx, claim, "received")?,
            Finish::Outcome(DeliveryOutcome::Stop) => mark_status(&tx, claim, "stopped")?,
            Finish::Outcome(DeliveryOutcome::Retry { .. }) if claim.attempts >= 7 => {
                mark_status(&tx, claim, "stopped")?;
            }
            Finish::Outcome(DeliveryOutcome::Retry { after_ms }) => {
                defer(
                    &tx,
                    claim,
                    now.saturating_add(retry_delay(claim).max(after_ms.clamp(0, 300_000))),
                )?;
            }
            Finish::Pause => defer(&tx, claim, now.saturating_add(1_000))?,
            Finish::Release => {}
        }
    }
    tx.commit()?;
    Ok(())
}

fn mark_status(db: &Connection, claim: &Claim, status: &str) -> Result<(), NoticeError> {
    db.execute(
        "UPDATE agent_event_deliveries SET status=?3 WHERE subscription_id=?1 AND notice_id=?2",
        params![claim.subscription, claim.notice, status],
    )?;
    Ok(())
}

fn defer(db: &Connection, claim: &Claim, at: i64) -> Result<(), NoticeError> {
    db.execute(
        "UPDATE agent_event_deliveries SET next_attempt=max(next_attempt,?3)
         WHERE subscription_id=?1 AND notice_id=?2",
        params![claim.subscription, claim.notice, at],
    )?;
    Ok(())
}

impl AgentEvents {
    pub(crate) async fn run(self: Arc<Self>, cancel: CancellationToken) {
        let mut pending = JoinSet::new();
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {}
                _ = self.store.published() => {}
                _ = pending.join_next(), if !pending.is_empty() => {}
            }
            while pending.len() < MAX_IN_FLIGHT && !cancel.is_cancelled() {
                let Some(keys) = self.authority_keys() else {
                    break;
                };
                let claim = self
                    .store
                    .run_event(move |db| claim_next(db, &keys, now_ms()))
                    .await;
                let Ok(Some(claim)) = claim else { break };
                let events = self.clone();
                let cancel = cancel.clone();
                pending.spawn(async move {
                    events.deliver(Arc::new(claim), cancel).await;
                });
            }
        }
        // All requests see the same cancellation and release their leases;
        // a contended DB can defer recovery until the existing lease expires.
        while pending.join_next().await.is_some() {}
    }

    async fn deliver(&self, claim: Arc<Claim>, cancel: CancellationToken) {
        let started_at = std::time::Instant::now();
        let Some(authority_cancel) = self.authority_cancellation(&claim.authority) else {
            let _ = self
                .store
                .run_event(move |db| finish(db, &claim, Finish::Release, now_ms()))
                .await;
            return;
        };
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Finish::Release,
            _ = authority_cancel.cancelled() => Finish::Release,
            result = self.deliver_inner(&claim) => result,
        };
        tracing::debug!(
            subsystem = "agent_events",
            subscription_id = %claim.subscription,
            notice_id = %claim.notice,
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            outcome = match result {
                Finish::Outcome(DeliveryOutcome::Received) => "received",
                Finish::Outcome(DeliveryOutcome::Retry { .. }) => "retry",
                Finish::Outcome(DeliveryOutcome::Stop) => "stopped",
                Finish::Outcome(DeliveryOutcome::Gone) => "gone",
                Finish::Pause => "refresh_required",
                Finish::Revoke => "revoked",
                Finish::Release => "released",
            },
            "MCP event delivery attempt finished"
        );
        for attempt in 0..3 {
            let claim_copy = claim.clone();
            match self
                .store
                .run_event(move |db| finish(db, &claim_copy, result, now_ms()))
                .await
            {
                Err(NoticeError::Busy) if attempt < 2 => {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                }
                _ => break,
            }
        }
        if matches!(
            result,
            Finish::Revoke | Finish::Outcome(DeliveryOutcome::Gone)
        ) {
            self.forget_authority(&claim.authority);
        }
    }

    async fn deliver_inner(&self, claim: &Arc<Claim>) -> Finish {
        match self.check_authority(&claim.authority).await {
            EventAuthorityDecision::Current => {}
            EventAuthorityDecision::Revoked => return Finish::Revoke,
            EventAuthorityDecision::RefreshRequired => return Finish::Pause,
        }
        let attempt = match self.attempt(claim) {
            Ok(attempt) => attempt,
            Err(_) => return Finish::Outcome(DeliveryOutcome::Stop),
        };
        let copy = claim.clone();
        match self
            .store
            .run_event(move |db| begin(db, &copy, now_ms()))
            .await
        {
            Ok(true) => {}
            Ok(false) => return Finish::Release,
            Err(_) => return Finish::Pause,
        }
        let remaining = claim.expires.saturating_sub(now_ms());
        if remaining <= 0 {
            return Finish::Release;
        }
        match tokio::time::timeout(
            Duration::from_millis(remaining as u64),
            self.sender.send(attempt),
        )
        .await
        {
            Ok(result) => Finish::Outcome(result),
            Err(_) => Finish::Release,
        }
    }

    fn attempt(&self, claim: &Arc<Claim>) -> Result<WebhookAttempt, EventError> {
        let key = |ciphertext| {
            self.open_key(&claim.subscription, ciphertext)
                .and_then(WebhookSecret::from_bytes)
        };
        let witness = {
            let claim = claim.clone();
            let store = self.store.clone();
            Box::new(
                move || -> futures::future::BoxFuture<'static, std::io::Result<()>> {
                    Box::pin(async move {
                        for retry in 0..3 {
                            let claim = claim.clone();
                            match store
                                .run_event(move |db| mark_offered(db, &claim, now_ms()))
                                .await
                            {
                                Ok(true) => return Ok(()),
                                Err(NoticeError::Busy) if retry < 2 => {
                                    tokio::time::sleep(Duration::from_millis(30)).await;
                                }
                                _ => break,
                            }
                        }
                        Err(std::io::Error::other("webhook offer is no longer eligible"))
                    })
                },
            )
        };
        Ok(WebhookAttempt {
            url: parse_callback(&claim.callback)?,
            subscription_id: claim.subscription.clone(),
            event_id: claim.notice.clone(),
            body: claim.body.clone(),
            key: key(&claim.key)?,
            previous_key: claim
                .previous
                .as_deref()
                .map(key)
                .transpose()?
                .map(|key| (key, claim.previous_until)),
            offer: Some(witness),
        })
    }
}

#[cfg(test)]
mod tests;
