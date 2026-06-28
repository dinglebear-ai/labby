use std::path::PathBuf;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::ToolError;

use super::path::VirtualPath;
use super::quota::StateWorkspaceLimits;

#[derive(Debug, Clone)]
pub(crate) struct StateWorkspace {
    root: PathBuf,
    limits: StateWorkspaceLimits,
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

impl StateWorkspace {
    pub(crate) fn new(root: PathBuf, limits: StateWorkspaceLimits) -> Result<Self, ToolError> {
        std::fs::create_dir_all(&root).map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create code mode workspace root: {err}"),
        })?;
        Ok(Self { root, limits })
    }

    fn resolve(&self, path: &VirtualPath) -> PathBuf {
        self.root.join(path.as_str())
    }

    pub(crate) async fn write_file(
        &self,
        path: &VirtualPath,
        content: &str,
    ) -> Result<(), ToolError> {
        if content.len() > self.limits.max_file_bytes {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "state file content is {} bytes; maximum is {}",
                    content.len(),
                    self.limits.max_file_bytes
                ),
                param: "content".to_string(),
            });
        }
        self.check_total_bytes_after_write(path, content.len() as u64)
            .await?;

        let destination = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(internal_io("create state directory"))?;
        }
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;

        let tmp = destination.with_extension("tmp-labby-state");
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)
            .await
            .map_err(internal_io("create state temp file"))?;
        file.write_all(content.as_bytes())
            .await
            .map_err(internal_io("write state temp file"))?;
        file.flush()
            .await
            .map_err(internal_io("flush state temp file"))?;
        drop(file);
        tokio::fs::rename(&tmp, &destination)
            .await
            .map_err(internal_io("move state temp file"))?;
        Ok(())
    }

    async fn check_total_bytes_after_write(
        &self,
        path: &VirtualPath,
        next_file_bytes: u64,
    ) -> Result<(), ToolError> {
        let destination = self.resolve(path);
        let current_file_bytes = match tokio::fs::metadata(&destination).await {
            Ok(metadata) if metadata.is_file() => metadata.len(),
            Ok(_) | Err(_) => 0,
        };
        let total = workspace_total_bytes(&self.root).await?;
        let projected = total
            .saturating_sub(current_file_bytes)
            .saturating_add(next_file_bytes);
        if projected > self.limits.max_total_bytes {
            return Err(ToolError::Sdk {
                sdk_kind: "quota_exceeded".to_string(),
                message: format!(
                    "state workspace would be {projected} bytes; maximum is {}",
                    self.limits.max_total_bytes
                ),
            });
        }
        Ok(())
    }

    pub(crate) async fn read_file(&self, path: &VirtualPath) -> Result<ReadFileResult, ToolError> {
        let destination = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        let file = tokio::fs::File::open(&destination)
            .await
            .map_err(not_found_or_internal("open state file"))?;
        let mut content = String::new();
        file.take(self.limits.max_result_bytes as u64 + 1)
            .read_to_string(&mut content)
            .await
            .map_err(internal_io("read state file"))?;
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

    pub(crate) async fn list(&self, path: &VirtualPath) -> Result<ListResult, ToolError> {
        let dir = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &dir)?;
        let mut read_dir = tokio::fs::read_dir(&dir)
            .await
            .map_err(not_found_or_internal("read state directory"))?;
        let mut entries = Vec::new();
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(internal_io("read state directory entry"))?
        {
            entries.push(entry.file_name().to_string_lossy().to_string());
            if entries.len() as u64 > self.limits.max_entries {
                return Err(ToolError::Sdk {
                    sdk_kind: "response_too_large".to_string(),
                    message: "state list exceeded max entries".to_string(),
                });
            }
        }
        entries.sort();
        Ok(ListResult { entries })
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

async fn workspace_total_bytes(root: &PathBuf) -> Result<u64, ToolError> {
    let mut total = 0_u64;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let mut read_dir = match tokio::fs::read_dir(&dir).await {
            Ok(read_dir) => read_dir,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(internal_io("scan state workspace")(error)),
        };
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(internal_io("scan state workspace entry"))?
        {
            let metadata = entry
                .metadata()
                .await
                .map_err(internal_io("read state workspace metadata"))?;
            if metadata.is_dir() {
                stack.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::quota::StateWorkspaceLimits;

    #[tokio::test]
    async fn workspace_writes_reads_and_reopens() {
        let temp = tempfile::tempdir().unwrap();
        let ws = StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
            .unwrap();
        ws.write_file(
            &VirtualPath::parse("/src/app.rs").unwrap(),
            "fn main() {}\n",
        )
        .await
        .unwrap();
        assert_eq!(
            ws.read_file(&VirtualPath::parse("src/app.rs").unwrap())
                .await
                .unwrap()
                .content,
            "fn main() {}\n"
        );
        let ws2 = StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
            .unwrap();
        assert_eq!(
            ws2.list(&VirtualPath::parse("src").unwrap())
                .await
                .unwrap()
                .entries
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn workspace_rejects_large_writes_and_reads() {
        let temp = tempfile::tempdir().unwrap();
        let limits = StateWorkspaceLimits {
            max_file_bytes: 4,
            max_result_bytes: 4,
            ..StateWorkspaceLimits::default()
        };
        let ws = StateWorkspace::new(temp.path().to_path_buf(), limits).unwrap();
        let err = ws
            .write_file(&VirtualPath::parse("too-big.txt").unwrap(), "12345")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "invalid_param");

        std::fs::write(temp.path().join("existing.txt"), "12345").unwrap();
        let err = ws
            .read_file(&VirtualPath::parse("existing.txt").unwrap())
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "response_too_large");
    }

    #[tokio::test]
    async fn workspace_enforces_total_byte_limit() {
        let temp = tempfile::tempdir().unwrap();
        let limits = StateWorkspaceLimits {
            max_file_bytes: 10,
            max_total_bytes: 6,
            ..StateWorkspaceLimits::default()
        };
        let ws = StateWorkspace::new(temp.path().to_path_buf(), limits).unwrap();
        ws.write_file(&VirtualPath::parse("a.txt").unwrap(), "1234")
            .await
            .unwrap();
        let err = ws
            .write_file(&VirtualPath::parse("b.txt").unwrap(), "1234")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "quota_exceeded");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn workspace_rejects_symlink_ancestors() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let ws = StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join("link")).unwrap();
        let err = ws
            .write_file(&VirtualPath::parse("link/file.txt").unwrap(), "x")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "symlink_rejected");
    }
}
