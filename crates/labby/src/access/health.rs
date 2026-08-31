use std::path::{Path, PathBuf};

use rusqlite::{Connection, ErrorCode, OpenFlags};

/// Stable, agent-safe classification of the on-disk access store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccessHealthStatus {
    Missing,
    Uninitialized,
    Prepared,
    Ready,
    Insecure,
    Corrupt,
    NewerSchema,
    Locked,
    ReadOnly,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AccessHealth {
    pub(crate) status: AccessHealthStatus,
    /// A stable recovery-oriented code. It deliberately contains no path, SQL, or raw error text.
    pub(crate) detail: &'static str,
}

impl AccessHealth {
    const fn new(status: AccessHealthStatus, detail: &'static str) -> Self {
        Self { status, detail }
    }
}

/// Inspects an access store without creating files, migrating schemas, changing pragmas, or
/// acquiring a write transaction.
pub(crate) fn inspect_health(path: &Path) -> AccessHealth {
    if !valid_store_path(path) {
        return AccessHealth::new(AccessHealthStatus::Insecure, "use_secure_access_store_path");
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A missing store is observational setup state. Creation still
            // validates and, where appropriate, creates the restrictive owned
            // parent before writing anything.
            return AccessHealth::new(AccessHealthStatus::Missing, "initialize_access_store");
        }
        Err(error) => {
            warn_health_io("store_metadata", &error);
            return AccessHealth::new(AccessHealthStatus::Unavailable, "check_access_store_file");
        }
    };
    let Some(parent) = path.parent() else {
        return AccessHealth::new(AccessHealthStatus::Insecure, "use_secure_access_store_path");
    };
    match std::fs::symlink_metadata(parent) {
        Ok(metadata) if secure_parent(&metadata) => {}
        Ok(_) => {
            return AccessHealth::new(AccessHealthStatus::Insecure, "secure_access_store_parent");
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return AccessHealth::new(AccessHealthStatus::Missing, "initialize_access_store");
        }
        Err(error) => {
            warn_health_io("parent_metadata", &error);
            return AccessHealth::new(AccessHealthStatus::Unavailable, "check_access_store_parent");
        }
    }

    if !secure_store_file(&metadata) {
        return AccessHealth::new(AccessHealthStatus::Insecure, "secure_access_store_file");
    }
    if store_is_read_only(&metadata) {
        return AccessHealth::new(AccessHealthStatus::ReadOnly, "make_access_store_writable");
    }
    let sidecars = match capture_sidecars(path) {
        Ok(sidecars) => sidecars,
        Err(health) => return health,
    };
    let has_wal = sidecars.iter().any(|sidecar| sidecar.suffix == "-wal");
    let has_shm = sidecars.iter().any(|sidecar| sidecar.suffix == "-shm");
    let has_journal = sidecars.iter().any(|sidecar| sidecar.suffix == "-journal");
    if has_wal != has_shm {
        return AccessHealth::new(AccessHealthStatus::Locked, "retry_access_store_check");
    }

    // With no sidecars immutable mode prevents their creation. A live WAL store instead uses a
    // normal read-only snapshot so SQLite honors WAL and locking. DB/WAL are exact snapshots;
    // SQLite may update the existing SHM lock region, so only its identity/security are stable.
    let snapshot = file_snapshot(&metadata);
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let connection = match if sidecars.is_empty() {
        immutable_uri(path).and_then(|uri| {
            Connection::open_with_flags(uri.as_str(), flags | OpenFlags::SQLITE_OPEN_URI)
                .map_err(|error| classify_sqlite_error(&error))
        })
    } else {
        canonical_store_path(path).and_then(|canonical_path| {
            Connection::open_with_flags(canonical_path, flags)
                .map_err(|error| classify_sqlite_error(&error))
        })
    } {
        Ok(connection) => connection,
        Err(health) => return health,
    };
    if let Err(error) = connection.busy_timeout(std::time::Duration::from_millis(50)) {
        tracing::warn!(
            surface = "access_health",
            operation = "configure_busy_timeout",
            sqlite_code = ?error.sqlite_error_code(),
            "access store health inspection failed"
        );
        return AccessHealth::new(AccessHealthStatus::Unavailable, "check_access_store");
    }
    if !same_snapshot(path, snapshot) {
        return AccessHealth::new(AccessHealthStatus::Locked, "retry_access_store_check");
    }
    if !same_sidecars(path, &sidecars) {
        return AccessHealth::new(AccessHealthStatus::Locked, "retry_access_store_check");
    }
    let health = inspect_connection(&connection);
    drop(connection);
    if !same_snapshot(path, snapshot) {
        return AccessHealth::new(AccessHealthStatus::Locked, "retry_access_store_check");
    }
    if !same_sidecars(path, &sidecars) {
        return AccessHealth::new(AccessHealthStatus::Locked, "retry_access_store_check");
    }
    if has_journal {
        return AccessHealth::new(AccessHealthStatus::Locked, "retry_access_store_check");
    }
    health
}

fn immutable_uri(path: &Path) -> Result<url::Url, AccessHealth> {
    let canonical_path = canonical_store_path(path)?;
    let mut uri = url::Url::from_file_path(canonical_path).map_err(|()| {
        AccessHealth::new(AccessHealthStatus::Insecure, "use_secure_access_store_path")
    })?;
    uri.query_pairs_mut().append_pair("immutable", "1");
    Ok(uri)
}

fn canonical_store_path(path: &Path) -> Result<PathBuf, AccessHealth> {
    let parent = path.parent().ok_or_else(|| {
        AccessHealth::new(AccessHealthStatus::Insecure, "use_secure_access_store_path")
    })?;
    let canonical_path = std::fs::canonicalize(parent)
        .map_err(|_| {
            AccessHealth::new(AccessHealthStatus::Unavailable, "check_access_store_parent")
        })?
        .join("access.db");
    Ok(canonical_path)
}

fn valid_store_path(path: &Path) -> bool {
    path.is_absolute() && path.file_name().is_some_and(|name| name == "access.db")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SidecarSnapshot {
    suffix: &'static str,
    snapshot: FileSnapshot,
}

fn capture_sidecars(path: &Path) -> Result<Vec<SidecarSnapshot>, AccessHealth> {
    let mut snapshots = Vec::new();
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = sidecar_path(path, suffix);
        match std::fs::symlink_metadata(sidecar) {
            Ok(metadata) if !secure_store_file(&metadata) => {
                return Err(AccessHealth::new(
                    AccessHealthStatus::Insecure,
                    "secure_access_store_sidecars",
                ));
            }
            Ok(metadata) => snapshots.push(SidecarSnapshot {
                suffix,
                snapshot: file_snapshot(&metadata),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                warn_health_io("sidecar_metadata", &error);
                return Err(AccessHealth::new(
                    AccessHealthStatus::Unavailable,
                    "check_access_store_sidecars",
                ));
            }
        }
    }
    Ok(snapshots)
}

fn warn_health_io(operation: &'static str, error: &std::io::Error) {
    tracing::warn!(
        surface = "access_health",
        operation,
        error_kind = ?error.kind(),
        raw_os_error = error.raw_os_error(),
        "access store health inspection failed"
    );
}

fn same_sidecars(path: &Path, expected: &[SidecarSnapshot]) -> bool {
    let Ok(actual) = capture_sidecars(path) else {
        return false;
    };
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(actual, expected)| {
            actual.suffix == expected.suffix
                && if actual.suffix == "-shm" {
                    actual.snapshot.same_coordination_file(expected.snapshot)
                } else {
                    actual.snapshot == expected.snapshot
                }
        })
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn inspect_connection(connection: &Connection) -> AccessHealth {
    let version = match connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
    {
        Ok(version) => version,
        Err(error) => return classify_sqlite_error(&error),
    };
    if version > super::migrations::SCHEMA_VERSION {
        return AccessHealth::new(AccessHealthStatus::NewerSchema, "upgrade_labby");
    }
    if version == super::migrations::V1_SCHEMA_VERSION {
        return match super::integrity::validate_v1_before_migration(connection) {
            Ok(()) => AccessHealth::new(
                AccessHealthStatus::Uninitialized,
                "initialize_or_migrate_access_store",
            ),
            Err(_) => AccessHealth::new(AccessHealthStatus::Corrupt, "repair_access_store"),
        };
    }
    if version == 0 {
        return AccessHealth::new(AccessHealthStatus::Uninitialized, "initialize_access_store");
    }
    if version != super::migrations::SCHEMA_VERSION {
        return AccessHealth::new(AccessHealthStatus::Corrupt, "repair_access_store");
    }
    match super::integrity::validate(connection) {
        Ok(()) => {
            let generation = connection.query_row(
                "SELECT bootstrap_generation FROM access_metadata WHERE singleton = 1",
                [],
                |row| row.get::<_, i64>(0),
            );
            match generation {
                Ok(0) => match connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM bootstrap_proofs WHERE status = 'active')",
                    [],
                    |row| row.get::<_, bool>(0),
                ) {
                    Ok(true) => {
                        AccessHealth::new(AccessHealthStatus::Prepared, "consume_bootstrap_proof")
                    }
                    Ok(false) => AccessHealth::new(
                        AccessHealthStatus::Uninitialized,
                        "bootstrap_access_store_owner",
                    ),
                    Err(error) => classify_sqlite_error(&error),
                },
                Ok(1) => AccessHealth::new(AccessHealthStatus::Ready, "ready"),
                Ok(_) => AccessHealth::new(AccessHealthStatus::Corrupt, "repair_access_store"),
                Err(error) => classify_sqlite_error(&error),
            }
        }
        Err(super::error::AccessStoreError::Locked) => {
            AccessHealth::new(AccessHealthStatus::Locked, "retry_access_store_check")
        }
        Err(super::error::AccessStoreError::ReadOnly) => {
            AccessHealth::new(AccessHealthStatus::ReadOnly, "make_access_store_writable")
        }
        Err(
            super::error::AccessStoreError::Corrupt
            | super::error::AccessStoreError::IntegrityViolation { .. }
            | super::error::AccessStoreError::ForeignKeyViolation,
        ) => AccessHealth::new(AccessHealthStatus::Corrupt, "repair_access_store"),
        Err(_) => AccessHealth::new(AccessHealthStatus::Unavailable, "check_access_store"),
    }
}

fn classify_sqlite_error(error: &rusqlite::Error) -> AccessHealth {
    let Some(failure) = error.sqlite_error() else {
        return AccessHealth::new(AccessHealthStatus::Unavailable, "check_access_store");
    };
    match failure.code {
        ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => {
            AccessHealth::new(AccessHealthStatus::Locked, "retry_access_store_check")
        }
        ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => {
            AccessHealth::new(AccessHealthStatus::Corrupt, "repair_access_store")
        }
        ErrorCode::ReadOnly => {
            AccessHealth::new(AccessHealthStatus::ReadOnly, "make_access_store_writable")
        }
        _ => AccessHealth::new(AccessHealthStatus::Unavailable, "check_access_store"),
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileSnapshot {
    dev: u64,
    ino: u64,
    len: u64,
    uid: u32,
    links: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
    mode: u32,
}

#[cfg(unix)]
fn file_snapshot(metadata: &std::fs::Metadata) -> FileSnapshot {
    use std::os::unix::fs::MetadataExt as _;
    FileSnapshot {
        dev: metadata.dev(),
        ino: metadata.ino(),
        len: metadata.len(),
        uid: metadata.uid(),
        links: metadata.nlink(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
        mode: metadata.mode(),
    }
}

#[cfg(unix)]
impl FileSnapshot {
    fn same_coordination_file(self, other: Self) -> bool {
        self.dev == other.dev
            && self.ino == other.ino
            && self.len == other.len
            && self.uid == other.uid
            && self.links == other.links
            && self.mode == other.mode
    }
}

#[cfg(unix)]
fn same_snapshot(path: &Path, expected: FileSnapshot) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| secure_store_file(&metadata) && file_snapshot(&metadata) == expected)
        .unwrap_or(false)
}

#[cfg(not(unix))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileSnapshot {
    len: u64,
    modified: std::time::SystemTime,
    read_only: bool,
}

#[cfg(not(unix))]
impl FileSnapshot {
    fn same_coordination_file(self, other: Self) -> bool {
        self.len == other.len && self.read_only == other.read_only
    }
}

#[cfg(not(unix))]
fn file_snapshot(metadata: &std::fs::Metadata) -> FileSnapshot {
    FileSnapshot {
        len: metadata.len(),
        modified: metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
        read_only: metadata.permissions().readonly(),
    }
}

#[cfg(not(unix))]
fn same_snapshot(path: &Path, expected: FileSnapshot) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| secure_store_file(&metadata) && file_snapshot(&metadata) == expected)
        .unwrap_or(false)
}

#[cfg(unix)]
fn secure_parent(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    metadata.is_dir()
        && !metadata.file_type().is_symlink()
        && metadata.uid() == nix::unistd::geteuid().as_raw()
        && metadata.permissions().mode().trailing_zeros() >= 6
}

#[cfg(not(unix))]
fn secure_parent(metadata: &std::fs::Metadata) -> bool {
    metadata.is_dir() && !metadata.file_type().is_symlink()
}

#[cfg(unix)]
fn secure_store_file(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata.uid() == nix::unistd::geteuid().as_raw()
        && metadata.nlink() == 1
        && metadata.permissions().mode().trailing_zeros() >= 6
}

#[cfg(not(unix))]
fn secure_store_file(metadata: &std::fs::Metadata) -> bool {
    metadata.is_file() && !metadata.file_type().is_symlink()
}

#[cfg(unix)]
fn store_is_read_only(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o200 == 0
}

#[cfg(not(unix))]
fn store_is_read_only(metadata: &std::fs::Metadata) -> bool {
    metadata.permissions().readonly()
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[tokio::test]
    async fn bootstrapped_store_is_ready() {
        use labby_auth::{Authenticator, VerifiedIdentity};

        let directory = super::super::test_support::secure_tempdir();
        let path = secure_path(&directory);
        let store = super::super::store::AccessStore::open(path.clone())
            .await
            .unwrap();
        let input = super::super::bootstrap::BootstrapOwnerInput::new(
            VerifiedIdentity::local_credential(
                Authenticator::StaticBearer,
                "static-bearer:health-test",
            )
            .unwrap(),
            "Local",
            "Default",
        )
        .unwrap();
        store.bootstrap_owner(input).await.unwrap();
        drop(store);

        assert_eq!(
            inspect_health(&path),
            AccessHealth::new(AccessHealthStatus::Ready, "ready")
        );
    }

    fn secure_path(directory: &tempfile::TempDir) -> PathBuf {
        #[cfg(unix)]
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        directory.path().join("access.db")
    }

    #[test]
    fn missing_store_is_observational() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_path(&directory);

        assert_eq!(
            inspect_health(&path),
            AccessHealth::new(AccessHealthStatus::Missing, "initialize_access_store")
        );
        assert!(!path.exists());
        assert!(!sidecar_path(&path, "-wal").exists());
        assert!(!sidecar_path(&path, "-shm").exists());
        assert!(!sidecar_path(&path, "-journal").exists());
    }

    #[tokio::test]
    async fn current_schema_inspection_is_exactly_observational() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_path(&directory);
        let store = super::super::store::AccessStore::open(path.clone())
            .await
            .unwrap();
        drop(store);
        assert!(!sidecar_path(&path, "-wal").exists());
        assert!(!sidecar_path(&path, "-shm").exists());

        let before_bytes = std::fs::read(&path).unwrap();
        let before_metadata = std::fs::metadata(&path).unwrap();
        let mut before_entries = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        before_entries.sort();

        assert_eq!(
            inspect_health(&path),
            AccessHealth::new(
                AccessHealthStatus::Uninitialized,
                "bootstrap_access_store_owner"
            )
        );

        let after_metadata = std::fs::metadata(&path).unwrap();
        let mut after_entries = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        after_entries.sort();
        assert_eq!(after_entries, before_entries);
        assert_eq!(std::fs::read(&path).unwrap(), before_bytes);
        assert_eq!(after_metadata.len(), before_metadata.len());
        assert_eq!(
            after_metadata.modified().unwrap(),
            before_metadata.modified().unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(after_metadata.dev(), before_metadata.dev());
            assert_eq!(after_metadata.ino(), before_metadata.ino());
            assert_eq!(after_metadata.mode(), before_metadata.mode());
        }
        assert!(!sidecar_path(&path, "-wal").exists());
        assert!(!sidecar_path(&path, "-shm").exists());
        assert!(!sidecar_path(&path, "-journal").exists());
    }

    #[test]
    fn v1_store_is_not_migrated_or_mutated() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_path(&directory);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(super::super::migrations::V1_METADATA_SCHEMA)
            .unwrap();
        connection
            .execute_batch(super::super::migrations::DOMAIN_SCHEMA)
            .unwrap();
        connection
            .execute(
                "INSERT INTO access_metadata VALUES (1, ?1, ?2, 7, unixepoch())",
                rusqlite::params![
                    super::super::migrations::V1_SCHEMA_VERSION,
                    super::super::migrations::V1_SCHEMA_FINGERPRINT
                ],
            )
            .unwrap();
        connection
            .pragma_update(
                None,
                "application_id",
                super::super::migrations::APPLICATION_ID,
            )
            .unwrap();
        connection
            .pragma_update(
                None,
                "user_version",
                super::super::migrations::V1_SCHEMA_VERSION,
            )
            .unwrap();
        drop(connection);
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let before = std::fs::read(&path).unwrap();

        assert_eq!(
            inspect_health(&path),
            AccessHealth::new(
                AccessHealthStatus::Uninitialized,
                "initialize_or_migrate_access_store"
            )
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let connection =
            Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            super::super::migrations::V1_SCHEMA_VERSION
        );
    }

    #[test]
    fn newer_schema_is_distinct_and_unchanged() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_path(&directory);
        let connection = Connection::open(&path).unwrap();
        connection.pragma_update(None, "user_version", 999).unwrap();
        drop(connection);
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        assert_eq!(
            inspect_health(&path),
            AccessHealth::new(AccessHealthStatus::NewerSchema, "upgrade_labby")
        );
    }

    #[test]
    fn malformed_v1_store_is_corrupt_not_uninitialized() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_path(&directory);
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(
                None,
                "user_version",
                super::super::migrations::V1_SCHEMA_VERSION,
            )
            .unwrap();
        drop(connection);
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        assert_eq!(
            inspect_health(&path),
            AccessHealth::new(AccessHealthStatus::Corrupt, "repair_access_store")
        );
    }

    #[test]
    fn existing_sidecar_prevents_opening_store() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_path(&directory);
        std::fs::write(&path, b"not opened").unwrap();
        let wal = sidecar_path(&path, "-wal");
        std::fs::write(&wal, b"present").unwrap();
        #[cfg(unix)]
        {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::set_permissions(&wal, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        assert_eq!(
            inspect_health(&path),
            AccessHealth::new(AccessHealthStatus::Locked, "retry_access_store_check")
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"not opened");
    }

    #[test]
    fn live_rollback_journal_transaction_is_refused_without_mutation() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_path(&directory);
        let mut writer = Connection::open(&path).unwrap();
        writer
            .execute_batch("CREATE TABLE live_write(value INTEGER NOT NULL); INSERT INTO live_write VALUES (1);")
            .unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let transaction = writer.transaction().unwrap();
        transaction
            .execute("UPDATE live_write SET value = 2", [])
            .unwrap();
        let journal = sidecar_path(&path, "-journal");
        assert!(journal.exists());

        let database_before = std::fs::read(&path).unwrap();
        let journal_before = std::fs::read(&journal).unwrap();
        let database_snapshot = file_snapshot(&std::fs::metadata(&path).unwrap());
        let journal_snapshot = file_snapshot(&std::fs::metadata(&journal).unwrap());

        assert_eq!(
            inspect_health(&path),
            AccessHealth::new(AccessHealthStatus::Locked, "retry_access_store_check")
        );
        assert_eq!(std::fs::read(&path).unwrap(), database_before);
        assert_eq!(std::fs::read(&journal).unwrap(), journal_before);
        assert_eq!(
            file_snapshot(&std::fs::metadata(&path).unwrap()),
            database_snapshot
        );
        assert_eq!(
            file_snapshot(&std::fs::metadata(&journal).unwrap()),
            journal_snapshot
        );

        transaction.rollback().unwrap();
    }

    #[tokio::test]
    async fn live_wal_store_is_inspected_without_sidecar_mutation() {
        use labby_auth::{Authenticator, VerifiedIdentity};

        let directory = super::super::test_support::secure_tempdir();
        let path = secure_path(&directory);
        let store = super::super::store::AccessStore::open(path.clone())
            .await
            .unwrap();
        store
            .bootstrap_owner(
                super::super::bootstrap::BootstrapOwnerInput::new(
                    VerifiedIdentity::local_credential(
                        Authenticator::StaticBearer,
                        "static-bearer:live-health-test",
                    )
                    .unwrap(),
                    "Local",
                    "Default",
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let wal = sidecar_path(&path, "-wal");
        let shm = sidecar_path(&path, "-shm");
        assert!(wal.exists());
        assert!(shm.exists());
        let mut entries_before = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        entries_before.sort();
        let before = [&path, &wal, &shm].map(|file| {
            (
                std::fs::read(file).unwrap(),
                file_snapshot(&std::fs::metadata(file).unwrap()),
            )
        });

        assert_eq!(
            inspect_health(&path),
            AccessHealth::new(AccessHealthStatus::Ready, "ready")
        );
        for (file, (expected_bytes, expected_snapshot)) in
            [&path, &wal, &shm].into_iter().zip(before)
        {
            let actual_snapshot = file_snapshot(&std::fs::metadata(file).unwrap());
            if file == &shm {
                assert!(actual_snapshot.same_coordination_file(expected_snapshot));
            } else {
                assert_eq!(std::fs::read(file).unwrap(), expected_bytes);
                assert_eq!(actual_snapshot, expected_snapshot);
            }
        }
        let mut entries_after = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        entries_after.sort();
        assert_eq!(entries_after, entries_before);
        assert!(!sidecar_path(&path, "-journal").exists());
        drop(store);
    }

    #[test]
    #[cfg(unix)]
    fn symlink_is_insecure_and_target_is_not_opened() {
        let directory = super::super::test_support::secure_tempdir();
        let path = secure_path(&directory);
        let target = directory.path().join("target.db");
        std::fs::write(&target, b"not sqlite").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        assert_eq!(inspect_health(&path).status, AccessHealthStatus::Insecure);
        assert_eq!(std::fs::read(target).unwrap(), b"not sqlite");
    }
}
