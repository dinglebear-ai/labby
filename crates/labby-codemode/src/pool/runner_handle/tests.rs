#[cfg(windows)]
use std::process::Stdio;

#[cfg(windows)]
use crate::error::ToolError;

#[cfg(any(target_os = "linux", windows))]
use super::PooledRunner;
use super::StderrBuffer;
#[cfg(windows)]
use super::arm_job_object_with;

#[cfg(target_os = "linux")]
#[tokio::test]
async fn direct_runner_clears_parent_environment_and_owns_child_temporary_files() {
    use futures::StreamExt;
    let inherited = ["CARGO_MANIFEST_DIR", "HOME", "USER"]
        .into_iter()
        .find(|name| std::env::var_os(name).is_some())
        .expect("parent environment");
    let script = format!(
        r#"test -z "${{{inherited}+x}}" && mkdir "$TMPDIR/fixture" && printf '%s\n' "$TMPDIR" && read line"#
    );
    let spawn = crate::pool::RunnerSpawn {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script],
    };
    let mut child = PooledRunner::spawn(&spawn, None, None).await.unwrap();
    let path = tokio::time::timeout(std::time::Duration::from_secs(5), child.lines.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let root = std::path::PathBuf::from(path);
    assert!(root.join("fixture").is_dir());
    child.shutdown().await.unwrap();
    assert!(!root.exists());
}

#[tokio::test]
async fn stderr_buffer_take_since_and_clear_releases_retained_lines() {
    let buffer = StderrBuffer::new();
    buffer.state.lock().await.lines.push("before".to_string());
    let mark = buffer.mark().await;
    {
        let mut state = buffer.state.lock().await;
        state.lines.push("during-one".to_string());
        state.lines.push("during-two".to_string());
    }

    let captured = buffer.take_since_and_clear(mark).await;

    assert_eq!(captured, ["during-one", "during-two"]);
    assert_eq!(buffer.state.lock().await.lines, Vec::<String>::new());
}

#[tokio::test]
async fn stderr_buffer_clear_discards_retained_lines() {
    let buffer = StderrBuffer::new();
    buffer
        .state
        .lock()
        .await
        .lines
        .push("discard me".to_string());

    buffer.clear().await;

    assert_eq!(buffer.state.lock().await.lines, Vec::<String>::new());
}

#[tokio::test]
async fn stderr_buffer_clear_resets_per_execution_caps() {
    let buffer = StderrBuffer::new();
    {
        let mut state = buffer.state.lock().await;
        state.lines.push("[labby] runner stderr truncated".into());
        state.total_bytes = 8 * 1024 * 1024;
        state.capped = true;
    }

    buffer.clear().await;

    let state = buffer.state.lock().await;
    assert_eq!(state.lines, Vec::<String>::new());
    assert_eq!(state.total_bytes, 0);
    assert!(!state.capped);
}

#[cfg(windows)]
#[tokio::test]
async fn shutdown_reaps_the_runner_and_its_descendant() {
    use futures::StreamExt as _;

    // Re-enter a native Rust fixture: PowerShell needs ambient Windows/.NET
    // configuration that the runner intentionally does not inherit.
    let executable = std::env::current_exe().expect("test executable");
    let mut runner = PooledRunner::spawn_stub_command(
        executable.to_str().expect("test executable path"),
        &[
            "--exact",
            "pool::runner_handle::windows_fixture::runner_descendant",
            "--ignored",
            "--nocapture",
        ],
    )
    .expect("spawn Windows runner stand-in");
    let runner_pid = runner.child_pid.expect("runner pid");
    let descendant_pid: u32 = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(line) = runner.lines.next().await {
            let line = line.expect("descendant pid line");
            if let Some(pid) = line.strip_prefix("LABBY_DESCENDANT_PID=") {
                return pid.parse().expect("numeric descendant pid");
            }
        }
        panic!(
            "runner stdout closed: {:?}",
            runner.stderr.take_since_and_clear(0).await
        );
    })
    .await
    .expect("descendant pid timeout");

    runner.shutdown().await.expect("clean runner shutdown");

    for pid in [runner_pid, descendant_pid] {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match labby_winjob::pid_liveness(pid).expect("inspect process liveness") {
                labby_winjob::ProcessLiveness::Alive if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                labby_winjob::ProcessLiveness::Alive => {
                    panic!("pid {pid} survived shutdown");
                }
                labby_winjob::ProcessLiveness::Exited | labby_winjob::ProcessLiveness::NotFound => {
                    break;
                }
            }
        }
    }
}

#[cfg(windows)]
#[tokio::test]
async fn failed_job_assignment_terminates_and_reaps_the_direct_child() {
    let mut command = tokio::process::Command::new(r"C:\Windows\System32\findstr.exe");
    command
        .arg("^")
        .env_clear()
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().expect("spawn Windows child");
    let pid = child.id().expect("child pid");

    let error = arm_job_object_with(&mut child, Some(pid), |_| {
        Err(labby_winjob::WinJobError {
            operation: "injected Job Object assignment",
            code: 5,
        })
    })
    .await
    .expect_err("injected assignment failure must fail closed");

    assert!(matches!(
        error,
        ToolError::Sdk {
            sdk_kind,
            message: _
        } if sdk_kind == "internal_error"
    ));
    assert!(matches!(
        labby_winjob::pid_liveness(pid).expect("inspect child liveness"),
        labby_winjob::ProcessLiveness::Exited | labby_winjob::ProcessLiveness::NotFound
    ));
}

/// Real KVM smoke. Example:
///
/// ```text
/// LABBY_MICROSANDBOX_SMOKE_MSB=/absolute/path/to/msb \
/// LABBY_MICROSANDBOX_SMOKE_RUNNER=/absolute/path/to/labby \
/// LABBY_MICROSANDBOX_SMOKE_IMAGE=debian \
/// cargo test -p labby-codemode microsandbox_runner_round_trip -- --ignored
/// ```
#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires Linux KVM, Microsandbox, a cached image, and a Labby binary"]
async fn microsandbox_runner_round_trip() {
    use futures::StreamExt as _;
    use tokio::io::AsyncWriteExt as _;

    let msb =
        std::env::var_os("LABBY_MICROSANDBOX_SMOKE_MSB").expect("LABBY_MICROSANDBOX_SMOKE_MSB");
    let runner = std::env::var_os("LABBY_MICROSANDBOX_SMOKE_RUNNER")
        .expect("LABBY_MICROSANDBOX_SMOKE_RUNNER");
    let image =
        std::env::var("LABBY_MICROSANDBOX_SMOKE_IMAGE").expect("LABBY_MICROSANDBOX_SMOKE_IMAGE");
    let spawn = crate::pool::RunnerSpawn {
        program: runner.into(),
        args: vec!["internal".into(), "code-mode-runner".into()],
    };
    let microsandbox = crate::pool::MicrosandboxSpawn {
        executable: msb.into(),
        image,
    };

    let mut runner = PooledRunner::spawn(&spawn, Some(&microsandbox), None)
        .await
        .expect("spawn microVM");
    runner
        .stdin
        .as_mut()
        .expect("runner stdin")
        .write_all(b"{\"type\":\"start\",\"code\":\"async () => 42\",\"proxy\":\"\"}\n")
        .await
        .expect("write start");
    runner
        .stdin
        .as_mut()
        .expect("runner stdin")
        .flush()
        .await
        .expect("flush start");
    let line = tokio::time::timeout(std::time::Duration::from_secs(15), runner.lines.next())
        .await
        .expect("runner response timeout")
        .expect("runner stdout closed")
        .expect("runner protocol line");
    let value: serde_json::Value = serde_json::from_str(&line).expect("valid protocol JSON");
    assert_eq!(value["type"], "done");
    assert_eq!(value["result"]["value"], 42);
}
