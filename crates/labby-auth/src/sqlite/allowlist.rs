use rusqlite::params;

use super::rows::row_to_allowed_user;
use super::{SqliteStore, sqlite_error};
use crate::error::AuthError;
use crate::types::AllowedUserRow;
use crate::util::{fingerprint, now_unix};

impl SqliteStore {
    /// Add an email address to the allowlist.
    ///
    /// `email` is normalised to lowercase before storage. Returns
    /// `AuthError::Validation` if the email is already present.
    pub async fn add_allowed_user(
        &self,
        email: &str,
        added_by: &str,
        created_at: i64,
    ) -> Result<(), AuthError> {
        let email = email.to_lowercase();
        let fp = fingerprint(&email);
        let added_by = added_by.to_string();
        self.with_conn(move |conn| {
            let changed = conn
                .execute(
                    "INSERT INTO allowed_users (email, added_by, created_at)
                     VALUES (?1, ?2, ?3)",
                    params![email, added_by, created_at],
                )
                .map_err(|error| match error {
                    rusqlite::Error::SqliteFailure(ref e, _)
                        if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                    {
                        AuthError::Validation(format!(
                            "email fingerprint {fp} is already in the allowlist"
                        ))
                    }
                    other => sqlite_error(other),
                })?;
            debug_assert_eq!(changed, 1);
            Ok(())
        })
        .await
    }

    /// Remove only an email address from the allowlist.
    ///
    /// This narrow operation is for deleting a redundant row matching the
    /// configured bootstrap admin, whose authorization remains in config.
    /// Other callers must use [`Self::remove_allowed_user`].
    pub async fn remove_bootstrap_admin_allowlist_entry(
        &self,
        email: &str,
    ) -> Result<(), AuthError> {
        let email = email.to_lowercase();
        self.with_conn(move |conn| {
            conn.execute("DELETE FROM allowed_users WHERE email = ?1", params![email])
                .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// Remove an allowlist entry and revoke renewable credentials for every
    /// subject currently associated with that email.
    ///
    /// The allowlist deletion, browser sessions, local OAuth grants, central
    /// provider credentials, and provider-revocation epochs share one
    /// transaction so a successful return cannot leave renewable authority
    /// behind. Already-issued signed access tokens remain valid only until
    /// their configured expiry because they are deliberately stateless.
    pub async fn remove_allowed_user(
        &self,
        email: &str,
    ) -> Result<crate::types::AllowedUserRevocation, AuthError> {
        let email = email.to_lowercase();
        self.with_conn(move |conn| {
            let transaction = conn.transaction().map_err(sqlite_error)?;
            let subjects = {
                let mut statement = transaction
                    .prepare(
                        "SELECT subject FROM browser_sessions WHERE email = ?1 COLLATE NOCASE
                         UNION SELECT subject FROM google_provider_credentials
                         WHERE email = ?1 COLLATE NOCASE",
                    )
                    .map_err(sqlite_error)?;
                statement
                    .query_map(params![email], |row| row.get::<_, String>(0))
                    .map_err(sqlite_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(sqlite_error)?
            };
            let encoded_subjects = serde_json::to_string(&subjects).map_err(|error| {
                AuthError::Storage(format!("failed to encode revocation subjects: {error}"))
            })?;
            let revoked_refresh_tokens = transaction
                .execute(
                    "DELETE FROM refresh_tokens WHERE subject IN
                     (SELECT value FROM json_each(?1))",
                    params![encoded_subjects],
                )
                .map_err(sqlite_error)?;
            let revoked_authorization_codes = transaction
                .execute(
                    "DELETE FROM authorization_codes WHERE subject IN
                     (SELECT value FROM json_each(?1))",
                    params![encoded_subjects],
                )
                .map_err(sqlite_error)?;
            let revoked_provider_credentials = transaction
                .execute(
                    "DELETE FROM google_provider_credentials WHERE subject IN
                     (SELECT value FROM json_each(?1))",
                    params![encoded_subjects],
                )
                .map_err(sqlite_error)?;
            for subject in &subjects {
                transaction
                    .execute(
                        "INSERT INTO google_provider_revocations (subject, epoch, updated_at)
                     VALUES (?1, 1, ?2) ON CONFLICT(subject) DO UPDATE SET
                     epoch = google_provider_revocations.epoch + 1,
                     updated_at = excluded.updated_at",
                        params![subject, now_unix()],
                    )
                    .map_err(sqlite_error)?;
            }
            if !subjects.is_empty() {
                transaction
                    .execute(
                        "INSERT INTO google_provider_revocations (subject, epoch, updated_at)
                     VALUES ('*', 1, ?1) ON CONFLICT(subject) DO UPDATE SET
                     epoch = google_provider_revocations.epoch + 1,
                     updated_at = excluded.updated_at",
                        params![now_unix()],
                    )
                    .map_err(sqlite_error)?;
            }
            let revoked_sessions = transaction
                .execute(
                    "DELETE FROM browser_sessions
                     WHERE email = ?1 COLLATE NOCASE
                        OR subject IN (SELECT value FROM json_each(?2))",
                    params![email, encoded_subjects],
                )
                .map_err(sqlite_error)?;
            transaction
                .execute("DELETE FROM allowed_users WHERE email = ?1", params![email])
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(crate::types::AllowedUserRevocation {
                subjects,
                revoked_sessions: revoked_sessions as u64,
                revoked_provider_credentials: revoked_provider_credentials as u64,
                revoked_refresh_tokens: revoked_refresh_tokens as u64,
                revoked_authorization_codes: revoked_authorization_codes as u64,
            })
        })
        .await
    }

    /// Return all allowlist rows ordered by `created_at ASC`.
    pub async fn list_allowed_users(&self) -> Result<Vec<AllowedUserRow>, AuthError> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT email, added_by, created_at
                     FROM allowed_users
                     ORDER BY created_at ASC",
                )
                .map_err(sqlite_error)?;
            let rows = stmt
                .query_map([], row_to_allowed_user)
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;
            Ok(rows)
        })
        .await
    }

    /// Indexed case-insensitive membership check for hot authorization paths.
    pub async fn is_allowed_user_email(&self, email: &str) -> Result<bool, AuthError> {
        let email = email.to_string();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM allowed_users WHERE email = ?1 COLLATE NOCASE)",
                params![email],
                |row| row.get(0),
            )
            .map_err(sqlite_error)
        })
        .await
    }
}
