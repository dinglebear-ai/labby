use super::{SqliteStore, sqlite_error};
use crate::error::AuthError;
use crate::types::{BrowserReauthChallengeRow, BrowserReauthResult};
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};

const MAX_CHALLENGES_GLOBAL: i64 = 64;
const MAX_CHALLENGES_PER_SESSION: i64 = 2;
const MAX_CHALLENGE_TTL: i64 = 300;

impl SqliteStore {
    pub async fn insert_browser_reauth_challenge(
        &self,
        row: BrowserReauthChallengeRow,
    ) -> Result<(), AuthError> {
        validate(&row)?;
        self.with_conn(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let now = crate::util::now_unix();
            tx.execute(
                "DELETE FROM browser_reauth_challenges WHERE expires_at <= ?1",
                params![now],
            )
            .map_err(sqlite_error)?;
            if row.expires_at <= now {
                return Err(AuthError::Validation(
                    "browser reauthentication challenge is expired".into(),
                ));
            }
            let (global, session): (i64, i64) = tx
                .query_row(
                    "SELECT COUNT(*), COALESCE(SUM(session_id = ?1), 0) FROM browser_reauth_challenges",
                    params![&row.session_id],
                    |record| Ok((record.get(0)?, record.get(1)?)),
                )
                .map_err(sqlite_error)?;
            if global >= MAX_CHALLENGES_GLOBAL || session >= MAX_CHALLENGES_PER_SESSION {
                return Err(AuthError::RateLimited {
                    message: "browser reauthentication challenge capacity reached".into(),
                    retry_after_ms: 1_000,
                });
            }
            tx.execute(
                "INSERT INTO browser_reauth_challenges (state, interaction_hash, session_id, subject, provider_code_verifier, nonce, purpose_json, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![row.state, row.interaction_hash.as_slice(), row.session_id, row.subject, row.provider_code_verifier, row.nonce, row.purpose_json, row.created_at, row.expires_at],
            ).map_err(sqlite_error)?;
            tx.commit().map_err(sqlite_error)
        }).await
    }

    pub async fn take_browser_reauth_challenge(
        &self,
        state: &str,
    ) -> Result<Option<BrowserReauthChallengeRow>, AuthError> {
        let state = state.to_string();
        self.with_conn(move |conn| {
            let now = crate::util::now_unix();
            conn.query_row(
                "UPDATE browser_reauth_challenges SET status = 1 WHERE state = ?1 AND status = 0 AND expires_at > ?2 RETURNING state, interaction_hash, session_id, subject, provider_code_verifier, nonce, purpose_json, created_at, expires_at",
                params![state, now],
                row,
            ).optional().map_err(sqlite_error)
        }).await
    }

    pub async fn complete_browser_reauth(&self, state: &str, proof: &str) -> Result<(), AuthError> {
        if proof.len() != 43 {
            return Err(AuthError::Validation(
                "invalid reauthentication proof".into(),
            ));
        }
        let state = state.to_string();
        let proof = proof.to_string();
        self.with_conn(move |conn| {
            let changed = conn.execute(
                "UPDATE browser_reauth_challenges SET status = 2, proof = ?2 WHERE state = ?1 AND status = 1 AND expires_at > ?3",
                params![state, proof, crate::util::now_unix()],
            ).map_err(sqlite_error)?;
            if changed == 1 { Ok(()) } else { Err(AuthError::InvalidGrant("reauthentication challenge is unavailable".into())) }
        }).await
    }

    pub async fn retry_browser_reauth_challenge(&self, state: &str) -> Result<(), AuthError> {
        let state = state.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE browser_reauth_challenges SET status = 0 WHERE state = ?1 AND status = 1 AND expires_at > ?2",
                params![state, crate::util::now_unix()],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    pub async fn poll_browser_reauth(
        &self,
        interaction_hash: &[u8; 32],
        session_id: &str,
    ) -> Result<Option<BrowserReauthResult>, AuthError> {
        let interaction_hash = *interaction_hash;
        let session_id = session_id.to_string();
        self.with_conn(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sqlite_error)?;
            let now = crate::util::now_unix();
            tx.execute("DELETE FROM browser_reauth_challenges WHERE expires_at <= ?1", params![now]).map_err(sqlite_error)?;
            let result: Option<(i64, Option<String>)> = tx.query_row(
                "SELECT status, proof FROM browser_reauth_challenges WHERE interaction_hash = ?1 AND session_id = ?2",
                params![interaction_hash.as_slice(), session_id], |record| Ok((record.get(0)?, record.get(1)?)),
            ).optional().map_err(sqlite_error)?;
            let result = match result {
                Some((2, Some(proof))) => {
                    tx.execute("DELETE FROM browser_reauth_challenges WHERE interaction_hash = ?1", params![interaction_hash.as_slice()]).map_err(sqlite_error)?;
                    Some(BrowserReauthResult::Completed(proof))
                }
                Some(_) => Some(BrowserReauthResult::Pending),
                None => None,
            };
            tx.commit().map_err(sqlite_error)?;
            Ok(result)
        }).await
    }

    pub async fn cancel_browser_reauth(
        &self,
        interaction_hash: &[u8; 32],
        session_id: &str,
    ) -> Result<bool, AuthError> {
        let interaction_hash = *interaction_hash;
        let session_id = session_id.to_string();
        self.with_conn(move |conn| {
            conn.execute(
            "DELETE FROM browser_reauth_challenges WHERE interaction_hash = ?1 AND session_id = ?2",
            params![interaction_hash.as_slice(), session_id],
        ).map(|changed| changed == 1).map_err(sqlite_error)
        })
        .await
    }
}

fn validate(row: &BrowserReauthChallengeRow) -> Result<(), AuthError> {
    let bounded = !row.state.is_empty()
        && row.state.len() <= 128
        && !row.session_id.is_empty()
        && row.session_id.len() <= 128
        && !row.subject.is_empty()
        && row.subject.len() <= 1024
        && !row.provider_code_verifier.is_empty()
        && row.provider_code_verifier.len() <= 128
        && !row.nonce.is_empty()
        && row.nonce.len() <= 128
        && !row.purpose_json.is_empty()
        && row.purpose_json.len() <= 65_536
        && row.created_at <= row.expires_at
        && row.expires_at - row.created_at <= MAX_CHALLENGE_TTL;
    if bounded {
        Ok(())
    } else {
        Err(AuthError::Validation(
            "invalid browser reauthentication challenge".into(),
        ))
    }
}

fn row(record: &rusqlite::Row<'_>) -> rusqlite::Result<BrowserReauthChallengeRow> {
    let hash: Vec<u8> = record.get(1)?;
    let interaction_hash: [u8; 32] = hash.try_into().map_err(|_| {
        rusqlite::Error::InvalidColumnType(
            1,
            "interaction_hash".into(),
            rusqlite::types::Type::Blob,
        )
    })?;
    Ok(BrowserReauthChallengeRow {
        state: record.get(0)?,
        interaction_hash,
        session_id: record.get(2)?,
        subject: record.get(3)?,
        provider_code_verifier: record.get(4)?,
        nonce: record.get(5)?,
        purpose_json: record.get(6)?,
        created_at: record.get(7)?,
        expires_at: record.get(8)?,
    })
}
