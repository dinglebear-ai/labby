use rusqlite::{Connection, TransactionBehavior, params};

use super::error::{AccessStoreError, AccessStoreResult};

use super::credential_schema;

pub(super) const SCHEMA_VERSION: i64 = 5;
pub(super) const APPLICATION_ID: i64 = 0x4c_41_43_31;
pub(super) const SCHEMA_FINGERPRINT: &str = "labby-access-v5-20260827";
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
                    schema_version INTEGER NOT NULL CHECK(schema_version = 5),
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
        transaction.execute_batch("CREATE TABLE access_admission_buckets ( admission_class TEXT NOT NULL CHECK(admission_class IN ('proof_global','proof_peer','credential_global','credential_peer')), bucket_fingerprint BLOB NOT NULL CHECK(length(bucket_fingerprint) = 32), window_started_at INTEGER NOT NULL, attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 64), updated_at INTEGER NOT NULL, PRIMARY KEY(admission_class, bucket_fingerprint) ) STRICT; CREATE INDEX access_admission_buckets_updated ON access_admission_buckets(updated_at); CREATE TABLE access_security_events ( event_id TEXT PRIMARY KEY CHECK(length(event_id) BETWEEN 1 AND 96), occurred_at INTEGER NOT NULL, event_kind TEXT NOT NULL CHECK(event_kind IN ('proof','credential_verify','credential_issue','credential_revoke')), decision TEXT NOT NULL CHECK(decision IN ('allow','deny')), reason_code TEXT NOT NULL CHECK(length(reason_code) BETWEEN 1 AND 64), target_fingerprint BLOB NOT NULL CHECK(length(target_fingerprint) = 32), peer_fingerprint BLOB CHECK(peer_fingerprint IS NULL OR length(peer_fingerprint) = 32), metadata_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(metadata_json) AND length(metadata_json) <= 1024) ) STRICT; CREATE INDEX access_security_events_retention ON access_security_events(occurred_at, event_id); ALTER TABLE access_metadata RENAME TO access_metadata_v4; CREATE TABLE access_metadata (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), schema_version INTEGER NOT NULL CHECK(schema_version = 5), schema_fingerprint TEXT NOT NULL, global_revision INTEGER NOT NULL CHECK(global_revision >= 0), updated_at INTEGER NOT NULL, bootstrap_generation INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_generation IN (0, 1)), bootstrap_identity_fingerprint TEXT, CHECK ( (bootstrap_generation = 0 AND bootstrap_identity_fingerprint IS NULL) OR (bootstrap_generation = 1 AND bootstrap_identity_fingerprint IS NOT NULL AND length(trim(bootstrap_identity_fingerprint)) > 0) ) ) STRICT;").map_err(super::store::map_sqlite_error)?;
        transaction.execute("INSERT INTO access_metadata SELECT singleton,?1,?2,global_revision,updated_at,bootstrap_generation,bootstrap_identity_fingerprint FROM access_metadata_v4",params![SCHEMA_VERSION,SCHEMA_FINGERPRINT]).map_err(super::store::map_sqlite_error)?;
        transaction
            .execute_batch("DROP TABLE access_metadata_v4;")
            .map_err(super::store::map_sqlite_error)?;
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
        _ => Err(AccessStoreError::IntegrityViolation {
            check: "schema_metadata",
        }),
    }
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
    if error.sqlite_error().is_some() {
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
    schema_version INTEGER NOT NULL CHECK(schema_version = 5),
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
