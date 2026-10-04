use super::*;
use crate::artifacts::ActiveArtifactRun;
use std::path::PathBuf;

async fn write_with_limits(
    root: &Path,
    name: &str,
    bytes: &[u8],
    store: u64,
    run: u64,
    files: u64,
) -> Result<(), ToolError> {
    let _reservation = STORE_MUTATION.lock().await;
    admit_with_limits(root, bytes.len(), store, run, files).await?;
    tokio::fs::create_dir_all(root).await.unwrap();
    crate::artifacts::publication::publish(&root.join(name), bytes).await
}

fn run(store: &Path) -> PathBuf {
    store.join(ulid::Ulid::new().to_string())
}

#[tokio::test]
async fn repeated_writes_share_bytes_and_count_budgets() {
    let dir = tempfile::tempdir().unwrap();
    let root = run(dir.path());
    write_with_limits(&root, "one", b"123", 100, 5, 2)
        .await
        .unwrap();
    assert_eq!(
        write_with_limits(&root, "too-large", b"456", 100, 5, 2)
            .await
            .unwrap_err()
            .kind(),
        "budget_exceeded"
    );
    assert!(!root.join("too-large").exists());
    write_with_limits(&root, "two", b"45", 100, 5, 2)
        .await
        .unwrap();
    // Even empty files consume the count quota.
    assert_eq!(
        write_with_limits(&root, "third", b"", 100, 5, 2)
            .await
            .unwrap_err()
            .kind(),
        "budget_exceeded"
    );
    assert_eq!(usage(&root, false).await.unwrap().bytes, 5);
    assert_eq!(usage(&root, false).await.unwrap().files, 2);
}

#[tokio::test]
async fn simultaneous_active_runs_cannot_overbook_store() {
    let dir = tempfile::tempdir().unwrap();
    let one = run(dir.path());
    let two = run(dir.path());
    let _one = ActiveArtifactRun::register(one.file_name().unwrap().to_str().unwrap());
    let _two = ActiveArtifactRun::register(two.file_name().unwrap().to_str().unwrap());
    let (first, second) = tokio::join!(
        write_with_limits(&one, "output", b"123", 5, 10, 10),
        write_with_limits(&two, "output", b"456", 5, 10, 10),
    );
    assert_ne!(first.is_ok(), second.is_ok());
    assert_eq!(
        first.err().or_else(|| second.err()).unwrap().kind(),
        "budget_exceeded"
    );
    assert_eq!(usage(dir.path(), true).await.unwrap().bytes, 3);
}

#[tokio::test]
async fn admission_reclaims_inactive_outputs_but_preserves_current_run() {
    let dir = tempfile::tempdir().unwrap();
    let old = run(dir.path());
    write_with_limits(&old, "old", b"12345", 5, 10, 10)
        .await
        .unwrap();
    let current = run(dir.path());
    write_with_limits(&current, "new", b"123", 5, 10, 10)
        .await
        .unwrap();
    assert!(!old.exists());
    assert!(current.join("new").exists());
    assert_eq!(
        write_with_limits(&current, "overflow", b"456", 5, 10, 10)
            .await
            .unwrap_err()
            .kind(),
        "budget_exceeded"
    );
    assert_eq!(std::fs::read(current.join("new")).unwrap(), b"123");
}

#[tokio::test]
async fn disabling_store_retention_does_not_disable_run_quotas() {
    let dir = tempfile::tempdir().unwrap();
    let root = run(dir.path());
    write_with_limits(&root, "one", b"123", 0, 3, 2)
        .await
        .unwrap();
    assert_eq!(
        write_with_limits(&root, "two", b"4", 0, 3, 2)
            .await
            .unwrap_err()
            .kind(),
        "budget_exceeded"
    );
}

#[tokio::test]
async fn admission_fails_closed_when_usage_cannot_be_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = run(dir.path());
    std::fs::write(&root, b"replaced by file").unwrap();
    assert_eq!(
        write_with_limits(&root, "output", b"123", 100, 10, 10)
            .await
            .unwrap_err()
            .kind(),
        "internal_error"
    );
}

#[tokio::test]
async fn nested_metadata_names_cannot_hide_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let root = run(dir.path());
    let nested = root
        .join("nested")
        .join(crate::artifact_access::METADATA_DIR);
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("payload"), b"12345").unwrap();
    // Only the reserved top-level metadata directory is excluded.
    let metadata = root.join(crate::artifact_access::METADATA_DIR);
    std::fs::create_dir(&metadata).unwrap();
    std::fs::write(metadata.join("receipt"), b"not payload").unwrap();
    assert_eq!(usage(&root, false).await.unwrap().bytes, 5);
    assert_eq!(usage(dir.path(), true).await.unwrap().bytes, 5);
    assert_eq!(
        write_with_limits(&root, "overflow", b"6", 5, 10, 10)
            .await
            .unwrap_err()
            .kind(),
        "budget_exceeded"
    );
}

#[tokio::test]
async fn preexisting_over_budget_payloads_reject_even_empty_writes() {
    let dir = tempfile::tempdir().unwrap();
    let root = run(dir.path());
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("existing"), b"123456").unwrap();
    assert_eq!(
        write_with_limits(&root, "empty", b"", 100, 5, 10)
            .await
            .unwrap_err()
            .kind(),
        "budget_exceeded"
    );
    assert_eq!(
        write_with_limits(&root, "empty", b"", 5, 10, 10)
            .await
            .unwrap_err()
            .kind(),
        "budget_exceeded"
    );
    assert!(!root.join("empty").exists());
}

#[tokio::test]
async fn pressure_reclaims_inactive_runs_after_routine_prune_was_coalesced() {
    let dir = tempfile::tempdir().unwrap();
    let current = dir.path().join("00000000000000000000000001");
    let old = dir.path().join("00000000000000000000000002");
    let _active = ActiveArtifactRun::register(current.file_name().unwrap().to_str().unwrap());
    write_with_limits(&current, "first", b"123", 10, 20, 10)
        .await
        .unwrap();
    // A newer inactive execution finishes after routine retention.
    // Its sort order must not prevent reclaim while the older run stays active.
    std::fs::create_dir(&old).unwrap();
    std::fs::write(old.join("inactive"), b"12345678").unwrap();
    write_with_limits(&current, "second", b"4", 10, 20, 10)
        .await
        .unwrap();
    assert!(!old.exists());
    assert_eq!(usage(dir.path(), true).await.unwrap().bytes, 4);
    assert_eq!(std::fs::read(current.join("first")).unwrap(), b"123");
}

#[tokio::test]
async fn admission_reclaims_last_byte_for_store_sized_payload() {
    let dir = tempfile::tempdir().unwrap();
    let inactive = dir.path().join("00000000000000000000000001");
    let current = dir.path().join("00000000000000000000000002");
    std::fs::create_dir(&inactive).unwrap();
    std::fs::write(inactive.join("one-byte"), b"1").unwrap();
    write_with_limits(&current, "whole-budget", b"12345", 5, 10, 10)
        .await
        .unwrap();
    assert!(!inactive.exists());
    assert_eq!(usage(dir.path(), true).await.unwrap().bytes, 5);
}

#[tokio::test]
async fn coalesced_retention_uses_one_authoritative_store_scan_per_write() {
    let dir = tempfile::tempdir().unwrap();
    let root = run(dir.path());
    write_with_limits(&root, "first", b"1", 100, 100, 100)
        .await
        .unwrap();
    let before = store_scan_counts().lock().unwrap()[dir.path()];
    write_with_limits(&root, "second", b"2", 100, 100, 100)
        .await
        .unwrap();
    let after = store_scan_counts().lock().unwrap()[dir.path()];
    assert_eq!(
        after - before,
        1,
        "coalesced retention cannot require another complete scan"
    );
    // Each admission still consults disk instead of trusting a stale cache.
    std::fs::write(root.join("external"), vec![0; 99]).unwrap();
    assert_eq!(
        write_with_limits(&root, "third", b"3", 100, 200, 100)
            .await
            .unwrap_err()
            .kind(),
        "budget_exceeded"
    );
}
