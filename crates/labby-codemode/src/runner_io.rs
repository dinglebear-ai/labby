//! Parent-side helpers driving the Code Mode runner subprocess: stdin writes
//! and termination.

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin};

use crate::error::ToolError;

use super::protocol::CodeModeRunnerInput;

use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

/// Allow the entire supported trace and internal-call burst. Byte permits
/// cover both queued replies and the frame currently being written.
const MAX_REPLY_FRAMES: usize =
    crate::runner_drive::MAX_HOST_TRACE_ROWS + crate::config::MAX_INTERNAL_CALLS_PER_RUN;
const MAX_REPLY_BYTES: usize = super::protocol::MAX_PENDING_TOOL_PARAMS_BYTES_PER_RUN;

type ReplyFrame = (Vec<u8>, OwnedSemaphorePermit);

pub(crate) struct RunnerReplies {
    sender: mpsc::Sender<ReplyFrame>,
    bytes: Arc<Semaphore>,
}

impl RunnerReplies {
    pub(crate) fn start<W: tokio::io::AsyncWrite + Unpin + Send + 'static>(
        mut pipe: W,
    ) -> (
        Self,
        tokio_util::task::AbortOnDropHandle<Result<W, ToolError>>,
    ) {
        let (sender, mut receiver) = mpsc::channel::<ReplyFrame>(MAX_REPLY_FRAMES);
        let writer = tokio::spawn(async move {
            while let Some((line, _permit)) = receiver.recv().await {
                write_frame(&mut pipe, &line).await?;
            }
            Ok(pipe)
        });
        (
            Self {
                sender,
                bytes: Arc::new(Semaphore::new(MAX_REPLY_BYTES)),
            },
            tokio_util::task::AbortOnDropHandle::new(writer),
        )
    }

    pub(crate) fn enqueue(&self, input: &CodeModeRunnerInput) -> Result<(), ToolError> {
        let overloaded = || ToolError::Sdk {
            sdk_kind: "budget_exceeded".into(),
            message: "Code Mode runner has too many undelivered replies".into(),
        };
        let slot = self.sender.try_reserve().map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => overloaded(),
            mpsc::error::TrySendError::Closed(_) => {
                ToolError::internal_message("Code Mode runner writer closed")
            }
        })?;
        let len = super::protocol::serialized_len_within(
            input,
            self.bytes.available_permits().saturating_sub(1),
        )
        .map_err(|error| match error {
            super::protocol::SerializedLenError::Exceeded => overloaded(),
            super::protocol::SerializedLenError::Encode => {
                ToolError::internal_message("failed to encode Code Mode runner reply")
            }
        })?;
        let permit = self
            .bytes
            .clone()
            .try_acquire_many_owned((len + 1) as u32)
            .map_err(|_| overloaded())?;
        slot.send((encode_frame(input, len + 1)?, permit));
        Ok(())
    }
}

fn encode_frame(input: &CodeModeRunnerInput, capacity: usize) -> Result<Vec<u8>, ToolError> {
    let mut line = Vec::with_capacity(capacity);
    serde_json::to_writer(&mut line, input).map_err(|err| {
        ToolError::internal_message(format!("failed to encode Code Mode runner input: {err}"))
    })?;
    line.push(b'\n');
    Ok(line)
}

async fn write_frame(
    pipe: &mut (impl tokio::io::AsyncWrite + Unpin),
    line: &[u8],
) -> Result<(), ToolError> {
    pipe.write_all(line).await.map_err(|err| {
        ToolError::internal_message(format!("failed to write Code Mode runner input: {err}"))
    })?;
    pipe.flush().await.map_err(|err| {
        ToolError::internal_message(format!("failed to flush Code Mode runner input: {err}"))
    })
}

pub(crate) async fn write_runner_input(
    stdin: &mut ChildStdin,
    input: &CodeModeRunnerInput,
) -> Result<(), ToolError> {
    write_frame(stdin, &encode_frame(input, 128)?).await
}

pub(crate) async fn terminate_code_mode_runner(child: &mut Child, _pid: Option<u32>) {
    // On Unix, kill the entire process group (pgid == pid because we spawned
    // with process_group(0)) so that grandchildren are not re-parented to
    // PID 1 and left running after the runner exits.
    #[cfg(unix)]
    {
        if let Some(raw_pid) = child.id() {
            use nix::sys::signal::Signal;
            use nix::unistd::Pid;
            let _ = nix::sys::signal::killpg(Pid::from_raw(raw_pid as i32), Signal::SIGKILL);
        }
    }
    // On Windows, the `PooledRunner._job_guard` (a `JobObjectGuard` armed at
    // spawn in `pool/runner_handle.rs`) owns a Job Object with
    // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE. Dropping that guard when the runner
    // handle drops (on eviction, including after a timeout) lets the OS terminate
    // the whole descendant tree. This kill() call is therefore a
    // belt-and-suspenders direct kill of the immediate child process.
    // Reaping is bounded too: a closed stdout must not leave either the exit
    // wait or teardown holding an execution/pool slot indefinitely. The runner
    // handle still owns kill-on-drop and the group/Job Object cleanup guard.
    drop(child.start_kill());
    drop(tokio::time::timeout(std::time::Duration::from_secs(1), child.wait()).await);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

    fn reply(seq: u64) -> CodeModeRunnerInput {
        CodeModeRunnerInput::ToolResult {
            seq,
            result: serde_json::json!({"text": "ordinary result"}),
        }
    }

    #[tokio::test]
    async fn replies_are_fifo_while_output_is_read_independently() {
        let (host, mut runner) = tokio::io::duplex(16);
        let (mut output, pipe) = tokio::io::split(host);
        let (replies, writer) = RunnerReplies::start(pipe);
        replies.enqueue(&reply(1)).unwrap();
        replies.enqueue(&reply(2)).unwrap();
        runner.write_all(b"progress").await.unwrap();
        let mut progress = [0; 8];
        output.read_exact(&mut progress).await.unwrap();
        assert_eq!(&progress, b"progress");
        let mut lines = BufReader::new(runner).lines();
        for seq in [1, 2] {
            let line = lines.next_line().await.unwrap().unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&line).unwrap()["seq"],
                seq
            );
        }
        drop(replies);
        drop(writer.await.unwrap().unwrap());
    }

    #[tokio::test]
    async fn queued_replies_are_bounded_and_cancellation_releases_the_writer() {
        let (pipe, _runner) = tokio::io::duplex(1);
        let (replies, writer) = RunnerReplies::start(pipe);
        let held = replies
            .bytes
            .clone()
            .try_acquire_many_owned((MAX_REPLY_BYTES - 2) as u32)
            .unwrap();
        assert_eq!(
            replies.enqueue(&reply(0)).unwrap_err().kind(),
            "budget_exceeded"
        );
        drop(held);
        for seq in 0..MAX_REPLY_FRAMES {
            replies.enqueue(&reply(seq as u64)).unwrap();
        }
        assert_eq!(
            replies.enqueue(&reply(0)).unwrap_err().kind(),
            "budget_exceeded"
        );
        tokio::task::yield_now().await;
        drop(writer);
        tokio::time::timeout(std::time::Duration::from_secs(2), replies.sender.closed())
            .await
            .unwrap();
        assert_eq!(replies.bytes.available_permits(), MAX_REPLY_BYTES);
    }
}
