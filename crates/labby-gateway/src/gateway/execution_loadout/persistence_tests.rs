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

#[test]
fn load_rejects_inconsistent_revision_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let configured = directory.path().join("labby.toml");
    let path = store_path(&configured).unwrap();
    let corrupt = serde_json::json!({
        "records": {
            "11:principal-1broken": {
                "draft": {
                    "id": "broken", "ownerPrincipal": "principal-1", "runtimeIdentity": "runtime",
                    "name": "Broken", "description": null, "members": [], "draftRevision": 1,
                    "desiredActiveRevision": 1, "effectiveRuntimeRevision": 1, "restartRequired": false
                },
                "revisions": {
                    "1": { "loadoutId": "different", "revision": 1, "draftRevision": 1,
                           "members": [], "catalogGeneration": "generation" }
                }
            }
        }
    });
    fs::write(&path, serde_json::to_vec(&corrupt).unwrap()).unwrap();
    assert!(ExecutionLoadoutStore::load(&configured).is_err());
}
