//! Tailscale Serve or Funnel publication for the ephemeral stdio proxy.

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

use crate::proxy::config::{ProxyAuthMode, ProxyExposure, ProxyPortPreference};

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

    #[must_use]
    pub fn backend_running(&self) -> bool {
        self.backend_state == "Running"
    }

    #[must_use]
    pub fn online(&self) -> bool {
        self.self_node.online
    }

    #[must_use]
    pub fn dns_name(&self) -> &str {
        &self.self_node.dns_name
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
    #[serde(default)]
    foreground: BTreeMap<String, ServeConfig>,
    #[serde(default)]
    allow_funnel: BTreeMap<String, bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ServeConfig {
    #[serde(default, rename = "TCP")]
    tcp: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    web: BTreeMap<String, ServeWeb>,
    #[serde(default)]
    allow_funnel: BTreeMap<String, bool>,
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
            .filter_map(|port| port.parse::<u16>().ok())
            .collect::<BTreeSet<_>>();
        extend_ports(&mut ports, &self.web);
        for config in self.foreground.values() {
            ports.extend(
                config
                    .tcp
                    .keys()
                    .filter_map(|port| port.parse::<u16>().ok()),
            );
            extend_ports(&mut ports, &config.web);
        }
        ports
    }

    #[must_use]
    pub fn backend_for(&self, dns_name: &str, port: u16) -> Option<&str> {
        let authority = format!("{dns_name}:{port}");
        backend_from_web(&self.web, &authority).or_else(|| {
            self.foreground
                .values()
                .find_map(|config| backend_from_web(&config.web, &authority))
        })
    }

    #[must_use]
    pub fn is_funnel(&self, dns_name: &str, port: u16) -> bool {
        let authority = format!("{dns_name}:{port}");
        self.allow_funnel.get(&authority) == Some(&true)
            || self
                .foreground
                .values()
                .any(|config| config.allow_funnel.get(&authority) == Some(&true))
    }
}

fn extend_ports(ports: &mut BTreeSet<u16>, web: &BTreeMap<String, ServeWeb>) {
    ports.extend(web.keys().filter_map(|authority| {
        authority
            .rsplit_once(':')
            .and_then(|(_, port)| port.parse::<u16>().ok())
    }));
}

fn backend_from_web<'a>(web: &'a BTreeMap<String, ServeWeb>, authority: &str) -> Option<&'a str> {
    web.get(authority)?.handlers.get("/")?.proxy.as_deref()
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
            bail!("Tailscale publication port {port} is already configured");
        }
        return Ok(port);
    }

    for port in candidates.into_iter().take(max_attempts) {
        if (range_start..=range_end).contains(&port) && !occupied.contains(&port) {
            return Ok(port);
        }
    }
    bail!(
        "no unused Tailscale publication port found in {range_start}..={range_end} after {max_attempts} attempts"
    )
}

#[derive(Debug, Clone)]
pub struct TailscaleServeOptions {
    pub executable: PathBuf,
    pub local_addr: SocketAddr,
    pub exposure: ProxyExposure,
    pub auth: ProxyAuthMode,
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
            exposure: ProxyExposure::Tailscale,
            auth: ProxyAuthMode::Tailnet,
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

fn require_funnel_oauth(options: &TailscaleServeOptions) -> Result<()> {
    if options.exposure == ProxyExposure::Funnel && options.auth != ProxyAuthMode::Oauth {
        bail!("public Tailscale Funnel exposure requires OAuth authentication");
    }
    Ok(())
}

struct ForegroundProcessGuard {
    #[cfg(unix)]
    _guard: Option<labby_gateway::upstream::process_guard::ProcessGroupGuard>,
    #[cfg(windows)]
    _guard: Option<labby_gateway::upstream::process_guard::JobObjectGuard>,
}

impl std::fmt::Debug for ForegroundProcessGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ForegroundProcessGuard")
    }
}

impl ForegroundProcessGuard {
    fn arm(child: &Child) -> Self {
        Self {
            #[cfg(unix)]
            _guard: child
                .id()
                .map(labby_gateway::upstream::process_guard::ProcessGroupGuard::arm),
            #[cfg(windows)]
            _guard: child
                .id()
                .map(labby_gateway::upstream::process_guard::JobObjectGuard::arm),
        }
    }
}

#[derive(Debug)]
pub struct TailscaleServe {
    executable: PathBuf,
    child: Option<Child>,
    process_guard: Option<ForegroundProcessGuard>,
    stdout_task: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    stderr_task: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    dns_name: String,
    external_port: u16,
    backend: String,
    exposure: ProxyExposure,
    public_url: url::Url,
    poll_interval: Duration,
    readiness_timeout: Duration,
}

#[derive(Debug)]
pub struct TailscaleServePlan {
    options: TailscaleServeOptions,
    dns_name: String,
    external_port: u16,
    backend: String,
    public_url: url::Url,
}

#[derive(Debug, thiserror::Error)]
pub enum TailscaleClaimError {
    #[error("Tailscale publication port collision: {0:#}")]
    Collision(anyhow::Error),
    #[error("Tailscale publication claim failed: {0:#}")]
    Failed(anyhow::Error),
}

impl TailscaleServePlan {
    pub async fn prepare(options: TailscaleServeOptions) -> Result<Self> {
        require_funnel_oauth(&options)?;
        if options.max_attempts == 0 {
            bail!("Tailscale publication port selection requires at least one attempt");
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
        } else if !options.candidate_ports.is_empty() {
            options.candidate_ports.clone()
        } else if options.exposure == ProxyExposure::Funnel {
            vec![443, 8443, 10000]
        } else {
            random_candidates(
                options.port_range_start,
                options.port_range_end,
                options.max_attempts,
            )?
        };
        let candidates = if options.exposure == ProxyExposure::Funnel {
            candidates
                .into_iter()
                .filter(|port| matches!(port, 443 | 8443 | 10000))
                .collect()
        } else {
            candidates
        };
        let external_port = select_port_from_candidates(
            options.port,
            if options.exposure == ProxyExposure::Funnel {
                443
            } else {
                options.port_range_start
            },
            if options.exposure == ProxyExposure::Funnel {
                10000
            } else {
                options.port_range_end
            },
            &initial_status,
            candidates,
            options.max_attempts,
        )?;
        let backend = format!("http://127.0.0.1:{}", options.local_addr.port());
        let public_url = build_public_url(&dns_name, external_port, &options.path)?;
        Ok(Self {
            options,
            dns_name,
            external_port,
            backend,
            public_url,
        })
    }

    #[must_use]
    pub const fn external_port(&self) -> u16 {
        self.external_port
    }

    #[must_use]
    pub fn public_url(&self) -> &url::Url {
        &self.public_url
    }

    pub async fn claim(self) -> Result<TailscaleServe> {
        self.claim_typed().await.map_err(anyhow::Error::from)
    }

    pub async fn claim_typed(self) -> std::result::Result<TailscaleServe, TailscaleClaimError> {
        let result = TailscaleServe::claim(
            &self.options,
            self.dns_name,
            self.external_port,
            self.backend,
        )
        .await;
        result.map_err(|error| {
            if is_collision_error(&error.to_string()) {
                TailscaleClaimError::Collision(error)
            } else {
                TailscaleClaimError::Failed(error)
            }
        })
    }
}

impl TailscaleServe {
    pub async fn start(options: TailscaleServeOptions) -> Result<Self> {
        require_funnel_oauth(&options)?;
        if options.max_attempts == 0 {
            bail!("Tailscale publication port selection requires at least one attempt");
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
        } else if !options.candidate_ports.is_empty() {
            options.candidate_ports.clone()
        } else if options.exposure == ProxyExposure::Funnel {
            vec![443, 8443, 10000]
        } else {
            random_candidates(
                options.port_range_start,
                options.port_range_end,
                options.max_attempts,
            )?
        };
        let occupied = initial_status.occupied_ports();
        let backend = format!("http://127.0.0.1:{}", options.local_addr.port());
        let mut last_error = None;
        let random_mode = options.port.fixed().is_none();

        for external_port in candidates.into_iter().take(options.max_attempts) {
            if !(if options.exposure == ProxyExposure::Funnel {
                matches!(external_port, 443 | 8443 | 10000)
            } else {
                (options.port_range_start..=options.port_range_end).contains(&external_port)
            }) && random_mode
            {
                continue;
            }
            if occupied.contains(&external_port) {
                if random_mode {
                    continue;
                }
                bail!("Tailscale publication port {external_port} is already configured");
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
            "no usable Tailscale publication port found in {}..={} after {} attempts{}",
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
        require_funnel_oauth(options)?;
        let verb = if options.exposure == ProxyExposure::Funnel {
            "funnel"
        } else {
            "serve"
        };
        let mut command = Command::new(&options.executable);
        command
            .arg(verb)
            .arg("--yes")
            .arg(format!("--https={external_port}"))
            .arg(&backend)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to start `{}` Serve process",
                options.executable.display()
            )
        })?;
        let process_guard = ForegroundProcessGuard::arm(&child);
        let stdout_task = child.stdout.take().map(drain_pipe);
        let stderr_task = child.stderr.take().map(drain_pipe);
        let deadline = tokio::time::Instant::now() + options.readiness_timeout;

        loop {
            if let Some(status) = child
                .try_wait()
                .context("failed to inspect Serve process")?
            {
                drop(process_guard);
                let stdout = join_output(stdout_task).await;
                let stderr = join_output(stderr_task).await;
                bail!(
                    "Tailscale publication exited before exact mapping verification with {status}: {}{}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr)
                );
            }
            let status = read_serve_status(&options.executable).await?;
            if status.backend_for(&dns_name, external_port) == Some(backend.as_str())
                && status.is_funnel(&dns_name, external_port)
                    == (options.exposure == ProxyExposure::Funnel)
            {
                return Ok(Self {
                    executable: options.executable.clone(),
                    child: Some(child),
                    process_guard: Some(process_guard),
                    stdout_task,
                    stderr_task,
                    dns_name: dns_name.clone(),
                    external_port,
                    backend,
                    exposure: options.exposure,
                    public_url: build_public_url(&dns_name, external_port, &options.path)?,
                    poll_interval: options.poll_interval,
                    readiness_timeout: options.readiness_timeout,
                });
            }
            if tokio::time::Instant::now() >= deadline {
                terminate_child(&mut child).await;
                bail!(
                    "timed out waiting for exact Tailscale publication mapping on {dns_name}:{external_port}"
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
                .context("Tailscale publication process is no longer owned")?
                .try_wait()
                .context("failed to inspect Tailscale publication process")?
            {
                bail!("Tailscale publication foreground process exited unexpectedly: {status}");
            }
            let status = read_serve_status(&self.executable).await?;
            match status.backend_for(&self.dns_name, self.external_port) {
                Some(backend)
                    if backend == self.backend
                        && status.is_funnel(&self.dns_name, self.external_port)
                            == (self.exposure == ProxyExposure::Funnel) => {}
                Some(backend) if backend == self.backend => {
                    bail!("owned Tailscale publication mode changed")
                }
                Some(backend) => bail!(
                    "Tailscale publication mapping ownership changed from {} to {backend}",
                    self.backend
                ),
                None => bail!("owned Tailscale publication mapping disappeared unexpectedly"),
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            terminate_child_with_timeout(&mut child, self.readiness_timeout).await;
        }
        // The leader may exit while descendants retain its inherited pipes.
        drop(self.process_guard.take());
        drop(join_output(self.stdout_task.take()).await);
        drop(join_output(self.stderr_task.take()).await);

        let deadline = tokio::time::Instant::now() + self.readiness_timeout;
        loop {
            let status = read_serve_status(&self.executable).await?;
            match status.backend_for(&self.dns_name, self.external_port) {
                None => return Ok(()),
                Some(backend) if backend != self.backend => {
                    bail!(
                        "Tailscale publication mapping ownership changed from {} to {backend}; refusing cleanup",
                        self.backend
                    );
                }
                Some(_)
                    if status.is_funnel(&self.dns_name, self.external_port)
                        != (self.exposure == ProxyExposure::Funnel) =>
                {
                    bail!("Tailscale publication mode changed; refusing cleanup");
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
                OsString::from(if self.exposure == ProxyExposure::Funnel {
                    "funnel"
                } else {
                    "serve"
                }),
                OsString::from("--yes"),
                OsString::from(format!("--https={}", self.external_port)),
                OsString::from("off"),
            ],
        )
        .await
        .context("exact-port Tailscale publication cleanup failed")?;
        let status = read_serve_status(&self.executable).await?;
        if status
            .backend_for(&self.dns_name, self.external_port)
            .is_some()
        {
            bail!("exact-port Tailscale publication cleanup did not remove the owned mapping");
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
        Some(mut task) => match tokio::time::timeout(Duration::from_millis(250), &mut task).await {
            Ok(result) => result.ok().and_then(Result::ok).unwrap_or_default(),
            Err(_) => {
                task.abort();
                drop(task.await);
                Vec::new()
            }
        },
        None => Vec::new(),
    }
}

pub(crate) async fn run_checked<I, S>(executable: &PathBuf, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    run_checked_with_timeout(executable, args, Duration::from_secs(5)).await
}

async fn read_bounded_output(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let count = pipe.read(&mut chunk).await?;
        if count == 0 {
            return Ok(output);
        }
        if output.len() + count > limit {
            return Err(std::io::Error::other(
                "Tailscale helper output exceeded capture limit",
            ));
        }
        output.extend_from_slice(&chunk[..count]);
    }
}

async fn run_checked_with_timeout<I, S>(
    executable: &PathBuf,
    args: I,
    timeout: Duration,
) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to execute `{}`", executable.display()))?;
    #[cfg(unix)]
    let process_guard = child
        .id()
        .map(labby_gateway::upstream::process_guard::ProcessGroupGuard::arm);
    #[cfg(windows)]
    let process_guard = child
        .id()
        .map(labby_gateway::upstream::process_guard::JobObjectGuard::arm);
    let stdout = child
        .stdout
        .take()
        .context("Tailscale helper stdout missing")?;
    let stderr = child
        .stderr
        .take()
        .context("Tailscale helper stderr missing")?;
    let result = tokio::time::timeout(timeout, async {
        tokio::try_join!(
            child.wait(),
            read_bounded_output(stdout, 1024 * 1024),
            read_bounded_output(stderr, 16 * 1024)
        )
    })
    .await;
    #[cfg(any(unix, windows))]
    drop(process_guard);
    let (status, stdout, stderr) = match result {
        Ok(Ok(output)) => output,
        error => {
            drop(child.start_kill());
            drop(tokio::time::timeout(Duration::from_secs(1), child.wait()).await);
            return match error {
                Ok(Err(error)) => Err(error).context("Tailscale helper capture failed"),
                Err(_) => Err(anyhow::anyhow!(
                    "Tailscale helper timed out after {}ms",
                    timeout.as_millis()
                )),
                Ok(Ok(_)) => unreachable!(),
            };
        }
    };
    if !status.success() {
        bail!(
            "`{}` exited with {}: {}",
            executable.display(),
            status,
            String::from_utf8_lossy(&stderr)
        );
    }
    String::from_utf8(stdout).context("Tailscale CLI emitted non-UTF-8 JSON")
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

#[cfg(all(test, unix))]
mod helper_tests {
    use super::*;

    #[tokio::test]
    async fn foreground_output_drain_aborts_when_pipe_stays_open() {
        use tokio::io::AsyncWriteExt as _;
        let (reader, mut writer) = tokio::io::duplex(64);
        let drain = drain_pipe(reader);
        tokio::time::timeout(Duration::from_secs(1), join_output(Some(drain)))
            .await
            .unwrap();
        assert!(
            writer.write_all(b"still open").await.is_err(),
            "timed out drain must be aborted, not detached"
        );
    }

    #[tokio::test]
    async fn foreground_owner_reaps_descendants_after_leader_exit() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("survived");
        let script = format!("(sleep 0.5; touch '{}') & exit 23", marker.display());
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", &script])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn().unwrap();
        let guard = ForegroundProcessGuard::arm(&child);
        let drain = child.stdout.take().map(drain_pipe);
        assert_eq!(child.wait().await.unwrap().code(), Some(23));
        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), join_output(drain))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn helper_timeout_reaps_descendants() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("survived");
        let script = format!("(sleep 0.5; touch '{}') & wait", marker.display());
        let error = run_checked_with_timeout(
            &PathBuf::from("/bin/sh"),
            ["-c", &script],
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!marker.exists(), "helper descendant survived timeout");
    }

    #[tokio::test]
    async fn helper_cancellation_reaps_descendants() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("survived");
        let started = temp.path().join("started");
        let script = format!(
            "(touch '{}'; sleep 0.5; touch '{}') & wait",
            started.display(),
            marker.display()
        );
        let task = tokio::spawn(async move {
            run_checked_with_timeout(
                &PathBuf::from("/bin/sh"),
                ["-c", &script],
                Duration::from_secs(5),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !started.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!marker.exists(), "helper descendant survived cancellation");
    }

    #[tokio::test]
    async fn helper_output_is_bounded() {
        let error = run_checked_with_timeout(
            &PathBuf::from("/bin/sh"),
            ["-c", "while :; do printf '%4096s' x; done"],
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("capture limit"));
        assert_eq!(
            run_checked(&PathBuf::from("/bin/sh"), ["-c", "printf '{}'"])
                .await
                .unwrap(),
            "{}"
        );
    }
}
