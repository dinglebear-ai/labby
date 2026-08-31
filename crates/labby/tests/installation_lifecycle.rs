use std::fs;

use labby::installation::{InstallationLifecycleLock, InstallationPaths};

#[test]
fn explicit_root_must_be_absolute() {
    let error = InstallationPaths::from_root("relative/labby")
        .expect_err("relative installation root must fail");
    assert!(error.to_string().contains("must be absolute"));
}

#[test]
fn explicit_root_normalizes_dot_and_parent_components() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let through_parent = InstallationPaths::from_root(temp.path().join("one/../two"))
        .expect("canonical parent path");
    let through_dot =
        InstallationPaths::from_root(temp.path().join("./two")).expect("canonical dot path");
    assert_eq!(through_parent, through_dot);
}

#[test]
fn canonical_paths_share_one_root() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let paths = InstallationPaths::from_root(temp.path().join("labby")).expect("absolute root");

    assert_eq!(paths.config_toml(), paths.root().join("config.toml"));
    assert_eq!(paths.dotenv(), paths.root().join(".env"));
    assert_eq!(paths.access_db(), paths.root().join("access.db"));
    assert_eq!(paths.lifecycle_lock(), paths.root().join("lifecycle.lock"));
}

#[test]
fn daemon_excludes_daemon_and_offline_owner_until_drop() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let paths = InstallationPaths::from_root(temp.path().join("labby")).expect("absolute root");
    let daemon = InstallationLifecycleLock::acquire_daemon(&paths).expect("first daemon lock");

    assert!(InstallationLifecycleLock::acquire_daemon(&paths).is_err());
    assert!(InstallationLifecycleLock::acquire_offline(&paths).is_err());
    drop(daemon);

    let offline = InstallationLifecycleLock::acquire_offline(&paths)
        .expect("offline owner acquires after daemon stops");
    assert_eq!(offline.paths(), &paths);
}

#[cfg(unix)]
#[test]
fn creates_private_root_and_lock_file() {
    use std::os::unix::fs::MetadataExt as _;

    let temp = tempfile::tempdir().expect("temporary directory");
    let paths = InstallationPaths::from_root(temp.path().join("labby")).expect("absolute root");
    let lock = InstallationLifecycleLock::acquire_daemon(&paths).expect("daemon lock");

    assert_eq!(
        fs::metadata(paths.root()).expect("root metadata").mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(paths.lifecycle_lock())
            .expect("lock metadata")
            .mode()
            & 0o777,
        0o600
    );
    drop(lock);
}

#[cfg(unix)]
#[test]
fn rejects_symlink_root_and_lock_path() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("temporary directory");
    let actual = temp.path().join("actual");
    fs::create_dir(&actual).expect("actual root");
    let linked = temp.path().join("linked");
    symlink(&actual, &linked).expect("root symlink");
    assert!(InstallationPaths::from_root(&linked).is_err());

    let root = temp.path().join("root");
    fs::create_dir(&root).expect("root");
    let target = temp.path().join("target-lock");
    fs::write(&target, b"").expect("target lock");
    symlink(&target, root.join("lifecycle.lock")).expect("lock symlink");
    let paths = InstallationPaths::from_root(root).expect("absolute root");
    assert!(InstallationLifecycleLock::acquire_daemon(&paths).is_err());
}

#[cfg(unix)]
#[test]
fn rejects_group_writable_existing_root() {
    use std::os::unix::fs::PermissionsExt as _;

    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().join("labby");
    fs::create_dir(&root).expect("root");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o770)).expect("loosen root");
    let paths = InstallationPaths::from_root(root).expect("absolute root");

    assert!(InstallationLifecycleLock::acquire_daemon(&paths).is_err());
}
