//! SQLite persistence for browser identity and sanitized catalog state.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use base64::Engine as _;
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{BrowserError, Result};
use crate::protocol::CatalogObservation;

const PAIRING_TTL_SECONDS: i64 = 300;
const CHALLENGE_TTL_SECONDS: i64 = 60;
const MAX_CATALOG_BYTES: usize = 256 * 1024;
const MAX_JSON_DEPTH: usize = 32;
const MAX_SESSIONS_PER_BROWSER: i64 = 256;
const CURRENT_SCHEMA_VERSION: i64 = 2;
const DEFAULT_SESSION_PAGE_SIZE: usize = 50;
const MAX_SESSION_PAGE_SIZE: usize = 100;
// The facade owns one mutex-protected SQLite connection. Admit only one blocking
// job at a time so contention waits asynchronously instead of occupying extra
// blocking-pool threads while they wait for the same connection mutex.
const MAX_BLOCKING_STORE_JOBS: usize = 1;

/// Durable paired browser.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BrowserRecord {
    pub id: String,
    pub display_name: String,
    pub extension_id: String,
    #[serde(skip_serializing)]
    pub public_key: Vec<u8>,
    pub paired_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// Sanitized browser document and its observed WebMCP catalog.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DocumentSession {
    pub id: String,
    pub browser_id: String,
    pub tab_id: i64,
    pub document_id: String,
    pub origin: String,
    pub sanitized_path: String,
    pub page_title: String,
    pub catalog_revision: i64,
    pub catalog_fingerprint: String,
    pub tools: Vec<crate::protocol::ToolDescriptor>,
    pub enabled: bool,
    pub status: String,
    pub last_seen_at: i64,
}

/// Metadata-only session projection used by bounded administrative listings.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DocumentSessionSummary {
    pub id: String,
    pub browser_id: String,
    pub tab_id: i64,
    pub document_id: String,
    pub origin: String,
    pub sanitized_path: String,
    pub page_title: String,
    pub catalog_revision: i64,
    pub catalog_fingerprint: String,
    pub tool_count: usize,
    pub enabled: bool,
    pub status: String,
    pub last_seen_at: i64,
}

/// One stable page of session summaries.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SessionPage {
    pub sessions: Vec<DocumentSessionSummary>,
    pub next_cursor: Option<String>,
}

/// Pairing state.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairingStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
}

impl PairingStatus {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            "expired" => Ok(Self::Expired),
            other => Err(BrowserError::InvalidRequest(format!(
                "unknown pairing status `{other}`"
            ))),
        }
    }
}

/// Durable pairing request, never containing private key material.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PairingRequest {
    pub id: String,
    pub display_name: String,
    pub extension_id: String,
    #[serde(skip_serializing)]
    pub public_key: Vec<u8>,
    pub status: PairingStatus,
    pub expires_at: i64,
    pub browser_id: Option<String>,
}

/// One-time authentication challenge.
#[derive(Clone, Debug)]
pub(crate) struct AuthChallenge {
    pub id: String,
    pub browser_id: String,
    pub nonce: Vec<u8>,
    pub expires_at: i64,
}

/// Cloneable async SQLite facade. Blocking work always runs on Tokio's blocking pool.
#[derive(Clone)]
pub struct Store {
    inner: Arc<BlockingStore>,
    permits: Arc<tokio::sync::Semaphore>,
}

struct BlockingStore {
    path: PathBuf,
    connection: Mutex<Connection>,
}

impl Store {
    #[cfg(test)]
    pub(crate) async fn hold_executor_for_test(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .expect("browser store test executor remains open")
    }

    /// Open a database and apply the transactional browser schema.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        Self::run_blocking(move || BlockingStore::open(path))
            .await
            .map(Self::from_blocking)
    }

    /// Open an in-memory store for tests.
    pub async fn memory() -> Result<Self> {
        Self::run_blocking(BlockingStore::memory)
            .await
            .map(Self::from_blocking)
    }

    /// Database path for diagnostics.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.inner.path.as_path()
    }

    fn from_blocking(inner: BlockingStore) -> Self {
        Self {
            inner: Arc::new(inner),
            permits: Arc::new(tokio::sync::Semaphore::new(MAX_BLOCKING_STORE_JOBS)),
        }
    }

    async fn call<T>(
        &self,
        operation: impl FnOnce(&BlockingStore) -> Result<T> + Send + 'static,
    ) -> Result<T>
    where
        T: Send + 'static,
    {
        let inner = Arc::clone(&self.inner);
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .map_err(|_| {
                BrowserError::InvalidRequest("browser store executor closed".to_string())
            })?;
        Self::run_blocking(move || {
            let _permit = permit;
            operation(&inner)
        })
        .await
    }

    async fn run_blocking<T>(operation: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T>
    where
        T: Send + 'static,
    {
        tokio::task::spawn_blocking(operation)
            .await
            .map_err(|error| {
                BrowserError::InvalidRequest(format!("browser store worker failed: {error}"))
            })?
    }

    pub async fn request_pairing(
        &self,
        display_name: &str,
        extension_id: &str,
        public_key: Vec<u8>,
    ) -> Result<PairingRequest> {
        let display_name = display_name.to_owned();
        let extension_id = extension_id.to_owned();
        self.call(move |store| store.request_pairing(&display_name, &extension_id, public_key))
            .await
    }
    pub async fn pairing(&self, id: &str) -> Result<Option<PairingRequest>> {
        let id = id.to_owned();
        self.call(move |s| s.pairing(&id)).await
    }
    pub async fn pending_pairings(&self) -> Result<Vec<PairingRequest>> {
        self.call(BlockingStore::pending_pairings).await
    }
    pub async fn approve_pairing(&self, id: &str) -> Result<BrowserRecord> {
        let id = id.to_owned();
        self.call(move |s| s.approve_pairing(&id)).await
    }
    pub async fn browser(&self, id: &str) -> Result<Option<BrowserRecord>> {
        let id = id.to_owned();
        self.call(move |s| s.browser(&id)).await
    }
    pub async fn browsers(&self) -> Result<Vec<BrowserRecord>> {
        self.call(BlockingStore::browsers).await
    }
    pub async fn revoke_browser(&self, id: &str) -> Result<BrowserRecord> {
        let id = id.to_owned();
        self.call(move |s| s.revoke_browser(&id)).await
    }
    pub(crate) async fn create_challenge(&self, id: &str) -> Result<AuthChallenge> {
        let id = id.to_owned();
        self.call(move |s| s.create_challenge(&id)).await
    }
    pub(crate) async fn take_challenge(&self, id: &str) -> Result<AuthChallenge> {
        let id = id.to_owned();
        self.call(move |s| s.take_challenge(&id)).await
    }
    pub(crate) async fn touch_browser(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        self.call(move |s| s.touch_browser(&id)).await
    }
    pub async fn observe(&self, id: &str, observation: &CatalogObservation) -> Result<()> {
        let id = id.to_owned();
        let observation = observation.clone();
        self.call(move |s| s.observe(&id, &observation)).await
    }
    pub async fn sessions(
        &self,
        cursor: Option<&str>,
        limit: Option<usize>,
    ) -> Result<SessionPage> {
        let cursor = cursor.map(str::to_owned);
        self.call(move |s| s.sessions(cursor.as_deref(), limit))
            .await
    }
    pub async fn session(&self, id: &str) -> Result<DocumentSession> {
        let id = id.to_owned();
        self.call(move |s| s.session(&id)).await
    }
    pub async fn set_session_enabled(&self, id: &str, enabled: bool) -> Result<DocumentSession> {
        let id = id.to_owned();
        self.call(move |s| s.set_session_enabled(&id, enabled))
            .await
    }
    pub async fn close_document(
        &self,
        browser_id: &str,
        tab_id: i64,
        document_id: &str,
    ) -> Result<()> {
        let browser_id = browser_id.to_owned();
        let document_id = document_id.to_owned();
        self.call(move |s| s.close_document(&browser_id, tab_id, &document_id))
            .await
    }
    pub async fn validate_call(
        &self,
        browser_id: &str,
        tab_id: i64,
        document_id: &str,
        revision: i64,
        tool_name: &str,
    ) -> Result<String> {
        let browser_id = browser_id.to_owned();
        let document_id = document_id.to_owned();
        let tool_name = tool_name.to_owned();
        self.call(move |s| s.validate_call(&browser_id, tab_id, &document_id, revision, &tool_name))
            .await
    }
    pub(crate) async fn begin_invocation(
        &self,
        browser_id: &str,
        tab_id: i64,
        document_id: &str,
        tool_name: &str,
        revision: i64,
    ) -> Result<String> {
        let browser_id = browser_id.to_owned();
        let document_id = document_id.to_owned();
        let tool_name = tool_name.to_owned();
        self.call(move |s| {
            s.begin_invocation(&browser_id, tab_id, &document_id, &tool_name, revision)
        })
        .await
    }
    pub(crate) async fn finish_invocation(
        &self,
        id: &str,
        result: &Result<serde_json::Value>,
        duration_ms: i64,
    ) -> Result<()> {
        let id = id.to_owned();
        let (outcome, error_kind) = match result {
            Ok(_) => ("succeeded", None),
            Err(error) => ("failed", Some(error.kind().to_string())),
        };
        self.call(move |s| {
            s.finish_invocation_parts(&id, outcome, error_kind.as_deref(), duration_ms)
        })
        .await
    }
    pub(crate) async fn abandon_invocation(&self, id: &str, duration_ms: i64) -> Result<()> {
        let id = id.to_owned();
        self.call(move |s| s.abandon_invocation(&id, duration_ms))
            .await
    }
}

impl BlockingStore {
    fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                BrowserError::InvalidRequest(format!(
                    "cannot create browser data directory: {error}"
                ))
            })?;
        }
        let connection = Connection::open(&path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&connection)?;
        Ok(Self {
            path,
            connection: Mutex::new(connection),
        })
    }

    /// Open an in-memory store for tests.
    fn memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&connection)?;
        Ok(Self {
            path: PathBuf::from(":memory:"),
            connection: Mutex::new(connection),
        })
    }

    /// Create or refresh one pending pairing request.
    fn request_pairing(
        &self,
        display_name: &str,
        extension_id: &str,
        public_key: Vec<u8>,
    ) -> Result<PairingRequest> {
        validate_pairing_input(display_name, extension_id, &public_key)?;
        let now = now_seconds()?;
        let expires_at = now + PAIRING_TTL_SECONDS;
        let id = Uuid::new_v4().to_string();
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE browser_pairing_requests SET status='expired', resolved_at=?1 WHERE extension_id=?2 AND status='pending'",
            params![now, extension_id],
        )?;
        transaction.execute(
            "INSERT INTO browser_pairing_requests(id,display_name,extension_id,public_key,status,expires_at,created_at) VALUES(?1,?2,?3,?4,'pending',?5,?6)",
            params![id, display_name, extension_id, public_key, expires_at, now],
        )?;
        transaction.commit()?;
        drop(connection);
        self.pairing(&id)?.ok_or(BrowserError::NotFound)
    }

    /// Read current pairing state, expiring stale requests atomically.
    fn pairing(&self, id: &str) -> Result<Option<PairingRequest>> {
        let now = now_seconds()?;
        let connection = self.lock()?;
        connection.execute(
            "UPDATE browser_pairing_requests SET status='expired', resolved_at=?1 WHERE id=?2 AND status='pending' AND expires_at<=?1",
            params![now, id],
        )?;
        pairing_row(&connection, id)
    }

    /// List pending pairing requests for operator approval.
    fn pending_pairings(&self) -> Result<Vec<PairingRequest>> {
        let now = now_seconds()?;
        let connection = self.lock()?;
        connection.execute(
            "UPDATE browser_pairing_requests SET status='expired', resolved_at=?1 WHERE status='pending' AND expires_at<=?1",
            params![now],
        )?;
        let mut statement = connection.prepare(
            "SELECT id,display_name,extension_id,public_key,status,expires_at,browser_id FROM browser_pairing_requests WHERE status='pending' ORDER BY created_at",
        )?;
        statement
            .query_map([], map_pairing)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Approve a pairing and create the durable browser identity.
    fn approve_pairing(&self, id: &str) -> Result<BrowserRecord> {
        let now = now_seconds()?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let pairing = pairing_row(&transaction, id)?.ok_or(BrowserError::NotFound)?;
        if pairing.status != PairingStatus::Pending || pairing.expires_at <= now {
            return Err(BrowserError::InvalidRequest(
                "pairing request is not pending".to_string(),
            ));
        }
        transaction.execute(
            "UPDATE document_sessions SET enabled=0,status='closed',last_seen_at=?1 WHERE browser_id IN (SELECT id FROM browsers WHERE extension_id=?2 AND revoked_at IS NULL) AND status='active'",
            params![now, pairing.extension_id],
        )?;
        transaction.execute(
            "UPDATE browser_auth_challenges SET used_at=?1 WHERE browser_id IN (SELECT id FROM browsers WHERE extension_id=?2 AND revoked_at IS NULL) AND used_at IS NULL",
            params![now, pairing.extension_id],
        )?;
        transaction.execute(
            "UPDATE browsers SET revoked_at=?1 WHERE extension_id=?2 AND revoked_at IS NULL",
            params![now, pairing.extension_id],
        )?;
        let browser_id = Uuid::new_v4().to_string();
        transaction.execute(
            "INSERT INTO browsers(id,display_name,extension_id,public_key,paired_at) VALUES(?1,?2,?3,?4,?5)",
            params![browser_id, pairing.display_name, pairing.extension_id, pairing.public_key, now],
        )?;
        transaction.execute(
            "UPDATE browser_pairing_requests SET status='approved',browser_id=?1,resolved_at=?2 WHERE id=?3",
            params![browser_id, now, id],
        )?;
        transaction.commit()?;
        drop(connection);
        self.browser(&browser_id)?.ok_or(BrowserError::NotFound)
    }

    /// Fetch an active or revoked browser by id.
    fn browser(&self, id: &str) -> Result<Option<BrowserRecord>> {
        self.lock()?
            .query_row(
                "SELECT id,display_name,extension_id,public_key,paired_at,last_seen_at,revoked_at FROM browsers WHERE id=?1",
                [id],
                map_browser,
            )
            .optional()
            .map_err(Into::into)
    }

    /// List durable browsers without exposing public-key bytes.
    fn browsers(&self) -> Result<Vec<BrowserRecord>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id,display_name,extension_id,public_key,paired_at,last_seen_at,revoked_at FROM browsers ORDER BY paired_at DESC",
        )?;
        statement
            .query_map([], map_browser)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Revoke one active browser identity and disable all of its sessions.
    fn revoke_browser(&self, id: &str) -> Result<BrowserRecord> {
        let now = now_seconds()?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE browsers SET revoked_at=?1 WHERE id=?2 AND revoked_at IS NULL",
            params![now, id],
        )?;
        if changed != 1 {
            return Err(BrowserError::NotFound);
        }
        transaction.execute(
            "UPDATE document_sessions SET enabled=0,status='closed',last_seen_at=?1 WHERE browser_id=?2 AND status='active'",
            params![now, id],
        )?;
        transaction.execute(
            "UPDATE browser_auth_challenges SET used_at=?1 WHERE browser_id=?2 AND used_at IS NULL",
            params![now, id],
        )?;
        transaction.commit()?;
        drop(connection);
        self.browser(id)?.ok_or(BrowserError::NotFound)
    }

    /// Create a one-time random challenge for an active browser.
    pub(crate) fn create_challenge(&self, browser_id: &str) -> Result<AuthChallenge> {
        let browser = self
            .browser(browser_id)?
            .ok_or(BrowserError::AuthenticationFailed)?;
        if browser.revoked_at.is_some() {
            return Err(BrowserError::AuthenticationFailed);
        }
        let now = now_seconds()?;
        let challenge = AuthChallenge {
            id: Uuid::new_v4().to_string(),
            browser_id: browser_id.to_string(),
            nonce: Uuid::new_v4().as_bytes().to_vec(),
            expires_at: now + CHALLENGE_TTL_SECONDS,
        };
        self.lock()?.execute(
            "INSERT INTO browser_auth_challenges(id,browser_id,nonce,expires_at,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![challenge.id, challenge.browser_id, challenge.nonce, challenge.expires_at, now],
        )?;
        Ok(challenge)
    }

    /// Atomically consume a challenge.
    pub(crate) fn take_challenge(&self, id: &str) -> Result<AuthChallenge> {
        let now = now_seconds()?;
        let connection = self.lock()?;
        let challenge = connection
            .query_row(
                "SELECT id,browser_id,nonce,expires_at FROM browser_auth_challenges WHERE id=?1 AND used_at IS NULL",
                [id],
                |row| Ok(AuthChallenge { id: row.get(0)?, browser_id: row.get(1)?, nonce: row.get(2)?, expires_at: row.get(3)? }),
            )
            .optional()?
            .ok_or(BrowserError::AuthenticationFailed)?;
        if challenge.expires_at <= now {
            return Err(BrowserError::AuthenticationFailed);
        }
        let updated = connection.execute(
            "UPDATE browser_auth_challenges SET used_at=?1 WHERE id=?2 AND used_at IS NULL",
            params![now, id],
        )?;
        if updated != 1 {
            return Err(BrowserError::AuthenticationFailed);
        }
        Ok(challenge)
    }

    /// Mark one browser as recently authenticated.
    pub(crate) fn touch_browser(&self, id: &str) -> Result<()> {
        let updated = self.lock()?.execute(
            "UPDATE browsers SET last_seen_at=?1 WHERE id=?2 AND revoked_at IS NULL",
            params![now_seconds()?, id],
        )?;
        if updated == 1 {
            Ok(())
        } else {
            Err(BrowserError::AuthenticationFailed)
        }
    }

    /// Persist a sanitized document/catalog observation.
    fn observe(&self, browser_id: &str, observation: &CatalogObservation) -> Result<()> {
        validate_observation(observation)?;
        let now = now_seconds()?;
        let catalog = serde_json::to_string(&observation.tools)?;
        self.lock()?.execute(
            "INSERT INTO document_sessions(id,browser_id,tab_id,document_id,origin,sanitized_path,page_title,catalog_revision,catalog_fingerprint,catalog_json,status,connected_at,last_seen_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'active',?11,?11) ON CONFLICT(browser_id,tab_id,document_id) DO UPDATE SET origin=excluded.origin,sanitized_path=excluded.sanitized_path,page_title=excluded.page_title,enabled=CASE WHEN document_sessions.catalog_revision=excluded.catalog_revision AND document_sessions.catalog_fingerprint=excluded.catalog_fingerprint THEN document_sessions.enabled ELSE 0 END,catalog_revision=excluded.catalog_revision,catalog_fingerprint=excluded.catalog_fingerprint,catalog_json=excluded.catalog_json,status='active',last_seen_at=excluded.last_seen_at",
            params![Uuid::new_v4().to_string(), browser_id, observation.tab_id, observation.document_id, observation.origin, observation.sanitized_path, observation.page_title, observation.catalog_revision, observation.catalog_fingerprint, catalog, now],
        )?;
        self.lock()?.execute(
            "DELETE FROM document_sessions WHERE browser_id=?1 AND id IN (SELECT id FROM document_sessions WHERE browser_id=?1 ORDER BY last_seen_at DESC, id DESC LIMIT -1 OFFSET ?2)",
            params![browser_id, MAX_SESSIONS_PER_BROWSER],
        )?;
        Ok(())
    }

    /// List a bounded, metadata-only page in stable `(last_seen_at,id)` order.
    fn sessions(&self, cursor: Option<&str>, limit: Option<usize>) -> Result<SessionPage> {
        let limit = limit.unwrap_or(DEFAULT_SESSION_PAGE_SIZE);
        if limit == 0 || limit > MAX_SESSION_PAGE_SIZE {
            return Err(BrowserError::InvalidRequest(format!(
                "session page limit must be between 1 and {MAX_SESSION_PAGE_SIZE}"
            )));
        }
        let (cursor_seen, cursor_id) = cursor
            .map(decode_session_cursor)
            .transpose()?
            .unwrap_or((i64::MAX, String::from("\u{10ffff}")));
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id,browser_id,tab_id,document_id,origin,sanitized_path,page_title,catalog_revision,catalog_fingerprint,json_array_length(catalog_json),enabled,status,last_seen_at FROM document_sessions WHERE (last_seen_at < ?1 OR (last_seen_at = ?1 AND id < ?2)) ORDER BY last_seen_at DESC,id DESC LIMIT ?3",
        )?;
        let mut sessions = statement
            .query_map(
                params![
                    cursor_seen,
                    cursor_id,
                    i64::try_from(limit + 1).unwrap_or(i64::MAX)
                ],
                map_session_summary,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next_cursor = if sessions.len() > limit {
            sessions.truncate(limit);
            sessions
                .last()
                .map(|session| encode_session_cursor(session.last_seen_at, &session.id))
        } else {
            None
        };
        Ok(SessionPage {
            sessions,
            next_cursor,
        })
    }

    /// Fetch one exact session including its bounded catalog.
    fn session(&self, id: &str) -> Result<DocumentSession> {
        self.lock()?.query_row(
            "SELECT id,browser_id,tab_id,document_id,origin,sanitized_path,page_title,catalog_revision,catalog_fingerprint,catalog_json,enabled,status,last_seen_at FROM document_sessions WHERE id=?1",
            [id], map_session,
        ).optional()?.ok_or(BrowserError::NotFound)
    }

    /// Explicitly enable or disable calls for one immutable document session.
    fn set_session_enabled(&self, session_id: &str, enabled: bool) -> Result<DocumentSession> {
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE document_sessions SET enabled=?1 WHERE id=?2 AND status='active'",
            params![enabled, session_id],
        )?;
        if changed != 1 {
            return Err(BrowserError::NotFound);
        }
        connection
            .query_row(
                "SELECT id,browser_id,tab_id,document_id,origin,sanitized_path,page_title,catalog_revision,catalog_fingerprint,catalog_json,enabled,status,last_seen_at FROM document_sessions WHERE id=?1",
                [session_id],
                map_session,
            )
            .map_err(Into::into)
    }

    /// Close an exact document without affecting another navigation in the same tab.
    fn close_document(&self, browser_id: &str, tab_id: i64, document_id: &str) -> Result<()> {
        self.lock()?.execute(
            "UPDATE document_sessions SET status='closed',enabled=0,last_seen_at=?1 WHERE browser_id=?2 AND tab_id=?3 AND document_id=?4",
            params![now_seconds()?, browser_id, tab_id, document_id],
        )?;
        Ok(())
    }

    /// Fail closed unless a call targets an enabled active session and an observed tool.
    fn validate_call(
        &self,
        browser_id: &str,
        tab_id: i64,
        document_id: &str,
        catalog_revision: i64,
        tool_name: &str,
    ) -> Result<String> {
        let session = self.lock()?.query_row(
            "SELECT ds.id,ds.browser_id,ds.tab_id,ds.document_id,ds.origin,ds.sanitized_path,ds.page_title,ds.catalog_revision,ds.catalog_fingerprint,ds.catalog_json,ds.enabled,ds.status,ds.last_seen_at FROM document_sessions ds JOIN browsers b ON b.id=ds.browser_id WHERE ds.browser_id=?1 AND ds.tab_id=?2 AND ds.document_id=?3 AND b.revoked_at IS NULL",
            params![browser_id, tab_id, document_id],
            map_session,
        ).optional()?.ok_or(BrowserError::StaleDocument)?;
        if !session.enabled
            || session.status != "active"
            || session.catalog_revision != catalog_revision
        {
            return Err(BrowserError::StaleDocument);
        }
        if !session.tools.iter().any(|tool| tool.name == tool_name) {
            return Err(BrowserError::InvalidRequest(
                "tool is not present in the enabled observed catalog".to_string(),
            ));
        }
        Ok(session.catalog_fingerprint)
    }

    /// Persist the redacted start of an accepted invocation.
    pub(crate) fn begin_invocation(
        &self,
        browser_id: &str,
        tab_id: i64,
        document_id: &str,
        tool_name: &str,
        catalog_revision: i64,
    ) -> Result<String> {
        let connection = self.lock()?;
        let session_id: Option<String> = connection
            .query_row(
                "SELECT id FROM document_sessions WHERE browser_id=?1 AND tab_id=?2 AND document_id=?3",
                params![browser_id, tab_id, document_id],
                |row| row.get(0),
            )
            .optional()?;
        let id = Uuid::new_v4().to_string();
        connection.execute(
            "INSERT INTO invocation_audits(id,browser_id,session_id,tool_name,catalog_revision,outcome,error_kind,duration_ms,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![id, browser_id, session_id, tool_name, catalog_revision, "started", Option::<String>::None, 0, now_seconds()?],
        )?;
        Ok(id)
    }

    /// Finish a previously accepted invocation without persisting arguments or results.
    fn finish_invocation_parts(
        &self,
        id: &str,
        outcome: &str,
        error_kind: Option<&str>,
        duration_ms: i64,
    ) -> Result<()> {
        self.lock()?.execute(
            "UPDATE invocation_audits SET outcome=?1,error_kind=?2,duration_ms=?3 WHERE id=?4 AND outcome='started'",
            params![outcome, error_kind, duration_ms, id],
        )?;
        Ok(())
    }

    /// Mark a runtime-dropped invocation abandoned.
    pub(crate) fn abandon_invocation(&self, id: &str, duration_ms: i64) -> Result<()> {
        self.lock()?.execute(
            "UPDATE invocation_audits SET outcome='abandoned',error_kind='caller_cancelled',duration_ms=?1 WHERE id=?2 AND outcome='started'",
            params![duration_ms, id],
        )?;
        Ok(())
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| BrowserError::InvalidRequest("browser store lock poisoned".to_string()))
    }
}

fn now_seconds() -> Result<i64> {
    Ok(Timestamp::now().as_second())
}

fn validate_pairing_input(display_name: &str, extension_id: &str, public_key: &[u8]) -> Result<()> {
    if display_name.trim().is_empty() || display_name.chars().count() > 80 {
        return Err(BrowserError::InvalidRequest(
            "invalid display_name".to_string(),
        ));
    }
    if extension_id.len() != 32
        || !extension_id
            .bytes()
            .all(|byte| (b'a'..=b'p').contains(&byte))
    {
        return Err(BrowserError::InvalidRequest(
            "invalid Chrome extension id".to_string(),
        ));
    }
    if public_key.len() != 32 {
        return Err(BrowserError::InvalidRequest(
            "invalid Ed25519 public key".to_string(),
        ));
    }
    Ok(())
}

fn validate_observation(observation: &CatalogObservation) -> Result<()> {
    if observation.tab_id < 0
        || observation.document_id.is_empty()
        || observation.catalog_revision < 1
    {
        return Err(BrowserError::InvalidRequest(
            "invalid document identity".to_string(),
        ));
    }
    if observation.tools.len() > 64 {
        return Err(BrowserError::InvalidRequest(
            "catalog exceeds 64 tools".to_string(),
        ));
    }
    if observation.document_id.len() > 256
        || observation.origin.len() > 2_048
        || observation.sanitized_path.len() > 2_048
        || observation.page_title.len() > 512
        || observation.catalog_fingerprint.len() > MAX_CATALOG_BYTES
        || observation.tools.iter().any(|tool| {
            tool.name.is_empty()
                || tool.name.len() > 256
                || tool.description.len() > 8_192
                || json_depth(&tool.input_schema) > MAX_JSON_DEPTH
                || json_depth(&tool.annotations) > MAX_JSON_DEPTH
        })
        || serde_json::to_vec(observation).is_ok_and(|encoded| encoded.len() > MAX_CATALOG_BYTES)
    {
        return Err(BrowserError::InvalidRequest(
            "catalog metadata exceeds protocol bounds".to_string(),
        ));
    }
    if !observation.origin.starts_with("http://") && !observation.origin.starts_with("https://") {
        return Err(BrowserError::InvalidRequest(
            "invalid observed origin".to_string(),
        ));
    }
    Ok(())
}

fn json_depth(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        serde_json::Value::Object(values) => 1 + values.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
}

fn pairing_row(connection: &Connection, id: &str) -> Result<Option<PairingRequest>> {
    connection
        .query_row(
            "SELECT id,display_name,extension_id,public_key,status,expires_at,browser_id FROM browser_pairing_requests WHERE id=?1",
            [id],
            map_pairing,
        )
        .optional()
        .map_err(Into::into)
}

fn map_pairing(row: &rusqlite::Row<'_>) -> rusqlite::Result<PairingRequest> {
    let status: String = row.get(4)?;
    let status = PairingStatus::parse(&status).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(PairingRequest {
        id: row.get(0)?,
        display_name: row.get(1)?,
        extension_id: row.get(2)?,
        public_key: row.get(3)?,
        status,
        expires_at: row.get(5)?,
        browser_id: row.get(6)?,
    })
}

fn map_browser(row: &rusqlite::Row<'_>) -> rusqlite::Result<BrowserRecord> {
    Ok(BrowserRecord {
        id: row.get(0)?,
        display_name: row.get(1)?,
        extension_id: row.get(2)?,
        public_key: row.get(3)?,
        paired_at: row.get(4)?,
        last_seen_at: row.get(5)?,
        revoked_at: row.get(6)?,
    })
}

fn map_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<DocumentSession> {
    let catalog: String = row.get(9)?;
    let tools = serde_json::from_str(&catalog).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(DocumentSession {
        id: row.get(0)?,
        browser_id: row.get(1)?,
        tab_id: row.get(2)?,
        document_id: row.get(3)?,
        origin: row.get(4)?,
        sanitized_path: row.get(5)?,
        page_title: row.get(6)?,
        catalog_revision: row.get(7)?,
        catalog_fingerprint: row.get(8)?,
        tools,
        enabled: row.get(10)?,
        status: row.get(11)?,
        last_seen_at: row.get(12)?,
    })
}

fn map_session_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<DocumentSessionSummary> {
    Ok(DocumentSessionSummary {
        id: row.get(0)?,
        browser_id: row.get(1)?,
        tab_id: row.get(2)?,
        document_id: row.get(3)?,
        origin: row.get(4)?,
        sanitized_path: row.get(5)?,
        page_title: row.get(6)?,
        catalog_revision: row.get(7)?,
        catalog_fingerprint: row.get(8)?,
        tool_count: usize::try_from(row.get::<_, i64>(9)?).unwrap_or(usize::MAX),
        enabled: row.get(10)?,
        status: row.get(11)?,
        last_seen_at: row.get(12)?,
    })
}

fn encode_session_cursor(last_seen_at: i64, id: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{last_seen_at}\n{id}"))
}

fn decode_session_cursor(cursor: &str) -> Result<(i64, String)> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| BrowserError::InvalidRequest("invalid session cursor".to_string()))?;
    let value = String::from_utf8(bytes)
        .map_err(|_| BrowserError::InvalidRequest("invalid session cursor".to_string()))?;
    let (seen, id) = value
        .split_once('\n')
        .ok_or_else(|| BrowserError::InvalidRequest("invalid session cursor".to_string()))?;
    let seen = seen
        .parse()
        .map_err(|_| BrowserError::InvalidRequest("invalid session cursor".to_string()))?;
    if id.is_empty() {
        return Err(BrowserError::InvalidRequest(
            "invalid session cursor".to_string(),
        ));
    }
    Ok((seen, id.to_string()))
}

fn migrate(connection: &Connection) -> Result<()> {
    connection.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<()> {
        connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS browser_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        INSERT INTO browser_meta(key,value) VALUES('schema_version','0') ON CONFLICT(key) DO NOTHING;
        "
    )?;
        let version_text: String = connection.query_row(
            "SELECT value FROM browser_meta WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )?;
        let version: i64 = version_text.parse().map_err(|_| {
            BrowserError::InvalidRequest(
                "browser database has an invalid schema version".to_string(),
            )
        })?;
        if version > CURRENT_SCHEMA_VERSION {
            return Err(BrowserError::InvalidRequest(format!(
                "browser database schema version {version} is newer than supported version {CURRENT_SCHEMA_VERSION}"
            )));
        }
        if version < 0 {
            return Err(BrowserError::InvalidRequest(
                "browser database has an invalid schema version".to_string(),
            ));
        }
        if version == 0 {
            connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS browsers(
          id TEXT PRIMARY KEY,display_name TEXT NOT NULL,extension_id TEXT NOT NULL,
          public_key BLOB NOT NULL,paired_at INTEGER NOT NULL,last_seen_at INTEGER,revoked_at INTEGER
        );
        CREATE UNIQUE INDEX IF NOT EXISTS browsers_active_extension ON browsers(extension_id) WHERE revoked_at IS NULL;
        CREATE TABLE IF NOT EXISTS browser_pairing_requests(
          id TEXT PRIMARY KEY,display_name TEXT NOT NULL,extension_id TEXT NOT NULL,
          public_key BLOB NOT NULL,status TEXT NOT NULL CHECK(status IN ('pending','approved','rejected','expired')),
          expires_at INTEGER NOT NULL,resolved_at INTEGER,browser_id TEXT REFERENCES browsers(id),created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS browser_pairing_status_expiry ON browser_pairing_requests(status,expires_at);
        CREATE UNIQUE INDEX IF NOT EXISTS browser_pairing_active_extension ON browser_pairing_requests(extension_id) WHERE status='pending';
        CREATE TABLE IF NOT EXISTS browser_auth_challenges(
          id TEXT PRIMARY KEY,browser_id TEXT NOT NULL REFERENCES browsers(id) ON DELETE CASCADE,
          nonce BLOB NOT NULL,expires_at INTEGER NOT NULL,used_at INTEGER,created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS document_sessions(
          id TEXT PRIMARY KEY,browser_id TEXT NOT NULL REFERENCES browsers(id) ON DELETE CASCADE,
          tab_id INTEGER NOT NULL,document_id TEXT NOT NULL,origin TEXT NOT NULL,sanitized_path TEXT NOT NULL,
          page_title TEXT NOT NULL,catalog_revision INTEGER NOT NULL,catalog_fingerprint TEXT NOT NULL,
          catalog_json TEXT NOT NULL,status TEXT NOT NULL CHECK(status IN ('active','replaced','closed')),
          enabled INTEGER NOT NULL DEFAULT 0,connected_at INTEGER NOT NULL,last_seen_at INTEGER NOT NULL,
          UNIQUE(browser_id,tab_id,document_id)
        );
        CREATE TABLE IF NOT EXISTS invocation_audits(
          id TEXT PRIMARY KEY,browser_id TEXT,session_id TEXT,tool_name TEXT NOT NULL,
          catalog_revision INTEGER NOT NULL,outcome TEXT NOT NULL CHECK(outcome IN ('started','succeeded','failed','abandoned')),
          error_kind TEXT,duration_ms INTEGER NOT NULL,created_at INTEGER NOT NULL
        );
        ",
        )?;
            connection.execute(
                "UPDATE browser_meta SET value=?1 WHERE key='schema_version'",
                [1_i64],
            )?;
        }
        if version <= 1 {
            connection.execute_batch(
            "CREATE INDEX IF NOT EXISTS document_sessions_page ON document_sessions(last_seen_at DESC,id DESC);",
        )?;
            connection.execute(
                "UPDATE browser_meta SET value=?1 WHERE key='schema_version'",
                [CURRENT_SCHEMA_VERSION],
            )?;
        }
        connection.execute(
        "UPDATE invocation_audits SET outcome='abandoned',error_kind='process_restarted' WHERE outcome='started'",
        [],
    )?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            connection.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(error) => {
            drop(connection.execute_batch("ROLLBACK"));
            Err(error)
        }
    }
}

/// Decode a base64url public key for pairing adapters.
pub(crate) fn decode_public_key(value: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| BrowserError::InvalidRequest("invalid public_key encoding".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extension_id() -> &'static str {
        "abcdefghijklmnopabcdefghijklmnop"
    }

    #[test]
    fn pairing_is_single_use_and_creates_browser() {
        let store = BlockingStore::memory().unwrap();
        let request = store
            .request_pairing("Chrome", extension_id(), vec![7; 32])
            .unwrap();
        assert_eq!(request.status, PairingStatus::Pending);
        let browser = store.approve_pairing(&request.id).unwrap();
        assert_eq!(browser.extension_id, extension_id());
        assert!(store.approve_pairing(&request.id).is_err());
    }

    #[test]
    fn rejects_oversized_catalog() {
        let store = BlockingStore::memory().unwrap();
        let request = store
            .request_pairing("Chrome", extension_id(), vec![7; 32])
            .unwrap();
        let browser = store.approve_pairing(&request.id).unwrap();
        let observation = CatalogObservation {
            tab_id: 1,
            document_id: "doc".into(),
            origin: "https://example.com".into(),
            sanitized_path: "/".into(),
            page_title: "Example".into(),
            catalog_revision: 1,
            catalog_fingerprint: "fingerprint".into(),
            tools: (0..65)
                .map(|index| crate::protocol::ToolDescriptor {
                    name: format!("tool-{index}"),
                    description: String::new(),
                    input_schema: serde_json::json!({"type":"object"}),
                    annotations: serde_json::Value::Null,
                })
                .collect(),
        };
        assert_eq!(
            store.observe(&browser.id, &observation).unwrap_err().kind(),
            "invalid_request"
        );
        let mut oversized = observation;
        oversized.tools.truncate(1);
        oversized.tools[0].description = "x".repeat(8_193);
        assert_eq!(
            store.observe(&browser.id, &oversized).unwrap_err().kind(),
            "invalid_request"
        );
    }

    #[test]
    fn revocation_closes_and_disables_active_sessions() {
        let store = BlockingStore::memory().unwrap();
        let request = store
            .request_pairing("Chrome", extension_id(), vec![7; 32])
            .unwrap();
        let browser = store.approve_pairing(&request.id).unwrap();
        store
            .observe(
                &browser.id,
                &CatalogObservation {
                    tab_id: 1,
                    document_id: "doc".into(),
                    origin: "https://example.com".into(),
                    sanitized_path: "/".into(),
                    page_title: "Example".into(),
                    catalog_revision: 1,
                    catalog_fingerprint: "hash".into(),
                    tools: vec![crate::protocol::ToolDescriptor {
                        name: "search".into(),
                        description: String::new(),
                        input_schema: serde_json::json!({"type":"object"}),
                        annotations: serde_json::Value::Null,
                    }],
                },
            )
            .unwrap();
        let session = store.sessions(None, None).unwrap().sessions.remove(0);
        store.set_session_enabled(&session.id, true).unwrap();

        let revoked = store.revoke_browser(&browser.id).unwrap();
        assert!(revoked.revoked_at.is_some());
        let session = store.sessions(None, None).unwrap().sessions.remove(0);
        assert!(!session.enabled);
        assert_eq!(session.status, "closed");
    }

    #[test]
    fn catalog_change_revokes_exact_session_consent() {
        let store = BlockingStore::memory().unwrap();
        let request = store
            .request_pairing("Chrome", extension_id(), vec![7; 32])
            .unwrap();
        let browser = store.approve_pairing(&request.id).unwrap();
        let mut observation = CatalogObservation {
            tab_id: 1,
            document_id: "doc".into(),
            origin: "https://example.com".into(),
            sanitized_path: "/".into(),
            page_title: "Example".into(),
            catalog_revision: 1,
            catalog_fingerprint: "one".into(),
            tools: vec![crate::protocol::ToolDescriptor {
                name: "search".into(),
                description: String::new(),
                input_schema: serde_json::json!({"type":"object"}),
                annotations: serde_json::Value::Null,
            }],
        };
        store.observe(&browser.id, &observation).unwrap();
        let session = store.sessions(None, None).unwrap().sessions.remove(0);
        store.set_session_enabled(&session.id, true).unwrap();
        observation.catalog_revision = 2;
        observation.catalog_fingerprint = "two".into();
        store.observe(&browser.id, &observation).unwrap();
        let changed = store.sessions(None, None).unwrap().sessions.remove(0);
        assert!(!changed.enabled);
        assert_eq!(
            store
                .validate_call(&browser.id, 1, "doc", 2, "search")
                .unwrap_err()
                .kind(),
            "stale_document"
        );
    }

    #[test]
    fn durable_identity_and_revocation_survive_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("browser.sqlite3");
        let browser_id = {
            let store = BlockingStore::open(path.clone()).unwrap();
            let request = store
                .request_pairing("Chrome", extension_id(), vec![7; 32])
                .unwrap();
            let browser = store.approve_pairing(&request.id).unwrap();
            store.revoke_browser(&browser.id).unwrap();
            browser.id
        };
        let reopened = BlockingStore::open(path).unwrap();
        assert!(
            reopened
                .browser(&browser_id)
                .unwrap()
                .unwrap()
                .revoked_at
                .is_some()
        );
        assert!(reopened.pending_pairings().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_store_contention_does_not_starve_tokio_workers() {
        let store = Store::memory().await.unwrap();
        assert_eq!(store.permits.available_permits(), 1);
        let inner = Arc::clone(&store.inner);
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _connection = inner.connection.lock().unwrap();
            locked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        locked_rx.recv().unwrap();
        let blocked_query = tokio::spawn({
            let store = store.clone();
            async move { store.browsers().await }
        });
        tokio::time::timeout(std::time::Duration::from_millis(100), async {
            while store.permits.available_permits() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_millis(100), async {
            tokio::task::yield_now().await;
        })
        .await
        .unwrap();
        release_tx.send(()).unwrap();
        blocker.await.unwrap();
        blocked_query.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn rejects_future_schema_without_changing_marker() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("browser.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE browser_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);\
             INSERT INTO browser_meta VALUES('schema_version','999');",
            )
            .unwrap();
        drop(connection);
        let opened = Store::open(&path).await;
        assert!(opened.is_err(), "future schema unexpectedly opened");
        let error = opened.err().unwrap();
        assert!(error.to_string().contains("newer than supported"));
        let marker: String = Connection::open(path)
            .unwrap()
            .query_row(
                "SELECT value FROM browser_meta WHERE key='schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marker, "999");
    }

    #[tokio::test]
    async fn migrates_version_one_fixture_transactionally() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("browser.sqlite3");
        drop(Store::open(&path).await.unwrap());
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "DROP INDEX document_sessions_page;\
             UPDATE browser_meta SET value='1' WHERE key='schema_version';",
            )
            .unwrap();
        drop(connection);
        drop(Store::open(&path).await.unwrap());
        let connection = Connection::open(path).unwrap();
        let marker: String = connection
            .query_row(
                "SELECT value FROM browser_meta WHERE key='schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let index_count: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='document_sessions_page'",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(marker, CURRENT_SCHEMA_VERSION.to_string());
        assert_eq!(index_count, 1);
    }

    #[tokio::test]
    async fn session_pages_are_bounded_stable_summaries_with_exact_detail() {
        let store = Store::memory().await.unwrap();
        let pairing = store
            .request_pairing("Chrome", extension_id(), vec![7; 32])
            .await
            .unwrap();
        let browser = store.approve_pairing(&pairing.id).await.unwrap();
        for tab_id in 0..5 {
            store
                .observe(
                    &browser.id,
                    &CatalogObservation {
                        tab_id,
                        document_id: format!("doc-{tab_id}"),
                        origin: "https://example.com".into(),
                        sanitized_path: "/".into(),
                        page_title: "Example".into(),
                        catalog_revision: 1,
                        catalog_fingerprint: format!("hash-{tab_id}"),
                        tools: vec![crate::protocol::ToolDescriptor {
                            name: "search".into(),
                            description: "large detail".repeat(100),
                            input_schema: serde_json::json!({"type":"object"}),
                            annotations: serde_json::Value::Null,
                        }],
                    },
                )
                .await
                .unwrap();
        }
        let first = store.sessions(None, Some(2)).await.unwrap();
        assert_eq!(first.sessions.len(), 2);
        assert!(first.next_cursor.is_some());
        assert!(serde_json::to_vec(&first).unwrap().len() < 4096);
        let second = store
            .sessions(first.next_cursor.as_deref(), Some(2))
            .await
            .unwrap();
        assert_eq!(second.sessions.len(), 2);
        assert!(
            first
                .sessions
                .iter()
                .all(|left| second.sessions.iter().all(|right| left.id != right.id))
        );
        let detail = store.session(&first.sessions[0].id).await.unwrap();
        assert_eq!(detail.tools.len(), 1);
    }
}
