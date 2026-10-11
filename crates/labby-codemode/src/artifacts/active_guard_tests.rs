use super::*;

#[tokio::test]
async fn nested_runner_guard_does_not_release_final_preservation_retention() {
    let store = tempfile::tempdir().unwrap();
    let id = Ulid::new();
    let newer_id = Ulid::from(u128::from(id) + 1).to_string();
    let run_id = id.to_string();
    let root = store.path().join(&run_id);
    tokio::fs::create_dir(&root).await.unwrap();
    tokio::fs::write(root.join("result.json"), b"saved")
        .await
        .unwrap();
    tokio::fs::create_dir(store.path().join(newer_id))
        .await
        .unwrap();
    let execution = ActiveArtifactRun::register(&run_id);
    let runner = ActiveArtifactRun::register(&run_id);
    drop(runner);
    prune_artifact_runs_locked(store.path(), 1, 0, &active_artifact_runs_snapshot()).await;
    assert_eq!(
        tokio::fs::read(root.join("result.json")).await.unwrap(),
        b"saved"
    );
    drop(execution);
    prune_artifact_runs_locked(store.path(), 1, 0, &active_artifact_runs_snapshot()).await;
    assert!(!root.exists());
}

#[tokio::test]
async fn artifact_writes_reject_metadata_aliases_before_publication() {
    let store = tempfile::tempdir().unwrap();
    let root = store.path().join(Ulid::new().to_string());
    for path in [
        ".labby-artifact-metadata/payload",
        "./.labby-artifact-metadata/payload",
        "././.labby-artifact-metadata/payload",
        r".\.\.labby-artifact-metadata\payload",
        ".//./.labby-artifact-metadata/payload",
        ".LABBY-ARTIFACT-METADATA/payload",
        "./.Labby-Artifact-Metadata/payload",
        r".\.lAbBy-ArTiFaCt-MeTaDaTa\payload",
    ] {
        let error = write_code_mode_artifact(
            &root,
            &CodeModeArtifactWrite {
                path: path.into(),
                content: "payload".into(),
                content_type: None,
            },
            1024,
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "invalid_param", "{path}");
        assert!(error.to_string().contains("reserved metadata"), "{path}");
        assert!(!root.exists(), "rejected path must create no run: {path}");
    }
}

#[tokio::test]
async fn artifact_receipts_use_the_published_normalized_path_without_clobber() {
    let store = tempfile::tempdir().unwrap();
    let root = store.path().join(Ulid::new().to_string());
    let mut request = CodeModeArtifactWrite {
        path: r".\./nested//./.labby-artifact-metadata\payload.txt".into(),
        content: "first".into(),
        content_type: None,
    };
    let receipt = write_code_mode_artifact(&root, &request, 1024)
        .await
        .unwrap();
    assert_eq!(receipt.path, "nested/.labby-artifact-metadata/payload.txt");
    assert_eq!(std::fs::read(root.join(&receipt.path)).unwrap(), b"first");
    request.path = format!("./{}", receipt.path);
    request.content = "second".into();
    assert!(
        write_code_mode_artifact(&root, &request, 1024)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(root.join(&receipt.path)).unwrap(), b"first");
}
