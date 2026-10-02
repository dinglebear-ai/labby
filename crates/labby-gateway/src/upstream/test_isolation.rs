//! Hermetic libtest subprocesses for fixtures that register process-wide authority.
use std::time::Duration;

const CHILD_TEST: &str = "LABBY_GATEWAY_ISOLATED_TEST";

pub(super) async fn run(test: &str, environment: &[(&str, &str)]) -> bool {
    if std::env::var(CHILD_TEST).as_deref() == Ok(test) {
        return false;
    }
    let home = if cfg!(target_os = "macos") {
        tempfile::Builder::new()
            .prefix("lgw-")
            .tempdir_in("/private/tmp")
    } else {
        tempfile::tempdir()
    }
    .expect("short isolated subprocess home");
    let mut command =
        tokio::process::Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env_clear()
        .envs(std::env::vars_os().filter(|(name, _)| {
            cfg!(windows)
                && name.to_str().is_some_and(|name| {
                    name.eq_ignore_ascii_case("SystemRoot") || name.eq_ignore_ascii_case("WINDIR")
                })
        }))
        .env(CHILD_TEST, test)
        .env("HOME", home.path())
        .env("LABBY_HOME", home.path())
        .env("TMPDIR", home.path())
        .envs(environment.iter().copied())
        .kill_on_drop(true);
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .expect("bounded isolated fixture")
        .expect("fixture subprocess");
    assert!(
        output.status.success(),
        "isolated fixture {test} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "exact filter must run the fixture: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    true
}
