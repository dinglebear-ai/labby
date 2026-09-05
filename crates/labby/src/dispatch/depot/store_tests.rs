use super::store::{Pair, Store, StoreError};
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
