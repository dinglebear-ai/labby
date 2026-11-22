use super::*;

impl StateWorkspace {
    pub(crate) async fn read_json(&self, path: &VirtualPath) -> Result<JsonReadResult, ToolError> {
        let file = self.read_file(path).await?;
        let value = serde_json::from_str(&file.content).map_err(|err| ToolError::InvalidParam {
            message: format!("state file is not valid JSON: {err}"),
            param: "path".to_string(),
        })?;
        Ok(JsonReadResult {
            path: path.as_str().to_string(),
            value,
        })
    }

    pub(crate) async fn write_json(
        &self,
        path: &VirtualPath,
        value: &serde_json::Value,
        pretty: bool,
    ) -> Result<(), ToolError> {
        let mut content = if pretty {
            serde_json::to_string_pretty(value).map_err(serialize_error)?
        } else {
            serde_json::to_string(value).map_err(serialize_error)?
        };
        content.push('\n');
        self.write_file(path, &content).await
    }

    pub(crate) async fn hash_file(
        &self,
        path: &VirtualPath,
        algorithm: &str,
    ) -> Result<HashFileResult, ToolError> {
        if algorithm != "sha256" {
            return Err(ToolError::InvalidParam {
                message: "state hashFile only supports sha256".to_string(),
                param: "algorithm".to_string(),
            });
        }
        let bytes = self.read_file_bytes(path).await?;
        Ok(HashFileResult {
            path: path.as_str().to_string(),
            algorithm: algorithm.to_string(),
            hex: hex::encode(Sha256::digest(&bytes)),
            bytes: bytes.len(),
        })
    }

    pub(crate) async fn detect_file(
        &self,
        path: &VirtualPath,
    ) -> Result<DetectFileResult, ToolError> {
        let bytes = self.read_file_bytes(path).await?;
        let extension = Path::new(path.as_str())
            .extension()
            .map(|value| value.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let text = std::str::from_utf8(&bytes).is_ok();
        let json =
            extension == "json" || serde_json::from_slice::<serde_json::Value>(&bytes).is_ok();
        Ok(DetectFileResult {
            path: path.as_str().to_string(),
            extension,
            text,
            json,
            bytes: bytes.len(),
        })
    }

    async fn read_file_bytes(&self, path: &VirtualPath) -> Result<Vec<u8>, ToolError> {
        let destination = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        self.reject_existing_symlink_path(&destination).await?;
        let file = tokio::fs::File::open(&destination)
            .await
            .map_err(not_found_or_internal("open state file"))?;
        let mut bytes = Vec::new();
        file.take(self.limits.max_file_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(internal_io("read state file"))?;
        if bytes.len() > self.limits.max_file_bytes {
            return Err(ToolError::Sdk {
                sdk_kind: "response_too_large".to_string(),
                message: "state file exceeded max readable bytes".to_string(),
            });
        }
        Ok(bytes)
    }

    pub(crate) async fn archive_create(
        &self,
        source: &VirtualPath,
        destination: &VirtualPath,
    ) -> Result<ArchiveCreateResult, ToolError> {
        if !destination.as_str().ends_with(".tar") {
            return Err(ToolError::InvalidParam {
                message: "state archiveCreate only supports .tar destinations".to_string(),
                param: "destination".to_string(),
            });
        }
        let source_path = self.resolve(source);
        let destination_path = self.resolve(destination);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &source_path)?;
        labby_runtime::path_safety::reject_existing_symlink_ancestors(
            &self.root,
            &destination_path,
        )?;
        self.reject_existing_symlink_path(&source_path).await?;
        self.reject_existing_symlink_path(&destination_path).await?;
        self.check_entry_quota_for_path(&destination_path).await?;
        if let Some(parent) = destination_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(internal_io("create state archive directory"))?;
        }

        let source_metadata = tokio::fs::metadata(&source_path)
            .await
            .map_err(not_found_or_internal("read state archive source metadata"))?;
        let files = if source_metadata.is_file() {
            vec![source.as_str().to_string()]
        } else if source_metadata.is_dir() {
            let tree = self
                .walk_tree(source, self.limits.max_entries as usize)
                .await?;
            if tree.truncated {
                return Err(ToolError::Sdk {
                    sdk_kind: "response_too_large".to_string(),
                    message: "state archive source exceeded max entries".to_string(),
                });
            }
            tree.entries
                .into_iter()
                .filter(|entry| entry.kind == "file")
                .map(|entry| entry.path)
                .collect()
        } else {
            return Err(ToolError::Sdk {
                sdk_kind: "permission_denied".to_string(),
                message: "state archive source kind is not supported".to_string(),
            });
        };

        let root = self.root.clone();
        let source_virtual = source.as_str().to_string();
        let destination_virtual = destination.as_str().to_string();
        let destination_for_blocking = destination_path.clone();
        let entries = tokio::task::spawn_blocking(move || {
            create_tar_archive(
                &root,
                &source_virtual,
                &destination_virtual,
                &destination_for_blocking,
                files,
            )
        })
        .await
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to join state archive creation task: {err}"),
        })??;

        if let Err(err) = self.enforce_total_limits().await {
            cleanup_file_after_quota_error(&destination_path, err, "state archive").await?;
        }

        Ok(ArchiveCreateResult {
            ok: true,
            destination: destination.as_str().to_string(),
            entries,
        })
    }

    pub(crate) async fn archive_list(
        &self,
        path: &VirtualPath,
        limit: usize,
    ) -> Result<ArchiveListResult, ToolError> {
        let limit = normalize_limit(limit);
        let archive_path = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &archive_path)?;
        self.reject_existing_symlink_path(&archive_path).await?;
        let path_string = path.as_str().to_string();
        let (entries, truncated) =
            tokio::task::spawn_blocking(move || list_tar_archive(&archive_path, limit))
                .await
                .map_err(|err| ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: format!("failed to join state archive listing task: {err}"),
                })??;
        Ok(ArchiveListResult {
            path: path_string,
            entries,
            truncated,
        })
    }
}
fn create_tar_archive(
    root: &Path,
    source_virtual: &str,
    destination_virtual: &str,
    destination: &Path,
    files: Vec<String>,
) -> Result<usize, ToolError> {
    let file = std::fs::File::create(destination).map_err(internal_io("create state archive"))?;
    let mut builder = tar::Builder::new(file);
    let mut entries = 0;
    for virtual_path in files {
        if virtual_path == destination_virtual {
            continue;
        }
        let host_path = root.join(&virtual_path);
        reject_sync_symlink(&host_path)?;
        let entry_name = archive_entry_name(source_virtual, &virtual_path)?;
        if entry_name.as_os_str().is_empty() {
            continue;
        }
        builder
            .append_path_with_name(&host_path, &entry_name)
            .map_err(internal_io("append state archive entry"))?;
        entries += 1;
    }
    builder
        .finish()
        .map_err(internal_io("finish state archive"))?;
    Ok(entries)
}

fn archive_entry_name(source_virtual: &str, virtual_path: &str) -> Result<PathBuf, ToolError> {
    let relative = if virtual_path == source_virtual {
        Path::new(virtual_path)
            .file_name()
            .ok_or_else(|| ToolError::InvalidParam {
                message: "state archive source must have a file name".to_string(),
                param: "source".to_string(),
            })?
            .to_string_lossy()
            .to_string()
    } else {
        virtual_path
            .strip_prefix(source_virtual)
            .and_then(|value| value.strip_prefix('/'))
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: "state archive entry escaped source".to_string(),
            })?
            .to_string()
    };
    validate_archive_member_path(&relative)?;
    Ok(PathBuf::from(relative))
}

fn list_tar_archive(path: &Path, limit: usize) -> Result<(Vec<String>, bool), ToolError> {
    let file = std::fs::File::open(path).map_err(not_found_or_internal("open state archive"))?;
    let mut archive = tar::Archive::new(file);
    let mut entries = Vec::new();
    for entry in archive
        .entries()
        .map_err(internal_io("read state archive entries"))?
    {
        let entry = entry.map_err(internal_io("read state archive entry"))?;
        let path = entry
            .path()
            .map_err(internal_io("read state archive entry path"))?;
        let value = labby_runtime::path_safety::rel_to_unix_string(&path);
        validate_archive_member_path(&value)?;
        entries.push(value);
        if entries.len() > limit {
            entries.sort();
            entries.truncate(limit);
            return Ok((entries, true));
        }
    }
    entries.sort();
    Ok((entries, false))
}

fn validate_archive_member_path(path: &str) -> Result<(), ToolError> {
    if path.trim().is_empty() || path.starts_with('/') || has_windows_drive_prefix(path) {
        return Err(ToolError::Sdk {
            sdk_kind: "path_traversal".to_string(),
            message: "state archive member path escapes the workspace".to_string(),
        });
    }
    for component in Path::new(path).components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ToolError::Sdk {
                    sdk_kind: "path_traversal".to_string(),
                    message: "state archive member path escapes the workspace".to_string(),
                });
            }
        }
    }
    Ok(())
}

fn has_windows_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn reject_sync_symlink(path: &Path) -> Result<(), ToolError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(not_found_or_internal("read state path metadata"))?;
    if metadata.file_type().is_symlink() {
        return Err(ToolError::Sdk {
            sdk_kind: "permission_denied".to_string(),
            message: "state archive rejected a symlink".to_string(),
        });
    }
    Ok(())
}
