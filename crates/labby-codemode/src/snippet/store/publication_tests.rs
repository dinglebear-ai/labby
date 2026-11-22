use super::*;
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
const BODY: &str = "async () => ({ ok: true })";
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        drop(self.0.kill());
        drop(self.0.wait());
    }
}
#[test]
fn external_lock_holder() {
    let Ok(directory) = std::env::var("LABBY_TEST_LOCK_DIRECTORY") else {
        return;
    };
    let dir = Path::new(&directory);
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(".snippet-write.lock"))
        .unwrap();
    file.lock().unwrap();
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "holder-ready").unwrap();
    stdout.flush().unwrap();
    drop(stdout);
    let mut release = [0; 1];
    let _ = std::io::stdin().read(&mut release).unwrap();
    drop(file);
}
#[test]
fn contended_write_has_bounded_busy_error_no_changes_and_recovers() {
    let home = tempfile::tempdir().unwrap();
    let original = create_user_snippet(home.path(), "demo", BODY, None, false).unwrap();
    let dir = user_snippet_dir(home.path());
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "--nocapture",
            "snippet::store::publication_tests::external_lock_holder",
        ])
        .env("LABBY_TEST_LOCK_DIRECTORY", &dir)
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = ChildGuard(child);
    let stdout = child.0.stdout.as_mut().unwrap();
    assert!(
        BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .any(|line| line.ends_with("holder-ready")),
        "child exited before acquiring the lock"
    );
    let before = read_snippet_body(&original.path).unwrap();
    let start = Instant::now();
    let error = create_user_snippet_checked(
        home.path(),
        "demo",
        "async () => null",
        None,
        true,
        original.content_digest.as_deref(),
    )
    .unwrap_err();
    // Broad watchdog bound verifies termination; performance comparisons have no timing assertions.
    assert!(start.elapsed() < Duration::from_secs(6));
    assert_eq!(error.kind(), "conflict");
    assert_eq!(error.extra_fields()["existing_id"], "snippet-write-lock");
    let envelope = serde_json::to_value(&error).unwrap();
    assert_eq!(envelope["side_effects"], "none_expected");
    assert_eq!(envelope["recovery"]["action"], "retry_later");
    assert_eq!(read_snippet_body(&original.path).unwrap(), before);
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 2); // snippet and persistent lock
    drop(child.0.stdin.take());
    assert!(child.0.wait().unwrap().success());
    create_user_snippet_checked(
        home.path(),
        "demo",
        "async () => null",
        None,
        true,
        original.content_digest.as_deref(),
    )
    .unwrap();
    let stale = create_user_snippet_checked(
        home.path(),
        "demo",
        BODY,
        None,
        true,
        original.content_digest.as_deref(),
    )
    .unwrap_err();
    assert!(matches!(stale,ToolError::Conflict { ref existing_id,.. } if existing_id=="demo"));
}
