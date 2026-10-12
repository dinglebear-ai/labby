use super::*;

#[test]
fn driver_fixture_resolves_its_owned_store_before_any_write() {
    let cfg = test_config(Duration::from_secs(5));
    let state = DriveState::for_config(&cfg);
    assert_eq!(state.artifact_root, cfg.test_artifact_root());
    assert_ne!(
        state.artifact_root,
        code_mode_artifact_root(&cfg.artifact_run_id)
    );
}

#[tokio::test]
async fn writable_driver_fixture_publishes_prunes_and_cleans_only_its_owned_store() {
    let cfg = test_config(Duration::from_secs(5));
    let store = cfg.test_artifact_store.path().to_path_buf();
    let root = cfg.test_artifact_root();
    let run = Ulid::from_string(&cfg.artifact_run_id).expect("valid owned run ID");
    let old =
        store.join(Ulid::from(u128::from(run).checked_sub(1).expect("nonzero run ID")).to_string());
    std::fs::create_dir(&old).expect("owned inactive run");
    std::fs::write(old.join("old.txt"), b"old").expect("owned inactive payload");
    let outside = tempfile::tempdir().expect("separate disposable store");
    std::fs::write(outside.path().join("sentinel"), b"unchanged").expect("outside sentinel");
    let host = NoopHost::default();
    let mut runner = PooledRunner::spawn_stub_script(
        r#"
IFS= read -r _
printf '%s\n' '{"type":"artifact_write","seq":0,"path":"owned.txt","content":"owned"}'
IFS= read -r _ || exit 21
printf '%s\n' '{"type":"done"}'
IFS= read -r _
"#,
    )
    .expect("owned artifact protocol fixture");
    let outcome = CodeModeBroker::new(Some(&host))
        .drive_runner(&mut runner, &cfg, tokio::time::Instant::now() + cfg.timeout)
        .await;
    assert!(
        matches!(&outcome, DriveOutcome::Completed(_)),
        "legal owned artifact must complete"
    );
    let DriveOutcome::Completed(response) = outcome else {
        return;
    };
    assert_eq!(response.artifacts.len(), 1);
    assert_eq!(response.artifacts[0].path, "owned.txt");
    assert_eq!(
        std::fs::read(root.join("owned.txt")).expect("owned publication"),
        b"owned"
    );
    crate::artifacts::prune_fixture_store(&cfg.test_artifact_store).await;
    assert!(!old.exists(), "retention acts inside the fixture store");
    assert_eq!(
        std::fs::read(root.join("owned.txt")).expect("retained current output"),
        b"owned"
    );
    assert_eq!(
        std::fs::read(outside.path().join("sentinel")).expect("outside payload"),
        b"unchanged"
    );
    runner.shutdown().await.expect("owned runner cleanup");
    drop(cfg);
    assert!(!store.exists(), "fixture-owned store drops after teardown");
}
