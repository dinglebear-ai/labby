//! Connection establishment for stdio (child-process) upstreams.
//!
//! `connect_stdio_upstream` spawns a child process and arms the process-group
//! guard. In-process peer construction is owned by `crate::mcp::in_process_peer`
//! (the `InProcessConnector` IoC seam) — this module no longer imports from
//! `crate::mcp` (A-M6 fix).

use labby_runtime::gateway_config::UpstreamConfig;
use rmcp::ClientHandler;
use rmcp::service::ClientServiceExt;
use rmcp::transport::TransportAdapterIdentity;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use super::super::auth::configured_bearer_token;
use super::super::types::{UpstreamRuntimeMetadata, UpstreamRuntimeOwner};
use super::catalog_pagination;
use super::connect::{
    OrderedRelayNotificationTransport, RelayNotificationInterceptor, runtime_origin_label,
};
use super::helpers::STDIO_DISCOVERY_TIMEOUT;
use super::legacy_client::VersionedClientHandler;
use super::lifecycle_compat::{
    LifecycleAttempt, compatibility_retry, legacy_protocol_version, log_fallback,
};
use super::stdio_stderr::{
    StdioConnectError, StdioDiagnostics, forward_upstream_stderr, upstream_stderr_log_level,
};
use super::tools::MAX_UPSTREAM_TOOLS;
use super::{UpstreamClientService, UpstreamConnection};

static LEGACY_STDIO_LIFECYCLE: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();

fn stdio_lifecycle_key(name: &str, command: &str, args: &[String]) -> String {
    format!("{name}\u{0}{command}\u{0}{}", args.join("\u{0}"))
}

fn prefers_legacy_stdio_lifecycle(key: &str) -> bool {
    LEGACY_STDIO_LIFECYCLE
        .get_or_init(|| RwLock::new(HashSet::new()))
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(key)
}

fn remember_legacy_stdio_lifecycle(key: String) {
    LEGACY_STDIO_LIFECYCLE
        .get_or_init(|| RwLock::new(HashSet::new()))
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key);
}

/// Connect to a stdio upstream MCP server (child process).
///
/// ## Security invariants
///
/// - **`env_clear` + allowlist (S1):** the child process is started with a
///   scrubbed environment (`cmd.env_clear()`). Only vars in `STDIO_ENV_ALLOWLIST`
///   (runtime essentials: PATH, HOME, TZ, SSL roots, …) are forwarded; the
///   upstream's declared `env` map and the optional bearer-token var are then
///   layered on top. `LABBY_*` secrets and every other ambient labby env var are
///   excluded.
///
/// - **Spawn-guard allowlist (S6 — accepted residual):** `validate_stdio_command`
///   in `spawn_guard.rs` checks that the command basename is in
///   `ALLOWED_RUNTIME_HINTS`. The check is **basename-only** — a path like
///   `/tmp/x/node` passes because `Path::file_name()` extracts `node`. This is
///   an accepted residual: the trust boundary is admin-write access to the gateway
///   config file or authenticated `gateway.add` / `gateway.update` calls. The
///   allowlist is applied at config-write time, not here.
pub(super) async fn connect_stdio_upstream<H: ClientHandler + Clone>(
    command: &str,
    args: &[String],
    config: &UpstreamConfig,
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
    handler: H,
    notification_interceptor: Option<RelayNotificationInterceptor>,
) -> anyhow::Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>)> {
    let env = stdio_environment(config, None);
    let command_spec = StdioCommandSpec {
        program: OsString::from(command),
        args: args.iter().map(OsString::from).collect(),
        cwd: None,
        env,
        inherit_env: Vec::new(),
        display: command.to_string(),
        name: config.name.clone(),
        runtime_origin: runtime_origin_label(runtime_origin, runtime_owner),
        runtime_owner: runtime_owner.cloned(),
    };
    connect_stdio_command(command_spec, handler, true, notification_interceptor).await
}

fn stdio_environment(
    config: &UpstreamConfig,
    dotenv_path: Option<&std::path::Path>,
) -> Vec<(OsString, OsString)> {
    let mut env = config
        .env
        .iter()
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
        .collect::<Vec<_>>();
    if let Some(ref env_name) = config.bearer_token_env
        && let Some(token) = dotenv_path.map_or_else(
            || configured_bearer_token(env_name),
            |path| super::super::auth::configured_bearer_token_from_path(env_name, Some(path)),
        )
    {
        env.push((OsString::from(env_name), OsString::from(token)));
    }
    env
}

pub(crate) async fn connect_direct_stdio<H: ClientHandler + Clone>(
    command: crate::upstream::direct_stdio::DirectStdioCommand,
    handler: H,
) -> anyhow::Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>)> {
    let spec = StdioCommandSpec {
        program: command.program,
        args: command.args,
        cwd: Some(command.cwd),
        env: command.env,
        inherit_env: command.inherit_env,
        name: "direct-stdio".to_string(),
        display: command.display,
        runtime_origin: Some("proxy:local-cli".to_string()),
        runtime_owner: None,
    };
    connect_stdio_command(spec, handler, false, None).await
}

#[derive(Clone)]
struct StdioCommandSpec {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    inherit_env: Vec<OsString>,
    display: String,
    name: String,
    runtime_origin: Option<String>,
    runtime_owner: Option<UpstreamRuntimeOwner>,
}

async fn connect_stdio_command<H: ClientHandler + Clone>(
    command: StdioCommandSpec,
    handler: H,
    allow_cache_repair: bool,
    notification_interceptor: Option<RelayNotificationInterceptor>,
) -> anyhow::Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>)> {
    // Cross-process spawn lock: stdio servers launched via `npx -y`/`uvx` install
    // into a shared package cache on first cold spawn; two processes installing
    // the same package at once corrupt it. Hold an advisory file lock (keyed on
    // the command + args) for the whole connect — spawn, handshake, list_tools,
    // and a possible targeted cache repair/retry.
    let lock_command = command.program.to_string_lossy();
    let lock_args = command
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut spawn_lock = super::spawn_lock::open(&lock_command, &lock_args);
    let _spawn_guard = super::spawn_lock::acquire(spawn_lock.as_mut()).await;

    let lifecycle_key = stdio_lifecycle_key(&command.name, lock_command.as_ref(), &lock_args);
    let initial_attempt = if prefers_legacy_stdio_lifecycle(&lifecycle_key) {
        LifecycleAttempt::LegacyInitialize
    } else {
        LifecycleAttempt::Modern
    };

    match connect_stdio_upstream_once(
        &command,
        handler.clone(),
        initial_attempt,
        notification_interceptor.clone(),
    )
    .await
    {
        Ok(ok) => Ok(ok),
        Err(first_error) => {
            let lifecycle_error = anyhow::anyhow!(first_error.diagnostics_with_error());
            if initial_attempt == LifecycleAttempt::Modern
                && let Some(attempt) = compatibility_retry(&lifecycle_error)
            {
                remember_legacy_stdio_lifecycle(lifecycle_key);
                log_fallback(&command.name, "stdio", attempt, &lifecycle_error);
                return connect_stdio_upstream_once(
                    &command,
                    handler,
                    attempt,
                    notification_interceptor.clone(),
                )
                .await
                .map_err(StdioConnectError::into_anyhow);
            }
            let diagnostics = first_error.diagnostics_with_error();
            if !allow_cache_repair {
                return Err(first_error.into_anyhow());
            }
            let repair = super::cache_repair::maybe_repair(&lock_command, &diagnostics).await;
            match &repair {
                super::cache_repair::CacheRepairOutcome::Repaired { summary } => {
                    tracing::warn!(
                        surface = "dispatch",
                        service = "upstream.pool",
                        upstream = %command.name,
                        command = %command.display,
                        action = "upstream.cache_repair",
                        repair = %summary,
                        "stdio package-runner cache repaired after startup failure; retrying once"
                    );
                }
                super::cache_repair::CacheRepairOutcome::Failed { summary } => {
                    tracing::warn!(
                        surface = "dispatch",
                        service = "upstream.pool",
                        upstream = %command.name,
                        command = %command.display,
                        action = "upstream.cache_repair",
                        repair = %summary,
                        "stdio package-runner cache repair failed; returning original startup error"
                    );
                    return Err(first_error.into_anyhow());
                }
                _ => return Err(first_error.into_anyhow()),
            }

            match connect_stdio_upstream_once(
                &command,
                handler,
                initial_attempt,
                notification_interceptor,
            )
            .await
            {
                Ok(ok) => Ok(ok),
                Err(retry_error) => Err(anyhow::anyhow!(
                    "stdio upstream failed after package-runner cache repair retry: {}",
                    retry_error.diagnostics_with_error()
                )),
            }
        }
    }
}

async fn connect_stdio_upstream_once<H: ClientHandler>(
    command: &StdioCommandSpec,
    handler: H,
    lifecycle: LifecycleAttempt,
    notification_interceptor: Option<RelayNotificationInterceptor>,
) -> Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>), StdioConnectError> {
    use process_wrap::tokio::CommandWrap;
    #[cfg(unix)]
    use process_wrap::tokio::ProcessGroup;
    use tokio::process::Command;

    use super::stdio_transport::DiagnosticChildTransport;

    // SECURITY (S1): never inherit labby's full environment — it holds
    // LABBY_OAUTH_ENCRYPTION_KEY and every upstream credential. Start from a
    // scrubbed allowlist of runtime essentials (so npx/uvx/docker/etc. can still
    // find binaries, caches, and TLS roots), then layer the upstream's declared
    // env (and bearer token, below) on top.
    const STDIO_ENV_ALLOWLIST: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "TERM",
        "TZ",
        "TMPDIR",
        "TMP",
        "TEMP",
        "LANG",
        "LANGUAGE",
        "LC_ALL",
        "LC_CTYPE",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_RUNTIME_DIR",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NODE_EXTRA_CA_CERTS",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
        "SYSTEMROOT",
        "SYSTEMDRIVE",
        "WINDIR",
        "PATHEXT",
        "COMSPEC",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
    ];

    let mut cmd = Command::new(&command.program);
    cmd.args(&command.args);
    if let Some(cwd) = &command.cwd {
        cmd.current_dir(cwd);
    }
    cmd.env_clear();
    for key in STDIO_ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    for key in &command.inherit_env {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    cmd.envs(command.env.iter().cloned());

    // A stdio MCP server logs to stderr (stdout is the JSON-RPC channel), so the
    // child's stderr is the ONLY place its server-side diagnostics go. Capture
    // it by default and forward into the gateway log at the level resolved from
    // `LABBY_GW_UPSTREAM_STDERR` (default DEBUG; `off` discards).
    let stderr_level = upstream_stderr_log_level();
    let stderr_capture = StdioDiagnostics::default();
    let wrapped = CommandWrap::from(cmd);
    #[cfg(unix)]
    let mut wrapped = wrapped;
    #[cfg(unix)]
    wrapped.wrap(ProcessGroup::leader());
    let (process, child_stderr) =
        DiagnosticChildTransport::spawn(wrapped, command.name.clone(), stderr_capture.clone())
            .map_err(StdioConnectError::without_diagnostics)?;

    // INVARIANT: a piped child stderr MUST be drained continuously. A chatty
    // upstream (e.g. axon at INFO) fills the ~64 KB pipe buffer and then blocks
    // on its next stderr write, hanging the upstream. The drain task reads to
    // EOF so failures are recoverable from the gateway log instead of lost.
    forward_upstream_stderr(
        child_stderr,
        command.name.clone(),
        stderr_level,
        stderr_capture.clone(),
    );

    let pid = process.id();
    let generation = process.generation();
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %command.name, transport = "stdio",
        action = "upstream.connect.start", command = %command.display, pid = ?pid,
        generation,
        "upstream connect start",
    );

    // INVARIANT: arm the process-tree guard immediately after spawn. If any
    // subsequent `?` propagates (serve fails, bounded tools/list fails, the outer
    // future is dropped on timeout), `Drop` on this guard reaps grandchildren
    // (npx → node, sh -c → python) that rmcp's per-PID TokioChildProcess Drop
    // would otherwise miss.
    //
    // Unix: `ProcessGroup::leader()` made the child its own group leader
    //   (pgid == pid). The guard SIGTERMs+SIGKILLs the group via `killpg`.
    //
    // Windows: `JobObjectGuard::arm` creates a Job Object, assigns the child
    //   (and therefore all its future descendants) to it, and sets
    //   JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE. Closing the handle (on Drop or in
    //   shutdown) lets the OS terminate the whole tree.
    #[cfg(unix)]
    let pg_guard = pid.map(super::super::process_guard::ProcessGroupGuard::arm);
    #[cfg(windows)]
    let job_guard = pid.map(super::super::process_guard::JobObjectGuard::arm);

    let process = OrderedRelayNotificationTransport::new(process, notification_interceptor);
    let service = match lifecycle {
        LifecycleAttempt::Modern => {
            let service = match handler
                .serve_with_lifecycle::<_, _, TransportAdapterIdentity>(process, lifecycle.mode())
                .await
            {
                Ok(service) => service,
                Err(error) => {
                    return Err(StdioConnectError::with_diagnostics(error, &stderr_capture).await);
                }
            };
            UpstreamClientService::Direct(service)
        }
        LifecycleAttempt::LegacyInitialize => {
            let service = match VersionedClientHandler::new(handler, legacy_protocol_version())
                .serve_with_lifecycle::<_, _, TransportAdapterIdentity>(process, lifecycle.mode())
                .await
            {
                Ok(service) => service,
                Err(error) => {
                    return Err(StdioConnectError::with_diagnostics(error, &stderr_capture).await);
                }
            };
            UpstreamClientService::Versioned(service)
        }
    };
    let peer = service.peer().clone();

    // Discover tools
    let tools =
        match catalog_pagination::list_tools(&peer, STDIO_DISCOVERY_TIMEOUT, MAX_UPSTREAM_TOOLS)
            .await
        {
            Ok(tools) => tools,
            Err(error) => {
                return Err(StdioConnectError::with_diagnostics(
                    error.into_service_error(&command.name),
                    &stderr_capture,
                )
                .await);
            }
        };
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %command.name, transport = "stdio",
        action = "upstream.connect.finish", pid = ?pid, generation,
        tool_count = tools.len(),
        "upstream connect finish",
    );

    // INVARIANT: disarm the guard right before successful construction. The
    // reaping resource (pgid on Unix, job handle on Windows) is transferred
    // to UpstreamConnection.runtime; its own Drop now owns cleanup.
    // `shutdown()` clears the field before any `.await` so Drop no-ops on
    // the graceful path.
    #[cfg(unix)]
    let pgid_for_runtime =
        pg_guard.and_then(super::super::process_guard::ProcessGroupGuard::disarm);
    // On Windows pgid has no meaning — leave it None. The job field
    // (set below) is the Windows-only reaping resource.
    #[cfg(windows)]
    let pgid_for_runtime: Option<u32> = None;
    // Non-Unix, non-Windows (hypothetical future target): no process-group
    // reaping mechanism; pgid stays None.
    #[cfg(all(not(unix), not(windows)))]
    let pgid_for_runtime: Option<u32> = None;

    // Transfer exclusive job ownership into the connection runtime.
    #[cfg(windows)]
    let job_for_runtime = job_guard.and_then(super::super::process_guard::JobObjectGuard::disarm);

    let conn = UpstreamConnection::new_with_client_service(
        service,
        None,
        peer,
        UpstreamRuntimeMetadata {
            pid,
            generation: Some(generation),
            pgid: pgid_for_runtime,
            #[cfg(windows)]
            job: job_for_runtime,
            started_at: Some(std::time::SystemTime::now()),
            origin: command.runtime_origin.clone(),
            owner: command.runtime_owner.clone(),
        },
    );

    Ok((conn, tools))
}

#[cfg(test)]
mod conformance_tests {
    use super::*;

    #[test]
    fn stdio_named_environment_credential_reaches_child_environment_without_oauth() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dotenv = dir.path().join(".env");
        std::fs::write(&dotenv, "UPSTREAM_TOKEN=stdio-sentinel-secret\n").expect("write dotenv");
        let mut config = super::super::testsupport::test_upstream_config();
        config.url = None;
        config.command = Some("server".to_string());
        config.bearer_token_env = Some("UPSTREAM_TOKEN".to_string());

        let child_env = stdio_environment(&config, Some(&dotenv));

        assert!(config.oauth.is_none());
        assert!(
            child_env.iter().any(|(name, value)| {
                name == "UPSTREAM_TOKEN" && value == "stdio-sentinel-secret"
            })
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::testsupport::named_test_upstream_config;
    use super::*;

    #[test]
    fn remembers_legacy_lifecycle_for_exact_stdio_command() {
        let config = named_test_upstream_config("legacy-stdio-cache-test");
        let args = vec![
            "nested-host".to_string(),
            "mcp".to_string(),
            "serve".to_string(),
        ];
        let key = stdio_lifecycle_key(&config.name, "ssh", &args);

        assert!(!prefers_legacy_stdio_lifecycle(&key));
        remember_legacy_stdio_lifecycle(key.clone());
        assert!(prefers_legacy_stdio_lifecycle(&key));

        let other_args = vec![
            "other-host".to_string(),
            "mcp".to_string(),
            "serve".to_string(),
        ];
        let other_key = stdio_lifecycle_key(&config.name, "ssh", &other_args);
        assert!(!prefers_legacy_stdio_lifecycle(&other_key));
    }
}
