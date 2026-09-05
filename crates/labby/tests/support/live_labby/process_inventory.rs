//! Bounded, nonrecursive inventory of the native process table.

use super::*;

const OUTPUT_CAP: usize = 1024 * 1024;
const REAP_RESERVE: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub(super) struct Failure {
    pub message: String,
    pub unsettled: Option<std::process::Child>,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            unsettled: None,
        }
    }
}

pub(super) fn read(deadline: Instant) -> Result<String, Failure> {
    let mut command = Command::new("/bin/ps");
    command
        .env_clear()
        .env("LC_ALL", "C")
        .args(["-axo", "pid=,pgid=,stat="]);
    read_command(&mut command, deadline, OUTPUT_CAP)
}

pub(super) fn start_identity(pid: u32, deadline: Instant) -> Result<String, Failure> {
    let mut command = Command::new("/bin/ps");
    command
        .env_clear()
        .env("LC_ALL", "C")
        .args(["-o", "lstart=", "-p", &pid.to_string()]);
    read_command(&mut command, deadline, 128)
}

fn read_command(
    command: &mut Command,
    deadline: Instant,
    output_cap: usize,
) -> Result<String, Failure> {
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
    let read_deadline = deadline
        .checked_sub(REAP_RESERVE)
        .filter(|cutoff| *cutoff > Instant::now())
        .ok_or_else(|| "process inventory deadline exhausted".to_string())?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("process inventory could not run: {error}"))?;
    let mut stdout = child.stdout.take().expect("requested inventory pipe");
    let outcome: Result<String, String> = (|| {
        let flags = fcntl_getfl(&stdout)
            .map_err(|error| format!("inventory pipe flags failed: {error}"))?;
        fcntl_setfl(&stdout, flags | OFlags::NONBLOCK)
            .map_err(|error| format!("inventory pipe configuration failed: {error}"))?;
        let mut output = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            if Instant::now() >= read_deadline {
                return Err("process inventory deadline exhausted".into());
            }
            match stdout.read(&mut buffer) {
                Ok(0) => {
                    if let Some(status) = child
                        .try_wait()
                        .map_err(|error| format!("inventory poll failed: {error}"))?
                    {
                        if !status.success() {
                            return Err(format!("process inventory failed with {status}"));
                        }
                        return String::from_utf8(output)
                            .map_err(|_| "process inventory was not UTF-8".into());
                    }
                }
                Ok(length) => {
                    if output.len() + length > output_cap {
                        return Err("process inventory output cap exceeded".into());
                    }
                    output.extend_from_slice(&buffer[..length]);
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(format!("process inventory read failed: {error}")),
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    })();
    // This is the directly spawned native ps process, not a shell or arbitrary
    // process tree. Never recurse through group inventory to settle inventory.
    let kill_error = outcome.is_err().then(|| child.kill().err()).flatten();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return outcome.map_err(Failure::from),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            result => {
                return Err(Failure {
                    message: settlement_failure_message(
                        outcome.as_ref().err().map(String::as_str),
                        kill_error.as_ref().map(ToString::to_string).as_deref(),
                        &format!("{result:?}"),
                    ),
                    unsettled: Some(child),
                });
            }
        }
    }
}

fn settlement_failure_message(
    primary: Option<&str>,
    kill: Option<&str>,
    settlement: &str,
) -> String {
    let bounded = |detail: &str| detail.chars().take(128).collect::<String>();
    format!(
        "inventory child could not settle: {}; operation: {}; kill: {}",
        bounded(settlement),
        bounded(primary.unwrap_or("completed")),
        bounded(kill.unwrap_or("no error")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settlement_diagnostics_preserve_each_bounded_failure() {
        let primary = format!("deadline exhausted {}", "界".repeat(1024));
        let kill = format!("permission denied {}", "y".repeat(1024));
        let settlement = format!("still running {}", "z".repeat(1024));
        let message = settlement_failure_message(Some(&primary), Some(&kill), &settlement);
        assert!(message.contains("deadline exhausted"));
        assert!(message.contains("permission denied"));
        assert!(message.contains("still running"));
        assert!(message.chars().count() < 512);
        let message = settlement_failure_message(None, None, "poll failed");
        assert!(message.contains("operation: completed"));
        assert!(message.contains("kill: no error"));
    }

    #[test]
    fn inventory_native_probe_is_bounded_and_parseable() {
        let text = read(Instant::now() + Duration::from_secs(1)).unwrap();
        assert!(
            parse_process_group_inventory(i32::MAX, &text)
                .unwrap()
                .is_empty()
        );
        let started = Instant::now();
        let mut sleeper = Command::new("/bin/sleep");
        sleeper.arg("5");
        let failure = read_command(
            &mut sleeper,
            started + Duration::from_millis(250),
            OUTPUT_CAP,
        )
        .unwrap_err();
        assert!(failure.message.contains("deadline"), "{failure:?}");
        assert!(failure.unsettled.is_none(), "{failure:?}");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "elapsed={:?}",
            started.elapsed()
        );
        let mut producer = Command::new("/usr/bin/yes");
        // Exercise the shared cap boundary without requiring a minimum process
        // throughput under load. Native inventory retains its 1 MiB cap.
        assert_eq!(OUTPUT_CAP, 1024 * 1024);
        let failure =
            read_command(&mut producer, Instant::now() + Duration::from_secs(1), 128).unwrap_err();
        assert!(failure.message.contains("output cap"), "{failure:?}");
        assert!(failure.unsettled.is_none(), "{failure:?}");
    }
}
