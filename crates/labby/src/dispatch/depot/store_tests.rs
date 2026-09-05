use super::store::{Pair, Store, StoreError};
use crate::config::host_write::HostConfigLock;
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

    let reader = std::thread::spawn(move || store.read_pair());
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        match HostConfigLock::acquire_with_timeout(&config_path, Duration::from_millis(5)) {
            Err(_) => break,
            Ok(lock) => drop(lock),
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pair reader never acquired its first lock"
        );
        std::thread::yield_now();
    }
    drop(environment);
    assert_eq!(
        reader.join().unwrap().unwrap(),
        Pair {
            config: "config".into(),
            environment: "ENV=value\n".into(),
        }
    );
}
