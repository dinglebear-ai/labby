use super::*;

impl StateWorkspace {
    pub(crate) async fn walk_tree(
        &self,
        path: &VirtualPath,
        limit: usize,
    ) -> Result<WalkTreeResult, ToolError> {
        let limit = normalize_limit(limit);
        let start = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &start)?;
        self.reject_existing_symlink_path(&start).await?;
        let mut entries = Vec::new();
        let mut stack = vec![start];
        while let Some(dir) = stack.pop() {
            let mut read_dir = tokio::fs::read_dir(&dir)
                .await
                .map_err(not_found_or_internal("read state directory"))?;
            while let Some(entry) = read_dir
                .next_entry()
                .await
                .map_err(internal_io("read state directory entry"))?
            {
                let path = entry.path();
                let relative = match path.strip_prefix(&self.root) {
                    Ok(relative) => relative,
                    Err(_) => continue,
                };
                let virtual_path = labby_runtime::path_safety::rel_to_unix_string(relative);
                if is_reserved_metadata_path(&virtual_path) {
                    continue;
                }
                let metadata = tokio::fs::symlink_metadata(&path)
                    .await
                    .map_err(internal_io("read state workspace metadata"))?;
                if metadata.file_type().is_symlink() {
                    return Err(ToolError::Sdk {
                        sdk_kind: "permission_denied".to_string(),
                        message: "state walk rejected a symlink".to_string(),
                    });
                }
                let kind = if metadata.is_dir() {
                    stack.push(path);
                    "directory"
                } else if metadata.is_file() {
                    "file"
                } else {
                    return Err(ToolError::Sdk {
                        sdk_kind: "permission_denied".to_string(),
                        message: "state path kind is not supported".to_string(),
                    });
                };
                entries.push(WalkEntry {
                    path: virtual_path,
                    kind: kind.to_string(),
                    bytes: metadata.len(),
                });
                if entries.len() > limit {
                    entries.sort_by(|left, right| left.path.cmp(&right.path));
                    entries.truncate(limit);
                    return Ok(WalkTreeResult {
                        entries,
                        truncated: true,
                    });
                }
            }
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(WalkTreeResult {
            entries,
            truncated: false,
        })
    }

    pub(crate) async fn list(&self, path: &VirtualPath) -> Result<ListResult, ToolError> {
        let dir = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &dir)?;
        self.reject_existing_symlink_path(&dir).await?;
        let mut read_dir = tokio::fs::read_dir(&dir)
            .await
            .map_err(not_found_or_internal("read state directory"))?;
        let mut entries = Vec::new();
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(internal_io("read state directory entry"))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let child_path = if path.as_str().is_empty() {
                name.clone()
            } else {
                format!("{}/{}", path.as_str(), name)
            };
            if is_reserved_metadata_path(&child_path) {
                continue;
            }
            entries.push(name);
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

    pub(crate) async fn glob(&self, pattern: &str, limit: usize) -> Result<GlobResult, ToolError> {
        let limit = normalize_limit(limit);
        let matcher = glob_pattern_regex(pattern)?;
        let walked = self.walk_files(self.limits.max_entries as usize).await?;
        let mut files = walked.files;
        files.sort();
        let mut matches = Vec::new();
        for file in files {
            if matcher.is_match(&file) {
                matches.push(file);
                if matches.len() > limit {
                    matches.truncate(limit);
                    return Ok(GlobResult {
                        matches,
                        truncated: true,
                    });
                }
            }
        }
        Ok(GlobResult {
            matches,
            truncated: walked.truncated,
        })
    }

    pub(super) async fn read_glob_text(
        &self,
        path: &str,
    ) -> Result<Option<(VirtualPath, String)>, ToolError> {
        let path = match VirtualPath::parse(path) {
            Ok(path) => path,
            Err(err) if err.kind() == "permission_denied" => return Ok(None),
            Err(err) => return Err(err),
        };
        match self.read_file(&path).await {
            Ok(file) => Ok(Some((path, file.content))),
            Err(err) if matches!(err.kind(), "response_too_large" | "invalid_data") => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub(crate) async fn search_files(
        &self,
        pattern: &str,
        query: &str,
        limit: usize,
    ) -> Result<SearchFilesResult, ToolError> {
        if query.is_empty() {
            return Err(ToolError::InvalidParam {
                message: "state search query must not be empty".to_string(),
                param: "query".to_string(),
            });
        }
        let limit = normalize_limit(limit);
        let glob = self.glob(pattern, self.limits.max_entries as usize).await?;
        let mut matches = Vec::new();
        let mut skipped = 0;
        let mut result_bytes = 2usize; // JSON array brackets
        for path in glob.matches {
            let Some((_, content)) = self.read_glob_text(&path).await? else {
                skipped += 1;
                continue;
            };
            for (index, line) in content.lines().enumerate() {
                if line.contains(query) {
                    if matches.len() == limit {
                        return Ok(SearchFilesResult {
                            skipped,
                            matches,
                            truncated: true,
                        });
                    }
                    let item = SearchMatch {
                        path: path.clone(),
                        line: index + 1,
                        text: cap_line_preview(line),
                    };
                    result_bytes = result_bytes.saturating_add(
                        usize::from(!matches.is_empty())
                            + serde_json::to_vec(&item).map_err(serialize_error)?.len(),
                    );
                    if result_bytes > self.limits.max_result_bytes {
                        return Err(ToolError::Sdk {
                            sdk_kind: "response_too_large".into(),
                            message: "state search result exceeded max result bytes".into(),
                        });
                    }
                    matches.push(item);
                }
            }
        }
        Ok(SearchFilesResult {
            skipped,
            matches,
            truncated: glob.truncated,
        })
    }

    async fn walk_files(&self, limit: usize) -> Result<WalkFilesResult, ToolError> {
        let mut files = Vec::new();
        let mut truncated = false;
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let mut read_dir = match tokio::fs::read_dir(&dir).await {
                Ok(read_dir) => read_dir,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(internal_io("walk state workspace")(error)),
            };
            while let Some(entry) = read_dir
                .next_entry()
                .await
                .map_err(internal_io("walk state workspace entry"))?
            {
                let path = entry.path();
                let relative = match path.strip_prefix(&self.root) {
                    Ok(relative) => relative,
                    Err(_) => continue,
                };
                let virtual_path = labby_runtime::path_safety::rel_to_unix_string(relative);
                if is_reserved_metadata_path(&virtual_path) {
                    continue;
                }
                let metadata = tokio::fs::symlink_metadata(&path)
                    .await
                    .map_err(internal_io("read state workspace metadata"))?;
                if metadata.file_type().is_symlink() {
                    continue;
                }
                if metadata.is_dir() {
                    stack.push(path);
                } else if metadata.is_file() {
                    files.push(virtual_path);
                    if files.len() > limit {
                        truncated = true;
                        break;
                    }
                }
            }
            if truncated {
                break;
            }
        }
        files.sort();
        if truncated {
            files.truncate(limit);
        }
        Ok(WalkFilesResult { files, truncated })
    }
}
fn glob_pattern_regex(pattern: &str) -> Result<Regex, ToolError> {
    if pattern.trim().is_empty() {
        return Err(ToolError::InvalidParam {
            message: "state glob pattern must not be empty".to_string(),
            param: "pattern".to_string(),
        });
    }
    if pattern.contains("..") || pattern.starts_with('/') || pattern.contains(':') {
        return Err(ToolError::Sdk {
            sdk_kind: "path_traversal".to_string(),
            message: "state glob pattern must stay inside the workspace".to_string(),
        });
    }
    let mut regex = String::from("^");
    let chars = pattern.chars().collect::<Vec<_>>();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') && chars.get(i + 2) == Some(&'/') => {
                regex.push_str("(?:.*/)?");
                i += 3;
            }
            '*' if chars.get(i + 1) == Some(&'*') => {
                regex.push_str(".*");
                i += 2;
            }
            '*' => {
                regex.push_str("[^/]*");
                i += 1;
            }
            '?' => {
                regex.push_str("[^/]");
                i += 1;
            }
            ch => {
                regex.push_str(&regex::escape(&ch.to_string()));
                i += 1;
            }
        }
    }
    regex.push('$');
    Regex::new(&regex).map_err(|err| ToolError::InvalidParam {
        message: format!("invalid state glob pattern: {err}"),
        param: "pattern".to_string(),
    })
}

fn cap_line_preview(line: &str) -> String {
    line.chars().take(512).collect()
}
