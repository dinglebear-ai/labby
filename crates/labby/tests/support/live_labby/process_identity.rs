//! Fallible native start identities; missing observations never grant ownership.

use super::*;

pub(super) fn capture(pid: u32, deadline: Instant) -> Result<String, String> {
    capture_typed(pid, deadline).map_err(settle_inventory_failure)
}

pub(super) fn capture_typed(
    pid: u32,
    deadline: Instant,
) -> Result<String, process_inventory::Failure> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err("invalid process identity PID".to_string().into());
    }
    let text = process_inventory::start_identity(pid, deadline)?;
    let identity = format!("pid:{pid}:{}", text.trim());
    validate(pid, &identity)?;
    Ok(identity)
}

pub(super) fn validate(pid: u32, identity: &str) -> Result<(), String> {
    let invalid = || "missing or malformed process start identity".to_string();
    if pid == 0
        || pid > i32::MAX as u32
        || identity.len() > 128
        || identity.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    let prefix = format!("pid:{pid}:");
    let text = identity.strip_prefix(&prefix).ok_or_else(invalid)?;
    let fields: Vec<_> = text.split_whitespace().collect();
    if fields.len() != 5
        || !["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].contains(&fields[0])
        || ![
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ]
        .contains(&fields[1])
        || !fields[2]
            .parse::<u8>()
            .is_ok_and(|day| (1..=31).contains(&day))
        || !fields[4]
            .parse::<u16>()
            .is_ok_and(|year| (1970..=9999).contains(&year))
    {
        return Err(invalid());
    }
    let time: Vec<_> = fields[3].split(':').map(str::parse::<u8>).collect();
    if time.len() != 3
        || !time[0].as_ref().is_ok_and(|hour| *hour < 24)
        || !time[1].as_ref().is_ok_and(|minute| *minute < 60)
        || !time[2].as_ref().is_ok_and(|second| *second < 60)
    {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn matches(
    pid: Option<u32>,
    expected: Option<&str>,
    observed: Result<String, String>,
) -> bool {
    let (Some(pid), Some(expected)) = (pid, expected) else {
        return false;
    };
    validate(pid, expected).is_ok() && observed.is_ok_and(|actual| actual == expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "externally reaped Tokio child ownership failure fixture"]
    async fn reaped_tokio_owner_fixture() {
        struct DropMarker(PathBuf);
        impl Drop for DropMarker {
            fn drop(&mut self) {
                drop(std::fs::write(&self.0, b"destructor executed"));
            }
        }
        let drop_marker = DropMarker(PathBuf::from(
            std::env::var_os("LABBY_REAPED_OWNER_MARKER").unwrap(),
        ));
        let mut command = TokioCommand::new("/bin/sh");
        command.args(["-c", "exit 0"]).kill_on_drop(true);
        configure_process_group(&mut command);
        let child = command.spawn().unwrap();
        let pid = child.id().unwrap();
        // Simulate another process-wide waiter reaping our child without
        // notifying Tokio's retained kill-on-drop guard.
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match rustix::process::waitpid(
                rustix::process::Pid::from_raw(pid as i32),
                rustix::process::WaitOptions::NOHANG,
            ) {
                Ok(Some(_)) => break,
                Ok(None) => {
                    assert!(Instant::now() < deadline, "fixture child did not exit");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => abort_unsettled_cleanup_helper(pid, &[error.to_string()]),
            }
        }
        if let Err(error) = std::fs::write(
            drop_marker.0.with_extension("admitted"),
            b"externally reaped",
        ) {
            abort_unsettled_cleanup_helper(pid, &[error.to_string()]);
        }
        require_waitable_cleanup_owner(&child, pid, &mut Vec::new());
        drop(child);
    }

    #[test]
    fn rejected_tokio_ownership_fail_stops_before_destructors_or_signals() {
        use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("destructor");
        let fixture = format!(
            "{}::reaped_tokio_owner_fixture",
            module_path!().split_once("::").unwrap().1
        );
        let child = Command::new(std::env::current_exe().unwrap())
            .args([&fixture, "--exact", "--ignored", "--nocapture"])
            .env("LABBY_REAPED_OWNER_MARKER", &marker)
            .process_group(0)
            .spawn()
            .unwrap();
        let mut observed = None;
        let result = run_spawned_owned_child(
            child,
            Instant::now() + Duration::from_secs(3),
            assign_cleanup_job,
            |child| {
                observed = child.try_wait()?;
                Ok(observed)
            },
        );
        assert_eq!(
            observed.and_then(|status| status.signal()),
            Some(6),
            "{result:?}"
        );
        assert!(
            marker.with_extension("admitted").is_file(),
            "fixture never reached rejected ownership check"
        );
        assert!(
            !marker.exists(),
            "rejected ownership unwound through a destructor"
        );
    }

    #[tokio::test]
    async fn readiness_requests_share_the_remaining_absolute_budget() {
        let mut guard = LiveLabbyBuilder::new().start().await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let silent = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        guard.descriptor.health_url = format!("http://{address}/health");
        guard.descriptor.ready_url = format!("http://{address}/ready");
        let started = Instant::now();
        let error = guard
            .wait_ready(started + Duration::from_millis(100))
            .await
            .unwrap_err();
        let elapsed = started.elapsed();
        silent.abort();
        drop(silent.await);
        let cleanup = guard.finish().await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
        assert!(error.contains("readiness deadline exceeded"), "{error}");
        assert!(
            elapsed < Duration::from_millis(500),
            "readiness overran shared budget: {elapsed:?}"
        );
    }

    #[tokio::test]
    #[ignore = "unsettled native probe failure subprocess fixture"]
    #[allow(
        clippy::panic,
        reason = "isolated fixture rejects an unexpected return from SIGABRT"
    )]
    async fn unsettled_probe_owner_fixture() {
        let root = PathBuf::from(std::env::var_os("LABBY_UNSETTLED_PROBE_ROOT").unwrap());
        let mut guard = LiveLabbyBuilder::new()
            .existing_root(root)
            .start()
            .await
            .unwrap();
        let record = serde_json::json!({
            "group": guard.ledger.process_group.unwrap(),
            "pid": guard.ledger.daemon_pid.unwrap(),
            "address": guard.ledger.listener.unwrap(),
        });
        std::fs::write(
            std::env::var_os("LABBY_UNSETTLED_PROBE_RECORD").unwrap(),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let probe = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        drop(
            guard.settle_observation_failure(process_inventory::Failure {
                message: "injected unsettled native observation".into(),
                unsettled: Some(probe),
            }),
        );
        panic!("unsettled native probe must fail-stop");
    }

    #[test]
    fn unsettled_observation_settles_real_daemon_before_owner_abort() {
        use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
        let temp = tempfile::tempdir().unwrap();
        let parent = std::env::temp_dir().join("labby-live-e2e");
        std::fs::create_dir_all(&parent).unwrap();
        let run_root = tempfile::tempdir_in(parent).unwrap();
        let record_path = temp.path().join("owned-daemon.json");
        let fixture = format!(
            "{}::unsettled_probe_owner_fixture",
            module_path!().split_once("::").unwrap().1
        );
        let child = Command::new(std::env::current_exe().unwrap())
            .args([&fixture, "--exact", "--ignored", "--nocapture"])
            .env("LABBY_UNSETTLED_PROBE_RECORD", &record_path)
            .env("LABBY_UNSETTLED_PROBE_ROOT", run_root.path())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut observed = None;
        let result = run_spawned_owned_child(
            child,
            Instant::now() + Duration::from_secs(10),
            assign_cleanup_job,
            |child| {
                observed = child.try_wait()?;
                Ok(observed)
            },
        );
        let record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(record_path).unwrap()).unwrap();
        let group = i32::try_from(record["group"].as_i64().unwrap()).unwrap();
        let pid = u32::try_from(record["pid"].as_u64().unwrap()).unwrap();
        let address: SocketAddr = record["address"].as_str().unwrap().parse().unwrap();
        let members = process_group_members_checked(group).unwrap();
        let actual = capture(pid, Instant::now() + Duration::from_secs(1));
        let listener_absent = TcpListener::bind(address).is_ok();
        // These are observations only. The already-reaped owner cannot grant
        // fresh authority to signal a numeric daemon PID or group.
        assert_eq!(
            observed.and_then(|status| status.signal()),
            Some(6),
            "{result:?}"
        );
        assert!(
            members.is_empty(),
            "owned daemon group survived probe abort: {members:?}"
        );
        assert!(actual.is_err(), "actual daemon survived probe abort");
        assert!(
            listener_absent,
            "actual daemon listener survived probe abort"
        );
    }

    #[test]
    #[ignore = "isolated PATH-spoof identity probe fixture"]
    fn identity_path_spoof_fixture() {
        let identity =
            capture(std::process::id(), Instant::now() + Duration::from_secs(1)).unwrap();
        std::fs::write(
            std::env::var_os("LABBY_IDENTITY_PROBE_RESULT").unwrap(),
            identity,
        )
        .unwrap();
    }

    #[test]
    fn native_start_probe_ignores_a_path_supplied_ps() {
        use std::os::unix::{fs::PermissionsExt as _, process::CommandExt as _};
        let temp = tempfile::tempdir().unwrap();
        let fake = temp.path().join("ps");
        let marker = temp.path().join("spoof-executed");
        let result = temp.path().join("identity");
        std::fs::write(&fake, b"#!/bin/sh\n/usr/bin/touch \"$LABBY_FAKE_PS_MARKER\"\nprintf 'Fri Sep 4 19:00:00 2026\\n'\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        let fixture = format!(
            "{}::identity_path_spoof_fixture",
            module_path!().split_once("::").unwrap().1
        );
        let child = Command::new(std::env::current_exe().unwrap())
            .args([&fixture, "--exact", "--ignored"])
            .env("PATH", temp.path())
            .env("LABBY_FAKE_PS_MARKER", &marker)
            .env("LABBY_IDENTITY_PROBE_RESULT", &result)
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        run_spawned_owned_child(
            child,
            Instant::now() + Duration::from_secs(3),
            assign_cleanup_job,
            |child| child.try_wait(),
        )
        .unwrap();
        validate(pid, &std::fs::read_to_string(result).unwrap()).unwrap();
        assert!(
            !marker.exists(),
            "PATH-supplied ps gained process ownership authority"
        );
    }

    #[test]
    fn failed_empty_and_malformed_observations_never_authorize_a_group() {
        for identity in ["pid:12:unknown", "pid:12:", "", "pid:12:not-a-date"] {
            assert!(!matches(Some(12), Some(identity), Ok(identity.into())));
        }
        assert!(!matches(None, None, Err("unavailable".into())));
        let good = "pid:12:Fri Sep 4 19:00:00 2026";
        assert!(matches(Some(12), Some(good), Ok(good.into())));
        assert!(!matches(Some(12), Some(good), Err("unavailable".into())));
        assert!(!matches(Some(13), Some(good), Ok(good.into())));
    }

    #[tokio::test]
    async fn failed_startup_and_restart_capture_settle_actual_daemon_and_listener() {
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        };
        for restart in [false, true] {
            for mode in ["failure", "empty", "publication", "deadline"] {
                let parent = std::env::temp_dir().join("labby-live-e2e");
                std::fs::create_dir_all(&parent).unwrap();
                let root = tempfile::tempdir_in(parent).unwrap();
                let manifest = root.path().join("ownership.json");
                let expected_error = if mode == "deadline" {
                    "readiness deadline exceeded"
                } else if mode == "publication" {
                    "ownership publication failed"
                } else {
                    "process identity capture failed"
                };
                let captures = Arc::new(Mutex::new(Vec::new()));
                let capture_records = Arc::clone(&captures);
                let stages = Arc::new(Mutex::new(Vec::new()));
                let capture_stages = Arc::clone(&stages);
                let calls = AtomicUsize::new(0);
                let probe: IdentityProbe = Arc::new(move |pid, address, admission, deadline| {
                    if restart && calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        capture_stages
                            .lock()
                            .unwrap()
                            .push("initial_restart_identity");
                        return capture(pid, deadline);
                    }
                    capture_stages.lock().unwrap().push("capture_entered");
                    let ready_deadline = deadline.min(Instant::now() + Duration::from_secs(3));
                    loop {
                        if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(20))
                            .is_ok()
                        {
                            break;
                        }
                        if Instant::now() >= ready_deadline {
                            return Err("identity fixture daemon never bound its listener".into());
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    capture_stages.lock().unwrap().push("listener_ready");
                    let daemon_pid = if let Some(admission) = admission {
                        guardian::read_daemon_pid(admission, deadline)?
                    } else {
                        pid
                    };
                    capture_stages.lock().unwrap().push("pid_available");
                    let members = process_group_members_checked_before(pid as i32, deadline)?;
                    assert!(members.contains(&daemon_pid));
                    capture_stages
                        .lock()
                        .unwrap()
                        .push("owned_membership_verified");
                    capture_records
                        .lock()
                        .unwrap()
                        .push((pid, daemon_pid, address, members));
                    if mode == "deadline" {
                        let identity = capture(pid, deadline)?;
                        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                        Ok(identity)
                    } else if mode == "publication" {
                        std::fs::remove_file(&manifest).map_err(|error| error.to_string())?;
                        std::fs::create_dir(&manifest).map_err(|error| error.to_string())?;
                        capture(pid, deadline)
                    } else if mode == "empty" {
                        Ok(String::new())
                    } else {
                        Err("injected native identity observation failure".into())
                    }
                });
                let mut builder = LiveLabbyBuilder::new().existing_root(root.path());
                if mode == "deadline" {
                    builder.readiness_deadline = Duration::from_secs(3);
                }
                builder.identity_probe = Some(probe);
                let observed_error = if restart {
                    let mut guard = builder.start().await.unwrap();
                    let old_group = guard.ledger.process_group.unwrap();
                    let error = guard.restart().await.unwrap_err();
                    assert!(error.contains(expected_error), "{error}");
                    assert!(process_group_members_checked(old_group).unwrap().is_empty());
                    let cleanup = guard.finish().await;
                    assert_eq!(
                        cleanup.is_clean(),
                        mode != "publication",
                        "{:?}",
                        cleanup.failures
                    );
                    error
                } else {
                    let error = builder
                        .start()
                        .await
                        .err()
                        .expect("failed identity capture unexpectedly started");
                    assert!(error.contains(expected_error), "{error}");
                    error
                };
                let records = captures.lock().unwrap();
                assert_eq!(
                    records.len(),
                    1,
                    "restart={restart} mode={mode} stages={:?}: {}",
                    stages.lock().unwrap().iter().take(8).collect::<Vec<_>>(),
                    observed_error.chars().take(512).collect::<String>()
                );
                let mut expected_stages = Vec::new();
                if restart {
                    expected_stages.push("initial_restart_identity");
                }
                expected_stages.extend([
                    "capture_entered",
                    "listener_ready",
                    "pid_available",
                    "owned_membership_verified",
                ]);
                assert_eq!(*stages.lock().unwrap(), expected_stages);
                let (group, daemon_pid, address, members) = &records[0];
                assert!(members.contains(daemon_pid));
                assert!(
                    process_group_members_checked(*group as i32)
                        .unwrap()
                        .is_empty()
                );
                assert!(capture(*daemon_pid, Instant::now() + Duration::from_secs(1)).is_err());
                assert!(
                    TcpListener::bind(address).is_ok(),
                    "failed capture retained actual daemon listener"
                );
            }
        }
    }
}
