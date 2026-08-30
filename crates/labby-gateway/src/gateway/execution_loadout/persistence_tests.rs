use super::*;

#[test]
fn store_path_is_fixed_inside_canonical_config_directory() {
    let directory = tempfile::tempdir().expect("temporary configuration directory");
    let configured = directory.path().join("custom.toml");

    let path = store_path(&configured).expect("resolve store path");

    assert_eq!(
        path,
        directory
            .path()
            .canonicalize()
            .expect("canonical temporary directory")
            .join("execution-loadouts.json")
    );
}

#[test]
fn store_path_rejects_a_missing_config_directory() {
    let directory = tempfile::tempdir().expect("temporary configuration directory");
    let configured = directory.path().join("missing").join("custom.toml");

    assert!(store_path(&configured).is_err());
}
