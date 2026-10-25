use super::store::{Pair, Store, StoreError};
use crate::config::host_write::HostConfigLock;
use std::sync::mpsc;
use std::time::Duration;
use tempfile::TempDir;

fn store(root: &TempDir) -> Store {
    Store::new(
        root.path().join("config.toml"),
        root.path().join("labby.env"),
        root.path().join("transactions"),
    )
}

#[test]
fn committed_pair_survives_response_loss_and_same_operation_retries() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    std::fs::write(root.path().join("config.toml"), "old-config").unwrap();
    std::fs::write(root.path().join("labby.env"), "OLD=secret\n").unwrap();
    let version = store.current_version().unwrap();
    let pair = Pair {
        config: "new-config".into(),
        environment: "NEW=secret\n".into(),
    };
    let first = store.commit("operation-1", &version, &pair).unwrap();
    let replay = store.commit("operation-1", &version, &pair).unwrap();
    assert_eq!(first, replay);
    assert_eq!(
        std::fs::read_to_string(root.path().join("config.toml")).unwrap(),
        "new-config"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("labby.env")).unwrap(),
        "NEW=secret\n"
    );
}

#[test]
fn stale_version_preserves_both_active_files() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    std::fs::write(root.path().join("config.toml"), "old-config").unwrap();
    std::fs::write(root.path().join("labby.env"), "OLD=secret\n").unwrap();
    let pair = Pair {
        config: "new-config".into(),
        environment: "NEW=secret\n".into(),
    };
    assert_eq!(
        store.commit("operation-1", "stale", &pair).unwrap_err(),
        StoreError::Stale
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("config.toml")).unwrap(),
        "old-config"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("labby.env")).unwrap(),
        "OLD=secret\n"
    );
}

#[test]
fn corrupt_recovery_intent_blocks_activation_without_touching_the_pair() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    std::fs::write(root.path().join("config.toml"), "safe-config").unwrap();
    std::fs::write(root.path().join("labby.env"), "SAFE=secret\n").unwrap();
    std::fs::create_dir_all(root.path().join("transactions")).unwrap();
    std::fs::write(root.path().join("transactions/active.json"), "{broken").unwrap();
    assert_eq!(store.recover().unwrap_err(), StoreError::RecoveryRequired);
    assert_eq!(
        std::fs::read_to_string(root.path().join("config.toml")).unwrap(),
        "safe-config"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("labby.env")).unwrap(),
        "SAFE=secret\n"
    );
}

#[test]
fn pair_read_holds_the_config_lock_while_waiting_for_the_environment_lock() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    let config_path = root.path().join("config.toml");
    let environment_path = root.path().join("labby.env");
    std::fs::write(&config_path, "config").unwrap();
    std::fs::write(&environment_path, "ENV=value\n").unwrap();
    let environment = HostConfigLock::acquire(&environment_path).unwrap();

    let (config_acquired, wait_for_config) = mpsc::sync_channel(0);
    let reader = std::thread::spawn(move || store.read_pair_notifying(config_acquired));
    wait_for_config
        .recv_timeout(Duration::from_secs(5))
        .expect("pair reader did not acquire the config lock");
    assert!(HostConfigLock::acquire_with_timeout(&config_path, Duration::from_millis(10)).is_err());
    drop(environment);
    assert_eq!(
        reader.join().unwrap().unwrap(),
        Pair {
            config: "config".into(),
            environment: "ENV=value\n".into(),
        }
    );
}

#[test]
fn completed_receipt_recovery_and_retry_preserve_later_host_edits() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    let version = store.current_version().unwrap();
    let pair = Pair {
        config: "provider-config".into(),
        environment: "TOKEN=old\n".into(),
    };
    let outcome = store.commit("completed", &version, &pair).unwrap();
    let config = root.path().join("config.toml");
    let environment = root.path().join("labby.env");
    HostConfigLock::acquire(&config)
        .unwrap()
        .write("newer-host-config")
        .unwrap();
    HostConfigLock::acquire(&environment)
        .unwrap()
        .write("ROTATED=new\n")
        .unwrap();
    assert_eq!(store.recover().unwrap(), Some(outcome.clone()));
    assert_eq!(store.commit("completed", &version, &pair).unwrap(), outcome);
    assert_eq!(
        store.commit("different", &version, &pair),
        Err(StoreError::Stale)
    );
    assert_eq!(
        std::fs::read_to_string(config).unwrap(),
        "newer-host-config"
    );
    assert_eq!(
        std::fs::read_to_string(environment).unwrap(),
        "ROTATED=new\n"
    );
}

#[test]
fn prepared_recovery_rejects_unrelated_edits_without_restoring_either_file() {
    let root = TempDir::new().unwrap();
    let store = store(&root);
    let version = store.current_version().unwrap();
    store
        .commit(
            "interrupted",
            &version,
            &Pair {
                config: "new".into(),
                environment: "NEW=1\n".into(),
            },
        )
        .unwrap();
    let intent_path = root.path().join("transactions/active.json");
    let mut intent: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&intent_path).unwrap()).unwrap();
    intent["phase"] = serde_json::json!("Prepared");
    std::fs::write(&intent_path, serde_json::to_vec(&intent).unwrap()).unwrap();
    std::fs::write(root.path().join("config.toml"), "unrelated").unwrap();
    assert_eq!(store.recover(), Err(StoreError::RecoveryRequired));
    assert_eq!(
        std::fs::read_to_string(root.path().join("config.toml")).unwrap(),
        "unrelated"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("labby.env")).unwrap(),
        "NEW=1\n"
    );
}

#[test]
fn prepared_recovery_restores_partial_pair_once_then_retires_intent() {
    for (config_written, environment_written) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        let root = TempDir::new().unwrap();
        let store = store(&root);
        std::fs::write(root.path().join("config.toml"), "old").unwrap();
        std::fs::write(root.path().join("labby.env"), "OLD=1\n").unwrap();
        let version = store.current_version().unwrap();
        store
            .commit(
                "interrupted",
                &version,
                &Pair {
                    config: "new".into(),
                    environment: "NEW=1\n".into(),
                },
            )
            .unwrap();
        let intent_path = root.path().join("transactions/active.json");
        let mut intent: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&intent_path).unwrap()).unwrap();
        intent["phase"] = serde_json::json!("Prepared");
        std::fs::write(&intent_path, serde_json::to_vec(&intent).unwrap()).unwrap();
        if !config_written {
            std::fs::write(root.path().join("config.toml"), "old").unwrap();
        }
        if !environment_written {
            std::fs::write(root.path().join("labby.env"), "OLD=1\n").unwrap();
        }
        assert_eq!(store.recover().unwrap(), None);
        assert_eq!(
            std::fs::read_to_string(root.path().join("config.toml")).unwrap(),
            "old"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("labby.env")).unwrap(),
            "OLD=1\n"
        );
        std::fs::write(root.path().join("config.toml"), "later").unwrap();
        assert_eq!(store.recover().unwrap(), None);
        assert_eq!(
            std::fs::read_to_string(root.path().join("config.toml")).unwrap(),
            "later"
        );
    }
}

#[test]
fn publication_holds_both_file_locks_and_skips_superseded_replays() {
    use crate::config::host_write::HostWriteError;
    let root = TempDir::new().unwrap();
    let store = store(&root);
    let expected = store.current_version().unwrap();
    let pair = Pair {
        config: "committed".into(),
        environment: "TOKEN=committed\n".into(),
    };
    let publish = || {
        for path in [
            root.path().join("config.toml"),
            root.path().join("labby.env"),
        ] {
            assert!(matches!(
                HostConfigLock::acquire_with_timeout(&path, Duration::ZERO),
                Err(HostWriteError::Busy)
            ));
        }
        Ok(())
    };
    let first = store
        .commit_with_publication("operation", &expected, &pair, publish)
        .unwrap();
    assert_eq!(
        store
            .commit_with_publication("operation", &expected, &pair, publish)
            .unwrap(),
        first
    );
    HostConfigLock::acquire(&root.path().join("config.toml"))
        .unwrap()
        .write("superseding")
        .unwrap();
    assert_eq!(
        store
            .commit_with_publication("operation", &expected, &pair, || panic!(
                "historical replay must not publish"
            ))
            .unwrap(),
        first
    );
    assert_eq!(store.read_pair().unwrap().config, "superseding");
}
