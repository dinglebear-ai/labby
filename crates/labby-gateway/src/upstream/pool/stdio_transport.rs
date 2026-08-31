//! Diagnostic stdio MCP transport with scoped child lifecycle telemetry.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap};
use rmcp::RoleClient;
use rmcp::service::{RawRxJsonRpcMessage, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::{Transport, async_rw::AsyncRwTransport};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};

use super::logging::UpstreamRequestLog;
use super::stdio_stderr::StdioDiagnostics;

const CHILD_EXIT_WAIT: Duration = Duration::from_secs(3);

type ChildProcessParts = (
    Box<dyn ChildWrapper>,
    ChildStdout,
    ChildStdin,
    Option<ChildStderr>,
);

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
static NEXT_INFLIGHT_ID: AtomicU64 = AtomicU64::new(1);
static INFLIGHT_REQUESTS: OnceLock<Mutex<HashMap<(String, u64), BTreeMap<u64, String>>>> =
    OnceLock::new();

fn inflight_requests() -> &'static Mutex<HashMap<(String, u64), BTreeMap<u64, String>>> {
    INFLIGHT_REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_unpoisoned<T>(mutex: &'static Mutex<T>) -> std::sync::MutexGuard<'static, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn begin_generation() -> u64 {
    NEXT_GENERATION.fetch_add(1, Ordering::Relaxed)
}

fn take_inflight(upstream: &str, generation: u64) -> Vec<String> {
    lock_unpoisoned(inflight_requests())
        .remove(&(upstream.to_string(), generation))
        .map(|requests| requests.into_values().collect())
        .unwrap_or_default()
}

/// RAII registration for one request using the currently active stdio
/// generation. On unexpected child exit, the transport snapshots these labels
/// before request futures unwind so logs identify exactly what was invalidated.
pub(super) struct StdioInflightGuard {
    upstream: String,
    generation: u64,
    id: u64,
}

impl Drop for StdioInflightGuard {
    fn drop(&mut self) {
        let key = (self.upstream.clone(), self.generation);
        let mut all = lock_unpoisoned(inflight_requests());
        if let Some(requests) = all.get_mut(&key) {
            requests.remove(&self.id);
            if requests.is_empty() {
                all.remove(&key);
            }
        }
    }
}

pub(super) fn register_inflight(
    event: UpstreamRequestLog<'_>,
    generation: Option<u64>,
) -> Option<StdioInflightGuard> {
    let generation = generation?;
    let id = NEXT_INFLIGHT_ID.fetch_add(1, Ordering::Relaxed);
    let item = event.item.unwrap_or("<none>");
    let label = format!("{}:{}:{}#{id}", event.capability, event.operation, item);
    lock_unpoisoned(inflight_requests())
        .entry((event.upstream.to_string(), generation))
        .or_default()
        .insert(id, label);
    Some(StdioInflightGuard {
        upstream: event.upstream.to_string(),
        generation,
        id,
    })
}

fn child_process(mut child: Box<dyn ChildWrapper>) -> std::io::Result<ChildProcessParts> {
    let stdin = child
        .inner_mut()
        .stdin()
        .take()
        .ok_or_else(|| std::io::Error::other("stdin was already taken"))?;
    let stdout = child
        .inner_mut()
        .stdout()
        .take()
        .ok_or_else(|| std::io::Error::other("stdout was already taken"))?;
    let stderr = child.inner_mut().stderr().take();
    Ok((child, stdout, stdin, stderr))
}

#[derive(Debug)]
struct ChildExit {
    status: Option<ExitStatus>,
    wait_error: Option<String>,
    killed_after_timeout: bool,
}

async fn wait_for_child(mut child: Box<dyn ChildWrapper>, kill_after_timeout: bool) -> ChildExit {
    match tokio::time::timeout(CHILD_EXIT_WAIT, child.wait()).await {
        Ok(Ok(status)) => ChildExit {
            status: Some(status),
            wait_error: None,
            killed_after_timeout: false,
        },
        Ok(Err(error)) => ChildExit {
            status: None,
            wait_error: Some(error.to_string()),
            killed_after_timeout: false,
        },
        Err(_) if kill_after_timeout => {
            let kill_error = Box::into_pin(child.kill())
                .await
                .err()
                .map(|error| error.to_string());
            let status = child.wait().await.ok();
            ChildExit {
                status,
                wait_error: kill_error,
                killed_after_timeout: true,
            }
        }
        Err(_) => ChildExit {
            status: None,
            wait_error: Some(format!(
                "child did not exit within {}ms",
                CHILD_EXIT_WAIT.as_millis()
            )),
            killed_after_timeout: false,
        },
    }
}

fn exit_code(status: Option<&ExitStatus>) -> Option<i32> {
    status.and_then(ExitStatus::code)
}

#[cfg(unix)]
fn exit_signal(status: Option<&ExitStatus>) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.and_then(ExitStatusExt::signal)
}

#[cfg(not(unix))]
fn exit_signal(_status: Option<&ExitStatus>) -> Option<i32> {
    None
}

async fn log_termination(
    upstream: String,
    generation: u64,
    pid: Option<u32>,
    event: &'static str,
    expected: bool,
    diagnostics: StdioDiagnostics,
    invalidated_requests: Vec<String>,
    exit: ChildExit,
) {
    let stderr_tail = diagnostics.snapshot().await;
    let status = exit.status.as_ref();
    let code = exit_code(status);
    let signal = exit_signal(status);
    let success = status.is_some_and(ExitStatus::success);
    let invalidated_count = invalidated_requests.len();

    if expected && success && invalidated_count == 0 {
        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.stdio.terminated",
            upstream = %upstream,
            generation,
            pid = ?pid,
            event,
            expected,
            exit_code = ?code,
            exit_signal = ?signal,
            killed_after_timeout = exit.killed_after_timeout,
            invalidated_count,
            stderr_tail = %stderr_tail,
            "stdio upstream child terminated"
        );
    } else {
        tracing::warn!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.stdio.terminated",
            upstream = %upstream,
            generation,
            pid = ?pid,
            event,
            expected,
            exit_code = ?code,
            exit_signal = ?signal,
            wait_error = exit.wait_error.as_deref(),
            killed_after_timeout = exit.killed_after_timeout,
            invalidated_count,
            invalidated_requests = ?invalidated_requests,
            stderr_tail = %stderr_tail,
            "stdio upstream child terminated with affected requests"
        );
    }
}

/// Labby-owned equivalent of rmcp's child transport. Keeping the child handle
/// here lets lifecycle logs retain upstream identity and exit status.
pub(super) struct DiagnosticChildTransport {
    child: Option<Box<dyn ChildWrapper>>,
    transport: AsyncRwTransport<RoleClient, ChildStdout, ChildStdin>,
    upstream: String,
    generation: u64,
    pid: Option<u32>,
    diagnostics: StdioDiagnostics,
}

impl DiagnosticChildTransport {
    pub(super) fn spawn(
        mut command: CommandWrap,
        upstream: String,
        diagnostics: StdioDiagnostics,
    ) -> std::io::Result<(Self, Option<ChildStderr>)> {
        command
            .command_mut()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (child, stdout, stdin, stderr) = child_process(command.spawn()?)?;
        let pid = child.id();
        let generation = begin_generation();
        Ok((
            Self {
                child: Some(child),
                transport: AsyncRwTransport::new(stdout, stdin),
                upstream,
                generation,
                pid,
                diagnostics,
            },
            stderr,
        ))
    }

    #[must_use]
    pub(super) const fn id(&self) -> Option<u32> {
        self.pid
    }

    #[must_use]
    pub(super) const fn generation(&self) -> u64 {
        self.generation
    }

    async fn finish_child(&mut self, event: &'static str, expected: bool) {
        let invalidated = take_inflight(&self.upstream, self.generation);
        let Some(child) = self.child.take() else {
            return;
        };
        let exit = wait_for_child(child, true).await;
        log_termination(
            self.upstream.clone(),
            self.generation,
            self.pid,
            event,
            expected,
            self.diagnostics.clone(),
            invalidated,
            exit,
        )
        .await;
    }
}

impl Drop for DiagnosticChildTransport {
    fn drop(&mut self) {
        let Some(child) = self.child.take() else {
            return;
        };
        let upstream = self.upstream.clone();
        let generation = self.generation;
        let pid = self.pid;
        let diagnostics = self.diagnostics.clone();
        let invalidated = take_inflight(&upstream, generation);
        tokio::spawn(async move {
            let exit = wait_for_child(child, true).await;
            log_termination(
                upstream,
                generation,
                pid,
                "transport_drop",
                false,
                diagnostics,
                invalidated,
                exit,
            )
            .await;
        });
    }
}

impl Transport<RoleClient> for DiagnosticChildTransport {
    type Error = std::io::Error;

    fn name() -> Cow<'static, str> {
        "labby-diagnostic-child-process".into()
    }

    fn preserves_raw_responses() -> bool {
        true
    }

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.transport.send(item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        let message = self.transport.receive().await;
        if message.is_none() {
            self.finish_child("transport_eof", false).await;
        }
        message
    }

    async fn receive_raw(&mut self) -> Option<RawRxJsonRpcMessage<RoleClient>> {
        let message = self.transport.receive_raw().await;
        if message.is_none() {
            self.finish_child("transport_eof", false).await;
        }
        message
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.transport.close().await?;
        self.finish_child("service_close", true).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inflight_registry_is_scoped_to_connection_generation() {
        let upstream = format!(
            "stdio-registry-{}",
            NEXT_GENERATION.fetch_add(1, Ordering::Relaxed)
        );
        let generation = begin_generation();
        let event = UpstreamRequestLog::tool(&upstream, "Bash", false);
        let guard = register_inflight(event, Some(generation)).expect("stdio generation");

        let requests = take_inflight(&upstream, generation);
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("Bash"));
        drop(guard);
        assert!(register_inflight(event, None).is_none());
    }
}
