//! Automatic update and schedule regression tests.
use super::*;

#[cfg(unix)]
mod feature;
#[cfg(unix)]
mod recovery;

#[test]
fn schedule_lock_child() {
    let Some(path) = std::env::var_os("LABBY_TEST_SCHEDULE_LOCK") else {
        return;
    };
    assert!(acquire_schedule_lock(Path::new(&path)).is_err());
}

#[test]
fn schedule_lock_excludes_other_processes_and_survives_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let plist = dir.path().join("updater.plist");
    let lock = acquire_schedule_lock(&plist).unwrap();
    fs::write(&plist, "first").unwrap();
    fs::remove_file(&plist).unwrap();
    fs::write(&plist, "replacement").unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "self_update::tests::schedule_lock_child"])
        .env("LABBY_TEST_SCHEDULE_LOCK", &plist)
        .status()
        .unwrap();
    assert!(child.success());
    drop(lock);
    assert!(acquire_schedule_lock(&plist).is_ok());
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_installer_is_reaped_before_unlocking_and_cannot_write_later() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("install.sh");
    fs::write(
        &script,
        r#"
mkdir -p "$LABBY_INSTALL_DIR/.labby-install/activation-journal"
printf 'staged' > "$LABBY_INSTALL_DIR/.labby-install/activation-journal/state"
(sleep 1; touch "$LABBY_INSTALL_DIR/late-write") &
echo $$ > "$LABBY_INSTALL_DIR/installer.pid"
wait
"#,
    )
    .unwrap();
    let lock = acquire_update_lock(dir.path()).unwrap();
    let path = dir.path().to_owned();
    let task = tokio::spawn(async move { install_release(&script, "v1.17.0", &path, lock).await });
    let pid_file = dir.path().join("installer.pid");
    let pid = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(pid) = fs::read_to_string(&pid_file)
                .ok()
                .and_then(|text| text.trim().parse::<i32>().ok())
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(acquire_update_lock(dir.path()).is_err());
    task.abort();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    );
    // Reaping the shell does not wait for its descendants to finish
    // exiting after SIGKILL. Their inherited descriptors must keep the
    // lock held until kernel cleanup completes.
    let _next_owner = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match acquire_update_lock(dir.path()) {
                Ok(lock) => break lock,
                Err(error)
                    if matches!(
                        error.downcast_ref::<fs::TryLockError>(),
                        Some(fs::TryLockError::WouldBlock)
                    ) =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => panic!("cannot reacquire update lock: {error:#}"),
            }
        }
    })
    .await
    .expect("cancelled installer descendants must release the update lock");
    assert_eq!(
        fs::read_to_string(dir.path().join(".labby-install/activation-journal/state")).unwrap(),
        "staged"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(!dir.path().join("late-write").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn installer_owner_child() {
    let Some(path) = std::env::var_os("LABBY_TEST_UPDATE_SIGNAL") else {
        return;
    };
    let path = Path::new(&path);
    let lock = acquire_update_lock(path).unwrap();
    // Cold re-exec of the test binary plus runtime start can take several
    // seconds under load; publish readiness so the parent measures only the
    // installer start against its tight deadline.
    fs::write(path.join("updater.ready"), "ready").unwrap();
    if std::env::var_os("LABBY_TEST_LEGACY_UPDATE_OWNER").is_some() {
        // Reproduce the reviewed implementation: the parent alone owns
        // the lock while an ordinary subprocess performs installation.
        let _lock = lock;
        assert!(
            installer_command(&path.join("install.sh"), "v1.17.0", path)
                .output()
                .unwrap()
                .status
                .success()
        );
    } else {
        install_release(&path.join("install.sh"), "v1.17.0", path, lock)
            .await
            .unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn installer_keeps_exclusion_after_updater_is_killed() {
    use nix::sys::signal::{Signal, kill, killpg};
    use nix::unistd::Pid;
    use std::os::unix::process::CommandExt;
    struct GroupCleanup(Pid);
    impl Drop for GroupCleanup {
        fn drop(&mut self) {
            let _ = killpg(self.0, Signal::SIGKILL);
        }
    }
    for legacy in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("install.sh"),
            r#"
echo $$ > "$LABBY_INSTALL_DIR/installer.pid"
sleep 120
"#,
        )
        .unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "self_update::tests::installer_owner_child"])
            .env("LABBY_TEST_UPDATE_SIGNAL", dir.path())
            .process_group(0);
        if legacy {
            command.env("LABBY_TEST_LEGACY_UPDATE_OWNER", "1");
        }
        let mut updater = command.spawn().unwrap();
        let updater_pid = Pid::from_raw(i32::try_from(updater.id()).unwrap());
        let _updater_cleanup = GroupCleanup(updater_pid);
        // The updater child owns the lock once it publishes `updater.ready`;
        // only the installer's own start is held to the short deadline.
        tokio::time::timeout(Duration::from_mins(1), async {
            while !dir.path().join("updater.ready").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("updater child must start and take the update lock within 60 s");
        let pid = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(pid) = fs::read_to_string(dir.path().join("installer.pid"))
                    .ok()
                    .and_then(|text| text.trim().parse::<i32>().ok())
                {
                    break Pid::from_raw(pid);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let installer_group = if legacy { updater_pid } else { pid };
        let _installer_cleanup = GroupCleanup(installer_group);
        kill(updater_pid, Signal::SIGKILL).unwrap();
        assert!(!updater.wait().unwrap().success());
        // Baseline releases exclusion while the orphan is alive; the fixed
        // child-local descriptor keeps a replacement updater excluded.
        assert_eq!(acquire_update_lock(dir.path()).is_err(), !legacy);
        killpg(installer_group, Signal::SIGKILL).unwrap();
        // Reaping the killed installer process group can lag on loaded macOS CI
        // runners even though the lock descriptor is released as the processes exit.
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if acquire_update_lock(dir.path()).is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[test]
fn renamed_executable_cannot_report_an_update_to_another_binary() {
    assert_eq!(
        install_directory(Path::new("/opt/bin/labby")).unwrap(),
        Path::new("/opt/bin")
    );
    assert!(install_directory(Path::new("/opt/bin/labby-preview")).is_err());
}

#[test]
fn failed_schedule_activation_restores_previous_job() {
    let dir = tempfile::tempdir().unwrap();
    let plist = dir.path().join("update.plist");
    fs::write(&plist, "previous schedule").unwrap();
    let mut calls = Vec::new();
    let result = replace_schedule(&plist, b"new schedule", true, |operation| {
        calls.push(operation.to_owned());
        if calls.len() == 2 {
            assert_eq!(fs::read_to_string(&plist).unwrap(), "new schedule");
            bail!("bootstrap rejected");
        }
        if calls.len() == 3 {
            assert_eq!(fs::read_to_string(&plist).unwrap(), "previous schedule");
        }
        Ok(())
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("previous schedule restored")
    );
    assert_eq!(calls, ["bootout", "bootstrap", "bootstrap"]);
    assert_eq!(fs::read_to_string(plist).unwrap(), "previous schedule");
}

#[test]
fn failed_first_schedule_activation_removes_new_plist() {
    let dir = tempfile::tempdir().unwrap();
    let plist = dir.path().join("update.plist");
    let result = replace_schedule(&plist, b"new schedule", false, |operation| {
        assert_eq!(operation, "bootstrap");
        bail!("bootstrap rejected")
    });
    assert!(result.is_err());
    assert!(!plist.exists());
}

#[test]
fn missing_loaded_schedule_is_preserved_without_unloading() {
    let dir = tempfile::tempdir().unwrap();
    let result = replace_schedule(&dir.path().join("missing.plist"), b"new", true, |_| {
        panic!("must not unload a job without rollback data")
    });
    assert!(result.is_err());
}

#[test]
fn schedule_rollback_failure_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let plist = dir.path().join("update.plist");
    fs::write(&plist, "previous schedule").unwrap();
    let result = replace_schedule(&plist, b"new schedule", true, |operation| {
        if operation == "bootstrap" {
            bail!("bootstrap rejected");
        }
        Ok(())
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("restoration also failed")
    );
    assert_eq!(fs::read_to_string(plist).unwrap(), "previous schedule");
}

fn release(tag: &str) -> Release {
    Release {
        tag_name: tag.into(),
        draft: false,
        prerelease: false,
        assets: vec![
            Asset { name: ASSET.into() },
            Asset {
                name: format!("{ASSET}.sha256"),
            },
        ],
    }
}

#[cfg(unix)]
#[tokio::test]
async fn server_restarts_only_after_successful_installation() {
    let mut results = std::collections::VecDeque::from([
        Err(anyhow::anyhow!("network failure")),
        Ok(json!({"installed": false})),
        Ok(json!({"installed": true, "version": "v1.17.0"})),
    ]);
    wait_for_installed_update(
        || std::future::ready(results.pop_front().expect("unexpected extra check")),
        Duration::ZERO,
        Duration::ZERO,
    )
    .await;
    assert!(results.is_empty());
}

#[test]
fn selects_newest_stable_binary_without_downgrading() {
    let mut draft = release("v9.0.0");
    draft.draft = true;
    let mut prerelease = release("v8.0.0");
    prerelease.prerelease = true;
    let mut missing = release("v7.0.0");
    missing.assets.pop();
    let releases = vec![
        draft,
        prerelease,
        missing,
        release("v2.0.0-rc.1"),
        release("v1.9.0"),
        release("v1.10.0"),
    ];
    assert_eq!(select_release(&releases, [1, 8, 0]), Some("v1.10.0"));
    assert_eq!(select_release(&releases, [1, 10, 0]), None);
    assert_eq!(select_release(&releases, [1, 16, 1]), None);
}

#[test]
fn unknown_versions_fail_closed() {
    for invalid in ["labby dev", "1.2", "1.2.3-rc.1", "1.2.+3", "1.2.3.4"] {
        assert!(version(invalid).is_err());
    }
    assert_eq!(version("labby 1.16.1\n").unwrap(), [1, 16, 1]);
}

#[test]
fn launchd_executes_native_binary_and_escapes_paths() {
    let plist = launch_agent(
        Path::new("/custom & bin/labby"),
        Path::new("/logs/<update>"),
        "/bin",
    )
    .unwrap();
    assert!(plist.contains("<string>/custom &amp; bin/labby</string>"));
    assert!(
        plist.contains("<string>host</string><string>update</string><string>--automatic</string>")
    );
    use clap::Parser as _;
    assert!(crate::cli::Cli::try_parse_from(["labby", "host", "update", "--automatic"]).is_ok());
    assert!(plist.contains("<integer>86400</integer>"));
    assert!(!plist.contains("python"));
}

#[cfg(unix)]
#[tokio::test]
async fn installer_failure_propagates_without_reporting_success() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("install.sh");
    fs::write(&script, "echo 'attestation rejected' >&2; exit 7\n").unwrap();
    let lock = acquire_update_lock(temp.path()).unwrap();
    let error = install_release(&script, "v1.17.0", temp.path(), lock)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("attestation rejected"));
}

#[test]
fn every_host_update_entry_point_pins_and_sanitizes_installer_control() {
    let command = installer_command(Path::new("/installer"), "v1.17.0", Path::new("/bin"));
    let env: std::collections::HashMap<_, _> = command.get_envs().collect();
    assert_eq!(
        env[std::ffi::OsStr::new("LABBY_INSTALL_VERSION")],
        Some(std::ffi::OsStr::new("v1.17.0"))
    );
    assert_eq!(
        env[std::ffi::OsStr::new("LABBY_ALLOW_SOURCE_FALLBACK")],
        Some(std::ffi::OsStr::new("0"))
    );
    for key in INSTALL_CONTROL_VARIABLES {
        assert_eq!(
            env[std::ffi::OsStr::new(key)],
            None,
            "ambient {key} must not alter an operator-requested update"
        );
    }
}

#[test]
fn installer_environment_drops_gh_host_overrides() {
    // GH_HOST and GH_CONFIG_DIR redirect gh at another host or credential
    // store, and GH_ENTERPRISE_TOKEN authenticates there; any of them in the
    // service environment would let the installer verify provenance against
    // a trust root other than github.com.
    let command = installer_command(Path::new("/installer"), "v1.17.0", Path::new("/bin"));
    let env: std::collections::HashMap<_, _> = command.get_envs().collect();
    for key in ["GH_HOST", "GH_ENTERPRISE_TOKEN", "GH_CONFIG_DIR"] {
        assert_eq!(
            env.get(std::ffi::OsStr::new(key)),
            Some(&None),
            "ambient {key} must not select the installer's trust root"
        );
    }
}

#[cfg(unix)]
#[test]
fn updates_explicitly_skip_first_run_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("install.sh");
    fs::write(
        &script,
        "test \"$LABBY_INSTALL_NO_SETUP\" = 1 || { echo 'update attempted onboarding' >&2; exit 1; }\n",
    )
    .unwrap();
    let output = installer_command(&script, "v1.17.0", temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
