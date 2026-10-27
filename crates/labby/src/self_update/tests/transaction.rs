use super::*;
use std::os::unix::fs::PermissionsExt;

fn executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn catalog_wait_rechecks_direct_installer_version_and_channel() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("labby");
    executable(&binary, "#!/bin/sh\necho 'labby 1.17.0-dev.1'\n");
    persist_channel(dir.path(), UpdateChannel::Dev).unwrap();
    let result = automatic_with_catalog(&binary, true, None, async {
        // A standalone install completes during catalog discovery.
        let _transaction = acquire_installer_transaction(dir.path()).unwrap();
        executable(&binary, "#!/bin/sh\necho 'labby 2.0.0'\n");
        persist_channel(dir.path(), UpdateChannel::Stable).unwrap();
        let mut dev = release("v1.17.0-dev.2");
        dev.prerelease = true;
        Ok(vec![dev, release("v2.0.0")])
    })
    .await
    .unwrap();
    assert_eq!(result["installed"], false);
    assert_eq!(result["channel"], "stable");
    assert!(result.get("version").is_none());
}

#[tokio::test]
async fn native_mutations_respect_real_shell_transaction_lock() {
    let dir = tempfile::tempdir().unwrap();
    let tools = dir.path().join("tools");
    fs::create_dir(&tools).unwrap();
    // The real installer calls sync after publishing its lock PID. Pause that
    // owned process there with a bounded private fixture, without network calls.
    executable(
        &tools.join("sync"),
        "#!/bin/sh\n: >\"$LOCK_READY\"\ni=0\nwhile [ ! -f \"$LOCK_RELEASE\" ] && [ \"$i\" -lt 100 ]; do i=$((i+1)); sleep 0.05; done\n[ -f \"$LOCK_RELEASE\" ]\n",
    );
    let script = dir.path().join("install.sh");
    fs::write(&script, INSTALL_SCRIPT).unwrap();
    let ready = dir.path().join("ready");
    let release_gate = dir.path().join("release");
    persist_channel(dir.path(), UpdateChannel::Stable).unwrap();
    let binary = dir.path().join("labby");
    executable(&binary, "#!/bin/sh\necho 'labby 1.17.0'\n");
    let mut child = installer_command(&script, "latest", dir.path(), UpdateChannel::Stable)
        .env("LABBY_INSTALL_RECOVER_ONLY", "1")
        .env("PATH", format!("{}:/usr/bin:/bin", tools.display()))
        .env("LOCK_READY", &ready)
        .env("LOCK_RELEASE", &release_gate)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !ready.exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Always release/reap the task-owned shell before making assertions.
    let was_ready = ready.exists();
    let schedule = replace_schedule_channel(
        &binary,
        &dir.path().join("schedule.plist"),
        b"job",
        Some(UpdateChannel::Dev),
        false,
        |_| Ok(()),
    );
    let automatic = automatic_with_catalog(&binary, false, Some(UpdateChannel::Dev), async {
        Ok(vec![])
    })
    .await;
    let unchanged = selected_channel(dir.path()).unwrap() == UpdateChannel::Stable;
    fs::write(&release_gate, "").unwrap();
    let status = child.wait().unwrap();
    assert!(was_ready);
    assert!(status.success());
    assert!(
        schedule
            .unwrap_err()
            .to_string()
            .contains("Installer transaction is locked")
    );
    assert!(
        automatic
            .unwrap_err()
            .to_string()
            .contains("Installer transaction is locked")
    );
    assert!(unchanged);
    assert!(!dir.path().join("schedule.plist").exists());
}

#[test]
fn schedule_rollback_cannot_overwrite_concurrent_direct_installation() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("labby");
    let script = dir.path().join("install.sh");
    fs::write(&script, INSTALL_SCRIPT).unwrap();
    let result = replace_schedule_channel(
        &binary,
        &dir.path().join("schedule.plist"),
        b"job",
        Some(UpdateChannel::Dev),
        false,
        |_| {
            assert!(acquire_installer_transaction(dir.path()).is_err());
            let attempt = installer_command(&script, "latest", dir.path(), UpdateChannel::Stable)
                .env("LABBY_INSTALL_RECOVER_ONLY", "1")
                .output()
                .unwrap();
            assert!(!attempt.status.success());
            assert!(
                String::from_utf8_lossy(&attempt.stderr)
                    .contains("another Labby installation is running")
            );
            bail!("launchctl bootstrap failed")
        },
    );
    assert!(result.is_err());
    assert!(!dir.path().join(".labby-update-channel").exists());
    // Only after rollback releases the common exclusion can the direct install
    // commit its own selection; no delayed rollback may overwrite it.
    let _transaction = acquire_installer_transaction(dir.path()).unwrap();
    persist_channel(dir.path(), UpdateChannel::Dev).unwrap();
    assert_eq!(selected_channel(dir.path()).unwrap(), UpdateChannel::Dev);
}

#[test]
fn failed_pid_write_removes_its_partially_created_lock() {
    let dir = tempfile::tempdir().unwrap();
    let result = acquire_installer_transaction_with(dir.path(), |pid| {
        fs::write(pid, b"partial")?;
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "simulated PID flush failure",
        ))
    });
    let error = result.err().unwrap();
    assert!(format!("{error:#}").contains("simulated PID flush failure"));
    assert!(!dir.path().join(".labby-install/transaction-lock").exists());
    assert!(acquire_installer_transaction(dir.path()).is_ok());
}
