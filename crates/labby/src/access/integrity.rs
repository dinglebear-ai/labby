use rusqlite::Connection;

use super::error::{AccessStoreError, AccessStoreResult};

pub(super) fn validate(connection: &Connection) -> AccessStoreResult<()> {
    let quick_check = connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(super::store::map_sqlite_error)?;
    if quick_check != "ok" {
        return Err(integrity("quick_check"));
    }

    let foreign_key_failure = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(super::store::map_sqlite_error)?;
    if foreign_key_failure {
        return Err(integrity("foreign_key_check"));
    }

    let application_id = connection
        .query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))
        .map_err(super::store::map_sqlite_error)?;
    if application_id != super::migrations::APPLICATION_ID {
        return Err(integrity("application_id"));
    }

    let metadata = connection.query_row(
        "SELECT schema_version, schema_fingerprint, global_revision
         FROM access_metadata WHERE singleton = 1",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    );
    let Ok((schema_version, fingerprint, global_revision)) = metadata else {
        return Err(integrity("schema_metadata"));
    };
    if schema_version != super::migrations::SCHEMA_VERSION
        || fingerprint != super::migrations::SCHEMA_FINGERPRINT
        || global_revision < 0
    {
        return Err(integrity("schema_metadata"));
    }

    validate_manifest(connection)
}

fn validate_manifest(connection: &Connection) -> AccessStoreResult<()> {
    let actual = schema_manifest(connection)?;
    let canonical = Connection::open_in_memory().map_err(super::store::map_sqlite_error)?;
    canonical
        .execute_batch(super::migrations::SCHEMA_V1)
        .map_err(super::store::map_sqlite_error)?;
    let expected = schema_manifest(&canonical)?;
    if actual != expected {
        return Err(integrity("schema_manifest"));
    }
    Ok(())
}

fn schema_manifest(
    connection: &Connection,
) -> AccessStoreResult<Vec<(String, String, String, String)>> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema
             WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
             ORDER BY type, name, tbl_name",
        )
        .map_err(super::store::map_sqlite_error)?;
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                normalize_sql(&row.get::<_, String>(3)?),
            ))
        })
        .map_err(super::store::map_sqlite_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(super::store::map_sqlite_error)
}

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

const fn integrity(check: &'static str) -> AccessStoreError {
    AccessStoreError::IntegrityViolation { check }
}
