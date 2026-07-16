//! Versioned auth-store schema migrations.

use rusqlite::{Connection, params};
use tracing::warn;

use super::{SCHEMA_VERSION, add_column_if_missing, hash_token, sqlite_error};
use crate::error::AuthError;

pub(super) fn run_migrations(conn: &Connection) -> Result<(), AuthError> {
    let current: i64 = conn
        .query_row("PRAGMA user_version;", [], |row| row.get(0))
        .map_err(sqlite_error)?;
    if current < 1 {
        let columns: Vec<String> = {
            let mut statement = conn
                .prepare("PRAGMA table_info(refresh_tokens);")
                .map_err(sqlite_error)?;
            statement
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<_>>()
                .map_err(sqlite_error)?
        };
        if !columns.iter().any(|column| column == "refresh_token_hash") {
            conn.execute_batch("ALTER TABLE refresh_tokens ADD COLUMN refresh_token_hash TEXT;")
                .map_err(sqlite_error)?;
            let rows: Vec<String> = {
                let mut statement = conn.prepare("SELECT refresh_token FROM refresh_tokens WHERE refresh_token_hash IS NULL;").map_err(sqlite_error)?;
                statement
                    .query_map([], |row| row.get(0))
                    .map_err(sqlite_error)?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(sqlite_error)?
            };
            for plaintext in rows {
                conn.execute("UPDATE refresh_tokens SET refresh_token_hash = ?1 WHERE refresh_token = ?2 AND refresh_token_hash IS NULL;", params![hash_token(&plaintext), plaintext]).map_err(sqlite_error)?;
            }
            warn!(
                "migration v1: backfilled refresh-token hashes; old plaintext tokens rotate on next use"
            );
        }
        conn.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS idx_refresh_tokens_hash ON refresh_tokens(refresh_token_hash); PRAGMA user_version = 1;").map_err(sqlite_error)?;
    }
    if current < 2 {
        add_column_if_missing(conn, "upstream_oauth_state", "dynamic_client_id", "TEXT")?;
        conn.execute_batch("PRAGMA user_version = 2;")
            .map_err(sqlite_error)?;
    }
    if current < 3 {
        conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_refresh_tokens_client_expiry ON refresh_tokens(client_id, expires_at); PRAGMA user_version = 3;").map_err(sqlite_error)?;
    }
    if current < 4 {
        conn.execute_batch("PRAGMA foreign_keys = OFF;")
            .map_err(sqlite_error)?;
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE refresh_tokens_new (
               refresh_token_hash TEXT PRIMARY KEY,
               client_id TEXT NOT NULL REFERENCES registered_clients(client_id),
               subject TEXT NOT NULL, resource TEXT NOT NULL DEFAULT '', scope TEXT NOT NULL,
               provider_refresh_token TEXT, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL
             );
             INSERT INTO refresh_tokens_new SELECT refresh_token_hash, client_id, subject, resource, scope, provider_refresh_token, created_at, expires_at FROM refresh_tokens;
             DROP TABLE refresh_tokens;
             ALTER TABLE refresh_tokens_new RENAME TO refresh_tokens;
             CREATE INDEX IF NOT EXISTS idx_refresh_tokens_client_expiry ON refresh_tokens(client_id, expires_at);
             COMMIT;"
        ).map_err(sqlite_error)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(sqlite_error)?;
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
            .map_err(sqlite_error)?;
    }
    Ok(())
}
