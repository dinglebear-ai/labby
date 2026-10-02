use super::*;
#[tokio::test]
async fn read_checks_digest_size_retention_and_containment() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("code-mode-artifacts/run1");
    std::fs::create_dir_all(&root).unwrap();
    let bytes = b"authorized artifact";
    std::fs::write(root.join("report.txt"), bytes).unwrap();
    let digest = hex::encode(Sha256::digest(bytes));
    assert_eq!(
        read_receipted_artifact(home.path(), "run1", "report.txt", &digest, bytes.len())
            .await
            .unwrap(),
        bytes
    );
    assert!(
        read_receipted_artifact(home.path(), "run1", "report.txt", "wrong", bytes.len())
            .await
            .is_err()
    );
    assert!(
        read_receipted_artifact(home.path(), "run1", "report.txt", &digest, bytes.len() + 1)
            .await
            .is_err()
    );
    assert!(
        read_receipted_artifact(home.path(), "run1", "../report.txt", &digest, bytes.len())
            .await
            .is_err()
    );
    assert!(
        read_receipted_artifact(home.path(), "../run1", "report.txt", &digest, bytes.len())
            .await
            .is_err()
    );
    assert_eq!(
        read_receipted_artifact(
            home.path(),
            "run1",
            "report.txt",
            &digest,
            MAX_DOWNLOAD_BYTES + 1
        )
        .await
        .unwrap_err()
        .kind(),
        "invalid_param"
    );
    std::fs::remove_file(root.join("report.txt")).unwrap();
    assert_eq!(
        read_receipted_artifact(home.path(), "run1", "report.txt", &digest, bytes.len())
            .await
            .unwrap_err()
            .kind(),
        "artifact_unavailable"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn read_rejects_symlinks_in_every_artifact_component() {
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let content = b"same authorized content";
    let digest = hex::encode(Sha256::digest(content));
    std::fs::write(other.path().join("file.txt"), content).unwrap();
    let store = home.path().join("code-mode-artifacts");
    std::fs::create_dir(&store).unwrap();
    symlink(other.path(), store.join("run1")).unwrap();
    assert!(
        read_receipted_artifact(home.path(), "run1", "file.txt", &digest, content.len())
            .await
            .is_err()
    );
    std::fs::remove_file(store.join("run1")).unwrap();
    std::fs::create_dir(store.join("run1")).unwrap();
    symlink(other.path().join("file.txt"), store.join("run1/file.txt")).unwrap();
    assert!(
        read_receipted_artifact(home.path(), "run1", "file.txt", &digest, content.len())
            .await
            .is_err()
    );
}
