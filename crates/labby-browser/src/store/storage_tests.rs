//! Storage admission and lifetime regression tests.
#![allow(clippy::panic)]
use super::{BrowserStorageLock, Store, storage};

fn directory() -> tempfile::TempDir {
    super::private_test_directory()
}

#[tokio::test]
async fn ownership_survives_clones_and_releases_on_last_drop() {
    let root = directory();
    let path = root.path().join("browser.db");
    let store = Store::open(&path).await.unwrap();
    let clone = store.clone();
    assert!(Store::open(&path).await.is_err());
    drop(store);
    assert!(Store::open(&path).await.is_err());
    drop(clone);
    let reopened = Store::open(&path).await.unwrap();
    assert!(reopened.pending_pairings().await.unwrap().is_empty());
    assert!(storage::sidecar(&path, ".lock").is_file());
}

#[tokio::test]
async fn offline_guard_excludes_live_store_without_creating_sqlite() {
    let root = directory();
    let missing = root.path().join("missing/browser.db");
    assert!(
        BrowserStorageLock::acquire_existing(&missing)
            .unwrap()
            .is_none()
    );
    assert!(!missing.parent().unwrap().exists());
    let path = root.path().join("browser.db");
    let guard = BrowserStorageLock::acquire_existing(&path)
        .unwrap()
        .unwrap();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        assert!(!storage::sidecar(&path, suffix).exists());
    }
    assert!(Store::open(&path).await.is_err());
    drop(guard);
    let store = Store::open(&path).await.unwrap();
    assert!(BrowserStorageLock::acquire_existing(&path).is_err());
    drop(store);
    assert!(
        BrowserStorageLock::acquire_existing(&path)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn failed_sqlite_open_releases_owner_without_replacing_data() {
    let root = directory();
    let path = root.path().join("browser.db");
    std::fs::write(&path, b"not sqlite").unwrap();
    assert!(Store::open(&path).await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"not sqlite");
    assert!(
        BrowserStorageLock::acquire_existing(&path)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn child_process_ownership_probe() {
    let Some(path) = std::env::var_os("LABBY_BROWSER_STORE_LOCK_TEST") else {
        return;
    };
    assert!(Store::open(std::path::PathBuf::from(path)).await.is_err());
}

#[tokio::test]
async fn second_process_cannot_open_owned_database() {
    let root = directory();
    let path = root.path().join("browser.db");
    let _store = Store::open(&path).await.unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "store::storage_tests::child_process_ownership_probe",
        ])
        .env("LABBY_BROWSER_STORE_LOCK_TEST", &path)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("storage ownership child exceeded deadline");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn database_sidecars_and_ancestors_reject_aliases_without_writes() {
    use std::os::unix::fs::symlink;
    for suffix in ["", "-wal", "-shm", "-journal", ".lock"] {
        let root = directory();
        let path = root.path().join("browser.db");
        let target = root.path().join("untouched");
        std::fs::write(&target, b"sentinel").unwrap();
        symlink(&target, storage::sidecar(&path, suffix)).unwrap();
        assert!(
            Store::open(&path).await.is_err(),
            "accepted symlink {suffix}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"sentinel");
    }
    let root = directory();
    let outside = directory();
    symlink(outside.path(), root.path().join("link")).unwrap();
    assert!(
        Store::open(root.path().join("link/new/browser.db"))
            .await
            .is_err()
    );
    assert!(!outside.path().join("new").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn hardlinks_and_shared_directories_are_rejected_without_repermissioning() {
    use std::os::unix::fs::PermissionsExt;
    let root = directory();
    let target = root.path().join("untouched");
    std::fs::write(&target, b"sentinel").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
    let path = root.path().join("browser.db");
    std::fs::hard_link(&target, &path).unwrap();
    assert!(Store::open(&path).await.is_err());
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o644
    );
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Store::open(root.path().join("other.db")).await.is_err());
    assert_eq!(
        std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(!root.path().join("other.db").exists());
}

#[tokio::test]
async fn relative_and_traversal_paths_are_rejected() {
    assert!(Store::open("browser.db").await.is_err());
    let root = directory();
    assert!(
        Store::open(root.path().join("child/../browser.db"))
            .await
            .is_err()
    );
    assert!(!root.path().join("child").exists());
}

#[tokio::test]
async fn sqlite_files_are_private_before_and_after_writes() {
    let root = directory();
    let path = root.path().join("browser.db");
    let (path, owner) = storage::prepare(&path).unwrap();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let file = storage::sidecar(&path, suffix);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        #[cfg(windows)]
        storage::verify_private_windows_acl(&file).unwrap();
    }
    drop(owner);
    let store = Store::open(&path).await.unwrap();
    store
        .request_pairing("Chrome", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", vec![7; 32])
        .await
        .unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let file = storage::sidecar(&path, suffix);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        #[cfg(windows)]
        {
            // SQLite can recreate sidecars: verify effective parent policy remains private.
            let handle = labby_winjob::fs::open_directory(path.parent().unwrap()).unwrap();
            labby_winjob::fs::verify_private_directory_dacl(&handle).unwrap();
            assert!(file.is_file());
        }
    }
}
