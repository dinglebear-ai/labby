use rusqlite::params;

use crate::error::AuthError;

use super::{SqliteStore, sqlite_error};

impl SqliteStore {
    /// Atomically consume a signed assertion identifier.
    ///
    /// The `(issuer, jti)` primary key makes replay rejection durable across
    /// process restarts and shared-database server instances.
    pub async fn consume_assertion_jti(
        &self,
        issuer: &str,
        jti: &str,
        issued_at: i64,
        expires_at: i64,
        now: i64,
    ) -> Result<bool, AuthError> {
        const MAX_ASSERTION_LIFETIME_SECS: i64 = 5 * 60;
        const MAX_ISSUER_BYTES: usize = 2_048;
        const MAX_JTI_BYTES: usize = 256;

        if issuer.is_empty()
            || issuer.len() > MAX_ISSUER_BYTES
            || jti.is_empty()
            || jti.len() > MAX_JTI_BYTES
            || issued_at > now.saturating_add(60)
            || expires_at <= now
            || expires_at.saturating_sub(issued_at) > MAX_ASSERTION_LIFETIME_SECS
        {
            return Ok(false);
        }
        let issuer = issuer.to_string();
        let jti = jti.to_string();
        self.with_conn(move |conn| {
            let transaction = conn.transaction().map_err(sqlite_error)?;
            transaction
                .execute(
                    "DELETE FROM assertion_jtis WHERE expires_at <= ?1",
                    params![now],
                )
                .map_err(sqlite_error)?;
            let inserted = transaction
                .execute(
                    "INSERT OR IGNORE INTO assertion_jtis (issuer, jti, expires_at)
                     VALUES (?1, ?2, ?3)",
                    params![issuer, jti, expires_at],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(inserted == 1)
        })
        .await
    }
}
