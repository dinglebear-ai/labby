//! Versioned auth-store schema migrations.

use rusqlite::{Connection, OptionalExtension, params};
use tracing::{info, warn};

use super::{add_column_if_missing, hash_token, sqlite_error};
use crate::error::AuthError;

pub(super) fn run_migrations(conn: &Connection) -> Result<(), AuthError> {
    run_migrations_inner(conn, None)
}

#[cfg(test)]
pub(super) fn run_migrations_with_fault(conn: &Connection, fault: &str) -> Result<(), AuthError> {
    run_migrations_inner(conn, Some(fault))
}

fn run_migrations_inner(conn: &Connection, fault: Option<&str>) -> Result<(), AuthError> {
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
        migrate_v4(conn)?;
    }
    if current < 5 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS assertion_jtis (
               issuer TEXT NOT NULL,
               jti TEXT NOT NULL,
               expires_at INTEGER NOT NULL,
               PRIMARY KEY (issuer, jti)
             );
             CREATE INDEX IF NOT EXISTS idx_assertion_jtis_expiry
               ON assertion_jtis(expires_at);
             PRAGMA user_version = 5;",
        )
        .map_err(sqlite_error)?;
    }
    if current < 6 {
        add_column_if_missing(conn, "refresh_tokens", "refresh_claim_id", "TEXT")?;
        add_column_if_missing(
            conn,
            "refresh_tokens",
            "refresh_claim_expires_at",
            "INTEGER",
        )?;
        conn.execute_batch("PRAGMA user_version = 6;")
            .map_err(sqlite_error)?;
    }
    if current < 7 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS google_provider_credentials (
               subject TEXT PRIMARY KEY,
               email TEXT,
               refresh_token TEXT NOT NULL,
               generation INTEGER NOT NULL,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_google_provider_credentials_email
               ON google_provider_credentials(email COLLATE NOCASE);
             INSERT OR IGNORE INTO google_provider_credentials (
               subject, email, refresh_token, generation, created_at, updated_at
             )
             SELECT newest.subject, NULL, newest.provider_refresh_token, 1,
                    newest.created_at, newest.created_at
             FROM refresh_tokens AS newest
             WHERE newest.provider_refresh_token IS NOT NULL
               AND newest.created_at = (
                 SELECT MAX(candidate.created_at)
                 FROM refresh_tokens AS candidate
                 WHERE candidate.subject = newest.subject
                   AND candidate.provider_refresh_token IS NOT NULL
               )
               AND 1 = (
                 SELECT COUNT(DISTINCT candidate.provider_refresh_token)
                 FROM refresh_tokens AS candidate
                 WHERE candidate.subject = newest.subject
                   AND candidate.provider_refresh_token IS NOT NULL
                   AND candidate.created_at = newest.created_at
               )
             GROUP BY newest.subject;
             PRAGMA user_version = 7;",
        )
        .map_err(sqlite_error)?;
    }
    if current < 8 {
        add_column_if_missing(
            conn,
            "google_provider_credentials",
            "client_id",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        add_column_if_missing(
            conn,
            "google_provider_credentials",
            "granted_scopes_json",
            "TEXT NOT NULL DEFAULT '[\"email\",\"openid\",\"profile\"]'",
        )?;
        add_column_if_missing(conn, "google_provider_credentials", "access_token", "TEXT")?;
        add_column_if_missing(
            conn,
            "google_provider_credentials",
            "token_received_at",
            "INTEGER",
        )?;
        add_column_if_missing(
            conn,
            "google_provider_credentials",
            "access_token_expires_at",
            "INTEGER",
        )?;
        add_column_if_missing(conn, "google_provider_credentials", "issuer", "TEXT")?;
        add_column_if_missing(
            conn,
            "google_provider_credentials",
            "last_refresh_at",
            "INTEGER",
        )?;
        add_column_if_missing(
            conn,
            "google_provider_credentials",
            "last_scope_upgrade_at",
            "INTEGER",
        )?;
        conn.execute_batch("PRAGMA user_version = 8;")
            .map_err(sqlite_error)?;
    }
    if current == 8 {
        repair_falsely_stamped_v8(conn)?;
    }
    if current < 9 {
        let transaction = conn.unchecked_transaction().map_err(sqlite_error)?;
        transaction
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS google_provider_revocations (
                   subject TEXT PRIMARY KEY,
                   epoch INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL
                 );",
            )
            .map_err(sqlite_error)?;
        if fault == Some("v9_after_table") {
            return Err(AuthError::Storage(
                "injected v9 migration fault".to_string(),
            ));
        }
        transaction
            .execute_batch("PRAGMA user_version = 9;")
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;
    }
    if current < 10 {
        let transaction = conn.unchecked_transaction().map_err(sqlite_error)?;
        add_column_if_missing(
            &transaction,
            "authorization_requests",
            "native_poll_token_hash",
            "TEXT",
        )?;
        if fault == Some("v10_after_column") {
            return Err(AuthError::Storage(
                "injected v10 migration fault".to_string(),
            ));
        }
        transaction
            .execute_batch(
                "DROP TABLE IF EXISTS native_authorization_results;
                 CREATE TABLE native_authorization_results (
                   poll_token_hash TEXT PRIMARY KEY,
                   code TEXT NOT NULL,
                   created_at INTEGER NOT NULL,
                   expires_at INTEGER NOT NULL
                 );",
            )
            .map_err(sqlite_error)?;
        if fault == Some("v10_after_table") {
            return Err(AuthError::Storage(
                "injected v10 migration fault".to_string(),
            ));
        }
        transaction
            .execute_batch("PRAGMA user_version = 10;")
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;
    }
    if current < 11 {
        let transaction = conn.unchecked_transaction().map_err(sqlite_error)?;
        transaction
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS refresh_token_replays (
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
                   ON refresh_token_replays(expires_at);",
            )
            .map_err(sqlite_error)?;
        if fault == Some("v11_after_table") {
            return Err(AuthError::Storage(
                "injected v11 migration fault".to_string(),
            ));
        }
        transaction
            .execute_batch("PRAGMA user_version = 11;")
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;
    }
    if current < 12 {
        let transaction = conn.unchecked_transaction().map_err(sqlite_error)?;
        add_column_if_missing(
            &transaction,
            "upstream_oauth_state",
            "expected_issuer",
            "TEXT",
        )?;
        if fault == Some("v12_after_expected_issuer") {
            return Err(AuthError::Storage(
                "injected v12 migration fault".to_string(),
            ));
        }
        add_column_if_missing(
            &transaction,
            "upstream_oauth_state",
            "require_issuer",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        add_column_if_missing(
            &transaction,
            "upstream_oauth_state",
            "requested_scopes_json",
            "TEXT NOT NULL DEFAULT '[]'",
        )?;
        transaction
            .execute_batch("PRAGMA user_version = 12;")
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;
    }
    if current < 12 {
        add_column_if_missing(conn, "browser_sessions", "project_binding_json", "TEXT")?;
        conn.execute_batch("PRAGMA user_version = 12;")
            .map_err(sqlite_error)?;
    }
    // Repair databases that were stamped v12 by the initial project-session
    // migration even if the binding column was not durably installed.
    add_column_if_missing(conn, "browser_sessions", "project_binding_json", "TEXT")?;
    if current < 13 {
        let transaction = conn.unchecked_transaction().map_err(sqlite_error)?;
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS reauth_proofs (
               nonce_hash BLOB PRIMARY KEY NOT NULL CHECK(length(nonce_hash) = 32),
               actor BLOB NOT NULL CHECK(length(actor) = 32),
               session BLOB NOT NULL CHECK(length(session) = 32),
               authority BLOB NOT NULL CHECK(length(authority) = 32),
               purpose BLOB NOT NULL CHECK(length(purpose) = 32),
               operation_id TEXT NOT NULL,
               authenticated_at INTEGER NOT NULL,
               expires_at INTEGER NOT NULL,
               state INTEGER NOT NULL DEFAULT 0 CHECK(state BETWEEN 0 AND 3)
             );
             CREATE INDEX IF NOT EXISTS reauth_proofs_session ON reauth_proofs(session);
             CREATE TABLE IF NOT EXISTS reauth_attempts (kind TEXT NOT NULL CHECK(kind IN ('issue', 'verify')), actor BLOB NOT NULL CHECK(length(actor) = 32), at INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS reauth_attempts_window ON reauth_attempts(kind, at);
             PRAGMA user_version = 13;"
        ).map_err(sqlite_error)?;
        if fault == Some("v13_before_commit") {
            return Err(AuthError::Storage(
                "injected v13 migration fault".to_string(),
            ));
        }
        transaction.commit().map_err(sqlite_error)?;
    }
    if current < 14 {
        let transaction = conn.unchecked_transaction().map_err(sqlite_error)?;
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS browser_reauth_challenges (
               state TEXT PRIMARY KEY NOT NULL,
               interaction_hash BLOB UNIQUE NOT NULL CHECK(length(interaction_hash) = 32),
               session_id TEXT NOT NULL,
               subject TEXT NOT NULL,
               provider_code_verifier TEXT NOT NULL,
               nonce TEXT NOT NULL,
               purpose_json TEXT NOT NULL,
               created_at INTEGER NOT NULL,
               expires_at INTEGER NOT NULL,
               status INTEGER NOT NULL DEFAULT 0 CHECK(status BETWEEN 0 AND 2),
               proof TEXT
             );
             CREATE INDEX IF NOT EXISTS browser_reauth_session ON browser_reauth_challenges(session_id, expires_at);
             PRAGMA user_version = 14;"
        ).map_err(sqlite_error)?;
        if fault == Some("v14_before_commit") {
            return Err(AuthError::Storage(
                "injected v14 migration fault".to_string(),
            ));
        }
        transaction.commit().map_err(sqlite_error)?;
    }
    Ok(())
}

fn repair_falsely_stamped_v8(conn: &Connection) -> Result<(), AuthError> {
    require_columns(
        conn,
        "refresh_tokens",
        &[
            "refresh_token_hash",
            "client_id",
            "subject",
            "resource",
            "scope",
            "provider_refresh_token",
            "created_at",
            "expires_at",
        ],
    )?;
    require_columns(
        conn,
        "google_provider_credentials",
        &[
            "subject",
            "email",
            "refresh_token",
            "generation",
            "created_at",
            "updated_at",
        ],
    )?;
    require_columns(conn, "assertion_jtis", &["issuer", "jti", "expires_at"])?;
    let refresh_missing = missing_columns(
        conn,
        "refresh_tokens",
        &["refresh_claim_id", "refresh_claim_expires_at"],
    )?;
    let broker_missing = missing_columns(
        conn,
        "google_provider_credentials",
        &[
            "client_id",
            "granted_scopes_json",
            "access_token",
            "token_received_at",
            "access_token_expires_at",
            "issuer",
            "last_refresh_at",
            "last_scope_upgrade_at",
        ],
    )?;
    let assertion_jtis_missing = !table_exists(conn, "assertion_jtis")?;
    if refresh_missing.is_empty() && broker_missing.is_empty() && !assertion_jtis_missing {
        return Ok(());
    }

    warn!(
        migration_version = 8,
        phase = "repair_start",
        refresh_missing_count = refresh_missing.len(),
        broker_missing_count = broker_missing.len(),
        assertion_jtis_missing,
        "repairing legacy falsely stamped auth schema"
    );
    let repair_result = (|| {
        let transaction = conn.unchecked_transaction().map_err(sqlite_error)?;
        if assertion_jtis_missing {
            transaction
                .execute_batch(
                    "CREATE TABLE assertion_jtis (
                   issuer TEXT NOT NULL,
                   jti TEXT NOT NULL,
                   expires_at INTEGER NOT NULL,
                   PRIMARY KEY (issuer, jti)
                 );
                 CREATE INDEX IF NOT EXISTS idx_assertion_jtis_expiry
                   ON assertion_jtis(expires_at);",
                )
                .map_err(sqlite_error)?;
        }
        for column in refresh_missing {
            let definition = match column.as_str() {
                "refresh_claim_id" => "TEXT",
                "refresh_claim_expires_at" => "INTEGER",
                _ => {
                    return Err(AuthError::Storage(
                        "unexpected refresh-token schema invariant".into(),
                    ));
                }
            };
            add_column_if_missing(&transaction, "refresh_tokens", &column, definition)?;
        }
        for column in broker_missing {
            let definition = match column.as_str() {
                "client_id" => "TEXT NOT NULL DEFAULT ''",
                "granted_scopes_json" => {
                    "TEXT NOT NULL DEFAULT '[\"email\",\"openid\",\"profile\"]'"
                }
                "access_token" | "issuer" => "TEXT",
                "token_received_at"
                | "access_token_expires_at"
                | "last_refresh_at"
                | "last_scope_upgrade_at" => "INTEGER",
                _ => {
                    return Err(AuthError::Storage(
                        "unexpected broker schema invariant".into(),
                    ));
                }
            };
            add_column_if_missing(
                &transaction,
                "google_provider_credentials",
                &column,
                definition,
            )?;
        }
        transaction.commit().map_err(sqlite_error)
    })();
    if let Err(error) = repair_result {
        warn!(migration_version = 8, phase = "repair_error", kind = "storage", error = %error, "legacy auth schema repair failed");
        return Err(error);
    }
    info!(
        migration_version = 8,
        phase = "repair_finish",
        "repaired legacy falsely stamped auth schema"
    );
    Ok(())
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, AuthError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
    .map_err(sqlite_error)
}

fn missing_columns(
    conn: &Connection,
    table: &str,
    required: &[&str],
) -> Result<Vec<String>, AuthError> {
    if !table_exists(conn, table)? {
        return Err(AuthError::Storage(format!(
            "auth schema v8 is missing required table `{table}`"
        )));
    }
    let escaped = table.replace('"', "\"\"");
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info(\"{escaped}\")"))
        .map_err(sqlite_error)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sqlite_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(sqlite_error)?;
    Ok(required
        .iter()
        .filter(|required| !columns.iter().any(|column| column == **required))
        .map(|column| (*column).to_string())
        .collect())
}

fn require_columns(conn: &Connection, table: &str, required: &[&str]) -> Result<(), AuthError> {
    let missing = missing_columns(conn, table, required)?;
    if missing.is_empty() {
        Ok(())
    } else {
        warn!(
            migration_version = 8,
            phase = "invariant_error",
            table,
            missing_count = missing.len(),
            "auth schema has non-repairable corruption"
        );
        Err(AuthError::Storage(format!(
            "auth schema v8 table `{table}` is missing non-repairable columns: {}",
            missing.join(", ")
        )))
    }
}

pub(super) fn migrate_v4(conn: &Connection) -> Result<(), AuthError> {
    info!(
        migration_version = 4,
        phase = "start",
        "auth schema migration"
    );
    conn.execute_batch("PRAGMA foreign_keys = OFF;")
        .map_err(sqlite_error)?;
    let result = (|| {
        let transaction = conn.unchecked_transaction().map_err(sqlite_error)?;
        transaction
            .execute_batch(
                "CREATE TABLE refresh_tokens_new (
                   refresh_token_hash TEXT PRIMARY KEY,
                   client_id TEXT NOT NULL REFERENCES registered_clients(client_id),
                   subject TEXT NOT NULL, resource TEXT NOT NULL DEFAULT '', scope TEXT NOT NULL,
                   provider_refresh_token TEXT, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL
                 );
                 INSERT INTO refresh_tokens_new SELECT refresh_token_hash, client_id, subject, resource, scope, provider_refresh_token, created_at, expires_at FROM refresh_tokens;
                 DROP TABLE refresh_tokens;
                 ALTER TABLE refresh_tokens_new RENAME TO refresh_tokens;
                 CREATE INDEX IF NOT EXISTS idx_refresh_tokens_client_expiry ON refresh_tokens(client_id, expires_at);
                 PRAGMA user_version = 4;",
            )
            .map_err(sqlite_error)?;
        let violation: Option<(String, i64, String, i64)> = transaction
            .query_row("PRAGMA foreign_key_check", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()
            .map_err(sqlite_error)?;
        if let Some((table, rowid, parent, constraint)) = violation {
            return Err(AuthError::Storage(format!(
                "foreign key validation failed after auth migration v4: table={table} rowid={rowid} parent={parent} constraint={constraint}"
            )));
        }
        transaction.commit().map_err(sqlite_error)
    })();
    let restore_result = conn
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(sqlite_error);
    if let Err(error) = result {
        warn!(migration_version = 4, phase = "error", kind = "foreign_key_or_storage", error = %error, "auth schema migration failed");
        restore_result?;
        return Err(error);
    }
    restore_result?;
    info!(
        migration_version = 4,
        phase = "finish",
        "auth schema migration"
    );
    Ok(())
}
