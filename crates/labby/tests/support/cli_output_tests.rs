use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn cli_output_timeout_releases_the_runtime_and_child() {
    assert_runtime_cleanup("timeout");
}

#[test]
fn cli_output_outer_cancellation_releases_the_runtime_and_child() {
    assert_runtime_cleanup("cancel");
}

#[cfg(windows)]
#[test]
fn cli_output_timeout_releases_descendant_held_pipes() {
    assert_runtime_cleanup("descendant");
}

fn assert_runtime_cleanup(mode: &str) {
    let root = tempfile::tempdir().expect("owned CLI timeout fixture");
    let pid_path = root.path().join("child.pid");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "cli_output_tests::cli_output_runtime_fixture",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env("LABBY_CLI_TIMEOUT_PID", &pid_path)
        .env("LABBY_CLI_TIMEOUT_MODE", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn().unwrap();
    #[cfg(windows)]
    let job = labby_winjob::JobObject::assign(child.id()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            #[cfg(unix)]
            nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(child.id() as i32),
                nix::sys::signal::Signal::SIGKILL,
            )
            .unwrap();
            #[cfg(windows)]
            job.close().unwrap();
            child.kill().ok();
            let cleanup_deadline = Instant::now() + Duration::from_secs(1);
            while child.try_wait().unwrap().is_none() {
                assert!(
                    Instant::now() < cleanup_deadline,
                    "fixture kill did not settle"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.is_some_and(|status| status.success()),
        "CLI timeout did not release the runtime within its bounded fixture"
    );
    let pid: u32 = std::fs::read_to_string(pid_path)
        .expect("the real CLI child started")
        .parse()
        .unwrap();
    #[cfg(windows)]
    {
        assert!(
            matches!(
                labby_winjob::pid_liveness(pid).expect("inspect timed-out child"),
                labby_winjob::ProcessLiveness::Exited | labby_winjob::ProcessLiveness::NotFound
            ),
            "timed-out child survived"
        );
        if mode == "descendant" {
            let descendant: u32 = std::fs::read_to_string(root.path().join("descendant.pid"))
                .expect("the pipe-holding descendant started after job admission")
                .parse()
                .unwrap();
            assert!(
                matches!(
                    labby_winjob::pid_liveness(descendant).expect("inspect owned descendant"),
                    labby_winjob::ProcessLiveness::Exited | labby_winjob::ProcessLiveness::NotFound
                ),
                "descendant survived"
            );
        }
    }
    #[cfg(unix)]
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None),
        Err(nix::errno::Errno::ESRCH),
        "timed-out child survived"
    );
}

#[test]
#[ignore = "subprocess-only runtime shutdown fixture"]
fn cli_output_runtime_fixture() {
    let pid_path = std::env::var_os("LABBY_CLI_TIMEOUT_PID").expect("owned fixture marker");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mode = std::env::var("LABBY_CLI_TIMEOUT_MODE").unwrap();
    runtime.block_on(async {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "cli_output_tests::cli_output_sleep_fixture",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .env("LABBY_CLI_TIMEOUT_PID", &pid_path);
        if mode == "cancel" {
            assert!(
                tokio::time::timeout(
                    Duration::from_secs(2),
                    crate::live_labby::bounded_cli_output(&mut command, Duration::from_secs(30)),
                )
                .await
                .is_err()
            );
        } else {
            #[cfg(windows)]
            let result = if mode == "descendant" {
                let admitted = std::path::PathBuf::from(&pid_path).with_file_name("admitted");
                crate::live_labby::bounded_cli_output_after_admission(
                    &mut command,
                    Duration::from_secs(2),
                    || std::fs::write(admitted, b"job assigned").unwrap(),
                )
                .await
            } else {
                crate::live_labby::bounded_cli_output(&mut command, Duration::from_secs(2)).await
            };
            #[cfg(not(windows))]
            let result =
                crate::live_labby::bounded_cli_output(&mut command, Duration::from_secs(2)).await;
            let error = result.unwrap_err();
            assert!(error.contains("CLI child exceeded"), "{error}");
        }
    });
    // Windows' blocking stdout/stderr readers must have been released too.
    drop(runtime);
}

#[test]
#[ignore = "subprocess-only child holding its output pipes open"]
fn cli_output_sleep_fixture() {
    let marker = std::env::var_os("LABBY_CLI_TIMEOUT_PID").expect("owned fixture marker");
    std::fs::write(&marker, std::process::id().to_string()).unwrap();
    #[cfg(windows)]
    if std::env::var("LABBY_CLI_TIMEOUT_MODE").as_deref() == Ok("descendant") {
        let marker = std::path::PathBuf::from(marker);
        while !marker.with_file_name("admitted").exists() {
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut descendant = Command::new(std::env::current_exe().unwrap());
        descendant
            .args([
                "cli_output_tests::cli_output_sleep_fixture",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .env("LABBY_CLI_TIMEOUT_MODE", "leaf")
            .env(
                "LABBY_CLI_TIMEOUT_PID",
                marker.with_file_name("descendant.pid"),
            );
        let mut descendant = descendant.spawn().unwrap();
        descendant.wait().unwrap();
    }
    std::thread::sleep(Duration::from_secs(30));
}
