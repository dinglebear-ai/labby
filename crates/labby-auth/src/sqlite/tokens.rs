//! Refresh-token persistence operations.
//!
//! Raw refresh tokens are hashed before storage. Upstream provider refresh
//! tokens are encrypted at rest when the store has an encryption key.

use rusqlite::{OptionalExtension, params};

use crate::at_rest::{maybe_decrypt, maybe_encrypt};
use crate::error::AuthError;
use crate::types::RefreshTokenRow;
use crate::util::now_unix;

use super::{SqliteStore, hash_token, sqlite_error};

impl SqliteStore {
    /// Insert a new refresh token row, storing a SHA-256 hash of the raw token
    /// as the primary key. The plaintext token is never persisted.
    ///
    /// Use [`Self::rotate_refresh_token`] when replacing an existing token so
    /// the swap remains atomic.
    pub async fn upsert_refresh_token(&self, token: RefreshTokenRow) -> Result<(), AuthError> {
        let hash = hash_token(&token.refresh_token);
        let encrypted_provider_rt = token
            .provider_refresh_token
            .as_deref()
            .map(|raw| maybe_encrypt(self.enc_key.as_deref(), raw))
            .transpose()?;
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO refresh_tokens (
                    refresh_token_hash, client_id, subject, resource, scope,
                    provider_refresh_token, created_at, expires_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(refresh_token_hash) DO UPDATE SET
                    client_id = excluded.client_id,
                    subject = excluded.subject,
                    resource = excluded.resource,
                    scope = excluded.scope,
                    provider_refresh_token = excluded.provider_refresh_token,
                    created_at = excluded.created_at,
                    expires_at = excluded.expires_at",
                params![
                    hash,
                    token.client_id,
                    token.subject,
                    token.resource,
                    token.scope,
                    encrypted_provider_rt,
                    token.created_at,
                    token.expires_at,
                ],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// Atomically replace an existing, unexpired refresh token with a new one.
    /// A missing or expired old token rolls back without inserting the new row.
    pub async fn rotate_refresh_token(
        &self,
        old_token: &str,
        new_token: RefreshTokenRow,
    ) -> Result<Option<RefreshTokenRow>, AuthError> {
        let old_hash = hash_token(old_token);
        let new_hash = hash_token(&new_token.refresh_token);
        let now = now_unix();
        let encrypted_provider_rt = new_token
            .provider_refresh_token
            .as_deref()
            .map(|raw| maybe_encrypt(self.enc_key.as_deref(), raw))
            .transpose()?;
        self.with_conn(move |conn| {
            conn.execute_batch("BEGIN").map_err(sqlite_error)?;

            let delete_result = conn
                .execute(
                    "DELETE FROM refresh_tokens
                     WHERE refresh_token_hash = ?1
                       AND expires_at > ?2",
                    params![old_hash, now],
                )
                .map_err(sqlite_error);

            let deleted = match delete_result {
                Ok(count) => count,
                Err(error) => {
                    drop(conn.execute_batch("ROLLBACK"));
                    return Err(error);
                }
            };

            if deleted == 0 {
                drop(conn.execute_batch("ROLLBACK"));
                return Ok(None);
            }

            let insert_result = conn
                .execute(
                    "INSERT INTO refresh_tokens (
                        refresh_token_hash, client_id, subject, resource, scope,
                        provider_refresh_token, created_at, expires_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        new_hash,
                        new_token.client_id,
                        new_token.subject,
                        new_token.resource,
                        new_token.scope,
                        encrypted_provider_rt,
                        new_token.created_at,
                        new_token.expires_at,
                    ],
                )
                .map_err(sqlite_error);

            match insert_result {
                Ok(_) => {
                    conn.execute_batch("COMMIT").map_err(sqlite_error)?;
                    Ok(Some(new_token))
                }
                Err(error) => {
                    drop(conn.execute_batch("ROLLBACK"));
                    Err(error)
                }
            }
        })
        .await
    }

    pub async fn find_refresh_token(
        &self,
        refresh_token: &str,
    ) -> Result<Option<RefreshTokenRow>, AuthError> {
        let hash = hash_token(refresh_token);
        let plaintext = refresh_token.to_string();
        let now = now_unix();
        let enc_key = self.enc_key.clone();
        self.with_conn(move |conn| {
            let row = conn
                .query_row(
                    "SELECT client_id, subject, scope,
                            provider_refresh_token, created_at, expires_at, resource
                     FROM refresh_tokens
                     WHERE refresh_token_hash = ?1
                       AND expires_at > ?2",
                    params![hash, now],
                    |row| {
                        Ok(RefreshTokenRow {
                            refresh_token: plaintext.clone(),
                            client_id: row.get(0)?,
                            subject: row.get(1)?,
                            scope: row.get(2)?,
                            provider_refresh_token: row.get(3)?,
                            created_at: row.get(4)?,
                            expires_at: row.get(5)?,
                            resource: row.get(6).unwrap_or_default(),
                        })
                    },
                )
                .optional()
                .map_err(sqlite_error)?;

            match row {
                Some(mut row) => {
                    if let Some(raw) = row.provider_refresh_token.as_deref() {
                        row.provider_refresh_token = Some(maybe_decrypt(enc_key.as_deref(), raw)?);
                    }
                    Ok(Some(row))
                }
                None => Ok(None),
            }
        })
        .await
    }

    /// Whether this local OAuth client holds an unexpired Lab refresh token.
    ///
    /// The check is scoped to `client_id`: consent established for one DCR
    /// client must not suppress the forced-consent flow for another client.
    pub async fn has_refresh_token_for_client(&self, client_id: &str) -> Result<bool, AuthError> {
        let now = now_unix();
        let client_id = client_id.to_string();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM refresh_tokens
                    WHERE expires_at > ?1 AND client_id = ?2
                 )",
                params![now, client_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|exists| exists != 0)
            .map_err(sqlite_error)
        })
        .await
    }
}
