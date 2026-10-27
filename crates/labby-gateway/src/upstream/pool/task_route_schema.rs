//! SQLite initialization and owner-only file handling for task routing.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, TransactionBehavior};

const SCHEMA_VERSION: i64 = 3;

pub(super) fn open(path: &Path) -> Result<Connection, String> {
    if !path.is_absolute() {
        return Err("task route database path must be absolute and file-backed".to_string());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|_| "cannot create task route database directory".to_string())?;
    }
    // Resolve directory aliases (including macOS /var -> /private/var),
    // but never canonicalize the database leaf: a symlink there is rejected.
    let parent = path
        .parent()
        .ok_or_else(|| "task database parent is missing".to_string())?;
    let name = path
        .file_name()
        .ok_or_else(|| "task database filename is missing".to_string())?;
    let resolved = parent
        .canonicalize()
        .map_err(|_| "cannot resolve task database directory".to_string())?
        .join(name);
    let path = resolved.as_path();
    prepare_file(path)?;
    // NOFOLLOW must apply to SQLite's actual open, not only the preparatory chmod.
    let mut connection =
        Connection::open_with_flags(path, OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW)
            .map_err(sqlite_error)?;
    initialize(&mut connection)?;
    harden(path)?;
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        let sidecar = PathBuf::from(name);
        if sidecar
            .try_exists()
            .map_err(|_| "cannot inspect task route database sidecar".to_string())?
        {
            harden(&sidecar)?;
        }
    }
    Ok(connection)
}

pub(super) fn initialize(connection: &mut Connection) -> Result<(), String> {
    connection
        .busy_timeout(std::time::Duration::from_secs(1))
        .map_err(sqlite_error)?;
    // Version inspection and initialization share one write snapshot. Concurrent
    // first opens cannot insert duplicate metadata rows or see half a schema.
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_error)?;
    let has_meta: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'task_route_meta')",
        [], |row| row.get(0),
    ).map_err(sqlite_error)?;
    if has_meta {
        let (count, version): (i64, Option<i64>) = transaction
            .query_row(
                "SELECT COUNT(*), MIN(schema_version) FROM task_route_meta",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(sqlite_error)?;
        if count != 1 || !matches!(version, Some(1 | 2 | SCHEMA_VERSION)) {
            return Err("unsupported or corrupt task route schema version".to_string());
        }
        // A version stamp is not proof of an intact schema. Validate without DDL
        // so neither a future database nor a damaged current one is rewritten.
        validate_columns(&transaction)?;
        if version == Some(1) {
            transaction
                .execute_batch(
                    "CREATE TABLE task_route_revocations (
                upstream_name TEXT NOT NULL, oauth_subject TEXT, oauth_only INTEGER NOT NULL, prepared INTEGER NOT NULL DEFAULT 0);
                UPDATE task_route_meta SET schema_version = 3;",
                )
                .map_err(sqlite_error)?;
        }
        if version == Some(2) {
            transaction.execute_batch("ALTER TABLE task_route_revocations ADD COLUMN prepared INTEGER NOT NULL DEFAULT 0;
                UPDATE task_route_meta SET schema_version = 3;").map_err(sqlite_error)?;
        }
    } else {
        let tables: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [], |row| row.get(0),
        ).map_err(sqlite_error)?;
        if tables != 0 {
            return Err("unrecognized task route database schema".to_string());
        }
        transaction.execute_batch(
            "CREATE TABLE task_route_meta (schema_version INTEGER NOT NULL);
             INSERT INTO task_route_meta VALUES (3);
             CREATE TABLE task_routes (
                 public_task_id TEXT PRIMARY KEY NOT NULL,
                 native_task_id TEXT NOT NULL, upstream_name TEXT NOT NULL,
                 caller_subject TEXT, oauth_subject TEXT, route_key TEXT NOT NULL,
                 allowed_upstreams_json TEXT, config_fingerprint TEXT NOT NULL,
                 created_at_unix_ms INTEGER NOT NULL CHECK(created_at_unix_ms >= 0),
                 updated_at_unix_ms INTEGER NOT NULL CHECK(updated_at_unix_ms >= created_at_unix_ms),
                 ttl_ms INTEGER CHECK(ttl_ms IS NULL OR ttl_ms >= 0),
                 poll_interval_ms INTEGER CHECK(poll_interval_ms IS NULL OR poll_interval_ms >= 0)
             );
             CREATE INDEX task_routes_upstream_oauth ON task_routes(upstream_name, oauth_subject);
             CREATE INDEX task_routes_owner ON task_routes(caller_subject);
             CREATE INDEX task_routes_expiry ON task_routes((created_at_unix_ms + ttl_ms));
             CREATE TABLE task_route_revocations (upstream_name TEXT NOT NULL, oauth_subject TEXT, oauth_only INTEGER NOT NULL, prepared INTEGER NOT NULL DEFAULT 0);",
        ).map_err(sqlite_error)?;
    }
    let mut schema = transaction
        .prepare("PRAGMA table_info(task_route_revocations)")
        .map_err(sqlite_error)?;
    let revocation_columns = schema
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sqlite_error)?;
    if revocation_columns
        != [
            ("upstream_name".into(), "TEXT".into(), 1),
            ("oauth_subject".into(), "TEXT".into(), 0),
            ("oauth_only".into(), "INTEGER".into(), 1),
            ("prepared".into(), "INTEGER".into(), 1),
        ]
    {
        return Err("corrupt task revocation columns".into());
    }
    drop(schema);
    let columns = transaction
        .prepare(
            "SELECT upstream_name, oauth_subject, oauth_only FROM task_route_revocations LIMIT 0",
        )
        .map_err(sqlite_error)?;
    drop(columns);
    let invalid: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM task_route_revocations WHERE upstream_name IS NULL OR oauth_only NOT IN (0, 1) OR oauth_only IS NULL OR prepared NOT IN (0, 1) OR prepared IS NULL)", [], |row| row.get(0)).map_err(sqlite_error)?;
    if invalid {
        return Err("corrupt task revocation metadata".into());
    }
    transaction.commit().map_err(sqlite_error)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(sqlite_error)?;
    // SQLite ignores unsupported fullfsync requests. On macOS it strengthens
    // FULL durability by flushing the device cache, not only the OS buffer.
    connection
        .pragma_update(None, "fullfsync", true)
        .map_err(sqlite_error)?;
    replay_revocations(connection, true)?;
    Ok(())
}

/// A committed intent remains authoritative through process exit or failed deletion.
pub(super) fn replay_revocations(
    connection: &mut Connection,
    include_prepared: bool,
) -> Result<usize, String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_error)?;
    let removed = transaction
        .execute(
            "DELETE FROM task_routes WHERE EXISTS (
        SELECT 1 FROM task_route_revocations r WHERE r.upstream_name = task_routes.upstream_name
        AND (r.oauth_subject IS NULL OR r.oauth_subject IS task_routes.oauth_subject)
        AND (r.oauth_only = 0 OR task_routes.oauth_subject IS NOT NULL)
        AND (?1 OR r.prepared = 0))",
            [include_prepared],
        )
        .map_err(sqlite_error)?;
    transaction
        .execute(
            "DELETE FROM task_route_revocations WHERE ?1 OR prepared = 0",
            [include_prepared],
        )
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)?;
    Ok(removed)
}

fn validate_columns(connection: &Connection) -> Result<(), String> {
    let mut statement = connection
        .prepare("PRAGMA table_info(task_routes)")
        .map_err(sqlite_error)?;
    let columns = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sqlite_error)?;
    let expected = [
        ("public_task_id", "TEXT", false),
        ("native_task_id", "TEXT", true),
        ("upstream_name", "TEXT", true),
        ("caller_subject", "TEXT", false),
        ("oauth_subject", "TEXT", false),
        ("route_key", "TEXT", true),
        ("allowed_upstreams_json", "TEXT", false),
        ("config_fingerprint", "TEXT", true),
        ("created_at_unix_ms", "INTEGER", true),
        ("updated_at_unix_ms", "INTEGER", true),
        ("ttl_ms", "INTEGER", false),
        ("poll_interval_ms", "INTEGER", false),
    ];
    if columns.len() != expected.len()
        || expected.iter().any(|(name, kind, required)| {
            !columns.iter().any(|(n, k, not_null, pk)| {
                n == name
                    && k == kind
                    && (!required || *not_null == 1)
                    && (*name != "public_task_id" || *pk == 1)
            })
        })
    {
        return Err("corrupt task route database columns".to_string());
    }
    Ok(())
}

#[cfg(unix)]
fn prepare_file(path: &Path) -> Result<(), String> {
    use rustix::fs::{Mode, OFlags};
    let fd = rustix::fs::open(
        path,
        OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|_| "cannot safely open task route database".to_string())?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::File::from(fd)
        .set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|_| "cannot restrict task route database permissions".to_string())
}

#[cfg(not(unix))]
fn prepare_file(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn harden(path: &Path) -> Result<(), String> {
    use rustix::fs::{Mode, OFlags};
    use std::os::unix::fs::PermissionsExt;
    let fd = rustix::fs::open(
        path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| "cannot safely open task route database permissions target".to_string())?;
    std::fs::File::from(fd)
        .set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|_| "cannot restrict task route database permissions".to_string())
}

#[cfg(windows)]
fn harden(path: &Path) -> Result<(), String> {
    labby_auth::util::harden_secret_file(path)
        .map_err(|_| "cannot restrict task route database ACL".to_string())
}

#[cfg(not(any(unix, windows)))]
fn harden(_path: &Path) -> Result<(), String> {
    Err("task route file protection is unsupported on this platform".to_string())
}

pub(super) fn sqlite_error(error: rusqlite::Error) -> String {
    // Retain a bounded diagnostic classification, not arbitrary trigger SQL or
    // database content that could contain credentials or native identifiers.
    match error.sqlite_error_code() {
        Some(code) => format!("task route SQLite failure: {code:?}"),
        None => "task route SQLite data or schema failure".to_string(),
    }
}
