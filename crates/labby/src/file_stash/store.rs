use super::schema;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, TransactionBehavior, params};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const ADMISSION_TIMEOUT: Duration = Duration::from_millis(100);
pub(super) type Result<T> = std::result::Result<T, FileStashStoreError>;
#[derive(Debug, thiserror::Error)]
pub(crate) enum FileStashStoreError {
    #[error("File Stash is busy")]
    Busy,
    #[error("File Stash quota is exhausted")]
    QuotaExceeded,
    #[error("File Stash name already exists")]
    Conflict,
    #[error("File Stash upload length does not match its reservation")]
    LengthMismatch,
    #[error("File Stash metadata and blob state do not agree")]
    Integrity,
    #[error("File Stash schema {0} is newer than this binary")]
    NewerSchema(i64),
    #[error("File Stash metadata is corrupt")]
    Corrupt,
    #[error("File Stash database and blob snapshot markers do not match")]
    BackupMismatch,
    #[error("File Stash storage is unavailable")]
    Unavailable,
}

#[derive(Clone, Debug)]
pub(crate) struct UploadReservation {
    pub(crate) upload_id: String,
    pub(crate) owner_principal_id: String,
    pub(crate) display_name: String,
    pub(crate) collision_key: String,
    pub(crate) reserved_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct StashUsage {
    pub(crate) committed_bytes: u64,
    pub(crate) reserved_bytes: u64,
    pub(crate) live_files: u64,
    pub(crate) owned_shared_file_count: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingRecovery {
    pub(crate) upload_id: String,
    pub(crate) state: String,
    pub(crate) reserved_bytes: u64,
}
impl FileStashStoreError {
    pub(super) fn sqlite(e: rusqlite::Error) -> Self {
        match &e {
            rusqlite::Error::SqliteFailure(c, _)
                if matches!(c.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) =>
            {
                Self::Busy
            }
            rusqlite::Error::SqliteFailure(c, _)
                if matches!(c.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) =>
            {
                Self::Corrupt
            }
            rusqlite::Error::SqliteFailure(c, _) if c.code == ErrorCode::ReadOnly => {
                Self::Unavailable
            }
            _ => Self::Unavailable,
        }
    }
}
#[derive(Clone)]
pub(crate) struct FileStashStore {
    connection: Arc<Mutex<Connection>>,
    admission: Arc<Semaphore>,
    queue: Arc<Semaphore>,
    admission_timeout: Duration,
    path: Arc<PathBuf>,
}
impl FileStashStore {
    pub(super) async fn open(path: PathBuf, snapshot_id: String) -> Result<Self> {
        Self::open_with_limits(path, snapshot_id, 64, ADMISSION_TIMEOUT).await
    }

    pub(super) async fn open_with_limits(
        path: PathBuf,
        snapshot_id: String,
        queue_capacity: usize,
        admission_timeout: Duration,
    ) -> Result<Self> {
        let p = path.clone();
        let connection = tokio::task::spawn_blocking(move || open_connection(&p, &snapshot_id))
            .await
            .map_err(|_| FileStashStoreError::Unavailable)??;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            admission: Arc::new(Semaphore::new(1)),
            queue: Arc::new(Semaphore::new(queue_capacity)),
            admission_timeout,
            path: Arc::new(path),
        })
    }
    pub(super) async fn with_connection<T: Send + 'static>(
        &self,
        op: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let queued = Arc::clone(&self.queue)
            .try_acquire_owned()
            .map_err(|error| match error {
                tokio::sync::TryAcquireError::Closed => FileStashStoreError::Unavailable,
                tokio::sync::TryAcquireError::NoPermits => FileStashStoreError::Busy,
            })?;
        let permit = tokio::time::timeout(
            self.admission_timeout,
            Arc::clone(&self.admission).acquire_owned(),
        )
        .await
        .map_err(|_| FileStashStoreError::Busy)?
        .map_err(|_| FileStashStoreError::Unavailable)?;
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let _queued = queued;
            let _permit = permit;
            let mut c = connection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            op(&mut c)
        })
        .await
        .map_err(|_| FileStashStoreError::Unavailable)?
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) async fn checkpoint(&self) -> Result<()> {
        self.with_connection(|connection| {
            connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .map_err(FileStashStoreError::sqlite)
        })
        .await
    }
    pub(super) fn close(&self) {
        self.queue.close();
        self.admission.close();
    }

    pub(crate) async fn reserve_upload(
        &self,
        owner: String,
        display_name: String,
        collision_key: String,
        declared_bytes: u64,
        expires_at: i64,
        principal_quota: u64,
        instance_quota: u64,
        max_live_files: u32,
    ) -> Result<UploadReservation> {
        let upload_id = ulid::Ulid::new().to_string();
        self.with_connection(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(FileStashStoreError::sqlite)?;
            let (principal_committed, principal_reserved, live_files, pending_files): (i64, i64, i64, i64) = tx
                .query_row(
                    "SELECT COALESCE((SELECT SUM(size_bytes) FROM files WHERE owner_principal_id=?1 AND ready=1),0),COALESCE((SELECT SUM(reserved_bytes) FROM pending_uploads WHERE owner_principal_id=?1),0),COALESCE((SELECT COUNT(*) FROM files WHERE owner_principal_id=?1 AND ready=1),0),COALESCE((SELECT COUNT(*) FROM pending_uploads WHERE owner_principal_id=?1),0)",
                    [&owner],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(FileStashStoreError::sqlite)?;
            let instance_used: i64 = tx
                .query_row(
                    "SELECT COALESCE((SELECT SUM(size_bytes) FROM files WHERE ready=1),0)+COALESCE((SELECT SUM(reserved_bytes) FROM pending_uploads),0)",
                    [],
                    |row| row.get(0),
                )
                .map_err(FileStashStoreError::sqlite)?;
            let declared = i64::try_from(declared_bytes).map_err(|_| FileStashStoreError::QuotaExceeded)?;
            if live_files.saturating_add(pending_files) >= i64::from(max_live_files)
                || principal_committed.saturating_add(principal_reserved).saturating_add(declared)
                    > i64::try_from(principal_quota).unwrap_or(i64::MAX)
                || instance_used.saturating_add(declared)
                    > i64::try_from(instance_quota).unwrap_or(i64::MAX)
            {
                return Err(FileStashStoreError::QuotaExceeded);
            }
            let now = unix_now();
            tx.execute(
                "INSERT INTO pending_uploads(upload_id,owner_principal_id,display_name,collision_key,reserved_bytes,state,expires_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'pending',?6,?7,?7)",
                params![upload_id, owner, display_name, collision_key, declared, expires_at, now],
            )
            .map_err(map_constraint)?;
            tx.commit().map_err(FileStashStoreError::sqlite)?;
            Ok(UploadReservation { upload_id, owner_principal_id: owner, display_name, collision_key, reserved_bytes: declared_bytes })
        }).await
    }

    pub(crate) async fn mark_blob_published(&self, upload_id: String) -> Result<()> {
        self.with_connection(move |connection| {
            let changed = connection.execute(
                "UPDATE pending_uploads SET state='blob_published',updated_at=unixepoch() WHERE upload_id=?1 AND state='pending'",
                [&upload_id],
            ).map_err(FileStashStoreError::sqlite)?;
            if changed == 1 { Ok(()) } else { Err(FileStashStoreError::Integrity) }
        }).await
    }

    pub(crate) async fn commit_upload(&self, upload_id: String) -> Result<String> {
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(FileStashStoreError::sqlite)?;
            let pending: Option<(String,String,String,i64,String)> = tx.query_row(
                "SELECT owner_principal_id,display_name,collision_key,reserved_bytes,state FROM pending_uploads WHERE upload_id=?1",
                [&upload_id],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
            ).optional().map_err(FileStashStoreError::sqlite)?;
            let Some((owner,name,key,size,state)) = pending else { return Err(FileStashStoreError::Integrity) };
            if state != "blob_published" { return Err(FileStashStoreError::Integrity); }
            // Delete pending first so its cross-table name claim is released in this transaction.
            tx.execute("DELETE FROM pending_uploads WHERE upload_id=?1", [&upload_id]).map_err(FileStashStoreError::sqlite)?;
            tx.execute(
                "INSERT INTO files(file_id,owner_principal_id,display_name,collision_key,size_bytes,blob_key,ready,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?1,1,unixepoch(),unixepoch())",
                params![upload_id,owner,name,key,size],
            ).map_err(map_constraint)?;
            tx.commit().map_err(FileStashStoreError::sqlite)?;
            Ok(upload_id)
        }).await
    }

    pub(crate) async fn cancel_upload(&self, upload_id: String) -> Result<()> {
        #[cfg(all(test, any(target_os = "linux", target_os = "android")))]
        {
            let mut injected = FAIL_CANCEL_ID
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if injected.as_deref() == Some(upload_id.as_str()) {
                *injected = None;
                return Err(FileStashStoreError::Busy);
            }
        }
        self.with_connection(move |connection| {
            connection
                .execute(
                    "DELETE FROM pending_uploads WHERE upload_id=?1",
                    [&upload_id],
                )
                .map_err(FileStashStoreError::sqlite)?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn usage(&self, owner: String) -> Result<StashUsage> {
        self.with_connection(move |connection| {
            connection.query_row(
                "SELECT COALESCE((SELECT SUM(size_bytes) FROM files WHERE owner_principal_id=?1 AND ready=1),0),COALESCE((SELECT SUM(reserved_bytes) FROM pending_uploads WHERE owner_principal_id=?1),0),COALESCE((SELECT COUNT(*) FROM files WHERE owner_principal_id=?1 AND ready=1),0),COALESCE((SELECT COUNT(*) FROM files f WHERE f.owner_principal_id=?1 AND f.ready=1 AND EXISTS(SELECT 1 FROM grants g WHERE g.file_id=f.file_id AND g.state='active')),0)",
                [&owner],
                |r| Ok(StashUsage { committed_bytes: r.get::<_, i64>(0)? as u64, reserved_bytes: r.get::<_, i64>(1)? as u64, live_files: r.get::<_, i64>(2)? as u64, owned_shared_file_count: r.get::<_, i64>(3)? as u64 }),
            ).map_err(FileStashStoreError::sqlite)
        }).await
    }

    pub(crate) async fn pending_for_recovery(
        &self,
        after: String,
        limit: usize,
    ) -> Result<Vec<PendingRecovery>> {
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT upload_id,state,reserved_bytes FROM pending_uploads WHERE upload_id>?1 ORDER BY upload_id LIMIT ?2",
                )
                .map_err(FileStashStoreError::sqlite)?;
            let rows = statement
                .query_map(params![after, i64::try_from(limit).unwrap_or(i64::MAX)], |r| {
                    Ok(PendingRecovery {
                        upload_id: r.get(0)?,
                        state: r.get(1)?,
                        reserved_bytes: r.get::<_, i64>(2)? as u64,
                    })
                })
                .map_err(FileStashStoreError::sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(FileStashStoreError::sqlite)
        })
        .await
    }

    pub(crate) async fn expired_pending(
        &self,
        now: i64,
        limit: usize,
    ) -> Result<Vec<PendingRecovery>> {
        self.with_connection(move |connection| {
            let mut statement = connection.prepare("SELECT upload_id,state,reserved_bytes FROM pending_uploads WHERE expires_at<=?1 ORDER BY expires_at,upload_id LIMIT ?2").map_err(FileStashStoreError::sqlite)?;
            let rows = statement.query_map(params![now, i64::try_from(limit).unwrap_or(i64::MAX)], |r| Ok(PendingRecovery { upload_id:r.get(0)?, state:r.get(1)?, reserved_bytes:r.get::<_, i64>(2)? as u64 })).map_err(FileStashStoreError::sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(FileStashStoreError::sqlite)
        }).await
    }

    pub(crate) async fn committed_blob_keys(
        &self,
        after: String,
        limit: usize,
    ) -> Result<Vec<(String, u64)>> {
        self.with_connection(move |connection| {
            let mut statement = connection
                .prepare("SELECT blob_key,size_bytes FROM files WHERE ready=1 AND blob_key>?1 ORDER BY blob_key LIMIT ?2")
                .map_err(FileStashStoreError::sqlite)?;
            let rows = statement
                .query_map(params![after, i64::try_from(limit).unwrap_or(i64::MAX)], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)))
                .map_err(FileStashStoreError::sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(FileStashStoreError::sqlite)
        })
        .await
    }

    pub(crate) async fn committed_blob_membership(
        &self,
        keys: Vec<String>,
    ) -> Result<HashSet<String>> {
        self.with_connection(move |connection| {
            if keys.is_empty() {
                return Ok(HashSet::new());
            }
            let placeholders = std::iter::repeat_n("?", keys.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql =
                format!("SELECT blob_key FROM files WHERE ready=1 AND blob_key IN({placeholders})");
            let mut statement = connection
                .prepare(&sql)
                .map_err(FileStashStoreError::sqlite)?;
            let rows = statement
                .query_map(rusqlite::params_from_iter(keys.iter()), |row| row.get(0))
                .map_err(FileStashStoreError::sqlite)?;
            rows.collect::<std::result::Result<HashSet<_>, _>>()
                .map_err(FileStashStoreError::sqlite)
        })
        .await
    }

    pub(crate) async fn expire_upload_now(&self, upload_id: String) -> Result<()> {
        self.with_connection(move |connection| {
            connection.execute("UPDATE pending_uploads SET expires_at=0,updated_at=unixepoch() WHERE upload_id=?1", [&upload_id]).map_err(FileStashStoreError::sqlite)?;
            Ok(())
        }).await
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
static FAIL_CANCEL_ID: std::sync::LazyLock<Mutex<Option<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
pub(super) fn inject_cancel_failure(upload_id: String) {
    *FAIL_CANCEL_ID
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(upload_id);
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn map_constraint(error: rusqlite::Error) -> FileStashStoreError {
    match &error {
        rusqlite::Error::SqliteFailure(code, _) if code.code == ErrorCode::ConstraintViolation => {
            FileStashStoreError::Conflict
        }
        _ => FileStashStoreError::sqlite(error),
    }
}
fn open_connection(path: &Path, snapshot_id: &str) -> Result<Connection> {
    let mut c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(FileStashStoreError::sqlite)?;
    c.busy_timeout(BUSY_TIMEOUT)
        .map_err(FileStashStoreError::sqlite)?;
    c.pragma_update(None, "journal_mode", "WAL")
        .map_err(FileStashStoreError::sqlite)?;
    c.pragma_update(None, "synchronous", "FULL")
        .map_err(FileStashStoreError::sqlite)?;
    c.pragma_update(None, "foreign_keys", true)
        .map_err(FileStashStoreError::sqlite)?;
    schema::migrate(&mut c, snapshot_id)?;
    Ok(c)
}

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
mod tests {
    use super::*;

    async fn store() -> (tempfile::TempDir, FileStashStore) {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("metadata.sqlite3");
        std::fs::File::create(&path).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let store = FileStashStore::open(path, ulid::Ulid::new().to_string())
            .await
            .unwrap();
        (temp, store)
    }

    #[tokio::test]
    async fn reservations_enforce_name_and_both_byte_quotas_transactionally() {
        let (_temp, store) = store().await;
        let first = store
            .reserve_upload(
                "owner".into(),
                "Report".into(),
                "report".into(),
                6,
                i64::MAX,
                10,
                20,
                2,
            )
            .await
            .unwrap();
        assert_eq!(first.owner_principal_id, "owner");
        assert_eq!(first.display_name, "Report");
        assert_eq!(first.collision_key, "report");
        assert!(matches!(
            store
                .reserve_upload(
                    "owner".into(),
                    "REPORT".into(),
                    "report".into(),
                    1,
                    i64::MAX,
                    10,
                    20,
                    2
                )
                .await,
            Err(FileStashStoreError::Conflict)
        ));
        assert!(matches!(
            store
                .reserve_upload(
                    "owner".into(),
                    "other".into(),
                    "other".into(),
                    5,
                    i64::MAX,
                    10,
                    20,
                    2
                )
                .await,
            Err(FileStashStoreError::QuotaExceeded)
        ));
        assert!(matches!(
            store
                .reserve_upload(
                    "second".into(),
                    "other".into(),
                    "other".into(),
                    15,
                    i64::MAX,
                    20,
                    20,
                    2
                )
                .await,
            Err(FileStashStoreError::QuotaExceeded)
        ));
        assert_eq!(store.usage("owner".into()).await.unwrap().reserved_bytes, 6);
        store.cancel_upload(first.upload_id).await.unwrap();
        assert_eq!(
            store.usage("owner".into()).await.unwrap(),
            StashUsage::default()
        );
    }

    #[tokio::test]
    async fn pending_uploads_count_toward_the_live_file_limit_under_concurrency() {
        let (_temp, store) = store().await;
        let left = store.clone();
        let right = store.clone();
        let (a, b) = tokio::join!(
            left.reserve_upload(
                "owner".into(),
                "a".into(),
                "a".into(),
                0,
                i64::MAX,
                10,
                20,
                1
            ),
            right.reserve_upload(
                "owner".into(),
                "b".into(),
                "b".into(),
                0,
                i64::MAX,
                10,
                20,
                1
            ),
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert!(matches!(
            a.err().or_else(|| b.err()),
            Some(FileStashStoreError::QuotaExceeded)
        ));
    }

    #[tokio::test]
    async fn publication_moves_reservation_to_committed_usage_in_one_transaction() {
        let (_temp, store) = store().await;
        let pending = store
            .reserve_upload(
                "owner".into(),
                "a".into(),
                "a".into(),
                7,
                i64::MAX,
                20,
                20,
                2,
            )
            .await
            .unwrap();
        store
            .mark_blob_published(pending.upload_id.clone())
            .await
            .unwrap();
        let file = store
            .commit_upload(pending.upload_id.clone())
            .await
            .unwrap();
        assert_eq!(file, pending.upload_id);
        assert_eq!(
            store.usage("owner".into()).await.unwrap(),
            StashUsage {
                committed_bytes: 7,
                reserved_bytes: 0,
                live_files: 1,
                owned_shared_file_count: 0,
            }
        );
    }

    #[tokio::test]
    async fn shared_count_is_distinct_per_file_and_ignores_revoked_grants() {
        let (_temp, store) = store().await;
        let pending = store
            .reserve_upload(
                "owner".into(),
                "a".into(),
                "a".into(),
                1,
                i64::MAX,
                10,
                20,
                2,
            )
            .await
            .unwrap();
        store
            .mark_blob_published(pending.upload_id.clone())
            .await
            .unwrap();
        let file_id = store.commit_upload(pending.upload_id).await.unwrap();
        store
            .with_connection(move |connection| {
                connection
                    .execute(
                        "INSERT INTO grants VALUES('g1',?1,'p1','active',1,NULL)",
                        [&file_id],
                    )
                    .map_err(FileStashStoreError::sqlite)?;
                connection
                    .execute(
                        "INSERT INTO grants VALUES('g2',?1,'p2','active',1,NULL)",
                        [&file_id],
                    )
                    .map_err(FileStashStoreError::sqlite)?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            store
                .usage("owner".into())
                .await
                .unwrap()
                .owned_shared_file_count,
            1
        );
        store.with_connection(|connection| {
            connection.execute("UPDATE grants SET state='revoked',revoked_at=2 WHERE grant_id IN('g1','g2')", []).map_err(FileStashStoreError::sqlite)?;
            Ok(())
        }).await.unwrap();
        assert_eq!(
            store
                .usage("owner".into())
                .await
                .unwrap()
                .owned_shared_file_count,
            0
        );
    }
}
