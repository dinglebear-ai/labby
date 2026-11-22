use super::super::{admit_with_retention, tests::store_scan_counts, usage, watch};
use crate::artifacts::publication::lock_store;
use std::time::{Duration, Instant};

#[tokio::test]
async fn large_store_concurrent_admission_reuses_watches_and_detects_external_resize() {
    let store = tempfile::tempdir().unwrap();
    if watch::StoreWatch::new(store.path()).is_none() {
        // Filesystems without synchronous native changes use the portable accounting path.
        return;
    }
    let owner = "fixture-owner";
    let root = store.path().join(ulid::Ulid::new().to_string());
    std::fs::create_dir_all(root.join(crate::artifact_access::METADATA_DIR)).unwrap();
    std::fs::write(
        root.join(crate::artifact_access::METADATA_DIR).join(owner),
        [],
    )
    .unwrap();
    let mut external = None;
    for _ in 0..199 {
        let run = store.path().join(ulid::Ulid::new().to_string());
        std::fs::create_dir(&run).unwrap();
        for index in 0..256 {
            let file = run.join(index.to_string());
            std::fs::write(&file, []).unwrap();
            external.get_or_insert(file);
        }
    }
    let cold = usage(store.path(), true).await.unwrap();
    assert_eq!(cold.files, 199 * 256);
    assert!(cold.metadata_checks >= 199 * 256);
    let initial = store_scan_counts().lock().unwrap()[store.path()];
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..64 {
        let root = root.clone();
        tasks.spawn(async move {
            let start = Instant::now();
            let _guard = lock_store(root.parent().unwrap()).await.unwrap();
            admit_with_retention(&root, 1, 128, 128, 256, 200, owner)
                .await
                .unwrap();
            crate::artifacts::publication::publish(&root.join(i.to_string()), b"x")
                .await
                .unwrap();
            start.elapsed()
        });
    }
    let mut latencies = Vec::new();
    while let Some(result) = tasks.join_next().await {
        latencies.push(result.unwrap());
    }
    latencies.sort_unstable();
    assert!(
        latencies[60] < Duration::from_secs(2),
        "64-writer p95: {:?}",
        latencies[60]
    );
    let warm = usage(store.path(), true).await.unwrap();
    assert_eq!((warm.bytes, warm.files), (64, 199 * 256 + 64));
    assert_eq!(store_scan_counts().lock().unwrap()[store.path()], initial);
    // An existing file can grow through a hard link outside the watched directories.
    let outside = tempfile::tempdir().unwrap();
    let link = outside.path().join("external");
    std::fs::hard_link(external.unwrap(), &link).unwrap();
    std::fs::write(link, [0; 65]).unwrap();
    let resized = usage(store.path(), true).await.unwrap();
    assert_eq!(resized.bytes, 129);
    assert_eq!(resized.directory_reads, 0);
    assert!(resized.metadata_checks <= 3);
    eprintln!(
        "200 runs, 64 writers: p95 {:?}; metadata checks for one resize: {}",
        latencies[60], resized.metadata_checks
    );
    let _guard = lock_store(store.path()).await.unwrap();
    let denied = admit_with_retention(&root, 1, 128, 128, 256, 200, owner)
        .await
        .unwrap_err();
    assert_eq!(denied.kind(), "budget_exceeded");
}
