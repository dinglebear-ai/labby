use super::quota::{WORKSPACE_USAGE_SCAN_COUNT, WORKSPACE_USAGE_SCAN_ROOT};
use super::*;
use crate::state::quota::StateWorkspaceLimits;

fn reset_workspace_usage_scan_count() {
    WORKSPACE_USAGE_SCAN_COUNT.store(0, Ordering::Relaxed);
}

fn track_workspace_usage_scans(root: Option<PathBuf>) {
    *WORKSPACE_USAGE_SCAN_ROOT.lock().unwrap() = root;
}

fn workspace_usage_scan_count() -> usize {
    WORKSPACE_USAGE_SCAN_COUNT.load(Ordering::Relaxed)
}

#[tokio::test]
async fn workspace_writes_reads_and_reopens() {
    let temp = tempfile::tempdir().unwrap();
    let ws =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();
    ws.write_file(
        &VirtualPath::parse("/src/app.rs").unwrap(),
        "fn main() {}\n",
    )
    .await
    .unwrap();
    assert_eq!(
        ws.read_file(&VirtualPath::parse("src/app.rs").unwrap())
            .await
            .unwrap()
            .content,
        "fn main() {}\n"
    );
    let ws2 =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();
    assert_eq!(
        ws2.list(&VirtualPath::parse("src").unwrap())
            .await
            .unwrap()
            .entries
            .len(),
        1
    );
}

#[tokio::test]
async fn writes_check_usage_once_and_batch_reconciles_once() {
    let temp = tempfile::tempdir().unwrap();
    let ws =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();

    reset_workspace_usage_scan_count();
    track_workspace_usage_scans(Some(temp.path().to_path_buf()));
    ws.write_file(
        &VirtualPath::parse("/src/app.rs").unwrap(),
        "fn main() {}\n",
    )
    .await
    .unwrap();
    track_workspace_usage_scans(None);

    assert_eq!(workspace_usage_scan_count(), 1);
    for name in ["first.rs", "second.rs", "third.rs"] {
        ws.write_file(&VirtualPath::parse(name).unwrap(), "old")
            .await
            .unwrap();
    }
    reset_workspace_usage_scan_count();
    track_workspace_usage_scans(Some(temp.path().to_path_buf()));
    let batch = ws
        .replace_in_files("*.rs", "old", "new", false)
        .await
        .unwrap();
    track_workspace_usage_scans(None);
    assert_eq!(batch.changed.len(), 3);
    assert_eq!(workspace_usage_scan_count(), 2);
}

#[tokio::test]
async fn workspace_rejects_large_writes_and_reads() {
    let temp = tempfile::tempdir().unwrap();
    let limits = StateWorkspaceLimits {
        max_file_bytes: 4,
        max_result_bytes: 4,
        ..StateWorkspaceLimits::default()
    };
    let ws = StateWorkspace::new(temp.path().to_path_buf(), limits).unwrap();
    let err = ws
        .write_file(&VirtualPath::parse("too-big.txt").unwrap(), "12345")
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "invalid_param");

    std::fs::write(temp.path().join("existing.txt"), "12345").unwrap();
    let err = ws
        .read_file(&VirtualPath::parse("existing.txt").unwrap())
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "response_too_large");
}

#[tokio::test]
async fn workspace_enforces_total_byte_limit() {
    let temp = tempfile::tempdir().unwrap();
    let limits = StateWorkspaceLimits {
        max_file_bytes: 10,
        max_total_bytes: 6,
        ..StateWorkspaceLimits::default()
    };
    let ws = StateWorkspace::new(temp.path().to_path_buf(), limits).unwrap();
    ws.write_file(&VirtualPath::parse("a.txt").unwrap(), "1234")
        .await
        .unwrap();
    let err = ws
        .write_file(&VirtualPath::parse("b.txt").unwrap(), "1234")
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "quota_exceeded");
}

#[tokio::test]
async fn workspace_enforces_visible_entry_limit_on_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let limits = StateWorkspaceLimits {
        max_entries: 1,
        ..StateWorkspaceLimits::default()
    };
    let ws = StateWorkspace::new(temp.path().to_path_buf(), limits).unwrap();
    ws.write_file(&VirtualPath::parse("a.txt").unwrap(), "a")
        .await
        .unwrap();

    let err = ws
        .write_file(&VirtualPath::parse("b.txt").unwrap(), "b")
        .await
        .unwrap_err();

    assert_eq!(err.kind(), "quota_exceeded");
    assert!(!temp.path().join("b.txt").exists());
}

#[tokio::test]
async fn plan_edits_removes_hidden_plan_file_after_quota_error() {
    let temp = tempfile::tempdir().unwrap();
    let limits = StateWorkspaceLimits {
        max_file_bytes: 1024,
        max_total_bytes: 1,
        ..StateWorkspaceLimits::default()
    };
    let ws = StateWorkspace::new(temp.path().to_path_buf(), limits).unwrap();

    let err = ws
        .plan_edits(vec![FileEdit {
            path: "src/app.rs".to_string(),
            search: "println".to_string(),
            replace: "eprintln".to_string(),
        }])
        .await
        .unwrap_err();

    assert_eq!(err.kind(), "quota_exceeded");
    let plans_dir = temp.path().join(".labby-state").join("plans");
    let remaining = std::fs::read_dir(&plans_dir)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn workspace_recursive_reads_hide_reserved_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let ws =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();
    std::fs::create_dir_all(temp.path().join("repo/.git/hooks")).unwrap();
    std::fs::write(temp.path().join("repo/.git/config"), "[core]\n").unwrap();
    std::fs::create_dir_all(temp.path().join("repo/.labby-state/plans")).unwrap();
    std::fs::write(temp.path().join("repo/.labby-state/plans/plan.json"), "{}").unwrap();
    ws.write_file(
        &VirtualPath::parse("repo/src/app.rs").unwrap(),
        "fn main() {}\n",
    )
    .await
    .unwrap();

    let listed = ws.list(&VirtualPath::parse("repo").unwrap()).await.unwrap();
    assert_eq!(listed.entries, vec!["src"]);

    let walked = ws
        .walk_tree(&VirtualPath::parse("repo").unwrap(), 100)
        .await
        .unwrap();
    assert!(
        walked
            .entries
            .iter()
            .all(|entry| !entry.path.contains("/.git/") && !entry.path.contains("/.labby-state/"))
    );

    let glob = ws.glob("repo/**/*", 100).await.unwrap();
    assert_eq!(glob.matches, vec!["repo/src/app.rs"]);

    ws.archive_create(
        &VirtualPath::parse("repo").unwrap(),
        &VirtualPath::parse("out/repo.tar").unwrap(),
    )
    .await
    .unwrap();
    let archive = ws
        .archive_list(&VirtualPath::parse("out/repo.tar").unwrap(), 100)
        .await
        .unwrap();
    assert_eq!(archive.entries, vec!["src/app.rs"]);
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_rejects_symlink_ancestors() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let ws =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();
    std::os::unix::fs::symlink(outside.path(), temp.path().join("link")).unwrap();
    let err = ws
        .write_file(&VirtualPath::parse("link/file.txt").unwrap(), "x")
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "symlink_rejected");
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_walkers_skip_symlinked_directories() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(outside.path().join("src")).unwrap();
    std::fs::write(outside.path().join("src/outside.rs"), "fn outside() {}\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), temp.path().join("linked")).unwrap();
    let ws =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();

    let glob = ws.glob("**/*.rs", 10).await.unwrap();

    assert!(glob.matches.is_empty(), "{:?}", glob.matches);
}
