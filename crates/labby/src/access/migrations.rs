use rusqlite::{Connection, TransactionBehavior, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

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

/// Where migration approval evidence comes from.
///
/// Every schema crossing (v1 through v6 to the current version) is gated:
/// an old store never migrates implicitly on open.
#[derive(Clone, Debug)]
pub(crate) enum MigrationEvidenceSource {
    /// `LABBY_ACCESS_MIGRATION_EVIDENCE` names the approval document.
    Environment,
    /// An explicit approval document path (operator tooling and tests).
    Path(PathBuf),
    /// Unit migration fixtures exercising the transform itself, never a
    /// production-shaped store. Only constructible from tests.
    #[cfg(test)]
    UnitFixture,
}

pub(super) fn migrate(connection: &mut Connection) -> AccessStoreResult<()> {
    migrate_with_evidence(connection, &MigrationEvidenceSource::Environment)
}

pub(super) fn migrate_with_evidence(
    connection: &mut Connection,
    evidence: &MigrationEvidenceSource,
) -> AccessStoreResult<()> {
    let found = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(super::store::map_sqlite_error)?;
    if found > SCHEMA_VERSION {
        return Err(AccessStoreError::UnsupportedSchema {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    let migration_operation = if found > 0 && found < SCHEMA_VERSION {
        Some(require_migration_evidence(connection, found, evidence)?)
    } else {
        None
    };
    migrate_found(connection, found)?;
    if let Some(operation) = &migration_operation {
        complete_migration_operation(operation);
    }
    Ok(())
}

fn migrate_found(connection: &mut Connection, found: i64) -> AccessStoreResult<()> {
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
        install_v7_expansion(&transaction)?;
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
        install_v7_expansion(&transaction)?;
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
        install_v7_expansion(&transaction)?;
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
        install_v7_expansion(&transaction)?;
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
        install_v7_expansion(&transaction)?;
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
        install_v7_expansion(&transaction)?;
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
        install_v7_expansion(&transaction)?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .commit()
            .map_err(super::store::map_sqlite_error)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationEvidence {
    schema_version: String,
    operation_id: String,
    source_version: i64,
    target_version: i64,
    target_fingerprint: String,
    /// Retained for documents written by the earlier byte-identical contract;
    /// when present it must equal `checkpoint_sha256`. The live store is
    /// verified logically, never by file bytes, because a WAL store's main
    /// file legitimately differs from its consolidated checkpoint.
    #[serde(default)]
    source_sha256: Option<String>,
    checkpoint_path: PathBuf,
    checkpoint_sha256: String,
    activate: bool,
}

struct MigrationOperation {
    marker_path: Option<PathBuf>,
    operation_id: String,
    checkpoint_sha256: String,
}

fn invalid_evidence(reason: impl Into<String>) -> AccessStoreError {
    AccessStoreError::MigrationEvidenceInvalid {
        reason: reason.into(),
    }
}

fn require_migration_evidence(
    connection: &Connection,
    found: i64,
    source: &MigrationEvidenceSource,
) -> AccessStoreResult<MigrationOperation> {
    match source {
        MigrationEvidenceSource::Environment => {
            let evidence_path = std::env::var_os("LABBY_ACCESS_MIGRATION_EVIDENCE")
                .map(PathBuf::from)
                .ok_or(AccessStoreError::MigrationApprovalRequired { found })?;
            verify_migration_evidence(connection, found, &evidence_path)
        }
        MigrationEvidenceSource::Path(evidence_path) => {
            verify_migration_evidence(connection, found, evidence_path)
        }
        #[cfg(test)]
        MigrationEvidenceSource::UnitFixture => Ok(MigrationOperation {
            marker_path: None,
            operation_id: format!("unit-v{found}"),
            checkpoint_sha256: "unit-fixture".into(),
        }),
    }
}

fn verify_migration_evidence(
    connection: &Connection,
    found: i64,
    evidence_path: &Path,
) -> AccessStoreResult<MigrationOperation> {
    let bytes = std::fs::read(evidence_path)
        .map_err(|error| invalid_evidence(format!("cannot read evidence: {error}")))?;
    let evidence: MigrationEvidence = serde_json::from_slice(&bytes)
        .map_err(|error| invalid_evidence(format!("malformed evidence: {error}")))?;
    if evidence.schema_version != "labby.access-migration-approval/v1"
        || evidence.source_version != found
        || evidence.target_version != SCHEMA_VERSION
        || evidence.target_fingerprint != SCHEMA_FINGERPRINT
        || !evidence.activate
        || evidence.operation_id.trim().is_empty()
        || evidence.operation_id.len() > 96
    {
        return Err(invalid_evidence(
            "approval does not bind the source, target, fingerprint, operation, and activation decision",
        ));
    }
    if !lowercase_sha256(&evidence.checkpoint_sha256)
        || evidence
            .source_sha256
            .as_deref()
            .is_some_and(|source| source != evidence.checkpoint_sha256)
    {
        return Err(invalid_evidence(
            "checkpoint digest must be a lowercase SHA-256 that any source digest repeats",
        ));
    }
    let database_path = main_database_path(connection)?;
    let checkpoint = std::fs::canonicalize(&evidence.checkpoint_path)
        .map_err(|error| invalid_evidence(format!("cannot resolve checkpoint: {error}")))?;
    if checkpoint
        == std::fs::canonicalize(&database_path)
            .map_err(|error| invalid_evidence(error.to_string()))?
    {
        return Err(invalid_evidence(
            "checkpoint must be an independent rollback artifact",
        ));
    }
    let checkpoint_digest = sha256_file(&checkpoint)?;
    if checkpoint_digest != evidence.checkpoint_sha256 {
        return Err(invalid_evidence(
            "checkpoint digest does not match evidence",
        ));
    }
    // The checkpoint is a consolidated SQLite backup of the quiesced source.
    // Compare the two stores logically (schema manifest plus every table's
    // primary-key-ordered content) so a WAL-mode source with committed frames
    // still verifies against its `VACUUM INTO`/backup-API checkpoint.
    let checkpoint_connection = open_immutable(&checkpoint)?;
    let checkpoint_logical = logical_fingerprint(&checkpoint_connection)?;
    drop(checkpoint_connection);
    if logical_fingerprint(connection)? != checkpoint_logical {
        return Err(invalid_evidence(
            "live source does not logically match the approved checkpoint",
        ));
    }
    let marker_path = database_path.with_extension(format!("migration-v{SCHEMA_VERSION}.state"));
    let expected = format!(
        "prepared\noperation_id={}\ncheckpoint_sha256={}\n",
        evidence.operation_id, checkpoint_digest
    );
    match std::fs::read_to_string(&marker_path) {
        Ok(existing) if existing == expected => {}
        Ok(existing) if existing.starts_with("complete\n") => {
            return Err(invalid_evidence(
                "completed migration marker exists beside a legacy store",
            ));
        }
        Ok(_) => {
            return Err(invalid_evidence(
                "migration marker belongs to different evidence",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_marker(&marker_path, &expected)?;
        }
        Err(error) => {
            return Err(invalid_evidence(format!(
                "cannot read migration marker: {error}"
            )));
        }
    }
    Ok(MigrationOperation {
        marker_path: Some(marker_path),
        operation_id: evidence.operation_id,
        checkpoint_sha256: checkpoint_digest,
    })
}

fn lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn main_database_path(connection: &Connection) -> AccessStoreResult<PathBuf> {
    connection
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |row| row.get::<_, String>(0).map(PathBuf::from),
        )
        .map_err(super::store::map_sqlite_error)
}

fn open_immutable(path: &Path) -> AccessStoreResult<Connection> {
    let mut uri = url::Url::from_file_path(path)
        .map_err(|()| invalid_evidence("checkpoint path is not absolute"))?;
    uri.set_query(Some("immutable=1&mode=ro"));
    Connection::open_with_flags(
        uri.as_str(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| invalid_evidence(format!("cannot open checkpoint: {error}")))
}

/// SHA-256 over the schema manifest and every table's primary-key-ordered
/// content, encoded with value-type tags. Two stores with the same logical
/// state produce the same fingerprint regardless of page layout, WAL state,
/// or free-list contents.
pub(super) fn logical_fingerprint(connection: &Connection) -> AccessStoreResult<String> {
    use rusqlite::types::ValueRef;
    let mut digest = Sha256::new();
    for entry in schema_manifest(connection)? {
        let parts: [String; 4] = entry.into();
        for part in parts {
            digest.update(part.len().to_be_bytes());
            digest.update(part.as_bytes());
        }
    }
    let tables = connection
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .map_err(super::store::map_sqlite_error)?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(super::store::map_sqlite_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(super::store::map_sqlite_error)?;
    for table in tables {
        let quoted = format!("\"{}\"", table.replace('"', "\"\""));
        let mut info = connection
            .prepare(&format!("PRAGMA table_info({quoted})"))
            .map_err(super::store::map_sqlite_error)?;
        let mut keys = info
            .query_map([], |row| {
                Ok((row.get::<_, i64>(5)?, row.get::<_, String>(1)?))
            })
            .map_err(super::store::map_sqlite_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(super::store::map_sqlite_error)?
            .into_iter()
            .filter(|(position, _)| *position > 0)
            .collect::<Vec<_>>();
        keys.sort();
        let order = if keys.is_empty() {
            "rowid".to_owned()
        } else {
            keys.iter()
                .map(|(_, column)| format!("\"{}\"", column.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(",")
        };
        let mut statement = connection
            .prepare(&format!("SELECT * FROM {quoted} ORDER BY {order}"))
            .map_err(super::store::map_sqlite_error)?;
        let columns = statement.column_count();
        let mut rows = statement
            .query([])
            .map_err(super::store::map_sqlite_error)?;
        digest.update([0xfe]);
        digest.update(table.len().to_be_bytes());
        digest.update(table.as_bytes());
        while let Some(row) = rows.next().map_err(super::store::map_sqlite_error)? {
            digest.update([0xff]);
            for index in 0..columns {
                match row.get_ref(index).map_err(super::store::map_sqlite_error)? {
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
    }
    Ok(hex::encode(digest.finalize()))
}

/// Streamed SHA-256 of a file; the checkpoint is never read into memory whole.
pub(super) fn sha256_file(path: &Path) -> AccessStoreResult<String> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)
        .map_err(|error| invalid_evidence(format!("cannot read checkpoint: {error}")))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| invalid_evidence(format!("cannot read checkpoint: {error}")))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn write_marker(path: &Path, contents: &str) -> AccessStoreResult<()> {
    let temporary = path.with_extension(format!("migration-v{SCHEMA_VERSION}.state.tmp"));
    std::fs::write(&temporary, contents)
        .map_err(|error| invalid_evidence(format!("cannot write migration marker: {error}")))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| invalid_evidence(format!("cannot publish migration marker: {error}")))
}

/// Publish the completion marker after the migration transaction committed.
/// A marker failure here must not fail the (already durable) migration open;
/// it is logged so the operator can republish the marker by hand.
fn complete_migration_operation(operation: &MigrationOperation) {
    let Some(marker_path) = &operation.marker_path else {
        return;
    };
    if let Err(error) = write_marker(
        marker_path,
        &format!(
            "complete\noperation_id={}\ncheckpoint_sha256={}\ntarget_version={}\ntarget_fingerprint={}\n",
            operation.operation_id, operation.checkpoint_sha256, SCHEMA_VERSION, SCHEMA_FINGERPRINT
        ),
    ) {
        tracing::error!(
            surface = "access_store",
            operation_id = %operation.operation_id,
            marker = %marker_path.display(),
            error = %error,
            "access schema migration committed but its completion marker could not be published"
        );
    }
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

/// Versioned v7 expansion applied by every migration path and by fresh stores.
///
/// Everything a v7 store contains beyond the frozen v1-v5 domain schema, the
/// v6 Team authority schema, and the Dev Container ledger lives here so the
/// integrity manifest of a migrated store is byte-identical (after whitespace
/// normalization) to a freshly created one. Tables must never be created
/// lazily at runtime: the manifest check would classify the store as corrupt
/// on the next open.
pub(super) const AUTHORITY_OUTBOX_SCHEMA: &str = "
CREATE TABLE authority_outbox_sequences (
    organization_id TEXT PRIMARY KEY,
    next_sequence INTEGER NOT NULL CHECK(next_sequence > 0),
    acknowledged_sequence INTEGER NOT NULL DEFAULT 0 CHECK(acknowledged_sequence >= 0),
    acknowledged_digest TEXT,
    CHECK ((acknowledged_sequence = 0) = (acknowledged_digest IS NULL)),
    FOREIGN KEY (organization_id) REFERENCES organizations(organization_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE authority_projection_outbox (
    organization_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK(sequence > 0),
    event_id TEXT NOT NULL UNIQUE,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','inflight','sent','failed')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    envelope_digest TEXT,
    created_at INTEGER NOT NULL,
    sent_at INTEGER,
    PRIMARY KEY (organization_id, sequence),
    FOREIGN KEY (organization_id) REFERENCES organizations(organization_id) ON DELETE RESTRICT,
    FOREIGN KEY (event_id) REFERENCES access_audit(event_id) ON DELETE RESTRICT
) STRICT;
CREATE INDEX authority_projection_outbox_delivery
    ON authority_projection_outbox(status,next_attempt_at,organization_id,sequence);

CREATE TRIGGER enqueue_authority_projection_after_audit
AFTER INSERT ON access_audit
WHEN NEW.decision='allow'
BEGIN
  INSERT OR IGNORE INTO authority_outbox_sequences(organization_id,next_sequence)
  VALUES(NEW.organization_id,1);
  INSERT INTO authority_projection_outbox(
    organization_id,sequence,event_id,payload_json,status,attempt_count,next_attempt_at,created_at)
  SELECT NEW.organization_id,next_sequence,NEW.event_id,
    json_object('event_id',NEW.event_id,'action',NEW.action,'target_kind',NEW.target_kind),
    'pending',0,0,NEW.occurred_at
  FROM authority_outbox_sequences WHERE organization_id=NEW.organization_id;
  UPDATE authority_outbox_sequences SET next_sequence=next_sequence+1
  WHERE organization_id=NEW.organization_id;
END;
";

/// Durable Agent definitions, sessions, and Task ledger.
pub(super) const AGENT_TASK_SCHEMA: &str = "
CREATE TABLE agent_definitions (
    agent_id TEXT PRIMARY KEY,
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('installation','team','project','personal')),
    owner_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK(version > 0),
    definition_json TEXT NOT NULL CHECK(json_valid(definition_json)),
    state TEXT NOT NULL CHECK(state IN ('active','suspended','deleted')),
    authority_epoch INTEGER NOT NULL CHECK(authority_epoch >= 0),
    publication_epoch INTEGER NOT NULL CHECK(publication_epoch >= 0),
    updated_at INTEGER NOT NULL
);
CREATE INDEX agent_definitions_owner
    ON agent_definitions(owner_kind,owner_id,state,agent_id);
CREATE TABLE agent_definition_audit (
    event_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    actor_principal_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('create','update','suspend','delete')),
    authority_epoch INTEGER NOT NULL,
    occurred_at INTEGER NOT NULL,
    FOREIGN KEY (agent_id) REFERENCES agent_definitions(agent_id)
);
CREATE TABLE agent_sessions (
    session_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    agent_version INTEGER NOT NULL,
    principal_id TEXT NOT NULL,
    authority_fingerprint TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('admitted','running','completed','failed','cancelled','revoked','interrupted')),
    lease_expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (agent_id) REFERENCES agent_definitions(agent_id)
);
CREATE INDEX agent_sessions_agent ON agent_sessions(agent_id,created_at,session_id);
CREATE TABLE agent_tasks (
    task_id TEXT PRIMARY KEY,
    idempotency_key TEXT NOT NULL,
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('installation','team','project','personal')),
    owner_id TEXT NOT NULL,
    project_id TEXT,
    creator_principal_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    agent_version INTEGER NOT NULL CHECK(agent_version > 0),
    agent_revision_digest TEXT NOT NULL,
    input_digest TEXT NOT NULL,
    catalog_generation TEXT NOT NULL,
    authority_fingerprint TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('created','queued','running','cancelling','succeeded','failed','cancelled','expired')),
    attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt >= 0),
    fencing_token TEXT,
    lease_expires_at INTEGER,
    output_digest TEXT,
    error_code TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (owner_kind,owner_id,idempotency_key)
);
CREATE INDEX agent_tasks_owner_state ON agent_tasks(owner_kind,owner_id,state,task_id);
CREATE TABLE agent_task_audit (
    event_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    actor_principal_id TEXT NOT NULL,
    from_state TEXT,
    to_state TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    occurred_at INTEGER NOT NULL,
    FOREIGN KEY (task_id) REFERENCES agent_tasks(task_id) ON DELETE CASCADE
);
";

/// Secret-free Team credential bindings for Gateway loadouts.
pub(super) const GATEWAY_CREDENTIAL_SCHEMA: &str = "
CREATE TABLE gateway_team_credential_bindings (
  binding_id TEXT PRIMARY KEY CHECK(length(trim(binding_id)) BETWEEN 1 AND 256),
  team_id TEXT NOT NULL CHECK(length(trim(team_id)) BETWEEN 1 AND 256),
  upstream_name TEXT NOT NULL CHECK(length(trim(upstream_name)) BETWEEN 1 AND 256),
  custodian_principal_id TEXT NOT NULL
    CHECK(length(trim(custodian_principal_id)) BETWEEN 1 AND 256),
  generation INTEGER NOT NULL CHECK(generation > 0),
  rotated_at_millis INTEGER NOT NULL CHECK(rotated_at_millis > 0),
  status TEXT NOT NULL CHECK(status IN ('active','revoked')),
  revoked_at_millis INTEGER,
  CHECK ((status = 'revoked') = (revoked_at_millis IS NOT NULL)),
  UNIQUE(team_id, upstream_name)
) STRICT;
CREATE INDEX gateway_team_credential_bindings_team
  ON gateway_team_credential_bindings(team_id,status,upstream_name);
";

/// Monotonic authority epochs for Principals and direct Project memberships.
///
/// They live beside the frozen domain tables and are maintained by triggers,
/// so every mutation path (including raw statements) advances the epoch in
/// the same transaction as the mutation. Display-label and timestamp edits do
/// not change authority and therefore do not bump an epoch.
pub(super) const AUTHORITY_EPOCH_SCHEMA: &str = "
CREATE TABLE principal_epochs (
    principal_id TEXT PRIMARY KEY,
    epoch INTEGER NOT NULL CHECK(epoch > 0),
    FOREIGN KEY (principal_id) REFERENCES principals(principal_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE project_membership_epochs (
    membership_id TEXT PRIMARY KEY,
    epoch INTEGER NOT NULL CHECK(epoch > 0),
    FOREIGN KEY (membership_id) REFERENCES project_memberships(membership_id) ON DELETE CASCADE
) STRICT;
CREATE TRIGGER principal_epochs_after_insert
AFTER INSERT ON principals
BEGIN
  INSERT INTO principal_epochs(principal_id,epoch) VALUES(NEW.principal_id,1);
END;
CREATE TRIGGER principal_epochs_after_update
AFTER UPDATE OF organization_id, kind, status ON principals
BEGIN
  UPDATE principal_epochs SET epoch=epoch+1 WHERE principal_id=NEW.principal_id;
END;
CREATE TRIGGER principal_epochs_after_link_update
AFTER UPDATE OF status, link_generation, principal_id ON principal_links
BEGIN
  UPDATE principal_epochs SET epoch=epoch+1
  WHERE principal_id IN (OLD.principal_id, NEW.principal_id);
END;
CREATE TRIGGER project_membership_epochs_after_insert
AFTER INSERT ON project_memberships
BEGIN
  INSERT INTO project_membership_epochs(membership_id,epoch) VALUES(NEW.membership_id,1);
END;
CREATE TRIGGER project_membership_epochs_after_update
AFTER UPDATE OF organization_id, project_id, principal_id, role, status ON project_memberships
BEGIN
  UPDATE project_membership_epochs SET epoch=epoch+1 WHERE membership_id=NEW.membership_id;
END;
";

fn install_v7_expansion(connection: &Connection) -> AccessStoreResult<()> {
    for schema in [
        AUTHORITY_OUTBOX_SCHEMA,
        AGENT_TASK_SCHEMA,
        GATEWAY_CREDENTIAL_SCHEMA,
        AUTHORITY_EPOCH_SCHEMA,
    ] {
        connection
            .execute_batch(schema)
            .map_err(super::store::map_sqlite_error)?;
    }
    // Existing rows predate the epoch triggers; seed them at epoch 1 so the
    // epoch tables are complete before any authority read joins against them.
    connection
        .execute_batch(
            "INSERT INTO principal_epochs(principal_id,epoch)
               SELECT principal_id,1 FROM principals
               WHERE principal_id NOT IN (SELECT principal_id FROM principal_epochs);
             INSERT INTO project_membership_epochs(membership_id,epoch)
               SELECT membership_id,1 FROM project_memberships
               WHERE membership_id NOT IN (SELECT membership_id FROM project_membership_epochs);",
        )
        .map_err(super::store::map_sqlite_error)
}

/// In-memory connection holding the exact current schema. Both integrity
/// validation and migration tests compare manifests against this.
pub(super) fn canonical_current_schema() -> AccessStoreResult<Connection> {
    let connection = Connection::open_in_memory().map_err(super::store::map_sqlite_error)?;
    for schema in [
        SCHEMA_V2_METADATA,
        DOMAIN_SCHEMA,
        TEAM_AUTHORITY_SCHEMA,
        super::dev_container::DEV_CONTAINER_SCHEMA,
    ] {
        connection
            .execute_batch(schema)
            .map_err(super::store::map_sqlite_error)?;
    }
    install_v7_expansion(&connection)?;
    Ok(connection)
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

    fn snapshot_into(connection: &Connection, path: &Path) {
        connection
            .execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])
            .unwrap();
    }

    #[test]
    fn migration_evidence_binds_checkpoint_and_replay_operation() {
        let directory = super::super::test_support::secure_tempdir();
        let database_path = directory.path().join("access-v5.db");
        let checkpoint_path = directory.path().join("access-v5.checkpoint.db");
        let evidence_path = directory.path().join("approval.json");
        let source = canonical_v5_schema().unwrap();
        snapshot_into(&source, &database_path);
        std::fs::copy(&database_path, &checkpoint_path).unwrap();
        let checkpoint_sha256 = sha256_file(&checkpoint_path).unwrap();
        let write_evidence = |operation_id: &str| {
            std::fs::write(
                &evidence_path,
                serde_json::to_vec(&serde_json::json!({
                    "schema_version": "labby.access-migration-approval/v1",
                    "operation_id": operation_id,
                    "source_version": 5,
                    "target_version": 7,
                    "target_fingerprint": SCHEMA_FINGERPRINT,
                    "source_sha256": checkpoint_sha256,
                    "checkpoint_path": checkpoint_path,
                    "checkpoint_sha256": checkpoint_sha256,
                    "activate": true
                }))
                .unwrap(),
            )
            .unwrap();
        };
        write_evidence("operation-one");
        let connection = Connection::open(&database_path).unwrap();
        let operation = verify_migration_evidence(&connection, 5, &evidence_path).unwrap();
        assert!(operation.marker_path.as_ref().unwrap().exists());
        write_evidence("operation-two");
        assert!(matches!(
            verify_migration_evidence(&connection, 5, &evidence_path),
            Err(AccessStoreError::MigrationEvidenceInvalid { .. })
        ));
    }

    fn write_approval(
        evidence_path: &Path,
        checkpoint_path: &Path,
        source_version: i64,
        operation_id: &str,
    ) {
        let checkpoint_sha256 = sha256_file(checkpoint_path).unwrap();
        std::fs::write(
            evidence_path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": "labby.access-migration-approval/v1",
                "operation_id": operation_id,
                "source_version": source_version,
                "target_version": SCHEMA_VERSION,
                "target_fingerprint": SCHEMA_FINGERPRINT,
                "checkpoint_path": checkpoint_path,
                "checkpoint_sha256": checkpoint_sha256,
                "activate": true
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn every_legacy_version_is_gated_and_the_gate_runs_end_to_end() {
        // Each supported legacy version is refused without evidence, before
        // any transform runs; nothing crosses implicitly.
        let fixtures: [(i64, fn() -> Connection); 4] = [
            (V2_SCHEMA_VERSION, canonical_v2),
            (V3_SCHEMA_VERSION, canonical_v3),
            (V4_SCHEMA_VERSION, canonical_v4),
            (V5_SCHEMA_VERSION, canonical_v5),
        ];
        for (version, fixture) in fixtures {
            let mut connection = fixture();
            assert!(matches!(
                migrate_with_evidence(
                    &mut connection,
                    &MigrationEvidenceSource::Path(PathBuf::from("/nonexistent/approval.json"))
                ),
                Err(AccessStoreError::MigrationEvidenceInvalid { .. })
            ));
            assert_eq!(
                connection
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                version
            );
        }

        // End to end: a WAL-mode v5 store whose committed rows still live in
        // its WAL (so its main file bytes differ from the consolidated
        // checkpoint) migrates with a logically bound approval document, and
        // the completion marker is published after commit.
        let directory = super::super::test_support::secure_tempdir();
        let database_path = directory.path().join("access.db");
        let checkpoint_path = directory.path().join("access.checkpoint.db");
        let evidence_path = directory.path().join("approval.json");
        let source = production_shaped_v5();
        snapshot_into(&source, &database_path);
        drop(source);
        let mut live = Connection::open(&database_path).unwrap();
        live.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        live.execute(
            "INSERT INTO projects VALUES('project-gamma','org-production','Gamma','active',1,300,300)",
            [],
        )
        .unwrap();
        // Consolidated checkpoint of the quiesced source (includes WAL frames).
        snapshot_into(&live, &checkpoint_path);
        assert_ne!(
            sha256_file(&database_path).unwrap(),
            sha256_file(&checkpoint_path).unwrap(),
            "the WAL-diverged main file must not be required to match the checkpoint byte-for-byte"
        );
        assert!(matches!(
            migrate_with_evidence(&mut live, &MigrationEvidenceSource::Environment),
            Err(AccessStoreError::MigrationApprovalRequired { found: 5 })
        ));
        write_approval(
            &evidence_path,
            &checkpoint_path,
            V5_SCHEMA_VERSION,
            "gate-e2e",
        );
        live.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrate_with_evidence(
            &mut live,
            &MigrationEvidenceSource::Path(evidence_path.clone()),
        )
        .unwrap();
        super::super::integrity::validate(&live).unwrap();
        let marker = std::fs::read_to_string(
            database_path.with_extension(format!("migration-v{SCHEMA_VERSION}.state")),
        )
        .unwrap();
        assert!(marker.starts_with("complete\noperation_id=gate-e2e\n"));
        assert_eq!(
            live.query_row("SELECT count(*) FROM projects", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            2
        );

        // A checkpoint that does not logically match the live source is refused.
        let other_directory = super::super::test_support::secure_tempdir();
        let other_path = other_directory.path().join("access.db");
        let stale_checkpoint = other_directory.path().join("stale.db");
        let stale_evidence = other_directory.path().join("approval.json");
        let source = production_shaped_v5();
        snapshot_into(&source, &stale_checkpoint);
        source
            .execute(
                "INSERT INTO projects VALUES('project-delta','org-production','Delta','active',1,301,301)",
                [],
            )
            .unwrap();
        snapshot_into(&source, &other_path);
        drop(source);
        let mut diverged = Connection::open(&other_path).unwrap();
        write_approval(
            &stale_evidence,
            &stale_checkpoint,
            V5_SCHEMA_VERSION,
            "stale",
        );
        assert!(matches!(
            migrate_with_evidence(&mut diverged, &MigrationEvidenceSource::Path(stale_evidence)),
            Err(AccessStoreError::MigrationEvidenceInvalid { reason })
                if reason.contains("logically")
        ));
        assert_eq!(
            diverged
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            V5_SCHEMA_VERSION
        );
    }

    #[test]
    fn fresh_database_contains_bounded_credential_schema() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrate_with_evidence(&mut connection, &MigrationEvidenceSource::UnitFixture).unwrap();
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
        migrate_with_evidence(&mut connection, &MigrationEvidenceSource::UnitFixture).unwrap();
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
        migrate_with_evidence(&mut connection, &MigrationEvidenceSource::UnitFixture).unwrap();
        let canonical = canonical_current_schema().unwrap();
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
        migrate_with_evidence(&mut connection, &MigrationEvidenceSource::UnitFixture).unwrap();
        let expected = canonical_current_schema().unwrap();
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

        migrate_with_evidence(&mut connection, &MigrationEvidenceSource::UnitFixture).unwrap();

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

        migrate_with_evidence(&mut connection, &MigrationEvidenceSource::UnitFixture).unwrap();
        super::super::integrity::validate(&connection).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM sqlite_schema
                     WHERE name IN ('authority_outbox_sequences',
                                    'authority_projection_outbox',
                                    'enqueue_authority_projection_after_audit')",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            3,
            "a v5 upgrade must install the complete v7 authority projection outbox",
        );
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
        migrate_with_evidence(&mut migrated, &MigrationEvidenceSource::UnitFixture).unwrap();
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
        migrate_with_evidence(&mut migrated, &MigrationEvidenceSource::UnitFixture).unwrap();
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
            migrate_with_evidence(&mut malformed, &MigrationEvidenceSource::UnitFixture),
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
            migrate_with_evidence(&mut read_only, &MigrationEvidenceSource::UnitFixture),
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
            migrate_with_evidence(&mut connection, &MigrationEvidenceSource::UnitFixture),
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
        migrate_with_evidence(&mut connection, &MigrationEvidenceSource::UnitFixture).unwrap();
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
            migrate_with_evidence(&mut connection, &MigrationEvidenceSource::UnitFixture),
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
        migrate_with_evidence(&mut connection, &MigrationEvidenceSource::UnitFixture).unwrap();
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

    #[test]
    fn migration_classifies_existing_rows_as_installation_or_bootstrap_personal_never_team() {
        let mut connection = production_shaped_v5();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrate_with_evidence(&mut connection, &MigrationEvidenceSource::UnitFixture).unwrap();
        super::super::integrity::validate(&connection).unwrap();
        // The production-shaped v5 fixture has bootstrap generation 0: no
        // bootstrap Principal exists, so no platform administrator or Team is
        // seeded and no pre-existing Project is assigned to any Team.
        let (administrators, teams, memberships, assignments): (i64, i64, i64, i64) = connection
            .query_row(
                "SELECT (SELECT count(*) FROM platform_administrators),
                        (SELECT count(*) FROM groups),
                        (SELECT count(*) FROM team_memberships),
                        (SELECT count(*) FROM team_project_assignments)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (administrators, teams, memberships, assignments),
            (0, 0, 0, 0)
        );
        // Direct Project memberships survive as-is and receive epoch 1.
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM project_memberships m JOIN project_membership_epochs e ON e.membership_id=m.membership_id WHERE e.epoch=1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM principals p JOIN principal_epochs e ON e.principal_id=p.principal_id WHERE e.epoch=1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
    }
}
