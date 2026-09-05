//! Keep CLI children owned when output collection is cancelled.

use std::process::{Output, Stdio};
use std::time::Duration;

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
