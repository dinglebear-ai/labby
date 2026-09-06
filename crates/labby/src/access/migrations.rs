use rusqlite::{Connection, TransactionBehavior, params};

use super::error::{AccessStoreError, AccessStoreResult};

use super::credential_schema;

pub(super) const SCHEMA_VERSION: i64 = 7;
pub(super) const APPLICATION_ID: i64 = 0x4c_41_43_31;
pub(super) const SCHEMA_FINGERPRINT: &str = "labby-access-v7-20260905";
pub(super) const V6_SCHEMA_VERSION: i64 = 6;
pub(super) const V6_SCHEMA_FINGERPRINT: &str = "labby-access-v6-20260905";
pub(super) const V5_SCHEMA_VERSION: i64 = 5;
pub(super) const V5_SCHEMA_FINGERPRINT: &str = "labby-access-v5-20260827";
pub(super) const V4_SCHEMA_VERSION: i64 = 4;
pub(super) const V4_SCHEMA_FINGERPRINT: &str = "labby-access-v4-20260827";
pub(super) const V3_SCHEMA_VERSION: i64 = 3;
pub(super) const V3_SCHEMA_FINGERPRINT: &str = "labby-access-v3-20260827";
pub(super) const V1_SCHEMA_VERSION: i64 = 1;
pub(super) const V1_SCHEMA_FINGERPRINT: &str = "labby-access-v1-20260823";
pub(super) const V2_SCHEMA_VERSION: i64 = 2;
pub(super) const V2_SCHEMA_FINGERPRINT: &str = "labby-access-v2-20260823";
pub(super) const V2_METADATA_SCHEMA: &str = "
CREATE TABLE access_metadata (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 2),
    schema_fingerprint TEXT NOT NULL,
    global_revision INTEGER NOT NULL CHECK(global_revision >= 0),
    updated_at INTEGER NOT NULL,
    bootstrap_generation INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_generation IN (0, 1)),
    bootstrap_identity_fingerprint TEXT,
    CHECK (
      (bootstrap_generation = 0 AND bootstrap_identity_fingerprint IS NULL)
      OR
      (bootstrap_generation = 1 AND bootstrap_identity_fingerprint IS NOT NULL
       AND length(trim(bootstrap_identity_fingerprint)) > 0)
    )
) STRICT;
";

pub(super) fn migrate(connection: &mut Connection) -> AccessStoreResult<()> {
    let found = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(super::store::map_sqlite_error)?;
    if found > SCHEMA_VERSION {
        return Err(AccessStoreError::UnsupportedSchema {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    if found == 0 {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .execute_batch(SCHEMA_V2_METADATA)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .execute_batch(DOMAIN_SCHEMA)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .execute(
                "INSERT INTO access_metadata(
                singleton, schema_version, schema_fingerprint, global_revision, updated_at,
                bootstrap_generation, bootstrap_identity_fingerprint
             ) VALUES (1, ?1, ?2, 0, unixepoch(), 0, NULL)",
                params![SCHEMA_VERSION, SCHEMA_FINGERPRINT],
            )
            .map_err(super::store::map_sqlite_error)?;
        install_team_schema_and_seed(&transaction)?;
        install_dev_container_schema(&transaction)?;
        transaction
            .pragma_update(None, "application_id", APPLICATION_ID)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .commit()
            .map_err(super::store::map_sqlite_error)?;
        return Ok(());
    }
    if found == V1_SCHEMA_VERSION {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(super::store::map_sqlite_error)?;
        super::integrity::validate_v1_before_migration(&transaction)?;
        rebuild_metadata_from_v1(&transaction)?;
        install_team_schema_and_seed(&transaction)?;
        install_dev_container_schema(&transaction)?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .commit()
            .map_err(super::store::map_sqlite_error)?;
        return Ok(());
    }
    if found == V2_SCHEMA_VERSION {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(super::store::map_sqlite_error)?;
        validate_v2_before_migration(&transaction)?;
        rebuild_metadata_from_v2(&transaction)?;
        install_team_schema_and_seed(&transaction)?;
        install_dev_container_schema(&transaction)?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .commit()
            .map_err(super::store::map_sqlite_error)?;
    }
    if found == V3_SCHEMA_VERSION {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(super::store::map_sqlite_error)?;
        validate_v3_before_migration(&transaction)?;
        transaction
            .execute_batch(
                "ALTER TABLE access_metadata RENAME TO access_metadata_v3;
                CREATE TABLE access_metadata (
                    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                    schema_version INTEGER NOT NULL CHECK(schema_version = 7),
                    schema_fingerprint TEXT NOT NULL,
                    global_revision INTEGER NOT NULL CHECK(global_revision >= 0),
                    updated_at INTEGER NOT NULL,
                    bootstrap_generation INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_generation IN (0, 1)),
                    bootstrap_identity_fingerprint TEXT,
                    CHECK (
                      (bootstrap_generation = 0 AND bootstrap_identity_fingerprint IS NULL)
                      OR
                      (bootstrap_generation = 1 AND bootstrap_identity_fingerprint IS NOT NULL
                        AND length(trim(bootstrap_identity_fingerprint)) > 0)
                    )
                ) STRICT;
                CREATE TABLE project_policy_publications (
                    project_id TEXT PRIMARY KEY CHECK(length(trim(project_id)) > 0),
                    policy_fingerprint BLOB NOT NULL CHECK(length(policy_fingerprint) = 32),
                    policy_epoch INTEGER NOT NULL CHECK(policy_epoch > 0),
                    updated_at INTEGER NOT NULL
                ) STRICT;
                CREATE TABLE access_admission_buckets ( admission_class TEXT NOT NULL CHECK(admission_class IN ('proof_global','proof_peer','credential_global','credential_peer')), bucket_fingerprint BLOB NOT NULL CHECK(length(bucket_fingerprint) = 32), window_started_at INTEGER NOT NULL, attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 64), updated_at INTEGER NOT NULL, PRIMARY KEY(admission_class, bucket_fingerprint) ) STRICT;
                CREATE INDEX access_admission_buckets_updated ON access_admission_buckets(updated_at);
                CREATE TABLE access_security_events ( event_id TEXT PRIMARY KEY CHECK(length(event_id) BETWEEN 1 AND 96), occurred_at INTEGER NOT NULL, event_kind TEXT NOT NULL CHECK(event_kind IN ('proof','credential_verify','credential_issue','credential_revoke')), decision TEXT NOT NULL CHECK(decision IN ('allow','deny')), reason_code TEXT NOT NULL CHECK(length(reason_code) BETWEEN 1 AND 64), target_fingerprint BLOB NOT NULL CHECK(length(target_fingerprint) = 32), peer_fingerprint BLOB CHECK(peer_fingerprint IS NULL OR length(peer_fingerprint) = 32), metadata_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(metadata_json) AND length(metadata_json) <= 1024) ) STRICT;
                CREATE INDEX access_security_events_retention ON access_security_events(occurred_at, event_id);",
            )
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .execute(
                "INSERT INTO access_metadata SELECT singleton,?1,?2,global_revision,updated_at,bootstrap_generation,bootstrap_identity_fingerprint FROM access_metadata_v3",
                params![SCHEMA_VERSION, SCHEMA_FINGERPRINT],
            )
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .execute_batch("DROP TABLE access_metadata_v3;")
            .map_err(super::store::map_sqlite_error)?;
        install_team_schema_and_seed(&transaction)?;
        install_dev_container_schema(&transaction)?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .commit()
            .map_err(super::store::map_sqlite_error)?;
    }
    if found == V4_SCHEMA_VERSION {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(super::store::map_sqlite_error)?;
        validate_v4_before_migration(&transaction)?;
        transaction.execute_batch("CREATE TABLE access_admission_buckets ( admission_class TEXT NOT NULL CHECK(admission_class IN ('proof_global','proof_peer','credential_global','credential_peer')), bucket_fingerprint BLOB NOT NULL CHECK(length(bucket_fingerprint) = 32), window_started_at INTEGER NOT NULL, attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 64), updated_at INTEGER NOT NULL, PRIMARY KEY(admission_class, bucket_fingerprint) ) STRICT; CREATE INDEX access_admission_buckets_updated ON access_admission_buckets(updated_at); CREATE TABLE access_security_events ( event_id TEXT PRIMARY KEY CHECK(length(event_id) BETWEEN 1 AND 96), occurred_at INTEGER NOT NULL, event_kind TEXT NOT NULL CHECK(event_kind IN ('proof','credential_verify','credential_issue','credential_revoke')), decision TEXT NOT NULL CHECK(decision IN ('allow','deny')), reason_code TEXT NOT NULL CHECK(length(reason_code) BETWEEN 1 AND 64), target_fingerprint BLOB NOT NULL CHECK(length(target_fingerprint) = 32), peer_fingerprint BLOB CHECK(peer_fingerprint IS NULL OR length(peer_fingerprint) = 32), metadata_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(metadata_json) AND length(metadata_json) <= 1024) ) STRICT; CREATE INDEX access_security_events_retention ON access_security_events(occurred_at, event_id); ALTER TABLE access_metadata RENAME TO access_metadata_v4; CREATE TABLE access_metadata (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), schema_version INTEGER NOT NULL CHECK(schema_version = 7), schema_fingerprint TEXT NOT NULL, global_revision INTEGER NOT NULL CHECK(global_revision >= 0), updated_at INTEGER NOT NULL, bootstrap_generation INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_generation IN (0, 1)), bootstrap_identity_fingerprint TEXT, CHECK ( (bootstrap_generation = 0 AND bootstrap_identity_fingerprint IS NULL) OR (bootstrap_generation = 1 AND bootstrap_identity_fingerprint IS NOT NULL AND length(trim(bootstrap_identity_fingerprint)) > 0) ) ) STRICT;").map_err(super::store::map_sqlite_error)?;
        transaction.execute("INSERT INTO access_metadata SELECT singleton,?1,?2,global_revision,updated_at,bootstrap_generation,bootstrap_identity_fingerprint FROM access_metadata_v4",params![SCHEMA_VERSION,SCHEMA_FINGERPRINT]).map_err(super::store::map_sqlite_error)?;
        transaction
            .execute_batch("DROP TABLE access_metadata_v4;")
            .map_err(super::store::map_sqlite_error)?;
        install_team_schema_and_seed(&transaction)?;
        install_dev_container_schema(&transaction)?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .commit()
            .map_err(super::store::map_sqlite_error)?;
    }
    if found == V5_SCHEMA_VERSION {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(super::store::map_sqlite_error)?;
        validate_v5_before_migration(&transaction)?;
        transaction
            .execute_batch(
                "ALTER TABLE access_metadata RENAME TO access_metadata_v5;
                 CREATE TABLE access_metadata (
                    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                    schema_version INTEGER NOT NULL CHECK(schema_version = 7),
                    schema_fingerprint TEXT NOT NULL,
                    global_revision INTEGER NOT NULL CHECK(global_revision >= 0),
                    updated_at INTEGER NOT NULL,
                    bootstrap_generation INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_generation IN (0, 1)),
                    bootstrap_identity_fingerprint TEXT,
                    CHECK ((bootstrap_generation = 0 AND bootstrap_identity_fingerprint IS NULL)
                      OR (bootstrap_generation = 1 AND bootstrap_identity_fingerprint IS NOT NULL
                        AND length(trim(bootstrap_identity_fingerprint)) > 0))
                 ) STRICT;",
            )
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .execute(
                "INSERT INTO access_metadata
                 SELECT singleton,?1,?2,global_revision,updated_at,
                        bootstrap_generation,bootstrap_identity_fingerprint
                 FROM access_metadata_v5",
                params![SCHEMA_VERSION, SCHEMA_FINGERPRINT],
            )
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .execute_batch("DROP TABLE access_metadata_v5;")
            .map_err(super::store::map_sqlite_error)?;
        install_team_schema_and_seed(&transaction)?;
        install_dev_container_schema(&transaction)?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .commit()
            .map_err(super::store::map_sqlite_error)?;
    }
    if found == V6_SCHEMA_VERSION {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(super::store::map_sqlite_error)?;
        validate_v6_before_migration(&transaction)?;
        let bootstrap_trigger = transaction
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='trigger' AND name='seed_bootstrap_team_authority'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .execute_batch("DROP TRIGGER seed_bootstrap_team_authority;")
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .execute_batch(
                "ALTER TABLE access_metadata RENAME TO access_metadata_v6;
                 CREATE TABLE access_metadata (
                    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                    schema_version INTEGER NOT NULL CHECK(schema_version = 7),
                    schema_fingerprint TEXT NOT NULL,
                    global_revision INTEGER NOT NULL CHECK(global_revision >= 0),
                    updated_at INTEGER NOT NULL,
                    bootstrap_generation INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_generation IN (0, 1)),
                    bootstrap_identity_fingerprint TEXT,
                    CHECK ((bootstrap_generation = 0 AND bootstrap_identity_fingerprint IS NULL)
                      OR (bootstrap_generation = 1 AND bootstrap_identity_fingerprint IS NOT NULL
                        AND length(trim(bootstrap_identity_fingerprint)) > 0))
                 ) STRICT;",
            )
            .map_err(super::store::map_sqlite_error)?;
        transaction.execute("INSERT INTO access_metadata SELECT singleton,?1,?2,global_revision,updated_at,bootstrap_generation,bootstrap_identity_fingerprint FROM access_metadata_v6", params![SCHEMA_VERSION, SCHEMA_FINGERPRINT]).map_err(super::store::map_sqlite_error)?;
        transaction
            .execute_batch("DROP TABLE access_metadata_v6;")
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .execute_batch(&bootstrap_trigger)
            .map_err(super::store::map_sqlite_error)?;
        install_dev_container_schema(&transaction)?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .commit()
            .map_err(super::store::map_sqlite_error)?;
    }
    Ok(())
}

pub(super) fn validate_migratable(connection: &Connection, version: i64) -> AccessStoreResult<()> {
    match version {
        V1_SCHEMA_VERSION => super::integrity::validate_v1_before_migration(connection),
        V2_SCHEMA_VERSION => validate_v2_before_migration(connection),
        V3_SCHEMA_VERSION => validate_v3_before_migration(connection),
        V4_SCHEMA_VERSION => validate_v4_before_migration(connection),
        V5_SCHEMA_VERSION => validate_v5_before_migration(connection),
        V6_SCHEMA_VERSION => validate_v6_before_migration(connection),
        _ => Err(AccessStoreError::IntegrityViolation {
            check: "schema_metadata",
        }),
    }
}

fn validate_v5_before_migration(connection: &Connection) -> AccessStoreResult<()> {
    let metadata = read_legacy_metadata(connection)?;
    let application_id = connection
        .query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))
        .map_err(super::store::map_sqlite_error)?;
    if metadata.schema_version != V5_SCHEMA_VERSION
        || metadata.schema_fingerprint != V5_SCHEMA_FINGERPRINT
        || metadata.global_revision < 0
        || !metadata.has_valid_bootstrap_fields()
        || application_id != APPLICATION_ID
    {
        return Err(AccessStoreError::IntegrityViolation {
            check: "schema_metadata",
        });
    }
    let canonical = canonical_v5_schema()?;
    if schema_manifest(connection)? != schema_manifest(&canonical)? {
        return Err(AccessStoreError::IntegrityViolation {
            check: "schema_manifest",
        });
    }
    validate_pre_migration_integrity(connection)?;
    super::integrity::validate_bootstrap_state(connection, metadata.bootstrap_generation)?;
    Ok(())
}

fn validate_v6_before_migration(connection: &Connection) -> AccessStoreResult<()> {
    let metadata = read_legacy_metadata(connection)?;
    let application_id = connection
        .query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))
        .map_err(super::store::map_sqlite_error)?;
    if metadata.schema_version != V6_SCHEMA_VERSION
        || metadata.schema_fingerprint != V6_SCHEMA_FINGERPRINT
        || metadata.global_revision < 0
        || !metadata.has_valid_bootstrap_fields()
        || application_id != APPLICATION_ID
    {
        return Err(AccessStoreError::IntegrityViolation {
            check: "schema_metadata",
        });
    }
    let canonical = canonical_v6_schema()?;
    if schema_manifest(connection)? != schema_manifest(&canonical)? {
        return Err(AccessStoreError::IntegrityViolation {
            check: "schema_manifest",
        });
    }
    validate_pre_migration_integrity(connection)?;
    super::integrity::validate_bootstrap_state(connection, metadata.bootstrap_generation)?;
    super::integrity::validate_team_authority(connection, metadata.bootstrap_generation)
}

fn canonical_v6_schema() -> AccessStoreResult<Connection> {
    let connection = Connection::open_in_memory().map_err(super::store::map_sqlite_error)?;
    let v6_metadata = SCHEMA_V2_METADATA.replace("schema_version = 7", "schema_version = 6");
    connection
        .execute_batch(&v6_metadata)
        .map_err(super::store::map_sqlite_error)?;
    connection
        .execute_batch(DOMAIN_SCHEMA)
        .map_err(super::store::map_sqlite_error)?;
    connection
        .execute_batch(TEAM_AUTHORITY_SCHEMA)
        .map_err(super::store::map_sqlite_error)?;
    Ok(connection)
}

pub(super) fn canonical_v5_schema() -> AccessStoreResult<Connection> {
    let connection = Connection::open_in_memory().map_err(super::store::map_sqlite_error)?;
    connection
        .execute_batch(SCHEMA_V2_METADATA)
        .map_err(super::store::map_sqlite_error)?;
    connection
        .execute_batch(DOMAIN_SCHEMA)
        .map_err(super::store::map_sqlite_error)?;
    connection
        .execute_batch(
            "ALTER TABLE access_metadata RENAME TO access_metadata_v6;
             CREATE TABLE access_metadata (
                singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                schema_version INTEGER NOT NULL CHECK(schema_version = 5),
                schema_fingerprint TEXT NOT NULL,
                global_revision INTEGER NOT NULL CHECK(global_revision >= 0),
                updated_at INTEGER NOT NULL,
                bootstrap_generation INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_generation IN (0, 1)),
                bootstrap_identity_fingerprint TEXT,
                CHECK ((bootstrap_generation = 0 AND bootstrap_identity_fingerprint IS NULL)
                  OR (bootstrap_generation = 1 AND bootstrap_identity_fingerprint IS NOT NULL
                    AND length(trim(bootstrap_identity_fingerprint)) > 0))
             ) STRICT;
             DROP TABLE access_metadata_v6;",
        )
        .map_err(super::store::map_sqlite_error)?;
    Ok(connection)
}

fn validate_v4_before_migration(connection: &Connection) -> AccessStoreResult<()> {
    let metadata = read_legacy_metadata(connection)?;
    let application_id = connection
        .query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))
        .map_err(super::store::map_sqlite_error)?;
    if metadata.schema_version != V4_SCHEMA_VERSION
        || metadata.schema_fingerprint != V4_SCHEMA_FINGERPRINT
        || metadata.global_revision < 0
        || !metadata.has_valid_bootstrap_fields()
        || application_id != APPLICATION_ID
    {
        return Err(AccessStoreError::IntegrityViolation {
            check: "schema_metadata",
        });
    }
    let canonical = canonical_v4_schema()?;
    if schema_manifest(connection)? != schema_manifest(&canonical)? {
        return Err(AccessStoreError::IntegrityViolation {
            check: "schema_manifest",
        });
    }
    validate_pre_migration_integrity(connection)?;
    super::integrity::validate_bootstrap_state(connection, metadata.bootstrap_generation)?;
    Ok(())
}

pub(super) fn canonical_v4_schema() -> AccessStoreResult<Connection> {
    let connection = Connection::open_in_memory().map_err(super::store::map_sqlite_error)?;
    connection
        .execute_batch(SCHEMA_V2_METADATA)
        .map_err(super::store::map_sqlite_error)?;
    connection
        .execute_batch(DOMAIN_SCHEMA)
        .map_err(super::store::map_sqlite_error)?;
    connection.execute_batch("DROP INDEX access_admission_buckets_updated; DROP TABLE access_admission_buckets; DROP INDEX access_security_events_retention; DROP TABLE access_security_events; ALTER TABLE access_metadata RENAME TO access_metadata_v5; CREATE TABLE access_metadata (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), schema_version INTEGER NOT NULL CHECK(schema_version = 4), schema_fingerprint TEXT NOT NULL, global_revision INTEGER NOT NULL CHECK(global_revision >= 0), updated_at INTEGER NOT NULL, bootstrap_generation INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_generation IN (0, 1)), bootstrap_identity_fingerprint TEXT, CHECK ((bootstrap_generation = 0 AND bootstrap_identity_fingerprint IS NULL) OR (bootstrap_generation = 1 AND bootstrap_identity_fingerprint IS NOT NULL AND length(trim(bootstrap_identity_fingerprint)) > 0))) STRICT; DROP TABLE access_metadata_v5;").map_err(super::store::map_sqlite_error)?;
    Ok(connection)
}

fn validate_v3_before_migration(connection: &Connection) -> AccessStoreResult<()> {
    let application_id = connection
        .query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))
        .map_err(super::store::map_sqlite_error)?;
    let metadata = read_legacy_metadata(connection)?;
    if application_id != APPLICATION_ID
        || metadata.schema_version != V3_SCHEMA_VERSION
        || metadata.schema_fingerprint != V3_SCHEMA_FINGERPRINT
        || metadata.global_revision < 0
        || !metadata.has_valid_bootstrap_fields()
    {
        return Err(AccessStoreError::IntegrityViolation {
            check: "schema_metadata",
        });
    }
    validate_pre_migration_integrity(connection)?;
    let canonical = canonical_v3_schema()?;
    if schema_manifest(connection)? != schema_manifest(&canonical)? {
        return Err(AccessStoreError::IntegrityViolation {
            check: "schema_manifest",
        });
    }
    super::integrity::validate_bootstrap_state(connection, metadata.bootstrap_generation)?;
    Ok(())
}

struct LegacyMetadata {
    schema_version: i64,
    schema_fingerprint: String,
    global_revision: i64,
    bootstrap_generation: i64,
    bootstrap_identity_fingerprint: Option<String>,
}

impl LegacyMetadata {
    fn has_valid_bootstrap_fields(&self) -> bool {
        matches!(
            (
                self.bootstrap_generation,
                self.bootstrap_identity_fingerprint.as_deref()
            ),
            (0, None)
        ) || matches!(
            (self.bootstrap_generation, self.bootstrap_identity_fingerprint.as_deref()),
            (1, Some(value)) if !value.is_empty()
        )
    }
}

fn read_legacy_metadata(connection: &Connection) -> AccessStoreResult<LegacyMetadata> {
    connection
        .query_row(
            "SELECT schema_version,schema_fingerprint,global_revision,bootstrap_generation,bootstrap_identity_fingerprint FROM access_metadata WHERE singleton=1",
            [],
            |row| {
                Ok(LegacyMetadata {
                    schema_version: row.get(0)?,
                    schema_fingerprint: row.get(1)?,
                    global_revision: row.get(2)?,
                    bootstrap_generation: row.get(3)?,
                    bootstrap_identity_fingerprint: row.get(4)?,
                })
            },
        )
        .map_err(map_metadata_read_error)
}

fn map_metadata_read_error(error: rusqlite::Error) -> AccessStoreError {
    if let Some(failure) = error.sqlite_error()
        && matches!(
            failure.code,
            rusqlite::ErrorCode::DatabaseBusy
                | rusqlite::ErrorCode::DatabaseLocked
                | rusqlite::ErrorCode::ReadOnly
                | rusqlite::ErrorCode::DiskFull
                | rusqlite::ErrorCode::DatabaseCorrupt
                | rusqlite::ErrorCode::NotADatabase
                | rusqlite::ErrorCode::OutOfMemory
                | rusqlite::ErrorCode::SystemIoFailure
                | rusqlite::ErrorCode::CannotOpen
        )
    {
        return super::store::map_sqlite_error(error);
    }
    AccessStoreError::IntegrityViolation {
        check: "schema_metadata",
    }
}

fn validate_pre_migration_integrity(connection: &Connection) -> AccessStoreResult<()> {
    let quick_check = connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(super::store::map_sqlite_error)?;
    let foreign_key_failure = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(super::store::map_sqlite_error)?;
    if quick_check != "ok" || foreign_key_failure {
        return Err(AccessStoreError::IntegrityViolation {
            check: "pre_migration",
        });
    }
    Ok(())
}

pub(super) fn canonical_v3_schema() -> AccessStoreResult<Connection> {
    let connection = Connection::open_in_memory().map_err(super::store::map_sqlite_error)?;
    connection
        .execute_batch(SCHEMA_V2_METADATA)
        .map_err(super::store::map_sqlite_error)?;
    connection
        .execute_batch(DOMAIN_SCHEMA)
        .map_err(super::store::map_sqlite_error)?;
    connection.execute_batch("DROP TABLE project_policy_publications; DROP INDEX access_admission_buckets_updated; DROP TABLE access_admission_buckets; DROP INDEX access_security_events_retention; DROP TABLE access_security_events; ALTER TABLE access_metadata RENAME TO access_metadata_v2; CREATE TABLE access_metadata (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), schema_version INTEGER NOT NULL CHECK(schema_version = 3), schema_fingerprint TEXT NOT NULL, global_revision INTEGER NOT NULL CHECK(global_revision >= 0), updated_at INTEGER NOT NULL, bootstrap_generation INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_generation IN (0, 1)), bootstrap_identity_fingerprint TEXT, CHECK ((bootstrap_generation = 0 AND bootstrap_identity_fingerprint IS NULL) OR (bootstrap_generation = 1 AND bootstrap_identity_fingerprint IS NOT NULL AND length(trim(bootstrap_identity_fingerprint)) > 0))) STRICT; DROP TABLE access_metadata_v2;").map_err(super::store::map_sqlite_error)?;
    Ok(connection)
}

fn rebuild_metadata_from_v1(transaction: &rusqlite::Transaction<'_>) -> AccessStoreResult<()> {
    transaction
        .execute_batch(SCHEMA_V2_REBUILD_BEGIN)
        .map_err(super::store::map_sqlite_error)?;
    transaction
        .execute_batch(SCHEMA_V2_METADATA)
        .map_err(super::store::map_sqlite_error)?;
    transaction.execute("INSERT INTO access_metadata(singleton, schema_version, schema_fingerprint, global_revision, updated_at, bootstrap_generation, bootstrap_identity_fingerprint) SELECT singleton, ?1, ?2, global_revision, updated_at, 0, NULL FROM access_metadata_v1", params![SCHEMA_VERSION, SCHEMA_FINGERPRINT]).map_err(super::store::map_sqlite_error)?;
    transaction
        .execute_batch(SCHEMA_V2_REBUILD_END)
        .map_err(super::store::map_sqlite_error)
}

fn rebuild_metadata_from_v2(transaction: &rusqlite::Transaction<'_>) -> AccessStoreResult<()> {
    transaction
        .execute_batch("ALTER TABLE access_metadata RENAME TO access_metadata_v2;")
        .map_err(super::store::map_sqlite_error)?;
    transaction
        .execute_batch(SCHEMA_V2_METADATA)
        .map_err(super::store::map_sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO access_metadata(
            singleton, schema_version, schema_fingerprint, global_revision, updated_at,
            bootstrap_generation, bootstrap_identity_fingerprint
         ) SELECT singleton, ?1, ?2, global_revision, updated_at,
                  bootstrap_generation, bootstrap_identity_fingerprint
           FROM access_metadata_v2",
            params![SCHEMA_VERSION, SCHEMA_FINGERPRINT],
        )
        .map_err(super::store::map_sqlite_error)?;
    transaction
        .execute_batch("DROP TABLE access_metadata_v2;")
        .map_err(super::store::map_sqlite_error)
}

fn validate_v2_before_migration(connection: &Connection) -> AccessStoreResult<()> {
    let application_id = connection
        .query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))
        .map_err(super::store::map_sqlite_error)?;
    let metadata = connection
        .query_row(
            "SELECT schema_version, schema_fingerprint, global_revision,
                bootstrap_generation, bootstrap_identity_fingerprint
         FROM access_metadata WHERE singleton=1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .map_err(map_metadata_read_error)?;
    let bootstrap_valid = matches!((&metadata.3, metadata.4.as_deref()), (0, None))
        || matches!((&metadata.3, metadata.4.as_deref()), (1, Some(value)) if !value.is_empty());
    if application_id != APPLICATION_ID
        || metadata.0 != V2_SCHEMA_VERSION
        || metadata.1 != V2_SCHEMA_FINGERPRINT
        || metadata.2 < 0
        || !bootstrap_valid
    {
        return Err(AccessStoreError::IntegrityViolation {
            check: "schema_metadata",
        });
    }
    let quick_check = connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(super::store::map_sqlite_error)?;
    let foreign_key_failure = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(super::store::map_sqlite_error)?;
    if quick_check != "ok" || foreign_key_failure {
        return Err(AccessStoreError::IntegrityViolation {
            check: "pre_migration",
        });
    }
    let canonical = Connection::open_in_memory().map_err(super::store::map_sqlite_error)?;
    canonical
        .execute_batch(V2_METADATA_SCHEMA)
        .map_err(super::store::map_sqlite_error)?;
    canonical
        .execute_batch(DOMAIN_SCHEMA)
        .map_err(super::store::map_sqlite_error)?;
    if schema_manifest(connection)? != schema_manifest(&canonical)? {
        return Err(AccessStoreError::IntegrityViolation {
            check: "schema_manifest",
        });
    }
    super::integrity::validate_bootstrap_state(connection, metadata.3)?;
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
            let sql = row.get::<_, String>(3)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                sql.split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .replace("( ", "(")
                    .replace(" )", ")"),
            ))
        })
        .map_err(super::store::map_sqlite_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(super::store::map_sqlite_error)
}

pub(super) const SCHEMA_V2_REBUILD_BEGIN: &str = "
ALTER TABLE access_metadata RENAME TO access_metadata_v1;
";

pub(super) const SCHEMA_V2_METADATA: &str = concat!(
    "
CREATE TABLE access_metadata (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 7),
    schema_fingerprint TEXT NOT NULL,
    global_revision INTEGER NOT NULL CHECK(global_revision >= 0),
    updated_at INTEGER NOT NULL,
    bootstrap_generation INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_generation IN (0, 1)),
    bootstrap_identity_fingerprint TEXT,
    CHECK (
      (bootstrap_generation = 0 AND bootstrap_identity_fingerprint IS NULL)
      OR
      (bootstrap_generation = 1 AND bootstrap_identity_fingerprint IS NOT NULL
       AND length(trim(bootstrap_identity_fingerprint)) > 0)
    )
) STRICT;
",
    credential_schema::credential_schema_sql!()
);
pub(super) const SCHEMA_V2_REBUILD_END: &str = "DROP TABLE access_metadata_v1;";

pub(super) const V1_METADATA_SCHEMA: &str = "
CREATE TABLE access_metadata (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 1),
    schema_fingerprint TEXT NOT NULL,
    global_revision INTEGER NOT NULL CHECK(global_revision >= 0),
    updated_at INTEGER NOT NULL
) STRICT;
";

pub(super) const TEAM_AUTHORITY_SCHEMA: &str = "
CREATE TABLE platform_administrators (
    principal_id TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK(status IN ('active', 'suspended', 'revoked')),
    authority_epoch INTEGER NOT NULL CHECK(authority_epoch > 0),
    granted_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    revoked_at INTEGER,
    CHECK ((status = 'revoked') = (revoked_at IS NOT NULL)),
    FOREIGN KEY (principal_id) REFERENCES principals(principal_id) ON DELETE RESTRICT,
    FOREIGN KEY (granted_by) REFERENCES principals(principal_id) ON DELETE RESTRICT
) STRICT;
CREATE INDEX platform_administrators_status
    ON platform_administrators(status, authority_epoch, principal_id);

CREATE TABLE groups (
    group_id TEXT PRIMARY KEY CHECK(length(trim(group_id)) > 0),
    organization_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind = 'team'),
    name TEXT NOT NULL CHECK(length(trim(name)) BETWEEN 1 AND 128),
    status TEXT NOT NULL CHECK(status IN
      ('active', 'suspended', 'deletion_pending', 'deleted')),
    policy_epoch INTEGER NOT NULL CHECK(policy_epoch > 0),
    membership_epoch INTEGER NOT NULL CHECK(membership_epoch > 0),
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER,
    CHECK ((status = 'deleted') = (deleted_at IS NOT NULL)),
    UNIQUE (organization_id, group_id),
    FOREIGN KEY (organization_id) REFERENCES organizations(organization_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, created_by)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT
) STRICT;
CREATE INDEX groups_organization_status
    ON groups(organization_id, status, group_id);

CREATE TABLE team_memberships (
    membership_id TEXT PRIMARY KEY CHECK(length(trim(membership_id)) > 0),
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('owner', 'admin', 'member')),
    status TEXT NOT NULL CHECK(status IN ('active', 'suspended', 'revoked')),
    membership_epoch INTEGER NOT NULL CHECK(membership_epoch > 0),
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    revoked_at INTEGER,
    CHECK ((status = 'revoked') = (revoked_at IS NOT NULL)),
    UNIQUE (organization_id, team_id, principal_id),
    FOREIGN KEY (organization_id, team_id)
      REFERENCES groups(organization_id, group_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, principal_id)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, created_by)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT
) STRICT;
CREATE INDEX team_memberships_principal
    ON team_memberships(organization_id, principal_id, status, team_id);
CREATE INDEX team_memberships_team
    ON team_memberships(organization_id, team_id, status, role, principal_id);

CREATE TABLE team_invitations (
    invitation_digest BLOB PRIMARY KEY CHECK(length(invitation_digest) = 32),
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('owner', 'admin', 'member')),
    invited_principal_id TEXT NOT NULL,
    inviter_principal_id TEXT NOT NULL,
    team_membership_epoch INTEGER NOT NULL CHECK(team_membership_epoch > 0),
    status TEXT NOT NULL CHECK(status IN ('pending', 'accepted', 'revoked', 'expired')),
    accepted_principal_id TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL CHECK(expires_at > created_at),
    accepted_at INTEGER,
    revoked_at INTEGER,
    updated_at INTEGER NOT NULL,
    CHECK ((status = 'accepted') =
      (accepted_principal_id IS NOT NULL AND accepted_at IS NOT NULL)),
    CHECK ((status = 'revoked') = (revoked_at IS NOT NULL)),
    FOREIGN KEY (organization_id, team_id)
      REFERENCES groups(organization_id, group_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, inviter_principal_id)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, invited_principal_id)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, accepted_principal_id)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT
) STRICT;
CREATE INDEX team_invitations_pending
    ON team_invitations(organization_id, team_id, status, expires_at);

CREATE TABLE team_project_assignments (
    assignment_id TEXT PRIMARY KEY CHECK(length(trim(assignment_id)) > 0),
    organization_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('owner', 'admin', 'member', 'viewer')),
    status TEXT NOT NULL CHECK(status IN ('active', 'suspended', 'revoked')),
    assignment_epoch INTEGER NOT NULL CHECK(assignment_epoch > 0),
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    revoked_at INTEGER,
    CHECK ((status = 'revoked') = (revoked_at IS NOT NULL)),
    UNIQUE (organization_id, team_id, project_id),
    FOREIGN KEY (organization_id, team_id)
      REFERENCES groups(organization_id, group_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, project_id)
      REFERENCES projects(organization_id, project_id) ON DELETE CASCADE,
    FOREIGN KEY (organization_id, created_by)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT
) STRICT;
CREATE INDEX team_project_assignments_project
    ON team_project_assignments(organization_id, project_id, status, team_id);

CREATE TRIGGER team_memberships_keep_last_owner_delete
BEFORE DELETE ON team_memberships
WHEN OLD.role = 'owner' AND OLD.status = 'active'
 AND EXISTS(SELECT 1 FROM groups
            WHERE organization_id=OLD.organization_id AND group_id=OLD.team_id
              AND status != 'deleted')
 AND NOT EXISTS(SELECT 1 FROM team_memberships
                WHERE organization_id=OLD.organization_id AND team_id=OLD.team_id
                  AND role='owner' AND status='active'
                  AND membership_id != OLD.membership_id)
BEGIN
  SELECT RAISE(ABORT, 'team requires an active owner');
END;

CREATE TRIGGER team_memberships_keep_last_owner_update
BEFORE UPDATE OF role, status ON team_memberships
WHEN OLD.role = 'owner' AND OLD.status = 'active'
 AND NOT (NEW.role = 'owner' AND NEW.status = 'active')
 AND EXISTS(SELECT 1 FROM groups
            WHERE organization_id=OLD.organization_id AND group_id=OLD.team_id
              AND status != 'deleted')
 AND NOT EXISTS(SELECT 1 FROM team_memberships
                WHERE organization_id=OLD.organization_id AND team_id=OLD.team_id
                  AND role='owner' AND status='active'
                  AND membership_id != OLD.membership_id)
BEGIN
  SELECT RAISE(ABORT, 'team requires an active owner');
END;

CREATE TRIGGER seed_bootstrap_team_authority
AFTER UPDATE OF bootstrap_generation ON access_metadata
WHEN OLD.bootstrap_generation = 0 AND NEW.bootstrap_generation = 1
BEGIN
  INSERT INTO platform_administrators(
    principal_id,status,authority_epoch,granted_by,created_at,updated_at,revoked_at)
  VALUES('bootstrap-owner','active',1,'bootstrap-owner',NEW.updated_at,NEW.updated_at,NULL);
  INSERT INTO groups(
    group_id,organization_id,kind,name,status,policy_epoch,membership_epoch,
    created_by,created_at,updated_at,deleted_at)
  VALUES('bootstrap-initial-team','bootstrap-local','team','Initial Team','active',1,1,
         'bootstrap-owner',NEW.updated_at,NEW.updated_at,NULL);
  INSERT INTO team_memberships(
    membership_id,organization_id,team_id,principal_id,role,status,membership_epoch,
    created_by,created_at,updated_at,revoked_at)
  VALUES('bootstrap-initial-team-owner','bootstrap-local','bootstrap-initial-team',
         'bootstrap-owner','owner','active',1,'bootstrap-owner',
         NEW.updated_at,NEW.updated_at,NULL);
  INSERT INTO access_audit(
    event_id,occurred_at,correlation_id,actor_principal_id,organization_id,project_id,
    action,target_kind,target_fingerprint,decision,reason_code,policy_epoch,metadata_json)
  VALUES('bootstrap-platform-admin-audit',NEW.updated_at,NULL,'bootstrap-owner',
         'bootstrap-local',NULL,'access.platform_admin.bootstrap','principal',
         'bootstrap-owner','allow','canonical_bootstrap_principal',0,'{}');
  INSERT INTO access_audit(
    event_id,occurred_at,correlation_id,actor_principal_id,organization_id,project_id,
    action,target_kind,target_fingerprint,decision,reason_code,policy_epoch,metadata_json)
  VALUES('bootstrap-initial-team-audit',NEW.updated_at,NULL,'bootstrap-owner',
         'bootstrap-local',NULL,'access.team.bootstrap','team',
         'bootstrap-initial-team','allow','canonical_bootstrap_principal',0,'{}');
END;
";

fn install_team_schema_and_seed(connection: &Connection) -> AccessStoreResult<()> {
    connection
        .execute_batch(TEAM_AUTHORITY_SCHEMA)
        .map_err(super::store::map_sqlite_error)?;
    let generation = connection
        .query_row(
            "SELECT bootstrap_generation FROM access_metadata WHERE singleton=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(super::store::map_sqlite_error)?;
    if generation == 1 {
        let updated_at = connection
            .query_row(
                "SELECT updated_at FROM access_metadata WHERE singleton=1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(super::store::map_sqlite_error)?;
        connection.execute("INSERT INTO platform_administrators VALUES('bootstrap-owner','active',1,'bootstrap-owner',?1,?1,NULL)", [updated_at]).map_err(super::store::map_sqlite_error)?;
        connection.execute("INSERT INTO groups VALUES('bootstrap-initial-team','bootstrap-local','team','Initial Team','active',1,1,'bootstrap-owner',?1,?1,NULL)", [updated_at]).map_err(super::store::map_sqlite_error)?;
        connection.execute("INSERT INTO team_memberships VALUES('bootstrap-initial-team-owner','bootstrap-local','bootstrap-initial-team','bootstrap-owner','owner','active',1,'bootstrap-owner',?1,?1,NULL)", [updated_at]).map_err(super::store::map_sqlite_error)?;
        connection.execute("INSERT INTO access_audit VALUES('bootstrap-platform-admin-audit',?1,NULL,'bootstrap-owner','bootstrap-local',NULL,'access.platform_admin.bootstrap','principal','bootstrap-owner','allow','canonical_bootstrap_principal',0,'{}')", [updated_at]).map_err(super::store::map_sqlite_error)?;
        connection.execute("INSERT INTO access_audit VALUES('bootstrap-initial-team-audit',?1,NULL,'bootstrap-owner','bootstrap-local',NULL,'access.team.bootstrap','team','bootstrap-initial-team','allow','canonical_bootstrap_principal',0,'{}')", [updated_at]).map_err(super::store::map_sqlite_error)?;
    }
    Ok(())
}

fn install_dev_container_schema(connection: &Connection) -> AccessStoreResult<()> {
    connection
        .execute_batch(super::dev_container::DEV_CONTAINER_SCHEMA)
        .map_err(super::store::map_sqlite_error)
}

pub(super) const DOMAIN_SCHEMA: &str = "
CREATE TABLE organizations (
    organization_id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK(length(trim(name)) > 0),
    status TEXT NOT NULL CHECK(status IN ('active', 'suspended', 'disabled')),
    policy_epoch INTEGER NOT NULL DEFAULT 0 CHECK(policy_epoch >= 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (organization_id, policy_epoch)
) STRICT;

CREATE TABLE principals (
    principal_id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('user', 'service_account')),
    status TEXT NOT NULL CHECK(status IN ('active', 'suspended', 'disabled')),
    display_name TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (organization_id, principal_id),
    FOREIGN KEY (organization_id) REFERENCES organizations(organization_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE principal_links (
    link_id TEXT PRIMARY KEY,
    principal_id TEXT NOT NULL,
    link_kind TEXT NOT NULL CHECK(link_kind IN ('external', 'local_credential')),
    issuer TEXT,
    subject TEXT,
    credential_id TEXT,
    status TEXT NOT NULL CHECK(status IN ('active', 'revoked')),
    verification_generation INTEGER NOT NULL CHECK(verification_generation > 0),
    link_generation INTEGER NOT NULL CHECK(link_generation > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (
      (link_kind = 'external' AND issuer IS NOT NULL AND length(trim(issuer)) > 0
       AND subject IS NOT NULL AND length(trim(subject)) > 0 AND credential_id IS NULL)
      OR
      (link_kind = 'local_credential' AND issuer IS NULL AND subject IS NULL
       AND credential_id IS NOT NULL AND length(trim(credential_id)) > 0)
    ),
    FOREIGN KEY (principal_id) REFERENCES principals(principal_id) ON DELETE RESTRICT
) STRICT;
CREATE UNIQUE INDEX principal_links_external_unique
    ON principal_links(issuer, subject) WHERE link_kind = 'external';
CREATE UNIQUE INDEX principal_links_local_unique
    ON principal_links(credential_id) WHERE link_kind = 'local_credential';

CREATE TABLE projects (
    project_id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    name TEXT NOT NULL CHECK(length(trim(name)) > 0),
    status TEXT NOT NULL CHECK(status IN ('active', 'suspended', 'disabled')),
    project_policy_epoch INTEGER NOT NULL DEFAULT 0 CHECK(project_policy_epoch >= 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (organization_id, project_id),
    FOREIGN KEY (organization_id) REFERENCES organizations(organization_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE project_memberships (
    membership_id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('owner', 'admin', 'member', 'viewer')),
    status TEXT NOT NULL CHECK(status IN ('active', 'suspended', 'disabled')),
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (organization_id, project_id, principal_id),
    FOREIGN KEY (organization_id, project_id)
      REFERENCES projects(organization_id, project_id) ON DELETE CASCADE,
    FOREIGN KEY (organization_id, principal_id)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, created_by)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE project_loadouts (
    organization_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    loadout_name TEXT NOT NULL CHECK(length(trim(loadout_name)) > 0),
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (organization_id, project_id),
    FOREIGN KEY (organization_id, project_id)
      REFERENCES projects(organization_id, project_id) ON DELETE CASCADE,
    FOREIGN KEY (organization_id, created_by)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE access_audit (
    event_id TEXT PRIMARY KEY,
    occurred_at INTEGER NOT NULL,
    correlation_id TEXT,
    actor_principal_id TEXT NOT NULL,
    organization_id TEXT NOT NULL,
    project_id TEXT,
    action TEXT NOT NULL CHECK(length(trim(action)) > 0),
    target_kind TEXT NOT NULL CHECK(length(trim(target_kind)) > 0),
    target_fingerprint TEXT NOT NULL CHECK(length(trim(target_fingerprint)) > 0),
    decision TEXT NOT NULL CHECK(decision IN ('allow', 'deny')),
    reason_code TEXT NOT NULL CHECK(length(trim(reason_code)) > 0),
    policy_epoch INTEGER NOT NULL CHECK(policy_epoch >= 0),
    metadata_json TEXT NOT NULL DEFAULT '{}',
    FOREIGN KEY (organization_id) REFERENCES organizations(organization_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, actor_principal_id)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, project_id)
      REFERENCES projects(organization_id, project_id) ON DELETE RESTRICT
) STRICT;

";

#[cfg(test)]
mod credential_migration_tests {
    use super::*;
    use labby_auth::{Authenticator, VerifiedIdentity};
    use sha2::{Digest, Sha256};

    #[test]
    fn legacy_metadata_reader_preserves_operational_sqlite_errors() {
        for code in [
            rusqlite::ffi::SQLITE_BUSY,
            rusqlite::ffi::SQLITE_LOCKED,
            rusqlite::ffi::SQLITE_READONLY,
            rusqlite::ffi::SQLITE_IOERR,
        ] {
            let error = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None);
            let mapped = map_metadata_read_error(error);
            assert!(!matches!(
                mapped,
                AccessStoreError::IntegrityViolation { .. }
            ));
        }

        assert!(matches!(
            map_metadata_read_error(rusqlite::Error::QueryReturnedNoRows),
            AccessStoreError::IntegrityViolation {
                check: "schema_metadata"
            }
        ));
    }

    fn canonical_v2() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        connection.execute_batch(V2_METADATA_SCHEMA).unwrap();
        connection.execute_batch(DOMAIN_SCHEMA).unwrap();
        connection
            .execute(
                "INSERT INTO access_metadata(
                    singleton,schema_version,schema_fingerprint,global_revision,updated_at,
                    bootstrap_generation,bootstrap_identity_fingerprint
                 ) VALUES(1,?1,?2,7,123,0,NULL)",
                params![V2_SCHEMA_VERSION, V2_SCHEMA_FINGERPRINT],
            )
            .unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        connection
            .pragma_update(None, "user_version", V2_SCHEMA_VERSION)
            .unwrap();
        connection
    }

    fn canonical_v3() -> Connection {
        let connection = canonical_v3_schema().unwrap();
        connection
            .execute(
                "INSERT INTO access_metadata(singleton,schema_version,schema_fingerprint,global_revision,updated_at,bootstrap_generation,bootstrap_identity_fingerprint) VALUES(1,?1,?2,9,123,0,NULL)",
                params![V3_SCHEMA_VERSION, V3_SCHEMA_FINGERPRINT],
            )
            .unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        connection
            .pragma_update(None, "user_version", V3_SCHEMA_VERSION)
            .unwrap();
        connection
    }

    fn canonical_v4() -> Connection {
        let connection = canonical_v4_schema().unwrap();
        connection
            .execute(
                "INSERT INTO access_metadata VALUES(1,?1,?2,11,123,0,NULL)",
                params![V4_SCHEMA_VERSION, V4_SCHEMA_FINGERPRINT],
            )
            .unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        connection
            .pragma_update(None, "user_version", V4_SCHEMA_VERSION)
            .unwrap();
        connection
    }

    fn canonical_v5() -> Connection {
        let connection = canonical_v5_schema().unwrap();
        connection
            .execute(
                "INSERT INTO access_metadata VALUES(1,?1,?2,13,123,0,NULL)",
                params![V5_SCHEMA_VERSION, V5_SCHEMA_FINGERPRINT],
            )
            .unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        connection
            .pragma_update(None, "user_version", V5_SCHEMA_VERSION)
            .unwrap();
        connection
    }

    fn production_shaped_v4() -> Connection {
        let connection = canonical_v4();
        connection
            .execute_batch(
                "INSERT INTO organizations VALUES
                   ('org-production', 'Production Équipe', 'active', 17, 100, 200);
                 INSERT INTO principals VALUES
                   ('principal-owner', 'org-production', 'user', 'active', 'Owner', 100, 200),
                   ('principal-member', 'org-production', 'user', 'active', 'Member', 101, 201),
                   ('principal-disabled', 'org-production', 'user', 'disabled', NULL, 102, 202);
                 INSERT INTO principal_links VALUES
                   ('link-owner', 'principal-owner', 'external', 'https://issuer.example', 'owner-subject', NULL, 'active', 1, 1, 100, 200),
                   ('link-member', 'principal-member', 'external', 'https://issuer.example', 'member-subject', NULL, 'active', 1, 2, 101, 201),
                   ('link-disabled', 'principal-disabled', 'local_credential', NULL, NULL, 'legacy-disabled', 'revoked', 1, 3, 102, 202);
                 INSERT INTO projects VALUES
                   ('project-alpha', 'org-production', 'Alpha', 'active', 9, 110, 210),
                   ('project-beta', 'org-production', 'Beta', 'suspended', 4, 111, 211);
                 INSERT INTO project_memberships VALUES
                   ('membership-owner', 'org-production', 'project-alpha', 'principal-owner', 'owner', 'active', 'principal-owner', 120, 220),
                   ('membership-member', 'org-production', 'project-alpha', 'principal-member', 'member', 'active', 'principal-owner', 121, 221),
                   ('membership-viewer', 'org-production', 'project-beta', 'principal-member', 'viewer', 'suspended', 'principal-owner', 122, 222);
                 INSERT INTO project_loadouts VALUES
                   ('org-production', 'project-alpha', 'production-default', 'principal-owner', 130, 230);
                 INSERT INTO access_audit VALUES
                   ('audit-owner', 140, 'correlation-1', 'principal-owner', 'org-production', 'project-alpha', 'project.read', 'project', 'sha256:alpha', 'allow', 'membership', 17, '{}'),
                   ('audit-deny', 141, 'correlation-2', 'principal-member', 'org-production', 'project-beta', 'project.use', 'project', 'sha256:beta', 'deny', 'project_suspended', 17, '{}');
                 INSERT INTO project_policy_publications VALUES
                   ('project-alpha', zeroblob(32), 9, 230);",
            )
            .unwrap();
        connection
    }

    fn production_shaped_v5() -> Connection {
        let connection = canonical_v5();
        connection
            .execute_batch(
                "INSERT INTO organizations VALUES
                   ('org-production', 'Production Équipe', 'active', 17, 100, 200);
                 INSERT INTO principals VALUES
                   ('principal-owner', 'org-production', 'user', 'active', 'Owner', 100, 200),
                   ('principal-member', 'org-production', 'user', 'active', 'Member', 101, 201);
                 INSERT INTO principal_links VALUES
                   ('link-owner', 'principal-owner', 'external', 'https://issuer.example', 'owner-subject', NULL, 'active', 1, 1, 100, 200),
                   ('link-member', 'principal-member', 'external', 'https://issuer.example', 'member-subject', NULL, 'active', 1, 2, 101, 201);
                 INSERT INTO projects VALUES
                   ('project-alpha', 'org-production', 'Alpha', 'active', 9, 110, 210);
                 INSERT INTO project_memberships VALUES
                   ('membership-owner', 'org-production', 'project-alpha', 'principal-owner', 'owner', 'active', 'principal-owner', 120, 220),
                   ('membership-member', 'org-production', 'project-alpha', 'principal-member', 'member', 'active', 'principal-owner', 121, 221);
                 INSERT INTO project_loadouts VALUES
                   ('org-production', 'project-alpha', 'production-default', 'principal-owner', 130, 230);
                 INSERT INTO access_audit VALUES
                   ('audit-owner', 140, 'correlation-1', 'principal-owner', 'org-production', 'project-alpha', 'project.read', 'project', 'sha256:alpha', 'allow', 'membership', 17, '{}');
                 INSERT INTO project_policy_publications VALUES
                   ('project-alpha', zeroblob(32), 9, 230);",
            )
            .unwrap();
        connection
            .execute_batch(
                "INSERT INTO access_admission_buckets VALUES
                   ('credential_peer', zeroblob(32), 300, 3, 303);
                 INSERT INTO access_security_events VALUES
                   ('security-deny', 304, 'credential_verify', 'deny',
                    'credential_invalid', zeroblob(32), NULL, '{}');",
            )
            .unwrap();
        validate_migratable(&connection, V5_SCHEMA_VERSION).unwrap();
        connection
    }

    fn logical_inventory(connection: &Connection) -> Vec<(String, i64, String)> {
        use rusqlite::types::ValueRef;

        let mut tables = connection
            .prepare(
                "SELECT name FROM sqlite_schema
                 WHERE type='table' AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .unwrap();
        let names = tables
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        drop(tables);

        let mut inventory = Vec::with_capacity(names.len());
        for table in names {
            let mut statement = connection
                .prepare(&format!("SELECT * FROM \"{table}\" ORDER BY rowid"))
                .unwrap();
            let columns = statement.column_count();
            let mut rows = statement.query([]).unwrap();
            let mut count = 0_i64;
            let mut digest = Sha256::new();
            while let Some(row) = rows.next().unwrap() {
                count += 1;
                digest.update([0xff]);
                for index in 0..columns {
                    match row.get_ref(index).unwrap() {
                        ValueRef::Null => digest.update([0]),
                        ValueRef::Integer(value) => {
                            digest.update([1]);
                            digest.update(value.to_be_bytes());
                        }
                        ValueRef::Real(value) => {
                            digest.update([2]);
                            digest.update(value.to_bits().to_be_bytes());
                        }
                        ValueRef::Text(value) => {
                            digest.update([3]);
                            digest.update(value.len().to_be_bytes());
                            digest.update(value);
                        }
                        ValueRef::Blob(value) => {
                            digest.update([4]);
                            digest.update(value.len().to_be_bytes());
                            digest.update(value);
                        }
                    }
                }
            }
            let digest = digest
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            inventory.push((table, count, digest));
        }
        inventory
    }

    fn snapshot_into(connection: &Connection, path: &std::path::Path) {
        connection
            .execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])
            .unwrap();
    }

    #[test]
    fn fresh_database_contains_bounded_credential_schema() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrate(&mut connection).unwrap();
        super::super::integrity::validate(&connection).unwrap();

        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        for table in [
            "access_installations",
            "bootstrap_proofs",
            "project_credentials",
            "credential_idempotency",
            "access_tombstones",
        ] {
            assert!(
                connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                        [table],
                        |row| row.get::<_, bool>(0),
                    )
                    .unwrap()
            );
        }
    }

    #[test]
    fn canonical_v2_upgrades_atomically_and_preserves_metadata() {
        let mut connection = canonical_v2();
        migrate(&mut connection).unwrap();
        super::super::integrity::validate(&connection).unwrap();
        let metadata = connection
            .query_row(
                "SELECT schema_version,schema_fingerprint,global_revision,
                        bootstrap_generation,bootstrap_identity_fingerprint
                 FROM access_metadata WHERE singleton=1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            metadata,
            (SCHEMA_VERSION, SCHEMA_FINGERPRINT.into(), 7, 0, None)
        );
    }

    #[test]
    fn canonical_v3_upgrades_atomically_with_empty_policy_epoch_registry() {
        let mut connection = canonical_v3();
        migrate(&mut connection).unwrap();
        let canonical = Connection::open_in_memory().unwrap();
        canonical.execute_batch(SCHEMA_V2_METADATA).unwrap();
        canonical.execute_batch(DOMAIN_SCHEMA).unwrap();
        canonical.execute_batch(TEAM_AUTHORITY_SCHEMA).unwrap();
        canonical
            .execute_batch(super::super::dev_container::DEV_CONTAINER_SCHEMA)
            .unwrap();
        assert_eq!(
            schema_manifest(&connection).unwrap(),
            schema_manifest(&canonical).unwrap()
        );
        super::super::integrity::validate(&connection).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT global_revision FROM access_metadata", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            9
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM project_policy_publications",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn canonical_v4_adds_bounded_security_tables_atomically() {
        let mut connection = canonical_v4();
        migrate(&mut connection).unwrap();
        let expected = Connection::open_in_memory().unwrap();
        expected.execute_batch(SCHEMA_V2_METADATA).unwrap();
        expected.execute_batch(DOMAIN_SCHEMA).unwrap();
        expected.execute_batch(TEAM_AUTHORITY_SCHEMA).unwrap();
        expected
            .execute_batch(super::super::dev_container::DEV_CONTAINER_SCHEMA)
            .unwrap();
        assert_eq!(
            schema_manifest(&connection).unwrap(),
            schema_manifest(&expected).unwrap()
        );
        super::super::integrity::validate(&connection).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM access_admission_buckets", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM access_security_events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn canonical_v6_adds_empty_dev_container_ledger_atomically() {
        let mut connection = canonical_v6_schema().unwrap();
        connection
            .execute(
                "INSERT INTO access_metadata(singleton,schema_version,schema_fingerprint,global_revision,updated_at,bootstrap_generation,bootstrap_identity_fingerprint) VALUES(1,?1,?2,11,100,0,NULL)",
                params![V6_SCHEMA_VERSION, V6_SCHEMA_FINGERPRINT],
            )
            .unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        connection
            .pragma_update(None, "user_version", V6_SCHEMA_VERSION)
            .unwrap();

        migrate(&mut connection).unwrap();

        super::super::integrity::validate(&connection).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        for table in [
            "dev_container_templates",
            "dev_container_owner_quotas",
            "dev_container_instances",
            "dev_container_ledger",
        ] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn v5_bootstrap_migrates_to_explicit_platform_admin_and_initial_team_owner() {
        let mut connection = canonical_v5();
        let identity = VerifiedIdentity::local_credential(
            Authenticator::StaticBearer,
            "static-bearer:primary",
        )
        .unwrap();
        let fingerprint = identity.safe_fingerprint();
        connection
            .execute_batch(
                "INSERT INTO organizations VALUES
                   ('bootstrap-local','Local','active',0,100,100);
                 INSERT INTO principals VALUES
                   ('bootstrap-owner','bootstrap-local','user','active',NULL,100,100);
                 INSERT INTO principal_links VALUES
                   ('bootstrap-owner-link','bootstrap-owner','local_credential',NULL,NULL,
                    'static-bearer:primary','active',1,1,100,100);
                 INSERT INTO projects VALUES
                   ('bootstrap-default','bootstrap-local','Default','active',0,100,100);
                 INSERT INTO project_memberships VALUES
                   ('bootstrap-owner-membership','bootstrap-local','bootstrap-default',
                    'bootstrap-owner','owner','active','bootstrap-owner',100,100);",
            )
            .unwrap();
        connection.execute("INSERT INTO access_audit VALUES('bootstrap-owner-audit',100,NULL,'bootstrap-owner','bootstrap-local','bootstrap-default','access.bootstrap_owner','project',?1,'allow','explicit_owner_bootstrap',0,'{}')", [&fingerprint]).unwrap();
        connection.execute("UPDATE access_metadata SET global_revision=1,bootstrap_generation=1,bootstrap_identity_fingerprint=?1,updated_at=100", [&fingerprint]).unwrap();

        migrate(&mut connection).unwrap();
        super::super::integrity::validate(&connection).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT principal_id,status,authority_epoch FROM platform_administrators",
                    [],
                    |row| Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?
                    )),
                )
                .unwrap(),
            ("bootstrap-owner".into(), "active".into(), 1)
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT team_id,principal_id,role,status,membership_epoch
                     FROM team_memberships",
                    [],
                    |row| Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?
                    )),
                )
                .unwrap(),
            (
                "bootstrap-initial-team".into(),
                "bootstrap-owner".into(),
                "owner".into(),
                "active".into(),
                1
            )
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM access_audit
                     WHERE event_id IN ('bootstrap-platform-admin-audit',
                                        'bootstrap-initial-team-audit')",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert!(
            connection
                .execute(
                    "UPDATE team_memberships SET status='revoked',revoked_at=999
                     WHERE membership_id='bootstrap-initial-team-owner'",
                    [],
                )
                .is_err(),
            "the last active Team owner must not be revocable"
        );
    }

    #[test]
    fn production_shaped_v4_rehearsal_preserves_inventory_reopens_and_restores() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("production-v4.db");
        let checkpoint_path = directory.path().join("production-v4.backup.db");
        let restored_path = directory.path().join("restored-v4.db");

        let source = production_shaped_v4();
        let before = logical_inventory(&source);
        snapshot_into(&source, &source_path);
        snapshot_into(&source, &checkpoint_path);
        drop(source);

        let mut migrated = Connection::open(&source_path).unwrap();
        migrated.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrate(&mut migrated).unwrap();
        super::super::integrity::validate(&migrated).unwrap();
        assert_eq!(
            migrated
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        let after = logical_inventory(&migrated);
        for (table, expected_count, expected_digest) in &before {
            if matches!(
                table.as_str(),
                "access_metadata" | "access_admission_buckets" | "access_security_events"
            ) {
                continue;
            }
            let (_, actual_count, actual_digest) = after
                .iter()
                .find(|(candidate, _, _)| candidate == table)
                .unwrap_or_else(|| panic!("table disappeared: {table}"));
            assert_eq!(
                actual_count, expected_count,
                "row count changed for {table}"
            );
            assert_eq!(
                actual_digest, expected_digest,
                "content changed for {table}"
            );
        }
        drop(migrated);

        for _ in 0..2 {
            let reopened = Connection::open(&source_path).unwrap();
            reopened.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
            super::super::integrity::validate(&reopened).unwrap();
        }

        std::fs::copy(&checkpoint_path, &restored_path).unwrap();
        let restored = Connection::open(&restored_path).unwrap();
        restored.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        validate_migratable(&restored, V4_SCHEMA_VERSION).unwrap();
        assert_eq!(logical_inventory(&restored), before);
        assert_eq!(
            restored
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            V4_SCHEMA_VERSION
        );
    }

    #[test]
    fn production_shaped_v5_checkpoint_restores_exact_logical_inventory() {
        let directory = tempfile::tempdir().unwrap();
        let checkpoint_path = directory.path().join("production-v5.backup.db");
        let restored_path = directory.path().join("restored-v5.db");
        let migrated_path = directory.path().join("migrated-v6.db");
        let source = production_shaped_v5();
        let expected = logical_inventory(&source);
        snapshot_into(&source, &checkpoint_path);
        drop(source);

        std::fs::copy(&checkpoint_path, &restored_path).unwrap();
        std::fs::copy(&checkpoint_path, &migrated_path).unwrap();
        let mut migrated = Connection::open(&migrated_path).unwrap();
        migrated.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrate(&mut migrated).unwrap();
        super::super::integrity::validate(&migrated).unwrap();
        drop(migrated);

        for _ in 0..2 {
            let restored = Connection::open(&restored_path).unwrap();
            restored.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
            validate_migratable(&restored, V5_SCHEMA_VERSION).unwrap();
            assert_eq!(logical_inventory(&restored), expected);
            assert_eq!(
                restored
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                V5_SCHEMA_VERSION
            );
        }
    }

    #[test]
    fn rehearsal_failures_leave_v4_inventory_and_version_unchanged() {
        let mut malformed = production_shaped_v4();
        malformed
            .execute_batch("DROP INDEX project_credentials_authority;")
            .unwrap();
        let malformed_before = logical_inventory(&malformed);
        assert!(matches!(
            migrate(&mut malformed),
            Err(AccessStoreError::IntegrityViolation {
                check: "schema_manifest"
            })
        ));
        assert_eq!(logical_inventory(&malformed), malformed_before);
        assert_eq!(
            malformed
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            V4_SCHEMA_VERSION
        );

        let mut read_only = production_shaped_v4();
        let read_only_before = logical_inventory(&read_only);
        read_only.execute_batch("PRAGMA query_only=ON;").unwrap();
        assert!(matches!(
            migrate(&mut read_only),
            Err(AccessStoreError::ReadOnly)
        ));
        assert_eq!(logical_inventory(&read_only), read_only_before);
        assert_eq!(
            read_only
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            V4_SCHEMA_VERSION
        );
    }

    #[test]
    fn altered_v3_manifest_refuses_without_advancing_version() {
        let mut connection = canonical_v3();
        connection.execute_batch("DROP INDEX project_credentials_authority; CREATE INDEX project_credentials_authority ON project_credentials(project_id);").unwrap();
        assert!(matches!(
            migrate(&mut connection),
            Err(AccessStoreError::IntegrityViolation {
                check: "schema_manifest"
            })
        ));
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            V3_SCHEMA_VERSION
        );
    }

    #[test]
    fn migrated_file_reopens_at_exact_current_schema() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("access.db");
        let mut connection = Connection::open(&path).unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrate(&mut connection).unwrap();
        drop(connection);

        let reopened = Connection::open(path).unwrap();
        reopened.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        super::super::integrity::validate(&reopened).unwrap();
    }

    #[test]
    fn altered_v2_manifest_refuses_without_advancing_version() {
        let mut connection = canonical_v2();
        connection.execute_batch("DROP INDEX principal_links_local_unique; CREATE INDEX principal_links_local_unique ON principal_links(credential_id);").unwrap();
        assert!(matches!(
            migrate(&mut connection),
            Err(AccessStoreError::IntegrityViolation {
                check: "schema_manifest"
            })
        ));
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            V2_SCHEMA_VERSION
        );
    }

    #[test]
    fn newer_schema_is_refused_without_mutation() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        assert!(matches!(
            migrate(&mut connection),
            Err(AccessStoreError::UnsupportedSchema { found, supported })
                if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION
        ));
    }

    #[test]
    fn digest_status_and_attempt_constraints_fail_closed() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrate(&mut connection).unwrap();
        let short_digest = vec![0_u8; 31];
        assert!(
            connection
                .execute(
                    "INSERT INTO bootstrap_proofs(
                proof_id,prepare_id,installation_id,installation_generation,proof_digest,
                manifest_digest,request_digest,idempotency_digest,credential_id,
                credential_digest,proof_generation,semantic_attempts,status,created_at,
                expires_at,updated_at
             ) VALUES('proof','prepare','install',1,?1,zeroblob(32),zeroblob(32),
                      randomblob(32),'credential',randomblob(32),1,0,'active',1,2,1)",
                    [&short_digest],
                )
                .is_err()
        );
        assert!(
            connection
                .execute(
                    "INSERT INTO access_tombstones(
                tombstone_id,installation_id,artifact_kind,public_id,canonical_digest,
                artifact_generation,reason_code,created_at
             ) VALUES('tombstone','install','unknown','id',zeroblob(32),1,'test',1)",
                    [],
                )
                .is_err()
        );
    }
}
