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
