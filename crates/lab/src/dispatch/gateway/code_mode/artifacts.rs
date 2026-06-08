//! Host-brokered artifact writes for Code Mode.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use ulid::Ulid;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{lab_home, redact_home, reject_path_traversal};
use crate::dispatch::path_safety::reject_existing_symlink_ancestors;

const DEFAULT_CONTENT_TYPE: &str = "text/plain";
const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;

/// Default number of per-run artifact directories retained under
/// `$LAB_HOME/code-mode-artifacts/`. Older run directories are pruned on each
/// new run so the on-disk store stays bounded. Override with
/// `LAB_CODE_MODE_ARTIFACT_RETENTION_RUNS`; set it to `0` to disable pruning
/// (unbounded growth).
const DEFAULT_ARTIFACT_RETENTION_RUNS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::dispatch::gateway::code_mode) struct CodeModeArtifactWrite {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub content_type: Option<String>,
}

/// Receipt for one successfully persisted artifact. Construction is
/// deliberately funnelled through [`write_code_mode_artifact`] — the only
/// producer — so `bytes`/`sha256`/`content_type` are always derived together
/// from the same content that was written. Fields are module-visible (not
/// `pub`) so no other code can mint a receipt whose digest/byte-count disagree
/// with reality; serde serializes them into the execution response regardless.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeModeArtifactReceipt {
    pub(in crate::dispatch::gateway::code_mode) path: String,
    pub(in crate::dispatch::gateway::code_mode) absolute_path: String,
    pub(in crate::dispatch::gateway::code_mode) content_type: String,
    pub(in crate::dispatch::gateway::code_mode) bytes: usize,
    pub(in crate::dispatch::gateway::code_mode) sha256: String,
}

fn artifact_store_root() -> PathBuf {
    lab_home().join("code-mode-artifacts")
}

#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn code_mode_artifact_root(run_id: &str) -> PathBuf {
    artifact_store_root().join(run_id)
}

/// Resolve the per-run artifact retention cap from the environment, falling back
/// to [`DEFAULT_ARTIFACT_RETENTION_RUNS`]. `0` disables pruning.
#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn artifact_retention_runs() -> usize {
    std::env::var("LAB_CODE_MODE_ARTIFACT_RETENTION_RUNS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_ARTIFACT_RETENTION_RUNS)
}

/// Best-effort prune of old per-run artifact directories so the store stays
/// bounded. Keeps the newest `retain` run directories (ULID names sort
/// chronologically) and removes the rest.
pub(in crate::dispatch::gateway::code_mode) async fn prune_artifact_runs(retain: usize) {
    prune_artifact_runs_in(&artifact_store_root(), retain).await;
}

/// Core prune over an explicit store root (so tests need no `$LAB_HOME`).
///
/// Only directories whose names parse as ULIDs — i.e. run directories this
/// feature created — are ever considered for removal, so an operator's stray
/// file or directory under the store can never be collected. Errors are
/// swallowed (best-effort, debug-logged); pruning must never fail a run.
pub(in crate::dispatch::gateway::code_mode) async fn prune_artifact_runs_in(
    store_root: &Path,
    retain: usize,
) {
    if retain == 0 {
        return;
    }
    let mut entries = match tokio::fs::read_dir(store_root).await {
        Ok(entries) => entries,
        // Store not created yet (no artifact has ever been written): nothing to prune.
        Err(_) => return,
    };
    let mut run_dirs: Vec<String> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
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
    if run_dirs.len() <= retain {
        return;
    }
    run_dirs.sort(); // ascending: oldest ULID first
    let remove_count = run_dirs.len() - retain;
    for name in run_dirs.into_iter().take(remove_count) {
        let path = store_root.join(&name);
        if let Err(err) = tokio::fs::remove_dir_all(&path).await {
            tracing::debug!(
                surface = "dispatch",
                service = "code_mode",
                action = "code_execute",
                error = %err,
                "failed to prune old code-mode artifact directory"
            );
        }
    }
}

pub(in crate::dispatch::gateway::code_mode) async fn write_code_mode_artifact(
    root: &Path,
    request: &CodeModeArtifactWrite,
) -> Result<CodeModeArtifactReceipt, ToolError> {
    let rel_path = normalize_artifact_path(&request.path)?;
    let bytes = request.content.as_bytes();
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(ToolError::InvalidParam {
            message: format!(
                "artifact content is {} bytes; maximum is {} bytes",
                bytes.len(),
                MAX_ARTIFACT_BYTES
            ),
            param: "content".to_string(),
        });
    }

    let destination = root.join(&rel_path);
    // Defense-in-depth per `reject_path_traversal`'s documented contract: the
    // lexical guard in `normalize_artifact_path` cannot see through symlinks, so
    // confirm the joined destination stays within `root` and that no existing
    // symlinked ancestor redirects the write outside the jail before any
    // directory or file is created.
    reject_existing_symlink_ancestors(root, &destination)?;

    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("failed to create artifact directory: {err}"),
            })?;
    }

    let mut file = tokio::fs::File::create(&destination)
        .await
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create artifact file: {err}"),
        })?;
    file.write_all(bytes).await.map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to write artifact file: {err}"),
    })?;
    file.flush().await.map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to flush artifact file: {err}"),
    })?;

    let sha256 = Sha256::digest(bytes);
    let content_type = request
        .content_type
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_CONTENT_TYPE)
        .to_string();

    Ok(CodeModeArtifactReceipt {
        path: rel_path,
        absolute_path: redact_home(&destination.display().to_string()),
        content_type,
        bytes: bytes.len(),
        sha256: hex::encode(sha256),
    })
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
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err(ToolError::InvalidParam {
            message: "artifact path must be a relative path".to_string(),
            param: "path".to_string(),
        });
    }
    reject_path_traversal(&normalized)?;
    Ok(normalized)
}
