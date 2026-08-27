use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::future::Future;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::process::{Child, Command as TokioCommand};

use super::evidence::{EvidenceKind, RunEvidence, sanitize};

const DEFAULT_DEADLINE: Duration = Duration::from_secs(20);
const LOG_TAIL_BYTES: usize = 32 * 1024;
const DROP_DEADLINE: Duration = Duration::from_secs(3);

struct RevocationGuard {
    revoke: Box<dyn FnMut() -> Result<(), String> + Send>,
    is_absent: Box<dyn Fn() -> Result<bool, String> + Send>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct RunIdentity {
    pub(crate) run_id: String,
    pub(crate) seed: u64,
    #[serde(default, skip_serializing)]
    pub(crate) nonce: String,
    pub(crate) git_sha: String,
    pub(crate) git_dirty: bool,
    pub(crate) binary_sha256: String,
    pub(crate) binary_version: String,
    pub(crate) platform: String,
    pub(crate) features: Vec<String>,
    pub(crate) ui_asset_sha256: String,
    pub(crate) fixture_versions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SanitizedConnectionDescriptor {
    pub(crate) run_id: String,
    pub(crate) base_url: String,
    pub(crate) health_url: String,
    pub(crate) ready_url: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct OwnershipLedger {
    pub(crate) generation: u64,
    pub(crate) created_at_ms: u128,
    pub(crate) nonce: String,
    pub(crate) root: PathBuf,
    pub(crate) pid: Option<u32>,
    pub(crate) process_start_identity: Option<String>,
    pub(crate) process_group: Option<i32>,
    pub(crate) listener: Option<SocketAddr>,
    pub(crate) listener_identity: Option<String>,
    pub(crate) locks: Vec<PathBuf>,
    pub(crate) credential_sessions: Vec<String>,
    pub(crate) owned_roots: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CleanupResult {
    pub(crate) primary_failure: Option<String>,
    pub(crate) graceful: bool,
    pub(crate) forced: bool,
    pub(crate) failures: Vec<String>,
    pub(crate) retention_failure: Option<String>,
}

impl CleanupResult {
    pub(crate) fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Clone)]
pub(crate) struct LiveLabbyBuilder {
    readiness_deadline: Duration,
    extra_env: BTreeMap<OsString, OsString>,
    args: Vec<OsString>,
    port: Option<u16>,
    ready_path: String,
    config: Option<String>,
    fail_evidence_writes: bool,
    existing_root: Option<PathBuf>,
}

impl Default for LiveLabbyBuilder {
    fn default() -> Self {
        Self {
            readiness_deadline: DEFAULT_DEADLINE,
            extra_env: BTreeMap::new(),
            args: Vec::new(),
            port: None,
            ready_path: "/ready".to_string(),
            config: None,
            fail_evidence_writes: false,
            existing_root: None,
        }
    }
}

impl LiveLabbyBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn readiness_deadline(mut self, deadline: Duration) -> Self {
        self.readiness_deadline = deadline;
        self
    }

    pub(crate) fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub(crate) fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.extra_env.insert(key.into(), value.into());
        self
    }

    pub(crate) fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub(crate) fn ready_path(mut self, path: impl Into<String>) -> Self {
        self.ready_path = path.into();
        self
    }

    pub(crate) fn config(mut self, config: impl Into<String>) -> Self {
        self.config = Some(config.into());
        self
    }

    pub(crate) fn fail_evidence_writes(mut self) -> Self {
        self.fail_evidence_writes = true;
        self
    }

    /// Start in a caller-owned canonical test root. This supports workflows
    /// whose offline setup phase must precede daemon startup in the same
    /// installation. The caller retains ownership and cleanup responsibility.
    pub(crate) fn existing_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.existing_root = Some(root.into());
        self
    }

    pub(crate) async fn start(self) -> Result<LiveLabbyGuard, String> {
        self.start_with_retries(4).await
    }

    fn start_with_retries(
        self,
        attempts: u8,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<LiveLabbyGuard, String>>>> {
        Box::pin(async move {
            let retry = self.clone();
            let owned_parent = std::env::temp_dir().join("labby-live-e2e");
            std::fs::create_dir_all(&owned_parent).map_err(|error| error.to_string())?;
            let (root_guard, root) = if let Some(root) = &self.existing_root {
                (None, canonical_owned_root(root, &owned_parent)?)
            } else {
                let guard = tempfile::Builder::new()
                    .prefix("run-")
                    .tempdir_in(&owned_parent)
                    .map_err(|error| error.to_string())?;
                let root = canonical_owned_root(guard.path(), &owned_parent)?;
                (Some(guard), root)
            };
            let identity = build_identity()?;
            let credential_canary = random_secret_canary()?;
            let nonce_path = root.join("ownership.nonce");
            write_nonce(&nonce_path, &identity.nonce)?;
            let manifest_path = root.join("ownership.json");
            let stdout_path = root.join("stdout.log");
            let stderr_path = root.join("stderr.log");
            let home = root.join("home");
            let labby_home = root.join("labby-home");
            let xdg_config = root.join("xdg/config");
            let xdg_cache = root.join("xdg/cache");
            let xdg_runtime = root.join("xdg/runtime");
            let temp = root.join("tmp");
            for path in [
                &home,
                &labby_home,
                &xdg_config,
                &xdg_cache,
                &xdg_runtime,
                &temp,
            ] {
                std::fs::create_dir_all(path).map_err(|error| error.to_string())?;
            }
            if let Some(config) = &self.config {
                std::fs::write(labby_home.join("config.toml"), config)
                    .map_err(|error| error.to_string())?;
            }

            let address = if let Some(port) = self.port {
                SocketAddr::from(([127, 0, 0, 1], port))
            } else {
                let listener =
                    TcpListener::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
                let address = listener.local_addr().map_err(|error| error.to_string())?;
                drop(listener);
                address
            };

            let mut evidence = RunEvidence::new(identity.clone());
            evidence.push(EvidenceKind::Setup, format!("allocated {}", root.display()));
            let mut ledger = OwnershipLedger {
                generation: 1,
                created_at_ms: unix_timestamp_ms(),
                nonce: identity.nonce.clone(),
                root: root.clone(),
                listener: Some(address),
                listener_identity: Some(format!("tcp:{address}")),
                owned_roots: vec![root.clone()],
                ..OwnershipLedger::default()
            };
            write_ledger(&manifest_path, &ledger)?;

            let stdout = std::fs::File::create(&stdout_path).map_err(|error| error.to_string())?;
            let stderr = std::fs::File::create(&stderr_path).map_err(|error| error.to_string())?;
            let restart = RestartRecipe {
                address,
                home: home.clone(),
                labby_home: labby_home.clone(),
                xdg_config: xdg_config.clone(),
                xdg_cache: xdg_cache.clone(),
                xdg_runtime: xdg_runtime.clone(),
                temp: temp.clone(),
                args: self.args.clone(),
                extra_env: self.extra_env.clone(),
            };
            // The ownership nonce is persisted by design, so it must never double as
            // a credential canary. The credential is independent and never serialized.
            let mut secret_canaries = vec![credential_canary.clone()];
            secret_canaries.extend(self.extra_env.iter().filter_map(|(key, value)| {
                let key = key.to_string_lossy().to_ascii_uppercase();
                (key.contains("CANARY") || key.contains("SECRET") || key.contains("TOKEN"))
                    .then(|| value.to_string_lossy().into_owned())
            }));
            let mut command = TokioCommand::new(env!("CARGO_BIN_EXE_labby"));
            command
                .env_clear()
                .args([
                    "serve",
                    "--host",
                    "127.0.0.1",
                    "--port",
                    &address.port().to_string(),
                ])
                .args(self.args)
                .env("HOME", &home)
                .env("LABBY_HOME", &labby_home)
                .env("LABBY_LOG_DIR", root.join("logs"))
                .env("XDG_CONFIG_HOME", &xdg_config)
                .env("XDG_CACHE_HOME", &xdg_cache)
                .env("XDG_RUNTIME_DIR", &xdg_runtime)
                .env("TMPDIR", &temp)
                .env("LABBY_AUTH_MODE", "bearer")
                .env("LABBY_MCP_HTTP_TOKEN", &credential_canary)
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .envs(self.extra_env)
                .stdin(Stdio::null())
                .stdout(stdout)
                .stderr(stderr)
                .kill_on_drop(true);
            configure_process_group(&mut command);
            let child = command.spawn().map_err(|error| error.to_string())?;
            #[cfg(windows)]
            let windows_job = child
                .id()
                .map(labby_winjob::JobObject::assign)
                .transpose()
                .map_err(|error| error.to_string())?;
            ledger.pid = child.id();
            ledger.process_start_identity = ledger.pid.map(process_start_identity);
            ledger.process_group = ledger.pid.and_then(|pid| i32::try_from(pid).ok());
            write_ledger(&manifest_path, &ledger)?;
            evidence.push(
                EvidenceKind::Process,
                format!("spawned pid {:?}", child.id()),
            );

            let descriptor = SanitizedConnectionDescriptor {
                run_id: identity.run_id.clone(),
                base_url: format!("http://{address}"),
                health_url: format!("http://{address}/health"),
                ready_url: format!("http://{address}{}", self.ready_path),
            };
            let mut guard = LiveLabbyGuard {
                root_guard,
                root,
                manifest_path,
                nonce_path,
                stdout_path,
                stderr_path,
                child: Some(child),
                ledger,
                identity,
                descriptor,
                evidence,
                restart,
                secret_canaries,
                credential_canary,
                revocations: Vec::new(),
                primary_failure: None,
                fail_evidence_writes: self.fail_evidence_writes,
                #[cfg(windows)]
                windows_job,
                finalized: false,
            };
            if let Err(error) = guard.wait_ready(self.readiness_deadline).await {
                guard.primary_failure = Some(error.clone());
                let diagnostics = guard.diagnostics(Some(&error));
                drop(guard.finish_with_deadline(Duration::from_secs(5)).await);
                if attempts > 1
                    && retry.port.is_none()
                    && (diagnostics.contains("Address already in use")
                        || diagnostics.contains("address already in use")
                        || diagnostics.contains("os error 48"))
                {
                    return retry.start_with_retries(attempts - 1).await;
                }
                return Err(diagnostics);
            }
            Ok(guard)
        })
    }
}

pub(crate) struct LiveLabbyGuard {
    root_guard: Option<tempfile::TempDir>,
    root: PathBuf,
    manifest_path: PathBuf,
    nonce_path: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    child: Option<Child>,
    ledger: OwnershipLedger,
    identity: RunIdentity,
    descriptor: SanitizedConnectionDescriptor,
    evidence: RunEvidence,
    restart: RestartRecipe,
    secret_canaries: Vec<String>,
    credential_canary: String,
    revocations: Vec<RevocationGuard>,
    primary_failure: Option<String>,
    fail_evidence_writes: bool,
    #[cfg(windows)]
    windows_job: Option<labby_winjob::JobObject>,
    finalized: bool,
}

#[derive(Clone)]
struct RestartRecipe {
    address: SocketAddr,
    home: PathBuf,
    labby_home: PathBuf,
    xdg_config: PathBuf,
    xdg_cache: PathBuf,
    xdg_runtime: PathBuf,
    temp: PathBuf,
    args: Vec<OsString>,
    extra_env: BTreeMap<OsString, OsString>,
}

impl LiveLabbyGuard {
    pub(crate) fn identity(&self) -> &RunIdentity {
        &self.identity
    }
    pub(crate) fn connection(&self) -> &SanitizedConnectionDescriptor {
        &self.descriptor
    }
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) async fn restart(&mut self) -> Result<(), String> {
        self.stop_process(Instant::now() + Duration::from_secs(5))
            .await?;
        let stdout = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.stdout_path)
            .map_err(|error| error.to_string())?;
        let stderr = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.stderr_path)
            .map_err(|error| error.to_string())?;
        let recipe = &self.restart;
        let mut command = TokioCommand::new(env!("CARGO_BIN_EXE_labby"));
        command
            .env_clear()
            .args([
                "serve",
                "--host",
                "127.0.0.1",
                "--port",
                &recipe.address.port().to_string(),
            ])
            .args(&recipe.args)
            .env("HOME", &recipe.home)
            .env("LABBY_HOME", &recipe.labby_home)
            .env("LABBY_LOG_DIR", self.root.join("logs"))
            .env("XDG_CONFIG_HOME", &recipe.xdg_config)
            .env("XDG_CACHE_HOME", &recipe.xdg_cache)
            .env("XDG_RUNTIME_DIR", &recipe.xdg_runtime)
            .env("TMPDIR", &recipe.temp)
            .env("LABBY_AUTH_MODE", "bearer")
            .env("LABBY_MCP_HTTP_TOKEN", &self.credential_canary)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .envs(recipe.extra_env.clone())
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .kill_on_drop(true);
        configure_process_group(&mut command);
        let child = command.spawn().map_err(|error| error.to_string())?;
        #[cfg(windows)]
        {
            self.windows_job = child
                .id()
                .map(labby_winjob::JobObject::assign)
                .transpose()
                .map_err(|error| error.to_string())?;
        }
        self.ledger.generation += 1;
        self.ledger.pid = child.id();
        self.ledger.process_start_identity = self.ledger.pid.map(process_start_identity);
        self.ledger.process_group = self.ledger.pid.and_then(|pid| i32::try_from(pid).ok());
        write_ledger(&self.manifest_path, &self.ledger)?;
        self.child = Some(child);
        self.evidence.push(EvidenceKind::Process, "restarted labby");
        self.wait_ready(DEFAULT_DEADLINE).await
    }

    pub(crate) async fn finish(mut self) -> CleanupResult {
        self.finish_inner(Duration::from_secs(10)).await
    }

    pub(crate) async fn finish_with_deadline(&mut self, deadline: Duration) -> CleanupResult {
        self.finish_inner(deadline).await
    }

    pub(crate) async fn run_with_timeout<F, T>(
        &mut self,
        timeout: Duration,
        future: F,
    ) -> Result<T, String>
    where
        F: Future<Output = T>,
    {
        match tokio::time::timeout(timeout, future).await {
            Ok(value) => Ok(value),
            Err(_) => {
                let cleanup = self.finish_inner(Duration::from_secs(5)).await;
                Err(format!(
                    "supervised case timed out; cleanup={:?}",
                    cleanup.failures
                ))
            }
        }
    }

    pub(crate) async fn finish_on_supported_signal(mut self) -> CleanupResult {
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("install SIGTERM handler");
            tokio::select! {
                result = tokio::signal::ctrl_c() => drop(result),
                _ = terminate.recv() => {}
            }
        }
        #[cfg(not(unix))]
        drop(tokio::signal::ctrl_c().await);
        self.finish_inner(Duration::from_secs(10)).await
    }

    pub(crate) fn register_credential_session(
        &mut self,
        session: impl Into<String>,
        revoke: impl FnMut() -> Result<(), String> + Send + 'static,
        is_absent: impl Fn() -> Result<bool, String> + Send + 'static,
    ) {
        self.ledger.credential_sessions.push(session.into());
        self.revocations.push(RevocationGuard {
            revoke: Box::new(revoke),
            is_absent: Box::new(is_absent),
        });
        drop(write_ledger(&self.manifest_path, &self.ledger));
    }

    async fn finish_inner(&mut self, timeout: Duration) -> CleanupResult {
        if self.finalized {
            return CleanupResult::default();
        }
        let deadline_exhausted = timeout.is_zero();
        let absolute = Instant::now() + timeout.max(Duration::from_secs(2));
        let mut result = CleanupResult {
            primary_failure: self.primary_failure.clone(),
            ..CleanupResult::default()
        };
        if deadline_exhausted {
            result.failures.push("cleanup deadline exhausted".into());
        }
        match self.stop_process(absolute).await {
            Ok(forced) => {
                result.forced = forced;
                result.graceful = !forced;
            }
            Err(error) => result.failures.push(error),
        }
        for lock in &self.ledger.locks {
            if lock.starts_with(&self.root) {
                drop(std::fs::remove_file(lock));
            } else {
                result
                    .failures
                    .push(format!("unsafe owned lock path: {}", lock.display()));
            }
        }
        for revoke in &mut self.revocations {
            if let Err(error) = (revoke.revoke)() {
                result
                    .failures
                    .push(format!("credential/session revocation failed: {error}"));
            } else {
                match (revoke.is_absent)() {
                    Ok(true) => {}
                    Ok(false) => result
                        .failures
                        .push("credential/session remained after revocation".into()),
                    Err(error) => result.failures.push(format!(
                        "credential/session absence verification failed: {error}"
                    )),
                }
            }
        }
        if self.revocations.len() != self.ledger.credential_sessions.len() {
            result
                .failures
                .push("credential/session ledger has no matching revocation guard".into());
        }
        self.revocations.clear();
        self.ledger.credential_sessions.clear();
        for path in [&self.stdout_path, &self.stderr_path] {
            if let Err(error) = cap_log_file(path) {
                result
                    .failures
                    .push(format!("log retention failed: {error}"));
            }
            let bytes = std::fs::read(path).unwrap_or_default();
            for canary in &self.secret_canaries {
                if !canary.is_empty()
                    && bytes
                        .windows(canary.len())
                        .any(|window| window == canary.as_bytes())
                {
                    result
                        .failures
                        .push(format!("secret canary appeared in {}", path.display()));
                }
            }
        }
        cap_log_tree(&self.root.join("logs"), &mut result.failures);
        scan_artifact_tree(&self.root, &self.secret_canaries, &mut result.failures);
        if TcpListener::bind(self.ledger.listener.expect("listener recorded")).is_err() {
            result
                .failures
                .push("owned listener remains bound".to_string());
        }
        self.evidence.push(
            EvidenceKind::Cleanup,
            format!("cleanup failures={}", result.failures.len()),
        );
        let retained = std::env::temp_dir()
            .join("labby-live-e2e-evidence")
            .join(format!("{}.json", self.identity.run_id));
        let evidence_result = if self.fail_evidence_writes {
            Err(std::io::Error::other("injected evidence disk failure"))
        } else {
            self.evidence.write_atomic(&retained)
        };
        if let Err(error) = evidence_result {
            eprintln!(
                "labby-e2e evidence fallback run={} error={error}",
                self.identity.run_id
            );
            result
                .failures
                .push(format!("evidence write failed: {error}"));
            result.retention_failure = Some(error.to_string());
        } else {
            scan_file_for_canaries(&retained, &self.secret_canaries, &mut result.failures);
        }
        self.finalized = true;
        let owns_root = self.root_guard.is_some();
        if let Some(root_guard) = self.root_guard.take()
            && let Err(error) = root_guard.close()
        {
            result
                .failures
                .push(format!("owned root deletion failed: {error}"));
        }
        if owns_root && self.root.exists() {
            result.failures.push(format!(
                "owned root retained after cleanup: {}",
                self.root.display()
            ));
        }
        result
    }

    async fn wait_ready(&mut self, deadline: Duration) -> Result<(), String> {
        // The workspace deliberately builds reqwest with rustls-no-provider so
        // each executable/test binary chooses its provider explicitly.
        drop(rustls::crypto::ring::default_provider().install_default());
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .map_err(|e| e.to_string())?;
        let expires = Instant::now() + deadline;
        loop {
            if let Some(status) = self
                .child
                .as_mut()
                .and_then(|child| child.try_wait().ok())
                .flatten()
            {
                return Err(format!("labby exited before readiness: {status}"));
            }
            let health = client.get(&self.descriptor.health_url).send().await;
            let ready = client.get(&self.descriptor.ready_url).send().await;
            self.evidence.push(
                EvidenceKind::Readiness,
                format!(
                    "health={} ready={}",
                    health.as_ref().map(|r| r.status().as_u16()).unwrap_or(0),
                    ready.as_ref().map(|r| r.status().as_u16()).unwrap_or(0)
                ),
            );
            if health
                .as_ref()
                .is_ok_and(|response| response.status().is_success())
                && ready
                    .as_ref()
                    .is_ok_and(|response| response.status().is_success())
            {
                self.evidence
                    .push(EvidenceKind::Readiness, "health and ready succeeded");
                return Ok(());
            }
            if Instant::now() >= expires {
                return Err("readiness deadline exceeded".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn stop_process(&mut self, deadline: Instant) -> Result<bool, String> {
        self.validate_ownership()?;
        let Some(mut child) = self.child.take() else {
            return Ok(false);
        };
        #[cfg(unix)]
        if let Some(group) = self.ledger.process_group {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(group),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            if let Some(job) = self.windows_job.take() {
                job.close().map_err(|error| error.to_string())?;
            }
            #[cfg(not(windows))]
            drop(child.start_kill());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let graceful_window = (remaining / 2).min(Duration::from_secs(2));
        let mut forced = false;
        match tokio::time::timeout(graceful_window, child.wait()).await {
            Ok(Ok(status)) => self
                .evidence
                .push(EvidenceKind::Process, format!("exit status={status}")),
            Ok(Err(error)) => return Err(error.to_string()),
            Err(_) => {
                forced = true;
                #[cfg(unix)]
                if let Some(group) = self.ledger.process_group {
                    let _ = nix::sys::signal::killpg(
                        nix::unistd::Pid::from_raw(group),
                        nix::sys::signal::Signal::SIGKILL,
                    );
                }
                drop(child.start_kill());
                let remaining = deadline.saturating_duration_since(Instant::now());
                let status = tokio::time::timeout(remaining, child.wait())
                    .await
                    .map_err(|_| "cleanup deadline exhausted".to_string())?
                    .map_err(|e| e.to_string())?;
                self.evidence.push(
                    EvidenceKind::Process,
                    format!("forced exit status={status}"),
                );
            }
        }
        #[cfg(unix)]
        if let Some(group) = self.ledger.process_group {
            let mut members = process_group_members(group);
            while !members.is_empty() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
                members = process_group_members(group);
            }
            if !members.is_empty() {
                return Err(format!(
                    "owned process group leaked descendants: {members:?}"
                ));
            }
        }
        Ok(forced)
    }

    fn validate_ownership(&self) -> Result<(), String> {
        let root_metadata =
            std::fs::symlink_metadata(&self.root).map_err(|error| error.to_string())?;
        if root_metadata.file_type().is_symlink() {
            return Err("owned root was replaced by a symlink".into());
        }
        let nonce_metadata =
            std::fs::symlink_metadata(&self.nonce_path).map_err(|error| error.to_string())?;
        if nonce_metadata.file_type().is_symlink() {
            return Err("ownership nonce was replaced by a symlink".into());
        }
        let nonce = std::fs::read_to_string(&self.nonce_path).map_err(|error| error.to_string())?;
        if nonce != self.identity.nonce {
            return Err("ownership nonce mismatch".into());
        }
        let bytes = std::fs::read(&self.manifest_path).map_err(|error| error.to_string())?;
        let persisted: OwnershipLedger = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid ownership manifest: {error}"))?;
        if persisted.nonce != self.ledger.nonce
            || persisted.generation != self.ledger.generation
            || persisted.created_at_ms != self.ledger.created_at_ms
            || persisted.root != self.ledger.root
            || persisted.pid != self.ledger.pid
            || persisted.process_start_identity != self.ledger.process_start_identity
            || persisted.listener_identity != self.ledger.listener_identity
            || persisted.owned_roots != self.ledger.owned_roots
        {
            return Err("foreign or stale ownership manifest".into());
        }
        if persisted
            .nonce
            .chars()
            .any(|character| character.is_control())
            || persisted.owned_roots.iter().any(|path| {
                path.as_os_str()
                    .to_string_lossy()
                    .chars()
                    .any(|c| c.is_control())
            })
        {
            return Err("ownership manifest contains control characters".into());
        }
        if persisted.owned_roots != [self.root.clone()]
            || persisted.listener_identity.as_deref()
                != persisted
                    .listener
                    .map(|address| format!("tcp:{address}"))
                    .as_deref()
        {
            return Err("unsafe ownership manifest identity".into());
        }
        if let (Some(pid), Some(expected)) = (self.ledger.pid, &self.ledger.process_start_identity)
            && process_start_identity(pid) != *expected
        {
            return Err("owned PID start identity changed".into());
        }
        Ok(())
    }

    pub(crate) fn diagnostics(&self, primary: Option<&str>) -> String {
        let readiness_history = self
            .evidence
            .events
            .iter()
            .filter(|event| event.kind == EvidenceKind::Readiness)
            .map(|event| event.message.as_str())
            .collect::<Vec<_>>();
        let process_inventory = self
            .ledger
            .process_group
            .map(process_group_inventory)
            .unwrap_or_default();
        format!(
            "run={} command={} version={} binary_sha256={} address={} primary={} stdout_tail={} stderr_tail={} health_ready_history={:?} process_inventory={:?} process_pid={:?} process_group={:?} generation={}",
            self.identity.run_id,
            env!("CARGO_BIN_EXE_labby"),
            self.identity.binary_version,
            self.identity.binary_sha256,
            self.descriptor.base_url,
            primary.unwrap_or("none"),
            tail(&self.stdout_path),
            tail(&self.stderr_path),
            readiness_history,
            process_inventory,
            self.ledger.pid,
            self.ledger.process_group,
            self.ledger.generation,
        )
    }
}

impl Drop for LiveLabbyGuard {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        self.evidence
            .push(EvidenceKind::Failure, "guard dropped without finish");
        let deadline = Instant::now() + DROP_DEADLINE;
        #[cfg(unix)]
        if let Some(group) = self.ledger.process_group {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(group),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        #[cfg(windows)]
        if let Some(job) = self.windows_job.take() {
            let _ = job.close();
        }
        if let Some(child) = self.child.as_mut() {
            drop(child.start_kill());
            while Instant::now() < deadline {
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        for revoke in &mut self.revocations {
            if let Err(error) = (revoke.revoke)() {
                self.evidence.push(
                    EvidenceKind::Failure,
                    format!("drop revocation failed: {error}"),
                );
            } else if !matches!((revoke.is_absent)(), Ok(true)) {
                self.evidence.push(
                    EvidenceKind::Failure,
                    "drop revocation absence verification failed",
                );
            }
        }
        #[cfg(unix)]
        if let Some(group) = self.ledger.process_group {
            while !process_group_members(group).is_empty() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let leaked = process_group_members(group);
            if !leaked.is_empty() {
                self.evidence.push(
                    EvidenceKind::Failure,
                    format!("drop finalization missed descendants: {leaked:?}"),
                );
            }
        }
        let mut artifact_scan_failures = Vec::new();
        scan_artifact_tree(
            &self.root,
            &self.secret_canaries,
            &mut artifact_scan_failures,
        );
        for failure in artifact_scan_failures {
            self.evidence.push(EvidenceKind::Failure, failure);
        }
        let retained = std::env::temp_dir()
            .join("labby-live-e2e-evidence")
            .join(format!("{}.json", self.identity.run_id));
        if let Err(error) = self.evidence.write_atomic(&retained) {
            eprintln!(
                "labby-e2e drop evidence fallback run={} error={error}",
                self.identity.run_id
            );
        } else {
            let mut scan_failures = Vec::new();
            scan_file_for_canaries(&retained, &self.secret_canaries, &mut scan_failures);
            for failure in scan_failures {
                eprintln!(
                    "labby-e2e drop evidence secret scan run={} failure={failure}",
                    self.identity.run_id
                );
            }
        }
        if let Some(root_guard) = self.root_guard.take() {
            drop(root_guard.close());
        }
    }
}

pub(crate) fn isolated_command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .env_clear()
        .env("HOME", home)
        .env("LABBY_HOME", home.join(".labby"))
        .env("LABBY_LOG_DIR", home.join("logs"))
        .env("TMPDIR", home.join("tmp"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default());
    command
}

pub(crate) fn sweep_stale_runs() -> Vec<String> {
    let parent = std::env::temp_dir().join("labby-live-e2e");
    let Ok(parent) = parent.canonicalize() else {
        return Vec::new();
    };
    let mut failures = Vec::new();
    let Ok(entries) = std::fs::read_dir(&parent) else {
        return failures;
    };
    for entry in entries.flatten() {
        let root = entry.path();
        let result = (|| -> Result<(), String> {
            if std::fs::symlink_metadata(&root)
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
            {
                return Err("stale candidate root is a symlink".into());
            }
            let root = root.canonicalize().map_err(|e| e.to_string())?;
            if !root.starts_with(&parent) {
                return Err("stale candidate escaped parent".into());
            }
            let ledger: OwnershipLedger = serde_json::from_slice(
                &std::fs::read(root.join("ownership.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let nonce =
                std::fs::read_to_string(root.join("ownership.nonce")).map_err(|e| e.to_string())?;
            if ledger.root != root || ledger.nonce != nonce || nonce.chars().any(|c| c.is_control())
            {
                return Err("stale candidate ownership mismatch".into());
            }
            if unix_timestamp_ms().saturating_sub(ledger.created_at_ms) < 300_000 {
                return Ok(());
            }
            let Some(pid) = ledger.pid else {
                // A manifest without a spawned PID may be another test currently
                // between allocation and spawn, so it is never sweepable.
                return Ok(());
            };
            if pid_is_alive(pid) {
                return Ok(());
            }
            std::fs::remove_dir_all(&root).map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            failures.push(format!("{}: {error}", root.display()));
        }
    }
    failures
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    i32::try_from(pid)
        .ok()
        .is_some_and(|pid| nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok())
}

#[cfg(windows)]
fn pid_is_alive(pid: u32) -> bool {
    labby_winjob::pid_is_alive(pid)
}

#[cfg(not(any(unix, windows)))]
fn pid_is_alive(_pid: u32) -> bool {
    false
}

fn build_identity() -> Result<RunIdentity, String> {
    let mut nonce = [0_u8; 24];
    getrandom::fill(&mut nonce).map_err(|error| error.to_string())?;
    let nonce = hex::encode(nonce);
    let run_id = ulid::Ulid::new().to_string();
    let seed = std::env::var("LABBY_E2E_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| u64::from_le_bytes(run_id.as_bytes()[..8].try_into().unwrap()));
    let binary = std::fs::read(env!("CARGO_BIN_EXE_labby")).map_err(|error| error.to_string())?;
    let binary_sha256 = hex::encode(Sha256::digest(binary));
    let binary_version = Command::new(env!("CARGO_BIN_EXE_labby"))
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let git_sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let git_dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .is_ok_and(|o| !o.stdout.is_empty());
    Ok(RunIdentity {
        run_id,
        seed,
        nonce,
        git_sha,
        git_dirty,
        binary_sha256,
        binary_version,
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        features: enabled_features(),
        ui_asset_sha256: "not-built".to_string(),
        fixture_versions: vec!["live-harness-fixture:v1".to_string()],
    })
}

fn unix_timestamp_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn random_secret_canary() -> Result<String, String> {
    let mut bytes = [0_u8; 24];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    Ok(format!("labby-e2e-secret-{}", hex::encode(bytes)))
}

fn enabled_features() -> Vec<String> {
    [
        ("gateway", cfg!(feature = "gateway")),
        ("fs", cfg!(feature = "fs")),
        ("skills", cfg!(feature = "skills")),
        ("lab-admin", cfg!(feature = "lab-admin")),
        ("api-docs", cfg!(feature = "api-docs")),
        ("systemd", cfg!(feature = "systemd")),
    ]
    .into_iter()
    .filter(|(_, enabled)| *enabled)
    .map(|(name, _)| name.to_string())
    .collect()
}

fn canonical_owned_root(root: &Path, parent: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("owned root must not be a symlink".into());
    }
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let parent = parent.canonicalize().map_err(|error| error.to_string())?;
    if !root.starts_with(&parent) {
        return Err("owned root escaped allocated parent".into());
    }
    Ok(root)
}

fn write_nonce(path: &Path, nonce: &str) -> Result<(), String> {
    std::fs::write(path, nonce).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn write_ledger(path: &Path, ledger: &OwnershipLedger) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(ledger).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}

fn tail(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    let start = bytes.len().saturating_sub(LOG_TAIL_BYTES);
    sanitize(&String::from_utf8_lossy(&bytes[start..]))
}

fn cap_log_file(path: &Path) -> std::io::Result<()> {
    let bytes = std::fs::read(path)?;
    if bytes.len() > LOG_TAIL_BYTES {
        let temporary = path.with_extension("rotating.tmp");
        std::fs::write(&temporary, &bytes[bytes.len() - LOG_TAIL_BYTES..])?;
        std::fs::rename(temporary, path)?;
    }
    Ok(())
}

fn scan_file_for_canaries(path: &Path, canaries: &[String], failures: &mut Vec<String>) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    for canary in canaries {
        if !canary.is_empty()
            && bytes
                .windows(canary.len())
                .any(|window| window == canary.as_bytes())
        {
            failures.push(format!("secret canary appeared in {}", path.display()));
        }
    }
}

fn scan_artifact_tree(root: &Path, canaries: &[String], failures: &mut Vec<String>) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&path) {
                pending.extend(entries.flatten().map(|entry| entry.path()));
            }
        } else if metadata.is_file() {
            scan_file_for_canaries(&path, canaries, failures);
        }
    }
}

fn cap_log_tree(root: &Path, failures: &mut Vec<String>) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&path) {
                pending.extend(entries.flatten().map(|entry| entry.path()));
            }
        } else if metadata.is_file()
            && let Err(error) = cap_log_file(&path)
        {
            failures.push(format!(
                "log retention failed for {}: {error}",
                path.display()
            ));
        }
    }
}

fn process_start_identity(pid: u32) -> String {
    Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            format!(
                "pid:{pid}:{}",
                String::from_utf8_lossy(&output.stdout).trim()
            )
        })
        .unwrap_or_else(|| format!("pid:{pid}:unknown"))
}

#[cfg(unix)]
fn process_group_members(group: i32) -> Vec<u32> {
    Command::new("pgrep")
        .args(["-g", &group.to_string()])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(unix)]
fn process_group_inventory(group: i32) -> Vec<String> {
    process_group_members(group)
        .into_iter()
        .map(|pid| process_start_identity(pid))
        .collect()
}

#[cfg(not(unix))]
fn process_group_inventory(_group: i32) -> Vec<String> {
    Vec::new()
}

#[cfg(unix)]
fn configure_process_group(command: &mut TokioCommand) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut TokioCommand) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[test]
    fn isolated_children_do_not_inherit_cloud_git_ssh_proxy_or_provider_state() {
        let temp = tempfile::tempdir().unwrap();
        let command = isolated_command(temp.path());
        let explicit = command
            .get_envs()
            .filter_map(|(key, value)| value.map(|_| key.to_string_lossy().into_owned()))
            .collect::<BTreeSet<_>>();
        for forbidden in [
            "AWS_ACCESS_KEY_ID",
            "AZURE_CLIENT_SECRET",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "SSH_AUTH_SOCK",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
        ] {
            assert!(
                !explicit.contains(forbidden),
                "ambient {forbidden} was inherited"
            );
        }
        assert_eq!(
            explicit,
            BTreeSet::from([
                "HOME".into(),
                "LABBY_HOME".into(),
                "LABBY_LOG_DIR".into(),
                "PATH".into(),
                "TMPDIR".into(),
            ])
        );
    }

    #[tokio::test]
    async fn ownership_validation_rejects_nonce_partial_stale_and_pid_reuse_simulations() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let original_nonce = std::fs::read(&guard.nonce_path).unwrap();
        let original_manifest = std::fs::read(&guard.manifest_path).unwrap();
        let original_ledger = guard.ledger.clone();

        std::fs::write(&guard.nonce_path, "foreign-nonce").unwrap();
        assert!(
            guard
                .validate_ownership()
                .unwrap_err()
                .contains("nonce mismatch")
        );
        std::fs::write(&guard.nonce_path, &original_nonce).unwrap();

        std::fs::write(&guard.manifest_path, b"{\"partial\":").unwrap();
        assert!(
            guard
                .validate_ownership()
                .unwrap_err()
                .contains("invalid ownership manifest")
        );
        std::fs::write(&guard.manifest_path, &original_manifest).unwrap();

        let mut stale = original_ledger.clone();
        stale.generation += 1;
        write_ledger(&guard.manifest_path, &stale).unwrap();
        assert!(
            guard
                .validate_ownership()
                .unwrap_err()
                .contains("stale ownership")
        );
        std::fs::write(&guard.manifest_path, &original_manifest).unwrap();

        guard.ledger.process_start_identity = Some("pid-reuse-simulation".into());
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        assert!(
            guard
                .validate_ownership()
                .unwrap_err()
                .contains("start identity")
        );
        guard.ledger = original_ledger;
        std::fs::write(&guard.manifest_path, &original_manifest).unwrap();

        let cleanup = guard.finish().await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
    }

    #[tokio::test]
    async fn stale_owned_lock_is_removed_and_verified_during_cleanup() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let lock = guard.root.join("stale-owned.lock");
        std::fs::write(&lock, "owned").unwrap();
        guard.ledger.locks.push(lock.clone());
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        let cleanup = guard.finish().await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
        assert!(!lock.exists());
    }

    #[tokio::test]
    async fn credential_sessions_are_revoked_instead_of_only_forgotten() {
        let revoked = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&revoked);
        let verified = Arc::clone(&revoked);
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        guard.register_credential_session(
            "synthetic-session",
            move || {
                observed.store(true, Ordering::SeqCst);
                Ok(())
            },
            move || Ok(verified.load(Ordering::SeqCst)),
        );
        let cleanup = guard.finish().await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
        assert!(
            revoked.load(Ordering::SeqCst),
            "revocation guard was not invoked"
        );
    }

    #[tokio::test]
    async fn exact_artifact_and_retained_evidence_bytes_are_canary_scanned() {
        let artifact_guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let canary = artifact_guard.secret_canaries[0].clone();
        std::fs::create_dir_all(artifact_guard.root.join("logs")).unwrap();
        std::fs::write(artifact_guard.root.join("logs/leaked.log"), &canary).unwrap();
        let artifact_cleanup = artifact_guard.finish().await;
        assert!(
            artifact_cleanup
                .failures
                .iter()
                .any(|failure| failure.contains("secret canary appeared"))
        );

        let mut evidence_guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let canary = evidence_guard.secret_canaries[0].clone();
        evidence_guard.evidence.push(EvidenceKind::Failure, &canary);
        let evidence_cleanup = evidence_guard.finish().await;
        assert!(
            evidence_cleanup
                .failures
                .iter()
                .any(|failure| failure.contains("secret canary appeared"))
        );
    }

    #[tokio::test]
    async fn ownership_nonce_and_bearer_never_escape_operator_safe_outputs() {
        let guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let nonce = guard.identity.nonce.clone();
        let bearer = guard.credential_canary.clone();
        assert!(guard.validate_ownership().is_ok());
        let ledger_bytes = std::fs::read(&guard.manifest_path).unwrap();
        assert!(
            ledger_bytes
                .windows(nonce.len())
                .any(|bytes| bytes == nonce.as_bytes())
        );

        let diagnostics = guard.diagnostics(None);
        assert!(!diagnostics.contains(&nonce));
        assert!(!diagnostics.contains(&bearer));
        let mut artifact_scan = Vec::new();
        scan_artifact_tree(&guard.root, &guard.secret_canaries, &mut artifact_scan);
        let rendered_scan = artifact_scan.join("\n");
        assert!(!rendered_scan.contains(&nonce));
        assert!(!rendered_scan.contains(&bearer));

        let retained = std::env::temp_dir()
            .join("labby-live-e2e-evidence")
            .join(format!("{}.json", guard.identity.run_id));
        let cleanup = guard.finish().await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
        let retained_bytes = std::fs::read(retained).unwrap();
        assert!(
            !retained_bytes
                .windows(nonce.len())
                .any(|bytes| bytes == nonce.as_bytes())
        );
        assert!(
            !retained_bytes
                .windows(bearer.len())
                .any(|bytes| bytes == bearer.as_bytes())
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retained_owned_root_is_an_explicit_cleanup_failure() {
        use std::os::unix::fs::PermissionsExt as _;

        let guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let root = guard.root.clone();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
        let cleanup = guard.finish().await;
        assert!(!cleanup.is_clean());
        assert!(
            cleanup
                .failures
                .iter()
                .any(|failure| failure.contains("owned root deletion failed")
                    || failure.contains("owned root retained"))
        );
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ownership_validation_rejects_nonce_symlink_swap() {
        use std::os::unix::fs::symlink;

        let guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let backup = guard.root.join("nonce.backup");
        std::fs::rename(&guard.nonce_path, &backup).unwrap();
        symlink(&backup, &guard.nonce_path).unwrap();
        assert!(guard.validate_ownership().unwrap_err().contains("symlink"));
        std::fs::remove_file(&guard.nonce_path).unwrap();
        std::fs::rename(backup, &guard.nonce_path).unwrap();
        let cleanup = guard.finish().await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ownership_validation_rejects_root_symlink_and_control_character_manifest() {
        use std::os::unix::fs::symlink;
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let moved = guard.root.with_extension("moved");
        std::fs::rename(&guard.root, &moved).unwrap();
        symlink(&moved, &guard.root).unwrap();
        assert!(guard.validate_ownership().unwrap_err().contains("symlink"));
        std::fs::remove_file(&guard.root).unwrap();
        std::fs::rename(&moved, &guard.root).unwrap();

        let original = guard.ledger.clone();
        guard
            .ledger
            .owned_roots
            .push(guard.root.join("unsafe\npath"));
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        assert!(
            guard
                .validate_ownership()
                .unwrap_err()
                .contains("control characters")
        );
        guard.ledger = original;
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        assert!(guard.finish().await.is_clean());
    }

    #[test]
    fn stale_sweep_removes_only_nonce_matched_dead_runs() {
        let parent = std::env::temp_dir().join("labby-live-e2e");
        std::fs::create_dir_all(&parent).unwrap();
        let stale = tempfile::Builder::new()
            .prefix("run-")
            .tempdir_in(&parent)
            .unwrap();
        let root = stale.keep();
        let nonce = "stale-owned-nonce";
        std::fs::write(root.join("ownership.nonce"), nonce).unwrap();
        write_ledger(
            &root.join("ownership.json"),
            &OwnershipLedger {
                generation: 1,
                created_at_ms: 0,
                nonce: nonce.into(),
                root: root.canonicalize().unwrap(),
                pid: Some(u32::MAX),
                owned_roots: vec![root.canonicalize().unwrap()],
                ..OwnershipLedger::default()
            },
        )
        .unwrap();
        let failures = sweep_stale_runs();
        assert!(
            failures
                .iter()
                .all(|failure| !failure.starts_with(&root.display().to_string()))
        );
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn exhausted_cleanup_deadline_is_a_case_failure_but_still_kills_the_child() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let cleanup = guard.finish_with_deadline(Duration::ZERO).await;
        assert!(!cleanup.is_clean(), "zero deadline must not silently pass");
    }

    #[tokio::test]
    #[allow(clippy::panic)]
    async fn outer_wrapper_cleans_on_timeout_and_drop_covers_panic_unwind() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let timeout = guard
            .run_with_timeout(Duration::from_millis(10), std::future::pending::<()>())
            .await
            .expect_err("pending case times out");
        assert!(timeout.contains("timed out"));

        let guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        let root = guard.root.clone();
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _owned = guard;
            panic!("injected case panic");
        }));
        assert!(unwind.is_err());
        assert!(!root.exists(), "panic Drop leaked owned root");

        let signal_contract = LiveLabbyGuard::finish_on_supported_signal;
        std::hint::black_box(signal_contract);
    }

    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::panic)]
    async fn panic_drop_reaps_forked_grandchild_and_held_listener() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        guard
            .stop_process(Instant::now() + Duration::from_secs(5))
            .await
            .unwrap();
        let marker = guard.root.join("panic-grandchild.marker");
        let mut command = TokioCommand::new(env!("CARGO_BIN_EXE_live-harness-fixture"));
        command
            .env_clear()
            .args(["grandchild-listener", "0"])
            .arg(&marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        configure_process_group(&mut command);
        let child = command.spawn().unwrap();
        guard.ledger.pid = child.id();
        guard.ledger.process_start_identity = guard.ledger.pid.map(process_start_identity);
        guard.ledger.process_group = guard.ledger.pid.and_then(|pid| i32::try_from(pid).ok());
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        guard.child = Some(child);
        let address = wait_for_fixture_listener(&marker, Duration::from_secs(3)).await;
        guard.ledger.listener = Some(address);
        guard.ledger.listener_identity = Some(format!("tcp:{address}"));
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        let root = guard.root.clone();
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = guard;
            panic!("panic with grandchild");
        }));
        assert!(unwind.is_err());
        assert!(!root.exists());
        assert!(
            TcpListener::bind(address).is_ok(),
            "grandchild retained listener"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ignored_termination_forces_owned_child_shutdown() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        guard
            .stop_process(Instant::now() + Duration::from_secs(5))
            .await
            .unwrap();
        let mut command = TokioCommand::new("/bin/sh");
        command
            .env_clear()
            .args(["-c", "trap '' TERM; while :; do sleep 1; done"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        configure_process_group(&mut command);
        let child = command.spawn().unwrap();
        guard.ledger.pid = child.id();
        guard.ledger.process_start_identity = guard.ledger.pid.map(process_start_identity);
        guard.ledger.process_group = guard.ledger.pid.and_then(|pid| i32::try_from(pid).ok());
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        guard.child = Some(child);
        let started = Instant::now();
        let cleanup = guard.finish_with_deadline(Duration::from_millis(250)).await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
        assert!(cleanup.forced, "ignored SIGTERM must use forced shutdown");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn forked_grandchild_and_held_listener_are_reaped_with_the_owned_group() {
        let mut guard = LiveLabbyBuilder::new().start().await.expect("live labby");
        guard
            .stop_process(Instant::now() + Duration::from_secs(5))
            .await
            .unwrap();
        let marker = guard.root.join("grandchild.marker");
        let mut command = TokioCommand::new(env!("CARGO_BIN_EXE_live-harness-fixture"));
        command
            .env_clear()
            .args(["grandchild-listener", "0"])
            .arg(&marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        configure_process_group(&mut command);
        let child = command.spawn().unwrap();
        guard.ledger.pid = child.id();
        guard.ledger.process_start_identity = guard.ledger.pid.map(process_start_identity);
        guard.ledger.process_group = guard.ledger.pid.and_then(|pid| i32::try_from(pid).ok());
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        guard.child = Some(child);
        let address = wait_for_fixture_listener(&marker, Duration::from_secs(3)).await;
        guard.ledger.listener = Some(address);
        guard.ledger.listener_identity = Some(format!("tcp:{address}"));
        write_ledger(&guard.manifest_path, &guard.ledger).unwrap();
        assert!(
            TcpListener::bind(address).is_err(),
            "fixture must hold listener"
        );
        let cleanup = guard.finish_with_deadline(Duration::from_secs(3)).await;
        assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
        assert!(
            TcpListener::bind(address).is_ok(),
            "grandchild listener leaked"
        );
    }

    #[cfg(unix)]
    async fn wait_for_fixture_listener(marker: &Path, timeout: Duration) -> SocketAddr {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(value) = std::fs::read_to_string(marker) {
                let fields = value.split_whitespace().collect::<Vec<_>>();
                if fields.len() == 3
                    && fields[2] == "ready"
                    && let Ok(port) = fields[1].parse::<u16>()
                {
                    return SocketAddr::from(([127, 0, 0, 1], port));
                }
            }
            assert!(
                Instant::now() < deadline,
                "grandchild listener did not become ready"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
