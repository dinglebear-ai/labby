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
    let Some(registry) = &guard.guardian_admission else {
        return Ok(());
    };
    let guardian_pid = guard
        .ledger
        .guardian_pid
        .ok_or("missing daemon guardian identity")?;
    let pid = read_daemon_pid(registry, deadline)?;
    if pid == guardian_pid
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

pub(super) fn read_daemon_pid(admission: &Path, deadline: Instant) -> Result<u32, String> {
    read_daemon_pid_with_observation(admission, deadline, || Ok(()))
}

fn read_daemon_pid_with_observation(
    admission: &Path,
    deadline: Instant,
    mut on_missing: impl FnMut() -> Result<(), String>,
) -> Result<u32, String> {
    use std::io::Read as _;
    // Listener readiness and atomic PID publication are independent events.
    // Wait only for a not-yet-published file, within the caller's original
    // readiness budget; malformed or unreadable evidence never retries.
    let expired = || "guarded daemon PID publication deadline exceeded".to_string();
    let file = loop {
        if Instant::now() >= deadline {
            return Err(expired());
        }
        match std::fs::File::open(admission.join("child.pid")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                on_missing()?;
                std::thread::sleep(
                    Duration::from_millis(5)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            result => {
                break result
                    .map_err(|error| format!("cannot read guarded daemon PID: {error}"))?;
            }
        }
    };
    let mut value = String::new();
    file.take(12)
        .read_to_string(&mut value)
        .map_err(|error| error.to_string())?;
    if Instant::now() >= deadline {
        return Err(expired());
    }
    if value.len() > 11 {
        return Err("guarded daemon PID exceeds its bound".into());
    }
    let pid = value
        .trim_end_matches('\n')
        .parse::<u32>()
        .map_err(|_| "invalid guarded daemon PID".to_string())?;
    if pid == 0 {
        return Err("invalid guarded daemon PID".into());
    }
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};

    #[test]
    fn daemon_pid_publication_preserves_deadline_and_rejects_invalid_evidence() {
        let admission = tempfile::tempdir().unwrap();
        let expired = read_daemon_pid_with_observation(admission.path(), Instant::now(), || {
            Err("expired reader must not retry".into())
        })
        .unwrap_err();
        assert!(expired.contains("deadline"));
        let failure = read_daemon_pid_with_observation(
            admission.path(),
            Instant::now() + Duration::from_secs(1),
            || Err("injected missing-publication observation failure".into()),
        )
        .unwrap_err();
        assert_eq!(failure, "injected missing-publication observation failure");
        for value in ["0\n", "invalid\n", "123456789012\n"] {
            std::fs::write(admission.path().join("child.pid"), value).unwrap();
            let error = read_daemon_pid_with_observation(
                admission.path(),
                Instant::now() + Duration::from_secs(1),
                || Err("malformed evidence must not retry".into()),
            )
            .unwrap_err();
            assert!(!error.contains("retry"));
        }
        std::fs::write(admission.path().join("child.pid"), "41\n").unwrap();
        assert_eq!(
            read_daemon_pid(admission.path(), Instant::now() + Duration::from_secs(1)).unwrap(),
            41
        );
        assert!(
            read_daemon_pid(admission.path(), Instant::now())
                .unwrap_err()
                .contains("deadline")
        );
    }

    #[test]
    fn listener_readiness_can_precede_guardian_pid_publication() {
        for publish in [true, false] {
            let registry = tempfile::tempdir().unwrap();
            let admission_id = format!("admission-{}", "a".repeat(48));
            let admission = registry.path().join(&admission_id);
            let marker = registry.path().join("listener");
            let release = registry.path().join("publish");
            let publication = "mv \"$admission/child.pid.pending\" \"$admission/child.pid\"";
            assert!(CLEANUP_HELPER_ADMISSION_GATE.contains(publication));
            let script = CLEANUP_HELPER_ADMISSION_GATE.replace(
                publication,
                &format!(
                    "while [ ! -f \"$registry/publish\" ]; do sleep 0.01; done\n{publication}"
                ),
            );
            let child = Command::new("/bin/sh")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("LABBY_E2E_GROUP_TOKEN", "pid-publication-test")
                .env("LABBY_E2E_GATE_MODE", "runtime")
                .env("LABBY_E2E_ADMISSION_ID", &admission_id)
                .args(["-c", &script, "pid-publication"])
                .arg(registry.path())
                .arg(env!("CARGO_BIN_EXE_live-harness-fixture"))
                .args(["child-listener", "0"])
                .arg(&marker)
                .process_group(0)
                .spawn()
                .unwrap();
            let group = child.id();
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut observed = None;
            let mut missing_observed = false;
            let result = run_spawned_owned_child(child, deadline, assign_cleanup_job, |_| {
                let Ok(text) = std::fs::read_to_string(&marker) else {
                    return Ok(None);
                };
                let fields: Vec<_> = text.split_whitespace().collect();
                if fields.len() != 3 {
                    return Ok(None);
                }
                let daemon_pid = fields[0].parse::<u32>().unwrap();
                let port = fields[1].parse::<u16>().unwrap();
                let address = SocketAddr::from(([127, 0, 0, 1], port));
                let listener_ready =
                    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(20))
                        .is_ok();
                let before_publication = !admission.join("child.pid").try_exists()?;
                let pid = read_daemon_pid_with_observation(&admission, deadline, || {
                    missing_observed = true;
                    if publish {
                        std::fs::write(&release, b"publish").map_err(|error| error.to_string())
                    } else {
                        Ok(())
                    }
                });
                observed = Some((daemon_pid, address, listener_ready, before_publication, pid));
                Ok(Some(std::process::ExitStatus::from_raw(0)))
            });
            // Cleanup uses the retained owner even when the publication assertion is red.
            assert!(result.is_ok(), "{result:?}");
            let (daemon_pid, address, listener_ready, before_publication, pid) = observed.unwrap();
            assert!(missing_observed && listener_ready && before_publication);
            assert!(
                process_group_members_checked(group as i32)
                    .unwrap()
                    .is_empty()
            );
            assert!(TcpListener::bind(address).is_ok());
            if publish {
                assert_eq!(pid, Ok(daemon_pid));
            } else {
                assert!(pid.unwrap_err().contains("deadline"));
            }
        }
    }

    #[test]
    fn eperm_requires_an_exited_owner_and_successful_empty_inventory() {
        for mode in ["live", "nonempty", "failed"] {
            let mut child = Command::new("/bin/sh")
                .args([
                    "-c",
                    if mode == "live" {
                        "exec sleep 5"
                    } else {
                        "exit 70"
                    },
                ])
                .process_group(0)
                .spawn()
                .unwrap();
            let pid = child.id();
            let deadline = Instant::now() + Duration::from_secs(3);
            if mode != "live" {
                while observe_cleanup_child(pid).unwrap().is_none() {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            let mut inventory_calls = 0;
            let result = signal_cleanup_group_with_probe(
                pid,
                deadline,
                |_| Err(nix::errno::Errno::EPERM),
                |_, _| {
                    inventory_calls += 1;
                    if mode == "failed" {
                        Err("injected inventory failure".into())
                    } else {
                        Ok(vec![pid])
                    }
                },
            );
            let mut errors = Vec::new();
            terminate_and_reap_owned_child(&mut child, unassigned_cleanup_job(), &mut errors);
            assert!(result.is_err(), "{mode}");
            assert_eq!(inventory_calls, usize::from(mode != "live"));
            assert!(errors.is_empty(), "{mode}: {errors:?}");
        }
    }

    #[test]
    #[ignore = "one-inventory cleanup budget subprocess fixture"]
    fn single_inventory_budget_fixture() {
        let marker = PathBuf::from(std::env::var_os("LABBY_SINGLE_INVENTORY_MARKER").unwrap());
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 70"])
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        let setup = Instant::now() + Duration::from_secs(3);
        while observe_cleanup_child(pid).unwrap().is_none() {
            assert!(Instant::now() < setup);
            std::thread::sleep(Duration::from_millis(2));
        }
        let calls = std::cell::Cell::new(0);
        let deadline = Instant::now() + Duration::from_secs(1);
        let inventory = |group, expires| {
            assert_eq!(expires, deadline);
            calls.set(calls.get() + 1);
            if calls.get() > 1 {
                return Err(
                    "injected second inventory exceeds remaining shared probe budget".into(),
                );
            }
            let members = process_group_members_checked_before(group, expires)?;
            assert!(members.is_empty());
            std::fs::write(&marker, b"empty observation admitted").unwrap();
            Ok(members)
        };
        let mut errors = Vec::new();
        terminate_and_reap_owned_child_with_operations(
            &mut child,
            unassigned_cleanup_job(),
            &mut errors,
            deadline,
            |pid| {
                signal_cleanup_group_with_probe(
                    pid,
                    deadline,
                    |_| Err(nix::errno::Errno::EPERM),
                    inventory,
                )
            },
            inventory,
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(calls.get(), 1);
        assert!(
            observe_cleanup_child(pid).is_err(),
            "retained child not finally reaped"
        );
        std::fs::write(marker, b"single observation completed").unwrap();
    }

    #[test]
    fn exited_empty_group_consumes_only_one_inventory_budget() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("inventory");
        let module = module_path!().split_once("::").unwrap().1;
        let fixture = format!("{module}::single_inventory_budget_fixture");
        let child = Command::new(std::env::current_exe().unwrap())
            .args([&fixture, "--exact", "--ignored", "--nocapture"])
            .env("LABBY_SINGLE_INVENTORY_MARKER", &marker)
            .process_group(0)
            .spawn()
            .unwrap();
        let result = run_spawned_owned_child(
            child,
            Instant::now() + Duration::from_secs(5),
            assign_cleanup_job,
            |child| child.try_wait(),
        );
        assert!(result.is_ok(), "{result:?}; admitted={}", marker.exists());
        assert_eq!(
            std::fs::read(marker).unwrap(),
            b"single observation completed"
        );
    }

    #[test]
    fn adoption_rechecks_an_exited_leader_before_reporting_cleanup_failure() {
        let supervisor = include_str!("../../../../../scripts/ci/labby-live-e2e.sh");
        let adoption = supervisor
            .split_once("adopt_cleanup_helpers() {")
            .unwrap()
            .1
            .split_once("\nterminate_children() {")
            .unwrap()
            .0;
        let registry = tempfile::tempdir().unwrap();
        let entry = registry.path().join("admission-exited");
        std::fs::create_dir(&entry).unwrap();
        std::fs::write(
            entry.join("identity"),
            "2147483646\nprevious start\ntoken\nadmission-exited\n",
        )
        .unwrap();
        let script = format!(
            r#"
helper_registry=$1; mode=$2; cleanup=0
ps() {{
  case "$*" in
    '-axo pid=,pgid=')
      if [ ! -f "$helper_registry/inventory-seen" ]; then
        touch "$helper_registry/inventory-seen"
        printf '2147483646 2147483646\n'
      elif [ "$mode" = live ]; then
        printf '2147483647 2147483646\n'
      elif [ "$mode" = failed ]; then
        return 1
      fi ;;
    '-o lstart= -p 2147483646') return 1 ;;
    *) return 99 ;;
  esac
}}
register_group() {{ exit 98; }}
adopt_cleanup_helpers() {{{adoption}
adopt_cleanup_helpers
exit "$cleanup"
"#
        );
        for (mode, expected) in [("absent", 0), ("live", 1), ("failed", 1)] {
            let status = Command::new("/bin/bash")
                .args(["-c", &script, "exited-admission"])
                .arg(registry.path())
                .arg(mode)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(expected), "{mode}: {status}");
            std::fs::remove_file(registry.path().join("inventory-seen")).unwrap();
        }
    }

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
                    .map(|()| SignalDisposition::Sent)
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
                    return Ok(SignalDisposition::Sent);
                }
                assert!(observe_cleanup_child(pid).unwrap().is_some());
                signal_cleanup_group(pid).map(|()| SignalDisposition::Sent)
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
