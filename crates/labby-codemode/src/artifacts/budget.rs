//! Serialized admission for explicit and automatically preserved artifacts.

#[cfg(target_os = "linux")]
mod watch;

use crate::artifact_access::FileIdentity;
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use crate::error::ToolError;

pub(super) const MAX_RUN_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const MAX_RUN_FILES: u64 = 256;

pub(super) type RunTotals = HashMap<String, (u64, u64)>;

#[derive(Clone, Default)]
struct Usage {
    bytes: u64,
    files: u64,
    run_dirs: usize,
    #[cfg(test)]
    directory_reads: usize,
    #[cfg(test)]
    metadata_checks: usize,
    // Populated only for whole-store scans. Each ULID directory is sized once
    // in the initial traversal. Non-run content still contributes to total bytes.
    runs: RunTotals,
}

impl Usage {
    fn metadata(&mut self, path: &Path) -> std::io::Result<std::fs::Metadata> {
        #[cfg(test)]
        {
            self.metadata_checks += 1;
        }
        std::fs::symlink_metadata(path)
    }
}

// A directory identity covers membership changes, but an in-place file resize
// does not change that identity. Without kernel notifications the portable
// inventory must check each child before trusting its recorded size.
type DirectoryInventory = HashMap<PathBuf, (FileIdentity, Arc<[OsString]>)>;
#[derive(Default)]
struct Ledger {
    #[cfg(target_os = "linux")]
    watch: Option<watch::StoreWatch>,
    directories: DirectoryInventory,
    sizes: HashMap<PathBuf, u64>,
    usage: Usage,
}

static INVENTORIES: LazyLock<std::sync::Mutex<VecDeque<(PathBuf, Ledger)>>> =
    LazyLock::new(Default::default);

async fn usage(root: &Path, store: bool) -> Result<Usage, ToolError> {
    let root = root.to_owned();
    tokio::task::spawn_blocking(move || {
        if !store {
            return measure_usage(&root, false, Ledger::default())
                .map(|(usage, _)| usage)
                .map_err(accounting_error);
        }
        let cached = {
            let mut inventories = INVENTORIES
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inventories
                .iter()
                .position(|(path, _)| path == &root)
                .and_then(|i| inventories.remove(i))
                .map(|(_, ledger)| ledger)
                .unwrap_or_default()
        };
        let (usage, updated) = measure_usage(&root, store, cached).map_err(accounting_error)?;
        if let Some(updated) = updated {
            let mut inventories = INVENTORIES
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inventories.retain(|(path, _)| path != &root);
            if inventories.len() == 2 {
                inventories.pop_front();
            }
            inventories.push_back((root, updated));
        }
        Ok(usage)
    })
    .await
    .map_err(|error| accounting_error(std::io::Error::other(error)))?
}

fn measure_usage(
    root: &Path,
    store: bool,
    mut previous: Ledger,
) -> std::io::Result<(Usage, Option<Ledger>)> {
    #[cfg(target_os = "linux")]
    if let Some(mut watch) = previous.watch.take() {
        #[cfg(test)]
        {
            previous.usage.directory_reads = 0;
            previous.usage.metadata_checks = 1; // root identity
        }
        if watch.refresh(&mut previous, root).is_some() {
            previous.watch = Some(watch);
            return Ok((previous.usage.clone(), Some(previous)));
        }
        // Discard the watch set and all deltas together after an overflow or move.
        previous = Ledger::default();
    }
    #[cfg(test)]
    if store {
        *tests::store_scan_counts()
            .lock()
            .unwrap()
            .entry(root.to_owned())
            .or_default() += 1;
    }
    let mut usage = previous.usage;
    usage.run_dirs = 0;
    #[cfg(test)]
    {
        usage.directory_reads = 0;
        usage.metadata_checks = 0;
    }
    let mut updated = Ledger::default();
    #[cfg(target_os = "linux")]
    if store {
        updated.watch = watch::StoreWatch::new(root);
    }
    let mut keep_sizes = store;
    let mut retained_sizes = 0usize;
    let mut retained_bytes = 0usize;
    let mut stack = vec![(root.to_owned(), !store)];
    while let Some((path, is_run)) = stack.pop() {
        let dir = match usage.metadata(&path) {
            Ok(dir) if dir.is_dir() => dir,
            Ok(_) => return Err(std::io::Error::other("artifact directory changed type")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && path == root => continue,
            Err(error) => return Err(error),
        };
        #[cfg(target_os = "linux")]
        watch::StoreWatch::observe(&mut updated.watch, &path, &dir);
        let identity = FileIdentity::from_metadata(&dir);
        let cached_names = previous
            .directories
            .remove(&path)
            .filter(|(cached, _)| Some(cached) == identity.as_ref())
            .map(|(_, names)| names);
        let mut cost = path.as_os_str().len() + 128;
        let available = (1024 * 1024_usize).saturating_sub(retained_bytes);
        let mut new_names = Vec::new();
        let mut read = if cached_names.is_some() {
            None
        } else {
            #[cfg(test)]
            {
                usage.directory_reads += 1;
            }
            Some(std::fs::read_dir(&path)?)
        };
        let mut cached = cached_names.as_deref().unwrap_or_default().iter();
        loop {
            let fresh;
            let name = if let Some(entries) = &mut read {
                let Some(entry) = entries.next() else {
                    break;
                };
                fresh = entry?.file_name();
                &fresh
            } else {
                let Some(name) = cached.next() else {
                    break;
                };
                name
            };
            cost = cost.saturating_add(name.len() + size_of::<OsString>());
            if read.is_some() && store && identity.is_some() {
                if cost <= available {
                    new_names.push(name.clone());
                } else {
                    new_names.clear();
                }
            }
            if is_run && name == crate::artifact_access::METADATA_DIR {
                continue;
            }
            let child = path.join(name);
            let metadata = usage.metadata(&child)?;
            if metadata.is_dir() {
                let run_name = name
                    .to_str()
                    .filter(|name| store && path == root && ulid::Ulid::from_string(name).is_ok());
                usage.run_dirs += usize::from(run_name.is_some());
                stack.push((child, run_name.is_some()));
            } else if metadata.is_file() {
                #[cfg(target_os = "linux")]
                watch::StoreWatch::observe(&mut updated.watch, &child, &metadata);
                #[cfg(target_os = "linux")]
                let size = if updated.watch.is_some() {
                    usage.metadata(&child)?.len()
                } else {
                    metadata.len()
                };
                #[cfg(not(target_os = "linux"))]
                let size = metadata.len();
                let old = previous.sizes.remove(&child);
                if old != Some(size) {
                    adjust_usage(&mut usage, root, store, &child, old, Some(size));
                }
                retained_sizes = retained_sizes.saturating_add(child.as_os_str().len() + 96);
                if keep_sizes && retained_sizes <= 16 * 1024 * 1024 {
                    updated.sizes.insert(child, size);
                } else if keep_sizes {
                    keep_sizes = false;
                    updated.sizes.clear();
                }
            }
        }
        if store && let Some(identity) = identity {
            if FileIdentity::from_metadata(&usage.metadata(&path)?).as_ref() != Some(&identity) {
                return Err(std::io::Error::other(
                    "artifact directory changed during accounting",
                ));
            }
            if cost <= available {
                retained_bytes += cost;
                let names = cached_names.unwrap_or_else(|| Arc::from(new_names));
                updated.directories.insert(path, (identity, names));
            }
        }
    }
    for (path, old) in previous.sizes {
        adjust_usage(&mut usage, root, store, &path, Some(old), None);
    }
    usage.runs.retain(|_, (_, files)| *files > 0);
    if keep_sizes {
        updated.usage = usage.clone();
        Ok((usage, Some(updated)))
    } else {
        Ok((usage, None))
    }
}

fn adjust_usage(
    usage: &mut Usage,
    root: &Path,
    store: bool,
    path: &Path,
    old: Option<u64>,
    new: Option<u64>,
) {
    let adjust = |bytes: &mut u64, files: &mut u64| {
        *bytes = bytes
            .saturating_sub(old.unwrap_or(0))
            .saturating_add(new.unwrap_or(0));
        *files = files
            .saturating_sub(u64::from(old.is_some()))
            .saturating_add(u64::from(new.is_some()));
    };
    adjust(&mut usage.bytes, &mut usage.files);
    if store
        && let Ok(relative) = path.strip_prefix(root)
        && relative
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
        && let Some(run) = relative.iter().next()
        && let Some(run) = run
            .to_str()
            .filter(|name| ulid::Ulid::from_string(name).is_ok())
    {
        let (bytes, files) = usage.runs.entry(run.to_owned()).or_default();
        adjust(bytes, files);
    }
}

pub(super) fn accounting_error(error: std::io::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("cannot account for artifact storage: {error}"),
    }
}

pub(super) async fn owned_run(root: &Path, owner: &str) -> bool {
    let marker = root.join(crate::artifact_access::METADATA_DIR).join(owner);
    labby_runtime::path_safety::reject_existing_symlink_ancestors(root, &marker).is_ok()
        && tokio::fs::symlink_metadata(marker)
            .await
            .is_ok_and(|meta| meta.is_file() && meta.len() == 0)
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
    owner: &str,
) -> Result<(), ToolError> {
    admit_with_limits(
        root,
        bytes,
        max_store_bytes,
        MAX_RUN_BYTES,
        MAX_RUN_FILES,
        owner,
    )
    .await
}

// Caller holds the store lock until publication completes or is abandoned.
async fn admit_with_limits(
    root: &Path,
    bytes: usize,
    max_store_bytes: u64,
    max_run_bytes: u64,
    max_run_files: u64,
    owner: &str,
) -> Result<(), ToolError> {
    admit_with_retention(
        root,
        bytes,
        max_store_bytes,
        max_run_bytes,
        max_run_files,
        super::artifact_retention_runs(),
        owner,
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
    owner: &str,
) -> Result<(), ToolError> {
    let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
    let store = root
        .parent()
        .ok_or_else(|| quota_error("artifact run has no store root"))?;
    let inventory = if max_store_bytes > 0 {
        Some(usage(store, true).await?)
    } else {
        None
    };
    let recorded = root
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| inventory.as_ref()?.runs.get(name));
    let (current_bytes, current_files) = if let Some(counts) = recorded {
        *counts
    } else {
        let current = usage(root, false).await?;
        (current.bytes, current.files)
    };
    if current_files >= max_run_files {
        return Err(quota_error(
            "Code Mode artifact file count exceeds the per-run budget",
        ));
    }
    if current_bytes > max_run_bytes || bytes > max_run_bytes.saturating_sub(current_bytes) {
        return Err(quota_error(
            "Code Mode artifact bytes exceed the per-run budget",
        ));
    }
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
    let stored_before = inventory.as_ref().map_or(0, |inventory| inventory.bytes);
    let under_pressure = max_store_bytes > 0
        && (stored_before > max_store_bytes
            || bytes > max_store_bytes.saturating_sub(stored_before));
    let prune = super::prune_artifact_runs_locked(
        store,
        retain,
        inventory
            .as_ref()
            .map(|usage| (pruning_budget, &usage.runs)),
        &active,
        owner,
        root.file_name().and_then(|name| name.to_str()),
    );
    let count_fits = match inventory.as_ref() {
        Some(usage) if retain == 0 || usage.run_dirs < retain => true,
        Some(usage) if usage.run_dirs == retain => {
            recorded.is_some()
                || tokio::fs::try_exists(root)
                    .await
                    .map_err(accounting_error)?
        }
        _ => false,
    };
    let reconciled = if under_pressure {
        prune.await;
        reclaim_inactive(store, max_store_bytes.saturating_sub(bytes), &active, owner).await?;
        true
    } else if count_fits {
        false
    } else {
        super::retention::prune_once(root, prune).await
    };
    // The store lock excludes in-process publication and retention throughout
    // admission. A coalesced pass makes no changes, so the authoritative scan
    // above remains valid until the next reconciliation.
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
    owner: &str,
) -> Result<(), ToolError> {
    let inventory = usage(store, true).await?;
    if inventory.bytes <= budget {
        return Ok(());
    }
    let mut eligible = Vec::new();
    for (name, (bytes, _)) in inventory.runs {
        if !active.contains(&name) && owned_run(&store.join(&name), owner).await {
            eligible.push((name, bytes));
        }
    }
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
