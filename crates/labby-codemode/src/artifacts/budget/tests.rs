use super::*;
use crate::artifacts::ActiveArtifactRun;
use crate::artifacts::publication::lock_store;
use std::path::PathBuf;

async fn write_with_limits(
    root: &Path,
    name: &str,
    bytes: &[u8],
    store: u64,
    run: u64,
    files: u64,
) -> Result<(), ToolError> {
    let _reservation = lock_store(root.parent().unwrap()).await.unwrap();
    admit_with_limits(root, bytes.len(), store, run, files, "fixture-owner").await?;
    mark_run(root, "fixture-owner");
    crate::artifacts::publication::publish(&root.join(name), bytes).await
}

fn mark_run(root: &Path, owner: &str) {
    let dir = root.join(crate::artifact_access::METADATA_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(owner), []).unwrap();
}

fn run(store: &Path) -> PathBuf {
    store.join(ulid::Ulid::new().to_string())
}

fn ordered_runs(store: &Path) -> (PathBuf, PathBuf) {
    // Run protection is process-global. Each fixture needs distinct ids even
    // though its filesystem store is isolated from concurrent fixtures.
    let older = ulid::Ulid::new();
    let newer = ulid::Ulid::from(u128::from(older) + 1);
    (store.join(older.to_string()), store.join(newer.to_string()))
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
    std::fs::create_dir_all(&metadata).unwrap();
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
    let (current, old) = ordered_runs(dir.path());
    let _active = ActiveArtifactRun::register(current.file_name().unwrap().to_str().unwrap());
    write_with_limits(&current, "first", b"123", 10, 20, 10)
        .await
        .unwrap();
    // A newer inactive execution finishes after routine retention.
    // Its sort order must not prevent reclaim while the older run stays active.
    mark_run(&old, "fixture-owner");
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
    let (inactive, current) = ordered_runs(dir.path());
    mark_run(&inactive, "fixture-owner");
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
    assert!(
        after - before <= 1,
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

#[tokio::test]
async fn admission_retains_payload_that_fits_when_metadata_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let (inactive, current) = ordered_runs(dir.path());
    let metadata = inactive.join(crate::artifact_access::METADATA_DIR);
    std::fs::create_dir_all(&metadata).unwrap();
    std::fs::write(inactive.join("payload"), b"123456789").unwrap();
    std::fs::write(metadata.join("receipt"), b"internal metadata").unwrap();
    write_with_limits(&current, "new", b"0", 10, 10, 10)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(inactive.join("payload")).unwrap(),
        b"123456789"
    );
    assert!(metadata.join("receipt").exists());
    assert_eq!(usage(dir.path(), true).await.unwrap().bytes, 10);
}

#[tokio::test]
async fn retention_counts_nested_metadata_names_as_payload() {
    let dir = tempfile::tempdir().unwrap();
    let inactive = run(dir.path());
    let nested = inactive
        .join("nested")
        .join(crate::artifact_access::METADATA_DIR);
    mark_run(&inactive, "fixture-owner");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("payload"), b"123456").unwrap();
    let _reservation = lock_store(dir.path()).await.unwrap();
    crate::artifacts::prune_artifact_runs_locked(
        dir.path(),
        0,
        Some((5, &usage(dir.path(), true).await.unwrap().runs)),
        &Default::default(),
        "fixture-owner",
        None,
    )
    .await;
    assert!(!inactive.exists());
}

#[tokio::test]
async fn impossible_pressure_has_constant_store_scans_and_preserves_outputs() {
    for inactive_count in [1, 32] {
        let dir = tempfile::tempdir().unwrap();
        let current = run(dir.path());
        std::fs::create_dir(&current).unwrap();
        std::fs::write(current.join("active"), b"a").unwrap();
        let _active = ActiveArtifactRun::register(current.file_name().unwrap().to_str().unwrap());
        std::fs::write(dir.path().join("operator-file"), vec![0; 100]).unwrap();
        let mut inactive = Vec::new();
        for _ in 0..inactive_count {
            let path = run(dir.path());
            mark_run(&path, "fixture-owner");
            std::fs::write(path.join("payload"), b"x").unwrap();
            inactive.push(path);
        }
        let _reservation = lock_store(dir.path()).await.unwrap();
        // Disable count pruning directly, without process-global env mutation.
        let admission = admit_with_retention(&current, 1, 100, 100, 100, 0, "fixture-owner").await;
        if admission.is_ok() {
            crate::artifacts::publication::publish(&current.join("new"), b"n")
                .await
                .unwrap();
        }
        let error = admission.unwrap_err();
        assert_eq!(error.kind(), "budget_exceeded");
        assert!(store_scan_counts().lock().unwrap()[dir.path()] <= 3);
        assert!(!current.join("new").exists());
        assert_eq!(std::fs::read(current.join("active")).unwrap(), b"a");
        assert_eq!(
            std::fs::metadata(dir.path().join("operator-file"))
                .unwrap()
                .len(),
            100
        );
        assert!(inactive.iter().all(|path| path.join("payload").exists()));
    }
}

#[tokio::test]
async fn exact_reclamation_uses_inventory_and_keeps_an_authoritative_final_check() {
    let dir = tempfile::tempdir().unwrap();
    let mut inactive = Vec::new();
    for _ in 0..8 {
        let path = run(dir.path());
        mark_run(&path, "fixture-owner");
        std::fs::write(path.join("payload"), b"x").unwrap();
        inactive.push(path);
    }
    let _reservation = lock_store(dir.path()).await.unwrap();
    reclaim_inactive(dir.path(), 0, &Default::default(), "fixture-owner")
        .await
        .unwrap();
    assert_eq!(store_scan_counts().lock().unwrap()[dir.path()], 1);
    assert!(inactive.iter().all(|path| !path.exists()));
    assert_eq!(usage(dir.path(), true).await.unwrap().bytes, 0);
}

#[tokio::test]
async fn retention_never_reclaims_another_owner_or_unmarked_legacy_run() {
    let dir = tempfile::tempdir().unwrap();
    let (old_b, current) = ordered_runs(dir.path());
    let a = run(dir.path());
    let legacy = run(dir.path());
    mark_run(&a, "owner-a");
    mark_run(&old_b, "owner-b");
    std::fs::create_dir(&legacy).unwrap();
    for path in [&a, &legacy, &old_b] {
        std::fs::write(path.join("payload"), b"123").unwrap();
    }
    let _guard = lock_store(dir.path()).await.unwrap();
    admit_with_retention(&current, 1, 10, 10, 10, 1, "owner-b")
        .await
        .unwrap();
    assert!(!old_b.exists());
    let denied = admit_with_retention(&current, 5, 10, 10, 10, 1, "owner-b")
        .await
        .unwrap_err();
    assert_eq!(denied.kind(), "budget_exceeded");
    for preserved in [&a, &legacy] {
        assert_eq!(std::fs::read(preserved.join("payload")).unwrap(), b"123");
    }
}

#[cfg(unix)]
#[test]
fn warm_inventory_reuses_membership_and_observes_external_payload_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = run(dir.path());
    mark_run(&root, "fixture-owner");
    let nested = root.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let payload = nested.join("payload");
    std::fs::write(&payload, b"old").unwrap();
    let measure = |inventory: Option<Ledger>| {
        measure_usage(dir.path(), true, inventory.unwrap_or_default()).unwrap()
    };
    let (cold, inventory) = measure(Default::default());
    #[cfg(target_os = "linux")]
    let changed_directory_reads =
        usize::from(inventory.as_ref().and_then(|l| l.watch.as_ref()).is_none());
    #[cfg(not(target_os = "linux"))]
    let changed_directory_reads = 1;
    assert_eq!((cold.bytes, cold.directory_reads), (3, 3));
    std::fs::write(&payload, b"resized in place").unwrap();
    let (warm, inventory) = measure(inventory);
    assert_eq!((warm.bytes, warm.files, warm.directory_reads), (16, 1, 0));
    let added = nested.join("added");
    std::fs::write(&added, b"new").unwrap();
    let (added_usage, inventory) = measure(inventory);
    assert_eq!(
        (
            added_usage.bytes,
            added_usage.files,
            added_usage.directory_reads
        ),
        (19, 2, changed_directory_reads)
    );
    std::fs::remove_file(payload).unwrap();
    let (remaining, _) = measure(inventory);
    assert_eq!(
        (remaining.bytes, remaining.files, remaining.directory_reads),
        (3, 1, changed_directory_reads)
    );
}

#[tokio::test]
async fn concurrent_publications_reconcile_the_existing_store_totals() {
    let store = tempfile::tempdir().unwrap();
    let current = run(store.path());
    mark_run(&current, "fixture-owner");
    for _ in 0..16 {
        let previous = run(store.path());
        mark_run(&previous, "other-owner");
        for item in 0..8 {
            std::fs::write(previous.join(item.to_string()), b"datum").unwrap();
        }
    }
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..64 {
        let root = current.clone();
        tasks.spawn(async move {
            write_with_limits(&root, &format!("sample-{index}"), b"x", 704, 64, 64).await
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap().unwrap();
    }
    let usage = usage(store.path(), true).await.unwrap();
    assert_eq!((usage.files, usage.bytes), (192, 704));
    assert_eq!(usage.runs.len(), 17);
    assert_eq!(
        write_with_limits(&current, "extra", b"x", 704, 128, 128)
            .await
            .unwrap_err()
            .kind(),
        "budget_exceeded"
    );
}

pub(super) fn store_scan_counts() -> &'static std::sync::Mutex<HashMap<PathBuf, usize>> {
    static COUNTS: std::sync::OnceLock<std::sync::Mutex<HashMap<PathBuf, usize>>> =
        std::sync::OnceLock::new();
    COUNTS.get_or_init(Default::default)
}
