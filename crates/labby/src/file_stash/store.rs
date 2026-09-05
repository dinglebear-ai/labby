use super::schema;
use rusqlite::{Connection, ErrorCode, OpenFlags};
use std::{
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
    #[error("File Stash schema {0} is newer than this binary")]
    NewerSchema(i64),
    #[error("File Stash metadata is corrupt")]
    Corrupt,
    #[error("File Stash database and blob snapshot markers do not match")]
    BackupMismatch,
    #[error("File Stash storage is unavailable")]
    Unavailable,
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
    path: Arc<PathBuf>,
}
impl FileStashStore {
    pub(super) async fn open(path: PathBuf, snapshot_id: String) -> Result<Self> {
        let p = path.clone();
        let connection = tokio::task::spawn_blocking(move || open_connection(&p, &snapshot_id))
            .await
            .map_err(|_| FileStashStoreError::Unavailable)??;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            admission: Arc::new(Semaphore::new(1)),
            path: Arc::new(path),
        })
    }
    pub(super) async fn with_connection<T: Send + 'static>(
        &self,
        op: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = tokio::time::timeout(
            ADMISSION_TIMEOUT,
            Arc::clone(&self.admission).acquire_owned(),
        )
        .await
        .map_err(|_| FileStashStoreError::Busy)?
        .map_err(|_| FileStashStoreError::Unavailable)?;
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
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
        self.admission.close();
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
