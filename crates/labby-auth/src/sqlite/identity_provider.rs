use rusqlite::{OptionalExtension, params};

use super::{SqliteStore, sqlite_error};
use crate::error::AuthError;
use crate::types::{InboundProviderState, ProviderSwitchRevocation};

impl SqliteStore {
    pub async fn upsert_verified_inbound_identity(
        &self,
        issuer: &str,
        subject: &str,
        email: &str,
        verified_at: i64,
    ) -> Result<(), AuthError> {
        let issuer = issuer.to_string();
        let subject = subject.to_string();
        let email = email.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO inbound_verified_identities
                   (identity_issuer, subject, email, provider_generation, verified_at)
                 SELECT ?1, ?2, ?3, generation, ?4 FROM inbound_identity_provider WHERE singleton = 1
                 ON CONFLICT(identity_issuer, subject, provider_generation) DO UPDATE SET
                   email = excluded.email, verified_at = excluded.verified_at",
                params![issuer, subject, email, verified_at],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    pub async fn upsert_bound_verified_inbound_identity(
        &self,
        subject: &str,
        email: &str,
        verified_at: i64,
        binding: crate::types::ProviderBinding,
    ) -> Result<(), AuthError> {
        let subject = subject.to_string();
        let email = email.to_string();
        self.with_conn(move |conn| {
            let count = conn
                .execute(
                    "INSERT INTO inbound_verified_identities
                   (identity_issuer, subject, email, provider_generation, verified_at)
                 SELECT ?1, ?2, ?3, ?4, ?5
                  WHERE EXISTS (SELECT 1 FROM inbound_identity_provider
                    WHERE singleton = 1 AND issuer = ?1 AND generation = ?4)
                 ON CONFLICT(identity_issuer, subject, provider_generation) DO UPDATE SET
                   email = excluded.email, verified_at = excluded.verified_at",
                    params![
                        binding.identity_issuer,
                        subject,
                        email,
                        binding.provider_generation,
                        verified_at
                    ],
                )
                .map_err(sqlite_error)?;
            if count == 1 {
                Ok(())
            } else {
                Err(AuthError::InvalidGrant(
                    "inbound provider changed while identity verification was in progress".into(),
                ))
            }
        })
        .await
    }

    pub async fn current_verified_inbound_email(
        &self,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<String>, AuthError> {
        let issuer = issuer.to_string();
        let subject = subject.to_string();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT email FROM inbound_verified_identities
                  WHERE identity_issuer = ?1 AND subject = ?2
                    AND provider_generation = (SELECT generation FROM inbound_identity_provider WHERE singleton = 1)",
                params![issuer, subject],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_error)
        })
        .await
    }

    pub async fn inbound_provider_state(&self) -> Result<InboundProviderState, AuthError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT provider, issuer, config_fingerprint, generation, updated_at
                 FROM inbound_identity_provider WHERE singleton = 1",
                [],
                |row| {
                    Ok(InboundProviderState {
                        provider: row.get(0)?,
                        issuer: row.get(1)?,
                        config_fingerprint: row.get(2)?,
                        generation: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                },
            )
            .map_err(sqlite_error)
        })
        .await
    }

    /// Activate one provider configuration and atomically revoke every grant
    /// minted by the previous generation. Re-applying the same fingerprint is
    /// idempotent and does not disturb active sessions.
    #[cfg(test)]
    pub async fn activate_inbound_provider(
        &self,
        provider: &str,
        issuer: &str,
        config_fingerprint: &str,
        updated_at: i64,
    ) -> Result<ProviderSwitchRevocation, AuthError> {
        self.activate_inbound_provider_checked(
            provider,
            issuer,
            config_fingerprint,
            None,
            updated_at,
        )
        .await
    }

    pub async fn activate_inbound_provider_checked(
        &self,
        provider: &str,
        issuer: &str,
        config_fingerprint: &str,
        provider_client_id: Option<&str>,
        updated_at: i64,
    ) -> Result<ProviderSwitchRevocation, AuthError> {
        if provider.is_empty() || issuer.is_empty() || config_fingerprint.is_empty() {
            return Err(AuthError::Validation(
                "inbound provider, issuer, and configuration fingerprint must be non-empty".into(),
            ));
        }
        let provider = provider.to_string();
        let issuer = issuer.to_string();
        let config_fingerprint = config_fingerprint.to_string();
        let provider_client_id = provider_client_id.map(str::to_string);
        self.with_conn(move |conn| {
            let transaction = conn.transaction().map_err(sqlite_error)?;
            let current = transaction
                .query_row(
                    "SELECT provider, issuer, config_fingerprint, generation
                     FROM inbound_identity_provider WHERE singleton = 1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?;
            if let Some((old_provider, old_issuer, old_fingerprint, generation)) = current.as_ref()
                && old_provider == &provider
                && old_issuer == &issuer
                && old_fingerprint == &config_fingerprint
            {
                transaction.commit().map_err(sqlite_error)?;
                return Ok(ProviderSwitchRevocation {
                    generation: *generation,
                    ..ProviderSwitchRevocation::default()
                });
            }

            // The v13 backfill cannot know the operator's Google client
            // fingerprint. Adopt it in place on first startup so an upgrade
            // preserves active Google grants instead of treating configuration
            // discovery as a provider switch.
            if let Some((old_provider, old_issuer, old_fingerprint, generation)) = current.as_ref()
                && old_provider == "google"
                && old_issuer == "https://accounts.google.com"
                && old_fingerprint == "legacy-google"
                && provider == "google"
                && issuer == "https://accounts.google.com"
                && legacy_google_client_matches(&transaction, provider_client_id.as_deref())?
            {
                transaction
                    .execute(
                        "UPDATE inbound_identity_provider
                            SET config_fingerprint = ?1, updated_at = ?2
                          WHERE singleton = 1 AND generation = ?3",
                        params![config_fingerprint, updated_at, generation],
                    )
                    .map_err(sqlite_error)?;
                transaction.commit().map_err(sqlite_error)?;
                return Ok(ProviderSwitchRevocation {
                    generation: *generation,
                    ..ProviderSwitchRevocation::default()
                });
            }

            let generation = current.map_or(1, |(_, _, _, value)| value + 1);
            transaction
                .execute(
                    "INSERT INTO inbound_identity_provider
                       (singleton, provider, issuer, config_fingerprint, generation, updated_at)
                     VALUES (1, ?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(singleton) DO UPDATE SET
                       provider = excluded.provider,
                       issuer = excluded.issuer,
                       config_fingerprint = excluded.config_fingerprint,
                       generation = excluded.generation,
                       updated_at = excluded.updated_at",
                    params![provider, issuer, config_fingerprint, generation, updated_at],
                )
                .map_err(sqlite_error)?;

            let revoked_authorization_requests =
                delete_old(&transaction, "authorization_requests", generation)?;
            let revoked_authorization_codes =
                delete_old(&transaction, "authorization_codes", generation)?;
            let revoked_refresh_tokens = delete_old(&transaction, "refresh_tokens", generation)?;
            let revoked_browser_sessions =
                delete_old(&transaction, "browser_sessions", generation)?;
            delete_old(&transaction, "inbound_verified_identities", generation)?;
            let revoked_browser_login_states =
                delete_old(&transaction, "browser_login_states", generation)?;
            let revoked_native_authorization_results =
                delete_old(&transaction, "native_authorization_results", generation)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(ProviderSwitchRevocation {
                generation,
                revoked_authorization_requests,
                revoked_authorization_codes,
                revoked_refresh_tokens,
                revoked_browser_sessions,
                revoked_browser_login_states,
                revoked_native_authorization_results,
            })
        })
        .await
    }

    /// Atomically revoke persisted grants for one issuer-qualified identity.
    pub async fn revoke_inbound_identity(
        &self,
        issuer: &str,
        subject: &str,
    ) -> Result<(u64, u64, u64), AuthError> {
        let issuer = issuer.to_string();
        let subject = subject.to_string();
        self.with_conn(move |conn| {
            let transaction = conn.transaction().map_err(sqlite_error)?;
            let codes = transaction
                .execute(
                    "DELETE FROM authorization_codes WHERE identity_issuer = ?1 AND subject = ?2",
                    params![issuer, subject],
                )
                .map_err(sqlite_error)? as u64;
            let tokens = transaction
                .execute(
                    "DELETE FROM refresh_tokens WHERE identity_issuer = ?1 AND subject = ?2",
                    params![issuer, subject],
                )
                .map_err(sqlite_error)? as u64;
            let sessions = transaction
                .execute(
                    "DELETE FROM browser_sessions WHERE identity_issuer = ?1 AND subject = ?2",
                    params![issuer, subject],
                )
                .map_err(sqlite_error)? as u64;
            transaction.execute(
                "DELETE FROM inbound_verified_identities WHERE identity_issuer = ?1 AND subject = ?2",
                params![issuer, subject],
            ).map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok((codes, tokens, sessions))
        })
        .await
    }
}

fn legacy_google_client_matches(
    transaction: &rusqlite::Transaction<'_>,
    configured_client_id: Option<&str>,
) -> Result<bool, AuthError> {
    let Some(configured_client_id) = configured_client_id else {
        return Ok(true);
    };
    let mismatch: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM google_provider_credentials
              WHERE client_id <> '' AND client_id <> ?1",
            params![configured_client_id],
            |row| row.get(0),
        )
        .map_err(sqlite_error)?;
    Ok(mismatch == 0)
}

fn delete_old(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    generation: i64,
) -> Result<u64, AuthError> {
    let sql = format!("DELETE FROM {table} WHERE provider_generation <> ?1");
    transaction
        .execute(&sql, params![generation])
        .map(|count| count as u64)
        .map_err(sqlite_error)
}
