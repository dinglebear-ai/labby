use super::*;
use labby_runtime::agent_error::{
    AgentRecoveryAction, AgentRecoveryAdvice, AgentSameArgumentsRetry, AgentSideEffectRisk,
};
use std::io::Write;
use std::time::{Duration, Instant};

/// A persistent lock file avoids lock-inode replacement races between processes.
pub(super) fn lock(dir: &Path) -> Result<fs::File, ToolError> {
    let path = dir.join(".snippet-write.lock");
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| io_error("open snippet lock", &path, e))?;
    let started = Instant::now();
    let budget = Duration::from_millis(1500);
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(fs::TryLockError::WouldBlock) if started.elapsed() < budget => {
                std::thread::sleep(Duration::from_millis(10).min(budget.saturating_sub(started.elapsed())));
            }
            Err(fs::TryLockError::WouldBlock) => return Err(ToolError::contract(
                "conflict",
                "snippet writer is busy; no snippet was changed",
                Map::from_iter([("existing_id".into(), serde_json::json!("snippet-write-lock"))]),
                None,
                Some(AgentRecoveryAdvice { action: AgentRecoveryAction::RetryLater, same_arguments: AgentSameArgumentsRetry::Conditional,
                    guidance: "Retry after the active writer finishes; any expected digest is rechecked before publication.".into(), retry_after_ms: Some(250) }),
                Some(AgentSideEffectRisk::NoneExpected),
            )),
            Err(fs::TryLockError::Error(error)) => return Err(io_error("lock snippets", &path, error)),
        }
    }
    Ok(file)
}

pub(super) fn publish(
    dir: &Path,
    name: &str,
    body: &str,
    force: bool,
    expected_digest: Option<&str>,
) -> Result<PathBuf, ToolError> {
    let _lock = lock(dir)?;
    let existing = find_snippet_file(dir, name);
    let conflict = |message| ToolError::Conflict {
        message,
        existing_id: name.to_string(),
    };
    if existing.is_some() && !force {
        return Err(conflict(format!("user snippet `{name}` already exists")));
    }
    if let Some(expected) = expected_digest {
        let actual = existing
            .as_ref()
            .map(|path| read_snippet_body(path))
            .transpose()?;
        if actual.as_deref().map(cache::digest).as_deref() != Some(expected) {
            return Err(conflict(format!(
                "snippet `{name}` changed since it was read; reload before editing"
            )));
        }
    }
    // Keep the active extension: replacing a JS override must not leave a second stale file.
    let path = existing.unwrap_or_else(|| dir.join(format!("{name}.md")));
    // The resolver detects Markdown by contents, so legacy .js paths retain new metadata.
    let bytes = body;
    let mut temp = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| io_error("create temp snippet", dir, e))?;
    temp.write_all(bytes.as_bytes())
        .map_err(|e| io_error("write temp snippet", dir, e))?;
    temp.as_file()
        .sync_all()
        .map_err(|e| io_error("sync temp snippet", dir, e))?;
    if force {
        temp.persist(&path)
            .map_err(|e| io_error("publish snippet", &path, e.error))?;
    } else {
        temp.persist_noclobber(&path).map_err(|e| {
            if e.error.kind() == std::io::ErrorKind::AlreadyExists {
                conflict(format!("user snippet `{name}` already exists"))
            } else {
                io_error("publish snippet", &path, e.error)
            }
        })?;
    }
    if let Ok(file) = fs::File::open(dir) {
        file.sync_all()
            .map_err(|e| io_error("sync snippets directory", dir, e))?;
    }
    Ok(path)
}
