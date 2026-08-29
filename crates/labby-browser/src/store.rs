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

/// Cloneable SQLite store with serialized access and WAL persistence.
#[derive(Clone)]
pub struct Store {
    path: Arc<PathBuf>,
    connection: Arc<Mutex<Connection>>,
}

impl Store {
    /// Open a database and apply the idempotent browser schema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
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
            path: Arc::new(path),
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Open an in-memory store for tests.
    pub fn memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&connection)?;
        Ok(Self {
            path: Arc::new(PathBuf::from(":memory:")),
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Database path for diagnostics.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    /// Create or refresh one pending pairing request.
    pub fn request_pairing(
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
    pub fn pairing(&self, id: &str) -> Result<Option<PairingRequest>> {
        let now = now_seconds()?;
        let connection = self.lock()?;
        connection.execute(
            "UPDATE browser_pairing_requests SET status='expired', resolved_at=?1 WHERE id=?2 AND status='pending' AND expires_at<=?1",
            params![now, id],
        )?;
        pairing_row(&connection, id)
    }

    /// List pending pairing requests for operator approval.
    pub fn pending_pairings(&self) -> Result<Vec<PairingRequest>> {
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
    pub fn approve_pairing(&self, id: &str) -> Result<BrowserRecord> {
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
    pub fn browser(&self, id: &str) -> Result<Option<BrowserRecord>> {
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
    pub fn browsers(&self) -> Result<Vec<BrowserRecord>> {
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
    pub fn revoke_browser(&self, id: &str) -> Result<BrowserRecord> {
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
    pub fn observe(&self, browser_id: &str, observation: &CatalogObservation) -> Result<()> {
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

    /// List sanitized document sessions. Executable page callbacks never enter SQLite.
    pub fn sessions(&self) -> Result<Vec<DocumentSession>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id,browser_id,tab_id,document_id,origin,sanitized_path,page_title,catalog_revision,catalog_fingerprint,catalog_json,enabled,status,last_seen_at FROM document_sessions ORDER BY last_seen_at DESC",
        )?;
        statement
            .query_map([], map_session)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Explicitly enable or disable calls for one immutable document session.
    pub fn set_session_enabled(&self, session_id: &str, enabled: bool) -> Result<DocumentSession> {
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
    pub fn close_document(&self, browser_id: &str, tab_id: i64, document_id: &str) -> Result<()> {
        self.lock()?.execute(
            "UPDATE document_sessions SET status='closed',enabled=0,last_seen_at=?1 WHERE browser_id=?2 AND tab_id=?3 AND document_id=?4",
            params![now_seconds()?, browser_id, tab_id, document_id],
        )?;
        Ok(())
    }

    /// Fail closed unless a call targets an enabled active session and an observed tool.
    pub fn validate_call(
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
    pub(crate) fn finish_invocation(
        &self,
        id: &str,
        result: &Result<serde_json::Value>,
        duration_ms: i64,
    ) -> Result<()> {
        let (outcome, error_kind) = match result {
            Ok(_) => ("succeeded", None),
            Err(error) => ("failed", Some(error.kind())),
        };
        self.lock()?.execute(
            "UPDATE invocation_audits SET outcome=?1,error_kind=?2,duration_ms=?3 WHERE id=?4 AND outcome='started'",
            params![outcome, error_kind, duration_ms, id],
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

fn migrate(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS browser_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        INSERT INTO browser_meta(key,value) VALUES('schema_version','1') ON CONFLICT(key) DO NOTHING;
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
        "UPDATE invocation_audits SET outcome='abandoned',error_kind='process_restarted' WHERE outcome='started'",
        [],
    )?;
    Ok(())
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
        let store = Store::memory().unwrap();
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
        let store = Store::memory().unwrap();
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
        let store = Store::memory().unwrap();
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
        let session = store.sessions().unwrap().remove(0);
        store.set_session_enabled(&session.id, true).unwrap();

        let revoked = store.revoke_browser(&browser.id).unwrap();
        assert!(revoked.revoked_at.is_some());
        let session = store.sessions().unwrap().remove(0);
        assert!(!session.enabled);
        assert_eq!(session.status, "closed");
    }

    #[test]
    fn catalog_change_revokes_exact_session_consent() {
        let store = Store::memory().unwrap();
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
        let session = store.sessions().unwrap().remove(0);
        store.set_session_enabled(&session.id, true).unwrap();
        observation.catalog_revision = 2;
        observation.catalog_fingerprint = "two".into();
        store.observe(&browser.id, &observation).unwrap();
        let changed = store.sessions().unwrap().remove(0);
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
            let store = Store::open(&path).unwrap();
            let request = store
                .request_pairing("Chrome", extension_id(), vec![7; 32])
                .unwrap();
            let browser = store.approve_pairing(&request.id).unwrap();
            store.revoke_browser(&browser.id).unwrap();
            browser.id
        };
        let reopened = Store::open(&path).unwrap();
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
}
