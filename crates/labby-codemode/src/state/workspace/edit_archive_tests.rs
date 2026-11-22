use super::*;

#[tokio::test]
async fn workspace_glob_scans_past_nonmatching_files() {
    let temp = tempfile::tempdir().unwrap();
    let ws =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();
    for index in 0..5 {
        ws.write_file(
            &VirtualPath::parse(&format!("docs/{index}.txt")).unwrap(),
            "not rust\n",
        )
        .await
        .unwrap();
    }
    ws.write_file(&VirtualPath::parse("src/app.rs").unwrap(), "fn main() {}\n")
        .await
        .unwrap();

    let glob = ws.glob("src/**/*.rs", 1).await.unwrap();

    assert_eq!(glob.matches, vec!["src/app.rs"]);
    assert!(!glob.truncated);
}
#[tokio::test]
async fn broad_search_and_replace_skip_ineligible_files() {
    let temp = tempfile::tempdir().unwrap();
    let ws = StateWorkspace::new(
        temp.path().to_path_buf(),
        StateWorkspaceLimits {
            max_result_bytes: 128,
            ..Default::default()
        },
    )
    .unwrap();
    for (path, bytes) in [
        ("a.rs", &b"old"[..]),
        (".env.example", &b"old"[..]),
        ("binary", &[0xff]),
        ("large", &[b'x'; 129]),
    ] {
        tokio::fs::write(temp.path().join(path), bytes)
            .await
            .unwrap();
    }
    let result = ws.search_files("**/*", "old", 10).await.unwrap();
    assert_eq!(result.skipped, 3);
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].path, "a.rs");
    let result = ws
        .replace_in_files("**/*", "old", "new", false)
        .await
        .unwrap();
    assert_eq!(result.changed, ["a.rs"]);
    assert_eq!(result.skipped, 3);
    assert_eq!(
        tokio::fs::read(temp.path().join(".env.example"))
            .await
            .unwrap(),
        b"old"
    );
}

#[tokio::test]
async fn workspace_glob_search_replace_and_edit_plan() {
    let temp = tempfile::tempdir().unwrap();
    let ws =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();
    ws.write_file(
        &VirtualPath::parse("src/app.rs").unwrap(),
        "fn main() { println!(\"hi\"); }\n",
    )
    .await
    .unwrap();

    let glob = ws.glob("src/**/*.rs", 10).await.unwrap();
    assert_eq!(glob.matches, vec!["src/app.rs"]);

    let matches = ws.search_files("src/**/*.rs", "println", 10).await.unwrap();
    assert_eq!(matches.matches.len(), 1);
    assert_eq!(matches.matches[0].line, 1);

    let dry = ws
        .replace_in_files("src/**/*.rs", "println", "eprintln", true)
        .await
        .unwrap();
    assert_eq!(dry.changed, vec!["src/app.rs"]);
    assert!(dry.dry_run);

    let plan = ws
        .plan_edits(vec![
            FileEdit {
                path: "src/app.rs".to_string(),
                search: "println".to_string(),
                replace: "eprintln".to_string(),
            },
            FileEdit {
                path: "src/app.rs".to_string(),
                search: "eprintln".to_string(),
                replace: "log".to_string(),
            },
        ])
        .await
        .unwrap();
    let applied = ws.apply_edit_plan(&plan.plan_id).await.unwrap();
    assert_eq!(applied.changed, vec!["src/app.rs"]);
    let updated = ws
        .read_file(&VirtualPath::parse("src/app.rs").unwrap())
        .await
        .unwrap();
    assert_eq!(updated.content, "fn main() { log!(\"hi\"); }\n");
}

#[tokio::test]
async fn apply_edit_plan_restores_earlier_file_on_write_failure() {
    let temp = tempfile::tempdir().unwrap();
    let ws = StateWorkspace::new(
        temp.path().to_path_buf(),
        StateWorkspaceLimits {
            max_file_bytes: 4,
            ..Default::default()
        },
    )
    .unwrap();
    let mut edits = Vec::new();
    for (path, replacement) in [("first", "new"), ("second", "too long")] {
        ws.write_file(&VirtualPath::parse(path).unwrap(), "old")
            .await
            .unwrap();
        edits.push(FileEdit {
            path: path.into(),
            search: "old".into(),
            replace: replacement.into(),
        });
    }
    let plan = ws.plan_edits(edits).await.unwrap();
    let err = ws.apply_edit_plan(&plan.plan_id).await.unwrap_err();
    assert_eq!(err.kind(), "invalid_param");
    for path in ["first", "second"] {
        assert_eq!(
            ws.read_file(&VirtualPath::parse(path).unwrap())
                .await
                .unwrap()
                .content,
            "old"
        );
    }
}

#[tokio::test]
async fn replace_in_files_restores_earlier_file_on_write_failure() {
    let temp = tempfile::tempdir().unwrap();
    let ws = StateWorkspace::new(
        temp.path().to_path_buf(),
        StateWorkspaceLimits {
            max_file_bytes: 4,
            ..Default::default()
        },
    )
    .unwrap();
    for (path, text) in [("a", "a"), ("b", "ab")] {
        ws.write_file(&VirtualPath::parse(path).unwrap(), text)
            .await
            .unwrap();
    }
    let err = ws
        .replace_in_files("*", "a", "long", false)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "invalid_param");
    for (path, text) in [("a", "a"), ("b", "ab")] {
        assert_eq!(
            ws.read_file(&VirtualPath::parse(path).unwrap())
                .await
                .unwrap()
                .content,
            text
        );
    }
}

#[tokio::test]
async fn apply_edit_plan_errors_when_a_planned_search_no_longer_matches() {
    let temp = tempfile::tempdir().unwrap();
    let ws =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();
    ws.write_file(&VirtualPath::parse("src/app.rs").unwrap(), "old\n")
        .await
        .unwrap();
    let plan = ws
        .plan_edits(vec![FileEdit {
            path: "src/app.rs".to_string(),
            search: "old".to_string(),
            replace: "new".to_string(),
        }])
        .await
        .unwrap();
    ws.write_file(&VirtualPath::parse("src/app.rs").unwrap(), "changed\n")
        .await
        .unwrap();

    let err = ws.apply_edit_plan(&plan.plan_id).await.unwrap_err();

    assert_eq!(err.kind(), "edit_conflict");
    let file = ws
        .read_file(&VirtualPath::parse("src/app.rs").unwrap())
        .await
        .unwrap();
    assert_eq!(file.content, "changed\n");
}

#[tokio::test]
async fn archive_create_rejects_over_entry_quota_without_writing_tar() {
    let temp = tempfile::tempdir().unwrap();
    let limits = StateWorkspaceLimits {
        max_entries: 1,
        ..StateWorkspaceLimits::default()
    };
    let ws = StateWorkspace::new(temp.path().to_path_buf(), limits).unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join("src/a.txt"), "a").unwrap();
    std::fs::write(temp.path().join("src/b.txt"), "b").unwrap();

    let err = ws
        .archive_create(
            &VirtualPath::parse("src").unwrap(),
            &VirtualPath::parse("out/src.tar").unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "quota_exceeded");
    assert!(!temp.path().join("out/src.tar").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn archive_create_rejects_symlinked_source_entries() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join("src/a.txt"), "a").unwrap();
    std::os::unix::fs::symlink(outside.path(), temp.path().join("src/link")).unwrap();
    let ws =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();

    let err = ws
        .archive_create(
            &VirtualPath::parse("src").unwrap(),
            &VirtualPath::parse("out/src.tar").unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "permission_denied");
}

#[tokio::test]
async fn archive_list_exact_limit_is_not_truncated() {
    let temp = tempfile::tempdir().unwrap();
    let ws =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();
    ws.write_file(&VirtualPath::parse("src/a.txt").unwrap(), "a")
        .await
        .unwrap();
    ws.write_file(&VirtualPath::parse("src/b.txt").unwrap(), "b")
        .await
        .unwrap();
    ws.archive_create(
        &VirtualPath::parse("src").unwrap(),
        &VirtualPath::parse("out/src.tar").unwrap(),
    )
    .await
    .unwrap();

    let listed = ws
        .archive_list(&VirtualPath::parse("out/src.tar").unwrap(), 2)
        .await
        .unwrap();
    assert_eq!(listed.entries, vec!["a.txt", "b.txt"]);
    assert!(!listed.truncated);
}
