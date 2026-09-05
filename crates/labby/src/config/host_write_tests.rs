use super::host_write::{HostConfigLock, HostWriteError};

#[test]
fn separate_process_writer_preserves_update_made_while_waiting() {
    const KEY: &str = "LABBY_TEST_HOST_LOCK_PATH";
    if let Some(path) = std::env::var_os(KEY) {
        super::patch_config_scalars(
            std::path::Path::new(&path),
            &[super::ConfigScalarPatch::new(
                "mcp.port",
                super::ConfigScalarValue::I64(8766),
            )],
        )
        .unwrap();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let lock = HostConfigLock::acquire(&path).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "config::host_write_tests::separate_process_writer_preserves_update_made_while_waiting",
        ])
        .env(KEY, &path)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(child.try_wait().unwrap().is_none());
    lock.write("[depot]\npublic_enabled=false\n").unwrap();
    drop(lock);
    assert!(child.wait().unwrap().success());
    let config = super::load_toml(&[path]).unwrap();
    assert!(!config.depot.public_enabled);
    assert_eq!(config.mcp.port, Some(8766));
}

#[test]
fn startup_rejects_oversized_configuration_before_deserialization() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "#".to_owned() + &" ".repeat(8 * 1024 * 1024)).unwrap();
    assert!(super::load_toml(&[path]).is_err());
}

#[test]
fn provider_patch_preserves_unknown_nested_fields_and_quarantined_siblings() {
    let mut document: toml_edit::DocumentMut = r#"
[[depot.providers]]
id = "broken"
enabled = "invalid but retained"
[[depot.providers]]
id = "team"
name = "Old"
endpoint = "https://example.com"
enabled = true
auth_mode = "anonymous"
[depot.providers.future]
# preserve this comment
value = 42
"#
    .parse()
    .unwrap();
    let provider = super::depot::ProviderConfig {
        id: "team".into(),
        name: "New".into(),
        endpoint: "https://example.com".into(),
        enabled: false,
        auth_mode: super::depot::AuthMode::Anonymous,
        bearer_token_env: None,
    };
    super::host_write::upsert_depot_provider(&mut document, &provider).unwrap();
    let raw = document.to_string();
    assert!(raw.contains("invalid but retained"));
    assert!(raw.contains("# preserve this comment"));
    assert!(raw.contains("value = 42"));
    assert!(raw.contains("name = \"New\""));
}

#[test]
fn legacy_env_writer_never_returns_existing_secret_in_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".env");
    std::fs::write(&path, "TOKEN=never-return-this-secret\n").unwrap();
    let conflicts =
        super::write_env_pairs(&path, &[("TOKEN".into(), "new".into())], false).unwrap();
    assert!(!format!("{conflicts:?}").contains("never-return-this-secret"));
}

#[cfg(unix)]
#[test]
fn env_merge_rejects_symlink_target_without_reading_or_replacing_it() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let path = dir.path().join(".env");
    std::fs::write(&target, "TOKEN=secret\n").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let result = super::env_merge::merge(
        &path,
        super::env_merge::MergeRequest {
            entries: vec![super::env_merge::EnvEntry::new("TOKEN", "new")],
            force: true,
            expected_mtime: None,
        },
    );
    assert!(result.is_err());
    assert!(
        std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read_to_string(target).unwrap(), "TOKEN=secret\n");
}

#[test]
fn scalar_writer_uses_shared_lock_before_reading() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let lock = HostConfigLock::acquire(&path).unwrap();
    let worker_path = path.clone();
    let worker = std::thread::spawn(move || {
        super::patch_config_scalars(
            &worker_path,
            &[super::ConfigScalarPatch::new(
                "web.dev_auth_bypass",
                super::ConfigScalarValue::Bool(false),
            )],
        )
    });
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(
        !worker.is_finished(),
        "scalar writer must wait for the shared host lock"
    );
    lock.write("[depot.future]\nvalue=42\n").unwrap();
    drop(lock);
    worker.join().unwrap().unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("value=42"));
}

#[cfg(feature = "gateway")]
#[test]
fn gateway_write_preserves_raw_depot_comments_and_unknown_fields() {
    use labby_gateway::gateway::config_store::GatewayConfigStore as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let depot = "[depot]\n# operator note\npublic_enabled=false\n[depot.future]\nvalue=42\n";
    std::fs::write(&path, depot).unwrap();
    let config: super::LabConfig = toml::from_str(depot).unwrap();
    let store = crate::dispatch::gateway::config_store::LabConfigStore::new(
        std::sync::Arc::new(std::sync::RwLock::new(config.clone())),
        path.clone(),
    );
    store.persist(&config.to_gateway_config()).unwrap();
    assert!(std::fs::read_to_string(path).unwrap().contains(depot));
}

#[test]
fn host_lock_reloads_and_preserves_unknown_nested_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "# keep\n[depot.future]\nvalue=42\n").unwrap();
    let lock = HostConfigLock::acquire(&path).unwrap();
    let mut document = lock.read().unwrap();
    document["depot"]["public_enabled"] = toml_edit::value(false);
    lock.write(&document.to_string()).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("# keep"));
    assert!(raw.contains("value=42"));
    assert!(raw.contains("public_enabled = false"));
}

#[test]
fn host_lock_has_bounded_wait_and_can_be_reacquired() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let lock = HostConfigLock::acquire(&path).unwrap();
    assert!(matches!(
        HostConfigLock::acquire_with_timeout(&path, std::time::Duration::from_millis(10)),
        Err(HostWriteError::Busy)
    ));
    drop(lock);
    assert!(HostConfigLock::acquire(&path).is_ok());
}

#[cfg(unix)]
#[test]
fn host_lock_rejects_symlink_targets_and_writes_private_files() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let path = dir.path().join("config.toml");
    std::fs::write(&target, "").unwrap();
    symlink(&target, &path).unwrap();
    assert!(HostConfigLock::acquire(&path).is_err());
    std::fs::remove_file(&path).unwrap();
    let lock = HostConfigLock::acquire(&path).unwrap();
    lock.write("[depot]\npublic_enabled=false\n").unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
