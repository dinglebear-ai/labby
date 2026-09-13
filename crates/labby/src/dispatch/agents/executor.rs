//! Bounded subprocess adapter for operator-approved Agent harnesses.

use std::process::Stdio;
use std::time::Duration;

use labby_primitives::agent::AgentDefinition;
use labby_runtime::{
    agent_runtime::{
        AgentExecutionOutput, AgentExecutionRequest, AgentExecutor, AgentRuntimeError,
        ExecutionGuard, system_now_millis,
    },
    authority::AuthoritySafeBoundary,
};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

use crate::config::AgentHarnessConfig;

const CANCELLATION_POLL: Duration = Duration::from_millis(100);
const REVOCATION_POLL: Duration = Duration::from_secs(1);
const TERMINATION_GRACE: Duration = Duration::from_millis(250);

pub(crate) enum AgentExecutorBackend {
    Process(Box<ProcessAgentExecutor>),
    #[cfg(feature = "proxy-testkit")]
    Deterministic,
}

impl AgentExecutor for AgentExecutorBackend {
    async fn execute(
        &self,
        request: AgentExecutionRequest,
        guard: ExecutionGuard<'_>,
    ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
        match self {
            Self::Process(executor) => executor.execute(request, guard).await,
            #[cfg(feature = "proxy-testkit")]
            Self::Deterministic => {
                const OUTPUT: &[u8] = b"deterministic agent output\n";
                guard
                    .check(
                        AuthoritySafeBoundary::BeforeExternalEffect,
                        system_now_millis(),
                    )
                    .await?;
                request.transcript.append(OUTPUT);
                Ok(AgentExecutionOutput {
                    digest: format!("sha256:{}", hex::encode(Sha256::digest(OUTPUT))),
                    bytes: OUTPUT.len(),
                    external_effects: 0,
                })
            }
        }
    }
}

pub(crate) fn resolve_executor(
    definition: &AgentDefinition,
) -> Result<AgentExecutorBackend, AgentRuntimeError> {
    #[cfg(feature = "proxy-testkit")]
    if std::env::var_os("LABBY_E2E_DETERMINISTIC_EXECUTORS").is_some() {
        return Ok(AgentExecutorBackend::Deterministic);
    }

    crate::config::resolved_agent_harnesses()
        .into_iter()
        .find(|harness| harness.matches_definition(definition))
        .map(ProcessAgentExecutor::new)
        .map(Box::new)
        .map(AgentExecutorBackend::Process)
        .ok_or(AgentRuntimeError::ExecutorUnavailable)
}

pub(crate) fn configured_harnesses() -> Vec<serde_json::Value> {
    crate::config::resolved_agent_harnesses()
        .into_iter()
        .map(|harness| {
            let available = executable_available(&harness.command)
                && harness
                    .cwd
                    .as_ref()
                    .is_none_or(|working_directory| working_directory.is_dir());
            serde_json::json!({
                "id": harness.id,
                "digest": harness.digest(),
                "available": available,
                "content_digest": harness.content_digest,
                "repository_digest": harness.repository_digest,
                "image_digest": harness.image_digest,
                "loadout_digest": harness.loadout_digest,
                "catalog_generation": harness.catalog_generation,
            })
        })
        .collect()
}

pub(crate) fn configured_harness_id(definition: &AgentDefinition) -> Option<String> {
    crate::config::resolved_agent_harnesses()
        .into_iter()
        .find(|harness| harness.matches_definition(definition))
        .map(|harness| harness.id)
}

pub(crate) struct ProcessAgentExecutor {
    harness: AgentHarnessConfig,
}

impl ProcessAgentExecutor {
    fn new(harness: AgentHarnessConfig) -> Self {
        Self { harness }
    }

    async fn run(
        &self,
        request: AgentExecutionRequest,
        guard: ExecutionGuard<'_>,
    ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
        if !executable_available(&self.harness.command)
            || self
                .harness
                .cwd
                .as_ref()
                .is_some_and(|working_directory| !working_directory.is_dir())
        {
            return Err(AgentRuntimeError::ExecutorUnavailable);
        }
        guard
            .check(
                AuthoritySafeBoundary::BeforeExternalEffect,
                system_now_millis(),
            )
            .await?;

        let mut command = Command::new(&self.harness.command);
        command
            .args(&self.harness.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &self.harness.cwd {
            command.current_dir(cwd);
        }
        for name in baseline_environment_names()
            .into_iter()
            .chain(self.harness.inherit_env.iter().map(String::as_str))
        {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command
            .spawn()
            .map_err(|_| AgentRuntimeError::ExecutorUnavailable)?;
        let mut process_tree = ProcessTreeGuard::attach(&child)?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or(AgentRuntimeError::ExecutorFailed)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(AgentRuntimeError::ExecutorFailed)?;
        let stderr = child
            .stderr
            .take()
            .ok_or(AgentRuntimeError::ExecutorFailed)?;

        let input = request.input.into_bytes();
        let stdin_task = tokio::spawn(async move {
            stdin.write_all(&input).await?;
            stdin.shutdown().await
        });
        let transcript = request.transcript;
        let stdout_task = tokio::spawn(read_stdout(stdout, transcript));
        let stderr_task = tokio::spawn(drain_output(stderr));

        let status = wait_for_child(&mut child, &guard).await;
        if status.is_err() {
            terminate_process_tree(&mut child, &mut process_tree).await;
        } else {
            // A harness may leave grandchildren alive after its leader exits.
            // Closing the tree guard before joining pipe readers reaps them and
            // guarantees inherited stdout/stderr descriptors eventually close.
            process_tree.terminate_now();
        }
        let stdin_result = stdin_task
            .await
            .map_err(|_| AgentRuntimeError::ExecutorFailed)?;
        let (digest, stdout_bytes) = stdout_task
            .await
            .map_err(|_| AgentRuntimeError::ExecutorFailed)?
            .map_err(|_| AgentRuntimeError::ExecutorFailed)?;
        let stderr_bytes = stderr_task
            .await
            .map_err(|_| AgentRuntimeError::ExecutorFailed)?
            .map_err(|_| AgentRuntimeError::ExecutorFailed)?;
        let status = status?;
        if stdin_result.is_err() || !status.success() {
            return Err(AgentRuntimeError::ExecutorFailed);
        }
        Ok(AgentExecutionOutput {
            digest,
            bytes: stdout_bytes.saturating_add(stderr_bytes),
            // Launching the configured harness is the one external effect this
            // adapter can count. Effects inside the harness remain governed by
            // the harness's own sandbox/permission policy.
            external_effects: 1,
        })
    }
}

impl AgentExecutor for ProcessAgentExecutor {
    async fn execute(
        &self,
        request: AgentExecutionRequest,
        guard: ExecutionGuard<'_>,
    ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
        self.run(request, guard).await
    }
}

async fn wait_for_child(
    child: &mut Child,
    guard: &ExecutionGuard<'_>,
) -> Result<std::process::ExitStatus, AgentRuntimeError> {
    let mut cancellation = tokio::time::interval(CANCELLATION_POLL);
    cancellation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut revocation = tokio::time::interval(REVOCATION_POLL);
    revocation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            status = child.wait() => return status.map_err(|_| AgentRuntimeError::ExecutorFailed),
            _ = cancellation.tick() => {
                if guard.is_cancelled() {
                    return Err(AgentRuntimeError::Cancelled);
                }
            }
            _ = revocation.tick(), if guard.requires_continuous_revocation_checks() => {
                guard.check(AuthoritySafeBoundary::BeforeChunk, system_now_millis()).await?;
            }
        }
    }
}

async fn read_stdout<R: AsyncRead + Unpin>(
    mut reader: R,
    transcript: labby_runtime::agent_runtime::AgentTranscript,
) -> Result<(String, usize), std::io::Error> {
    let mut hasher = Sha256::new();
    let mut total = 0usize;
    let mut chunk = [0u8; 8192];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
        transcript.append(&chunk[..read]);
        total = total.saturating_add(read);
    }
    Ok((format!("sha256:{}", hex::encode(hasher.finalize())), total))
}

async fn drain_output<R: AsyncRead + Unpin>(mut reader: R) -> Result<usize, std::io::Error> {
    let mut total = 0usize;
    let mut chunk = [0u8; 8192];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read);
    }
    Ok(total)
}

fn baseline_environment_names() -> [&'static str; 6] {
    [
        "PATH",
        "LANG",
        "LC_ALL",
        "TMPDIR",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ]
}

fn executable_available(path: &std::path::Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

struct ProcessTreeGuard {
    #[cfg(unix)]
    process_group: Option<i32>,
    #[cfg(windows)]
    job: Option<labby_winjob::JobObject>,
}

impl ProcessTreeGuard {
    fn attach(child: &Child) -> Result<Self, AgentRuntimeError> {
        let pid = child.id().ok_or(AgentRuntimeError::ExecutorFailed)?;
        #[cfg(unix)]
        {
            Ok(Self {
                process_group: Some(
                    i32::try_from(pid).map_err(|_| AgentRuntimeError::ExecutorFailed)?,
                ),
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                job: Some(
                    labby_winjob::JobObject::assign(pid)
                        .map_err(|_| AgentRuntimeError::ExecutorFailed)?,
                ),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = pid;
            Ok(Self {})
        }
    }

    fn terminate_now(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.process_group.take() {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            drop(job.close());
        }
    }
}

impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        self.terminate_now();
    }
}

async fn terminate_process_tree(child: &mut Child, guard: &mut ProcessTreeGuard) {
    #[cfg(unix)]
    if let Some(pid) = guard.process_group {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGTERM,
        );
        tokio::time::sleep(TERMINATION_GRACE).await;
    }
    guard.terminate_now();
    drop(child.wait().await);
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_primitives::{
        access::{Capability, OwnerScope, PrincipalId, ResourceId},
        agent::{
            AgentDefinition, AgentRevision, AgentSessionBinding, AgentState,
            RunningRevocationPolicy,
        },
    };
    use labby_runtime::{
        agent_runtime::{
            AgentAuthority, AgentResourceBounds, AgentTranscript, Cancellation, execute_agent,
        },
        authority::{
            AuthorityBinding, AuthorityEpochVector, AuthorityEpochVectorInput, AuthorityLease,
            AuthoritySafeBoundary,
        },
    };

    struct TestAuthority {
        epochs: AuthorityEpochVector,
        now: u64,
    }

    impl AgentAuthority for TestAuthority {
        async fn current_epochs(&self) -> Result<AuthorityEpochVector, AgentRuntimeError> {
            Ok(self.epochs.clone())
        }

        fn now_millis(&self) -> u64 {
            self.now
        }
    }

    fn epochs() -> AuthorityEpochVector {
        AuthorityEpochVector::new(AuthorityEpochVectorInput {
            version: 1,
            authority_schema_generation: 1,
            installation_epoch: 1,
            organization_epoch: 1,
            principal_epoch: 1,
            team_membership_epochs: vec![],
            team_policy_epoch: None,
            project_membership_epoch: None,
            project_policy_epoch: None,
            resource_policy_epoch: None,
            gateway_catalog_generation: Some(1),
            depot_projection_watermark: None,
            credential_generation: Some(1),
            session_generation: 1,
        })
        .unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn configured_process_receives_exact_input_and_retains_bounded_stdout() {
        let owner = OwnerScope::Personal(PrincipalId::new("p-1").unwrap());
        let harness = AgentHarnessConfig {
            id: "cat".into(),
            content_digest: format!("sha256:{}", "a".repeat(64)),
            repository_digest: format!("sha256:{}", "b".repeat(64)),
            image_digest: format!("sha256:{}", "c".repeat(64)),
            loadout_digest: format!("sha256:{}", "d".repeat(64)),
            catalog_generation: "catalog-1".into(),
            command: "/bin/cat".into(),
            args: vec![],
            cwd: None,
            inherit_env: vec![],
        };
        let definition = AgentDefinition {
            id: "agent-1".into(),
            owner: owner.clone(),
            revision: AgentRevision {
                version: 1,
                content_digest: format!("sha256:{}", "a".repeat(64)),
                repository_digest: format!("sha256:{}", "b".repeat(64)),
                image_digest: format!("sha256:{}", "c".repeat(64)),
                harness_digest: harness.digest(),
                loadout_digest: format!("sha256:{}", "d".repeat(64)),
                catalog_generation: "catalog-1".into(),
                credential_references: vec![],
            },
            state: AgentState::Active,
            required_capabilities: vec![Capability::ScopeOperate],
            authority_epoch: 1,
            publication_epoch: 1,
            revocation_policy: RunningRevocationPolicy::StopAtSafeBoundary,
        };
        assert!(harness.matches_definition(&definition));
        let mut mismatched = definition.clone();
        mismatched.revision.image_digest = format!("sha256:{}", "e".repeat(64));
        assert!(!harness.matches_definition(&mismatched));
        let current = system_now_millis();
        let authority_epochs = epochs();
        let binding = AuthorityBinding::new(
            PrincipalId::new("p-1").unwrap(),
            owner.clone(),
            Capability::ScopeOperate,
            "EXECUTE",
            ResourceId::new("agent-1").unwrap(),
            None,
        )
        .unwrap();
        let lease = AuthorityLease::new(
            binding,
            &authority_epochs,
            current,
            current + 60_000,
            [
                AuthoritySafeBoundary::BeforeDispatch,
                AuthoritySafeBoundary::BeforeExternalEffect,
                AuthoritySafeBoundary::BeforeCommit,
            ],
        )
        .unwrap();
        let transcript = AgentTranscript::new(4).unwrap();
        let input = "hello from Labby";
        let output = execute_agent(
            &TestAuthority {
                epochs: authority_epochs.clone(),
                now: current,
            },
            &ProcessAgentExecutor::new(harness),
            AgentExecutionRequest {
                definition,
                session: AgentSessionBinding {
                    session_id: "session-1".into(),
                    agent_id: "agent-1".into(),
                    agent_version: 1,
                    principal: PrincipalId::new("p-1").unwrap(),
                    owner,
                    catalog_generation: "catalog-1".into(),
                    authority_fingerprint: authority_epochs.fingerprint().as_str().into(),
                    lease_expires_at: i64::try_from(current + 60_000).unwrap(),
                },
                input: input.into(),
                input_digest: format!("sha256:{}", hex::encode(Sha256::digest(input.as_bytes()))),
                transcript: transcript.clone(),
                lease,
                bounds: AgentResourceBounds {
                    max_runtime_millis: 10_000,
                    max_output_bytes: 1024,
                    max_external_effects: 1,
                },
            },
            Cancellation::new(),
            current,
        )
        .await
        .unwrap();

        assert_eq!(output.bytes, input.len());
        assert_eq!(
            output.digest,
            format!("sha256:{}", hex::encode(Sha256::digest(input.as_bytes())))
        );
        assert_eq!(transcript.snapshot().text, "hell");
        assert!(transcript.snapshot().truncated);
    }
}
