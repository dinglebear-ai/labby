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

    /// Whether two handles share the same live connection pool.
    #[cfg(feature = "upstream-oauth-rmcp")]
    pub(crate) fn shares_google_provider_store(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.conns, &other.conns)
    }

    /// Resolve membership from one metadata snapshot, without loading token material.
    /// Exact subjects take precedence over case-insensitive verified email selectors.
    #[cfg(feature = "upstream-oauth-rmcp")]
    pub(crate) async fn google_provider_subject_memberships(
        &self,
        selectors: Vec<Option<String>>,
    ) -> Result<Vec<Result<Option<String>, AuthError>>, AuthError> {
        self.with_conn(move |conn| {
            let mut statement = conn.prepare(
                "SELECT subject, email FROM google_provider_credentials",
            ).map_err(sqlite_error)?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            }).map_err(sqlite_error)?;
            let mut subjects = std::collections::HashSet::new();
            let mut emails = std::collections::HashMap::<String, Option<String>>::new();
            for row in rows {
                let (subject, email) = row.map_err(sqlite_error)?;
                subjects.insert(subject.clone());
                if let Some(email) = email {
                    emails.entry(email.to_ascii_lowercase())
                        .and_modify(|value| *value = None)
                        .or_insert(Some(subject));
                }
            }
            Ok(selectors.into_iter().map(|selector| {
                let selector = selector.as_deref().map(str::trim).filter(|value| !value.is_empty());
                let ambiguous = || AuthError::Config(
                    "multiple Google provider credentials exist; configure an account selector".into(),
                );
                match selector {
                    Some(selector) if subjects.contains(selector) => Ok(Some(selector.to_owned())),
                    Some(selector) => match emails.get(&selector.to_ascii_lowercase()) {
                        Some(Some(subject)) => Ok(Some(subject.clone())),
                        Some(None) => Err(ambiguous()),
                        None => Ok(None),
                    },
                    None if subjects.len() > 1 => Err(ambiguous()),
                    None => Ok(subjects.iter().next().cloned()),
                }
            }).collect())
        }).await
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

#[cfg(all(test, feature = "upstream-oauth-rmcp"))]
mod membership_scaling_tests {
    use super::*;
    use crate::upstream::{cache::OauthClientCache, manager::UpstreamOauthManager};
    use dashmap::DashMap;
    use std::sync::{Arc, atomic::Ordering};

    #[tokio::test]
    async fn membership_snapshot_is_fresh_and_preserves_selector_authority() {
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteStore::open_with_key(
            directory.path().join("auth.db"),
            Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
                "membership",
            )),
        )
        .await
        .unwrap();
        for subject in ["account-a", "account-b", "shared@example.com"] {
            store
                .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
                    subject: subject.into(),
                    email: Some("shared@example.com".into()),
                    client_id: "client".into(),
                    granted_scopes: vec!["openid".into()],
                    access_token: "test-access".into(),
                    refresh_token: "test-refresh".into(),
                    token_received_at: 1,
                    access_token_expires_at: 3601,
                    issuer: Some("https://accounts.google.com".into()),
                    refreshed: false,
                    scope_upgraded: true,
                })
                .await
                .unwrap();
        }
        let mut memberships = store
            .google_provider_subject_memberships(vec![
                None,
                Some(" SHARED@example.com ".into()),
                Some("shared@example.com".into()),
                Some("account-b".into()),
                Some("missing".into()),
            ])
            .await
            .unwrap()
            .into_iter();
        assert!(matches!(
            memberships.next().unwrap(),
            Err(AuthError::Config(_))
        ));
        assert!(matches!(
            memberships.next().unwrap(),
            Err(AuthError::Config(_))
        ));
        assert_eq!(
            memberships.next().unwrap().unwrap().as_deref(),
            Some("shared@example.com")
        );
        assert_eq!(
            memberships.next().unwrap().unwrap().as_deref(),
            Some("account-b")
        );
        assert_eq!(memberships.next().unwrap().unwrap(), None);

        store.with_conn(|conn| {
            conn.execute("UPDATE google_provider_credentials SET email = 'other@example.com' WHERE subject <> 'account-a'", []).map_err(sqlite_error)?;
            Ok(())
        }).await.unwrap();
        let memberships = store
            .google_provider_subject_memberships(vec![Some(" SHARED@example.com ".into())])
            .await
            .unwrap();
        assert_eq!(
            memberships.into_iter().next().unwrap().unwrap().as_deref(),
            Some("account-a"),
            "a new snapshot must observe account replacement"
        );
    }

    #[tokio::test]
    async fn handlers_rebind_replaced_accounts_and_fence_old_membership_on_reload() {
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteStore::open_with_key(
            directory.path().join("auth.db"),
            Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
                "membership",
            )),
        )
        .await
        .unwrap();
        let bundle = |subject: &str| GoogleProviderCredentialUpdate {
            subject: subject.into(),
            email: Some("alias@example.com".into()),
            client_id: "client".into(),
            granted_scopes: vec!["openid".into()],
            access_token: "test-access".into(),
            refresh_token: "test-refresh".into(),
            token_received_at: 1,
            access_token_expires_at: 3601,
            issuer: Some("https://accounts.google.com".into()),
            refreshed: false,
            scope_upgraded: true,
        };
        store
            .upsert_google_provider_token_bundle(bundle("account-a"))
            .await
            .unwrap();
        let config = |name: &str, account: &str| {
            serde_json::from_value::<labby_runtime::gateway_config::UpstreamConfig>(serde_json::json!({
            "name": name, "url": "https://example.com/mcp",
            "oauth": {"mode": "authorization_code_pkce", "registration": {"strategy": "preregistered", "client_id": "client"},
                      "credential": {"source": "google_provider", "account": account}}
        })).unwrap()
        };
        let selected = config("selected", "alias@example.com");
        let sibling = config("sibling", "account-a");
        let managers = Arc::new(DashMap::new());
        for config in [&selected, &sibling] {
            managers.insert(
                config.name.clone(),
                UpstreamOauthManager::new(
                    store.clone(),
                    crate::upstream::encryption::load_key(
                        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                    )
                    .unwrap(),
                    config.clone(),
                    "http://127.0.0.1/callback".into(),
                ),
            );
        }
        let cache = OauthClientCache::new(managers);
        let old = cache
            .refresh_rejection_handler(&selected, "operator")
            .await
            .unwrap();
        store.with_conn(|conn| {
            conn.execute("UPDATE google_provider_credentials SET email = 'old@example.com' WHERE subject = 'account-a'", []).map_err(sqlite_error)?;
            Ok(())
        }).await.unwrap();
        store
            .upsert_google_provider_token_bundle(bundle("account-b"))
            .await
            .unwrap();
        // The reload/authorization coordinator fences the replaced upstream.
        cache.advance_upstream_epoch("selected");
        old(Some("account-a".into()));
        let events = cache.take_pending_invalidations();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].upstream, "sibling",
            "old cleanup must not detach the replacement account"
        );
        let replacement = cache
            .refresh_rejection_handler(&selected, "operator")
            .await
            .unwrap();
        replacement(Some("account-a".into()));
        assert!(cache.take_pending_invalidations().is_empty());
        replacement(Some("account-b".into()));
        let events = cache.take_pending_invalidations();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].upstream, "selected",
            "new membership must exclude the previous subject's upstream"
        );
        assert!(events[0].subject.is_none());
    }

    #[tokio::test]
    async fn rejection_membership_resolves_aliases_in_each_independent_store() {
        // Keep both directories alive until every store and callback is dropped.
        let directory_a = tempfile::tempdir().unwrap();
        let directory_b = tempfile::tempdir().unwrap();
        let mut stores = Vec::new();
        for directory in [&directory_a, &directory_b] {
            stores.push(
                SqliteStore::open_with_key(
                    directory.path().join("auth.db"),
                    Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
                        "membership",
                    )),
                )
                .await
                .unwrap(),
            );
        }
        assert!(!stores[0].shares_google_provider_store(&stores[1]));
        for (store, subject) in stores.iter().zip(["account-a", "account-b"]) {
            for (subject, email) in [
                (subject, "alias@example.com"),
                ("unrelated", "unrelated@example.com"),
            ] {
                store
                    .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
                        subject: subject.into(),
                        email: Some(email.into()),
                        client_id: "client".into(),
                        granted_scopes: vec!["openid".into()],
                        access_token: "test-access".into(),
                        refresh_token: "test-refresh".into(),
                        token_received_at: 1,
                        access_token_expires_at: 3601,
                        issuer: Some("https://accounts.google.com".into()),
                        refreshed: false,
                        scope_upgraded: true,
                    })
                    .await
                    .unwrap();
            }
        }
        let config = |name: &str, account: &str| {
            serde_json::from_value::<labby_runtime::gateway_config::UpstreamConfig>(serde_json::json!({
                "name": name, "url": "https://example.com/mcp",
                "oauth": {"mode": "authorization_code_pkce", "registration": {"strategy": "preregistered", "client_id": "client"},
                          "credential": {"source": "google_provider", "account": account}}
            })).unwrap()
        };
        let configs = [
            config("selected-a", "alias@example.com"),
            config("sibling-a", "account-a"),
            config("unrelated-a", "unrelated@example.com"),
            config("selected-b", "alias@example.com"),
            config("sibling-b", "account-b"),
            config("unrelated-b", "unrelated@example.com"),
        ];
        let managers = Arc::new(DashMap::new());
        for (index, config) in configs.iter().enumerate() {
            managers.insert(
                config.name.clone(),
                UpstreamOauthManager::new(
                    stores[index / 3].clone(),
                    crate::upstream::encryption::load_key(
                        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                    )
                    .unwrap(),
                    config.clone(),
                    "http://127.0.0.1/callback".into(),
                ),
            );
        }
        let cache = OauthClientCache::new(managers);
        let epochs = configs
            .iter()
            .map(|config| cache.lifecycle_epoch_for_upstream(&config.name))
            .collect::<Vec<_>>();
        let handler_a = cache
            .refresh_rejection_handler(&configs[0], "operator")
            .await
            .unwrap();
        let handler_b = cache
            .refresh_rejection_handler(&configs[3], "operator")
            .await
            .unwrap();

        handler_a(Some("account-b".into()));
        handler_b(Some("account-a".into()));
        assert!(cache.take_pending_invalidations().is_empty());
        for (handler, provider, expected) in [
            (&handler_a, "account-a", ["selected-a", "sibling-a"]),
            (&handler_b, "account-b", ["selected-b", "sibling-b"]),
        ] {
            handler(Some(provider.into()));
            let events = cache.take_pending_invalidations();
            let mut upstreams = events
                .iter()
                .map(|event| {
                    assert!(event.subject.is_none());
                    assert!(event.is_current());
                    event.upstream.as_str()
                })
                .collect::<Vec<_>>();
            upstreams.sort_unstable();
            assert_eq!(upstreams, expected);
            assert!(epochs.iter().all(|epoch| epoch.is_current()));
        }

        // Fencing one selected upstream must leave its sibling and the other
        // store's authority usable, without touching unrelated accounts.
        cache.advance_upstream_epoch("selected-a");
        assert!(!epochs[0].is_current());
        assert!(epochs[1..].iter().all(|epoch| epoch.is_current()));
        handler_a(Some("account-a".into()));
        let events = cache.take_pending_invalidations();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].upstream, "sibling-a");
        handler_b(Some("account-b".into()));
        let mut upstreams = cache
            .take_pending_invalidations()
            .into_iter()
            .map(|event| event.upstream)
            .collect::<Vec<_>>();
        upstreams.sort_unstable();
        assert_eq!(upstreams, ["selected-b", "sibling-b"]);
        assert!(epochs[1..].iter().all(|epoch| epoch.is_current()));
        for store in &stores {
            let membership = store
                .google_provider_subject_memberships(vec![Some("unrelated@example.com".into())])
                .await
                .unwrap();
            assert_eq!(
                membership.into_iter().next().unwrap().unwrap().as_deref(),
                Some("unrelated")
            );
        }
    }

    #[tokio::test]
    async fn google_handler_setup_uses_constant_store_operations_per_upstream() {
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteStore::open_with_key(
            directory.path().join("auth.db"),
            Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
                "membership",
            )),
        )
        .await
        .unwrap();
        store
            .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
                subject: "account-a".into(),
                email: Some("a@example.com".into()),
                client_id: "client".into(),
                granted_scopes: vec!["openid".into()],
                access_token: "test-access".into(),
                refresh_token: "test-refresh".into(),
                token_received_at: 1,
                access_token_expires_at: 3601,
                issuer: Some("https://accounts.google.com".into()),
                refreshed: false,
                scope_upgraded: true,
            })
            .await
            .unwrap();
        let configs = (0..16).map(|index| serde_json::from_value::<labby_runtime::gateway_config::UpstreamConfig>(serde_json::json!({
            "name": format!("google-{index}"), "url": "https://example.com/mcp",
            "oauth": {"mode": "authorization_code_pkce", "registration": {"strategy": "preregistered", "client_id": "client"},
                      "credential": {"source": "google_provider", "account": "a@example.com"}}
        })).unwrap()).collect::<Vec<_>>();
        let managers = Arc::new(DashMap::new());
        for config in &configs {
            managers.insert(
                config.name.clone(),
                UpstreamOauthManager::new(
                    store.clone(),
                    crate::upstream::encryption::load_key(
                        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                    )
                    .unwrap(),
                    config.clone(),
                    "http://127.0.0.1/callback".into(),
                ),
            );
        }
        let cache = OauthClientCache::new(managers);
        let before = store.next_conn.load(Ordering::Relaxed);
        for config in &configs {
            let handler = cache
                .refresh_rejection_handler(config, "operator")
                .await
                .unwrap();
            handler(Some("account-a".into()));
        }
        let operations = store.next_conn.load(Ordering::Relaxed) - before;
        eprintln!(
            "Google handler setup: {operations} store operations for {} upstreams",
            configs.len()
        );
        assert!(
            operations <= 3 * configs.len(),
            "fleet setup used {operations} store operations for {} upstreams",
            configs.len()
        );
        assert_eq!(cache.take_pending_invalidations().len(), 16);
    }
}
