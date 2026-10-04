//! Serialized admission for explicit and automatically preserved artifacts.

use std::path::Path;

use tokio::sync::Mutex;

use crate::error::ToolError;

// Shared with retention so it cannot remove files during quota admission or
// publication. Holding this guard reserves the admitted bytes until commit.
pub(super) static STORE_MUTATION: Mutex<()> = Mutex::const_new(());
pub(super) const MAX_RUN_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const MAX_RUN_FILES: u64 = 256;

#[derive(Default)]
struct Usage {
    bytes: u64,
    files: u64,
}

// Unlike retention's best-effort walk, admission fails closed on unreadable
// paths. Metadata is bounded separately and is not artifact payload usage.
async fn usage(root: &Path, store: bool) -> Result<Usage, ToolError> {
    let mut usage = Usage::default();
    let mut stack = vec![(root.to_path_buf(), !store)];
    while let Some((path, is_run)) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&path).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && path == root => continue,
            Err(error) => return Err(accounting_error(error)),
        };
        while let Some(entry) = entries.next_entry().await.map_err(accounting_error)? {
            if is_run && entry.file_name() == crate::artifact_access::METADATA_DIR {
                continue;
            }
            let kind = entry.file_type().await.map_err(accounting_error)?;
            if kind.is_dir() {
                let child_is_run = store
                    && path == root
                    && entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| ulid::Ulid::from_string(name).is_ok());
                stack.push((entry.path(), child_is_run));
            } else if kind.is_file() {
                let metadata = entry.metadata().await.map_err(accounting_error)?;
                usage.bytes = usage.bytes.saturating_add(metadata.len());
                usage.files = usage.files.saturating_add(1);
            }
        }
    }
    Ok(usage)
}

fn accounting_error(error: std::io::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("cannot account for artifact storage: {error}"),
    }
}

fn quota_error(message: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "budget_exceeded".into(),
        message: message.into(),
    }
}

pub(super) async fn admit(
    root: &Path,
    bytes: usize,
    max_store_bytes: u64,
) -> Result<(), ToolError> {
    admit_with_limits(root, bytes, max_store_bytes, MAX_RUN_BYTES, MAX_RUN_FILES).await
}

// Caller holds STORE_MUTATION until publication completes or is abandoned.
async fn admit_with_limits(
    root: &Path,
    bytes: usize,
    max_store_bytes: u64,
    max_run_bytes: u64,
    max_run_files: u64,
) -> Result<(), ToolError> {
    let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
    let current = usage(root, false).await?;
    if current.files >= max_run_files {
        return Err(quota_error(
            "Code Mode artifact file count exceeds the per-run budget",
        ));
    }
    if current.bytes > max_run_bytes || bytes > max_run_bytes.saturating_sub(current.bytes) {
        return Err(quota_error(
            "Code Mode artifact bytes exceed the per-run budget",
        ));
    }
    let store = root
        .parent()
        .ok_or_else(|| quota_error("artifact run has no store root"))?;
    let mut active = super::active_artifact_runs_snapshot();
    if let Some(name) = root.file_name().and_then(|name| name.to_str()) {
        active.insert(name.to_owned());
    }
    if max_store_bytes > 0 && bytes > max_store_bytes {
        return Err(quota_error("artifact exceeds the total-store byte budget"));
    }
    // Make room before admission, preserving all active executions. A zero
    // remainder must not be passed as 0 (which disables byte retention).
    let pruning_budget = if max_store_bytes == 0 {
        0
    } else {
        max_store_bytes.saturating_sub(bytes).max(1)
    };
    let stored_before = if max_store_bytes > 0 {
        usage(store, true).await?.bytes
    } else {
        0
    };
    let under_pressure = max_store_bytes > 0
        && (stored_before > max_store_bytes
            || bytes > max_store_bytes.saturating_sub(stored_before));
    let prune = super::prune_artifact_runs_locked(
        store,
        super::artifact_retention_runs(),
        pruning_budget,
        &active,
    );
    if under_pressure {
        prune.await;
        reclaim_inactive(store, max_store_bytes.saturating_sub(bytes), &active).await?;
    } else {
        super::retention::prune_once(root, prune).await;
    }
    let stored = if max_store_bytes > 0 {
        usage(store, true).await?.bytes
    } else {
        0
    };
    if max_store_bytes > 0
        && (stored > max_store_bytes || bytes > max_store_bytes.saturating_sub(stored))
    {
        return Err(quota_error(
            "Code Mode artifact store has no byte budget available; active outputs are retained",
        ));
    }
    Ok(())
}

// Enforce the exact payload budget after best-effort retention. In particular,
// zero remaining bytes must reclaim every eligible inactive payload even though
// the legacy retention helper treats a zero byte limit as disabled.
async fn reclaim_inactive(
    store: &Path,
    budget: u64,
    active: &std::collections::HashSet<String>,
) -> Result<(), ToolError> {
    if usage(store, true).await?.bytes <= budget {
        return Ok(());
    }
    let mut entries = tokio::fs::read_dir(store).await.map_err(accounting_error)?;
    let mut eligible = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(accounting_error)? {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if ulid::Ulid::from_string(&name).is_ok()
            && !active.contains(&name)
            && entry.file_type().await.map_err(accounting_error)?.is_dir()
        {
            eligible.push(name);
        }
    }
    eligible.sort();
    for name in eligible {
        tokio::fs::remove_dir_all(store.join(name))
            .await
            .map_err(accounting_error)?;
        if usage(store, true).await?.bytes <= budget {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
