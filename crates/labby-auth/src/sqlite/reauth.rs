//! Parameterized, transactionally bounded proof redemption and rate accounting.
use super::{SqliteStore, sqlite_error};
use crate::error::AuthError;
use crate::reauth::{Outcome, ProofBinding, ProofError, ReservationState};
use rusqlite::{OptionalExtension as _, Transaction, TransactionBehavior, params};
use subtle::ConstantTimeEq as _;

impl SqliteStore {
    pub(crate) async fn reauth_insert(
        &self,
        binding: ProofBinding,
        authenticated_at: i64,
        expires_at: i64,
    ) -> Result<(), ProofError> {
        self.with_conn(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sqlite_error)?;
            let now = crate::util::now_unix();
            if !session_current(&tx, &binding, now)? { return finish(tx, Err(ProofError::Denied)); }
            cleanup(&tx, now)?;
            if !admit(&tx, "issue", &binding.actor, now, 30, 5)? { return finish(tx, Err(ProofError::RateLimited)); }
            let (global, session): (i64, i64) = tx.query_row(
                "SELECT COUNT(*), COALESCE(SUM(session = ?1), 0) FROM reauth_proofs",
                params![binding.session.as_slice()], |row| Ok((row.get(0)?, row.get(1)?)),
            ).map_err(sqlite_error)?;
            if global >= 128 || session >= 8 { return finish(tx, Err(ProofError::Capacity)); }
            if expires_at <= now { return finish(tx, Err(ProofError::Expired)); }
            tx.execute(
                "INSERT INTO reauth_proofs (nonce_hash, actor, session, authority, purpose, operation_id, authenticated_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![binding.hash.as_slice(), binding.actor.as_slice(), binding.session.as_slice(), binding.authority.as_slice(), binding.purpose.as_slice(), binding.operation, authenticated_at, expires_at],
            ).map_err(sqlite_error)?;
            finish(tx, Ok(()))
        }).await.map_err(|_| ProofError::Unavailable)?
    }

    pub(crate) async fn reauth_reserve(
        &self,
        binding: ProofBinding,
    ) -> Result<ReservationState, ProofError> {
        self.with_conn(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sqlite_error)?;
            let now = crate::util::now_unix();
            if !session_current(&tx, &binding, now)? { return finish(tx, Err(ProofError::Denied)); }
            cleanup(&tx, now)?;
            if !admit(&tx, "verify", &binding.actor, now, 120, 30)? { return finish(tx, Err(ProofError::RateLimited)); }
            let stored = tx.query_row(
                "SELECT actor, session, authority, purpose, operation_id, authenticated_at, expires_at, state FROM reauth_proofs WHERE nonce_hash = ?1",
                params![binding.hash.as_slice()], |row| Ok(Stored {
                    actor: row.get(0)?, session: row.get(1)?, authority: row.get(2)?, purpose: row.get(3)?, operation: row.get(4)?, authenticated_at: row.get(5)?, expires_at: row.get(6)?, state: row.get(7)?,
                }),
            ).optional().map_err(sqlite_error)?;
            let Some(stored) = stored else { return finish(tx, Err(ProofError::Expired)); };
            if !same(&stored.actor, &binding.actor) || !same(&stored.session, &binding.session) || !same(&stored.authority, &binding.authority) { return finish(tx, Err(ProofError::Denied)); }
            if !same(&stored.purpose, &binding.purpose) || stored.operation != binding.operation { return finish(tx, Err(ProofError::Replayed)); }
            if stored.expires_at <= now || stored.authenticated_at <= now - 300 || stored.authenticated_at > now { return finish(tx, Err(ProofError::Expired)); }
            let state = match stored.state {
                0 | 1 => {
                    tx.execute("UPDATE reauth_proofs SET state = 1 WHERE nonce_hash = ?1 AND state = 0", params![binding.hash.as_slice()]).map_err(sqlite_error)?;
                    ReservationState::Reserved
                }
                2 => ReservationState::Finalized(Outcome::Committed),
                3 => ReservationState::Finalized(Outcome::Aborted),
                _ => return finish(tx, Err(ProofError::Unavailable)),
            };
            finish(tx, Ok(state))
        }).await.map_err(|_| ProofError::Unavailable)?
    }

    pub(crate) async fn reauth_finalize(
        &self,
        binding: ProofBinding,
        outcome: Outcome,
    ) -> Result<(), ProofError> {
        self.with_conn(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sqlite_error)?;
            let desired = match outcome { Outcome::Committed => 2, Outcome::Aborted => 3 };
            let changed = tx.execute(
                "UPDATE reauth_proofs SET state = ?1 WHERE nonce_hash = ?2 AND operation_id = ?3 AND purpose = ?4 AND actor = ?5 AND state IN (1, ?1)",
                params![desired, binding.hash.as_slice(), binding.operation, binding.purpose.as_slice(), binding.actor.as_slice()],
            ).map_err(sqlite_error)?;
            if changed == 0 {
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM reauth_proofs WHERE nonce_hash = ?1)", params![binding.hash.as_slice()], |row| row.get(0)).map_err(sqlite_error)?;
                if exists { return finish(tx, Err(ProofError::Replayed)); }
                // Expired/deleted proofs are already inactive. Durable operation
                // outcomes are retained by the caller's transaction journal.
            }
            finish(tx, Ok(()))
        }).await.map_err(|_| ProofError::Unavailable)?
    }
}
struct Stored {
    actor: Vec<u8>,
    session: Vec<u8>,
    authority: Vec<u8>,
    purpose: Vec<u8>,
    operation: String,
    authenticated_at: i64,
    expires_at: i64,
    state: i64,
}
fn same(stored: &[u8], expected: &[u8; 32]) -> bool {
    bool::from(stored.ct_eq(expected))
}
fn session_current(
    tx: &Transaction<'_>,
    binding: &ProofBinding,
    now: i64,
) -> Result<bool, AuthError> {
    let current = tx.query_row(
        "SELECT session_id, subject, email, csrf_token, created_at, expires_at, project_binding_json FROM browser_sessions WHERE session_id = ?1 AND expires_at > ?2",
        params![binding.session_snapshot.session_id, now], super::rows::row_to_browser_session,
    ).optional().map_err(sqlite_error)?;
    Ok(current.as_ref() == Some(&binding.session_snapshot))
}
fn cleanup(tx: &Transaction<'_>, now: i64) -> Result<(), AuthError> {
    tx.execute(
        "DELETE FROM reauth_proofs WHERE expires_at <= ?1",
        params![now],
    )
    .map_err(sqlite_error)?;
    tx.execute(
        "DELETE FROM reauth_attempts WHERE at <= ?1",
        params![now - 60],
    )
    .map_err(sqlite_error)?;
    Ok(())
}
fn admit(
    tx: &Transaction<'_>,
    kind: &str,
    actor: &[u8; 32],
    now: i64,
    global_cap: i64,
    actor_cap: i64,
) -> Result<bool, AuthError> {
    let (global, count): (i64, i64) = tx
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(actor = ?1), 0) FROM reauth_attempts WHERE kind = ?2",
            params![actor.as_slice(), kind],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sqlite_error)?;
    if global >= global_cap || count >= actor_cap {
        return Ok(false);
    }
    tx.execute(
        "INSERT INTO reauth_attempts (kind, actor, at) VALUES (?1, ?2, ?3)",
        params![kind, actor.as_slice(), now],
    )
    .map_err(sqlite_error)?;
    Ok(true)
}
fn finish<T>(
    tx: Transaction<'_>,
    result: Result<T, ProofError>,
) -> Result<Result<T, ProofError>, AuthError> {
    tx.commit().map_err(sqlite_error)?;
    Ok(result)
}
