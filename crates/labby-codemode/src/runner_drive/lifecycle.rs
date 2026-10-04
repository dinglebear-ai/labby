use super::*;

impl<H: CodeModeHost> CodeModeBroker<'_, H> {
    /// Spawn the runner subprocess, send the code, and drive the
    /// tool-call/artifact/completion protocol loop until the runner exits
    /// or the wall-clock deadline fires.
    ///
    /// The runtime params are packed into [`RunnerConfig`] and the loop arms
    /// are delegated to named helpers. Timeout and killpg invariants are
    /// preserved exactly.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_in_runner(
        &self,
        code_to_run: String,
        proxy: String,
        timeout: Duration,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        max_log_entries: usize,
        max_log_bytes: usize,
        trace_params: bool,
        capability_filter: ToolScope,
        snippet_max_bytes: usize,
        execution_id: Option<Arc<str>>,
        trace_context: Option<Arc<TraceContext>>,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        // Read the openapi registry/client from the host at the config-build site
        // (per the plan's I4 fix — do NOT thread them down the positional arg
        // list). Host-less runs (some tests) get an empty registry + a hardened
        // client; the openapi shim is only ever emitted on the host path anyway.
        let (openapi_registry, openapi_http_client) = match self.host {
            Some(host) => (host.openapi_registry(), host.openapi_http_client()),
            None => (
                // Host-less runs have an empty registry, so this client is never
                // used to dispatch (`registry.operation` errors before any HTTP).
                // Prefer the hardened build; on the catastrophic TLS-init failure
                // that makes it fail, log and fall back to a default client — the
                // fallback is never sent, and `reqwest::Client::new()` would abort
                // on the same condition anyway, so this only adds a WARN trace.
                labby_openapi::OpenApiRegistry::default(),
                labby_openapi::http::build_dispatch_client().unwrap_or_else(|_| {
                    tracing::warn!(
                        service = "openapi",
                        "hardened dispatch client build failed on host-less run; using default (unused) client"
                    );
                    reqwest::Client::new()
                }),
            ),
        };
        let cfg = RunnerConfig {
            code_to_run,
            proxy,
            timeout,
            caller,
            surface,
            max_log_entries,
            max_log_bytes,
            trace_params,
            capability_filter,
            snippet_max_bytes: snippet_max_bytes.min(MAX_SNIPPET_RESOLVED_BYTES_PER_RUN),
            execution_id,
            trace_context,
            openapi_registry,
            openapi_http_client,
        };
        self.run_in_runner_with_config(cfg).await
    }

    pub(super) async fn run_in_runner_with_config(
        &self,
        cfg: RunnerConfig,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        // One end-to-end budget covers pool/semaphore admission, runner spawn,
        // the initial Start write, and the complete protocol execution.
        let deadline = tokio::time::Instant::now() + cfg.timeout;
        // Acquire a runner. With a host, use the shared warm pool (Perf H1): a
        // pooled runner amortizes the fork/startup cost across executions while
        // still building a fresh `javy::Runtime` per `Start` (runner-side), so JS
        // state isolation holds. Without a host (some tests / standalone paths),
        // spawn a one-shot runner directly.
        match self.host {
            Some(host) => self.run_via_pool(host.runner_pool(), cfg, deadline).await,
            None => self.run_standalone(cfg, deadline).await,
        }
    }

    /// Run one execution against a runner checked out from the shared pool.
    ///
    /// On a clean completion (`Done`) or a runner-reported execution `Error`,
    /// the runner is parked and returned to the pool — it stayed alive and built
    /// a fresh runtime, so it is safe to reuse. On a crash (EOF/exit), timeout,
    /// or protocol fault the runner is evicted (killed) and the slot respawns on
    /// the next checkout.
    pub(super) async fn run_via_pool(
        &self,
        pool: &RunnerPool,
        cfg: RunnerConfig,
        deadline: tokio::time::Instant,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        let mut lease = pool.checkout(deadline).await?;
        let outcome = self.drive_runner(lease.runner_mut(), &cfg, deadline).await;
        match outcome {
            DriveOutcome::Completed(response) => {
                drop(tokio::time::timeout_at(deadline, lease.release()).await);
                Ok(response)
            }
            DriveOutcome::ExecutionError(err) => {
                // The runner parked after emitting its `Error` line and is
                // healthy (fresh runtime dropped); reuse it.
                drop(tokio::time::timeout_at(deadline, lease.release()).await);
                Err(err)
            }
            DriveOutcome::RunnerUnavailableBeforeActivity(err) => {
                // A pooled runner can die while idle. Because it emitted no valid
                // protocol event for this execution, no host-visible side effect
                // crossed the sandbox boundary and one replay is safe.
                drop(tokio::time::timeout_at(deadline, lease.evict()).await);
                tracing::warn!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "pool.retry_fresh",
                    kind = err.kind(),
                    error = %err,
                    "Code Mode runner died before protocol activity; retrying once on a fresh runner"
                );

                let mut retry = pool.checkout_fresh(deadline).await?;
                let retry_outcome = self.drive_runner(retry.runner_mut(), &cfg, deadline).await;
                match retry_outcome {
                    DriveOutcome::Completed(response) => {
                        drop(tokio::time::timeout_at(deadline, retry.release()).await);
                        Ok(response)
                    }
                    DriveOutcome::ExecutionError(err) => {
                        drop(tokio::time::timeout_at(deadline, retry.release()).await);
                        Err(err)
                    }
                    DriveOutcome::RunnerUnavailableBeforeActivity(err)
                    | DriveOutcome::RunnerUnhealthy(err) => {
                        drop(tokio::time::timeout_at(deadline, retry.evict()).await);
                        Err(err)
                    }
                }
            }
            DriveOutcome::RunnerUnhealthy(err) => {
                // Crash / timeout / protocol fault after activity: discard the
                // runner without replaying the execution, bounded by the remaining deadline.
                drop(tokio::time::timeout_at(deadline, lease.evict()).await);
                Err(err)
            }
        }
    }

    /// Run one execution against a freshly-spawned one-shot runner (no pool).
    async fn run_standalone(
        &self,
        cfg: RunnerConfig,
        deadline: tokio::time::Instant,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        let (spawn, microsandbox) = crate::runner_backend::resolve_runner_spawn()?;
        let mut runner =
            crate::pool::spawn_before_deadline(&spawn, microsandbox.as_ref(), deadline).await?;
        let outcome = self.drive_runner(&mut runner, &cfg, deadline).await;
        let result = match outcome {
            DriveOutcome::Completed(response) => Ok(response),
            DriveOutcome::ExecutionError(err)
            | DriveOutcome::RunnerUnavailableBeforeActivity(err)
            | DriveOutcome::RunnerUnhealthy(err) => Err(err),
        };
        drop(tokio::time::timeout_at(deadline, runner.shutdown()).await);
        result
    }
}
