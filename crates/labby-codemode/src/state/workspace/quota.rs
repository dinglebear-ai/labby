use super::*;

impl StateWorkspace {
    pub(crate) async fn write_file(
        &self,
        path: &VirtualPath,
        content: &str,
    ) -> Result<(), ToolError> {
        self.write_with_usage(path, content, &mut None).await
    }

    pub(super) async fn write_with_usage(
        &self,
        path: &VirtualPath,
        content: &str,
        usage: &mut Option<WorkspaceUsage>,
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
        let destination = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        self.reject_existing_symlink_path(&destination).await?;
        if usage.is_none() {
            *usage = Some(workspace_usage(&self.root).await?);
        }
        let projected = self
            .check_write_quota(
                &destination,
                content.len() as u64,
                usage.as_ref().expect("initialized usage"),
            )
            .await?;
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(internal_io("create state directory"))?;
        }
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        self.reject_existing_symlink_path(&destination).await?;

        let tmp = self.create_temp_path().await?;
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
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
        let tmp_metadata = tokio::fs::symlink_metadata(&tmp)
            .await
            .map_err(internal_io("inspect state temp file"))?;
        if !tmp_metadata.is_file() || tmp_metadata.file_type().is_symlink() {
            drop(tokio::fs::remove_file(&tmp).await);
            return Err(ToolError::Sdk {
                sdk_kind: "permission_denied".to_string(),
                message: "state temp path is not a regular file".to_string(),
            });
        }
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        self.reject_existing_symlink_path(&destination).await?;
        tokio::fs::rename(&tmp, &destination)
            .await
            .map_err(internal_io("move state temp file"))?;
        *usage = Some(projected);
        Ok(())
    }

    async fn create_temp_path(&self) -> Result<PathBuf, ToolError> {
        let dir = self.root.join(".labby-state").join("tmp");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(internal_io("create state temp directory"))?;
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &dir)?;
        let metadata = tokio::fs::symlink_metadata(&dir)
            .await
            .map_err(internal_io("inspect state temp directory"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(ToolError::Sdk {
                sdk_kind: "permission_denied".to_string(),
                message: "state temp directory is not a directory".to_string(),
            });
        }
        Ok(dir.join(format!("{}.tmp", ulid::Ulid::new())))
    }
    async fn check_write_quota(
        &self,
        destination: &Path,
        next_file_bytes: u64,
        usage: &WorkspaceUsage,
    ) -> Result<WorkspaceUsage, ToolError> {
        let current_file_bytes = match tokio::fs::metadata(&destination).await {
            Ok(metadata) if metadata.is_file() => metadata.len(),
            Ok(_) | Err(_) => 0,
        };
        let projected = usage
            .bytes
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
        let projected_entries = usage
            .entries
            .saturating_add(missing_entry_count(&self.root, destination).await?);
        if projected_entries > self.limits.max_entries {
            return Err(ToolError::Sdk {
                sdk_kind: "quota_exceeded".to_string(),
                message: format!(
                    "state workspace would have {projected_entries} entries; maximum is {}",
                    self.limits.max_entries
                ),
            });
        }
        Ok(WorkspaceUsage {
            bytes: projected,
            entries: projected_entries,
        })
    }

    /// Preflight a new destination before an operation with an unknown final size.
    pub(crate) async fn check_new_destination_quota(
        &self,
        destination: &Path,
    ) -> Result<(), ToolError> {
        let usage = workspace_usage(&self.root).await?;
        self.check_write_quota(destination, 1, &usage)
            .await
            .map(|_| ())
    }

    pub(crate) async fn enforce_total_bytes(&self) -> Result<(), ToolError> {
        self.enforce_total_limits().await
    }

    pub(super) async fn enforce_total_limits(&self) -> Result<(), ToolError> {
        let usage = workspace_usage(&self.root).await?;
        if usage.bytes > self.limits.max_total_bytes {
            return Err(ToolError::Sdk {
                sdk_kind: "quota_exceeded".to_string(),
                message: format!(
                    "state workspace is {} bytes; maximum is {}",
                    usage.bytes, self.limits.max_total_bytes
                ),
            });
        }
        if usage.entries > self.limits.max_entries {
            return Err(ToolError::Sdk {
                sdk_kind: "quota_exceeded".to_string(),
                message: format!(
                    "state workspace has {} entries; maximum is {}",
                    usage.entries, self.limits.max_entries
                ),
            });
        }
        Ok(())
    }

    pub(super) async fn check_entry_quota_for_path(
        &self,
        destination: &Path,
    ) -> Result<(), ToolError> {
        let usage = workspace_usage(&self.root).await?;
        let projected = usage
            .entries
            .saturating_add(missing_entry_count(&self.root, destination).await?);
        if projected > self.limits.max_entries {
            return Err(ToolError::Sdk {
                sdk_kind: "quota_exceeded".to_string(),
                message: format!(
                    "state workspace would have {projected} entries; maximum is {}",
                    self.limits.max_entries
                ),
            });
        }
        Ok(())
    }
}
pub(super) struct WorkspaceUsage {
    bytes: u64,
    entries: u64,
}

#[cfg(test)]
pub(super) static WORKSPACE_USAGE_SCAN_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
pub(super) static WORKSPACE_USAGE_SCAN_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

async fn workspace_usage(root: &Path) -> Result<WorkspaceUsage, ToolError> {
    #[cfg(test)]
    if WORKSPACE_USAGE_SCAN_ROOT
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .is_some_and(|tracked| tracked == root)
    {
        WORKSPACE_USAGE_SCAN_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    let mut usage = WorkspaceUsage {
        bytes: 0,
        entries: 0,
    };
    let mut stack = vec![root.to_path_buf()];
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
            let path = entry.path();
            let metadata = tokio::fs::symlink_metadata(&path)
                .await
                .map_err(internal_io("read state workspace metadata"))?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if entry.path().strip_prefix(root).is_ok() {
                usage.entries = usage.entries.saturating_add(1);
            }
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                usage.bytes = usage.bytes.saturating_add(metadata.len());
            }
        }
    }
    Ok(usage)
}

async fn missing_entry_count(root: &Path, destination: &Path) -> Result<u64, ToolError> {
    let relative = destination.strip_prefix(root).map_err(|_| ToolError::Sdk {
        sdk_kind: "path_traversal".to_string(),
        message: "state path escapes the workspace".to_string(),
    })?;
    let virtual_path = labby_runtime::path_safety::rel_to_unix_string(relative);
    if is_reserved_metadata_path(&virtual_path) {
        return Ok(0);
    }
    let parts = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut current = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        current.push(part);
        match tokio::fs::symlink_metadata(&current).await {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok((parts.len() - index) as u64);
            }
            Err(err) => return Err(internal_io("read state path metadata")(err)),
        }
    }
    Ok(0)
}

pub(super) async fn cleanup_file_after_quota_error(
    path: &Path,
    original: ToolError,
    label: &str,
) -> Result<(), ToolError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Err(original),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(original),
        Err(err) => Err(ToolError::Sdk {
            sdk_kind: "quota_cleanup_failed".to_string(),
            message: format!("{label} exceeded quota and cleanup failed: {err}"),
        }),
    }
}
