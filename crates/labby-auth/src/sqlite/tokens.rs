//! Refresh-token persistence operations.
//!
//! Raw refresh tokens are hashed before storage. Upstream provider refresh
//! tokens are encrypted at rest when the store has an encryption key.

use rusqlite::{OptionalExtension, params};

use crate::at_rest::{maybe_decrypt, maybe_encrypt, require_encrypt};
use crate::error::AuthError;
use crate::types::{
    GoogleProviderCredentialRow, GoogleProviderInvalidation, RefreshTokenRow, TokenResponse,
};
use crate::util::now_unix;

use super::{SqliteStore, hash_token, sqlite_error};

impl SqliteStore {
    #[cfg(all(test, feature = "http-axum"))]
    pub(crate) async fn refresh_claim_state(
        &self,
        refresh_token: &str,
    ) -> Result<Option<(String, i64)>, AuthError> {
        let hash = hash_token(refresh_token);
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT refresh_claim_id, refresh_claim_expires_at
                 FROM refresh_tokens
                 WHERE refresh_token_hash = ?1 AND refresh_claim_id IS NOT NULL",
                params![hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sqlite_error)
        })
        .await
    }

    /// Atomically lease a refresh token before contacting the upstream
    /// provider. A stale lease can be recovered after `lease_expires_at`.
    pub async fn claim_refresh_token(
        &self,
        refresh_token: &str,
        claim_id: &str,
        lease_expires_at: i64,
    ) -> Result<Option<RefreshTokenRow>, AuthError> {
        let hash = hash_token(refresh_token);
        let plaintext = refresh_token.to_string();
        let claim_id = claim_id.to_string();
        let now = now_unix();
        let enc_key = self.enc_key.clone();
        self.with_conn(move |conn| {
            let transaction = conn.transaction().map_err(sqlite_error)?;
            let claimed = transaction
                .execute(
                    "UPDATE refresh_tokens
                     SET refresh_claim_id = ?2, refresh_claim_expires_at = ?3
                     WHERE refresh_token_hash = ?1
                       AND expires_at > ?4
                       AND (refresh_claim_id IS NULL OR refresh_claim_expires_at <= ?4)",
                    params![hash, claim_id, lease_expires_at, now],
                )
                .map_err(sqlite_error)?;
            if claimed == 0 {
                return Ok(None);
            }
            let mut row = transaction
                .query_row(
                    "SELECT client_id, subject, scope, provider_refresh_token,
                            created_at, expires_at, resource
                     FROM refresh_tokens
                     WHERE refresh_token_hash = ?1 AND refresh_claim_id = ?2",
                    params![hash, claim_id],
                    |row| {
                        Ok(RefreshTokenRow {
                            refresh_token: plaintext,
                            client_id: row.get(0)?,
                            subject: row.get(1)?,
                            scope: row.get(2)?,
                            provider_refresh_token: row.get(3)?,
                            created_at: row.get(4)?,
                            expires_at: row.get(5)?,
                            resource: row.get(6)?,
                        })
                    },
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            if let Some(raw) = row.provider_refresh_token.as_deref() {
                row.provider_refresh_token = Some(maybe_decrypt(enc_key.as_deref(), raw)?);
            }
            Ok(Some(row))
        })
        .await
    }

    pub async fn claim_bound_refresh_token(
        &self,
        refresh_token: &str,
        claim_id: &str,
        lease_expires_at: i64,
    ) -> Result<Option<crate::types::ProviderBound<RefreshTokenRow>>, AuthError> {
        let hash = hash_token(refresh_token);
        let plaintext = refresh_token.to_string();
        let claim_id = claim_id.to_string();
        let now = now_unix();
        let enc_key = self.enc_key.clone();
        self.with_conn(move |conn| {
            let transaction = conn.transaction().map_err(sqlite_error)?;
            let claimed = transaction.execute(
                "UPDATE refresh_tokens SET refresh_claim_id = ?2, refresh_claim_expires_at = ?3
                 WHERE refresh_token_hash = ?1 AND expires_at > ?4
                   AND provider_generation = (SELECT generation FROM inbound_identity_provider WHERE singleton = 1)
                   AND (refresh_claim_id IS NULL OR refresh_claim_expires_at <= ?4)",
                params![hash, claim_id, lease_expires_at, now],
            ).map_err(sqlite_error)?;
            if claimed == 0 { return Ok(None); }
            let mut bound = transaction.query_row(
                "SELECT client_id, subject, scope, provider_refresh_token, created_at, expires_at,
                        resource, identity_issuer, provider_generation
                 FROM refresh_tokens WHERE refresh_token_hash = ?1 AND refresh_claim_id = ?2",
                params![hash, claim_id],
                |row| Ok(crate::types::ProviderBound {
                    value: RefreshTokenRow { refresh_token: plaintext, client_id: row.get(0)?,
                        subject: row.get(1)?, scope: row.get(2)?, provider_refresh_token: row.get(3)?,
                        created_at: row.get(4)?, expires_at: row.get(5)?, resource: row.get(6)? },
                    binding: crate::types::ProviderBinding {
                        identity_issuer: row.get(7)?, provider_generation: row.get(8)?,
                    },
                }),
            ).map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            if let Some(raw) = bound.value.provider_refresh_token.as_deref() {
                bound.value.provider_refresh_token = Some(maybe_decrypt(enc_key.as_deref(), raw)?);
            }
            Ok(Some(bound))
        }).await
    }

    pub async fn release_refresh_claim(
        &self,
        refresh_token: &str,
        claim_id: &str,
    ) -> Result<(), AuthError> {
        let hash = hash_token(refresh_token);
        let claim_id = claim_id.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE refresh_tokens
                 SET refresh_claim_id = NULL, refresh_claim_expires_at = NULL
                 WHERE refresh_token_hash = ?1 AND refresh_claim_id = ?2",
                params![hash, claim_id],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// Extend a live refresh-token lease only while `claim_id` owns it.
    /// A false result means ownership or token validity was lost.
    pub async fn renew_refresh_claim(
        &self,
        refresh_token: &str,
        claim_id: &str,
        lease_expires_at: i64,
    ) -> Result<bool, AuthError> {
        let hash = hash_token(refresh_token);
        let claim_id = claim_id.to_string();
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE refresh_tokens
                 SET refresh_claim_expires_at = ?3
                 WHERE refresh_token_hash = ?1
                   AND refresh_claim_id = ?2
                   AND refresh_claim_expires_at > ?4
                   AND expires_at > ?4",
                params![hash, claim_id, lease_expires_at, now],
            )
            .map(|updated| updated == 1)
            .map_err(sqlite_error)
        })
        .await
    }

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
                    provider_refresh_token, created_at, expires_at,
                    identity_issuer, provider_generation
                 ) SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, issuer, generation
                     FROM inbound_identity_provider WHERE singleton = 1
                 ON CONFLICT(refresh_token_hash) DO UPDATE SET
                    client_id = excluded.client_id,
                    subject = excluded.subject,
                    resource = excluded.resource,
                    scope = excluded.scope,
                    provider_refresh_token = excluded.provider_refresh_token,
                    created_at = excluded.created_at,
                    expires_at = excluded.expires_at,
                    identity_issuer = excluded.identity_issuer,
                    provider_generation = excluded.provider_generation",
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

    /// Insert a refresh token only while the provider generation that verified
    /// its authorization code is still the active durable generation.
    pub async fn upsert_bound_refresh_token(
        &self,
        token: RefreshTokenRow,
        binding: crate::types::ProviderBinding,
    ) -> Result<(), AuthError> {
        let hash = hash_token(&token.refresh_token);
        let encrypted_provider_rt = token
            .provider_refresh_token
            .as_deref()
            .map(|raw| maybe_encrypt(self.enc_key.as_deref(), raw))
            .transpose()?;
        self.with_conn(move |conn| {
            let count = conn
                .execute(
                    "INSERT INTO refresh_tokens (
                        refresh_token_hash, client_id, subject, resource, scope,
                        provider_refresh_token, created_at, expires_at,
                        identity_issuer, provider_generation)
                     SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10
                      WHERE EXISTS (SELECT 1 FROM inbound_identity_provider
                        WHERE singleton = 1 AND issuer = ?9 AND generation = ?10)
                     ON CONFLICT(refresh_token_hash) DO UPDATE SET
                        client_id = excluded.client_id,
                        subject = excluded.subject,
                        resource = excluded.resource,
                        scope = excluded.scope,
                        provider_refresh_token = excluded.provider_refresh_token,
                        created_at = excluded.created_at,
                        expires_at = excluded.expires_at,
                        identity_issuer = excluded.identity_issuer,
                        provider_generation = excluded.provider_generation",
                    params![
                        hash,
                        token.client_id,
                        token.subject,
                        token.resource,
                        token.scope,
                        encrypted_provider_rt,
                        token.created_at,
                        token.expires_at,
                        binding.identity_issuer,
                        binding.provider_generation,
                    ],
                )
                .map_err(sqlite_error)?;
            if count == 1 {
                Ok(())
            } else {
                Err(AuthError::InvalidGrant(
                    "inbound provider changed before refresh issuance".into(),
                ))
            }
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
            let transaction = conn.transaction().map_err(sqlite_error)?;
            let deleted = transaction
                .execute(
                    "DELETE FROM refresh_tokens
                     WHERE refresh_token_hash = ?1
                       AND expires_at > ?2",
                    params![old_hash, now],
                )
                .map_err(sqlite_error)?;

            if deleted == 0 {
                return Ok(None);
            }

            transaction
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
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(Some(new_token))
        })
        .await
    }

    /// Atomically replace a refresh token only when the caller owns its lease.
    pub async fn rotate_claimed_refresh_token(
        &self,
        old_token: &str,
        claim_id: &str,
        new_token: RefreshTokenRow,
        response: &TokenResponse,
        replay_expires_at: i64,
        binding: crate::types::ProviderBinding,
    ) -> Result<Option<RefreshTokenRow>, AuthError> {
        let old_hash = hash_token(old_token);
        let claim_id = claim_id.to_string();
        let new_hash = hash_token(&new_token.refresh_token);
        let now = now_unix();
        let encrypted_provider_rt = new_token
            .provider_refresh_token
            .as_deref()
            .map(|raw| maybe_encrypt(self.enc_key.as_deref(), raw))
            .transpose()?;
        let response_json = serde_json::to_string(response)
            .map_err(|error| AuthError::Storage(format!("serialize refresh replay: {error}")))?;
        let encrypted_response = require_encrypt(self.enc_key.as_deref(), &response_json)?;
        let replay_client_id = new_token.client_id.clone();
        let replay_resource = new_token.resource.clone();
        self.with_conn(move |conn| {
            let transaction = conn.transaction().map_err(sqlite_error)?;
            let deleted = transaction
                .execute(
                    "DELETE FROM refresh_tokens
                     WHERE refresh_token_hash = ?1
                       AND refresh_claim_id = ?2
                       AND refresh_claim_expires_at > ?3
                       AND expires_at > ?3
                       AND identity_issuer = ?4 AND provider_generation = ?5
                       AND provider_generation = (SELECT generation FROM inbound_identity_provider WHERE singleton = 1)",
                    params![old_hash, claim_id, now, binding.identity_issuer, binding.provider_generation],
                )
                .map_err(sqlite_error)?;
            if deleted == 0 {
                return Ok(None);
            }
            transaction
                .execute(
                    "INSERT INTO refresh_tokens (
                        refresh_token_hash, client_id, subject, resource, scope,
                        provider_refresh_token, created_at, expires_at,
                        identity_issuer, provider_generation
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        new_hash,
                        new_token.client_id,
                        new_token.subject,
                        new_token.resource,
                        new_token.scope,
                        encrypted_provider_rt,
                        new_token.created_at,
                        new_token.expires_at,
                        binding.identity_issuer,
                        binding.provider_generation,
                    ],
                )
                .map_err(sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO refresh_token_replays (
                        predecessor_token_hash, client_id, resource, response,
                        replacement_token_hash, created_at, expires_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        old_hash,
                        replay_client_id,
                        replay_resource,
                        encrypted_response,
                        new_hash,
                        now,
                        replay_expires_at,
                    ],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(Some(new_token))
        })
        .await
    }

    /// Return a still-valid idempotent response for a recently rotated token.
    /// The successor foreign key makes consumption or revocation invalidate it.
    pub async fn find_refresh_token_replay(
        &self,
        predecessor_token: &str,
        client_id: &str,
        requested_resource: Option<&str>,
    ) -> Result<Option<TokenResponse>, AuthError> {
        let predecessor_hash = hash_token(predecessor_token);
        let client_id = client_id.to_string();
        let requested_resource = requested_resource.map(str::to_string);
        let now = now_unix();
        let enc_key = self.enc_key.clone();
        let encrypted = self
            .with_conn(move |conn| {
                conn.query_row(
                    "SELECT replay.resource, replay.response
                     FROM refresh_token_replays AS replay
                     JOIN refresh_tokens AS replacement
                       ON replacement.refresh_token_hash = replay.replacement_token_hash
                     WHERE replay.predecessor_token_hash = ?1
                       AND replay.client_id = ?2
                       AND replay.expires_at > ?3
                       AND replacement.expires_at > ?3",
                    params![predecessor_hash, client_id, now],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(sqlite_error)
            })
            .await?;
        let Some((resource, encrypted_response)) = encrypted else {
            return Ok(None);
        };
        if requested_resource
            .as_deref()
            .is_some_and(|requested| requested != resource)
        {
            return Ok(None);
        }
        let response_json = maybe_decrypt(enc_key.as_deref(), &encrypted_response)?;
        serde_json::from_str(&response_json)
            .map(Some)
            .map_err(|error| AuthError::Storage(format!("deserialize refresh replay: {error}")))
    }

    /// Return the client bound to a still-valid replayable predecessor.
    pub async fn find_refresh_token_replay_client(
        &self,
        predecessor_token: &str,
    ) -> Result<Option<String>, AuthError> {
        let predecessor_hash = hash_token(predecessor_token);
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT replay.client_id
                 FROM refresh_token_replays AS replay
                 JOIN refresh_tokens AS replacement
                   ON replacement.refresh_token_hash = replay.replacement_token_hash
                 WHERE replay.predecessor_token_hash = ?1
                   AND replay.expires_at > ?2
                   AND replacement.expires_at > ?2",
                params![predecessor_hash, now],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_error)
        })
        .await
    }

    /// Revoke the successor represented by a replayable predecessor.
    /// The foreign-key cascade removes the replay row in the same transaction.
    pub async fn revoke_refresh_token_replay(
        &self,
        predecessor_token: &str,
        client_id: &str,
    ) -> Result<(), AuthError> {
        let predecessor_hash = hash_token(predecessor_token);
        let client_id = client_id.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM refresh_tokens
                 WHERE refresh_token_hash = (
                   SELECT replacement_token_hash
                   FROM refresh_token_replays
                   WHERE predecessor_token_hash = ?1 AND client_id = ?2
                 )",
                params![predecessor_hash, client_id],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    #[cfg(all(test, feature = "http-axum"))]
    pub(crate) async fn expire_refresh_token_replay(
        &self,
        predecessor_token: &str,
    ) -> Result<(), AuthError> {
        let predecessor_hash = hash_token(predecessor_token);
        let expires_at = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE refresh_token_replays SET expires_at = ?2
                 WHERE predecessor_token_hash = ?1",
                params![predecessor_hash, expires_at],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    #[cfg(all(test, feature = "http-axum"))]
    pub(crate) async fn refresh_token_replay_expires_at(
        &self,
        predecessor_token: &str,
    ) -> Result<Option<i64>, AuthError> {
        let predecessor_hash = hash_token(predecessor_token);
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT expires_at FROM refresh_token_replays WHERE predecessor_token_hash = ?1",
                params![predecessor_hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_error)
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

    pub async fn find_bound_refresh_token(
        &self,
        refresh_token: &str,
    ) -> Result<Option<crate::types::ProviderBound<RefreshTokenRow>>, AuthError> {
        let hash = hash_token(refresh_token);
        let plaintext = refresh_token.to_string();
        let now = now_unix();
        let enc_key = self.enc_key.clone();
        self.with_conn(move |conn| {
            let row = conn.query_row(
                "SELECT client_id, subject, scope, provider_refresh_token, created_at, expires_at,
                        resource, identity_issuer, provider_generation
                 FROM refresh_tokens WHERE refresh_token_hash = ?1 AND expires_at > ?2
                   AND provider_generation = (SELECT generation FROM inbound_identity_provider WHERE singleton = 1)",
                params![hash, now],
                |row| Ok(crate::types::ProviderBound {
                    value: RefreshTokenRow { refresh_token: plaintext, client_id: row.get(0)?,
                        subject: row.get(1)?, scope: row.get(2)?, provider_refresh_token: row.get(3)?,
                        created_at: row.get(4)?, expires_at: row.get(5)?, resource: row.get(6)? },
                    binding: crate::types::ProviderBinding {
                        identity_issuer: row.get(7)?, provider_generation: row.get(8)?,
                    },
                }),
            ).optional().map_err(sqlite_error)?;
            match row {
                Some(mut bound) => {
                    if let Some(raw) = bound.value.provider_refresh_token.as_deref() {
                        bound.value.provider_refresh_token = Some(maybe_decrypt(enc_key.as_deref(), raw)?);
                    }
                    Ok(Some(bound))
                }
                None => Ok(None),
            }
        }).await
    }

    /// Store the single reusable Google refresh credential for a verified subject.
    ///
    /// Local OAuth clients receive their own Labby refresh tokens, but they all
    /// reference this subject-scoped provider credential instead of copying the
    /// Google token into every client session.
    pub async fn upsert_google_provider_credential(
        &self,
        subject: &str,
        email: Option<&str>,
        refresh_token: &str,
    ) -> Result<(), AuthError> {
        let subject = subject.to_string();
        let email = email
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let encrypted_refresh_token = require_encrypt(self.enc_key.as_deref(), refresh_token)?;
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO google_provider_credentials (
                    subject, email, refresh_token, generation, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 1, ?4, ?4)
                 ON CONFLICT(subject) DO UPDATE SET
                    email = COALESCE(excluded.email, google_provider_credentials.email),
                    refresh_token = excluded.refresh_token,
                    generation = google_provider_credentials.generation + 1,
                    updated_at = excluded.updated_at",
                params![subject, email, encrypted_refresh_token, now],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// Attach a verified email to a migrated credential without replacing its token.
    pub async fn associate_google_provider_email(
        &self,
        subject: &str,
        email: &str,
    ) -> Result<bool, AuthError> {
        let subject = subject.to_string();
        let email = email.trim().to_ascii_lowercase();
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE google_provider_credentials
                 SET email = ?2, updated_at = ?3
                 WHERE subject = ?1",
                params![subject, email, now],
            )
            .map(|updated| updated != 0)
            .map_err(sqlite_error)
        })
        .await
    }

    pub async fn find_google_provider_credential(
        &self,
        subject: &str,
    ) -> Result<Option<GoogleProviderCredentialRow>, AuthError> {
        self.find_google_provider_credential_by_selector(Some(subject))
            .await
    }

    pub async fn has_google_provider_credential_for_subject(
        &self,
        subject: &str,
    ) -> Result<bool, AuthError> {
        let subject = subject.to_string();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM google_provider_credentials WHERE subject = ?1
                 )",
                params![subject],
                |row| row.get::<_, i64>(0),
            )
            .map(|exists| exists != 0)
            .map_err(sqlite_error)
        })
        .await
    }

    /// Whether the only allowed Google account already has a reusable credential.
    ///
    /// This email-scoped check is safe across arbitrary DCR client IDs because
    /// Google's refresh-token issuance is keyed by Google account and the Labby
    /// Google OAuth client, not by Labby's downstream OAuth client IDs.
    pub async fn has_google_provider_credential_for_email(
        &self,
        email: &str,
    ) -> Result<bool, AuthError> {
        let email = email.trim().to_ascii_lowercase();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM google_provider_credentials
                    WHERE email = ?1 COLLATE NOCASE
                 )",
                params![email],
                |row| row.get::<_, i64>(0),
            )
            .map(|exists| exists != 0)
            .map_err(sqlite_error)
        })
        .await
    }

    /// Delete a provider credential only if it is still the generation that failed.
    ///
    /// When the compare-and-delete succeeds, every dependent Labby refresh token
    /// and pending authorization code for that Google subject is revoked in the
    /// same transaction. A concurrent request that already installed a newer
    /// provider credential wins and leaves its sessions untouched.
    pub async fn invalidate_google_provider_credential(
        &self,
        subject: &str,
        generation: i64,
    ) -> Result<GoogleProviderInvalidation, AuthError> {
        let subject = subject.to_string();
        self.with_conn(move |conn| {
            let transaction = conn.transaction().map_err(sqlite_error)?;
            let invalidated = transaction
                .execute(
                    "DELETE FROM google_provider_credentials
                     WHERE subject = ?1 AND generation = ?2",
                    params![subject, generation],
                )
                .map_err(sqlite_error)?;
            if invalidated == 0 {
                return Ok(GoogleProviderInvalidation::default());
            }
            transaction
                .execute(
                    "INSERT INTO google_provider_revocations (subject, epoch, updated_at)
                     VALUES (?1, 1, ?2)
                     ON CONFLICT(subject) DO UPDATE SET
                       epoch = google_provider_revocations.epoch + 1,
                       updated_at = excluded.updated_at",
                    params![subject, now_unix()],
                )
                .map_err(sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO google_provider_revocations (subject, epoch, updated_at)
                     VALUES ('*', 1, ?1)
                     ON CONFLICT(subject) DO UPDATE SET
                       epoch = google_provider_revocations.epoch + 1,
                       updated_at = excluded.updated_at",
                    params![now_unix()],
                )
                .map_err(sqlite_error)?;
            let revoked_authorization_codes = transaction
                .execute(
                    "DELETE FROM authorization_codes WHERE subject = ?1",
                    params![subject],
                )
                .map_err(sqlite_error)?;
            let revoked_refresh_tokens = transaction
                .execute(
                    "DELETE FROM refresh_tokens WHERE subject = ?1",
                    params![subject],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(GoogleProviderInvalidation {
                invalidated: true,
                revoked_refresh_tokens: revoked_refresh_tokens as u64,
                revoked_authorization_codes: revoked_authorization_codes as u64,
            })
        })
        .await
    }

    /// Revoke a refresh token. Unknown tokens are deliberately indistinguishable
    /// from known tokens, as required by RFC 7009.
    pub async fn revoke_refresh_token(&self, refresh_token: &str) -> Result<(), AuthError> {
        let hash = hash_token(refresh_token);
        self.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM refresh_tokens WHERE refresh_token_hash = ?1",
                params![hash],
            )
            .map_err(sqlite_error)?;
            Ok(())
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
