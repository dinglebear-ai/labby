use super::*;

impl StateWorkspace {
    pub(crate) async fn replace_in_files(
        &self,
        pattern: &str,
        search: &str,
        replace: &str,
        dry_run: bool,
    ) -> Result<ReplaceInFilesResult, ToolError> {
        if search.is_empty() {
            return Err(ToolError::InvalidParam {
                message: "state replace search must not be empty".to_string(),
                param: "search".to_string(),
            });
        }
        let glob = self.glob(pattern, self.limits.max_entries as usize).await?;
        if glob.truncated {
            return Err(ToolError::Sdk {
                sdk_kind: "response_too_large".to_string(),
                message: "state replace input exceeded max entries".to_string(),
            });
        }
        let mut changed_paths = Vec::new();
        let mut skipped = 0;
        for path in glob.matches {
            let Some((virtual_path, content)) = self.read_glob_text(&path).await? else {
                skipped += 1;
                continue;
            };
            if !content.contains(search) {
                continue;
            }
            changed_paths.push(virtual_path);
        }
        let changed = changed_paths
            .iter()
            .map(|path| path.as_str().to_string())
            .collect::<Vec<_>>();
        if dry_run {
            return Ok(ReplaceInFilesResult {
                skipped,
                changed,
                dry_run,
            });
        }

        let mut originals = Vec::new();
        let mut usage = None;
        for path in changed_paths {
            let file = match self.read_file(&path).await {
                Ok(file) => file,
                Err(err) => {
                    return Err(self.restore_originals_after_failure(&originals, err).await);
                }
            };
            if !file.content.contains(search) {
                return Err(self
                    .restore_originals_after_failure(
                        &originals,
                        ToolError::Sdk {
                            sdk_kind: "edit_conflict".to_string(),
                            message: format!(
                                "state replace input no longer matches `{}`",
                                path.as_str()
                            ),
                        },
                    )
                    .await);
            }
            let original = file.content;
            let next = original.replace(search, replace);
            if let Err(err) = self.write_with_usage(&path, &next, &mut usage).await {
                return Err(self.restore_originals_after_failure(&originals, err).await);
            }
            originals.push((path, original));
        }
        // Reconcile changes made outside this process before reporting success.
        if usage.is_some()
            && let Err(err) = self.enforce_total_limits().await
        {
            return Err(self.restore_originals_after_failure(&originals, err).await);
        }
        Ok(ReplaceInFilesResult {
            skipped,
            changed,
            dry_run,
        })
    }

    pub(crate) async fn plan_edits(
        &self,
        edits: Vec<FileEdit>,
    ) -> Result<EditPlanResult, ToolError> {
        let edits = normalize_edits(edits)?;
        let canonical = serde_json::to_vec(&edits).map_err(serialize_error)?;
        let plan_id = hex::encode(Sha256::digest(&canonical));
        let plan_path = self.plan_path(&plan_id);
        if let Some(parent) = plan_path.parent() {
            self.check_entry_quota_for_path(parent).await?;
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(internal_io("create state edit plan directory"))?;
        }
        self.check_entry_quota_for_path(&plan_path).await?;
        tokio::fs::write(&plan_path, canonical)
            .await
            .map_err(internal_io("write state edit plan"))?;
        if let Err(err) = self.enforce_total_limits().await {
            cleanup_file_after_quota_error(&plan_path, err, "state edit plan").await?;
        }
        Ok(EditPlanResult { plan_id, edits })
    }

    pub(crate) async fn apply_edit_plan(
        &self,
        plan_id: &str,
    ) -> Result<ApplyEditPlanResult, ToolError> {
        validate_plan_id(plan_id)?;
        let plan_path = self.plan_path(plan_id);
        let plan = tokio::fs::read(&plan_path)
            .await
            .map_err(not_found_or_internal("read state edit plan"))?;
        let edits: Vec<FileEdit> = serde_json::from_slice(&plan).map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to parse state edit plan: {err}"),
        })?;

        let mut planned = Vec::<(VirtualPath, String, String)>::new();
        let mut indices = std::collections::HashMap::new();
        for edit in edits {
            let path = VirtualPath::parse(&edit.path)?;
            let index = match indices.entry(path.clone()) {
                std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let original = self.read_file(&path).await?.content;
                    let index = planned.len();
                    planned.push((path.clone(), original.clone(), original));
                    entry.insert(index);
                    index
                }
            };
            let next = &mut planned[index].2;
            if !next.contains(&edit.search) {
                return Err(ToolError::Sdk {
                    sdk_kind: "edit_conflict".to_string(),
                    message: format!("state edit plan no longer matches `{}`", path.as_str()),
                });
            }
            *next = next.replace(&edit.search, &edit.replace);
        }

        let mut changed = Vec::new();
        let mut originals = Vec::new();
        let mut usage = None;
        for (path, original, next) in planned {
            if let Err(err) = self.write_with_usage(&path, &next, &mut usage).await {
                return Err(self.restore_originals_after_failure(&originals, err).await);
            }
            originals.push((path.clone(), original));
            changed.push(path.as_str().to_string());
        }

        // Reconcile changes made outside this process before reporting success.
        if usage.is_some()
            && let Err(err) = self.enforce_total_limits().await
        {
            return Err(self.restore_originals_after_failure(&originals, err).await);
        }
        Ok(ApplyEditPlanResult { ok: true, changed })
    }

    async fn restore_originals_after_failure(
        &self,
        originals: &[(VirtualPath, String)],
        original_error: ToolError,
    ) -> ToolError {
        for (path, content) in originals.iter().rev() {
            if let Err(rollback_error) = self.write_file(path, content).await {
                return ToolError::Sdk {
                    sdk_kind: "rollback_failed".to_string(),
                    message: format!(
                        "state batch mutation failed with `{}` and rollback of `{}` failed with `{}`",
                        original_error.kind(),
                        path.as_str(),
                        rollback_error.kind()
                    ),
                };
            }
        }
        original_error
    }

    fn plan_path(&self, plan_id: &str) -> PathBuf {
        self.root
            .join(".labby-state")
            .join("plans")
            .join(format!("{plan_id}.json"))
    }
}
fn normalize_edits(edits: Vec<FileEdit>) -> Result<Vec<FileEdit>, ToolError> {
    if edits.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "state edit plan must include at least one edit".to_string(),
            param: "edits".to_string(),
        });
    }
    edits
        .into_iter()
        .map(|edit| {
            if edit.search.is_empty() {
                return Err(ToolError::InvalidParam {
                    message: "state edit search must not be empty".to_string(),
                    param: "search".to_string(),
                });
            }
            let path = VirtualPath::parse(&edit.path)?.as_str().to_string();
            Ok(FileEdit {
                path,
                search: edit.search,
                replace: edit.replace,
            })
        })
        .collect()
}

fn validate_plan_id(plan_id: &str) -> Result<(), ToolError> {
    let valid = plan_id.len() == 64 && plan_id.chars().all(|ch| ch.is_ascii_hexdigit());
    if !valid {
        return Err(ToolError::InvalidParam {
            message: "state edit plan id must be a sha256 hex string".to_string(),
            param: "planId".to_string(),
        });
    }
    Ok(())
}
