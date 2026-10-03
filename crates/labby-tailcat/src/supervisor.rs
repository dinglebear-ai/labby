use crate::{
    BridgeError, ValidatedBridgeConfig,
    protocol::{Start, read_event},
};
use labby_gateway::upstream::process_guard::ProcessGroupGuard;
use process_wrap::tokio::{ChildWrapper, CommandWrap, ProcessGroup};
use std::{fmt, os::unix::fs::PermissionsExt, process::Stdio, time::Duration};
use tokio::io::AsyncWriteExt;

/// Lifecycle state without connection capabilities or private keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeStatus {
    Ready,
    Stopped,
    Failed,
}

/// Opaque delivery encrypted to the helper's single approved browser peer.
pub struct SealedDelivery {
    pub sender: String,
    pub ciphertext: String,
}
impl fmt::Debug for SealedDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SealedDelivery([redacted])")
    }
}
type SealWaiter =
    std::sync::Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<SealedDelivery>>>>;

/// Secret address, delivered only through an authorized pairing response.
pub struct ConnectionCapability {
    address: String,
    port: u16,
}
impl fmt::Debug for ConnectionCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConnectionCapability([redacted])")
    }
}
impl ConnectionCapability {
    pub fn address(&self) -> &str {
        &self.address
    }
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Owns both the helper process group and its private verified artifact snapshot.
pub struct Bridge {
    child: std::sync::Arc<std::sync::Mutex<Option<Box<dyn ChildWrapper>>>>,
    stdin: Option<tokio::process::ChildStdin>,
    guard: Option<ProcessGroupGuard>,
    stderr_task: tokio::task::JoinHandle<()>,
    event_task: tokio::task::JoinHandle<()>,
    failure: std::sync::Arc<std::sync::atomic::AtomicBool>,
    capability: ConnectionCapability,
    snapshot: tempfile::TempDir,
    status: BridgeStatus,
    seal_waiter: SealWaiter,
    seal_requested: bool,
}

impl Bridge {
    pub async fn start(validated: ValidatedBridgeConfig) -> Result<Self, BridgeError> {
        Self::start_with_timeout(validated, Duration::from_secs(30)).await
    }

    /// A shorter startup bound is useful to callers with an enclosing deadline.
    pub async fn start_with_timeout(
        validated: ValidatedBridgeConfig,
        timeout: Duration,
    ) -> Result<Self, BridgeError> {
        crate::ensure_platform_supported()?;
        if timeout.is_zero() || timeout > Duration::from_secs(30) {
            return Err(BridgeError::InvalidConfig);
        }
        let snapshot = tempfile::Builder::new()
            .prefix("tailcat-")
            .tempdir_in(&validated.config.state_dir)
            .map_err(|_| BridgeError::ArtifactUnavailable)?;
        let executable = snapshot.path().join("bridge");
        std::fs::write(&executable, validated.bytes)
            .map_err(|_| BridgeError::ArtifactUnavailable)?;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o500))
            .map_err(|_| BridgeError::ArtifactUnavailable)?;
        let mut command = CommandWrap::with_new(&executable, |cmd| {
            cmd.current_dir(snapshot.path())
                .env_clear()
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
        });
        command.wrap(ProcessGroup::leader());
        let mut child = command.spawn().map_err(|_| BridgeError::ProcessFailed)?;
        let process_group = child.id().ok_or(BridgeError::ProcessFailed)?;
        let guard = ProcessGroupGuard::arm(process_group);
        let mut stdin = child.stdin().take().ok_or(BridgeError::ProcessFailed)?;
        let mut stdout = child.stdout().take().ok_or(BridgeError::ProcessFailed)?;
        let mut stderr = child.stderr().take().ok_or(BridgeError::ProcessFailed)?;
        let stderr_task = tokio::spawn(async move {
            drop(tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await);
        });
        // Construct ownership before the first await so cancellation kills the process tree.
        let failure = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seal_waiter: SealWaiter = Default::default();
        let mut bridge = Self {
            child: std::sync::Arc::new(std::sync::Mutex::new(Some(child))),
            stdin: None,
            guard: Some(guard),
            stderr_task,
            event_task: tokio::spawn(async {}),
            failure: failure.clone(),
            capability: ConnectionCapability {
                address: String::new(),
                port: 0,
            },
            snapshot,
            status: BridgeStatus::Failed,
            seal_waiter: seal_waiter.clone(),
            seal_requested: false,
        };
        let start = Start {
            version: 1,
            kind: "start",
            target: validated.config.target.to_string(),
            peer: &validated.config.peer,
            derp_map_url: &validated.config.derp_map_url,
        };
        let mut frame = serde_json::to_vec(&start).map_err(|_| BridgeError::Protocol)?;
        frame.push(b'\n');
        let ready = tokio::time::timeout(timeout, async {
            stdin
                .write_all(&frame)
                .await
                .map_err(|_| BridgeError::ProcessFailed)?;
            read_event(&mut stdout).await
        })
        .await
        .map_err(|_| BridgeError::StartupTimeout)??;
        if ready.kind != "ready"
            || ready.code.is_some()
            || ready.port != Some(1)
            || ready.sender.is_some()
            || ready.ciphertext.is_some()
        {
            return Err(BridgeError::Protocol);
        }
        let address = ready.address.ok_or(BridgeError::Protocol)?;
        if !address.starts_with("tcp")
            || address.len() > 4096
            || !address
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(BridgeError::Protocol);
        }
        bridge.capability = ConnectionCapability { address, port: 1 };
        bridge.status = BridgeStatus::Ready;
        bridge.stdin = Some(stdin);
        let monitored_child = bridge.child.clone();
        bridge.event_task = tokio::spawn(async move {
            // A single solicited seal response is allowed; everything else retires
            // the generation. Private helper stdout is never copied into logs.
            loop {
                let Ok(event) = read_event(&mut stdout).await else {
                    break;
                };
                if event.kind != "sealed"
                    || event.address.is_some()
                    || event.port.is_some()
                    || event.code.is_some()
                {
                    break;
                }
                let (Some(sender), Some(ciphertext)) = (event.sender, event.ciphertext) else {
                    break;
                };
                if !sender.strip_prefix("nodekey:").is_some_and(|key| {
                    key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
                }) || ciphertext.is_empty()
                    || ciphertext.len() > 24 * 1024
                    || !ciphertext
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte))
                {
                    break;
                }
                let waiter = seal_waiter
                    .lock()
                    .expect("seal ownership lock poisoned")
                    .take();
                let Some(waiter) = waiter else { break };
                if waiter.send(SealedDelivery { sender, ciphertext }).is_err() {
                    break;
                }
            }
            failure.store(true, std::sync::atomic::Ordering::Release);
            let _signal_result =
                labby_gateway::process::unix::terminate_process_group_sigkill(process_group);
            // Reap without requiring callers to poll status. Never hold the lock across await.
            loop {
                let finished = monitored_child
                    .lock()
                    .expect("child ownership lock poisoned")
                    .as_mut()
                    .is_none_or(|child| !matches!(child.try_wait(), Ok(None)));
                if finished {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        Ok(bridge)
    }

    pub fn capability(&self) -> &ConnectionCapability {
        &self.capability
    }

    /// Seal once using the running helper's key and its fixed approved peer.
    /// Cancellation or timeout invalidates the helper rather than retrying.
    pub async fn seal_delivery(&mut self, payload: &str) -> Result<SealedDelivery, BridgeError> {
        if self.status() != BridgeStatus::Ready
            || self.seal_requested
            || payload.is_empty()
            || payload.len() > 16 * 1024
        {
            return Err(BridgeError::Protocol);
        }
        let mut frame =
            serde_json::to_vec(&serde_json::json!({"version":1,"type":"seal","payload":payload}))
                .map_err(|_| BridgeError::Protocol)?;
        if frame.len() >= crate::protocol::FRAME_LIMIT {
            return Err(BridgeError::Protocol);
        }
        frame.push(b'\n');
        let (sender, receiver) = tokio::sync::oneshot::channel();
        *self
            .seal_waiter
            .lock()
            .expect("seal ownership lock poisoned") = Some(sender);
        self.seal_requested = true;
        // The in-flight future owns the kill guard, including when dropped.
        let owned_guard = self.guard.take();
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            self.stdin
                .as_mut()
                .ok_or(BridgeError::ProcessFailed)?
                .write_all(&frame)
                .await
                .map_err(|_| BridgeError::ProcessFailed)?;
            receiver.await.map_err(|_| BridgeError::Protocol)
        })
        .await;
        match result {
            Ok(Ok(delivery)) => {
                self.guard = owned_guard;
                Ok(delivery)
            }
            _ => {
                self.failure
                    .store(true, std::sync::atomic::Ordering::Release);
                drop(owned_guard);
                Err(BridgeError::Protocol)
            }
        }
    }

    pub fn status(&mut self) -> BridgeStatus {
        if self.status == BridgeStatus::Ready
            && (self.failure.load(std::sync::atomic::Ordering::Acquire)
                || self
                    .child
                    .lock()
                    .expect("child ownership lock poisoned")
                    .as_mut()
                    .is_none_or(|child| !matches!(child.try_wait(), Ok(None))))
        {
            self.guard.take();
            self.status = BridgeStatus::Failed;
        }
        self.status
    }

    pub async fn stop(&mut self) -> Result<(), BridgeError> {
        // Shutdown owns stdout and child termination from this point onward.
        self.event_task.abort();
        let owned_child = self
            .child
            .lock()
            .expect("child ownership lock poisoned")
            .take();
        let owned_guard = self.guard.take();
        if let Some(mut child) = owned_child {
            let graceful = tokio::time::timeout(Duration::from_secs(2), async {
                if let Some(input) = self.stdin.as_mut() {
                    input
                        .write_all(b"{\"version\":1,\"type\":\"stop\"}\n")
                        .await?;
                    input.shutdown().await?;
                }
                child.wait().await
            })
            .await;
            drop(owned_guard);
            if !matches!(graceful, Ok(Ok(_))) {
                drop(child.start_kill());
                drop(tokio::time::timeout(Duration::from_secs(2), child.wait()).await);
            }
        }
        self.stderr_task.abort();
        self.event_task.abort();
        self.capability.address.clear();
        self.status = BridgeStatus::Stopped;
        Ok(())
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.guard.take();
        self.stderr_task.abort();
        self.event_task.abort();
        // Child uses kill_on_drop; normal shutdown additionally awaits reaping.
        self.child
            .lock()
            .expect("child ownership lock poisoned")
            .take();
        let _ = self.snapshot.path();
    }
}
