//! Central Google provider credential persistence.

use rusqlite::{OptionalExtension, params};

use super::{SqliteStore, sqlite_error};
use crate::at_rest::{decrypt_provider_token_with_context, encrypt_provider_token_with_context};
use crate::error::AuthError;
use crate::google::merge_google_scopes;
use crate::types::{GoogleProviderCredentialRow, GoogleProviderCredentialUpdate};

#[cfg(test)]
type EncryptionSnapshotHook = (
    std::sync::mpsc::SyncSender<()>,
    std::sync::mpsc::Receiver<()>,
);
#[cfg(test)]
pub(super) static LEGACY_ENCRYPTION_SNAPSHOT_HOOK: std::sync::LazyLock<
    std::sync::Mutex<Option<EncryptionSnapshotHook>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

fn normalize_scopes(scopes: &[String]) -> Vec<String> {
    merge_google_scopes(&[], scopes)
}

fn decode_row(
    enc_key: Option<&crate::at_rest::TokenEncryptionKey>,
    mut row: GoogleProviderCredentialRow,
) -> Result<GoogleProviderCredentialRow, AuthError> {
    if enc_key.is_none() {
        return Err(AuthError::Config(
            "TOKEN_ENCRYPTION_KEY is required to read Google provider credentials".to_string(),
        ));
    }
    let key = enc_key.expect("encryption key checked above");
    row.refresh_token = decrypt_provider_token_with_context(
        key,
        &row.refresh_token,
        &google_token_context(&row.subject, &row.client_id, "refresh"),
    )?;
    if let Some(access_token) = row.access_token.as_deref() {
        row.access_token = Some(decrypt_provider_token_with_context(
            key,
            access_token,
            &google_token_context(&row.subject, &row.client_id, "access"),
        )?);
    }
    Ok(row)
}

fn google_token_context(subject: &str, client_id: &str, token_kind: &str) -> Vec<u8> {
    format!("issuer=https://accounts.google.com\0subject={subject}\0client_id={client_id}\0kind={token_kind}")
        .into_bytes()
}

fn encrypt_google_token(
    key: Option<&crate::at_rest::TokenEncryptionKey>,
    value: &str,
    subject: &str,
    client_id: &str,
    token_kind: &str,
) -> Result<String, AuthError> {
    let key = key.ok_or_else(|| {
        AuthError::Config(
            "TOKEN_ENCRYPTION_KEY is required to persist Google provider credentials".to_string(),
        )
    })?;
    encrypt_provider_token_with_context(
        key,
        value,
        &google_token_context(subject, client_id, token_kind),
    )
}

impl SqliteStore {
    pub async fn google_provider_revocation_epoch(&self, subject: &str) -> Result<i64, AuthError> {
        let subject = subject.to_string();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT epoch FROM google_provider_revocations WHERE subject = ?1",
                params![subject],
                |row| row.get(0),
            )
            .optional()
            .map(|epoch| epoch.unwrap_or(0))
            .map_err(sqlite_error)
        })
        .await
    }

    pub async fn google_provider_fence_epoch(&self) -> Result<i64, AuthError> {
        self.google_provider_revocation_epoch("*").await
    }
    /// Encrypt legacy plaintext Google provider tokens when an at-rest key is available.
    ///
    /// Schema v7 deployments could persist the central refresh credential before
    /// the shared broker wired `TokenEncryptionKey` into every SQLite opener. This
    /// one-time, idempotent pass upgrades both provider token columns without
    /// changing generation or lifecycle timestamps.
    pub(super) async fn encrypt_legacy_google_provider_credentials(
        &self,
    ) -> Result<u64, AuthError> {
        let Some(enc_key) = self.enc_key.clone() else {
            return Ok(0);
        };
        self.with_conn(move |conn| {
            let transaction = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let rows = {
                let mut statement = transaction
                    .prepare(
                        "SELECT subject, client_id, access_token, refresh_token
                         FROM google_provider_credentials",
                    )
                    .map_err(sqlite_error)?;
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    })
                    .map_err(sqlite_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(sqlite_error)?
            };
            #[cfg(test)]
            if let Some((reached, resume)) = LEGACY_ENCRYPTION_SNAPSHOT_HOOK
                .lock()
                .expect("legacy encryption hook lock")
                .take()
            {
                reached.send(()).expect("signal encryption snapshot");
                resume.recv().expect("resume encryption snapshot");
            }

            let mut updated = 0_u64;
            for (subject, client_id, access_token, refresh_token) in rows {
                let next_access = access_token
                    .as_deref()
                    .filter(|token| !token.starts_with("enc2:"))
                    .map(|token| {
                        let plaintext = decrypt_provider_token_with_context(
                            enc_key.as_ref(),
                            token,
                            &google_token_context(&subject, &client_id, "access"),
                        )?;
                        encrypt_google_token(
                            Some(enc_key.as_ref()),
                            &plaintext,
                            &subject,
                            &client_id,
                            "access",
                        )
                    })
                    .transpose()?;
                let next_refresh = (!refresh_token.starts_with("enc2:"))
                    .then(|| {
                        let plaintext = decrypt_provider_token_with_context(
                            enc_key.as_ref(),
                            &refresh_token,
                            &google_token_context(&subject, &client_id, "refresh"),
                        )?;
                        encrypt_google_token(
                            Some(enc_key.as_ref()),
                            &plaintext,
                            &subject,
                            &client_id,
                            "refresh",
                        )
                    })
                    .transpose()?;
                if next_access.is_none() && next_refresh.is_none() {
                    continue;
                }
                transaction
                    .execute(
                        "UPDATE google_provider_credentials
                         SET access_token = COALESCE(?2, access_token),
                             refresh_token = COALESCE(?3, refresh_token)
                         WHERE subject = ?1",
                        params![subject, next_access, next_refresh],
                    )
                    .map_err(sqlite_error)?;
                updated += 1;
            }
            transaction.commit().map_err(sqlite_error)?;
            Ok(updated)
        })
        .await
    }

    /// Resolve a central Google credential by subject or verified email.
    ///
    /// When `selector` is absent, resolution succeeds only when exactly one
    /// credential exists. This prevents a gateway-wide upstream from silently
    /// choosing the wrong Google account.
    pub async fn find_google_provider_credential_by_selector(
        &self,
        selector: Option<&str>,
    ) -> Result<Option<GoogleProviderCredentialRow>, AuthError> {
        let selector = selector
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let enc_key = self.enc_key.clone();
        self.with_conn(move |conn| {
            let rows = if let Some(selector) = selector {
                let exact_subject = conn
                    .query_row(
                        "SELECT subject, email, client_id, granted_scopes_json,
                                access_token, refresh_token, token_received_at,
                                access_token_expires_at, issuer, last_refresh_at,
                                last_scope_upgrade_at, generation, created_at, updated_at
                         FROM google_provider_credentials
                         WHERE subject = ?1",
                        params![selector],
                        row_from_sql,
                    )
                    .optional()
                    .map_err(sqlite_error)?;
                if let Some(row) = exact_subject {
                    vec![row]
                } else {
                    let mut statement = conn
                        .prepare(
                            "SELECT subject, email, client_id, granted_scopes_json,
                                    access_token, refresh_token, token_received_at,
                                    access_token_expires_at, issuer, last_refresh_at,
                                    last_scope_upgrade_at, generation, created_at, updated_at
                             FROM google_provider_credentials
                             WHERE email = ?1 COLLATE NOCASE
                             ORDER BY created_at ASC
                             LIMIT 2",
                        )
                        .map_err(sqlite_error)?;
                    statement
                        .query_map(params![selector], row_from_sql)
                        .map_err(sqlite_error)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(sqlite_error)?
                }
            } else {
                let mut statement = conn
                    .prepare(
                        "SELECT subject, email, client_id, granted_scopes_json,
                                access_token, refresh_token, token_received_at,
                                access_token_expires_at, issuer, last_refresh_at,
                                last_scope_upgrade_at, generation, created_at, updated_at
                         FROM google_provider_credentials
                         ORDER BY created_at ASC
                         LIMIT 2",
                    )
                    .map_err(sqlite_error)?;
                statement
                    .query_map([], row_from_sql)
                    .map_err(sqlite_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(sqlite_error)?
            };

            match rows.as_slice() {
                [] => Ok(None),
                [row] => decode_row(enc_key.as_deref(), row.clone()).map(Some),
                _ => Err(AuthError::Config(
                    "multiple Google provider credentials exist; configure an account selector"
                        .to_string(),
                )),
            }
        })
        .await
    }

    pub async fn find_google_provider_credential_by_email(
        &self,
        email: &str,
    ) -> Result<Option<GoogleProviderCredentialRow>, AuthError> {
        self.find_google_provider_credential_by_selector(Some(email))
            .await
    }

    pub async fn count_google_provider_credentials(&self) -> Result<u64, AuthError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM google_provider_credentials",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count.max(0) as u64)
            .map_err(sqlite_error)
        })
        .await
    }

    /// Persist a verified Google token bundle in the central broker row.
    ///
    /// Both access and refresh tokens are encrypted at rest. Every successful
    /// write advances `generation`, which makes terminal invalidation safe when
    /// refreshes and scope upgrades race.
    pub async fn upsert_google_provider_token_bundle(
        &self,
        update: GoogleProviderCredentialUpdate,
    ) -> Result<(), AuthError> {
        let GoogleProviderCredentialUpdate {
            subject,
            email,
            client_id,
            granted_scopes,
            access_token,
            refresh_token,
            token_received_at,
            access_token_expires_at,
            issuer,
            refreshed,
            scope_upgraded,
        } = update;
        let email = email
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        let granted_scopes = normalize_scopes(&granted_scopes);
        let granted_scopes_json = serde_json::to_string(&granted_scopes)
            .map_err(|error| AuthError::Storage(format!("serialize Google scopes: {error}")))?;
        let encrypted_access_token = encrypt_google_token(
            self.enc_key.as_deref(),
            &access_token,
            &subject,
            &client_id,
            "access",
        )?;
        let encrypted_refresh_token = encrypt_google_token(
            self.enc_key.as_deref(),
            &refresh_token,
            &subject,
            &client_id,
            "refresh",
        )?;
        let now = crate::util::now_unix();
        let refreshed = i64::from(refreshed);
        let scope_upgraded = i64::from(scope_upgraded);

        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO google_provider_credentials (
                    subject, email, client_id, granted_scopes_json, access_token,
                    refresh_token, token_received_at, access_token_expires_at, issuer,
                    last_refresh_at, last_scope_upgrade_at, generation, created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                    CASE WHEN ?10 != 0 THEN ?12 ELSE NULL END,
                    CASE WHEN ?11 != 0 THEN ?12 ELSE NULL END,
                    1, ?12, ?12
                 )
                 ON CONFLICT(subject) DO UPDATE SET
                    email = COALESCE(excluded.email, google_provider_credentials.email),
                    client_id = excluded.client_id,
                    granted_scopes_json = excluded.granted_scopes_json,
                    access_token = excluded.access_token,
                    refresh_token = excluded.refresh_token,
                    token_received_at = excluded.token_received_at,
                    access_token_expires_at = excluded.access_token_expires_at,
                    issuer = COALESCE(excluded.issuer, google_provider_credentials.issuer),
                    last_refresh_at = CASE WHEN ?10 != 0 THEN ?12
                        ELSE google_provider_credentials.last_refresh_at END,
                    last_scope_upgrade_at = CASE WHEN ?11 != 0 THEN ?12
                        ELSE google_provider_credentials.last_scope_upgrade_at END,
                    generation = google_provider_credentials.generation + 1,
                    updated_at = ?12",
                params![
                    subject,
                    email,
                    client_id,
                    granted_scopes_json,
                    encrypted_access_token,
                    encrypted_refresh_token,
                    token_received_at,
                    access_token_expires_at,
                    issuer,
                    refreshed,
                    scope_upgraded,
                    now,
                ],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// Replace an existing provider token bundle only when the caller observed
    /// the current generation.
    ///
    /// This is the persistence boundary for read-modify-write provider flows.
    /// A `false` result means another callback or refresh installed a newer
    /// bundle and the caller must not publish its stale exchange.
    pub async fn replace_google_provider_token_bundle_if_generation(
        &self,
        update: GoogleProviderCredentialUpdate,
        expected_generation: i64,
    ) -> Result<bool, AuthError> {
        let GoogleProviderCredentialUpdate {
            subject,
            email,
            client_id,
            granted_scopes,
            access_token,
            refresh_token,
            token_received_at,
            access_token_expires_at,
            issuer,
            refreshed,
            scope_upgraded,
        } = update;
        let email = email
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        let granted_scopes_json = serde_json::to_string(&normalize_scopes(&granted_scopes))
            .map_err(|error| AuthError::Storage(format!("serialize Google scopes: {error}")))?;
        let encrypted_access_token = encrypt_google_token(
            self.enc_key.as_deref(),
            &access_token,
            &subject,
            &client_id,
            "access",
        )?;
        let encrypted_refresh_token = encrypt_google_token(
            self.enc_key.as_deref(),
            &refresh_token,
            &subject,
            &client_id,
            "refresh",
        )?;
        let now = crate::util::now_unix();
        let refreshed = i64::from(refreshed);
        let scope_upgraded = i64::from(scope_upgraded);

        self.with_conn(move |conn| {
            let updated = conn
                .execute(
                    "UPDATE google_provider_credentials SET
                        email = COALESCE(?2, email),
                        client_id = ?3,
                        granted_scopes_json = ?4,
                        access_token = ?5,
                        refresh_token = ?6,
                        token_received_at = ?7,
                        access_token_expires_at = ?8,
                        issuer = COALESCE(?9, issuer),
                        last_refresh_at = CASE WHEN ?10 != 0 THEN ?12 ELSE last_refresh_at END,
                        last_scope_upgrade_at = CASE WHEN ?11 != 0 THEN ?12
                            ELSE last_scope_upgrade_at END,
                        generation = generation + 1,
                        updated_at = ?12
                     WHERE subject = ?1 AND generation = ?13",
                    params![
                        subject,
                        email,
                        client_id,
                        granted_scopes_json,
                        encrypted_access_token,
                        encrypted_refresh_token,
                        token_received_at,
                        access_token_expires_at,
                        issuer,
                        refreshed,
                        scope_upgraded,
                        now,
                        expected_generation,
                    ],
                )
                .map_err(sqlite_error)?;
            Ok(updated == 1)
        })
        .await
    }

    /// Install the first bundle for a subject without overwriting a bundle
    /// concurrently created by another process.
    pub async fn insert_google_provider_token_bundle_if_absent(
        &self,
        update: GoogleProviderCredentialUpdate,
        expected_revocation_epoch: i64,
    ) -> Result<bool, AuthError> {
        let GoogleProviderCredentialUpdate {
            subject,
            email,
            client_id,
            granted_scopes,
            access_token,
            refresh_token,
            token_received_at,
            access_token_expires_at,
            issuer,
            refreshed,
            scope_upgraded,
        } = update;
        let email = email
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        let granted_scopes_json = serde_json::to_string(&normalize_scopes(&granted_scopes))
            .map_err(|error| AuthError::Storage(format!("serialize Google scopes: {error}")))?;
        let encrypted_access_token = encrypt_google_token(
            self.enc_key.as_deref(),
            &access_token,
            &subject,
            &client_id,
            "access",
        )?;
        let encrypted_refresh_token = encrypt_google_token(
            self.enc_key.as_deref(),
            &refresh_token,
            &subject,
            &client_id,
            "refresh",
        )?;
        let now = crate::util::now_unix();
        let refreshed = i64::from(refreshed);
        let scope_upgraded = i64::from(scope_upgraded);
        self.with_conn(move |conn| {
            let inserted = conn
                .execute(
                    "INSERT OR IGNORE INTO google_provider_credentials (
                    subject, email, client_id, granted_scopes_json, access_token,
                    refresh_token, token_received_at, access_token_expires_at, issuer,
                    last_refresh_at, last_scope_upgrade_at, generation, created_at, updated_at
                 ) SELECT
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                    CASE WHEN ?10 != 0 THEN ?12 ELSE NULL END,
                    CASE WHEN ?11 != 0 THEN ?12 ELSE NULL END,
                    1, ?12, ?12
                 WHERE COALESCE((SELECT epoch FROM google_provider_revocations
                                 WHERE subject = '*'), 0) = ?13",
                    params![
                        subject,
                        email,
                        client_id,
                        granted_scopes_json,
                        encrypted_access_token,
                        encrypted_refresh_token,
                        token_received_at,
                        access_token_expires_at,
                        issuer,
                        refreshed,
                        scope_upgraded,
                        now,
                        expected_revocation_epoch
                    ],
                )
                .map_err(sqlite_error)?;
            Ok(inserted == 1)
        })
        .await
    }

    /// Remove the selected provider credential and all Labby grants that depend on it.
    ///
    /// Compare-and-delete remains generation-safe, but an explicit operator revoke
    /// retries a bounded number of times so a refresh that completed immediately
    /// before the revoke cannot leave a newer generation behind.
    pub async fn revoke_google_provider_credential(
        &self,
        selector: Option<&str>,
    ) -> Result<crate::types::GoogleProviderInvalidation, AuthError> {
        for _ in 0..4 {
            let Some(row) = self
                .find_google_provider_credential_by_selector(selector)
                .await?
            else {
                return Ok(crate::types::GoogleProviderInvalidation::default());
            };
            let invalidation = self
                .invalidate_google_provider_credential(&row.subject, row.generation)
                .await?;
            if invalidation.invalidated {
                return Ok(invalidation);
            }
        }
        Err(AuthError::Storage(
            "Google provider credential changed repeatedly during explicit revoke".to_string(),
        ))
    }
}

fn row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<GoogleProviderCredentialRow> {
    let granted_scopes_json: String = row.get(3)?;
    let granted_scopes = serde_json::from_str(&granted_scopes_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(GoogleProviderCredentialRow {
        subject: row.get(0)?,
        email: row.get(1)?,
        client_id: row.get(2)?,
        granted_scopes,
        access_token: row.get(4)?,
        refresh_token: row.get(5)?,
        token_received_at: row.get(6)?,
        access_token_expires_at: row.get(7)?,
        issuer: row.get(8)?,
        last_refresh_at: row.get(9)?,
        last_scope_upgrade_at: row.get(10)?,
        generation: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}
