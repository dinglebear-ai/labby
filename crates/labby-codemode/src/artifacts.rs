//! Host-brokered artifact writes for Code Mode.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use futures::stream::{self, StreamExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ulid::Ulid;

use crate::error::ToolError;
use crate::util::{lab_home, redact_home};
use labby_runtime::path_safety::reject_existing_symlink_ancestors;
use labby_runtime::path_safety::reject_path_traversal;

const DEFAULT_CONTENT_TYPE: &str = "text/plain";
mod budget;
mod config;
mod publication;
pub use config::install_artifact_config_defaults;
pub(crate) use config::{artifact_max_bytes, artifact_max_store_bytes, artifact_retention_runs};
mod read;
mod retention;
pub use read::read_receipted_artifact;

/// Upper bound on the `content_type` metadata string.
///
/// This is the one artifact field that *does* reach the model: unlike `content`
/// (written to disk, never returned), `content_type` rides the receipt back into
/// the execution response and the truncation marker. So it gets a context-bound
/// cap; a snippet can't bloat the response with a megabyte `contentType`.
const MAX_CONTENT_TYPE_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CodeModeArtifactWrite {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub content_type: Option<String>,
}

/// Receipt for one successfully persisted artifact. `bytes`/`sha256`/
/// `content_type` are always derived together from the same content that was
/// written. Fields are module-visible (not `pub`), so no code outside the
/// `code_mode` module can mint a receipt; within the module,
/// [`write_code_mode_artifact`] is by convention the sole producer, which keeps
/// the digest and byte-count honest. serde serializes the fields into the
/// execution response regardless of their visibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CodeModeArtifactReceipt {
    /// Opaque retrieval identifier; absent on legacy or restricted writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) artifact_id: Option<String>,
    pub(crate) path: String,
    pub(crate) absolute_path: String,
    pub(crate) content_type: String,
    pub(crate) bytes: usize,
    pub(crate) sha256: String,
}

impl CodeModeArtifactReceipt {
    /// Opaque storage identity derived from the broker-minted receipt. Authorize
    /// the execution before using it to resolve retained artifact bytes.
    #[must_use]
    pub fn storage_run_id(&self) -> Option<String> {
        let suffix = format!("/{}", self.path);
        let absolute = self.absolute_path.replace('\\', "/");
        let parent = absolute.strip_suffix(&suffix)?;
        let (store, id) = parent.rsplit_once('/')?;
        if !store.ends_with("/code-mode-artifacts")
            || id.is_empty()
            || !id.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return None;
        }
        Some(id.to_owned())
    }
}

fn artifact_store_root() -> PathBuf {
    lab_home().join("code-mode-artifacts")
}

#[must_use]
pub(crate) fn code_mode_artifact_root(run_id: &str) -> PathBuf {
    artifact_store_root().join(run_id)
}

/// Best-effort recursive byte size of a directory. Symlinks are not followed
/// (`file_type()` does not traverse them), so the count can never wander outside
/// the store. Unreadable entries are skipped — this only feeds a retention
/// heuristic, never a correctness decision.
async fn dir_size_bytes(path: PathBuf) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![path];
    while let Some(dir) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            match entry.file_type().await {
                Ok(ft) if ft.is_dir() => stack.push(entry.path()),
                Ok(ft) if ft.is_file() => {
                    if let Ok(meta) = entry.metadata().await {
                        total = total.saturating_add(meta.len());
                    }
                }
                _ => {}
            }
        }
    }
    total
}

/// Process-global set of run ids whose execution is still in flight.
///
/// The artifact store is shared across all concurrent Code Mode executions, and
/// pruning runs on the first write of *any* run. Without this set, a run with a
/// low `retain` could `remove_dir_all` a *different* concurrent run's directory
/// while that run is still writing into it. Membership here makes a run's
/// directory un-prunable for as long as it is executing — see
/// [`ActiveArtifactRun`].
fn active_runs() -> &'static Mutex<HashSet<String>> {
    static ACTIVE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Snapshot the currently-active run ids so a prune pass can exclude them.
pub(crate) fn active_artifact_runs_snapshot() -> HashSet<String> {
    active_runs()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// RAII registration of an in-flight run id. Construct once per execution and
/// hold it for the whole run; `Drop` removes the id so the directory becomes
/// eligible for pruning only after the run has finished.
pub(crate) struct ActiveArtifactRun {
    run_id: String,
}

impl ActiveArtifactRun {
    pub(crate) fn register(run_id: &str) -> Self {
        active_runs()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(run_id.to_string());
        Self {
            run_id: run_id.to_string(),
        }
    }
}

impl Drop for ActiveArtifactRun {
    fn drop(&mut self) {
        active_runs()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.run_id);
    }
}

/// Core prune over an explicit store root (so tests need no `$LABBY_HOME`).
///
/// Removes the oldest run directories that fall outside *either* the run-count
/// cap (`retain`, newest-N) *or* the total-byte budget (`max_store_bytes`,
/// newest-fits-first). `retain == 0` disables the count rule and
/// `max_store_bytes == 0` disables the byte rule; with both off this is a no-op.
///
/// Only directories whose names parse as ULIDs — i.e. run directories this
/// feature created — are ever considered for removal, so an operator's stray
/// file or directory under the store can never be collected. Run ids in
/// `active` are skipped unconditionally (even past either limit) so a concurrent
/// run's directory is never deleted while it is still writing. Errors are
/// swallowed (best-effort, debug-logged); pruning must never fail a run.
async fn prune_artifact_runs_locked(
    store_root: &Path,
    retain: usize,
    max_store_bytes: u64,
    active: &HashSet<String>,
) {
    let count_pruning = retain > 0;
    let byte_pruning = max_store_bytes > 0;
    if !count_pruning && !byte_pruning {
        return;
    }
    let mut entries = match tokio::fs::read_dir(store_root).await {
        Ok(entries) => entries,
        // Store not created yet (no artifact has ever been written): nothing to prune.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        // Any other read failure (EACCES, EIO, store replaced by a file, …)
        // disables retention for this run; surface it so unbounded growth is
        // diagnosable rather than silent.
        Err(err) => {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "codemode",
                error = %err,
                "code-mode artifact retention disabled: cannot read store directory"
            );
            return;
        }
    };
    let mut run_dirs: Vec<String> = Vec::new();
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            // A mid-enumeration failure can leave `run_dirs` short and skip
            // pruning entirely; log it so under-pruning isn't silent.
            Err(err) => {
                tracing::warn!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "codemode",
                    error = %err,
                    "code-mode artifact retention: store enumeration interrupted; store may be under-pruned"
                );
                break;
            }
        };
        let is_dir = entry
            .file_type()
            .await
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if Ulid::from_string(&name).is_ok() {
            run_dirs.push(name);
        }
    }
    run_dirs.sort(); // ascending: oldest ULID first
    let newest_first: Vec<String> = run_dirs.iter().rev().cloned().collect();

    // When byte-pruning is on, size every run directory concurrently up front —
    // the walks are independent — instead of serializing them inside the
    // decision loop below.
    let sizes: Vec<u64> = if byte_pruning {
        const SIZE_WALK_CONCURRENCY: usize = 8;
        stream::iter(newest_first.iter().cloned().map(|name| {
            let path = store_root.join(name);
            async move { dir_size_bytes(path).await }
        }))
        .buffered(SIZE_WALK_CONCURRENCY)
        .collect()
        .await
    } else {
        Vec::new()
    };

    // Active payloads have priority regardless of ULID order. Reserve their
    // bytes before selecting inactive runs, so a newer inactive run cannot
    // consume space that an older protected execution already occupies.
    let mut cumulative: u64 = if byte_pruning {
        newest_first
            .iter()
            .zip(&sizes)
            .filter(|(name, _)| active.contains(*name))
            .map(|(_, bytes)| *bytes)
            .fold(0, u64::saturating_add)
    } else {
        0
    };
    let mut to_remove: Vec<String> = Vec::new();
    for (idx, name) in newest_first.iter().enumerate() {
        if active.contains(name) {
            continue;
        }
        let candidate = if byte_pruning {
            cumulative.saturating_add(sizes[idx])
        } else {
            cumulative
        };
        let within_count = !count_pruning || idx < retain;
        let within_bytes = !byte_pruning || candidate <= max_store_bytes;
        if within_count && within_bytes {
            cumulative = candidate;
        } else {
            to_remove.push(name.clone());
        }
    }

    for name in to_remove {
        let path = store_root.join(&name);
        if let Err(err) = tokio::fs::remove_dir_all(&path).await {
            tracing::debug!(
                surface = "dispatch",
                service = "code_mode",
                action = "codemode",
                error = %err,
                "failed to prune old code-mode artifact directory"
            );
        }
    }
}

pub(crate) async fn write_code_mode_artifact(
    root: &Path,
    request: &CodeModeArtifactWrite,
    max_bytes: usize,
) -> Result<CodeModeArtifactReceipt, ToolError> {
    let rel_path = normalize_artifact_path(&request.path)?;
    if rel_path.split('/').next() == Some(crate::artifact_access::METADATA_DIR) {
        return Err(ToolError::InvalidParam {
            message: "artifact path uses a reserved metadata directory".into(),
            param: "path".into(),
        });
    }
    let content_type = normalize_content_type(request.content_type.as_deref())?;
    let bytes = request.content.as_bytes();
    if bytes.len() > max_bytes {
        return Err(ToolError::InvalidParam {
            message: format!(
                "artifact content is {} bytes; maximum is {max_bytes} bytes",
                bytes.len(),
            ),
            param: "content".to_string(),
        });
    }

    let destination = root.join(&rel_path);
    reject_existing_symlink_ancestors(root, &destination)?;
    let _reservation = budget::STORE_MUTATION.lock().await;
    budget::admit(root, bytes.len(), artifact_max_store_bytes()).await?;

    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("failed to create artifact directory: {err}"),
            })?;
    }
    reject_existing_symlink_ancestors(root, &destination)?;

    publication::publish(&destination, bytes).await?;

    let sha256 = Sha256::digest(bytes);

    Ok(CodeModeArtifactReceipt {
        artifact_id: None,
        path: rel_path,
        absolute_path: redact_home(&destination.display().to_string()),
        content_type,
        bytes: bytes.len(),
        sha256: hex::encode(sha256),
    })
}

/// Normalize and validate the artifact receipt `content_type`.
///
/// The receipt (and the truncation marker) carry this string into the model's
/// context, so unlike the on-disk content it needs a small fixed cap and a
/// conservative media-type grammar.
fn normalize_content_type(content_type: Option<&str>) -> Result<String, ToolError> {
    let Some(value) = content_type else {
        return Ok(DEFAULT_CONTENT_TYPE.to_string());
    };
    if value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(invalid_content_type(
            "artifact content_type must not contain ASCII control characters",
        ));
    }

    let trimmed = value.trim_matches(' ');
    if trimmed.is_empty() {
        return Ok(DEFAULT_CONTENT_TYPE.to_string());
    }
    if trimmed.len() > MAX_CONTENT_TYPE_BYTES {
        return Err(ToolError::InvalidParam {
            message: format!(
                "artifact content_type is {} bytes; maximum is {MAX_CONTENT_TYPE_BYTES} bytes",
                trimmed.len(),
            ),
            param: "content_type".to_string(),
        });
    }
    if !trimmed.is_ascii() {
        return Err(invalid_content_type(
            "artifact content_type must be ASCII type/subtype",
        ));
    }
    if trimmed.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(invalid_content_type(
            "artifact content_type must not contain embedded whitespace",
        ));
    }

    let Some((media_type, subtype)) = trimmed.split_once('/') else {
        return Err(invalid_content_type(
            "artifact content_type must use type/subtype syntax",
        ));
    };
    if media_type.is_empty() || subtype.is_empty() || subtype.contains('/') {
        return Err(invalid_content_type(
            "artifact content_type must use type/subtype syntax",
        ));
    }
    if !media_type.bytes().all(is_content_type_token_char)
        || !subtype.bytes().all(is_content_type_token_char)
    {
        return Err(invalid_content_type(
            "artifact content_type must contain only token characters",
        ));
    }

    Ok(trimmed.to_string())
}

fn invalid_content_type(message: impl Into<String>) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: "content_type".to_string(),
    }
}

fn is_content_type_token_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
        )
}

fn normalize_artifact_path(raw: &str) -> Result<String, ToolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "artifact path must be a non-empty relative path".to_string(),
            param: "path".to_string(),
        });
    }
    // Normalize Windows-style separators to `/` BEFORE the lexical guards below.
    // On Unix a backslash is an ordinary filename byte, so `a\..\..\etc\evil`
    // would pass `is_absolute`/`reject_path_traversal` as a single innocent
    // component and only afterwards (when the receipt path is built) turn into
    // real `../` separators that escape the jail. Converting first makes the
    // guards see exactly the separators the filesystem will.
    let normalized = trimmed.replace('\\', "/");
    if is_rooted_or_drive_absolute(&normalized) {
        return Err(ToolError::InvalidParam {
            message: "artifact path must be a relative path".to_string(),
            param: "path".to_string(),
        });
    }
    reject_path_traversal(&normalized)?;
    Ok(normalized)
}

/// Cross-platform "is this artifact path rooted or absolute?" check.
///
/// `Path::is_absolute()` is platform-dependent: on Windows a POSIX-rooted path
/// like `/etc/evil` (or `\etc\evil`, which we normalize to `/etc/evil`) is NOT
/// absolute because it lacks a drive prefix, so it would slip past the guard and
/// only later be caught — with the wrong error kind — by `reject_path_traversal`.
/// We operate on the already-`\`->`/`-normalized string so the rule is identical
/// on every OS: reject a leading `/`, a Windows drive prefix (`C:`), or anything
/// the current platform already considers absolute.
fn is_rooted_or_drive_absolute(normalized: &str) -> bool {
    normalized.starts_with('/')
        || has_windows_drive_prefix(normalized)
        || Path::new(normalized).is_absolute()
}

/// True when the path begins with a Windows drive-letter prefix such as `C:` —
/// covering both drive-absolute (`C:/foo`) and drive-relative (`C:foo`) forms,
/// neither of which is a valid jailed relative artifact path on any platform.
fn has_windows_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}
