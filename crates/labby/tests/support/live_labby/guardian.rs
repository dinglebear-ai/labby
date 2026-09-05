//! The outer supervisor owns daemon groups even when their test owner aborts.

use super::*;

pub(super) fn supervise(command: TokioCommand) -> Result<(TokioCommand, Option<PathBuf>), String> {
    let Some(registry) = std::env::var_os("LABBY_E2E_HELPER_REGISTRY") else {
        return Ok((command, None));
    };
    let token = std::env::var("LABBY_E2E_GROUP_TOKEN")
        .map_err(|_| "supervised daemon has no owned shard token".to_string())?;
    let mut wrapper = supervised_cleanup_command(command.as_std(), Path::new(&registry), &token)?;
    let admission = supervised_admission_path(&wrapper, Path::new(&registry))?;
    wrapper.env("LABBY_E2E_GATE_MODE", "runtime");
    Ok((TokioCommand::from(wrapper), Some(admission)))
}

pub(super) fn record_daemon_identity(
    guard: &mut LiveLabbyGuard,
    deadline: Instant,
) -> Result<(), String> {
    use std::io::Read as _;
    let Some(registry) = &guard.guardian_admission else {
        return Ok(());
    };
    let guardian_pid = guard
        .ledger
        .guardian_pid
        .ok_or("missing daemon guardian identity")?;
    let file = std::fs::File::open(registry.join("child.pid"))
        .map_err(|error| format!("cannot read guarded daemon PID: {error}"))?;
    let mut value = String::new();
    file.take(12)
        .read_to_string(&mut value)
        .map_err(|error| error.to_string())?;
    if value.len() > 11 {
        return Err("guarded daemon PID exceeds its bound".into());
    }
    let pid = value
        .trim_end_matches('\n')
        .parse::<u32>()
        .map_err(|_| "invalid guarded daemon PID".to_string())?;
    if pid == 0
        || pid == guardian_pid
        || !process_group_members_typed(guardian_pid as i32, deadline)
            .map_err(|failure| guard.settle_observation_failure(failure))?
            .contains(&pid)
    {
        return Err("daemon PID is not a live member of its owned guardian group".into());
    }
    let identity = process_identity::capture_typed(pid, deadline)
        .map_err(|failure| guard.settle_observation_failure(failure))?;
    guard.ledger.daemon_pid = Some(pid);
    guard.ledger.daemon_process_start_identity = Some(identity);
    write_ledger(&guard.manifest_path, &guard.ledger)?;
    guard.evidence.push(
        EvidenceKind::Process,
        format!("verified daemon pid={pid} guardian pid={guardian_pid}"),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};

    #[test]
    fn outer_adoption_rejects_excess_entries_and_ignores_reused_group_identity() {
        let supervisor = include_str!("../../../../../scripts/ci/labby-live-e2e.sh");
        let adoption = supervisor
            .split_once("adopt_cleanup_helpers() {")
            .unwrap()
            .1
            .split_once("\nterminate_children() {")
            .unwrap()
            .0;
        assert!(adoption.contains("[ \"$admissions\" -le 8192 ]"));
        let adoption = format!("adopt_cleanup_helpers() {{{adoption}")
            .replace("[ \"$admissions\" -le 8192 ]", "[ \"$admissions\" -le 2 ]");
        let registry = tempfile::tempdir().unwrap();
        for index in 0..3 {
            let entry = registry.path().join(format!("admission-{index}"));
            std::fs::create_dir(&entry).unwrap();
            std::fs::write(
                entry.join("identity"),
                format!("2147483647\nold\ntoken\nadmission-{index}\n"),
            )
            .unwrap();
        }
        let status = Command::new("/bin/bash")
            .args([
                "-c",
                &format!("helper_registry=$1; cleanup=0; {adoption}\nadopt_cleanup_helpers"),
                "entry-cap",
            ])
            .arg(registry.path())
            .status()
            .unwrap();
        assert_eq!(
            status.code(),
            Some(70),
            "entry limit must fail, not truncate"
        );

        let registry = tempfile::tempdir().unwrap();
        let script = format!(
            "helper_registry=$1; cleanup=0; mkdir \"$1/admission-old\"; printf '%s\\nold\\ntoken\\nadmission-old\\n' $$ >\"$1/admission-old/identity\"; {adoption}\nadopt_cleanup_helpers; exit $cleanup"
        );
        let status = Command::new("/bin/bash")
            .args(["-c", &script, "stale-admission"])
            .arg(registry.path())
            .process_group(0)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "stale observation grants no authority over reused group"
        );
    }

    #[test]
    #[ignore = "inventory failure fail-stop subprocess fixture"]
    #[allow(
        clippy::panic,
        reason = "fixture must distinguish unexpected return from SIGABRT"
    )]
    fn inventory_failure_fail_stop_fixture() {
        let marker = PathBuf::from(std::env::var_os("LABBY_INVENTORY_KILL_MARKER").unwrap());
        let mut child = Command::new("/bin/sleep")
            .arg("0.1")
            .process_group(0)
            .spawn()
            .unwrap();
        terminate_and_reap_owned_child_with_operations(
            &mut child,
            unassigned_cleanup_job(),
            &mut Vec::new(),
            Instant::now() + Duration::from_secs(1),
            |pid| {
                signal_cleanup_group(pid)?;
                std::fs::write(&marker, b"owned group kill attempted")
                    .map_err(|error| error.to_string())
            },
            |_, _| Err("injected process inventory failure".into()),
        );
        panic!("inventory failure must fail-stop");
    }

    #[test]
    fn inventory_failure_attempts_owned_kill_before_fail_stop() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("kill-attempt");
        let module = module_path!().split_once("::").unwrap().1;
        let fixture = format!("{module}::inventory_failure_fail_stop_fixture");
        let child = Command::new(std::env::current_exe().unwrap())
            .args([&fixture, "--exact", "--ignored", "--nocapture"])
            .env("LABBY_INVENTORY_KILL_MARKER", &marker)
            .process_group(0)
            .spawn()
            .unwrap();
        let mut observed = None;
        let error = run_spawned_owned_child(
            child,
            Instant::now() + Duration::from_secs(3),
            assign_cleanup_job,
            |child| {
                observed = child.try_wait()?;
                Ok(observed)
            },
        )
        .unwrap_err();
        assert_eq!(
            observed.and_then(|status| status.signal()),
            Some(6),
            "{error}"
        );
        assert_eq!(
            std::fs::read(&marker).unwrap(),
            b"owned group kill attempted"
        );
    }

    #[test]
    fn cleanup_observer_retains_exited_leader_until_final_reap() {
        for script in ["exit 17", "kill -TERM $$", "ulimit -c 0; kill -ABRT $$"] {
            let mut child = Command::new("/bin/sh")
                .args(["-c", script])
                .process_group(0)
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            let observed = loop {
                if let Some(status) = observe_cleanup_child(child.id()).unwrap() {
                    break status;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            };
            assert_eq!(observe_cleanup_child(child.id()).unwrap(), Some(observed));
            assert!(
                process_group_members_checked(child.id() as i32)
                    .unwrap()
                    .is_empty()
            );
            signal_cleanup_group(child.id()).unwrap();
            assert_eq!(child.wait().unwrap(), observed);
            if script == "exit 17" {
                assert_eq!(observed.code(), Some(17));
            } else {
                assert_eq!(
                    observed.signal(),
                    Some(if script.contains("TERM") { 15 } else { 6 })
                );
            }
        }
    }

    #[test]
    fn surviving_descendant_is_resignaled_before_leader_identity_is_released() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("listener");
        let mut command = Command::new(env!("CARGO_BIN_EXE_live-harness-fixture"));
        command
            .args(["grandchild-listener", "0"])
            .arg(&marker)
            .process_group(0);
        let mut child = command.spawn().unwrap();
        let group = child.id();
        let deadline = Instant::now() + Duration::from_secs(3);
        let port = loop {
            if let Ok(text) = std::fs::read_to_string(&marker) {
                let fields: Vec<_> = text.split_whitespace().collect();
                if fields.len() == 3 && fields[2] == "ready" {
                    break fields[1].parse::<u16>().unwrap();
                }
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(observe_cleanup_child(group).unwrap(), None);
        let mut signals = 0;
        let mut errors = Vec::new();
        terminate_and_reap_owned_child_with_signal(
            &mut child,
            assign_cleanup_job(group).unwrap(),
            &mut errors,
            Instant::now() + Duration::from_secs(1),
            |pid| {
                signals += 1;
                if signals == 1 {
                    // Deterministically leave the descendant alive through the
                    // first group-signal attempt. Direct kill still exits the leader.
                    return Ok(());
                }
                assert!(observe_cleanup_child(pid).unwrap().is_some());
                signal_cleanup_group(pid)
            },
        );
        assert!(signals >= 2);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            process_group_members_checked(group as i32)
                .unwrap()
                .is_empty()
        );
        assert!(TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).is_ok());
        assert!(
            observe_cleanup_child(group).is_err(),
            "leader was not finally reaped"
        );
    }

    #[test]
    fn runtime_and_browser_guardians_reap_descendants_after_leader_exit() {
        for mode in ["runtime", "browser"] {
            let registry = tempfile::tempdir().unwrap();
            std::fs::set_permissions(registry.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let marker = registry.path().join("late-mutation");
            let mut command = Command::new("/bin/sh");
            command
                .args(["-c", "(sleep 0.3; touch -- \"$1\") & exit 0", "fixture"])
                .arg(&marker);
            let mut wrapper = if mode == "browser" {
                let mut wrapper = Command::new("/bin/sh");
                wrapper
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .env("LABBY_E2E_GROUP_TOKEN", "owned-gate-test")
                    .env("LABBY_E2E_HELPER_REGISTRY", registry.path())
                    .env("LABBY_E2E_BROWSER_EXECUTABLE", "/bin/sh")
                    .args(["-c", CLEANUP_HELPER_ADMISSION_GATE, "guardian"])
                    .args(command.get_args());
                wrapper
            } else {
                supervised_cleanup_command(&command, registry.path(), "owned-gate-test").unwrap()
            };
            wrapper.env("LABBY_E2E_GATE_MODE", mode).process_group(0);
            let child = wrapper.spawn().unwrap();
            let group = child.id() as i32;
            let mut observed_status = None;
            // Bound and reap the exact owned group even on a red regression.
            let result = run_spawned_owned_child(
                child,
                Instant::now() + Duration::from_secs(3),
                assign_cleanup_job,
                |child| {
                    observed_status = child.try_wait()?;
                    Ok(observed_status)
                },
            );
            assert_eq!(
                observed_status.and_then(|status| status.signal()),
                Some(9),
                "{mode}: {result:?}"
            );
            assert!(process_group_members_checked(group).unwrap().is_empty());
            std::thread::sleep(Duration::from_millis(400));
            assert!(
                !marker.exists(),
                "{mode}: descendant mutated after settlement"
            );
        }
    }

    #[test]
    fn closed_runtime_admission_cannot_execute_daemon_command() {
        let registry = tempfile::tempdir().unwrap();
        std::fs::set_permissions(registry.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(registry.path().join("closed")).unwrap();
        let marker = registry.path().join("forbidden-daemon-start");
        let mut command = Command::new("/usr/bin/touch");
        command.arg(&marker);
        let mut wrapper =
            supervised_cleanup_command(&command, registry.path(), "closed-daemon-test").unwrap();
        wrapper
            .env("LABBY_E2E_GATE_MODE", "runtime")
            .process_group(0);
        let mut observed_status = None;
        let result = run_spawned_owned_child(
            wrapper.spawn().unwrap(),
            Instant::now() + Duration::from_secs(3),
            assign_cleanup_job,
            |child| {
                observed_status = child.try_wait()?;
                Ok(observed_status)
            },
        );
        assert_eq!(
            observed_status.and_then(|status| status.code()),
            Some(70),
            "{result:?}"
        );
        assert!(!marker.exists());
    }

    #[test]
    fn guardian_publication_failure_reaps_child_before_later_mutation() {
        for mode in ["helper", "runtime"] {
            for publication in ["child.pid.pending", "status.pending"] {
                if mode == "runtime" && publication == "status.pending" {
                    continue; // Runtime exit is not a parked-helper status publication.
                }
                let registry = tempfile::tempdir().unwrap();
                std::fs::set_permissions(registry.path(), std::fs::Permissions::from_mode(0o700))
                    .unwrap();
                let marker = registry.path().join("forbidden-post-failure-mutation");
                // A native fixture inserted before spawn deterministically makes
                // the selected metadata write fail, without racing a watcher.
                let script = CLEANUP_HELPER_ADMISSION_GATE.replace(
                    "trap '' TERM",
                    &format!("mkdir \"$admission/{publication}\"\ntrap '' TERM"),
                );
                let mut command = Command::new("/bin/sh");
                command
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .env("LABBY_E2E_GROUP_TOKEN", "publication-failure-test")
                    .env("LABBY_E2E_GATE_MODE", mode)
                    .args(["-c", &script, "publication-fixture"])
                    .arg(registry.path())
                    .args([
                        "/bin/sh",
                        "-c",
                        "(sleep 0.3; touch -- \"$1\") & exit 0",
                        "fixture",
                    ])
                    .arg(&marker)
                    .process_group(0);
                let mut observed_status = None;
                let result = run_spawned_owned_child(
                    command.spawn().unwrap(),
                    Instant::now() + Duration::from_secs(3),
                    assign_cleanup_job,
                    |child| {
                        observed_status = child.try_wait()?;
                        Ok(observed_status)
                    },
                );
                assert_eq!(
                    observed_status.and_then(|status| status.signal()),
                    Some(9),
                    "{mode}/{publication}: {result:?}"
                );
                std::thread::sleep(Duration::from_millis(400));
                assert!(
                    !marker.exists(),
                    "{mode}/{publication}: mutation survived publication failure"
                );
            }
        }
    }

    #[tokio::test]
    #[ignore = "outer supervisor abrupt daemon-owner termination fixture"]
    async fn owned_daemon_abrupt_exit_fixture() {
        let mode = std::env::var("LABBY_E2E_DAEMON_OWNER_EXIT").unwrap();
        let mut guard = LiveLabbyBuilder::new().start().await.unwrap();
        let original_group = guard.ledger.process_group.unwrap();
        let original_start = process_start_identity(original_group as u32);
        if mode == "kill-after-restart" {
            guard.restart().await.unwrap();
            assert!(
                process_group_members_checked(original_group)
                    .unwrap()
                    .is_empty()
            );
        }
        let group = guard.ledger.process_group.unwrap();
        let members = process_group_members_checked(group).unwrap();
        let daemon_pid = guard.ledger.daemon_pid.unwrap();
        assert_ne!(daemon_pid, group as u32);
        assert_eq!(guard.ledger.guardian_pid, Some(group as u32));
        assert!(members.contains(&daemon_pid));
        let marker = PathBuf::from(std::env::var_os("LABBY_E2E_DAEMON_OWNER_MARKER").unwrap());
        let record = serde_json::json!({
            "owner_pid": std::process::id(),
            "original_group": original_group,
            "original_start": original_start,
            "group": group,
            "group_start": process_start_identity(group as u32),
            "daemon_pid": daemon_pid,
            "daemon_start": guard.ledger.daemon_process_start_identity,
            "members": members,
            "address": guard.restart.address.to_string(),
        });
        std::fs::write(marker, serde_json::to_vec(&record).unwrap()).unwrap();
        if mode == "abort" {
            std::process::abort();
        }
        assert_eq!(mode, "kill-after-restart");
        nix::sys::signal::kill(nix::unistd::getpid(), nix::sys::signal::Signal::SIGKILL).unwrap();
        // Signal delivery can trail the syscall return; never unwind the guard
        // and accidentally let Drop satisfy this abrupt-owner regression.
        loop {
            std::hint::spin_loop();
        }
    }
}
