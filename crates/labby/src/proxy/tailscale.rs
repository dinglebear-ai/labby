//! Tailscale Serve publication for the ephemeral stdio proxy.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

use crate::proxy::config::ProxyPortPreference;

/// Relevant fields from `tailscale status --json`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TailscaleStatus {
    backend_state: String,
    #[serde(rename = "Self")]
    self_node: TailscaleSelf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TailscaleSelf {
    online: bool,
    #[serde(rename = "DNSName")]
    dns_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailscaleIdentity {
    pub dns_name: String,
}

impl TailscaleStatus {
    pub fn parse(json: &str) -> Result<Self> {
        serde_json::from_str(json).context("invalid `tailscale status --json` response")
    }

    pub fn require_online(&self) -> Result<TailscaleIdentity> {
        if self.backend_state != "Running" {
            bail!(
                "Tailscale backend state is {:?}, expected Running",
                self.backend_state
            );
        }
        if !self.self_node.online {
            bail!("the local Tailscale node is offline");
        }
        if self.self_node.dns_name.is_empty() {
            bail!("the local Tailscale node has no DNS name");
        }
        Ok(TailscaleIdentity {
            dns_name: self.self_node.dns_name.clone(),
        })
    }
}

/// Relevant fields from `tailscale serve status --json`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ServeStatus {
    #[serde(default, rename = "TCP")]
    tcp: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    web: BTreeMap<String, ServeWeb>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ServeWeb {
    #[serde(default)]
    handlers: BTreeMap<String, ServeHandler>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ServeHandler {
    proxy: Option<String>,
}

impl ServeStatus {
    pub fn parse(json: &str) -> Result<Self> {
        serde_json::from_str(json).context("invalid `tailscale serve status --json` response")
    }

    #[must_use]
    pub fn occupied_ports(&self) -> BTreeSet<u16> {
        let mut ports = self
            .tcp
            .keys()
            .filter_map(|port| port.parse().ok())
            .collect::<BTreeSet<_>>();
        ports.extend(self.web.keys().filter_map(|authority| {
            authority
                .rsplit_once(':')
                .and_then(|(_, port)| port.parse::<u16>().ok())
        }));
        ports
    }

    #[must_use]
    pub fn backend_for(&self, dns_name: &str, port: u16) -> Option<&str> {
        let authority = format!("{dns_name}:{port}");
        self.web
            .get(&authority)?
            .handlers
            .get("/")?
            .proxy
            .as_deref()
    }
}

pub fn build_public_url(dns_name: &str, port: u16, path: &str) -> Result<url::Url> {
    let host = dns_name.strip_suffix('.').unwrap_or(dns_name);
    url::Url::parse(&format!("https://{host}:{port}{path}"))
        .context("failed to construct Tailscale proxy URL")
}

pub fn select_port_from_candidates(
    preference: ProxyPortPreference,
    range_start: u16,
    range_end: u16,
    status: &ServeStatus,
    candidates: impl IntoIterator<Item = u16>,
    max_attempts: usize,
) -> Result<u16> {
    let occupied = status.occupied_ports();
    if let Some(port) = preference.fixed() {
        if occupied.contains(&port) {
            bail!("Tailscale Serve port {port} is already configured");
        }
        return Ok(port);
    }

    for port in candidates.into_iter().take(max_attempts) {
        if (range_start..=range_end).contains(&port) && !occupied.contains(&port) {
            return Ok(port);
        }
    }
    bail!(
        "no unused Tailscale Serve port found in {range_start}..={range_end} after {max_attempts} attempts"
    )
}

#[derive(Debug, Clone)]
pub struct TailscaleServeOptions {
    pub executable: PathBuf,
    pub local_addr: SocketAddr,
    pub path: String,
    pub port: ProxyPortPreference,
    pub port_range_start: u16,
    pub port_range_end: u16,
    pub candidate_ports: Vec<u16>,
    pub max_attempts: usize,
    pub poll_interval: Duration,
    pub readiness_timeout: Duration,
}

impl TailscaleServeOptions {
    #[must_use]
    pub fn for_proxy(
        local_addr: SocketAddr,
        path: String,
        port: ProxyPortPreference,
        port_range_start: u16,
        port_range_end: u16,
    ) -> Self {
        Self {
            executable: PathBuf::from("tailscale"),
            local_addr,
            path,
            port,
            port_range_start,
            port_range_end,
            candidate_ports: Vec::new(),
            max_attempts: 32,
            poll_interval: Duration::from_millis(50),
            readiness_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug)]
pub struct TailscaleServe {
    executable: PathBuf,
    child: Option<Child>,
    stdout_task: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    stderr_task: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    dns_name: String,
    external_port: u16,
    backend: String,
    public_url: url::Url,
    poll_interval: Duration,
    readiness_timeout: Duration,
}

impl TailscaleServe {
    pub async fn start(options: TailscaleServeOptions) -> Result<Self> {
        if options.max_attempts == 0 {
            bail!("Tailscale Serve port selection requires at least one attempt");
        }
        let version = run_checked(&options.executable, ["version"]).await?;
        if version.trim().is_empty() {
            bail!("Tailscale CLI returned an empty version");
        }
        let status_output = run_checked(&options.executable, ["status", "--json"]).await?;
        let identity = TailscaleStatus::parse(&status_output)?.require_online()?;
        let dns_name = identity
            .dns_name
            .strip_suffix('.')
            .unwrap_or(&identity.dns_name)
            .to_string();
        let serve_output = run_checked(&options.executable, ["serve", "status", "--json"]).await?;
        let initial_status = ServeStatus::parse(&serve_output)?;

        let candidates = if let Some(port) = options.port.fixed() {
            vec![port]
        } else if options.candidate_ports.is_empty() {
            random_candidates(
                options.port_range_start,
                options.port_range_end,
                options.max_attempts,
            )?
        } else {
            options.candidate_ports.clone()
        };
        let occupied = initial_status.occupied_ports();
        let backend = format!("http://127.0.0.1:{}", options.local_addr.port());
        let mut last_error = None;
        let random_mode = options.port.fixed().is_none();

        for external_port in candidates.into_iter().take(options.max_attempts) {
            if !(options.port_range_start..=options.port_range_end).contains(&external_port)
                && random_mode
            {
                continue;
            }
            if occupied.contains(&external_port) {
                if random_mode {
                    continue;
                }
                bail!("Tailscale Serve port {external_port} is already configured");
            }

            match Self::claim(&options, dns_name.clone(), external_port, backend.clone()).await {
                Ok(serve) => return Ok(serve),
                Err(error) if random_mode && is_collision_error(&error.to_string()) => {
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }

        let suffix = last_error
            .map(|error| format!("; last Serve error: {error:#}"))
            .unwrap_or_default();
        bail!(
            "no usable Tailscale Serve port found in {}..={} after {} attempts{}",
            options.port_range_start,
            options.port_range_end,
            options.max_attempts,
            suffix
        )
    }

    async fn claim(
        options: &TailscaleServeOptions,
        dns_name: String,
        external_port: u16,
        backend: String,
    ) -> Result<Self> {
        let mut child = Command::new(&options.executable)
            .arg("serve")
            .arg("--yes")
            .arg(format!("--https={external_port}"))
            .arg(&backend)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                format!(
                    "failed to start `{}` Serve process",
                    options.executable.display()
                )
            })?;
        let stdout_task = child.stdout.take().map(drain_pipe);
        let stderr_task = child.stderr.take().map(drain_pipe);
        let deadline = tokio::time::Instant::now() + options.readiness_timeout;

        loop {
            if let Some(status) = child
                .try_wait()
                .context("failed to inspect Serve process")?
            {
                let stdout = join_output(stdout_task).await;
                let stderr = join_output(stderr_task).await;
                bail!(
                    "Tailscale Serve exited before exact mapping verification with {status}: {}{}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr)
                );
            }
            let status = read_serve_status(&options.executable).await?;
            if status.backend_for(&dns_name, external_port) == Some(backend.as_str()) {
                return Ok(Self {
                    executable: options.executable.clone(),
                    child: Some(child),
                    stdout_task,
                    stderr_task,
                    dns_name: dns_name.clone(),
                    external_port,
                    backend,
                    public_url: build_public_url(&dns_name, external_port, &options.path)?,
                    poll_interval: options.poll_interval,
                    readiness_timeout: options.readiness_timeout,
                });
            }
            if tokio::time::Instant::now() >= deadline {
                terminate_child(&mut child).await;
                bail!(
                    "timed out waiting for exact Tailscale Serve mapping on {dns_name}:{external_port}"
                );
            }
            tokio::time::sleep(options.poll_interval).await;
        }
    }

    #[must_use]
    pub fn public_url(&self) -> &url::Url {
        &self.public_url
    }

    #[must_use]
    pub const fn external_port(&self) -> u16 {
        self.external_port
    }

    pub async fn wait_for_failure(&mut self) -> Result<()> {
        loop {
            if let Some(status) = self
                .child
                .as_mut()
                .context("Tailscale Serve process is no longer owned")?
                .try_wait()
                .context("failed to inspect Tailscale Serve process")?
            {
                bail!("Tailscale Serve foreground process exited unexpectedly: {status}");
            }
            let status = read_serve_status(&self.executable).await?;
            match status.backend_for(&self.dns_name, self.external_port) {
                Some(backend) if backend == self.backend => {}
                Some(backend) => bail!(
                    "Tailscale Serve mapping ownership changed from {} to {backend}",
                    self.backend
                ),
                None => bail!("owned Tailscale Serve mapping disappeared unexpectedly"),
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            terminate_child_with_timeout(&mut child, self.readiness_timeout).await;
        }
        if let Some(task) = self.stdout_task.take() {
            drop(task.await);
        }
        if let Some(task) = self.stderr_task.take() {
            drop(task.await);
        }

        let deadline = tokio::time::Instant::now() + self.readiness_timeout;
        loop {
            let status = read_serve_status(&self.executable).await?;
            match status.backend_for(&self.dns_name, self.external_port) {
                None => return Ok(()),
                Some(backend) if backend != self.backend => {
                    bail!(
                        "Tailscale Serve mapping ownership changed from {} to {backend}; refusing cleanup",
                        self.backend
                    );
                }
                Some(_) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(self.poll_interval).await;
                }
                Some(_) => break,
            }
        }

        run_checked(
            &self.executable,
            [
                OsString::from("serve"),
                OsString::from("--yes"),
                OsString::from(format!("--https={}", self.external_port)),
                OsString::from("off"),
            ],
        )
        .await
        .context("exact-port Tailscale Serve cleanup failed")?;
        let status = read_serve_status(&self.executable).await?;
        if status
            .backend_for(&self.dns_name, self.external_port)
            .is_some()
        {
            bail!("exact-port Tailscale Serve cleanup did not remove the owned mapping");
        }
        Ok(())
    }
}

impl Drop for TailscaleServe {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            drop(child.start_kill());
        }
        if let Some(task) = self.stdout_task.take() {
            task.abort();
        }
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

fn drain_pipe(
    mut pipe: impl tokio::io::AsyncRead + Unpin + Send + 'static,
) -> JoinHandle<std::io::Result<Vec<u8>>> {
    tokio::spawn(async move {
        let mut output = Vec::new();
        let mut chunk = [0_u8; 1_024];
        loop {
            let read = pipe.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            output.extend_from_slice(&chunk[..read]);
            const CAPTURE_LIMIT: usize = 16 * 1_024;
            if output.len() > CAPTURE_LIMIT {
                output.drain(..output.len() - CAPTURE_LIMIT);
            }
        }
        Ok(output)
    })
}

async fn join_output(task: Option<JoinHandle<std::io::Result<Vec<u8>>>>) -> Vec<u8> {
    match task {
        Some(task) => task.await.ok().and_then(Result::ok).unwrap_or_default(),
        None => Vec::new(),
    }
}

async fn run_checked<I, S>(executable: &PathBuf, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("failed to execute `{}`", executable.display()))?;
    if !output.status.success() {
        bail!(
            "`{}` exited with {}: {}",
            executable.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout).context("Tailscale CLI emitted non-UTF-8 JSON")
}

async fn read_serve_status(executable: &PathBuf) -> Result<ServeStatus> {
    ServeStatus::parse(&run_checked(executable, ["serve", "status", "--json"]).await?)
}

fn random_candidates(start: u16, end: u16, count: usize) -> Result<Vec<u16>> {
    if start > end {
        bail!("invalid proxy port range {start}..={end}");
    }
    let width = u32::from(end) - u32::from(start) + 1;
    let mut result = Vec::with_capacity(count);
    while result.len() < count {
        let mut bytes = [0_u8; 2];
        getrandom::fill(&mut bytes)
            .context("OS randomness unavailable for proxy port selection")?;
        let candidate = u32::from(u16::from_ne_bytes(bytes)) % width + u32::from(start);
        let candidate = u16::try_from(candidate).context("proxy port candidate overflowed")?;
        result.push(candidate);
    }
    Ok(result)
}

fn is_collision_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("already configured")
        || lower.contains("already in use")
        || lower.contains("conflict")
}

async fn terminate_child(child: &mut Child) {
    terminate_child_with_timeout(child, Duration::from_secs(1)).await;
}

async fn terminate_child_with_timeout(child: &mut Child, timeout: Duration) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ignored = crate::process::unix::terminate_sigterm(pid);
    }
    #[cfg(not(unix))]
    drop(child.start_kill());

    if tokio::time::timeout(timeout, child.wait()).await.is_err() {
        drop(child.start_kill());
        drop(child.wait().await);
    }
}
