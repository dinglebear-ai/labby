//! Real server ownership survives abrupt termination of its Rust test owner.

use std::net::{SocketAddr, TcpListener};
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn outer_supervisor_reaps_ready_daemons_after_abort_and_kill_after_restart() {
    for mode in ["abort", "kill-after-restart"] {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join(mode);
        let marker = root.join("daemon-owner.json");
        let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/ci/labby-live-e2e.sh");
        let started = Instant::now();
        let status = Command::new("bash")
            .arg(script)
            .args(["pr", "1"])
            .env("LABBY_E2E_RUN_ROOT", &root)
            .env("LABBY_E2E_ESCAPED_HELPER_SELFTEST", "1")
            .env(
                "LABBY_E2E_HELPER_TEST_BINARY",
                std::env::current_exe().unwrap(),
            )
            .env(
                "LABBY_E2E_HELPER_TEST_FILTER",
                "support::live_labby::guardian::tests::owned_daemon_abrupt_exit_fixture",
            )
            .env("LABBY_E2E_DAEMON_OWNER_EXIT", mode)
            .env("LABBY_E2E_DAEMON_OWNER_MARKER", &marker)
            .env("LABBY_E2E_PREBUILT", "1")
            .env("LABBY_E2E_BINARY", env!("CARGO_BIN_EXE_labby"))
            .env("LABBY_E2E_SHARD_TIMEOUT_SECONDS", "25")
            .env("LABBY_E2E_RUN_TIMEOUT_SECONDS", "35")
            .status()
            .unwrap();
        let record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(marker).expect("owner reached real readiness"))
                .unwrap();
        let group = record["group"].as_i64().unwrap() as i32;
        let original_group = record["original_group"].as_i64().unwrap() as i32;
        let daemon_pid = record["daemon_pid"].as_u64().unwrap();
        let inventory = Command::new("ps")
            .args(["-axo", "pid=,pgid=,stat="])
            .output()
            .unwrap();
        let inventory_text = String::from_utf8(inventory.stdout).unwrap();
        let rows: Result<Vec<_>, &str> = inventory_text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let mut fields = line.split_whitespace();
                let pid = fields.next().and_then(|value| value.parse::<u32>().ok());
                let pgid = fields.next().and_then(|value| value.parse::<i32>().ok());
                let state = fields.next();
                match (pid, pgid, state, fields.next()) {
                    (Some(pid), Some(pgid), Some(state), None) => Ok((pid, pgid, state)),
                    _ => Err("invalid process inventory row"),
                }
            })
            .collect();
        let address: SocketAddr = record["address"].as_str().unwrap().parse().unwrap();
        let listener_absent = TcpListener::bind(address).is_ok();
        // A red regression must not leave the server behind. Only signal
        // groups whose leader start identity still matches this owned fixture.
        for (owned_group, key) in [(group, "group_start"), (original_group, "original_start")] {
            let identity = Command::new("ps")
                .args(["-o", "lstart=", "-p", &owned_group.to_string()])
                .output()
                .unwrap();
            if identity.status.success()
                && format!(
                    "pid:{owned_group}:{}",
                    String::from_utf8_lossy(&identity.stdout).trim()
                ) == record[key].as_str().unwrap()
            {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(owned_group),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
        }
        // The guardian itself may be gone in a red regression. The recorded
        // daemon identity plus its still-owned PG is independent kill evidence.
        let daemon_identity = Command::new("ps")
            .args(["-o", "lstart=", "-p", &daemon_pid.to_string()])
            .output()
            .unwrap();
        let daemon_group = Command::new("ps")
            .args(["-o", "pgid=", "-p", &daemon_pid.to_string()])
            .output()
            .unwrap();
        if daemon_identity.status.success()
            && daemon_group.status.success()
            && format!(
                "pid:{daemon_pid}:{}",
                String::from_utf8_lossy(&daemon_identity.stdout).trim()
            ) == record["daemon_start"].as_str().unwrap()
            && String::from_utf8_lossy(&daemon_group.stdout)
                .trim()
                .parse::<i32>()
                == Ok(group)
        {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(group),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        assert!(
            !status.success(),
            "abrupt owner exit must fail qualification"
        );
        assert!(started.elapsed() < Duration::from_secs(35));
        assert!(inventory.status.success(), "process inventory unavailable");
        let rows = rows.expect("complete valid process inventory required");
        let survivors: Vec<_> = rows
            .iter()
            .filter(|(pid, pgid, state)| {
                (*pgid == group || *pgid == original_group || u64::from(*pid) == daemon_pid)
                    && !state.starts_with('Z')
            })
            .collect();
        assert!(
            survivors.is_empty(),
            "{mode}: orphaned daemon processes {survivors:?}"
        );
        assert!(
            listener_absent,
            "{mode}: real daemon listener remained bound"
        );
        assert!(
            record["members"].as_array().unwrap().len() >= 2,
            "must exercise guardian plus real daemon"
        );
        assert_ne!(daemon_pid, group as u64);
        assert!(
            record["members"]
                .as_array()
                .unwrap()
                .iter()
                .any(|pid| pid.as_u64() == Some(daemon_pid))
        );
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("artifacts/status.json")).unwrap())
                .unwrap();
        assert_eq!(report["primary"], 1);
        assert_eq!(report["cleanup"], 0);
    }
}
