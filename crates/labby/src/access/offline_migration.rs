//! Installation-owner schema activation. No bootstrap or credential changes.

use serde::Serialize;
use thiserror::Error;

use super::{AccessStore, AccessStoreError, migrations::MigrationEvidenceSource};
use crate::installation::{InstallationError, InstallationLifecycleLock, InstallationPaths};

#[derive(Debug, Error)]
pub(crate) enum OfflineMigrationError {
    #[error(transparent)]
    Installation(#[from] InstallationError),
    #[error("access migration requires an existing initialized regular access.db")]
    MissingStore,
    #[error(transparent)]
    Access(#[from] AccessStoreError),
}

#[derive(Debug, Serialize)]
pub(crate) struct OfflineMigrationOutcome {
    pub schema_version: i64,
    pub verified_reopens: u8,
}

pub(crate) async fn migrate(
    paths: &InstallationPaths,
    evidence: MigrationEvidenceSource,
) -> Result<OfflineMigrationOutcome, OfflineMigrationError> {
    let _lock = InstallationLifecycleLock::acquire_offline(paths)?;
    let path = paths.access_db();
    if !std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
        return Err(OfflineMigrationError::MissingStore);
    }
    super::store::validated_access_path(&path)
        .map_err(|()| AccessStoreError::InsecurePath { path: path.clone() })?;
    let connection = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(super::store::map_sqlite_error)?;
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(super::store::map_sqlite_error)?;
    drop(connection);
    if version == 0 {
        return Err(OfflineMigrationError::MissingStore);
    }
    // Reuse the store's path, schema, approval, checkpoint and transaction checks.
    // Reopening never bootstraps an owner or issues a credential.
    for _ in 0..3 {
        drop(AccessStore::open_with_migration_evidence(path.clone(), evidence.clone()).await?);
    }
    Ok(OfflineMigrationOutcome {
        schema_version: super::migrations::SCHEMA_VERSION,
        verified_reopens: 2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{migration_fixture, migrations, test_support};

    fn fixture() -> (tempfile::TempDir, InstallationPaths) {
        let directory = test_support::secure_tempdir();
        let paths = InstallationPaths::from_root(directory.path()).unwrap();
        let connection = migrations::canonical_v5_schema().unwrap();
        connection
            .execute(
                "INSERT INTO access_metadata VALUES(1,5,?1,0,1,0,NULL)",
                [migrations::V5_SCHEMA_FINGERPRINT],
            )
            .unwrap();
        connection
            .pragma_update(None, "application_id", migrations::APPLICATION_ID)
            .unwrap();
        connection.pragma_update(None, "user_version", 5).unwrap();
        connection
            .execute("VACUUM INTO ?1", [paths.access_db().to_str().unwrap()])
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(paths.access_db(), std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        (directory, paths)
    }

    fn approve(paths: &InstallationPaths) -> MigrationEvidenceSource {
        migration_fixture::approve_restored_store(
            &paths.access_db(),
            &paths.root().join("checkpoint.db"),
            &paths.root().join("approval.json"),
        )
    }

    #[tokio::test]
    async fn offline_migration_refuses_absent_and_invalid_approval() {
        let (_directory, paths) = fixture();
        let evidence = paths.root().join("approval.json");
        for contents in [None, Some("{}"), Some("not json")] {
            if let Some(contents) = contents {
                std::fs::write(&evidence, contents).unwrap();
            }
            assert!(matches!(
                migrate(&paths, MigrationEvidenceSource::Path(evidence.clone())).await,
                Err(OfflineMigrationError::Access(
                    AccessStoreError::MigrationEvidenceInvalid { .. }
                ))
            ));
        }
        assert_eq!(version(&paths), 5);
    }

    #[tokio::test]
    async fn offline_migration_refuses_running_daemon() {
        let (_directory, paths) = fixture();
        let evidence = approve(&paths);
        let _daemon = InstallationLifecycleLock::acquire_daemon(&paths).unwrap();
        assert!(matches!(
            migrate(&paths, evidence).await,
            Err(OfflineMigrationError::Installation(
                InstallationError::Locked { .. }
            ))
        ));
        assert_eq!(version(&paths), 5);
    }

    #[tokio::test]
    async fn offline_migration_refuses_checkpoint_mismatch() {
        let (_directory, paths) = fixture();
        let evidence = approve(&paths);
        let connection = rusqlite::Connection::open(paths.access_db()).unwrap();
        connection
            .execute("UPDATE access_metadata SET global_revision=1", [])
            .unwrap();
        drop(connection);
        assert!(matches!(
            migrate(&paths, evidence).await,
            Err(OfflineMigrationError::Access(
                AccessStoreError::MigrationEvidenceInvalid { .. }
            ))
        ));
        assert_eq!(version(&paths), 5);
    }

    #[tokio::test]
    async fn offline_migration_rehearses_and_reopens_without_bootstrap() {
        let (_directory, paths) = fixture();
        let evidence = approve(&paths);
        let checkpoint = std::fs::read(paths.root().join("checkpoint.db")).unwrap();
        let outcome = migrate(&paths, evidence.clone()).await.unwrap();
        assert_eq!(outcome.schema_version, 7);
        assert_eq!(outcome.verified_reopens, 2);
        migrate(&paths, evidence).await.unwrap();
        assert_eq!(version(&paths), 7);
        assert_eq!(
            checkpoint,
            std::fs::read(paths.root().join("checkpoint.db")).unwrap()
        );
        let connection = rusqlite::Connection::open(paths.access_db()).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT bootstrap_generation FROM access_metadata",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM principals", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn offline_migration_never_initializes_missing_or_empty_store() {
        let directory = test_support::secure_tempdir();
        let paths = InstallationPaths::from_root(directory.path()).unwrap();
        let evidence = MigrationEvidenceSource::Path(paths.root().join("absent.json"));
        assert!(matches!(
            migrate(&paths, evidence.clone()).await,
            Err(OfflineMigrationError::MissingStore)
        ));
        std::fs::File::create(paths.access_db()).unwrap();
        assert!(matches!(
            migrate(&paths, evidence).await,
            Err(OfflineMigrationError::MissingStore)
        ));
        assert_eq!(std::fs::metadata(paths.access_db()).unwrap().len(), 0);
    }

    fn version(paths: &InstallationPaths) -> i64 {
        rusqlite::Connection::open(paths.access_db())
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }
}
