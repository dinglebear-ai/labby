use std::path::{Component, Path, PathBuf};
#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::ToolError;

use super::path::{VirtualPath, is_reserved_metadata_path};
use super::quota::StateWorkspaceLimits;

mod content;
mod discovery;
#[cfg(test)]
mod edit_archive_tests;
mod edits;
mod quota;
#[cfg(test)]
mod tests;

use quota::cleanup_file_after_quota_error;

#[derive(Debug, Clone)]
pub(crate) struct StateWorkspace {
    root: PathBuf,
    limits: StateWorkspaceLimits,
}

struct WalkFilesResult {
    files: Vec<String>,
    truncated: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ReadFileResult {
    pub(crate) path: String,
    pub(crate) content: String,
    pub(crate) bytes: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ListResult {
    pub(crate) entries: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MutationResult {
    pub(crate) ok: bool,
    pub(crate) path: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExistsResult {
    pub(crate) path: String,
    pub(crate) exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StatResult {
    pub(crate) path: String,
    pub(crate) kind: String,
    pub(crate) bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WalkEntry {
    pub(crate) path: String,
    pub(crate) kind: String,
    pub(crate) bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WalkTreeResult {
    pub(crate) entries: Vec<WalkEntry>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct JsonReadResult {
    pub(crate) path: String,
    pub(crate) value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HashFileResult {
    pub(crate) path: String,
    pub(crate) algorithm: String,
    pub(crate) hex: String,
    pub(crate) bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DetectFileResult {
    pub(crate) path: String,
    pub(crate) extension: String,
    pub(crate) text: bool,
    pub(crate) json: bool,
    pub(crate) bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ArchiveCreateResult {
    pub(crate) ok: bool,
    pub(crate) destination: String,
    pub(crate) entries: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ArchiveListResult {
    pub(crate) path: String,
    pub(crate) entries: Vec<String>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GlobResult {
    pub(crate) matches: Vec<String>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SearchMatch {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SearchFilesResult {
    pub(crate) skipped: usize,
    pub(crate) matches: Vec<SearchMatch>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReplaceInFilesResult {
    pub(crate) skipped: usize,
    pub(crate) changed: Vec<String>,
    pub(crate) dry_run: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct FileEdit {
    pub(crate) path: String,
    pub(crate) search: String,
    pub(crate) replace: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct EditPlanResult {
    pub(crate) plan_id: String,
    pub(crate) edits: Vec<FileEdit>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ApplyEditPlanResult {
    pub(crate) ok: bool,
    pub(crate) changed: Vec<String>,
}

pub(crate) fn default_search_limit() -> usize {
    200
}

pub(crate) fn default_true() -> bool {
    true
}

impl StateWorkspace {
    pub(crate) fn new(root: PathBuf, limits: StateWorkspaceLimits) -> Result<Self, ToolError> {
        std::fs::create_dir_all(&root).map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create code mode workspace root: {err}"),
        })?;
        Ok(Self { root, limits })
    }

    pub(crate) fn root_path(&self) -> &PathBuf {
        &self.root
    }

    fn resolve(&self, path: &VirtualPath) -> PathBuf {
        self.root.join(path.as_str())
    }

    async fn reject_existing_symlink_path(&self, path: &Path) -> Result<(), ToolError> {
        match tokio::fs::symlink_metadata(path).await {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(ToolError::Sdk {
                sdk_kind: "permission_denied".to_string(),
                message: "state path is denied because it is a symlink".to_string(),
            }),
            Ok(_) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(internal_io("read state path metadata")(err)),
        }
    }
}

impl StateWorkspace {
    pub(crate) async fn read_file(&self, path: &VirtualPath) -> Result<ReadFileResult, ToolError> {
        let destination = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        self.reject_existing_symlink_path(&destination).await?;
        let file = tokio::fs::File::open(&destination)
            .await
            .map_err(not_found_or_internal("open state file"))?;
        let mut content = String::new();
        file.take(self.limits.max_result_bytes as u64 + 1)
            .read_to_string(&mut content)
            .await
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::InvalidData => ToolError::Sdk {
                    sdk_kind: "invalid_data".to_string(),
                    message: "state file is not readable UTF-8 text".to_string(),
                },
                _ => internal_io("read state file")(err),
            })?;
        if content.len() > self.limits.max_result_bytes {
            return Err(ToolError::Sdk {
                sdk_kind: "response_too_large".to_string(),
                message: "state read result exceeded max result bytes".to_string(),
            });
        }
        Ok(ReadFileResult {
            path: path.as_str().to_string(),
            bytes: content.len(),
            content,
        })
    }

    pub(crate) async fn append_file(
        &self,
        path: &VirtualPath,
        content: &str,
    ) -> Result<MutationResult, ToolError> {
        let existing = match self.read_file(path).await {
            Ok(file) => file.content,
            Err(err) if err.kind() == "not_found" => String::new(),
            Err(err) => return Err(err),
        };
        let next = format!("{existing}{content}");
        self.write_file(path, &next).await?;
        Ok(MutationResult {
            ok: true,
            path: path.as_str().to_string(),
        })
    }

    pub(crate) async fn exists(&self, path: &VirtualPath) -> Result<ExistsResult, ToolError> {
        let destination = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        self.reject_existing_symlink_path(&destination).await?;
        let exists = match tokio::fs::metadata(&destination).await {
            Ok(_) => true,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => return Err(internal_io("read state path metadata")(err)),
        };
        Ok(ExistsResult {
            path: path.as_str().to_string(),
            exists,
        })
    }

    pub(crate) async fn stat(&self, path: &VirtualPath) -> Result<StatResult, ToolError> {
        let destination = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        self.reject_existing_symlink_path(&destination).await?;
        let metadata = tokio::fs::metadata(&destination)
            .await
            .map_err(not_found_or_internal("read state path metadata"))?;
        let kind = if metadata.is_file() {
            "file"
        } else if metadata.is_dir() {
            "directory"
        } else {
            return Err(ToolError::Sdk {
                sdk_kind: "permission_denied".to_string(),
                message: "state path kind is not supported".to_string(),
            });
        };
        Ok(StatResult {
            path: path.as_str().to_string(),
            kind: kind.to_string(),
            bytes: metadata.len(),
        })
    }

    pub(crate) async fn mkdir(&self, path: &VirtualPath) -> Result<MutationResult, ToolError> {
        let destination = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        self.reject_existing_symlink_path(&destination).await?;
        self.check_entry_quota_for_path(&destination).await?;
        tokio::fs::create_dir_all(&destination)
            .await
            .map_err(internal_io("create state directory"))?;
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        self.reject_existing_symlink_path(&destination).await?;
        Ok(MutationResult {
            ok: true,
            path: path.as_str().to_string(),
        })
    }

    pub(crate) async fn remove(
        &self,
        path: &VirtualPath,
        recursive: bool,
    ) -> Result<MutationResult, ToolError> {
        if is_reserved_metadata_path(path.as_str()) {
            return Err(ToolError::Sdk {
                sdk_kind: "permission_denied".to_string(),
                message: "state metadata paths cannot be removed".to_string(),
            });
        }
        let destination = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        self.reject_existing_symlink_path(&destination).await?;
        let metadata = tokio::fs::metadata(&destination)
            .await
            .map_err(not_found_or_internal("read state path metadata"))?;
        if metadata.is_file() {
            tokio::fs::remove_file(&destination)
                .await
                .map_err(internal_io("remove state file"))?;
        } else if metadata.is_dir() {
            if recursive {
                tokio::fs::remove_dir_all(&destination)
                    .await
                    .map_err(internal_io("remove state directory tree"))?;
            } else {
                tokio::fs::remove_dir(&destination)
                    .await
                    .map_err(internal_io("remove state directory"))?;
            }
        } else {
            return Err(ToolError::Sdk {
                sdk_kind: "permission_denied".to_string(),
                message: "state path kind is not supported".to_string(),
            });
        }
        Ok(MutationResult {
            ok: true,
            path: path.as_str().to_string(),
        })
    }

    pub(crate) async fn copy(
        &self,
        from: &VirtualPath,
        to: &VirtualPath,
    ) -> Result<MutationResult, ToolError> {
        let source = self.read_file(from).await?;
        self.write_file(to, &source.content).await?;
        Ok(MutationResult {
            ok: true,
            path: to.as_str().to_string(),
        })
    }

    pub(crate) async fn move_path(
        &self,
        from: &VirtualPath,
        to: &VirtualPath,
    ) -> Result<MutationResult, ToolError> {
        let source = self.resolve(from);
        let destination = self.resolve(to);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &source)?;
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        self.reject_existing_symlink_path(&source).await?;
        self.reject_existing_symlink_path(&destination).await?;
        self.check_entry_quota_for_path(&destination).await?;
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(internal_io("create state move directory"))?;
        }
        tokio::fs::rename(&source, &destination)
            .await
            .map_err(not_found_or_internal("move state path"))?;
        Ok(MutationResult {
            ok: true,
            path: to.as_str().to_string(),
        })
    }
}
fn internal_io(action: &'static str) -> impl FnOnce(std::io::Error) -> ToolError {
    move |err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to {action}: {err}"),
    }
}

fn not_found_or_internal(action: &'static str) -> impl FnOnce(std::io::Error) -> ToolError {
    move |err| ToolError::Sdk {
        sdk_kind: if err.kind() == std::io::ErrorKind::NotFound {
            "not_found"
        } else {
            "internal_error"
        }
        .to_string(),
        message: format!("failed to {action}: {err}"),
    }
}

fn normalize_limit(limit: usize) -> usize {
    limit.clamp(1, 10_000)
}

fn serialize_error(err: serde_json::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to serialize state value: {err}"),
    }
}
