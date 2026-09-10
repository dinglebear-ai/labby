use rusqlite::{Connection, TransactionBehavior, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use super::error::{AccessStoreError, AccessStoreResult};

use super::credential_schema;

pub(super) const SCHEMA_VERSION: i64 = 7;
const MAX_MIGRATION_EVIDENCE_BYTES: usize = 128 * 1024;
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
    use std::io::Read as _;

    let file = std::fs::File::open(evidence_path)
        .map_err(|error| invalid_evidence(format!("cannot read evidence: {error}")))?;
    let mut bytes = Vec::with_capacity(MAX_MIGRATION_EVIDENCE_BYTES.min(4096));
    file.take((MAX_MIGRATION_EVIDENCE_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| invalid_evidence(format!("cannot read evidence: {error}")))?;
    if bytes.len() > MAX_MIGRATION_EVIDENCE_BYTES {
        return Err(invalid_evidence(
            "migration approval evidence exceeds the size limit",
        ));
    }
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
    let temporary = path.with_extension(format!("migration-v{SCHEMA_V