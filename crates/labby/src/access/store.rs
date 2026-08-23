use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(test)]
use rusqlite::types::Value;
use rusqlite::{Connection, ErrorCode, OpenFlags};

use super::error::{AccessStoreError, AccessStoreResult};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
#[derive(Clone)]
pub(super) struct AccessStore {
    connection: Arc<Mutex<Connection>>,
    path: Arc<PathBuf>,
}

impl std::fmt::Debug for AccessStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccessStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl AccessStore {
    pub(super) async fn open(path: PathBuf) -> AccessStoreResult<Self> {
        let open_path = path.clone();
        let connection = tokio::task::spawn_blocking(move || open_connection(&open_path))
            .await
            .map_err(|error| AccessStoreError::Unavailable(error.to_string()))??;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            path: Arc::new(path),
        })
    }

    #[cfg(test)]
    async fn with_connection<T, F>(&self, operation: F) -> AccessStoreResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> AccessStoreResult<T> + Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let connection = connection
                .lock()
                .map_err(|_| AccessStoreError::Unavailable("connection mutex poisoned".into()))?;
            operation(&connection)
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    #[cfg(test)]
    async fn pragma_for_test(&self, name: &'static str) -> AccessStoreResult<String> {
        self.with_connection(move |connection| {
            let value = connection
                .query_row(&format!("PRAGMA {name}"), [], |row| row.get::<_, Value>(0))
                .map_err(map_sqlite_error)?;
            Ok(match value {
                Value::Text(value) => value,
                Value::Integer(value) => value.to_string(),
                other => format!("{other:?}"),
            })
        })
        .await
    }

    #[cfg(test)]
    async fn tables_for_test(&self) -> AccessStoreResult<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT name FROM sqlite_schema
                     WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
                )
                .map_err(map_sqlite_error)?;
            statement
                .query_map([], |row| row.get(0))
                .map_err(map_sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(map_sqlite_error)
        })
        .await
    }

    #[cfg(test)]
    async fn metadata_for_test(&self) -> AccessStoreResult<(i64, String, i64)> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT schema_version, schema_fingerprint, global_revision
                     FROM access_metadata WHERE singleton = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(map_sqlite_error)
        })
        .await
    }

    #[cfg(test)]
    async fn execute_test_statement(&self, sql: &'static str) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            connection.execute_batch(sql).map_err(map_sqlite_error)
        })
        .await
    }

    #[cfg(test)]
    async fn seed_tenant_test_rows(&self) -> AccessStoreResult<()> {
        self.execute_test_statement(
            "INSERT INTO organizations VALUES
               ('org_a', 'A', 'active', 0, 1, 1),
               ('org_b', 'B', 'active', 0, 1, 1);
             INSERT INTO principals VALUES
               ('principal_a', 'org_a', 'user', 'active', NULL, 1, 1),
               ('principal_b', 'org_b', 'user', 'active', NULL, 1, 1);
             INSERT INTO projects VALUES
               ('project_a', 'org_a', 'A', 'active', 0, 1, 1),
               ('project_b', 'org_b', 'B', 'active', 0, 1, 1);",
        )
        .await
    }
}

fn open_connection(path: &Path) -> AccessStoreResult<Connection> {
    if !path.is_absolute() || path.file_name().is_none_or(|name| name != "access.db") {
        return Err(AccessStoreError::InsecurePath {
            path: path.to_path_buf(),
        });
    }
    labby_runtime::path_safety::reject_existing_symlinks_in_path(path).map_err(|_| {
        AccessStoreError::InsecurePath {
            path: path.to_path_buf(),
        }
    })?;
    let parent = path
        .parent()
        .ok_or_else(|| AccessStoreError::InsecurePath {
            path: path.to_path_buf(),
        })?;
    prepare_parent(parent)?;
    validate_existing_store_files(path)?;

    let existed = path.exists();
    if !existed {
        create_restricted_database(path)?;
    }
    validate_store_file(path)?;

    let mut connection = configure_connection(open_nofollow(path)?)?;
    validate_store_file(path)?;
    validate_sidecars(path)?;
    super::migrations::migrate(&mut connection)?;
    validate_store_file(path)?;
    validate_sidecars(path)?;
    super::integrity::validate(&connection)?;
    Ok(connection)
}

fn open_nofollow(path: &Path) -> AccessStoreResult<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(map_sqlite_error)
}

fn validate_existing_store_files(path: &Path) -> AccessStoreResult<()> {
    if path.exists() {
        validate_store_file(path)?;
    }
    validate_sidecars(path)
}

fn validate_sidecars(path: &Path) -> AccessStoreResult<()> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
        match std::fs::symlink_metadata(&sidecar) {
            Ok(_) => validate_store_file(&sidecar)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(AccessStoreError::Unavailable(error.to_string())),
        }
    }
    Ok(())
}

fn prepare_parent(path: &Path) -> AccessStoreResult<()> {
    if path.exists() {
        return validate_secure_parent(path);
    }
    let ancestor = path
        .parent()
        .ok_or_else(|| AccessStoreError::InsecurePath {
            path: path.to_path_buf(),
        })?;
    if !ancestor.exists() {
        return Err(AccessStoreError::MissingParent {
            path: ancestor.to_path_buf(),
        });
    }
    labby_runtime::path_safety::reject_existing_symlinks_in_path(ancestor).map_err(|_| {
        AccessStoreError::InsecurePath {
            path: ancestor.to_path_buf(),
        }
    })?;
    create_restricted_directory(path)?;
    validate_secure_parent(path)
}

#[cfg(unix)]
fn create_restricted_directory(path: &Path) -> AccessStoreResult<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(path)
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))
}

#[cfg(not(unix))]
fn create_restricted_directory(path: &Path) -> AccessStoreResult<()> {
    std::fs::create_dir(path).map_err(|error| AccessStoreError::Unavailable(error.to_string()))
}

#[cfg(unix)]
fn create_restricted_database(path: &Path) -> AccessStoreResult<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map(|_| ())
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))
}

#[cfg(not(unix))]
fn create_restricted_database(path: &Path) -> AccessStoreResult<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(|_| ())
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))
}

fn configure_connection(connection: Connection) -> AccessStoreResult<Connection> {
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(map_sqlite_error)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(map_sqlite_error)?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(map_sqlite_error)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(map_sqlite_error)?;
    let foreign_keys = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
        .map_err(map_sqlite_error)?;
    if foreign_keys != 1 {
        return Err(AccessStoreError::IntegrityViolation {
            check: "foreign_keys",
        });
    }
    Ok(connection)
}

#[cfg(unix)]
fn ensure_restrictive_permissions(path: &Path) -> AccessStoreResult<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::symlink_metadata(path)
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        return Err(AccessStoreError::InsecurePermissions {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(unix)]
fn validate_secure_parent(path: &Path) -> AccessStoreResult<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?;
    let mode = metadata.permissions().mode() & 0o777;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || mode & 0o077 != 0
    {
        return Err(AccessStoreError::InsecurePath {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_secure_parent(_path: &Path) -> AccessStoreResult<()> {
    Ok(())
}

#[cfg(unix)]
fn validate_store_file(path: &Path) -> AccessStoreResult<()> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.nlink() != 1
    {
        return Err(AccessStoreError::InsecurePath {
            path: path.to_path_buf(),
        });
    }
    ensure_restrictive_permissions(path)
}

#[cfg(not(unix))]
fn validate_store_file(path: &Path) -> AccessStoreResult<()> {
    ensure_restrictive_permissions(path)
}

#[cfg(not(unix))]
fn ensure_restrictive_permissions(_path: &Path) -> AccessStoreResult<()> {
    Ok(())
}

#[cfg(all(unix, test))]
fn restrict_permissions(path: &Path) -> AccessStoreResult<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))
}

#[cfg(all(not(unix), test))]
fn restrict_permissions(_path: &Path) -> AccessStoreResult<()> {
    Ok(())
}

pub(super) fn map_sqlite_error(error: rusqlite::Error) -> AccessStoreError {
    let Some(failure) = error.sqlite_error() else {
        return AccessStoreError::Unavailable(error.to_string());
    };
    if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY {
        return AccessStoreError::ForeignKeyViolation;
    }
    match failure.code {
        ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => AccessStoreError::Locked,
        ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => AccessStoreError::Corrupt,
        ErrorCode::DiskFull => AccessStoreError::DiskFull,
        ErrorCode::ReadOnly => AccessStoreError::ReadOnly,
        _ => AccessStoreError::Unavailable(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secure_test_path(directory: &tempfile::TempDir) -> PathBuf {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        directory.path().join("access.db")
    }

    #[tokio::test]
    async fn fresh_store_has_exact_v1_schema_and_security_pragmas() {
        let directory = tempfile::tempdir().unwrap();
        let path = secure_test_path(&directory);
        let store = AccessStore::open(path).await.unwrap();

        assert_eq!(store.pragma_for_test("user_version").await.unwrap(), "1");
        assert_eq!(store.pragma_for_test("journal_mode").await.unwrap(), "wal");
        assert_eq!(store.pragma_for_test("synchronous").await.unwrap(), "2");
        assert_eq!(store.pragma_for_test("foreign_keys").await.unwrap(), "1");
        assert_eq!(store.pragma_for_test("busy_timeout").await.unwrap(), "5000");
        assert_eq!(
            store.tables_for_test().await.unwrap(),
            vec![
                "access_audit",
                "access_metadata",
                "organizations",
                "principal_links",
                "principals",
                "project_loadouts",
                "project_memberships",
                "projects",
            ]
        );
        assert_eq!(
            store.metadata_for_test().await.unwrap(),
            (
                super::super::migrations::SCHEMA_VERSION,
                super::super::migrations::SCHEMA_FINGERPRINT.to_string(),
                0,
            )
        );
    }

    #[tokio::test]
    async fn canonical_reopen_preserves_data_and_global_revision() {
        let directory = tempfile::tempdir().unwrap();
        let path = secure_test_path(&directory);
        let store = AccessStore::open(path.clone()).await.unwrap();
        store
            .execute_test_statement(
                "INSERT INTO organizations VALUES
                   ('org_durable', 'Durable', 'active', 0, 1, 1);
                 UPDATE access_metadata SET global_revision = 7 WHERE singleton = 1;",
            )
            .await
            .unwrap();
        drop(store);

        let reopened = AccessStore::open(path).await.unwrap();
        assert_eq!(reopened.metadata_for_test().await.unwrap().2, 7);
        let organization_count = reopened
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM organizations
                         WHERE organization_id = 'org_durable'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(organization_count, 1);
    }

    #[tokio::test]
    async fn newer_schema_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = secure_test_path(&directory);
        let connection = Connection::open(&path).unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
        drop(connection);
        restrict_permissions(&path).unwrap();
        assert!(matches!(
            AccessStore::open(path).await,
            Err(AccessStoreError::UnsupportedSchema {
                found: 2,
                supported: 1
            })
        ));
    }

    #[tokio::test]
    async fn stamped_v1_without_canonical_schema_identity_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = secure_test_path(&directory);
        let connection = Connection::open(&path).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        drop(connection);
        restrict_permissions(&path).unwrap();

        assert!(matches!(
            AccessStore::open(path).await,
            Err(AccessStoreError::IntegrityViolation { .. })
        ));
    }

    #[tokio::test]
    async fn canonical_names_and_metadata_do_not_hide_altered_schema_definition() {
        let directory = tempfile::tempdir().unwrap();
        let path = secure_test_path(&directory);
        let store = AccessStore::open(path.clone()).await.unwrap();
        drop(store);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "DROP INDEX principal_links_external_unique;
                 CREATE INDEX principal_links_external_unique
                   ON principal_links(issuer, subject) WHERE link_kind = 'external';",
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            AccessStore::open(path).await,
            Err(AccessStoreError::IntegrityViolation {
                check: "schema_manifest"
            })
        ));
    }

    #[tokio::test]
    async fn composite_foreign_keys_reject_cross_tenant_edges() {
        let directory = tempfile::tempdir().unwrap();
        let store = AccessStore::open(secure_test_path(&directory))
            .await
            .unwrap();
        store.seed_tenant_test_rows().await.unwrap();
        let membership = store
            .execute_test_statement(
                "INSERT INTO project_memberships
             (membership_id, organization_id, project_id, principal_id, role, status,
              created_by, created_at, updated_at)
             VALUES ('mem_bad', 'org_b', 'project_a', 'principal_b', 'member', 'active',
                     'principal_b', 1, 1)",
            )
            .await;
        assert!(matches!(
            membership,
            Err(AccessStoreError::ForeignKeyViolation)
        ));
        let loadout = store
            .execute_test_statement(
                "INSERT INTO project_loadouts
             (organization_id, project_id, loadout_name, created_by, created_at, updated_at)
             VALUES ('org_b', 'project_a', 'default', 'principal_b', 1, 1)",
            )
            .await;
        assert!(matches!(
            loadout,
            Err(AccessStoreError::ForeignKeyViolation)
        ));

        let missing_same_tenant_parent = store
            .execute_test_statement(
                "INSERT INTO project_loadouts
                 (organization_id, project_id, loadout_name, created_by, created_at, updated_at)
                 VALUES ('org_a', 'missing', 'default', 'principal_a', 1, 1)",
            )
            .await;
        assert!(matches!(
            missing_same_tenant_parent,
            Err(AccessStoreError::ForeignKeyViolation)
        ));
    }

    #[tokio::test]
    async fn principal_link_shape_and_uniqueness_are_database_invariants() {
        let directory = tempfile::tempdir().unwrap();
        let store = AccessStore::open(secure_test_path(&directory))
            .await
            .unwrap();
        store.seed_tenant_test_rows().await.unwrap();
        store
            .execute_test_statement(
                "INSERT INTO principal_links
             (link_id, principal_id, link_kind, issuer, subject, credential_id, status,
              verification_generation, link_generation, created_at, updated_at)
             VALUES ('link_a', 'principal_a', 'external', 'https://idp.example.com', 'alice',
                     NULL, 'active', 1, 1, 1, 1)",
            )
            .await
            .unwrap();
        assert!(
            store
                .execute_test_statement(
                    "INSERT INTO principal_links
             (link_id, principal_id, link_kind, issuer, subject, credential_id, status,
              verification_generation, link_generation, created_at, updated_at)
             VALUES ('link_dup', 'principal_b', 'external', 'https://idp.example.com', 'alice',
                     NULL, 'active', 1, 1, 1, 1)"
                )
                .await
                .is_err()
        );
        assert!(
            store
                .execute_test_statement(
                    "INSERT INTO principal_links
             (link_id, principal_id, link_kind, issuer, subject, credential_id, status,
              verification_generation, link_generation, created_at, updated_at)
             VALUES ('link_bad', 'principal_a', 'external', NULL, 'alice', 'credential',
                     'active', 1, 1, 1, 1)"
                )
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_weak_permissions_symlinks_hardlinks_and_corrupt_files() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let weak_directory = tempfile::tempdir().unwrap();
        let weak_path = secure_test_path(&weak_directory);
        std::fs::write(&weak_path, []).unwrap();
        std::fs::set_permissions(&weak_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            AccessStore::open(weak_path).await,
            Err(AccessStoreError::InsecurePermissions { .. })
        ));

        let symlink_directory = tempfile::tempdir().unwrap();
        let symlink_path = secure_test_path(&symlink_directory);
        let target = symlink_directory.path().join("target.db");
        std::fs::write(&target, []).unwrap();
        symlink(&target, &symlink_path).unwrap();
        assert!(matches!(
            AccessStore::open(symlink_path).await,
            Err(AccessStoreError::InsecurePath { .. })
        ));

        let hardlink_directory = tempfile::tempdir().unwrap();
        let hardlink_path = secure_test_path(&hardlink_directory);
        let hardlink_target = hardlink_directory.path().join("other.db");
        let original = b"must remain byte-for-byte unchanged";
        std::fs::write(&hardlink_target, original).unwrap();
        std::fs::set_permissions(&hardlink_target, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::hard_link(&hardlink_target, &hardlink_path).unwrap();
        assert!(matches!(
            AccessStore::open(hardlink_path).await,
            Err(AccessStoreError::InsecurePath { .. })
        ));
        assert_eq!(std::fs::read(&hardlink_target).unwrap(), original);

        let corrupt_directory = tempfile::tempdir().unwrap();
        let corrupt_path = secure_test_path(&corrupt_directory);
        std::fs::write(&corrupt_path, b"not a sqlite database").unwrap();
        std::fs::set_permissions(&corrupt_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            AccessStore::open(corrupt_path).await,
            Err(AccessStoreError::Corrupt)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn creates_new_leaf_and_store_with_owner_only_permissions_without_fixing_weak_dirs() {
        use std::os::unix::fs::PermissionsExt as _;

        let base = tempfile::tempdir().unwrap();
        std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let leaf = base.path().join("access-state");
        let path = leaf.join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        assert_eq!(
            std::fs::metadata(&leaf).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(store);

        let weak = base.path().join("weak");
        std::fs::create_dir(&weak).unwrap();
        std::fs::set_permissions(&weak, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            AccessStore::open(weak.join("access.db")).await,
            Err(AccessStoreError::InsecurePath { .. })
        ));
        assert_eq!(
            std::fs::metadata(&weak).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}
