//! Keep CLI children owned when output collection is cancelled.

use std::process::{Output, Stdio};
use std::time::Duration;

#[cfg(not(windows))]
struct OwnedCliChild(tokio::process::Child);

#[cfg(not(windows))]
impl Drop for OwnedCliChild {
    fn drop(&mut self) {
        // An outer timeout can cancel this future and immediately destroy its
        // runtime. Tokio's orphan reaper is then only best-effort. Retain the
        // waitable child and synchronously settle this exceptional drop path
        // within a separate one-second cleanup budget; never wait indefinitely.
        if matches!(self.0.try_wait(), Ok(Some(_))) {
            return;
        }
        drop(self.0.start_kill());
        let expires = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            match self.0.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) if std::time::Instant::now() < expires => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Ok(None) => return,
            }
        }
    }
}

#[cfg(not(windows))]
async fn collect_owned_output(
    child: tokio::process::Child,
    deadline: Duration,
) -> Result<Output, String> {
    use tokio::io::AsyncReadExt as _;

    let mut child = OwnedCliChild(child);
    let mut stdout = child.0.stdout.take().ok_or("CLI stdout pipe missing")?;
    let mut stderr = child.0.stderr.take().ok_or("CLI stderr pipe missing")?;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    // Unlike wait_with_output(), these futures borrow the child. Cancelling
    // collection therefore does not lose the handle required to kill and reap.
    let result = tokio::time::timeout(deadline, async {
        let (status, _, _) = tokio::try_join!(
            child.0.wait(),
            stdout.read_to_end(&mut stdout_bytes),
            stderr.read_to_end(&mut stderr_bytes),
        )?;
        Ok::<_, std::io::Error>(Output {
            status,
            stdout: stdout_bytes,
            stderr: stderr_bytes,
        })
    })
    .await;
    let error = match result {
        Ok(Ok(output)) => return Ok(output),
        Ok(Err(error)) => error.to_string(),
        Err(_) => format!("CLI child exceeded {deadline:?}"),
    };
    match tokio::time::timeout(Duration::from_secs(1), child.0.kill()).await {
        Ok(Ok(())) => Err(error),
        Ok(Err(cleanup)) => Err(format!("{error}; CLI child cleanup failed: {cleanup}")),
        Err(_) => Err(format!("{error}; CLI child cleanup deadline exceeded")),
    }
}

/// Windows pipe reads use Tokio's blocking pool. Dropping an output future
/// alone neither kills its child nor releases those reads, so runtime shutdown
/// can outlive the test timeout. Retain the job until every completion path.
pub(crate) async fn bounded_cli_output(
    command: &mut tokio::process::Command,
    deadline: Duration,
) -> Result<Output, String> {
    bounded_cli_output_after_admission(command, deadline, || {}).await
}

/// The fixture callback runs only after process containment has been attached.
pub(crate) async fn bounded_cli_output_after_admission(
    command: &mut tokio::process::Command,
    deadline: Duration,
    admitted: impl FnOnce(),
) -> Result<Output, String> {
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().map_err(|error| error.to_string())?;
    #[cfg(windows)]
    let job = {
        let mut child = child;
        let assigned = child
            .id()
            .ok_or_else(|| "CLI child has no process identity".to_owned())
            .and_then(|pid| {
                labby_winjob::JobObject::assign(pid).map_err(|error| error.to_string())
            });
        let job = match assigned {
            Ok(job) => job,
            Err(error) => {
                let cleanup = tokio::time::timeout(Duration::from_secs(1), child.kill()).await;
                return Err(format!(
                    "CLI job assignment failed: {error}; cleanup: {cleanup:?}"
                ));
            }
        };
        (child, job)
    };
    #[cfg(windows)]
    let (child, job) = job;

    admitted();
    #[cfg(not(windows))]
    let result = collect_owned_output(child, deadline).await;
    #[cfg(windows)]
    let result = tokio::time::timeout(deadline, child.wait_with_output())
        .await
        .map_err(|_| format!("CLI child exceeded {deadline:?}"))
        .and_then(|output| output.map_err(|error| error.to_string()));
    #[cfg(windows)]
    if let Err(error) = job.close() {
        return Err(format!(
            "CLI process-tree cleanup failed: {error}; output status: {}",
            result
                .as_ref()
                .map_or_else(|error| error.as_str(), |_| "collected")
        ));
    }
    result
}
