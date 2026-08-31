use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use tracing::warn;

mod allowlist;
mod assertions;
mod google_credentials;
mod migrations;
mod oauth;
mod rows;
mod tokens;
use migrations::run_migrations;
use rows::*;

use crate::at_rest::TokenEncryptionKey;
use crate::error::AuthError;
use crate::types::{
    AuthorizationCodeRow, AuthorizationRequestRow, BrowserLoginStateRow, BrowserSessionRow,
    NativeAuthorizationResultRow, RegisteredClient,
};

const UPSTREAM_OAUTH_STATE_MAX_TTL_SECS: i64 = 600;
/// Schema version for the `PRAGMA user_version` migration guard.
/// Increment this whenever a migration step is added to `run_migrations`.
use crate::util::{ensure_restrictive_permissions, now_unix, set_restrictive_permissions};

const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;
const SQLITE_POOL_SIZE: usize = 4;

#[derive(Clone)]
pub struct SqliteStore {
    conns: Arc<Vec<Mutex<Connection>>>,
    next_conn: Arc<AtomicUsize>,
    path: Arc<PathBuf>,
    /// Optional at-rest encryption key for upstream provider refresh tokens.
    enc_key: Option<Arc<TokenEncryptionKey>>,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore")
            .field("path", &self.path)
            .field("enc_key", &self.enc_key.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

impl SqliteStore {
    pub async fn open(path: PathBuf) -> Result<Self, AuthError> {
        Self::open_with_key(path, None).await
    }

    pub async fn open_with_key(
        path: PathBuf,
        enc_key: Option<TokenEncryptionKey>,
    ) -> Result<Self, AuthError> {
        let path_for_open = path.clone();
        let conns = tokio::task::spawn_blocking(move || {
            open_connections(path_for_open.as_path(), SQLITE_POOL_SIZE)
        })
        .await;
        let store = match conns {
            Ok(result) => result,
            Err(error) => Err(AuthError::Storage(format!(
                "sqlite open task failed: {error}"
            ))),
        }
        .map(|conns| Self {
            conns: Arc::new(conns.into_iter().map(Mutex::new).collect()),
            next_conn: Arc::new(AtomicUsize::new(0)),
            path: Arc::new(path),
            enc_key: enc_key.map(Arc::new),
        })?;

        store.cleanup_expired_bounded(256).await?;
        let encrypted_legacy_rows = store.encrypt_legacy_google_provider_credentials().await?;
        if encrypted_legacy_rows > 0 {
            warn!(
                encrypted_legacy_rows,
                "encrypted legacy plaintext Google provider credentials"
            );
        }
        Ok(store)
    }

    #[cfg(test)]
    pub(crate) async fn reopen_for_test(&self) -> Result<Self, AuthError> {
        Self::open_with_key(self.path.as_ref().clone(), self.enc_key.as_deref().cloned()).await
    }

    pub async fn pragma(&self, name: &str) -> Result<String, AuthError> {
        let pragma = match name {
            "journal_mode" | "busy_timeout" | "foreign_keys" => name.to_string(),
            other => {
                return Err(AuthError::Config(format!(
                    "unsupported pragma query `{other}`"
                )));
            }
        };

        self.with_conn(move |conn| {
            conn.query_row(&format!("PRAGMA {pragma};"), [], |row| {
                row.get::<_, Value>(0)
            })
            .map(|value| match value {
                Value::Text(text) => text,
                Value::Integer(int) => int.to_string(),
                other => format!("{other:?}"),
            })
            .map_err(sqlite_error)
        })
        .await
    }

    pub async fn register_client(&self, client: RegisteredClient) -> Result<(), AuthError> {
        self.with_conn(move |conn| {
            let redirect_uris = serde_json::to_string(&client.redirect_uris)
                .map_err(|error| AuthError::Storage(format!("serialize redirect_uris: {error}")))?;
            conn.execute(
                "INSERT INTO registered_clients (client_id, redirect_uris, created_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(client_id) DO UPDATE SET
                    redirect_uris = excluded.redirect_uris,
                    created_at = excluded.created_at",
                params![client.client_id, redirect_uris, client.created_at],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    pub async fn find_client(
        &self,
        client_id: &str,
    ) -> Result<Option<RegisteredClient>, AuthError> {
        let client_id = client_id.to_string();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT client_id, redirect_uris, created_at
                 FROM registered_clients
                 WHERE client_id = ?1",
                params![client_id],
                |row| {
                    let redirect_uris: String = row.get(1)?;
                    let redirect_uris = serde_json::from_str(&redirect_uris).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    Ok(RegisteredClient {
                        client_id: row.get(0)?,
                        redirect_uris,
                        created_at: row.get(2)?,
                        token_endpoint_auth_method: "none".to_string(),
                        token_endpoint_auth_methods: Vec::new(),
                        jwks: None,
                        jwks_uri: None,
                    })
                },
            )
            .optional()
            .map_err(sqlite_error)
        })
        .await
    }

    pub async fn insert_authorization_request(
        &self,
        request: AuthorizationRequestRow,
    ) -> Result<(), AuthError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO authorization_requests (
                    state, client_id, redirect_uri, client_state, resource, scope, provider_code_verifier,
                    code_challenge, code_challenge_method, created_at, expires_at, native_poll_token_hash
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    request.state,
                    request.client_id,
                    request.redirect_uri,
                    request.client_state,
                    request.resource,
                    request.scope,
                    request.provider_code_verifier,
                    request.code_challenge,
                    request.code_challenge_method,
                    request.created_at,
                    request.expires_at,
                    request.native_poll_token_hash,
                ],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    pub async fn take_authorization_request(
        &self,
        state: &str,
    ) -> Result<AuthorizationRequestRow, AuthError> {
        let state = state.to_string();
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.query_row(
                "DELETE FROM authorization_requests
                 WHERE state = ?1
                   AND expires_at > ?2
                 RETURNING state, client_id, redirect_uri, client_state, scope, provider_code_verifier,
                           code_challenge, code_challenge_method, created_at, expires_at, resource,
                           native_poll_token_hash",
                params![state, now],
                row_to_authorization_request,
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => AuthError::InvalidGrant(
                    "authorization state is missing, expired, or already used".to_string(),
                ),
                other => sqlite_error(other),
            })
        })
        .await
    }

    pub async fn insert_auth_code(&self, code: AuthorizationCodeRow) -> Result<(), AuthError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO authorization_codes (
                    code, client_id, subject, redirect_uri, resource, scope,
                    code_challenge, code_challenge_method, provider_refresh_token,
                    created_at, expires_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    code.code,
                    code.client_id,
                    code.subject,
                    code.redirect_uri,
                    code.resource,
                    code.scope,
                    code.code_challenge,
                    code.code_challenge_method,
                    code.provider_refresh_token,
                    code.created_at,
                    code.expires_at,
                ],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// Test-only primitive for exercising one-time and expiry semantics without
    /// bypassing verified redemption in production code.
    #[cfg(test)]
    pub(crate) async fn redeem_auth_code(
        &self,
        code: &str,
    ) -> Result<AuthorizationCodeRow, AuthError> {
        let code = code.to_string();
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.query_row(
                "DELETE FROM authorization_codes
                 WHERE code = ?1
                   AND expires_at > ?2
                 RETURNING code, client_id, subject, redirect_uri, scope,
                           code_challenge, code_challenge_method, provider_refresh_token,
                           created_at, expires_at, resource",
                params![code, now],
                row_to_authorization_code,
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => AuthError::InvalidGrant(
                    "authorization code is missing, expired, or already redeemed".to_string(),
                ),
                other => sqlite_error(other),
            })
        })
        .await
    }

    /// Atomically verify every grant-bound authorization-code attribute and
    /// consume the code only when all of them match.
    pub async fn redeem_verified_auth_code(
        &self,
        code: &str,
        client_id: &str,
        redirect_uri: &str,
        resource: Option<&str>,
        code_challenge: &str,
        code_challenge_method: &str,
    ) -> Result<AuthorizationCodeRow, AuthError> {
        let code = code.to_string();
        let client_id = client_id.to_string();
        let redirect_uri = redirect_uri.to_string();
        let resource = resource.map(str::to_string);
        let code_challenge = code_challenge.to_string();
        let code_challenge_method = code_challenge_method.to_string();
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.query_row(
                "DELETE FROM authorization_codes
                 WHERE code = ?1
                   AND expires_at > ?2
                   AND client_id = ?3
                   AND redirect_uri = ?4
                   AND (?5 IS NULL OR resource = ?5)
                   AND code_challenge = ?6
                   AND code_challenge_method = ?7
                 RETURNING code, client_id, subject, redirect_uri, scope,
                           code_challenge, code_challenge_method, provider_refresh_token,
                           created_at, expires_at, resource",
                params![
                    code,
                    now,
                    client_id,
                    redirect_uri,
                    resource,
                    code_challenge,
                    code_challenge_method,
                ],
                row_to_authorization_code,
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => AuthError::InvalidGrant(
                    "authorization code is missing, expired, already redeemed, or does not match the grant"
                        .to_string(),
                ),
                other => sqlite_error(other),
            })
        })
        .await
    }

    pub async fn upsert_browser_session(
        &self,
        session: BrowserSessionRow,
    ) -> Result<(), AuthError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO browser_sessions (
                    session_id, subject, email, csrf_token, created_at, expires_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(session_id) DO UPDATE SET
                    subject = excluded.subject,
                    email = excluded.email,
                    csrf_token = excluded.csrf_token,
                    created_at = excluded.created_at,
                    expires_at = excluded.expires_at",
                params![
                    session.session_id,
                    session.subject,
                    session.email,
                    session.csrf_token,
                    session.created_at,
                    session.expires_at,
                ],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    pub async fn find_browser_session(
        &self,
        session_id: &str,
    ) -> Result<Option<BrowserSessionRow>, AuthError> {
        let session_id = session_id.to_string();
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT session_id, subject, email, csrf_token, created_at, expires_at
                 FROM browser_sessions
                 WHERE session_id = ?1
                   AND expires_at > ?2",
                params![session_id, now],
                row_to_browser_session,
            )
            .optional()
            .map_err(sqlite_error)
        })
        .await
    }

    pub async fn revoke_browser_session(&self, session_id: &str) -> Result<(), AuthError> {
        let session_id = session_id.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM browser_sessions WHERE session_id = ?1",
                params![session_id],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    pub async fn execute_test_statement(&self, sql: &str) -> Result<(), AuthError> {
        let sql = sql.to_string();
        self.with_conn(move |conn| conn.execute_batch(&sql).map_err(sqlite_error))
            .await
    }

    pub async fn insert_browser_login_state(
        &self,
        login: BrowserLoginStateRow,
    ) -> Result<(), AuthError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO browser_login_states (
                    state, return_to, provider_code_verifier, created_at, expires_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    login.state,
                    login.return_to,
                    login.provider_code_verifier,
                    login.created_at,
                    login.expires_at,
                ],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    pub async fn count_pending_oauth_states(&self) -> Result<usize, AuthError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            let authorization_requests: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM authorization_requests WHERE expires_at > ?1",
                    params![now],
                    |row| row.get(0),
                )
                .map_err(sqlite_error)?;
            let browser_login_states: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM browser_login_states WHERE expires_at > ?1",
                    params![now],
                    |row| row.get(0),
                )
                .map_err(sqlite_error)?;
            let native_authorization_results: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM native_authorization_results WHERE expires_at > ?1",
                    params![now],
                    |row| row.get(0),
                )
                .map_err(sqlite_error)?;
            Ok(
                (authorization_requests + browser_login_states + native_authorization_results)
                    as usize,
            )
        })
        .await
    }

    pub async fn take_browser_login_state(
        &self,
        state: &str,
    ) -> Result<Option<BrowserLoginStateRow>, AuthError> {
        let state = state.to_string();
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.query_row(
                "DELETE FROM browser_login_states
                 WHERE state = ?1
                   AND expires_at > ?2
                 RETURNING state, return_to, provider_code_verifier, created_at, expires_at",
                params![state, now],
                row_to_browser_login_state,
            )
            .optional()
            .map_err(sqlite_error)
        })
        .await
    }

    /// Store a native-flow authorization code keyed by a polling-token hash, for the
    /// polling desktop client to retrieve via `take_native_authorization_result`.
    ///
    /// Last-write-wins on an effectively impossible token-hash collision: each row is
    /// single-use (deleted on first successful poll), so overwriting with the
    /// newest code is correct — silently dropping the newest code instead
    /// (`DO NOTHING`) would leave the polling client hung until the row's TTL
    /// expires, with no error surfaced anywhere.
    pub async fn insert_native_authorization_result(
        &self,
        result: NativeAuthorizationResultRow,
    ) -> Result<(), AuthError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO native_authorization_results (poll_token_hash, code, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(poll_token_hash) DO UPDATE SET
                    code = excluded.code,
                    created_at = excluded.created_at,
                    expires_at = excluded.expires_at",
                params![
                    result.poll_token_hash,
                    result.code,
                    result.created_at,
                    result.expires_at,
                ],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// One-shot read-and-delete of a pending native-flow authorization code.
    pub async fn take_native_authorization_result(
        &self,
        poll_token_hash: &str,
    ) -> Result<Option<NativeAuthorizationResultRow>, AuthError> {
        let poll_token_hash = poll_token_hash.to_string();
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.query_row(
                "DELETE FROM native_authorization_results
                 WHERE poll_token_hash = ?1
                   AND expires_at > ?2
                 RETURNING poll_token_hash, code, created_at, expires_at",
                params![poll_token_hash, now],
                row_to_native_authorization_result,
            )
            .optional()
            .map_err(sqlite_error)
        })
        .await
    }

    /// Delete expired rows from all short-lived tables. Also drops upstream OAuth
    /// credential rows whose access token has expired AND have no refresh token
    /// available for re-use (SEC-9). Returns the total number of deleted rows.
    pub async fn cleanup_expired(&self) -> Result<u64, AuthError> {
        self.cleanup_expired_bounded(u32::MAX).await
    }

    /// Delete at most `limit` expired rows from each short-lived table.
    ///
    /// The fixed table set gives the operation a hard upper bound while ensuring
    /// a busy table cannot starve cleanup of the tables that follow it.
    pub async fn cleanup_expired_bounded(&self, limit: u32) -> Result<u64, AuthError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            let transaction = conn.transaction().map_err(sqlite_error)?;
            let mut total: u64 = 0;
            for (table, key) in [
                ("authorization_requests", "state"),
                ("authorization_codes", "code"),
                ("refresh_token_replays", "predecessor_token_hash"),
                ("refresh_tokens", "refresh_token_hash"),
                ("browser_sessions", "session_id"),
                ("browser_login_states", "state"),
                ("native_authorization_results", "poll_token_hash"),
            ] {
                let deleted = transaction
                    .execute(
                        &format!(
                            "DELETE FROM {table} WHERE {key} IN (
                                SELECT {key} FROM {table} WHERE expires_at <= ?1 LIMIT ?2
                             )"
                        ),
                        params![now, i64::from(limit)],
                    )
                    .map_err(sqlite_error)?;
                total += deleted as u64;
            }
            let deleted = transaction
                .execute(
                    "DELETE FROM assertion_jtis WHERE (issuer, jti) IN (
                            SELECT issuer, jti FROM assertion_jtis
                            WHERE expires_at <= ?1 LIMIT ?2
                         )",
                    params![now, i64::from(limit)],
                )
                .map_err(sqlite_error)?;
            total += deleted as u64;
            let deleted = transaction
                .execute(
                    "DELETE FROM upstream_oauth_state
                         WHERE (upstream_name, subject, csrf_token) IN (
                            SELECT upstream_name, subject, csrf_token FROM upstream_oauth_state
                            WHERE expires_at <= ?1 LIMIT ?2
                         )",
                    params![now, i64::from(limit)],
                )
                .map_err(sqlite_error)?;
            total += deleted as u64;
            let deleted = transaction
                .execute(
                    "DELETE FROM upstream_oauth_credentials
                         WHERE (upstream_name, subject) IN (
                            SELECT upstream_name, subject FROM upstream_oauth_credentials
                            WHERE access_token_expires_at <= ?1 AND refresh_token_present = 0
                            LIMIT ?2
                         )",
                    params![now, i64::from(limit)],
                )
                .map_err(sqlite_error)?;
            total += deleted as u64;
            transaction.commit().map_err(sqlite_error)?;
            Ok(total)
        })
        .await
    }

    async fn with_conn<T, F>(&self, op: F) -> Result<T, AuthError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, AuthError> + Send + 'static,
    {
        let conns = Arc::clone(&self.conns);
        let path = Arc::clone(&self.path);
        let len = conns.len();
        let idx = self.next_conn.fetch_add(1, Ordering::Relaxed) % len;
        tokio::task::spawn_blocking(move || {
            let mut guard = conns[idx]
                .lock()
                .map_err(|_| AuthError::Storage("sqlite mutex poisoned".to_string()))?;
            validate_or_reopen_connection(&mut guard, path.as_ref())?;
            op(&mut guard)
        })
        .await
        .map_err(|error| AuthError::Storage(format!("sqlite task failed: {error}")))?
    }

    #[cfg(test)]
    fn connection_count(&self) -> usize {
        self.conns.len()
    }
}

fn open_connections(path: &Path, count: usize) -> Result<Vec<Connection>, AuthError> {
    (0..count).map(|_| open_connection(path)).collect()
}

#[allow(clippy::too_many_lines)]
fn open_connection(path: &Path) -> Result<Connection, AuthError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            AuthError::Storage(format!(
                "create auth database directory `{}`: {error}",
                parent.display()
            ))
        })?;
    }

    let existed = path.exists();
    if existed {
        ensure_restrictive_permissions(path)?;
    }

    let conn = Connection::open(path).map_err(sqlite_error)?;
    conn.busy_timeout(std::time::Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(sqlite_error)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(sqlite_error)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(sqlite_error)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS registered_clients (
            client_id TEXT PRIMARY KEY,
            redirect_uris TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS authorization_requests (
            state TEXT PRIMARY KEY,
            client_id TEXT NOT NULL,
            redirect_uri TEXT NOT NULL,
            client_state TEXT NOT NULL,
            native_poll_token_hash TEXT,
            resource TEXT NOT NULL DEFAULT '',
            scope TEXT NOT NULL,
            provider_code_verifier TEXT NOT NULL,
            code_challenge TEXT NOT NULL,
            code_challenge_method TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS authorization_codes (
            code TEXT PRIMARY KEY,
            client_id TEXT NOT NULL,
            subject TEXT NOT NULL,
            redirect_uri TEXT NOT NULL,
            resource TEXT NOT NULL DEFAULT '',
            scope TEXT NOT NULL,
            code_challenge TEXT NOT NULL,
            code_challenge_method TEXT NOT NULL,
            provider_refresh_token TEXT,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS refresh_tokens (
            refresh_token_hash TEXT PRIMARY KEY,
            client_id TEXT NOT NULL REFERENCES registered_clients(client_id),
            subject TEXT NOT NULL,
            resource TEXT NOT NULL DEFAULT '',
            scope TEXT NOT NULL,
            provider_refresh_token TEXT,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS refresh_token_replays (
            predecessor_token_hash TEXT PRIMARY KEY,
            client_id TEXT NOT NULL,
            resource TEXT NOT NULL,
            response TEXT NOT NULL,
            replacement_token_hash TEXT NOT NULL
                REFERENCES refresh_tokens(refresh_token_hash) ON DELETE CASCADE,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_refresh_token_replays_expiry
            ON refresh_token_replays(expires_at);
        CREATE TABLE IF NOT EXISTS google_provider_credentials (
            subject TEXT PRIMARY KEY,
            email TEXT,
            client_id TEXT NOT NULL DEFAULT '',
            granted_scopes_json TEXT NOT NULL DEFAULT '[\"email\",\"openid\",\"profile\"]',
            access_token TEXT,
            refresh_token TEXT NOT NULL,
            token_received_at INTEGER,
            access_token_expires_at INTEGER,
            issuer TEXT,
            last_refresh_at INTEGER,
            last_scope_upgrade_at INTEGER,
            generation INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_google_provider_credentials_email
            ON google_provider_credentials(email COLLATE NOCASE);
        CREATE TABLE IF NOT EXISTS google_provider_revocations (
            subject TEXT PRIMARY KEY,
            epoch INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS assertion_jtis (
            issuer TEXT NOT NULL,
            jti TEXT NOT NULL,
            expires_at INTEGER NOT NULL,
            PRIMARY KEY (issuer, jti)
        );
        CREATE INDEX IF NOT EXISTS idx_assertion_jtis_expiry
            ON assertion_jtis(expires_at);
        CREATE TABLE IF NOT EXISTS browser_sessions (
            session_id TEXT PRIMARY KEY,
            subject TEXT NOT NULL,
            email TEXT,
            csrf_token TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS browser_login_states (
            state TEXT PRIMARY KEY,
            return_to TEXT NOT NULL,
            provider_code_verifier TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS native_authorization_results (
            poll_token_hash TEXT PRIMARY KEY,
            code TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS upstream_oauth_credentials (
            upstream_name             TEXT NOT NULL,
            subject                   TEXT NOT NULL,
            client_id                 TEXT NOT NULL,
            granted_scopes_json       TEXT NOT NULL,
            token_blob                BLOB NOT NULL,
            token_blob_nonce          BLOB NOT NULL,
            token_received_at         INTEGER NOT NULL,
            access_token_expires_at   INTEGER NOT NULL,
            refresh_token_present     INTEGER NOT NULL,
            PRIMARY KEY (upstream_name, subject)
        ) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS upstream_oauth_state (
            upstream_name   TEXT NOT NULL,
            subject         TEXT NOT NULL,
            csrf_token      TEXT NOT NULL,
            pkce_verifier   TEXT NOT NULL,
            expected_issuer TEXT,
            require_issuer INTEGER NOT NULL DEFAULT 0,
            requested_scopes_json TEXT NOT NULL DEFAULT '[]',
            created_at      INTEGER NOT NULL,
            expires_at      INTEGER NOT NULL,
            PRIMARY KEY (upstream_name, subject, csrf_token)
        ) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS upstream_oauth_dynamic_clients (
            upstream_name   TEXT NOT NULL,
            subject         TEXT NOT NULL,
            client_id       TEXT NOT NULL,
            created_at      INTEGER NOT NULL,
            PRIMARY KEY (upstream_name, subject)
        ) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS allowed_users (
            email       TEXT PRIMARY KEY NOT NULL,
            added_by    TEXT NOT NULL,
            created_at  INTEGER NOT NULL
        );",
    )
    .map_err(sqlite_error)?;
    add_column_if_missing(
        &conn,
        "authorization_requests",
        "resource",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(
        &conn,
        "authorization_codes",
        "resource",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(
        &conn,
        "refresh_tokens",
        "resource",
        "TEXT NOT NULL DEFAULT ''",
    )?;

    if !existed {
        set_restrictive_permissions(path)?;
    }
    ensure_restrictive_permissions(path)?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
        if sidecar.exists() {
            set_restrictive_permissions(&sidecar)?;
            ensure_restrictive_permissions(&sidecar)?;
        }
    }

    run_migrations(&conn)?;

    Ok(conn)
}

/// Compute a hex-encoded SHA-256 digest of a token for safe storage.
///
/// The raw token (24+ bytes of random entropy) has sufficient pre-image
/// resistance for SHA-256 to be appropriate here — Argon2 would add
/// per-request latency without a meaningful security benefit.
fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in &digest {
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

fn validate_or_reopen_connection(conn: &mut Connection, path: &Path) -> Result<(), AuthError> {
    let Err(error) = conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0)) else {
        return Ok(());
    };
    warn!(
        path = %path.display(),
        error = %error,
        "stale sqlite connection detected, reopening"
    );

    *conn = open_connection(path)?;
    conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
        .map(|_| ())
        .map_err(sqlite_error)
}

#[allow(clippy::needless_pass_by_value)]
fn sqlite_error(error: rusqlite::Error) -> AuthError {
    AuthError::Storage(format!("sqlite error: {error}"))
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), AuthError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(sqlite_error)?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sqlite_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(sqlite_error)?
        .iter()
        .any(|name| name == column);
    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )
        .map_err(sqlite_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
