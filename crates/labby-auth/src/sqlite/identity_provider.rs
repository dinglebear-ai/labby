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
                 SELECT ?1, ?2, ?3, generation, ?4 FROM inbound_identity_providers WHERE issuer = ?1
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
                  WHERE EXISTS (SELECT 1 FROM inbound_identity_providers
                    WHERE issuer = ?1 AND generation = ?4)
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
                    AND provider_generation = (SELECT generation FROM inbound_identity_providers WHERE issuer = identity_issuer)",
                params![issuer, subject],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_error)
        })
        .await
    }

    pub async fn current_verified_inbound_identity(
        &self,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<(String, i64)>, AuthError> {
        let issuer = issuer.to_string();
        let subject = subject.to_string();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT email, verified_at FROM inbound_verified_identities
                  WHERE identity_issuer = ?1 AND subject = ?2
                    AND provider_generation = (SELECT generation FROM inbound_identity_providers WHERE issuer = identity_issuer)",
                params![issuer, subject],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sqlite_error)
        })
        .await
    }

    /// State of the only configured inbound provider. Fails closed when more
    /// than one provider is recorded; use [`Self::inbound_provider_state_for`].
    pub async fn inbound_provider_state(&self) -> Result<InboundProviderState, AuthError> {
        self.with_conn(|conn| {
            require_sole_provider(conn)?;
            conn.query_row(
                "SELECT provider, issuer, config_fingerprint, generation, updated_at
                 FROM inbound_identity_providers",
                [],
                provider_state_from_row,
            )
            .map_err(sqlite_error)
        })
        .await
    }

    /// State of one inbound provider (`google`, `authelia`), if recorded.
    pub async fn inbound_provider_state_for(
        &self,
        provider: &str,
    ) -> Result<Option<InboundProviderState>, AuthError> {
        let provider = provider.to_string();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT provider, issuer, config_fingerprint, generation, updated_at
                 FROM inbound_identity_providers WHERE provider = ?1",
                params![provider],
                provider_state_from_row,
            )
            .optional()
            .map_err(sqlite_error)
        })
        .await
    }

    /// Test helper with single-provider switch semantics: activate one
    /// provider, then retire every other one (and its grants).
    #[cfg(test)]
    pub async fn activate_inbound_provider(
        &self,
        provider: &str,
        issuer: &str,
        config_fingerprint: &str,
        updated_at: i64,
    ) -> Result<ProviderSwitchRevocation, AuthError> {
        let mut activation = self
            .activate_inbound_provider_inner(
                provider,
                issuer,
                config_fingerprint,
                None,
                updated_at,
                true,
            )
            .await?;
        let retired = self
            .retire_inbound_providers_except_inner(&[provider], true)
            .await?;
        activation.revoked_authorization_requests += retired.revoked_authorization_requests;
        activation.revoked_authorization_codes += retired.revoked_authorization_codes;
        activation.revoked_refresh_tokens += retired.revoked_refresh_tokens;
        activation.revoked_browser_sessions += retired.revoked_browser_sessions;
        activation.revoked_browser_login_states += retired.revoked_browser_login_states;
        activation.revoked_native_authorization_results +=
            retired.revoked_native_authorization_results;
        Ok(activation)
    }

    /// Retire every recorded provider not in `keep`, revoking its grants.
    /// Refuses (changing nothing) when a retiring provider still has live
    /// grants, so a provider switch never silently signs everyone out.
    pub async fn retire_inbound_providers_except_checked(
        &self,
        keep: &[&str],
    ) -> Result<ProviderSwitchRevocation, AuthError> {
        self.retire_inbound_providers_except_inner(keep, false)
            .await
    }

    async fn retire_inbound_providers_except_inner(
        &self,
        keep: &[&str],
        allow_populated: bool,
    ) -> Result<ProviderSwitchRevocation, AuthError> {
        let keep: Vec<String> = keep
            .iter()
            .map(|provider| (*provider).to_string())
            .collect();
        self.with_conn(move |conn| {
            let transaction = conn.transaction().map_err(sqlite_error)?;
            let retiring: Vec<(String, String)> = {
                let mut statement = transaction
                    .prepare("SELECT provider, issuer FROM inbound_identity_providers")
                    .map_err(sqlite_error)?;
                statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .map_err(sqlite_error)?
                    .collect::<rusqlite::Result<Vec<(String, String)>>>()
                    .map_err(sqlite_error)?
                    .into_iter()
                    .filter(|(provider, _)| !keep.contains(provider))
                    .collect()
            };
            let mut revoked = ProviderSwitchRevocation::default();
            for (provider, issuer) in retiring {
                if !allow_populated && provider_state_has_live_grants(&transaction, &issuer)? {
                    return Err(AuthError::Config(format!(
                        "inbound provider `{provider}` is no longer configured but still has live grants in the populated auth database; switch providers through the authenticated admin endpoint"
                    )));
                }
                let revoke = |table: &str| -> Result<u64, AuthError> {
                    transaction
                        .execute(
                            &format!("DELETE FROM {table} WHERE identity_issuer = ?1"),
                            params![issuer],
                        )
                        .map(|count| count as u64)
                        .map_err(sqlite_error)
                };
                revoked.revoked_authorization_requests += revoke("authorization_requests")?;
                revoked.revoked_authorization_codes += revoke("authorization_codes")?;
                revoked.revoked_refresh_tokens += revoke("refresh_tokens")?;
                revoked.revoked_browser_sessions += revoke("browser_sessions")?;
                revoked.revoked_browser_login_states += revoke("browser_login_states")?;
                revoked.revoked_native_authorization_results +=
                    revoke("native_authorization_results")?;
                revoke("inbound_verified_identities")?;
                revoke("desktop_login_states")?;
                revoke("desktop_session_handoffs")?;
                transaction
                    .execute(
                        "DELETE FROM inbound_identity_providers WHERE provider = ?1",
                        params![provider],
                    )
                    .map_err(sqlite_error)?;
            }
            transaction.commit().map_err(sqlite_error)?;
            Ok(revoked)
        })
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
        self.activate_inbound_provider_inner(
            provider,
            issuer,
            config_fingerprint,
            provider_client_id,
            updated_at,
            false,
        )
        .await
    }

    async fn activate_inbound_provider_inner(
        &self,
        provider: &str,
        issuer: &str,
        config_fingerprint: &str,
        provider_client_id: Option<&str>,
        updated_at: i64,
        allow_populated_switch: bool,
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
            // A fresh store carries one `uninitialized` placeholder row; the
            // first provider activated adopts it in place.
            let placeholder = transaction
                .query_row(
                    "SELECT provider FROM inbound_identity_providers
                      WHERE config_fingerprint = 'uninitialized'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(sqlite_error)?;
            if let Some(placeholder) = placeholder.as_ref()
                && placeholder != &provider
            {
                transaction
                    .execute(
                        "UPDATE inbound_identity_providers SET provider = ?1, issuer = ?2
                          WHERE provider = ?3 AND config_fingerprint = 'uninitialized'",
                        params![provider, issuer, placeholder],
                    )
                    .map_err(sqlite_error)?;
            }
            let current = transaction
                .query_row(
                    "SELECT provider, issuer, config_fingerprint, generation
                     FROM inbound_identity_providers WHERE provider = ?1",
                    params![provider],
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

            if let Some((_, _, old_fingerprint, generation)) = current.as_ref()
                && old_fingerprint == "uninitialized"
            {
                transaction
                    .execute(
                        "UPDATE inbound_identity_providers
                            SET issuer = ?2, config_fingerprint = ?3, updated_at = ?4
                          WHERE provider = ?1 AND generation = ?5",
                        params![provider, issuer, config_fingerprint, updated_at, generation],
                    )
                    .map_err(sqlite_error)?;
                transaction.commit().map_err(sqlite_error)?;
                return Ok(ProviderSwitchRevocation {
                    generation: *generation,
                    ..ProviderSwitchRevocation::default()
                });
            }

            if !allow_populated_switch
                && current
                    .as_ref()
                    .is_some_and(|(old_provider, _, fingerprint, _)| {
                        fingerprint != "legacy-google"
                            || old_provider != "google"
                            || provider != "google"
                    })
                && provider_state_has_live_grants(
                    &transaction,
                    current.as_ref().map_or(issuer.as_str(), |(_, old, _, _)| old.as_str()),
                )?
            {
                return Err(AuthError::Config(
                    "configured inbound provider does not match the provider bound to the populated auth database; switch providers through the authenticated admin endpoint"
                        .into(),
                ));
            }

            // The v15 backfill cannot know the operator's Google client
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
                        "UPDATE inbound_identity_providers
                            SET config_fingerprint = ?1, updated_at = ?2
                          WHERE provider = 'google' AND generation = ?3",
                        params![config_fingerprint, updated_at, generation],
                    )
                    .map_err(sqlite_error)?;
                transaction.commit().map_err(sqlite_error)?;
                return Ok(ProviderSwitchRevocation {
                    generation: *generation,
                    ..ProviderSwitchRevocation::default()
                });
            }

            // A new provider starts at generation 1; a changed one bumps its own
            // generation. Either way only rows for this provider's old or new
            // issuer are revoked — other providers' grants are untouched.
            let old_issuer = current
                .as_ref()
                .map_or_else(|| issuer.clone(), |(_, old, _, _)| old.clone());
            // Generations are globally monotonic across providers, so a
            // stale binding can never match a re-added or bumped provider.
            let generation: i64 = transaction
                .query_row(
                    "SELECT COALESCE(MAX(generation), 0) + 1 FROM inbound_identity_providers",
                    [],
                    |row| row.get(0),
                )
                .map_err(sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO inbound_identity_providers
                       (provider, issuer, config_fingerprint, generation, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(provider) DO UPDATE SET
                       issuer = excluded.issuer,
                       config_fingerprint = excluded.config_fingerprint,
                       generation = excluded.generation,
                       updated_at = excluded.updated_at",
                    params![provider, issuer, config_fingerprint, generation, updated_at],
                )
                .map_err(sqlite_error)?;

            let revoke = |table: &str| delete_old(&transaction, table, &old_issuer, &issuer, generation);
            let revoked_authorization_requests = revoke("authorization_requests")?;
            let revoked_authorization_codes = revoke("authorization_codes")?;
            let revoked_refresh_tokens = revoke("refresh_tokens")?;
            let revoked_browser_sessions = revoke("browser_sessions")?;
            revoke("inbound_verified_identities")?;
            let revoked_browser_login_states = revoke("browser_login_states")?;
            let revoked_native_authorization_results = revoke("native_authorization_results")?;
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
        return Ok(false);
    };
    let (matching, mismatch): (i64, i64) = transaction
        .query_row(
            "SELECT
               COALESCE(SUM(CASE WHEN client_id = ?1 THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN client_id <> '' AND client_id <> ?1 THEN 1 ELSE 0 END), 0)
             FROM google_provider_credentials",
            params![configured_client_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sqlite_error)?;
    Ok(matching > 0 && mismatch == 0)
}

/// Legacy single-provider call sites (row inserts that do not yet name an
/// issuer) are only well-defined while exactly one provider is recorded.
pub(super) fn require_sole_provider(conn: &rusqlite::Connection) -> Result<(), AuthError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM inbound_identity_providers",
            [],
            |row| row.get(0),
        )
        .map_err(sqlite_error)?;
    if count == 1 {
        Ok(())
    } else {
        Err(AuthError::Config(format!(
            "expected exactly one inbound provider for an issuer-less auth write, found {count}"
        )))
    }
}

fn provider_state_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InboundProviderState> {
    Ok(InboundProviderState {
        provider: row.get(0)?,
        issuer: row.get(1)?,
        config_fingerprint: row.get(2)?,
        generation: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

/// Whether one provider (by issuer) has any live grant or pending flow.
fn provider_state_has_live_grants(
    transaction: &rusqlite::Transaction<'_>,
    issuer: &str,
) -> Result<bool, AuthError> {
    let count: i64 = transaction
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM authorization_requests WHERE identity_issuer = ?1) +
               (SELECT COUNT(*) FROM authorization_codes WHERE identity_issuer = ?1) +
               (SELECT COUNT(*) FROM refresh_tokens WHERE identity_issuer = ?1) +
               (SELECT COUNT(*) FROM browser_sessions WHERE identity_issuer = ?1) +
               (SELECT COUNT(*) FROM browser_login_states WHERE identity_issuer = ?1) +
               (SELECT COUNT(*) FROM native_authorization_results WHERE identity_issuer = ?1)",
            params![issuer],
            |row| row.get(0),
        )
        .map_err(sqlite_error)?;
    Ok(count > 0)
}

/// Revoke one provider's stale rows: everything under a replaced issuer, and
/// every row of the current issuer minted by an older generation.
fn delete_old(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    old_issuer: &str,
    new_issuer: &str,
    generation: i64,
) -> Result<u64, AuthError> {
    let sql = format!(
        "DELETE FROM {table}
          WHERE (identity_issuer = ?1 AND ?1 <> ?2)
             OR (identity_issuer = ?2 AND provider_generation <> ?3)"
    );
    transaction
        .execute(&sql, params![old_issuer, new_issuer, generation])
        .map(|count| count as u64)
        .map_err(sqlite_error)
}
