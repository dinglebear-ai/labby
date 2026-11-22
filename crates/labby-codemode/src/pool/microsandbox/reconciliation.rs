use super::*;

pub(super) async fn reconcile_stale_sandboxes(
    config: &crate::pool::MicrosandboxSpawn,
) -> Result<(), ToolError> {
    const RETRY_INTERVAL: Duration = Duration::from_secs(5);
    let cache = RECONCILED_EXECUTABLES.get_or_init(Default::default);
    let mut reconciled = cache.lock().await;
    match reconciled.get(&config.executable) {
        Some(None) => return Ok(()),
        Some(Some((failed_at, error))) if failed_at.elapsed() < RETRY_INTERVAL => {
            return Err(error.clone());
        }
        _ => {}
    }
    let result = reconcile_once(config).await;
    reconciled.insert(
        config.executable.clone(),
        result
            .as_ref()
            .err()
            .map(|error| (Instant::now(), error.clone())),
    );
    result
}

async fn reconcile_once(config: &crate::pool::MicrosandboxSpawn) -> Result<(), ToolError> {
    let started = Instant::now();
    let mut list = Command::new(&config.executable);
    list.args(["list", "--quiet", "--label", "labby.owner=codemode"])
        .env_clear()
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = run_helper(list, helper_timeout(DEFAULT_HELPER_TIMEOUT))
        .await
        .map_err(|error| ToolError::Sdk {
            sdk_kind: error.sdk_kind.into(),
            message: format!(
                "failed to reconcile stale Microsandbox runners: {}",
                error.message
            ),
        })?;
    if !output.status.success() {
        return Err(ToolError::Sdk {
            sdk_kind: "cleanup_failed".into(),
            message: format!(
                "failed to reconcile stale Microsandbox runners: {}",
                sanitize_stderr(&output.stderr)
            ),
        });
    }
    for name in String::from_utf8_lossy(&output.stdout).lines() {
        let Some(pid) = sandbox_owner_pid(name) else {
            continue;
        };
        if owner_process_is_alive(pid) {
            continue;
        }
        let mut remove = Command::new(&config.executable);
        remove
            .args(["remove", "--quiet", "--force", name])
            .env_clear()
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let removed = run_helper(remove, helper_timeout(DEFAULT_HELPER_TIMEOUT))
            .await
            .map_err(|error| ToolError::Sdk {
                sdk_kind: error.sdk_kind.into(),
                message: format!(
                    "failed to remove stale Microsandbox `{name}`: {}",
                    error.message
                ),
            })?;
        if !removed.status.success() {
            return Err(ToolError::Sdk {
                sdk_kind: "cleanup_failed".into(),
                message: format!(
                    "failed to remove stale Microsandbox `{name}`: {}",
                    sanitize_stderr(&removed.stderr)
                ),
            });
        }
    }
    log_lifecycle(
        "microsandbox.reconcile",
        "labby.owner=codemode",
        started,
        "success",
        None,
    );
    Ok(())
}

/// Whether the process that owns a sandbox is still running.
///
/// This gates deletion during reconciliation, so it must fail *closed*: a pid
/// we cannot prove is dead is treated as alive and its sandbox is left alone.
/// The previous `/proc/{pid}` probe was Linux-only, so on every other platform
/// each owner read as dead and reconciliation deleted sandboxes belonging to
/// live processes — including the running one.
#[cfg(unix)]
pub(super) fn owner_process_is_alive(pid: u32) -> bool {
    // A pid outside the platform's pid range cannot name a live process, so
    // this is proof of death rather than an unknown.
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    // Signal 0 runs the existence and permission checks without delivering
    // anything. `EPERM` means the process exists but belongs to another user,
    // which is still alive for our purposes.
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(raw), None) {
        Ok(()) => true,
        Err(nix::errno::Errno::ESRCH) => false,
        Err(_) => true,
    }
}

#[cfg(not(unix))]
pub(super) fn owner_process_is_alive(_pid: u32) -> bool {
    // No portable probe here; never delete what we cannot prove is dead.
    true
}

pub(super) fn sandbox_owner_pid(name: &str) -> Option<u32> {
    name.strip_prefix("labby-codemode-")?
        .split_once('-')?
        .0
        .parse()
        .ok()
}

pub(super) async fn reconcile_failed_cleanups(
    executable: &std::path::Path,
) -> Result<(), ToolError> {
    let pending = {
        let failed = failed_cleanups()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        failed
            .keys()
            .filter(|identity| identity.executable == executable)
            .cloned()
            .collect::<Vec<_>>()
    };
    if pending.is_empty() {
        return Ok(());
    }

    let started = Instant::now();
    let live = list_owned_sandbox_names(executable).await?;
    for identity in pending {
        if !live.contains(&identity.name) {
            resolve_failed_cleanup(&identity);
            log_lifecycle(
                "microsandbox.cleanup_reconcile",
                &identity.name,
                started,
                "absent",
                None,
            );
            continue;
        }

        let mut remove = Command::new(executable);
        remove
            .args(["remove", "--quiet", "--force", &identity.name])
            .env_clear()
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        match run_helper(remove, helper_timeout(DEFAULT_HELPER_TIMEOUT)).await {
            Ok(output) if output.status.success() => {
                resolve_failed_cleanup(&identity);
                log_lifecycle(
                    "microsandbox.cleanup_reconcile",
                    &identity.name,
                    started,
                    "success",
                    None,
                );
            }
            Ok(output) => {
                tracing::warn!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "microsandbox.cleanup_reconcile",
                    kind = "cleanup_failed",
                    sandbox = %identity.name,
                    status = %output.status,
                    error = sanitize_stderr(&output.stderr),
                    "Microsandbox cleanup reconciliation could not remove a live guest"
                );
            }
            Err(error) => {
                tracing::warn!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "microsandbox.cleanup_reconcile",
                    kind = error.kind,
                    sandbox = %identity.name,
                    executable = %executable.display(),
                    error = %error.message,
                    "Microsandbox cleanup reconciliation could not verify removal"
                );
            }
        }
    }

    ensure_cleanup_circuit_closed(executable)
}

async fn list_owned_sandbox_names(
    executable: &std::path::Path,
) -> Result<HashSet<String>, ToolError> {
    let mut list = Command::new(executable);
    list.args(["list", "--quiet", "--label", "labby.owner=codemode"])
        .env_clear()
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = run_helper(list, helper_timeout(DEFAULT_HELPER_TIMEOUT))
        .await
        .map_err(|error| ToolError::Sdk {
            sdk_kind: "cleanup_failed".into(),
            message: format!(
                "failed to verify unresolved Microsandbox cleanups: {}",
                error.message
            ),
        })?;
    if !output.status.success() {
        return Err(ToolError::Sdk {
            sdk_kind: "cleanup_failed".into(),
            message: format!(
                "failed to verify unresolved Microsandbox cleanups: {}",
                sanitize_stderr(&output.stderr)
            ),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub(super) fn ensure_cleanup_circuit_closed(executable: &std::path::Path) -> Result<(), ToolError> {
    let failed = failed_cleanups()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let identities = failed
        .keys()
        .filter(|identity| identity.executable == executable)
        .count();
    if identities == 0 {
        return Ok(());
    }
    Err(ToolError::Sdk {
        sdk_kind: "cleanup_failed".into(),
        message: format!(
            "refusing to create a Microsandbox runner while {identities} sandbox cleanup(s) remain unresolved"
        ),
    })
}
