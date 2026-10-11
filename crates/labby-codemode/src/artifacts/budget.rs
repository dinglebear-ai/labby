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
    // Populated only for whole-store scans. Each ULID directory is sized once
    // in that same traversal; non-run content still contributes to total bytes.
    runs: Vec<(String, u64)>,
}

// Shared payload classification for admission and retention. Metadata is
// bounded separately; unreadable payload paths fail closed.
async fn usage(root: &Path, store: bool) -> Result<Usage, ToolError> {
    #[cfg(test)]
    if store {
        *store_scan_counts()
            .lock()
            .unwrap()
            .entry(root.to_owned())
            .or_default() += 1;
    }
    let mut usage = Usage::default();
    let mut stack = vec![(root.to_path_buf(), !store, None)];
    while let Some((path, is_run, run_index)) = stack.pop() {
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
                let name = entry.file_name();
                let run_name = name
                    .to_str()
                    .filter(|name| store && path == root && ulid::Ulid::from_string(name).is_ok());
                let child_index = if let Some(name) = run_name {
                    let index = usage.runs.len();
                    usage.runs.push((name.to_owned(), 0));
                    Some(index)
                } else {
                    run_index
                };
                stack.push((entry.path(), run_name.is_some(), child_index));
            } else if kind.is_file() {
                let metadata = entry.metadata().await.map_err(accounting_error)?;
                usage.bytes = usage.bytes.saturating_add(metadata.len());
                usage.files = usage.files.saturating_add(1);
                if let Some(index) = run_index {
                    let bytes = &mut usage.runs[index].1;
                    *bytes = bytes.saturating_add(metadata.len());
                }
            }
        }
    }
    Ok(usage)
}

pub(super) async fn run_payload_bytes(root: &Path) -> Result<u64, ToolError> {
    usage(root, false).await.map(|usage| usage.bytes)
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
    admit_with_retention(
        root,
        bytes,
        max_store_bytes,
        max_run_bytes,
        max_run_files,
        super::artifact_retention_runs(),
    )
    .await
}

async fn admit_with_retention(
    root: &Path,
    bytes: usize,
    max_store_bytes: u64,
    max_run_bytes: u64,
    max_run_files: u64,
    retain: usize,
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
    let prune = super::prune_artifact_runs_locked(store, retain, pruning_budget, &active);
    let reconciled = if under_pressure {
        prune.await;
        reclaim_inactive(store, max_store_bytes.saturating_sub(bytes), &active).await?;
        true
    } else {
        super::retention::prune_once(root, prune).await
    };
    // STORE_MUTATION excludes in-process publication and retention throughout
    // admission. A coalesced pass makes no changes, so the authoritative scan
    // above remains valid; no cached usage survives this admission.
    let stored = if max_store_bytes > 0 && reconciled {
        usage(store, true).await?.bytes
    } else {
        stored_before
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
    let inventory = usage(store, true).await?;
    if inventory.bytes <= budget {
        return Ok(());
    }
    let mut eligible: Vec<_> = inventory
        .runs
        .into_iter()
        .filter(|(name, _)| !active.contains(name))
        .collect();
    let reclaimable = eligible
        .iter()
        .map(|(_, bytes)| *bytes)
        .fold(0, u64::saturating_add);
    if inventory.bytes.saturating_sub(reclaimable) > budget {
        return Err(quota_error(
            "Code Mode artifact store has no byte budget available; active and non-run outputs are retained",
        ));
    }
    eligible.sort_by(|(left, _), (right, _)| left.cmp(right));
    let mut remaining = inventory.bytes;
    for (name, bytes) in eligible {
        if remaining <= budget {
            break;
        }
        tokio::fs::remove_dir_all(store.join(name))
            .await
            .map_err(accounting_error)?;
        remaining = remaining.saturating_sub(bytes);
    }
    // These totals select candidates only. The caller rescans the whole store
    // before admitting bytes, so external changes cannot cause overbooking.
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
fn store_scan_counts()
-> &'static std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, usize>> {
    static COUNTS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, usize>>,
    > = std::sync::OnceLock::new();
    COUNTS.get_or_init(Default::default)
}
