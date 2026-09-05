use super::store::{FileStashStore, FileStashStoreError};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{Mutex, Semaphore};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileStashBlockedReason {
    UnsafeRoot,
    Permission,
    Corrupt,
    NewerSchema,
    BackupMismatch,
    Unavailable,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileStashStatus {
    Ready,
    Blocked(FileStashBlockedReason),
    Shutdown,
}
enum State {
    Ready(FileStashStore),
    Blocked(FileStashBlockedReason),
    Shutdown,
}

/// Sole process owner for Stash persistence. The retained root handle pins the
/// verified directory while SQLite and later blob operations address children.
pub(crate) struct FileStashRuntime {
    root: Arc<PathBuf>,
    _root_handle: Option<Arc<File>>,
    state: Arc<Mutex<State>>,
    janitor_admission: Arc<Semaphore>,
    janitor_cancel: tokio_util::sync::CancellationToken,
    janitor_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl FileStashRuntime {
    pub(crate) fn blocked() -> Self {
        Self {
            root: Arc::new(PathBuf::new()),
            _root_handle: None,
            state: Arc::new(Mutex::new(State::Blocked(
                FileStashBlockedReason::Unavailable,
            ))),
            janitor_admission: Arc::new(Semaphore::new(1)),
            janitor_cancel: tokio_util::sync::CancellationToken::new(),
            janitor_task: Mutex::new(None),
        }
    }
    pub(crate) async fn initialize(root: PathBuf) -> Self {
        Self::initialize_with_interval(root, std::time::Duration::from_mins(1)).await
    }
    pub(crate) async fn initialize_with_interval(
        root: PathBuf,
        janitor_interval: std::time::Duration,
    ) -> Self {
        #[cfg(target_os = "macos")]
        {
            let _ = janitor_interval;
            tracing::warn!(
                "File Stash is unavailable on macOS: descriptor-rooted SQLite is not supported"
            );
            Self {
                root: Arc::new(root),
                _root_handle: None,
                state: Arc::new(Mutex::new(State::Blocked(
                    FileStashBlockedReason::UnsafeRoot,
                ))),
                janitor_admission: Arc::new(Semaphore::new(1)),
                janitor_cancel: tokio_util::sync::CancellationToken::new(),
                janitor_task: Mutex::new(None),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let initialized = initialize_owned(&root).await;
            let admission = Arc::new(Semaphore::new(1));
            let cancel = tokio_util::sync::CancellationToken::new();
            let (state, root_handle, janitor_task) = match initialized {
                Ok((store, handle)) => {
                    let task = spawn_janitor(
                        store.clone(),
                        Arc::clone(&admission),
                        cancel.clone(),
                        janitor_interval,
                    );
                    (State::Ready(store), Some(Arc::new(handle)), Some(task))
                }
                Err(reason) => {
                    tracing::warn!(?reason, "file stash runtime initialization blocked");
                    (State::Blocked(reason), None, None)
                }
            };
            Self {
                root: Arc::new(root),
                _root_handle: root_handle,
                state: Arc::new(Mutex::new(state)),
                janitor_admission: admission,
                janitor_cancel: cancel,
                janitor_task: Mutex::new(janitor_task),
            }
        }
    }
    pub(crate) async fn status(&self) -> FileStashStatus {
        match &*self.state.lock().await {
            State::Ready(_) => FileStashStatus::Ready,
            State::Blocked(reason) => FileStashStatus::Blocked(*reason),
            State::Shutdown => FileStashStatus::Shutdown,
        }
    }
    pub(crate) async fn store(&self) -> Result<FileStashStore, FileStashBlockedReason> {
        match &*self.state.lock().await {
            State::Ready(store) => Ok(store.clone()),
            State::Blocked(reason) => Err(*reason),
            State::Shutdown => Err(FileStashBlockedReason::Unavailable),
        }
    }
    pub(crate) async fn shutdown(&self) {
        let store = match &*self.state.lock().await {
            State::Ready(store) => Some(store.clone()),
            State::Blocked(_) | State::Shutdown => None,
        };
        self.janitor_admission.close();
        self.janitor_cancel.cancel();
        if let Some(task) = self.janitor_task.lock().await.take() {
            drop(task.await);
        }
        if let Some(store) = store
            && let Err(error) = store.checkpoint().await
        {
            tracing::warn!(?error, "file stash shutdown checkpoint failed");
        }
        if let Some(store) = match &*self.state.lock().await {
            State::Ready(store) => Some(store.clone()),
            State::Blocked(_) | State::Shutdown => None,
        } {
            store.close();
        }
        *self.state.lock().await = State::Shutdown;
    }
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

fn spawn_janitor(
    store: FileStashStore,
    admission: Arc<Semaphore>,
    cancel: tokio_util::sync::CancellationToken,
    interval: std::time::Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                () = tokio::time::sleep(interval) => {
                    let Ok(_permit) = Arc::clone(&admission).try_acquire_owned() else { continue };
                    if let Err(error) = store.with_connection(|connection| {
                        connection.query_row("SELECT 1", [], |_| Ok(())).map_err(FileStashStoreError::sqlite)
                    }).await {
                        tracing::warn!(?error, "file stash janitor health pass failed");
                    }
                }
            }
        }
    })
}

async fn initialize_owned(root: &Path) -> Result<(FileStashStore, File), FileStashBlockedReason> {
    let owned = root.to_path_buf();
    let verified = tokio::task::spawn_blocking(move || prepare_root(&owned))
        .await
        .map_err(|_| FileStashBlockedReason::Unavailable)??;
    prepare_database_files(&verified.handle).map_err(|_| FileStashBlockedReason::UnsafeRoot)?;
    let marker =
        read_or_create_marker(&verified.handle).map_err(|_| FileStashBlockedReason::UnsafeRoot)?;
    let database = anchored_child_path(&verified.handle, &verified.path, "metadata.sqlite3")?;
    let store = FileStashStore::open(database, marker)
        .await
        .map_err(map_store_error)?;
    verify_database_identity(&verified.handle, store.path())?;
    Ok((store, verified.handle))
}
fn map_store_error(error: FileStashStoreError) -> FileStashBlockedReason {
    match error {
        FileStashStoreError::Corrupt => FileStashBlockedReason::Corrupt,
        FileStashStoreError::NewerSchema(_) => FileStashBlockedReason::NewerSchema,
        FileStashStoreError::BackupMismatch => FileStashBlockedReason::BackupMismatch,
        FileStashStoreError::Busy | FileStashStoreError::Unavailable => {
            FileStashBlockedReason::Unavailable
        }
    }
}

#[cfg(unix)]
struct VerifiedRoot {
    handle: File,
    path: PathBuf,
}
#[cfg(not(unix))]
struct VerifiedRoot {
    handle: File,
    path: PathBuf,
}

#[cfg(unix)]
fn prepare_root(root: &Path) -> Result<VerifiedRoot, FileStashBlockedReason> {
    use rustix::fs::{Mode, OFlags, mkdirat, openat};
    if !root.is_absolute() {
        return Err(FileStashBlockedReason::UnsafeRoot);
    }
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut fd = openat(rustix::fs::CWD, "/", flags, Mode::empty())
        .map_err(|_| FileStashBlockedReason::UnsafeRoot)?;
    for component in root.components() {
        let std::path::Component::Normal(name) = component else {
            continue;
        };
        fd = match openat(&fd, name, flags, Mode::empty()) {
            Ok(next) => next,
            Err(rustix::io::Errno::NOENT) => {
                mkdirat(&fd, name, Mode::RUSR | Mode::WUSR | Mode::XUSR)
                    .map_err(|_| FileStashBlockedReason::Permission)?;
                openat(&fd, name, flags, Mode::empty())
                    .map_err(|_| FileStashBlockedReason::UnsafeRoot)?
            }
            Err(_) => return Err(FileStashBlockedReason::UnsafeRoot),
        };
    }
    let path = std::fs::canonicalize(root).map_err(|_| FileStashBlockedReason::UnsafeRoot)?;
    let root = File::from(fd);
    validate_private_directory(&root)?;
    for name in ["blobs", "tmp"] {
        let child = match openat(&root, name, flags, Mode::empty()) {
            Ok(child) => child,
            Err(rustix::io::Errno::NOENT) => {
                mkdirat(&root, name, Mode::RUSR | Mode::WUSR | Mode::XUSR)
                    .map_err(|_| FileStashBlockedReason::Permission)?;
                openat(&root, name, flags, Mode::empty())
                    .map_err(|_| FileStashBlockedReason::UnsafeRoot)?
            }
            Err(_) => return Err(FileStashBlockedReason::UnsafeRoot),
        };
        validate_private_directory(&File::from(child))?;
    }
    verify_root_identity(&root, &path)?;
    Ok(VerifiedRoot { handle: root, path })
}
#[cfg(not(unix))]
fn prepare_root(_: &Path) -> Result<VerifiedRoot, FileStashBlockedReason> {
    // Fail closed until a handle-relative Windows creator is available in the
    // sanctioned labby-winjob boundary.
    Err(FileStashBlockedReason::UnsafeRoot)
}

#[cfg(unix)]
fn verify_root_identity(handle: &File, path: &Path) -> Result<(), FileStashBlockedReason> {
    use std::os::unix::fs::MetadataExt as _;
    let expected = handle
        .metadata()
        .map_err(|_| FileStashBlockedReason::Unavailable)?;
    let observed = std::fs::metadata(path).map_err(|_| FileStashBlockedReason::Unavailable)?;
    if expected.dev() != observed.dev() || expected.ino() != observed.ino() {
        return Err(FileStashBlockedReason::UnsafeRoot);
    }
    Ok(())
}
#[cfg(unix)]
fn validate_private_directory(file: &File) -> Result<(), FileStashBlockedReason> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let metadata = file
        .metadata()
        .map_err(|_| FileStashBlockedReason::Unavailable)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(FileStashBlockedReason::Permission);
    }
    Ok(())
}

#[cfg(unix)]
fn read_or_create_marker(root: &File) -> std::io::Result<String> {
    use rustix::fs::{Mode, OFlags, openat};
    let opened = openat(
        root,
        "snapshot-id",
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    );
    match opened {
        Ok(fd) => {
            let mut file = File::from(fd);
            let id = ulid::Ulid::new().to_string();
            file.write_all(id.as_bytes())?;
            file.sync_all()?;
            Ok(id)
        }
        Err(rustix::io::Errno::EXIST) => {
            let fd = openat(
                root,
                "snapshot-id",
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?;
            let mut file = File::from(fd);
            validate_private_marker(&file)?;
            let mut id = String::new();
            (&mut file).take(27).read_to_string(&mut id)?;
            if id.len() != 26 || !id.bytes().all(valid_ulid_byte) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid snapshot marker",
                ));
            }
            Ok(id)
        }
        Err(error) => Err(std::io::Error::from(error)),
    }
}
#[cfg(not(unix))]
fn read_or_create_marker(_: &File) -> std::io::Result<String> {
    Err(std::io::Error::other("unsupported File Stash platform"))
}
fn valid_ulid_byte(byte: u8) -> bool {
    byte.is_ascii_digit()
        || matches!(byte, b'A'..=b'H' | b'J'..=b'K' | b'M'..=b'N' | b'P'..=b'T' | b'V'..=b'Z')
}
#[cfg(unix)]
fn validate_private_marker(file: &File) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.len() != 26
    {
        return Err(std::io::Error::other("unsafe File Stash snapshot marker"));
    }
    Ok(())
}

#[cfg(unix)]
fn prepare_database_files(root: &File) -> std::io::Result<()> {
    use rustix::fs::{Mode, OFlags, openat};
    match openat(
        root,
        "metadata.sqlite3",
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    ) {
        Ok(fd) => validate_private_regular(&File::from(fd)),
        Err(rustix::io::Errno::EXIST) => {
            let fd = openat(
                root,
                "metadata.sqlite3",
                OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?;
            validate_private_regular(&File::from(fd))
        }
        Err(error) => Err(std::io::Error::from(error)),
    }?;
    for name in ["metadata.sqlite3-wal", "metadata.sqlite3-shm"] {
        match openat(
            root,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => validate_private_regular(&File::from(fd))?,
            Err(rustix::io::Errno::NOENT) => {}
            Err(error) => return Err(std::io::Error::from(error)),
        }
    }
    Ok(())
}
#[cfg(not(unix))]
fn prepare_database_files(_: &File) -> std::io::Result<()> {
    Err(std::io::Error::other("unsupported File Stash platform"))
}
#[cfg(unix)]
fn validate_private_regular(file: &File) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(std::io::Error::other("unsafe File Stash database file"));
    }
    Ok(())
}

#[cfg(unix)]
fn verify_database_identity(root: &File, opened_path: &Path) -> Result<(), FileStashBlockedReason> {
    use rustix::fs::{Mode, OFlags, openat};
    use std::os::unix::fs::MetadataExt as _;
    let fd = openat(
        root,
        "metadata.sqlite3",
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| FileStashBlockedReason::UnsafeRoot)?;
    let anchored = File::from(fd);
    let expected = anchored
        .metadata()
        .map_err(|_| FileStashBlockedReason::Unavailable)?;
    let observed =
        std::fs::metadata(opened_path).map_err(|_| FileStashBlockedReason::Unavailable)?;
    if expected.dev() != observed.dev() || expected.ino() != observed.ino() {
        return Err(FileStashBlockedReason::UnsafeRoot);
    }
    Ok(())
}
#[cfg(not(unix))]
fn verify_database_identity(_: &File, _: &Path) -> Result<(), FileStashBlockedReason> {
    Err(FileStashBlockedReason::UnsafeRoot)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn anchored_child_path(
    root: &File,
    _: &Path,
    child: &str,
) -> Result<PathBuf, FileStashBlockedReason> {
    use std::os::fd::AsRawFd as _;
    Ok(PathBuf::from(format!(
        "/proc/self/fd/{}/{child}",
        root.as_raw_fd()
    )))
}
#[cfg(target_os = "macos")]
fn anchored_child_path(
    _: &File,
    root_path: &Path,
    child: &str,
) -> Result<PathBuf, FileStashBlockedReason> {
    Ok(root_path.join(child))
}
#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn anchored_child_path(_: &File, _: &Path, _: &str) -> Result<PathBuf, FileStashBlockedReason> {
    Err(FileStashBlockedReason::UnsafeRoot)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn root(temp: &tempfile::TempDir, name: &str) -> PathBuf {
        std::fs::canonicalize(temp.path()).unwrap().join(name)
    }
    #[tokio::test]
    #[cfg(any(target_os = "linux", target_os = "android"))]
    async fn initializes_restarts_checkpoints_and_detects_mismatch() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = root(&temp, "stash");
        let runtime = FileStashRuntime::initialize(root.clone()).await;
        assert_eq!(runtime.status().await, FileStashStatus::Ready);
        runtime.shutdown().await;
        assert_eq!(runtime.status().await, FileStashStatus::Shutdown);
        assert_eq!(
            FileStashRuntime::initialize(root.clone())
                .await
                .status()
                .await,
            FileStashStatus::Ready
        );
        std::fs::write(root.join("snapshot-id"), "01J00000000000000000000000").unwrap();
        assert_eq!(
            FileStashRuntime::initialize(root).await.status().await,
            FileStashStatus::Blocked(FileStashBlockedReason::BackupMismatch)
        );
    }
    #[tokio::test]
    #[cfg(any(target_os = "linux", target_os = "android"))]
    async fn schema_enforces_cross_table_names_and_grantee_separation() {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime = FileStashRuntime::initialize(root(&temp, "stash")).await;
        let store = runtime.store().await.unwrap();
        store.with_connection(|connection| {
            connection.execute("INSERT INTO files VALUES('a','owner','Name','name',0,'a',1,1,1)",[]).map_err(FileStashStoreError::sqlite)?;
            assert!(connection.execute("INSERT INTO pending_uploads VALUES('u','owner','NAME','name',0,'pending',9,1,1)",[]).is_err());
            assert!(connection.execute("INSERT INTO grants VALUES('g','a','owner','active',1,NULL)",[]).is_err());
            connection.execute("INSERT INTO grants VALUES('g','a','other','active',1,NULL)",[]).map_err(FileStashStoreError::sqlite)?;
            assert!(connection.execute("UPDATE grants SET grantee_principal_id='owner' WHERE grant_id='g'",[]).is_err());
            let plan: String = connection.query_row(
                "EXPLAIN QUERY PLAN SELECT file_id FROM files WHERE owner_principal_id='owner' AND ready=1 ORDER BY created_at DESC,file_id DESC LIMIT 50",
                [],
                |row| row.get(3),
            ).map_err(FileStashStoreError::sqlite)?;
            assert!(plan.contains("stash_files_owner_list"), "unexpected plan: {plan}");
            Ok(())
        }).await.unwrap();
    }
    #[tokio::test]
    #[cfg(any(target_os = "linux", target_os = "android"))]
    async fn rejects_intermediate_symlink_and_insecure_existing_mode() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};
        let temp = tempfile::TempDir::new().unwrap();
        let canonical = std::fs::canonicalize(temp.path()).unwrap();
        let target = canonical.join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = canonical.join("link");
        symlink(&target, &link).unwrap();
        assert_eq!(
            FileStashRuntime::initialize(link.join("stash"))
                .await
                .status()
                .await,
            FileStashStatus::Blocked(FileStashBlockedReason::UnsafeRoot)
        );
        let insecure = canonical.join("insecure");
        std::fs::create_dir(&insecure).unwrap();
        std::fs::set_permissions(&insecure, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            FileStashRuntime::initialize(insecure).await.status().await,
            FileStashStatus::Blocked(FileStashBlockedReason::Permission)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(any(target_os = "linux", target_os = "android"))]
    async fn database_queue_saturates_and_shutdown_closes_existing_handles() {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime = FileStashRuntime::initialize(root(&temp, "stash")).await;
        let store = runtime.store().await.unwrap();
        let held = store.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let entered = Arc::clone(&barrier);
        let task = tokio::spawn(async move {
            held.with_connection(move |_| {
                entered.wait();
                std::thread::sleep(std::time::Duration::from_millis(250));
                Ok(())
            })
            .await
        });
        barrier.wait();
        assert!(matches!(
            store.with_connection(|_| Ok(())).await,
            Err(FileStashStoreError::Busy)
        ));
        task.await.unwrap().unwrap();
        runtime.shutdown().await;
        assert!(matches!(
            store.with_connection(|_| Ok(())).await,
            Err(FileStashStoreError::Unavailable)
        ));
    }

    #[tokio::test]
    #[cfg(any(target_os = "linux", target_os = "android"))]
    async fn rejects_corrupt_schema_fingerprint() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = root(&temp, "stash");
        let runtime = FileStashRuntime::initialize(root.clone()).await;
        runtime.shutdown().await;
        let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
        connection
            .execute(
                "UPDATE stash_metadata SET schema_fingerprint='tampered' WHERE singleton=1",
                [],
            )
            .unwrap();
        drop(connection);
        assert_eq!(
            FileStashRuntime::initialize(root).await.status().await,
            FileStashStatus::Blocked(FileStashBlockedReason::Corrupt)
        );
    }

    #[tokio::test]
    #[cfg(any(target_os = "linux", target_os = "android"))]
    async fn rejects_future_schema_without_partial_migration() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = root(&temp, "stash");
        let runtime = FileStashRuntime::initialize(root.clone()).await;
        runtime.shutdown().await;
        let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
        drop(connection);
        assert_eq!(
            FileStashRuntime::initialize(root.clone())
                .await
                .status()
                .await,
            FileStashStatus::Blocked(FileStashBlockedReason::NewerSchema)
        );
        let connection = rusqlite::Connection::open(root.join("metadata.sqlite3")).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_fails_closed_without_descriptor_rooted_sqlite() {
        let temp = tempfile::TempDir::new().unwrap();
        let runtime = FileStashRuntime::initialize(temp.path().join("stash")).await;
        assert_eq!(
            runtime.status().await,
            FileStashStatus::Blocked(FileStashBlockedReason::UnsafeRoot)
        );
        assert!(!temp.path().join("stash").exists());
    }
}
